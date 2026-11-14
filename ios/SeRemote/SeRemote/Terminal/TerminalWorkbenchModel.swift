import Foundation
import Observation

/// The one terminal surface's state machine.
///
/// A single mounted canvas is fed by either a workspace backend
/// (`WorkspaceSession` + `TerminalStore`) or a standalone backend
/// (`TerminalPTYViewer`). Switching backends is ordered so two phone-fit
/// leases never overlap, and no path kills or respawns a PTY:
///
/// - standalone → workspace: release the viewer first, then reveal + resume.
/// - workspace → standalone: park the workspace surface first, then attach.
/// - overlay presentation: never calls `resize`.
@MainActor
@Observable
final class TerminalWorkbenchModel {
    typealias WorkspaceFactory = @MainActor (WorkspaceSession) -> any TerminalSurfaceBackend
    typealias StandaloneFactory = @MainActor (WorkspaceSession, TerminalOpenRequest) async throws -> any TerminalSurfaceBackend

    private let session: WorkspaceSession
    private let makeWorkspaceBackend: WorkspaceFactory
    private let makeStandaloneBackend: StandaloneFactory
    /// Production probes the hook transcript to light the AgentChip. Tests
    /// turn it off so no HTTP poller outlives them.
    private let probesHook: Bool
    /// Whether the store already holds a live watch for a terminal. Injected so
    /// the resume fast path is testable without a host socket.
    private let isWatching: @MainActor (String) -> Bool

    /// The request the surface is showing. Kept across `back()` so returning to
    /// the workbench resumes the same terminal instead of rebuilding it.
    private(set) var request: TerminalOpenRequest?
    /// Overlay panel state. Purely presentational: toggling it never resizes.
    var panelExpanded = false
    private(set) var errorMessage: String?
    private(set) var activeBackend: (any TerminalSurfaceBackend)?
    private(set) var isOpening = false

    /// A sheet the workbench itself presents. `.files`/`.attach` leave through
    /// `onExternalAction` instead, because their views belong to other lanes.
    private(set) var sheetAction: TerminalPanelAction?
    var onExternalAction: ((TerminalPanelAction) -> Void)?
    /// Set by the shell while a sidebar inspector owns the agent surface. The
    /// `.agent` action then selects the inspector's Agent segment instead of
    /// presenting a sheet, so one terminal never runs two hook pollers.
    var onSelectInspectorAgent: (() -> Void)?

    /// Link capabilities, mirrored into the hook probe. The screen owns the
    /// environment read; the model must not guess.
    var allowsAgentView = true {
        didSet { hookProbe?.canView = allowsAgentView }
    }
    var allowsAgentInput = true {
        didSet { hookProbe?.canInput = allowsAgentInput }
    }

    @ObservationIgnored private var workspaceBackend: (any TerminalSurfaceBackend)?
    @ObservationIgnored private var standaloneBackend: (any TerminalSurfaceBackend)?
    @ObservationIgnored private var hookProbe: AgentChatStore?
    @ObservationIgnored private var hookTask: Task<Void, Never>?

    init(
        session: WorkspaceSession,
        workspaceBackend: @escaping WorkspaceFactory = { WorkspaceTerminalBackend(session: $0) },
        standaloneBackend: @escaping StandaloneFactory = { session, request in
            let backend = StandaloneTerminalBackend(session: session, request: request)
            try await backend.attach()
            return backend
        },
        probesHook: Bool = true,
        isWatching: (@MainActor (String) -> Bool)? = nil
    ) {
        self.session = session
        self.makeWorkspaceBackend = workspaceBackend
        self.makeStandaloneBackend = standaloneBackend
        self.probesHook = probesHook
        self.isWatching = isWatching ?? { session.terminals.isWatching($0) }
    }

    // MARK: - Reading the surface

    var activeTitle: String {
        activeBackend?.activeTitle ?? request?.title ?? ""
    }

    var activeCwd: String? {
        activeBackend?.activeCwd ?? request?.cwd
    }

    var activeProjectID: String? {
        activeBackend?.activeProjectID ?? request?.projectID
    }

    var items: [TerminalSurfaceItem] {
        activeBackend?.items ?? []
    }

    var activeID: String? {
        activeBackend?.activeID ?? request?.terminalID
    }

    var isStandalone: Bool {
        request?.attach.isStandalone == true
    }

    /// What the AgentChip and the agent sheet should show. Recomputes from the
    /// live stores, so the chip follows a hook that appears after opening.
    var agentSurface: TerminalAgentSurface {
        AgentChatSurfaceResolver.resolve(
            conversationID: session.workspace == .conversation ? session.conversations.active?.id : nil,
            terminalID: request?.terminalID,
            hookReady: hookProbe?.isReady == true
        )
    }

    var agentReady: Bool {
        if activeBackend?.agentReady == true { return true }
        if case .none = agentSurface { return false }
        return true
    }

    /// The single hook poller for the active terminal, shared with
    /// `AgentChatPanel` so the same terminal never has two followers.
    var hookStore: AgentChatStore? {
        hookProbe
    }

    var agentName: String? {
        switch agentSurface {
        case .acp:
            session.chat.agentLabel
        case .hook:
            hookProbe?.agent ?? String(localized: "Agent")
        case .none:
            activeBackend?.agentReady == true ? session.chat.agentLabel : nil
        }
    }

    var agentPermissionCount: Int {
        switch agentSurface {
        case .acp:
            return session.chat.permissions.count
        case .hook:
            guard let kind = hookProbe?.hookKind?.lowercased() else { return 0 }
            return kind.contains("permission") || kind.contains("approval") ? 1 : 0
        case .none:
            return 0
        }
    }

    /// What the chrome's error banner shows: a workbench failure, or the
    /// store's own last failure (watch, write, fit) for the live terminal.
    var displayError: String? {
        errorMessage ?? session.terminals.errorMessage
    }

    func clearError() {
        errorMessage = nil
        session.terminals.errorMessage = nil
    }

    var panelPresentation: TerminalPanelPresentation {
        TerminalPanelPresentation.make(
            request: request,
            items: items,
            activeID: activeBackend?.activeID ?? request?.terminalID,
            cols: canvasHost?.canvasCols,
            rows: canvasHost?.canvasRows,
            agentName: agentName,
            agentReady: agentReady,
            permissionCount: agentPermissionCount,
            subtitle: session.currentTerminalProjectName,
            fitActive: session.terminals.displayMode == .phone,
            standaloneAttached: activeBackend?.isLiveAttached ?? false
        )
    }

    var canvasHost: (any TerminalSurfaceCanvasHost)? {
        activeBackend as? any TerminalSurfaceCanvasHost
    }

    // MARK: - Opening and switching

    /// Show a terminal. Backend switches release or park the previous lease
    /// before the new one claims geometry, and never kill the shell.
    ///
    /// Returning to the surface `back()` parked is a resume, not an open: the
    /// canvas never unmounted, so re-running select/watch would re-watch the
    /// PTY from seq 0 and replay the whole scrollback into the live view.
    func open(_ request: TerminalOpenRequest) async {
        if activeBackend == nil,
           let parkedRequest = self.request,
           parkedRequest.attach == request.attach,
           case .workspace = request.attach,
           let backend = workspaceBackend,
           isWatching(request.terminalID) {
            panelExpanded = false
            errorMessage = nil
            // Keep the display metadata current without touching the surface.
            self.request = request
            activeBackend = backend
            await backend.resume()
            startHookProbe(for: request.terminalID)
            return
        }

        let previousRequest = self.request
        let previousBackend = activeBackend

        let switchesKind = previousRequest?.attach.isStandalone != request.attach.isStandalone
        if let previousRequest {
            if switchesKind {
                if previousRequest.attach.isStandalone {
                    // standalone → workspace: drop the viewer's lease first.
                    await standaloneBackend?.park()
                    standaloneBackend = nil
                } else {
                    // workspace → standalone: hand the host its desktop size
                    // before the viewer pushes its own phone fit.
                    await previousBackend?.park()
                }
            } else if previousRequest.attach.isStandalone,
                      previousRequest.terminalID != request.terminalID {
                // Different quick terminal: the old viewer owns a lease.
                await standaloneBackend?.park()
                standaloneBackend = nil
            }
        }

        panelExpanded = false
        errorMessage = nil
        isOpening = true
        defer { isOpening = false }

        switch request.attach {
        case .workspace:
            let backend = workspaceBackend ?? makeWorkspaceBackend(session)
            workspaceBackend = backend
            self.request = request
            await backend.select(request.terminalID)
            await backend.resume()
            activeBackend = backend
            startHookProbe(for: request.terminalID)
        case .standalone:
            do {
                let backend = try await makeStandaloneBackend(session, request)
                standaloneBackend = backend
                activeBackend = backend
                self.request = request
                stopHookProbe()
            } catch {
                // No surface exists: keep no request, or the panel would offer a
                // destructive "Close Terminal" for a shell this app never
                // attached to. A failed attach may still have taken a host view
                // lease, so drop that too — the PTY itself keeps running.
                activeBackend = nil
                standaloneBackend = nil
                self.request = nil
                errorMessage = error.localizedDescription
                await session.terminals.releaseViewer(for: request.terminalID)
            }
        }
    }

    /// Switch terminals inside the active surface. Selection stays in the
    /// backend (a workspace reveal, not a re-attach).
    func select(_ terminalID: String) async {
        guard let current = request, !current.attach.isStandalone,
              terminalID != current.terminalID,
              let backend = activeBackend else { return }
        await backend.select(terminalID)
        await backend.resume()
        self.request = TerminalOpenRequest(
            attach: .workspace(terminalID: terminalID),
            title: backend.activeTitle,
            cwd: backend.activeCwd,
            projectID: backend.activeProjectID,
            projectName: current.projectName
        )
        startHookProbe(for: terminalID)
    }

    /// The surface left or returned to the screen. The PTY, the request and the
    /// backend all stay; only the agent poller follows visibility, so a hidden
    /// workbench stops probing at the visible cadence.
    func setActive(_ active: Bool) {
        guard active else {
            stopHookProbe()
            return
        }
        guard activeBackend != nil, let terminalID = request?.terminalID else { return }
        startHookProbe(for: terminalID)
    }

    /// Leave the workbench. Only the viewport lease is released; the PTY and
    /// the request survive so the same surface can resume.
    func back() async {
        panelExpanded = false
        sheetAction = nil
        await activeBackend?.park()
        activeBackend = nil
        if isStandalone { standaloneBackend = nil }
        stopHookProbe()
        if let id = request?.terminalID { _ = session.takeAgentDraft(terminalID: id) }
    }

    // MARK: - Panel actions

    /// Collapse the panel and route the action. `.files`/`.attach` are owned by
    /// the shell; the workbench presents `.desktop`/`.agent`/`.more` itself.
    /// This never touches the PTY: no resize, no write, no re-attach.
    func present(_ action: TerminalPanelAction) {
        panelExpanded = false
        switch action {
        case .files, .attach:
            onExternalAction?(action)
        case .agent:
            if let onSelectInspectorAgent {
                onSelectInspectorAgent()
            } else {
                sheetAction = .agent
            }
        case .desktop, .more:
            sheetAction = action
        }
    }

    func dismissSheet() {
        sheetAction = nil
    }

    /// Type a staged path into the live PTY.
    func insertPath(_ path: String) {
        guard !path.isEmpty else { return }
        let text = DesktopShellPath.stagedArgument(path)
        guard let backend = activeBackend else {
            errorMessage = String(localized: "No live terminal.")
            return
        }
        Task { await backend.write(text) }
    }

    /// Hand a staged path to the agent composer instead of the PTY. The draft
    /// is queued on the session, so it survives a panel that is not mounted.
    func sendToAgent(_ path: String) {
        guard !path.isEmpty, let terminalID = request?.terminalID else { return }
        session.offerAgentDraft(DesktopShellPath.stagedArgument(path), terminalID: terminalID)
    }

    // MARK: - Closing

    enum CloseResult: Equatable {
        /// A quick terminal's host shell was asked to close.
        case closedQuickTerminal
        /// A workspace terminal only disconnected its view.
        case disconnectedView
    }

    /// The more sheet's close action. Quick terminals call the host close
    /// route; workspace terminals have no kill route and only detach.
    @discardableResult
    func closeActiveTerminal() async -> CloseResult? {
        guard let request else { return nil }
        panelExpanded = false
        sheetAction = nil

        if case let .standalone(terminalID, _) = request.attach {
            let attached = activeBackend?.isLiveAttached ?? false
            await standaloneBackend?.park()
            standaloneBackend = nil
            activeBackend = nil
            stopHookProbe()
            self.request = nil
            // An unattached quick terminal has no host close route: the failed
            // open already detached, and the shell keeps running.
            guard attached else { return .disconnectedView }
            let record = session.quickTerminals.records.first {
                $0.terminalId == terminalID || $0.terminalIds.contains(terminalID)
            }
            guard let record else { return .disconnectedView }
            do {
                _ = try await session.quickTerminals.closeTerminal(id: record.id, terminalId: terminalID)
                return .closedQuickTerminal
            } catch {
                errorMessage = error.localizedDescription
                return .closedQuickTerminal
            }
        }

        await activeBackend?.park()
        activeBackend = nil
        stopHookProbe()
        return .disconnectedView
    }

    // MARK: - Hook probe

    private func startHookProbe(for terminalID: String) {
        guard probesHook else { return }
        guard hookProbe?.terminalID != terminalID else {
            hookProbe?.canView = allowsAgentView
            hookProbe?.canInput = allowsAgentInput
            return
        }
        stopHookProbe()
        let store = AgentChatStore(
            terminalID: terminalID,
            client: AgentChatClient(http: session.http),
            canView: allowsAgentView,
            canInput: allowsAgentInput,
            conversationVisible: false
        )
        hookProbe = store
        hookTask = Task { await store.follow() }
    }

    private func stopHookProbe() {
        hookTask?.cancel()
        hookTask = nil
        hookProbe = nil
    }
}

// MARK: - Canvas access

/// The extra surface facts the SwiftTerm canvas needs. Kept out of
/// `TerminalSurfaceBackend` so the shared contract stays unchanged.
@MainActor
protocol TerminalSurfaceCanvasHost: AnyObject {
    var canvasTerminalID: String? { get }
    var canvasCols: Int { get }
    var canvasRows: Int { get }
    /// Grid the host reported when this surface took its replay. `nil` when no
    /// replay is in flight (a resume) or the host did not state one.
    var canvasReplayGrid: TerminalReplayGrid? { get }
    var canvasLocksToHostCols: Bool { get }
    /// `feed(data, needsReset)`: `needsReset` is true when the buffered stream
    /// starts mid-sequence (a gapped host replay) and the canvas must reset
    /// the emulator before painting it.
    func installCanvasFeed(_ feed: @escaping @MainActor (Data, Bool) -> Void)
    func sendCanvasInput(_ text: String)
    func canvasResized(cols: Int, rows: Int)
}
