import Foundation
import Observation

enum TerminalDisplayMode: String {
    case phone
    case desktop
}

struct LiveTerminal: Identifiable, Hashable, Sendable {
    let id: String
    var claim: String?
    var lastSeq: Int
    var cols: Int
    var rows: Int
    var title: String
    var cwd: String?
    var gitBranch: String?
    /// True only for PTYs this phone spawned.
    var owned: Bool
    /// Catalog binding carried by the host row. Quick terminals never enter
    /// this list, so a standalone surface always leaves these nil.
    var projectId: String? = nil
    var conversationId: String? = nil
    var terminalScreenEngine: HostTerminalScreenEngine? = nil
    var screenObservation: HostScreenObservation = .legacyHost
    /// Grid the watch reply that delivered this terminal's replay was authored
    /// at. It stays at the takeover-time value while `cols`/`rows` follow the
    /// live PTY, so the canvas can tell history from live layout.
    var replayGrid: TerminalReplayGrid? = nil
}

@MainActor
@Observable
final class TerminalStore {
    var terminals: [LiveTerminal] = []
    var activeId: String?
    /// Bytes that arrived while no canvas feed was installed *for that
    /// terminal*. Keyed by PTY id, so output for a tab whose canvas has not
    /// remounted yet is never swallowed by the feed of the tab still on screen.
    private(set) var pendingOutput: [String: Data] = [:]
    /// Terminals whose buffered stream starts mid-sequence because the host
    /// could not replay every byte. The canvas must reset before consuming it.
    private var pendingReset: Set<String> = []
    /// Terminal whose canvas owns `onFeed`. A feed installed for A must not
    /// swallow B's bytes: those wait in `pendingOutput` until B installs.
    private var feedTerminalId: String?
    /// One terminal's queued output must not grow without bound while its
    /// canvas is away (a tab switch, a cover, a slow remount). Oldest bytes are
    /// dropped first; a reset flag then marks the truncated stream.
    static let pendingOutputLimit = 4 * 1024 * 1024
    var errorMessage: String?
    var isConnecting = false
    var isRefreshingScreenObservation = false
    /// Receives `(data, needsReset)` for the terminal named by
    /// `feedTerminalId`; bytes for any other terminal are buffered instead.
    var onFeed: (@MainActor (String, Data, Bool) -> Void)?
    /// Phone-fit takeover when viewing; desktop restores the parked host size.
    var displayMode: TerminalDisplayMode = .phone
    /// True only while the terminal tab is the visible workspace surface.
    /// Covered surfaces must never leak phone dims into the host PTY.
    var geometryActive = false

    /// All viewport changes funnel through one 150 ms debounce (keyboard,
    /// rotation, text scale, tab return, resume, reconnect). The commit reads
    /// the latest grid, so an oscillation inside the window collapses to the
    /// final size and the host never sees a mid-keystroke reflow.
    private var refitTask: Task<Void, Never>?
    /// Forced refits re-assert the viewport even when dims are unchanged —
    /// resume/reconnect converge when the host PTY changed behind our back.
    /// Cleared only by a successful push, so a failed/raced attempt re-arms.
    private var forceRefit = false
    /// Monotonic token bumped by every schedule/release/open/mode switch.
    /// A commit that crosses an await validates against it afterwards: a
    /// leave+return ABA or a terminal switch cannot resurrect a stale lease.
    private var refitEpoch = 0
    /// The grid the live view last fitted for `fittedTerminalId`. This is the
    /// ONLY source commitRefit pushes — catalog rows, host replies, and
    /// display-mode events describe host-side truth and must never
    /// masquerade as the phone viewport.
    private var lastFitted: (cols: Int, rows: Int)?
    private var fittedTerminalId: String?
    private var lastPushed: [String: (cols: Int, rows: Int)] = [:]

    private var coalesceBuffers: [String: Data] = [:]
    private var coalesceGapped: Set<String> = []
    /// Max host cursor seen per terminal inside the coalescing window.
    private var coalesceSeq: [String: Int] = [:]
    private var coalesceTask: Task<Void, Never>?
    private var socket: TerminalSocket?
    private var origin: URL?
    private var credentials: HostCredentials?
    private var watchedId: String?
    /// Connection generation that took the watch. Watches are per-connection,
    /// so `watchedId` alone cannot prove the host still replays to us.
    private var watchedGeneration: Int?
    /// Last known host output cursor per terminal, kept outside the `terminals`
    /// catalog so a mid-refresh catalog (or an evicted row) cannot lose the
    /// resume point a reconnect needs.
    private var watchedSeq: [String: Int] = [:]
    /// Serializes `recoverAfterReconnect` so the onConnected hook, foreground
    /// revalidation and a user-driven open coalesce into one re-authorization.
    /// `Task` is a value type, so an incrementing serial — not identity —
    /// decides which run may clear the in-flight slot.
    private var recoveryTask: Task<Void, Never>?
    private var recoverySerial = 0
    /// Ordered outbound input buffers per terminal so rapid typing, paste, or
    /// mouse-wheel bursts collapse into serialized WebSocket `write` frames
    /// instead of flooding concurrent RPCs.
    @ObservationIgnored private var pendingWrites: [String: String] = [:]
    @ObservationIgnored private var activeWriteDrains: Set<String> = []
    @ObservationIgnored private var backgroundReconnectTask: Task<Void, Never>?
    private var lastConversationId: String?
    private var lastProjectId: String?

    /// Standalone viewers (quick terminals) keyed by PTY id. Held weakly so a
    /// dismissed console cannot keep routing bytes into a dead canvas.
    @ObservationIgnored private var viewers: [String: TerminalViewerBox] = [:]
    /// Claims this device holds for standalone PTYs. The host mints a claim
    /// only when it spawns the shell, so a later open that reuses it returns
    /// none; remembering it keeps reopen working for this app run.
    @ObservationIgnored private var standaloneClaims: [String: String] = [:]

    /// Side channel for a local notification when a watched PTY exits.
    var onTerminalExit: ((String) -> Void)?

    func attach(socket: TerminalSocket, origin: URL, credentials: HostCredentials) {
        self.socket = socket
        self.origin = origin
        self.credentials = credentials
        // `handleFrame` fires BOTH `onOutputFrame` and `onBytes` for one
        // frame (the tuple API stays for compatibility). Subscribing to both
        // here would deliver every frame twice, so this store reads only the
        // richer `onOutputFrame` — same bytes plus the host cursor.
        socket.onOutputFrame = { [weak self] frame in
            guard let self else { return }
            // Keep the true host cursor so a reconnect resumes replay here
            // instead of seq 0 (which would repaint the whole scrollback).
            if let latestSeq = frame.latestSeq {
                self.watchedSeq[frame.terminalId] = max(self.watchedSeq[frame.terminalId] ?? 0, latestSeq)
            }
            self.enqueueOutput(terminalId: frame.terminalId, data: frame.data, gap: frame.gap, latestSeq: frame.latestSeq)
        }
        socket.onConnected = { [weak self] _ in
            guard let self else { return }
            Task { await self.recoverAfterReconnect() }
        }
        socket.onDisconnected = { [weak self] in
            guard let self else { return }
            self.scheduleProactiveReconnectIfNeeded()
        }
        socket.onCatalogChanged = { [weak self] in
            guard let self else { return }
            Task { await self.refresh(conversationId: self.lastConversationId, projectId: self.lastProjectId) }
        }
        socket.onDisplayModeChanged = { [weak self] terminalId, mode, cols, rows in
            guard let self else { return }
            // The event carries the authoritative grid (the parked desktop
            // size on restore), so the model — and the desktop-mode column
            // fit derived from it — stays truthful.
            if cols > 1, rows > 1,
               let index = terminals.firstIndex(where: { $0.id == terminalId }) {
                terminals[index].cols = cols
                terminals[index].rows = rows
            }
            // While a forced phone takeover is pending, a late echo of our
            // own tab-leave release must not cancel it.
            guard !forceRefit else { return }
            guard terminalId == activeId,
                  mode == TerminalDisplayMode.desktop.rawValue,
                  geometryActive,
                  displayMode == .phone
            else { return }
            refitEpoch &+= 1
            displayMode = .desktop
            HostLog.session.info("Host restored desktop display mode")
        }
        socket.onExit = { [weak self] terminalId in
            guard let self else { return }
            self.onTerminalExit?(terminalId)
            self.terminals.removeAll { $0.id == terminalId }
            self.pendingOutput.removeValue(forKey: terminalId)
            self.pendingReset.remove(terminalId)
            self.standaloneClaims.removeValue(forKey: terminalId)
            self.watchedSeq.removeValue(forKey: terminalId)
            self.lastPushed.removeValue(forKey: terminalId)
            self.pendingWrites.removeValue(forKey: terminalId)
            if let viewer = self.viewers[terminalId]?.viewer {
                viewer.markExited()
            }
            if self.watchedId == terminalId {
                self.watchedId = nil
                self.watchedGeneration = nil
            }
            if self.activeId == terminalId {
                self.activeId = self.terminals.first?.id
                if let next = self.activeId {
                    Task { await self.open(next) }
                }
            }
        }
    }

    func ensureConnected() async throws {
        guard let socket, let origin, let credentials else {
            throw HostError.unexpected(String(localized: "Not connected."))
        }
        if socket.isConnected { return }
        try await socket.connect(origin: origin, credentials: credentials)
    }

    /// Attach an already-running host PTY (a quick terminal) as a standalone
    /// viewer. No spawn, no catalog entry, no active tab, and no change to the
    /// workbench geometry lease: closing the console only detaches.
    func attachViewer(
        terminalId: String,
        claim: String?,
        title: String,
        cols: Int,
        rows: Int
    ) async throws -> TerminalPTYViewer {
        try await ensureConnected()
        guard let socket else {
            throw HostError.network(String(localized: "Disconnected from the host."))
        }
        if let existing = viewers[terminalId]?.viewer, !existing.isClosed {
            // The socket may have reconnected since this viewer attached: the
            // host's authorization map is empty on the new connection, so
            // re-authorize before handing the same viewer back.
            if !existing.isLiveOnCurrentConnection {
                await existing.reattachAfterReconnect(claim: claim ?? standaloneClaims[terminalId])
            }
            return existing
        }
        let resolved = claim ?? standaloneClaims[terminalId]
        let viewer = TerminalPTYViewer(
            terminalId: terminalId,
            title: title,
            socket: socket,
            cols: cols,
            rows: rows,
            ensureConnected: { [weak self] in
                try await self?.ensureConnected()
            },
            claimProvider: { [weak self] in
                resolved ?? self?.standaloneClaims[terminalId]
            }
        )
        let box = TerminalViewerBox(viewer: viewer)
        viewers[terminalId] = box
        do {
            try await viewer.attach(claim: resolved)
        } catch {
            if viewers[terminalId] === box {
                viewers.removeValue(forKey: terminalId)
            }
            throw error
        }
        if let resolved {
            rememberClaim(resolved, for: terminalId)
        }
        return viewer
    }

    /// Drop the viewer's routing and view lease. The host PTY is not killed.
    func releaseViewer(_ viewer: TerminalPTYViewer) async {
        let terminalId = viewer.terminalId
        await viewer.close()
        if viewers[terminalId]?.viewer === viewer {
            viewers.removeValue(forKey: terminalId)
        }
    }

    /// Drop whatever standalone viewer this PTY still holds. A failed open must
    /// not leave a host view lease behind for the rest of the app run.
    func releaseViewer(for terminalId: String) async {
        guard let box = viewers[terminalId] else { return }
        guard let viewer = box.viewer else {
            viewers.removeValue(forKey: terminalId)
            return
        }
        await releaseViewer(viewer)
    }

    /// Claims are one-per-spawn. Keeping the ones this device received lets a
    /// later open that reuses the same live PTY attach instead of stranding it.
    private func rememberClaim(_ claim: String, for terminalId: String) {
        standaloneClaims[terminalId] = claim
        while standaloneClaims.count > 16, let oldest = standaloneClaims.keys.first {
            standaloneClaims.removeValue(forKey: oldest)
        }
    }

    func refresh(conversationId: String?, projectId: String?) async {
        lastConversationId = conversationId
        lastProjectId = projectId
        isConnecting = true
        defer { isConnecting = false }
        do {
            try await ensureConnected()
            guard let socket else { return }
            let listed = try await socket.list(conversationId: conversationId, projectId: projectId)
            let ownedById = Dictionary(uniqueKeysWithValues: terminals.filter(\.owned).map { ($0.id, $0) })
            let previousActive = activeId
            terminals = Self.liveTerminals(from: listed, ownedById: ownedById)
            if let previousActive, terminals.contains(where: { $0.id == previousActive }) {
                activeId = previousActive
            } else {
                activeId = terminals.first?.id
            }
            errorMessage = nil
            if let activeId, !isWatching(activeId) {
                await open(activeId)
            }
        } catch {
            HostLog.session.error("Terminal catalog refresh failed")
            errorMessage = error.localizedDescription
        }
    }

    /// This is a user action, never part of watching, fitting or polling a PTY.
    func refreshScreenObservation() async {
        guard !isRefreshingScreenObservation, let terminalId = activeId, let socket else { return }
        isRefreshingScreenObservation = true
        defer { isRefreshingScreenObservation = false }
        let observation: HostScreenObservation
        do {
            observation = try await socket.screenStatus(terminalId: terminalId)
        } catch {
            // A legacy host may reject the operation. Never retain a previous
            // ready label after a failed query or infer it from the preference.
            observation = .unavailable(
                reason: .protocol,
                engine: terminals.first(where: { $0.id == terminalId })?.terminalScreenEngine ?? .existing
            )
        }
        guard self.socket === socket,
              let index = terminals.firstIndex(where: { $0.id == terminalId }) else { return }
        terminals[index].screenObservation = observation
    }

    /// True when this PTY is already the watched terminal on the live
    /// connection. A second watch would ask the host to replay from seq 0 into
    /// a canvas that already shows that output.
    func isWatching(_ terminalId: String) -> Bool {
        guard let socket, socket.isConnected else { return false }
        return watchedId == terminalId && watchedGeneration == socket.connectionGeneration
    }

    /// Best-known host cursor for one terminal. The side-channel map wins: the
    /// catalog may be mid-refresh or have evicted the row, and a reconnect
    /// must resume from the real cursor regardless.
    func lastWatchedSeq(for terminalId: String) -> Int {
        if let side = watchedSeq[terminalId] { return side }
        return terminals.first(where: { $0.id == terminalId })?.lastSeq ?? 0
    }

    /// Re-authorize everything this store holds on the current terminal-socket
    /// connection. A reconnect empties the host's per-connection authorization
    /// map, so `set_display_mode`/`write`/replay would all answer UNAUTHORIZED
    /// ("Un…") until the watch and every standalone viewer are re-taken.
    ///
    /// In-flight guard: `onConnected`, foreground revalidation and a user
    /// action can all land here at once; concurrent callers coalesce into the
    /// running recovery and re-check liveness when it finishes.
    func recoverAfterReconnect() async {
        // Coalesce concurrent callers into the running recovery, then re-check:
        // a caller that arrived mid-recovery still needs its own pass if the
        // state it cares about was re-staled while the first run was awaited.
        if let recoveryTask {
            _ = await recoveryTask.value
            guard needsRecovery else { return }
        }
        guard needsRecovery else { return }
        recoverySerial &+= 1
        let serial = recoverySerial
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performRecovery()
        }
        recoveryTask = task
        _ = await task.value
        if recoverySerial == serial { recoveryTask = nil }
    }

    private var needsRecovery: Bool {
        guard let socket, socket.isConnected else { return false }
        if let watchedId, !isWatching(watchedId) { return true }
        for (_, box) in viewers {
            if let viewer = box.viewer, !viewer.isClosed, !viewer.hasExited,
               !viewer.isLiveOnCurrentConnection {
                return true
            }
        }
        return false
    }

    private func performRecovery() async {
        guard let socket else { return }

        // 1. The workbench's watched PTY.
        if let id = watchedId, !isWatching(id) {
            let resume = lastWatchedSeq(for: id)
            do {
                let reply = try await socket.watch(terminalId: id, lastSeq: resume)
                watchedGeneration = socket.connectionGeneration
                if let seq = reply.latestSeq, seq > resume {
                    watchedSeq[id] = seq
                }
                errorMessage = nil
                HostLog.session.info("Re-watched workbench PTY after reconnect")
                // Cached grid can match while the host PTY changed in background;
                // re-assert equal dims to converge after resume.
                if geometryActive, displayMode == .phone, activeId == id {
                    scheduleRefit(force: true)
                }
            } catch {
                // The PTY may have exited while we were away; that is a clean
                // downgrade, not an error banner over a live session.
                if Self.isTerminalNotFound(error) {
                    HostLog.session.info("Watched PTY is gone after reconnect")
                    watchedId = nil
                    watchedGeneration = nil
                    watchedSeq.removeValue(forKey: id)
                } else {
                    HostLog.session.error("Workbench PTY re-watch failed after reconnect")
                }
            }
        }

        // 2. Standalone viewers (quick terminals).
        for (id, box) in viewers {
            guard let viewer = box.viewer, !viewer.isClosed, !viewer.hasExited else { continue }
            await viewer.reattachAfterReconnect(claim: standaloneClaims[id])
        }
    }

    /// The host reports a vanished PTY as TERMINAL_NOT_FOUND; recovery treats
    /// that as a clean downgrade instead of surfacing an error banner.
    nonisolated private static func isTerminalNotFound(_ error: Error) -> Bool {
        if case HostError.ipc(_, let code) = error, code.uppercased() == "TERMINAL_NOT_FOUND" {
            return true
        }
        return false
    }

    func open(_ terminalId: String) async {
        guard let socket else { return }
        do {
            try await ensureConnected()
            // Already watching this PTY on this connection: only re-assert the
            // phone viewport. Re-watching here replayed the whole scrollback
            // into the mounted canvas on every return from T1.
            if isWatching(terminalId) {
                activeId = terminalId
                scheduleRefit(force: true)
                return
            }
            // Same PTY on a new connection: resume at the tracked cursor. A
            // watch from seq 0 resends the whole retained log, and every
            // reply on this socket queues behind it.
            if watchedId == terminalId {
                await recoverAfterReconnect()
                if isWatching(terminalId) {
                    activeId = terminalId
                    scheduleRefit(force: true)
                    return
                }
            }
            if let watchedId, watchedId != terminalId {
                await releaseDisplayMode(for: watchedId)
                await socket.detach(terminalId: watchedId)
            }
            let attached = try await socket.watch(terminalId: terminalId, lastSeq: 0)
            if let index = terminals.firstIndex(where: { $0.id == terminalId }) {
                terminals[index].terminalScreenEngine = attached.terminalScreenEngine
                if let seq = attached.latestSeq {
                    terminals[index].lastSeq = max(terminals[index].lastSeq, seq)
                    watchedSeq[terminalId] = max(watchedSeq[terminalId] ?? 0, seq)
                }
                // This watch just replayed the whole log. Remember the grid it
                // was authored at: the phone fit below resizes the PTY, and a
                // replay interpreted at the new grid cannot rebuild the screen.
                terminals[index].replayGrid = TerminalReplayGrid(
                    cols: attached.cols,
                    rows: attached.rows
                )
            }
            watchedId = terminalId
            watchedGeneration = socket.connectionGeneration
            activeId = terminalId
            refitEpoch &+= 1
            errorMessage = nil
            // A fresh watch may land on a PTY the desktop resized meanwhile;
            // re-assert the phone viewport deterministically instead of
            // waiting for a view-driven sizeChanged that may never come.
            // The fittedTerminalId gate keeps this pending until the (possibly
            // remounted) view reports the real grid for this terminal.
            scheduleRefit(force: true)
        } catch {
            HostLog.session.error("Terminal watch failed")
            errorMessage = error.localizedDescription
        }
    }

    func spawn(conversationId: String?, projectId: String?, cols: Int = 80, rows: Int = 24) async {
        isConnecting = true
        defer { isConnecting = false }
        do {
            try await ensureConnected()
            guard let socket else { return }
            let spawned = try await socket.spawn(
                conversationId: conversationId,
                projectId: projectId,
                cols: cols,
                rows: rows
            )
            let live = LiveTerminal(
                id: spawned.id,
                claim: spawned.claim,
                lastSeq: 0,
                cols: spawned.cols ?? cols,
                rows: spawned.rows ?? rows,
                title: String(localized: "Terminal"),
                cwd: spawned.cwd,
                gitBranch: nil,
                owned: true,
                projectId: projectId,
                conversationId: conversationId,
                terminalScreenEngine: spawned.terminalScreenEngine
            )
            if !terminals.contains(where: { $0.id == live.id }) {
                terminals.append(live)
            }
            errorMessage = nil
            HostLog.session.info("Created a host terminal from the phone")
            await open(live.id)
        } catch {
            HostLog.session.error("Phone terminal create failed")
            errorMessage = error.localizedDescription
        }
    }

    func write(_ data: String) async {
        guard !data.isEmpty, socket != nil, let terminalId = activeId else { return }
        pendingWrites[terminalId, default: ""].append(data)
        guard !activeWriteDrains.contains(terminalId) else { return }
        activeWriteDrains.insert(terminalId)
        defer { activeWriteDrains.remove(terminalId) }

        // Yield one micro-turn so synchronous bursts (e.g. multi-line SGR
        // mouse-wheel ticks or fast keystrokes) coalesce into a single RPC.
        await Task.yield()

        while let chunk = pendingWrites.removeValue(forKey: terminalId), !chunk.isEmpty {
            do {
                try await sendWriteWithSingleRecoveryRetry(terminalId: terminalId, data: chunk)
                errorMessage = nil
            } catch {
                HostLog.session.error("Terminal write failed")
                errorMessage = error.localizedDescription
                pendingWrites.removeValue(forKey: terminalId)
                return
            }
        }
    }

    private func sendWriteWithSingleRecoveryRetry(terminalId: String, data: String) async throws {
        guard let socket else {
            throw HostError.network(String(localized: "Disconnected from the host."))
        }
        try await ensureConnected()
        if !isWatching(terminalId) {
            if watchedId == terminalId {
                await recoverAfterReconnect()
            } else {
                _ = try await socket.watch(terminalId: terminalId, lastSeq: lastWatchedSeq(for: terminalId))
                watchedId = terminalId
                watchedGeneration = socket.connectionGeneration
            }
        }
        let generation = socket.connectionGeneration
        do {
            try await socket.write(terminalId: terminalId, data: data)
        } catch {
            // Only input the host provably did not apply is resent: it never
            // left this device, or the host rejected it. An unacknowledged
            // write (`TerminalWriteUnconfirmed`) is surfaced instead.
            guard Self.isRecoverableWriteFailure(error) else { throw error }
            HostLog.session.info("Reconnecting terminal socket to retry failed write")
            // Another path may already be rebuilding the socket; stopping
            // here would cancel its handshake.
            if socket.isConnected, socket.connectionGeneration == generation {
                socket.stop()
            }
            try await ensureConnected()
            if watchedId == nil { watchedId = terminalId }
            await recoverAfterReconnect()
            try await socket.write(terminalId: terminalId, data: data)
        }
    }

    nonisolated static func isRecoverableWriteFailure(_ error: Error) -> Bool {
        switch error {
        case HostError.deviceRevoked, HostError.forbidden, HostError.rePairingRequired:
            return false
        case HostError.ipc(_, let code):
            let upper = code.uppercased()
            return upper == "UNAUTHORIZED" || upper == "NETWORK_ERROR"
        case HostError.network:
            return true
        default:
            return false
        }
    }

    private func scheduleProactiveReconnectIfNeeded() {
        guard backgroundReconnectTask == nil else { return }
        let hasLiveConsumer = watchedId != nil || viewers.values.contains { box in
            guard let viewer = box.viewer else { return false }
            return !viewer.isClosed && !viewer.hasExited
        }
        guard hasLiveConsumer, origin != nil, credentials != nil else { return }
        backgroundReconnectTask = Task { @MainActor [weak self] in
            defer { self?.backgroundReconnectTask = nil }
            try? await Task.sleep(for: .milliseconds(300))
            guard let self, !Task.isCancelled else { return }
            do {
                try await self.ensureConnected()
                await self.recoverAfterReconnect()
            } catch {
                HostLog.session.info("Proactive terminal socket reconnect deferred")
            }
        }
    }

    /// The view fitted itself to a new grid (layout, rotation, keyboard,
    /// text scale). Record it and converge through the debounce — the push
    /// to the host happens in `commitRefit`, never inline here. Covered
    /// surfaces never record: a keyboard shrinking a hidden terminal must
    /// not become the takeover grid. Synchronous: the Coordinator calls this
    /// straight from sizeChanged, so identical grids return without any
    /// Task churn.
    func resize(cols: Int, rows: Int) {
        guard geometryActive, let activeId else { return }
        if fittedTerminalId == activeId, lastFitted?.cols == cols, lastFitted?.rows == rows {
            return
        }
        lastFitted = (cols, rows)
        fittedTerminalId = activeId
        scheduleRefit()
    }

    func scheduleRefit(force: Bool = false) {
        if force {
            forceRefit = true
        }
        refitEpoch &+= 1
        let epoch = refitEpoch
        refitTask?.cancel()
        refitTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(150))
            guard let self, !Task.isCancelled else { return }
            await self.commitRefit(epoch: epoch)
        }
    }

    /// Invalidation hook for layout-level transitions (compact/regular swap):
    /// a pending commit issued under the old layout must never land.
    func refitEpochBump() {
        refitEpoch &+= 1
    }

    private func commitRefit(epoch: Int) async {
        // fittedTerminalId gate: until the live view has fitted itself FOR
        // THIS terminal, only host-side dims exist; the force flag survives
        // so the first real report still performs its forced takeover push.
        guard geometryActive, displayMode == .phone,
              let activeId, fittedTerminalId == activeId,
              let fitted = lastFitted else { return }
        guard let socket else { return }
        let terminalId = activeId
        // Authorization gate: `set_display_mode` on a PTY this connection
        // never watched answers UNAUTHORIZED (the raw "Un…" string). If a
        // reconnect dropped the watch, recover it first; if recovery cannot
        // authorize yet, leave the force flag armed so the open/watch that
        // follows performs the takeover push instead.
        if !isWatching(terminalId) {
            await recoverAfterReconnect()
            guard isWatching(terminalId) else {
                HostLog.session.info("Phone fit deferred until the PTY is watched again")
                return
            }
        }
        let cols = max(fitted.cols, 20)
        let rows = max(fitted.rows, 4)
        let forced = forceRefit
        if !forced, let pushed = lastPushed[terminalId], pushed.cols == cols, pushed.rows == rows {
            return
        }
        do {
            let state = try await socket.setDisplayMode(
                terminalId: terminalId,
                mode: TerminalDisplayMode.phone.rawValue,
                cols: cols,
                rows: rows
            )
            // Epoch+identity re-validation: a schedule, release, terminal
            // switch, or host-side restore that happened while this request
            // was in flight must not let a stale phone lease land.
            guard epoch == refitEpoch,
                  activeId == terminalId,
                  geometryActive,
                  displayMode == .phone
            else {
                if activeId != terminalId || !geometryActive || displayMode != .phone {
                    // This terminal is no longer the live phone viewer —
                    // restore the desktop grid it should never have kept.
                    HostLog.session.info("Dropping stale phone fit; restoring desktop")
                    do {
                        _ = try await socket.setDisplayMode(
                            terminalId: terminalId,
                            mode: TerminalDisplayMode.desktop.rawValue
                        )
                    } catch {
                        HostLog.session.error("Stale-fit desktop restore failed")
                    }
                } else {
                    // Pure epoch mismatch (e.g. leave+return): a newer refit
                    // already owns the live lease — restoring desktop here
                    // would undo it. Drop only.
                    HostLog.session.info("Dropping stale phone fit; newer refit owns the lease")
                }
                return
            }
            applyState(state, for: terminalId)
            lastPushed[terminalId] = (state.cols, state.rows)
            forceRefit = false
            errorMessage = nil
            HostLog.ui.info("Phone fit \(state.cols)x\(state.rows)\(forced ? " (forced)" : "", privacy: .public)")
        } catch {
            HostLog.session.error("Phone fit push failed")
            errorMessage = error.localizedDescription
            // force stays armed; the next trigger re-asserts.
        }
    }

    func setDisplayMode(_ mode: TerminalDisplayMode) async {
        displayMode = mode
        refitEpoch &+= 1
        guard let socket, let activeId else { return }
        if mode == .desktop {
            lastPushed.removeValue(forKey: activeId)
            do {
                let state = try await socket.setDisplayMode(
                    terminalId: activeId,
                    mode: TerminalDisplayMode.desktop.rawValue
                )
                applyState(state, for: activeId)
                errorMessage = nil
            } catch {
                HostLog.session.error("Desktop display mode switch failed")
                errorMessage = error.localizedDescription
            }
            HostLog.session.info("Terminal display mode desktop")
            return
        }
        // Phone takeover is deterministic: the debounced refit adopts the
        // current fitted grid even if it equals the last push.
        scheduleRefit(force: true)
        HostLog.session.info("Terminal display mode phone")
    }

    func releaseDisplayMode(for terminalId: String?) async {
        guard let socket, let terminalId else { return }
        refitEpoch &+= 1
        lastPushed.removeValue(forKey: terminalId)
        do {
            let state = try await socket.setDisplayMode(
                terminalId: terminalId,
                mode: TerminalDisplayMode.desktop.rawValue
            )
            applyState(state, for: terminalId)
        } catch {
            // Release is best-effort: the host sweeps stale phone fits on
            // disconnect; the parked desktop size is what it restores to.
            HostLog.session.info("Display-mode release failed")
        }
    }

    private func applyState(_ state: TerminalDisplayModeState, for terminalId: String) {
        guard state.cols > 1, state.rows > 1,
              let index = terminals.firstIndex(where: { $0.id == terminalId }) else { return }
        terminals[index].cols = state.cols
        terminals[index].rows = state.rows
    }

    private func enqueueOutput(terminalId: String, data: Data, gap: Bool, latestSeq: Int? = nil) {
        // A gap means the new stream starts mid-sequence. Bytes from the
        // previous stream that are still waiting in the 16 ms window belong to
        // the truncated prefix; drop them so the reset cannot be preceded by
        // stale bytes.
        if gap {
            coalesceBuffers[terminalId] = Data()
            coalesceGapped.insert(terminalId)
        }
        coalesceBuffers[terminalId, default: Data()].append(data)
        if let latestSeq {
            coalesceSeq[terminalId] = max(coalesceSeq[terminalId] ?? 0, latestSeq)
        }
        if coalesceTask == nil {
            coalesceTask = Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(16))
                guard let self else { return }
                let pending = self.coalesceBuffers
                let gapped = self.coalesceGapped
                let seqs = self.coalesceSeq
                self.coalesceBuffers.removeAll()
                self.coalesceGapped.removeAll()
                self.coalesceSeq.removeAll()
                self.coalesceTask = nil
                for (id, bytes) in pending {
                    self.deliverOutput(terminalId: id, data: bytes, gap: gapped.contains(id), latestSeq: seqs[id])
                }
            }
        }
    }

    /// Route one terminal's bytes. Only the feed installed for this exact
    /// terminal may consume them; every other terminal buffers by id, which is
    /// what keeps a replay that lands before its canvas remounts from being
    /// dropped by the outgoing tab's feed.
    ///
    /// Internal (not private) so the replay-continuity tests can drive the
    /// coalesced destination without a live socket.
    func deliverOutput(terminalId: String, data: Data, gap: Bool = false, latestSeq: Int? = nil) {
        if let existing = terminals.firstIndex(where: { $0.id == terminalId }) {
            // The host's own cursor is authoritative when present; the +1
            // fallback keeps old hosts (no seq on the frame) monotonic.
            if let latestSeq {
                terminals[existing].lastSeq = max(terminals[existing].lastSeq, latestSeq)
            } else {
                terminals[existing].lastSeq += 1
            }
            watchedSeq[terminalId] = max(watchedSeq[terminalId] ?? 0, terminals[existing].lastSeq)
        } else if let latestSeq {
            watchedSeq[terminalId] = max(watchedSeq[terminalId] ?? 0, latestSeq)
        }
        // A standalone viewer (quick terminal) owns this PTY's bytes, so the
        // workbench's single `onFeed` never has to know it exists.
        if let box = viewers[terminalId] {
            if let viewer = box.viewer {
                viewer.receive(data, reset: gap, latestSeq: latestSeq)
                return
            }
            viewers.removeValue(forKey: terminalId)
        }
        if let onFeed, feedTerminalId == terminalId {
            onFeed(terminalId, data, gap)
            return
        }
        if gap {
            // The buffered prefix came from before the gap and cannot be
            // trusted; the reset flag tells the canvas to start clean.
            pendingOutput[terminalId] = Data()
            pendingReset.insert(terminalId)
        }
        var buffered = pendingOutput[terminalId] ?? Data()
        buffered.append(data)
        if buffered.count > Self.pendingOutputLimit {
            buffered.removeFirst(buffered.count - Self.pendingOutputLimit)
            // Dropping the oldest bytes cuts the stream mid-sequence, exactly
            // like a host gap, so the canvas must start clean.
            pendingReset.insert(terminalId)
        }
        pendingOutput[terminalId] = buffered
    }

    /// Install the mounted canvas for `terminalId` as the destination for its
    /// output, in the same synchronous step that hands over whatever the store
    /// buffered while that canvas did not exist. Splitting the two would let
    /// output that arrives in between land in the buffer and be lost.
    func installFeed(for terminalId: String, _ feed: @escaping @MainActor (Data, Bool) -> Void) {
        feedTerminalId = terminalId
        onFeed = { incomingId, data, reset in
            guard incomingId == terminalId else { return }
            feed(data, reset)
        }
        if let buffered = consumeOutput(for: terminalId) {
            feed(buffered.data, buffered.needsReset)
        }
    }

    func consumeOutput(for terminalId: String) -> (data: Data, needsReset: Bool)? {
        guard let data = pendingOutput.removeValue(forKey: terminalId) else { return nil }
        let needsReset = pendingReset.remove(terminalId) != nil
        return data.isEmpty && !needsReset ? nil : (data, needsReset)
    }

    /// Pure catalog mapping. Owned flags, claims and replay cursors survive a
    /// refresh; catalog rows contribute their project/conversation binding.
    /// Keeping it out of the socket path makes the mapping testable.
    static func liveTerminals(
        from items: [LiveTerminalSummary],
        ownedById: [String: LiveTerminal]
    ) -> [LiveTerminal] {
        items.map { item in
            let owned = ownedById[item.id]
            return LiveTerminal(
                id: item.id,
                claim: owned?.claim,
                lastSeq: owned?.lastSeq ?? 0,
                cols: item.cols ?? owned?.cols ?? 80,
                rows: item.rows ?? owned?.rows ?? 24,
                title: Self.title(for: item),
                cwd: item.cwd,
                gitBranch: item.gitBranch,
                owned: owned != nil,
                projectId: item.projectId ?? owned?.projectId,
                conversationId: item.conversationId ?? owned?.conversationId,
                terminalScreenEngine: item.terminalScreenEngine,
                screenObservation: item.screenObservation ?? .legacyHost,
                replayGrid: owned?.replayGrid
            )
        }
    }

    private static func title(for item: LiveTerminalSummary) -> String {
        if let title = item.title, !title.isEmpty {
            if let branch = item.gitBranch, !branch.isEmpty {
                return "\(title) · \(branch)"
            }
            return title
        }
        if let cwd = item.cwd, let name = cwd.split(separator: "/").last, !name.isEmpty {
            return String(name)
        }
        return String(localized: "Terminal")
    }
}

/// Weak slot for a standalone PTY viewer so a dismissed console cannot keep
/// receiving host bytes.
@MainActor
private final class TerminalViewerBox {
    weak var viewer: TerminalPTYViewer?

    init(viewer: TerminalPTYViewer) {
        self.viewer = viewer
    }
}
