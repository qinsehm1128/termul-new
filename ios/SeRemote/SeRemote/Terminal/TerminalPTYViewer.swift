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
    var errorMessage: String?

    @ObservationIgnored private let socket: TerminalSocket
    /// Bytes that arrived before the SwiftTerm canvas installed its feed.
    @ObservationIgnored private var backlog = Data()
    @ObservationIgnored private var feed: ((Data) -> Void)?
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
        isAttached = true
        HostLog.session.info("Attached standalone host PTY")
    }

    /// Host output for this PTY, already coalesced by `TerminalStore`.
    func receive(_ data: Data) {
        guard !isClosed, !hasExited else { return }
        if let feed {
            feed(data)
            return
        }
        backlog.append(data)
        if backlog.count > Self.backlogLimit {
            backlog.removeFirst(backlog.count - Self.backlogLimit)
        }
    }

    /// Install the mounted canvas as the destination for host output and flush
    /// whatever arrived while it was being created.
    func install(_ feed: @escaping (Data) -> Void) {
        guard !isClosed, !hasExited else { return }
        self.feed = feed
        guard !backlog.isEmpty else { return }
        let pending = backlog
        backlog.removeAll(keepingCapacity: false)
        feed(pending)
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
