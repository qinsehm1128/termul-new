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
    /// Connection generation this viewer's attach/watch succeeded on. Host
    /// authorization is per-connection, so a mismatch means this PTY is no
    /// longer authorized for `write`/`set_display_mode` until it re-attaches.
    private(set) var attachedGeneration: Int?
    /// Host sequence cursor for this PTY's stream. Recovery resumes replay
    /// here instead of seq 0, so a reconnect does not repaint the backlog.
    private(set) var lastSeq: Int = 0
    /// Set when the host refuses the phone fit; the canvas then mirrors the
    /// host grid instead of pushing viewport dimensions.
    private(set) var locksToHostCols = false
    /// Grid the attach reply was authored at. A quick terminal usually starts
    /// at the host grid, so the first canvas fit has to treat the replay as a
    /// takeover instead of painting it as-is.
    private(set) var replayGrid: TerminalReplayGrid?
    var errorMessage: String?

    @ObservationIgnored private let socket: TerminalSocket
    @ObservationIgnored private let ensureConnected: (@MainActor () async throws -> Void)?
    @ObservationIgnored private let claimProvider: (@MainActor () -> String?)?
    /// Bytes that arrived before the SwiftTerm canvas installed its feed.
    @ObservationIgnored private var backlog = Data()
    /// The backlog starts mid-stream; the canvas must reset before it.
    @ObservationIgnored private var backlogNeedsReset = false
    @ObservationIgnored private var feed: (@MainActor (Data, Bool) -> Void)?
    @ObservationIgnored private var refitTask: Task<Void, Never>?
    @ObservationIgnored private var pendingSend = ""
    @ObservationIgnored private var isDrainingSend = false

    /// A viewer that is installed late (or never) must not grow unbounded.
    private static let backlogLimit = 262_144

    init(
        terminalId: String,
        title: String,
        socket: TerminalSocket,
        cols: Int,
        rows: Int,
        ensureConnected: (@MainActor () async throws -> Void)? = nil,
        claimProvider: (@MainActor () -> String?)? = nil
    ) {
        self.terminalId = terminalId
        self.title = title
        self.socket = socket
        self.cols = max(cols, 20)
        self.rows = max(rows, 4)
        self.ensureConnected = ensureConnected
        self.claimProvider = claimProvider
    }

    /// True when this viewer's attach/watch is still authorized on the live
    /// socket connection. A reconnect mints a new `connectionGeneration` with
    /// an empty server-side authorization map, so this is the gate every
    /// outbound request must pass.
    var isLiveOnCurrentConnection: Bool {
        isAttached && !isClosed && !hasExited
            && socket.isConnected && attachedGeneration == socket.connectionGeneration
    }

    /// Attach the existing host PTY. `claim` is nil for a PTY this device did
    /// not start, and the host mints no replacement, so `watch` is the only
    /// remaining route (it fails when Terminal Core owns the shell).
    func attach(claim: String?) async throws {
        let reply: TerminalAttachResult
        if let claim, !claim.isEmpty {
            reply = try await socket.attach(terminalId: terminalId, claim: claim, lastSeq: lastSeq)
        } else {
            reply = try await socket.watch(terminalId: terminalId, lastSeq: lastSeq)
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
        if let seq = reply.latestSeq {
            lastSeq = max(lastSeq, seq)
        }
        isAttached = true
        attachedGeneration = socket.connectionGeneration
        errorMessage = nil
        HostLog.session.info("Attached standalone host PTY")
    }

    /// Re-authorize this PTY on a fresh terminal-socket connection. The host
    /// keeps no per-viewer authorization across connections, so after a
    /// reconnect every `write`/`set_display_mode` would fail `UNAUTHORIZED`
    /// until this runs. Replay resumes at the tracked cursor, not seq 0.
    func reattachAfterReconnect(claim: String?) async {
        guard !isClosed, !hasExited, !isLiveOnCurrentConnection else { return }
        do {
            if let claim, !claim.isEmpty {
                try await attach(claim: claim)
            } else {
                let reply = try await socket.watch(terminalId: terminalId, lastSeq: lastSeq)
                guard !isClosed else {
                    await socket.detach(terminalId: terminalId)
                    return
                }
                if let replyCols = reply.cols, replyCols > 1 { cols = replyCols }
                if let replyRows = reply.rows, replyRows > 1 { rows = replyRows }
                if let seq = reply.latestSeq {
                    lastSeq = max(lastSeq, seq)
                }
                isAttached = true
                attachedGeneration = socket.connectionGeneration
                errorMessage = nil
                HostLog.session.info("Re-watched standalone host PTY after reconnect")
            }
        } catch {
            guard !isClosed else { return }
            errorMessage = error.localizedDescription
            HostLog.session.error("Standalone PTY re-authorization failed after reconnect")
        }
    }

    /// Host output for this PTY, already coalesced by `TerminalStore`.
    /// `reset` marks a gapped replay: the stream starts mid-sequence, so the
    /// pre-gap backlog is discarded and the canvas is told to reset first.
    /// `latestSeq` is the host cursor this frame was authored at, recorded so
    /// a later reconnect resumes replay exactly here.
    func receive(_ data: Data, reset: Bool = false, latestSeq: Int? = nil) {
        guard !isClosed, !hasExited else { return }
        if let latestSeq {
            lastSeq = max(lastSeq, latestSeq)
        }
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
        guard !isClosed, !hasExited, !text.isEmpty else { return }
        pendingSend.append(text)
        guard !isDrainingSend else { return }
        isDrainingSend = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.isDrainingSend = false }
            await Task.yield()
            while !self.isClosed, !self.hasExited, !self.pendingSend.isEmpty {
                let chunk = self.pendingSend
                self.pendingSend.removeAll(keepingCapacity: true)
                do {
                    try await self.writeChunkWithRecovery(chunk)
                    self.errorMessage = nil
                } catch {
                    self.errorMessage = error.localizedDescription
                    self.pendingSend.removeAll(keepingCapacity: false)
                    return
                }
            }
        }
    }

    private func writeChunkWithRecovery(_ chunk: String) async throws {
        if !socket.isConnected {
            try await ensureConnected?()
        }
        if !isLiveOnCurrentConnection {
            await reattachAfterReconnect(claim: claimProvider?())
        }
        let generation = socket.connectionGeneration
        do {
            try await socket.write(terminalId: terminalId, data: chunk)
        } catch {
            guard TerminalStore.isRecoverableWriteFailure(error), let ensureConnected else {
                throw error
            }
            HostLog.session.info("Reconnecting standalone viewer socket to retry write")
            if socket.isConnected, socket.connectionGeneration == generation {
                socket.stop()
            }
            try await ensureConnected()
            await reattachAfterReconnect(claim: claimProvider?())
            try await socket.write(terminalId: terminalId, data: chunk)
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
        // A reconnect mints a fresh connection with an empty server-side
        // authorization map: pushing a fit before re-attach is answered with
        // UNAUTHORIZED and would strand the "Un…" banner. Recovery re-attaches
        // first; the armed refit then lands on an authorized socket.
        guard isLiveOnCurrentConnection else { return }
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
            errorMessage = nil
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
        pendingSend.removeAll(keepingCapacity: false)
        refitTask?.cancel()
        refitTask = nil
    }

    /// Release the display-mode lease, then the view lease. Never kills the
    /// PTY: the shell keeps running on the host.
    func close() async {
        guard !isClosed else { return }
        isClosed = true
        pendingSend.removeAll(keepingCapacity: false)
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
