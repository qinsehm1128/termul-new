import Foundation
import Observation

/// A standalone live attachment to a host PTY that the workbench does not list.
///
/// Quick terminals are workspace-scoped: they belong to no conversation or
/// project catalog, so they must not enter `TerminalStore.terminals` (that list
/// is refreshed from the catalog, which would evict them, and its single
/// `activeId` owns the workbench's geometry lease). The viewer shares the
/// session's one `TerminalSocket` and attaches with the claim `open` returned —
/// `attach` is the operation that can reach a Terminal Core PTY, while `watch`
/// reads the in-process manager. It keeps its own replay backlog and phone-fit
/// lease, and `close` only detaches: the host shell keeps running.
@MainActor
@Observable
final class TerminalPTYViewer {
    let terminalId: String
    let title: String

    private(set) var cols: Int
    private(set) var rows: Int
    private(set) var isAttached = false
    private(set) var isClosed = false
    private(set) var hasExited = false
    /// Set when the host refuses the phone fit; the canvas then mirrors the
    /// host grid instead of pushing viewport dimensions.
    private(set) var locksToHostCols = false
    /// Grid the attach reply was authored at. A quick terminal usually starts
    /// at the host grid, so the first canvas fit has to treat the replay as a
    /// takeover instead of painting it as-is.
    private(set) var replayGrid: TerminalReplayGrid?
    var errorMessage: String?

    @ObservationIgnored private let socket: TerminalSocket
    /// Bytes that arrived before the SwiftTerm canvas installed its feed.
    @ObservationIgnored private var backlog = Data()
    /// The backlog starts mid-stream; the canvas must reset before it.
    @ObservationIgnored private var backlogNeedsReset = false
    @ObservationIgnored private var feed: (@MainActor (Data, Bool) -> Void)?
    @ObservationIgnored private var refitTask: Task<Void, Never>?

    /// A viewer that is installed late (or never) must not grow unbounded.
    private static let backlogLimit = 262_144

    init(terminalId: String, title: String, socket: TerminalSocket, cols: Int, rows: Int) {
        self.terminalId = terminalId
        self.title = title
        self.socket = socket
        self.cols = max(cols, 20)
        self.rows = max(rows, 4)
    }

    /// Attach the existing host PTY. `claim` is nil for a PTY this device did
    /// not start, and the host mints no replacement, so `watch` is the only
    /// remaining route (it fails when Terminal Core owns the shell).
    func attach(claim: String?) async throws {
        let reply: TerminalAttachResult
        if let claim, !claim.isEmpty {
            reply = try await socket.attach(terminalId: terminalId, claim: claim, lastSeq: 0)
        } else {
            reply = try await socket.watch(terminalId: terminalId, lastSeq: 0)
        }
        guard !isClosed else {
            // The cover closed while the request was in flight: the host now
            // holds a view lease this viewer will never release.
            await socket.detach(terminalId: terminalId)
            return
        }
        if let replyCols = reply.cols, replyCols > 1 {
            cols = replyCols
        }
        if let replyRows = reply.rows, replyRows > 1 {
            rows = replyRows
        }
        replayGrid = TerminalReplayGrid(cols: reply.cols, rows: reply.rows)
        isAttached = true
        HostLog.session.info("Attached standalone host PTY")
    }

    /// Host output for this PTY, already coalesced by `TerminalStore`.
    /// `reset` marks a gapped replay: the stream starts mid-sequence, so the
    /// pre-gap backlog is discarded and the canvas is told to reset first.
    func receive(_ data: Data, reset: Bool = false) {
        guard !isClosed, !hasExited else { return }
        if let feed {
            feed(data, reset)
            return
        }
        if reset {
            backlog.removeAll(keepingCapacity: true)
            backlogNeedsReset = true
        }
        backlog.append(data)
        if backlog.count > Self.backlogLimit {
            backlog.removeFirst(backlog.count - Self.backlogLimit)
        }
    }

    /// Install the mounted canvas as the destination for host output and flush
    /// whatever arrived while it was being created. The reset flag travels
    /// with the backlog so the canvas can clear before painting a gapped
    /// replay; if the reset arrived without bytes it is still delivered.
    func install(_ feed: @escaping @MainActor (Data, Bool) -> Void) {
        guard !isClosed, !hasExited else { return }
        self.feed = feed
        let needsReset = backlogNeedsReset
        backlogNeedsReset = false
        let pending = backlog
        backlog.removeAll(keepingCapacity: false)
        if pending.isEmpty && !needsReset { return }
        feed(pending, needsReset)
    }

    func send(_ text: String) {
        guard !isClosed, !text.isEmpty else { return }
        Task {
            do {
                try await socket.write(terminalId: terminalId, data: text)
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    /// Debounced phone-fit takeover for this terminal alone. The workbench's
    /// `geometryActive`/`activeId` lease stays untouched.
    func resize(cols: Int, rows: Int) {
        guard isAttached, !isClosed, !locksToHostCols,
              cols > 1, rows > 1, cols != self.cols || rows != self.rows else { return }
        refitTask?.cancel()
        refitTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(150))
            guard let self, !Task.isCancelled else { return }
            await self.pushPhoneFit(cols: cols, rows: rows)
        }
    }

    private func pushPhoneFit(cols: Int, rows: Int) async {
        do {
            let state = try await socket.setDisplayMode(
                terminalId: terminalId,
                mode: TerminalDisplayMode.phone.rawValue,
                cols: cols,
                rows: rows
            )
            guard !isClosed else {
                _ = try? await socket.setDisplayMode(
                    terminalId: terminalId,
                    mode: TerminalDisplayMode.desktop.rawValue
                )
                return
            }
            if state.cols > 1, state.rows > 1 {
                self.cols = state.cols
                self.rows = state.rows
            }
        } catch {
            guard !isClosed else { return }
            // Another viewer owns the phone lease; mirror the host grid.
            locksToHostCols = true
            HostLog.session.info("Standalone phone fit refused; mirroring host grid")
        }
    }

    /// The host reported the shell exited; keep the canvas readable.
    func markExited() {
        hasExited = true
        feed = nil
        refitTask?.cancel()
        refitTask = nil
    }

    /// Release the display-mode lease, then the view lease. Never kills the
    /// PTY: the shell keeps running on the host.
    func close() async {
        guard !isClosed else { return }
        isClosed = true
        refitTask?.cancel()
        refitTask = nil
        feed = nil
        backlog.removeAll(keepingCapacity: false)
        guard isAttached, !hasExited else { return }
        if !locksToHostCols {
            do {
                _ = try await socket.setDisplayMode(
                    terminalId: terminalId,
                    mode: TerminalDisplayMode.desktop.rawValue
                )
            } catch {
                HostLog.session.info("Standalone display-mode release failed")
            }
        }
        await socket.detach(terminalId: terminalId)
        HostLog.session.info("Detached standalone host PTY")
    }
}
