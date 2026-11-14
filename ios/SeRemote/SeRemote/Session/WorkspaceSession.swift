import Foundation
import Network
import Observation
import UIKit

enum KeyboardGuard {
    @MainActor
    static func resign() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }

    static func overlapHeight(from notification: Notification) -> CGFloat {
        guard let frame = notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect else {
            return 0
        }
        let window = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .first(where: \.isKeyWindow)
        guard let window else { return frame.height }
        let end = window.convert(frame, from: nil)
        return max(0, window.bounds.maxY - end.minY)
    }
}

enum SessionPhase: Equatable {
    case idle
    case connecting
    case connected
    case revoked
    case failed(String)
}

enum WorkspaceKind: Equatable {
    case home
    case conversation
    case project
}

@MainActor
@Observable
final class WorkspaceSession {
    let origin: URL
    let credentials: HostCredentials
    let http: HostHTTP
    let acp = AcpSocket()
    let terminalSocket = TerminalSocket()
    let desktopSocket = DesktopSocket()
    let chat = ChatStore()
    let conversations = ConversationStore()
    let projects = ProjectStore()
    let files = FileStore()
    let terminals = TerminalStore()
    let git = GitStore()
    let scheduledTasks = ScheduledTaskStore()
    let quickTerminals = QuickTerminalStore()
    let search = SearchStore()
    let liveAgents = LiveAgentSessionStore()

    var workspace: WorkspaceKind = .home
    var phase: SessionPhase = .idle
    /// The handshake completed at least once in this session's life. Once
    /// true, a foreground/transport recovery keeps the mounted shell visible:
    /// the live terminal/chat canvas is never blanked behind a full-screen
    /// "Connecting…" spinner for a link that already worked.
    private(set) var hasEstablishedConnection = false
    /// True while `start()` is re-running its handshakes to recover an
    /// already-established session (transport blip, foreground return). The
    /// UI keeps `phase == .connected` presentation during this window.
    private(set) var isRecovering = false
    /// The last open failed in a way a retry cannot fix: the saved origin no
    /// longer reaches the host, or the host refuses this device's credential.
    /// The caller offers a pairing scan instead of another retry.
    private(set) var needsRePairing = false
    /// Software keyboard is covering the terminal surface. Chrome hides while typing.
    var terminalKeyboardVisible = false
    var terminalBlurToken: UInt64 = 0
    /// The workbench surface is the visible page. Keyboard ownership reads this;
    /// PTY geometry is owned by park/resume, so a hidden surface never claims it.
    var terminalSurfaceVisible = false
    private(set) var isBackgrounded = false
    /// Whether the host serves the remote desktop. `nil` until probed: unknown
    /// hides nothing, `false` hides every desktop entry.
    private(set) var desktopSupported: Bool?
    /// Pending composer drafts keyed by terminal id. The draft survives a
    /// panel that is not mounted, replacing the one-shot notification route.
    @ObservationIgnored private var agentDrafts: [String: String] = [:]
    private var isStarting = false
    /// Watches the phone's path (Wi-Fi ↔ cellular ↔ VPN). A changed route can
    /// silently strand both WebSockets, so the session re-validates them instead
    /// of waiting for the next user request to fail.
    private var pathMonitor: NWPathMonitor?
    private var pathRefreshTask: Task<Void, Never>?
    private var lastPathWasSatisfied: Bool?
    /// Host-backed running state (live terminals + live agent processes) is
    /// polled: the host does not push a terminal-started event to the phone.
    private var runningRefreshTask: Task<Void, Never>?
    private var startEpoch = 0
    private let notificationSettings: AppSettings?
    /// Stable host id for notification payloads; `nil` on hosts that predate it.
    private let hostID: String?
    @ObservationIgnored nonisolated(unsafe) private var revocationObserver: NSObjectProtocol?

    init(accessURL: URL, bearer: String? = nil, settings: AppSettings? = nil, hostID: String? = nil) {
        credentials = HostCredentials(accessURL: accessURL, bearer: bearer)
        origin = credentials.origin
        http = HostHTTP(origin: origin, credentials: credentials)
        notificationSettings = settings
        self.hostID = hostID
        chat.attach(socket: acp)
        chat.onHostConversationsChanged = { [weak self] in
            guard let self else { return }
            Task { await self.conversations.refresh() }
        }
        wireBackgroundNotifications()
        conversations.attach(http: http)
        projects.attach(http: http, socket: acp)
        liveAgents.attach(http: http)
        files.attach(http: http)
        git.attach(http: http)
        scheduledTasks.attach(http: http)
        quickTerminals.attach(http: http)
        search.attach(http: http)
        terminals.attach(socket: terminalSocket, origin: origin, credentials: credentials)
        desktopSocket.configure(http: http)
        let originString = origin.absoluteString
        revocationObserver = NotificationCenter.default.addObserver(
            forName: .hostDeviceRevoked, object: nil, queue: .main
        ) { [weak self] notification in
            guard notification.object as? String == originString else { return }
            MainActor.assumeIsolated { self?.markDeviceRevoked() }
        }
    }

    deinit {
        if let revocationObserver { NotificationCenter.default.removeObserver(revocationObserver) }
    }

    /// Revoke transports without changing workspace selection, PTYs, mounted
    /// terminal state or the existing geometry lease.
    func markDeviceRevoked() {
        guard phase != .revoked else { return }
        phase = .revoked
        needsRePairing = false
        acp.revoke()
        terminalSocket.revoke()
        desktopSocket.revoke()
        stop()
    }

    /// Fire local notifications for host events that land while backgrounded,
    /// gated on the settings toggle. Mirrors the browser client's remote
    /// notification behavior.
    private func wireBackgroundNotifications() {
        chat.onTurnSettled = { [weak self] in
            guard let self else { return }
            notifyIfBackgrounded(
                title: conversations.active?.displayTitle ?? String(localized: "Agent"),
                body: String(localized: "The agent finished its turn.")
            )
        }
        chat.onAttentionNeeded = { [weak self] title, body in
            self?.notifyIfBackgrounded(title: title, body: body)
        }
        terminals.onTerminalExit = { [weak self] terminalId in
            guard let self else { return }
            let name = terminals.terminals.first(where: { $0.id == terminalId })?.title
                ?? String(localized: "Terminal")
            notifyIfBackgrounded(
                title: name,
                body: String(localized: "The terminal exited."),
                terminalID: terminalId
            )
        }
    }

    private func notifyIfBackgrounded(title: String, body: String, terminalID: String? = nil) {
        guard isBackgrounded, notificationSettings?.notificationsEnabled == true else { return }
        Task {
            await SeNotifications.post(title: title, body: body, hostID: hostID, terminalID: terminalID)
        }
    }

    func start() async {
        guard phase != .revoked, !isStarting else { return }
        startEpoch += 1
        let epoch = startEpoch
        isStarting = true
        // Recovery of an already-established session keeps the mounted shell
        // on screen: only a first-ever connect (or an exhausted retry) flips
        // the visible phase, so a foreground return never blanks the live
        // terminal/chat canvas behind a full-screen "Connecting…" spinner.
        let recovering = hasEstablishedConnection
        isRecovering = recovering
        defer {
            if startEpoch == epoch {
                isStarting = false
                if isRecovering { isRecovering = false }
            }
        }
        if !recovering {
            phase = .connecting
        }
        needsRePairing = false
        HostLog.session.info("Host connect started")
        do {
            await HostNetwork.waitUntilReady()
            try Task.checkCancellation()
            guard startEpoch == epoch else { return }
            try await http.probeHealth()
            try Task.checkCancellation()
            guard startEpoch == epoch else { return }
            try await connectAgent()
            guard startEpoch == epoch else { return }
            await conversations.refresh()
            await projects.refresh()
            await liveAgents.refresh()
            do {
                try await terminals.ensureConnected()
            } catch {
                terminals.errorMessage = error.localizedDescription
            }
            guard startEpoch == epoch else { return }
            hasEstablishedConnection = true
            phase = .connected
            startPathMonitor()
            startRunningRefresh()
            await refreshDesktopSupport()
            // A rebuilt terminal socket is a fresh connection with an empty
            // server-side authorization map: re-watch/re-attach now so the
            // first user action is not answered with UNAUTHORIZED.
            await terminals.recoverAfterReconnect()
            HostLog.session.info("Host connect succeeded")
        } catch is CancellationError {
            HostLog.session.info("Host connect cancelled")
        } catch {
            guard startEpoch == epoch, !Task.isCancelled else { return }
            if case HostError.deviceRevoked = error {
                markDeviceRevoked()
                return
            }
            if case HostError.forbidden = error {
                // The global capability notice explains the failed operation;
                // other scopes and the mounted workspace remain usable.
                hasEstablishedConnection = true
                phase = .connected
                startRunningRefresh()
                await refreshDesktopSupport()
                return
            }
            HostLog.session.error("Host connect failed after restart-safe retries")
            // The origin is the classifier's evidence: a timeout on a private
            // LAN address means the host moved, not that the Mac is slow.
            needsRePairing = PairingRecovery.needsRePairing(error, origin: origin)
            phase = .failed(error.localizedDescription)
        }
    }

    private func connectAgent() async throws {
        var lastError: Error?
        for attempt in 0 ..< 3 {
            do {
                try await acp.connect(origin: origin, credentials: credentials)
                return
            } catch {
                if case HostError.deviceRevoked = error { throw error }
                if case HostError.forbidden = error { throw error }
                guard phase != .revoked else { throw HostError.deviceRevoked }
                lastError = error
                acp.stop()
                if attempt < 2 {
                    try await Task.sleep(for: .milliseconds(600 * (1 << attempt)))
                }
            }
        }
        throw lastError ?? HostError.network(String(localized: "The phone could not open the host WebSocket."))
    }

    func retry() async {
        stop()
        await start()
    }

    func stop() {
        startEpoch += 1
        isStarting = false
        // Teardown is the one place the recovery promise ends: an explicit
        // disconnect (or the session being replaced) must not keep a stale
        // shell mounted behind a claimed "connected" phase.
        hasEstablishedConnection = false
        isRecovering = false
        pathRefreshTask?.cancel()
        pathRefreshTask = nil
        pathMonitor?.cancel()
        pathMonitor = nil
        lastPathWasSatisfied = nil
        runningRefreshTask?.cancel()
        runningRefreshTask = nil
        acp.stop()
        terminalSocket.stop()
        desktopSocket.close()
    }

    /// Poll the same 15 s cadence as the desktop agent tracker. The first tick
    /// is skipped because `start()` already refreshed both stores.
    private func startRunningRefresh() {
        runningRefreshTask?.cancel()
        runningRefreshTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                guard let self, !Task.isCancelled else { return }
                guard self.phase == .connected, !self.isBackgrounded else { continue }
                await self.refreshRunningState()
            }
        }
    }

    private func refreshRunningState() async {
        await projects.refresh()
        await liveAgents.refresh()
    }

    /// Re-probe on connect and on foreground so a host upgrade or downgrade is
    /// picked up without reopening the workspace. Failure keeps the last answer.
    func refreshDesktopSupport() async {
        guard phase == .connected else { return }
        guard let info = try? await http.desktopInfo() else { return }
        desktopSupported = info.supported
    }

    func handleScene(isBackground: Bool) {
        isBackgrounded = isBackground
        guard phase != .revoked else { return }
        acp.handleLifecycle(isBackground: isBackground)
        desktopSocket.handleLifecycle(isBackground: isBackground)
        if !isBackground {
            // Everything the foreground return needs happens AFTER the
            // transports are proven alive (or rebuilt): scheduling a refit or
            // a catalog refresh at t=0 races the still-suspended socket and
            // pushes set_display_mode/list at a connection that has not
            // re-watched yet — the raw "Unauthorized" answer. The settle
            // delay lets iOS finish resuming the sockets first.
            scheduleTransportRefresh(reason: "foreground", settleMilliseconds: 150)
        }
    }

    // MARK: - Transport freshness

    /// One monitor per live session. The first callback describes the current
    /// path, so it only records the baseline; later callbacks are real changes
    /// (Wi-Fi ↔ cellular, VPN up/down) that can strand both WebSockets.
    private func startPathMonitor() {
        guard pathMonitor == nil else { return }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in
                self?.handlePathUpdate(satisfied: satisfied)
            }
        }
        monitor.start(queue: DispatchQueue(label: "\(HostLog.subsystem).session.path"))
        pathMonitor = monitor
    }

    private func handlePathUpdate(satisfied: Bool) {
        let wasSatisfied = lastPathWasSatisfied
        lastPathWasSatisfied = satisfied
        guard satisfied else {
            HostLog.session.info("Network path unavailable")
            return
        }
        // The initial callback is the baseline, not a change.
        guard wasSatisfied != nil else { return }
        HostLog.session.info("Network path changed")
        scheduleTransportRefresh(reason: "path-change")
    }

    /// Coalesce foreground and path notifications into one revalidation after
    /// the route has settled. The foreground path uses a shorter settle: the
    /// sockets were merely suspended, not lost.
    private func scheduleTransportRefresh(reason: String, settleMilliseconds: Int = 350) {
        guard !isBackgrounded else { return }
        switch phase {
        case .connected, .failed:
            break
        case .idle, .connecting, .revoked:
            return
        }
        pathRefreshTask?.cancel()
        pathRefreshTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(settleMilliseconds))
            guard let self, !Task.isCancelled else { return }
            await self.revalidateTransports(reason: reason)
        }
    }

    /// Rebuild transports that outlived the previous network path, and retry a
    /// link that is already offline. `phase` only becomes `.failed` inside
    /// `start()`, after its retry budget is spent.
    private func revalidateTransports(reason: String) async {
        guard phase != .revoked, !isBackgrounded else { return }
        switch phase {
        case .failed:
            HostLog.session.info("Retrying host link after \(reason, privacy: .public)")
            await start()
            if phase == .connected {
                await convergeAfterTransportRecovery()
            }
        case .connected:
            let acpLiveness = await acp.probeLiveness()
            let terminalLiveness = await terminalSocket.probeLiveness()
            guard acpLiveness.isDead else {
                if acpLiveness == .unknown {
                    HostLog.session.info("ACP liveness inconclusive after \(reason, privacy: .public)")
                }
                // Half-open terminal WebSockets commonly fail to answer a ping
                // within 4 s (`.unknown`) rather than reporting `-1004` right
                // away; keeping them leaves `write` wedged until timeout.
                if terminalLiveness != .alive {
                    HostLog.session.info("Terminal transport \(String(describing: terminalLiveness), privacy: .public) after \(reason, privacy: .public); rebuilding terminal socket")
                    await rebuildTerminalTransport()
                }
                await convergeAfterTransportRecovery()
                return
            }
            HostLog.session.info("Stale host transport after \(reason, privacy: .public); reconnecting link")
            await start()
            if phase == .connected {
                await convergeAfterTransportRecovery()
            }
        case .idle, .connecting:
            // The in-flight start owns the retry budget.
            break
        case .revoked:
            break
        }
    }

    /// Post-recovery convergence: cached grid can match while the host PTY
    /// changed in the background, and running state may have moved while
    /// polling was parked. Runs only after the transports are verified alive
    /// or freshly rebuilt, never speculatively at foreground t=0.
    private func convergeAfterTransportRecovery() async {
        guard phase == .connected else { return }
        terminals.scheduleRefit(force: true)
        await refreshRunningState()
        await refreshDesktopSupport()
    }

    /// Rebuild the terminal WebSocket before a user-visible request if the
    /// socket outlived its network path. A ping is only sent while the socket
    /// still claims to be connected; a known-down socket reconnects at once.
    /// This keeps one stale-path failure from surfacing as a failed open.
    func ensureTerminalTransport() async {
        guard phase == .connected else { return }
        guard await terminalSocket.probeLiveness() != .alive else { return }
        await rebuildTerminalTransport()
    }

    private func rebuildTerminalTransport() async {
        HostLog.session.info("Terminal transport stale; reconnecting")
        terminalSocket.stop()
        do {
            try await terminals.ensureConnected()
            // The rebuilt socket is a new connection with an empty
            // server-side authorization map: re-watch the workbench PTY and
            // re-attach standalone viewers so the next user action is not
            // answered with UNAUTHORIZED ("Un…").
            await terminals.recoverAfterReconnect()
        } catch {
            HostLog.session.error("Terminal transport reconnect failed")
            terminals.errorMessage = error.localizedDescription
        }
    }

    /// Same freshness check for the agent socket. A failed reconnect leaves the
    /// last chat state alone instead of claiming the whole link is down.
    func ensureAgentTransport() async {
        guard phase == .connected else { return }
        guard await acp.probeLiveness().isDead else { return }
        HostLog.session.info("Agent transport stale; reconnecting")
        do {
            try await connectAgent()
        } catch {
            HostLog.session.error("Agent transport reconnect failed")
        }
    }

    func selectConversation(_ conversation: HostConversation) async {
        projects.clearSelection()
        workspace = .conversation
        await ensureAgentTransport()
        _ = await conversations.open(conversation)
        let opened = conversations.active ?? conversation
        let binding = await conversations.binding(for: opened)
        await chat.bindConversation(opened, binding: binding)
        await refreshActiveTerminals()
        await files.openRoot(opened.workspaceCwd)
    }

    func selectProject(_ project: HostProject) async {
        conversations.clearSelection()
        workspace = .project
        await ensureAgentTransport()
        await projects.select(project)
        await refreshActiveTerminals()
        if let path = project.path {
            await files.openRoot(path)
        }
    }

    func leaveWorkspace() {
        dismissTerminalKeyboard()
        terminals.geometryActive = false
        let terminalId = terminals.activeId
        Task { await terminals.releaseDisplayMode(for: terminalId) }
        terminals.displayMode = .phone
        workspace = .home
        conversations.clearSelection()
        projects.clearSelection()
        terminals.terminals = []
        terminals.activeId = nil
        chat.leave()
    }

    /// Watch an existing host shell. Presentation belongs to the shell, so this
    /// only moves the store's watch; it never ends or respawns the PTY.
    func revealTerminal(_ terminalId: String) async {
        await terminals.open(terminalId)
    }

    func dismissTerminalKeyboard() {
        let wasVisible = terminalKeyboardVisible
        terminalBlurToken &+= 1
        noteTerminalKeyboard(height: 0)
        KeyboardGuard.resign()
        if wasVisible {
            HostLog.session.info("Terminal keyboard dismissed")
        }
    }

    func noteTerminalKeyboard(height: CGFloat) {
        // A keyboard over the chat composer is not the terminal's keyboard:
        // the composer's claim decides whose keyboard this is.
        let coversTerminal = !chat.composerActive && terminalSurfaceVisible
        let visible = coversTerminal && height > 40
        terminalKeyboardVisible = visible
    }

    /// Host folder the open workspace is rooted at: the conversation's
    /// workspace cwd, or the project path. Git, search and Tools scope to it.
    var workspaceRoot: String? {
        switch workspace {
        case .conversation:
            conversations.active?.workspaceCwd
        case .project:
            projects.active?.path
        case .home:
            nil
        }
    }

    // MARK: - Terminal workbench surface (additive)

    /// Release the live viewport lease without ending the PTY or dropping the
    /// mounted canvas. Order is load-bearing: dismiss the keyboard, stop
    /// recording fitted grids, hand the host its desktop size, then reset the
    /// local mode. The visible page is untouched — the shell owns presentation.
    func parkTerminalSurface() async {
        dismissTerminalKeyboard()
        terminals.geometryActive = false
        let terminalId = terminals.activeId
        await terminals.releaseDisplayMode(for: terminalId)
        terminals.displayMode = .phone
    }

    /// Re-assert the phone viewport for the terminal that is already watched.
    /// A forced refit converges even when the grid is unchanged (reconnect,
    /// panel return, host-side resize while parked).
    func resumeTerminalSurface() async {
        guard terminals.activeId != nil else { return }
        terminals.displayMode = .phone
        terminals.geometryActive = true
        terminals.scheduleRefit(force: true)
    }

    /// Project the current terminal belongs to. Quick terminals are standalone
    /// and never have one; the catalog binding travels with the live row.
    var currentTerminalProjectID: String? {
        switch workspace {
        case .conversation:
            conversations.active?.projectId
        case .project:
            projects.active?.id
        case .home:
            nil
        }
    }

    var currentTerminalProjectName: String? {
        switch workspace {
        case .conversation:
            guard let id = conversations.active?.projectId else { return nil }
            return projects.projects.first(where: { $0.id == id })?.name
        case .project:
            return projects.active?.name
        case .home:
            return nil
        }
    }

    /// Directory the active terminal is in, falling back to the workspace root
    /// before the first catalog reply carries a cwd.
    var currentTerminalCwd: String? {
        let terminal = terminals.terminals.first(where: { $0.id == terminals.activeId })
        return terminal?.cwd ?? workspaceRoot
    }

    /// Root for Git, search, scheduled tasks and the file panel: the terminal's
    /// own cwd wins, because that is where the user is actually working.
    var activeToolRoot: String? {
        currentTerminalCwd ?? workspaceRoot
    }

    /// Deliver a staged path to the agent composer. The draft stays queued
    /// until a panel takes it, so a closed panel cannot lose the text.
    func offerAgentDraft(_ text: String, terminalID: String) {
        guard !text.isEmpty else { return }
        agentDrafts[terminalID] = text
    }

    func takeAgentDraft(terminalID: String) -> String? {
        agentDrafts.removeValue(forKey: terminalID)
    }

    /// Spawn a conversation/project-scoped PTY on the host and reveal it.
    func spawnTerminal() async {
        switch workspace {
        case .conversation:
            guard let conversation = conversations.active else { return }
            await terminals.spawn(conversationId: conversation.id, projectId: conversation.projectId)
        case .project:
            guard let project = projects.active else { return }
            await terminals.spawn(conversationId: nil, projectId: project.id)
        case .home:
            return
        }
        if let id = terminals.activeId {
            await revealTerminal(id)
        }
    }

    func refreshActiveTerminals() async {
        switch workspace {
        case .conversation:
            guard let conversation = conversations.active else {
                terminals.terminals = []
                terminals.activeId = nil
                return
            }
            await terminals.refresh(conversationId: conversation.id, projectId: nil)
        case .project:
            guard let project = projects.active else {
                terminals.terminals = []
                terminals.activeId = nil
                return
            }
            await terminals.refresh(conversationId: nil, projectId: project.id)
        case .home:
            terminals.terminals = []
            terminals.activeId = nil
        }
    }
}
