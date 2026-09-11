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
}

@MainActor
@Observable
final class TerminalStore {
    var terminals: [LiveTerminal] = []
    var activeId: String?
    var pendingOutput: [String: Data] = [:]
    var errorMessage: String?
    var isConnecting = false
    var onFeed: (@MainActor (String, Data) -> Void)?
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
    private var coalesceTask: Task<Void, Never>?
    private var socket: TerminalSocket?
    private var origin: URL?
    private var credentials: HostCredentials?
    private var watchedId: String?
    private var lastConversationId: String?
    private var lastProjectId: String?

    /// Side channel for a local notification when a watched PTY exits.
    var onTerminalExit: ((String) -> Void)?

    func attach(socket: TerminalSocket, origin: URL, credentials: HostCredentials) {
        self.socket = socket
        self.origin = origin
        self.credentials = credentials
        socket.onBytes = { [weak self] terminalId, data in
            self?.enqueueOutput(terminalId: terminalId, data: data)
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
            if self.watchedId == terminalId {
                self.watchedId = nil
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
            terminals = listed.map { item in
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
                    owned: owned != nil
                )
            }
            if let previousActive, terminals.contains(where: { $0.id == previousActive }) {
                activeId = previousActive
            } else {
                activeId = terminals.first?.id
            }
            if let activeId, watchedId != activeId {
                await open(activeId)
            }
        } catch {
            HostLog.session.error("Terminal catalog refresh failed")
            errorMessage = error.localizedDescription
        }
    }

    func open(_ terminalId: String) async {
        guard let socket else { return }
        do {
            try await ensureConnected()
            if let watchedId, watchedId != terminalId {
                await releaseDisplayMode(for: watchedId)
                await socket.detach(terminalId: watchedId)
            }
            _ = try await socket.watch(terminalId: terminalId, lastSeq: 0)
            watchedId = terminalId
            activeId = terminalId
            refitEpoch &+= 1
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
                owned: true
            )
            if !terminals.contains(where: { $0.id == live.id }) {
                terminals.append(live)
            }
            HostLog.session.info("Created a host terminal from the phone")
            await open(live.id)
        } catch {
            HostLog.session.error("Phone terminal create failed")
            errorMessage = error.localizedDescription
        }
    }

    func write(_ data: String) async {
        guard let socket, let activeId else { return }
        do {
            try await socket.write(terminalId: activeId, data: data)
        } catch {
            HostLog.session.error("Terminal write failed")
            errorMessage = error.localizedDescription
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
                HostLog.session.info("Dropping stale phone fit; restoring desktop")
                do {
                    _ = try await socket.setDisplayMode(
                        terminalId: terminalId,
                        mode: TerminalDisplayMode.desktop.rawValue
                    )
                } catch {
                    HostLog.session.error("Stale-fit desktop restore failed")
                }
                return
            }
            applyState(state, for: terminalId)
            lastPushed[terminalId] = (state.cols, state.rows)
            forceRefit = false
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

    private func enqueueOutput(terminalId: String, data: Data) {
        coalesceBuffers[terminalId, default: Data()].append(data)
        if coalesceTask == nil {
            coalesceTask = Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(16))
                guard let self else { return }
                let pending = self.coalesceBuffers
                self.coalesceBuffers.removeAll()
                self.coalesceTask = nil
                for (id, bytes) in pending {
                    self.deliverOutput(terminalId: id, data: bytes)
                }
            }
        }
    }

    private func deliverOutput(terminalId: String, data: Data) {
        if let existing = terminals.firstIndex(where: { $0.id == terminalId }) {
            terminals[existing].lastSeq += 1
        }
        if let onFeed {
            onFeed(terminalId, data)
        } else {
            pendingOutput[terminalId, default: Data()].append(data)
        }
    }

    func consumeOutput(for terminalId: String) -> Data? {
        let data = pendingOutput.removeValue(forKey: terminalId)
        return data?.isEmpty == false ? data : nil
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
