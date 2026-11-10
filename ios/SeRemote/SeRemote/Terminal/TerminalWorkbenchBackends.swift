import Foundation

/// Workspace terminals: `TerminalStore` owns the PTY, `WorkspaceSession` owns
/// the viewport lease. Parking hands the host its desktop size; resuming
/// re-asserts the phone fit. Neither ends the shell.
@MainActor
final class WorkspaceTerminalBackend: TerminalSurfaceBackend, TerminalSurfaceCanvasHost {
    private let session: WorkspaceSession

    init(session: WorkspaceSession) {
        self.session = session
    }

    var items: [TerminalSurfaceItem] {
        session.terminals.terminals.map { terminal in
            TerminalSurfaceItem(
                id: terminal.id,
                title: terminal.title,
                cwd: terminal.cwd,
                projectID: terminal.projectId,
                hasAgent: terminal.conversationId != nil
            )
        }
    }

    var activeID: String? { session.terminals.activeId }

    var activeTitle: String {
        activeTerminal?.title ?? String(localized: "Terminal")
    }

    var activeCwd: String? { activeTerminal?.cwd ?? session.workspaceRoot }

    var activeProjectID: String? { session.currentTerminalProjectID }

    /// ACP conversation terminals always have an agent surface. Hook-only
    /// terminals are resolved from the model's probe instead.
    var agentReady: Bool {
        session.workspace == .conversation && session.conversations.active != nil
    }

    func select(_ id: String) async {
        await session.revealTerminal(id)
    }

    func write(_ text: String) async {
        await session.terminals.write(text)
    }

    func resize(cols: Int, rows: Int) {
        session.terminals.resize(cols: cols, rows: rows)
    }

    func park() async {
        await session.parkTerminalSurface()
    }

    func resume() async {
        await session.resumeTerminalSurface()
    }

    // MARK: - Canvas

    var canvasTerminalID: String? { session.terminals.activeId }

    var canvasCols: Int { activeTerminal?.cols ?? 80 }
    var canvasRows: Int { activeTerminal?.rows ?? 24 }

    var canvasLocksToHostCols: Bool {
        session.terminals.displayMode == .desktop
    }

    /// Install the mounted canvas as the destination for this terminal's output
    /// and replay whatever the store buffered while no feed existed, in one
    /// synchronous step. Splitting the replay from the install would let output
    /// that arrives in between land in the buffer and be lost on the next mount.
    func installCanvasFeed(_ feed: @escaping (Data) -> Void) {
        guard let id = session.terminals.activeId else { return }
        session.terminals.onFeed = { incomingId, data in
            if incomingId == id { feed(data) }
        }
        if let buffered = session.terminals.consumeOutput(for: id) {
            feed(buffered)
        }
    }

    func sendCanvasInput(_ text: String) {
        Task { await session.terminals.write(text) }
    }

    func canvasResized(cols: Int, rows: Int) {
        session.terminals.resize(cols: cols, rows: rows)
    }

    private var activeTerminal: LiveTerminal? {
        session.terminals.terminals.first(where: { $0.id == session.terminals.activeId })
    }
}

/// Quick terminals: a lease-isolated `TerminalPTYViewer`. Nothing here writes
/// `TerminalStore.terminals`, so a catalog refresh cannot evict the shell and
/// the workbench's single `activeId` lease is never stolen.
@MainActor
final class StandaloneTerminalBackend: TerminalSurfaceBackend, TerminalSurfaceCanvasHost {
    private let session: WorkspaceSession
    private let request: TerminalOpenRequest
    private(set) var viewer: TerminalPTYViewer?

    init(session: WorkspaceSession, request: TerminalOpenRequest) {
        self.session = session
        self.request = request
    }

    /// Attach the already-running host PTY. No spawn, no catalog entry.
    func attach() async throws {
        guard case let .standalone(terminalID, claim) = request.attach else { return }
        viewer = try await session.terminals.attachViewer(
            terminalId: terminalID,
            claim: claim,
            title: request.title,
            cols: 80,
            rows: 24
        )
    }

    var items: [TerminalSurfaceItem] {
        [TerminalSurfaceItem(
            id: request.terminalID,
            title: request.title,
            cwd: request.cwd,
            projectID: request.projectID
        )]
    }

    var activeID: String? { request.terminalID }

    var activeTitle: String { viewer?.title ?? request.title }

    var activeCwd: String? { request.cwd }

    var activeProjectID: String? { request.projectID }

    /// True only while the viewer holds a host attach. A quick terminal whose
    /// attach failed has no close route and must not claim one.
    var isLiveAttached: Bool { viewer?.isAttached == true }

    /// Quick terminals are not bound to a conversation, so there is no hook
    /// surface to advertise.
    var agentReady: Bool { false }

    func select(_ id: String) async {
        // One quick terminal per surface; the panel omits the tab row.
    }

    func write(_ text: String) async {
        viewer?.send(text)
    }

    func resize(cols: Int, rows: Int) {
        viewer?.resize(cols: cols, rows: rows)
    }

    /// Release the display-mode lease and the view lease. Never kills the PTY.
    func park() async {
        guard let viewer else { return }
        self.viewer = nil
        await session.terminals.releaseViewer(viewer)
    }

    func resume() async {
        // The viewer stays attached while the surface is shown; a parked
        // standalone surface is re-attached through `open`.
    }

    // MARK: - Canvas

    var canvasTerminalID: String? { viewer?.terminalId ?? request.terminalID }

    var canvasCols: Int { viewer?.cols ?? 80 }
    var canvasRows: Int { viewer?.rows ?? 24 }

    var canvasLocksToHostCols: Bool { viewer?.locksToHostCols ?? false }

    func installCanvasFeed(_ feed: @escaping (Data) -> Void) {
        viewer?.install(feed)
    }

    func sendCanvasInput(_ text: String) {
        viewer?.send(text)
    }

    func canvasResized(cols: Int, rows: Int) {
        viewer?.resize(cols: cols, rows: rows)
    }
}
