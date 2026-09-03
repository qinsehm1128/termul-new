import Foundation
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
    let chat = ChatStore()
    let conversations = ConversationStore()
    let projects = ProjectStore()
    let files = FileStore()
    let terminals = TerminalStore()

    /// Chat sits on a Conversation. A project workspace is terminals only.
    var showChat = true
    var workspaceTab: WorkspaceTab = .chat
    var workspace: WorkspaceKind = .home
    var phase: SessionPhase = .idle
    /// Software keyboard is covering the terminal tab. Chrome hides; host PTY stays put.
    var terminalKeyboardVisible = false
    var terminalKeyboardHeight: CGFloat = 0
    var terminalBlurToken: UInt64 = 0
    private var isStarting = false
    private var startEpoch = 0

    init(accessURL: URL, bearer: String? = nil) {
        credentials = HostCredentials(accessURL: accessURL, bearer: bearer)
        origin = credentials.origin
        http = HostHTTP(origin: origin, credentials: credentials)
        chat.attach(socket: acp)
        chat.onHostConversationsChanged = { [weak self] in
            guard let self else { return }
            Task { await self.conversations.refresh() }
        }
        conversations.attach(http: http)
        projects.attach(http: http, socket: acp)
        files.attach(http: http)
        terminals.attach(socket: terminalSocket, origin: origin, credentials: credentials)
    }

    func start() async {
        guard !isStarting else { return }
        startEpoch += 1
        let epoch = startEpoch
        isStarting = true
        defer {
            if startEpoch == epoch {
                isStarting = false
            }
        }
        phase = .connecting
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
            do {
                try await terminals.ensureConnected()
            } catch {
                terminals.errorMessage = error.localizedDescription
            }
            guard startEpoch == epoch else { return }
            phase = .connected
            HostLog.session.info("Host connect succeeded")
        } catch is CancellationError {
            HostLog.session.info("Host connect cancelled")
        } catch {
            guard startEpoch == epoch, !Task.isCancelled else { return }
            HostLog.session.error("Host connect failed after restart-safe retries")
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
        acp.stop()
        terminalSocket.stop()
    }

    func handleScene(isBackground: Bool) {
        acp.handleLifecycle(isBackground: isBackground)
    }

    func selectConversation(_ conversation: HostConversation) async {
        projects.clearSelection()
        workspace = .conversation
        setWorkspaceTab(.chat)
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
        setWorkspaceTab(.terminal)
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
        setWorkspaceTab(.chat)
    }

    func leaveConversation() {
        leaveWorkspace()
    }

    func revealTerminal(_ terminalId: String) async {
        setWorkspaceTab(.terminal)
        await terminals.open(terminalId)
    }

    func revealChat() {
        setWorkspaceTab(.chat)
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
        let visible = workspaceTab == .terminal && height > 40
        terminalKeyboardHeight = visible ? height : 0
        terminalKeyboardVisible = visible
        terminals.suppressHostResize = visible
    }

    func setWorkspaceTab(_ tab: WorkspaceTab) {
        guard workspaceTab != tab else { return }
        dismissTerminalKeyboard()
        if tab != .terminal {
            terminals.geometryActive = false
            let terminalId = terminals.activeId
            Task { await terminals.releaseDisplayMode(for: terminalId) }
        }
        HostLog.session.info("Workspace tab \(tab.rawValue, privacy: .public)")
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(80))
            workspaceTab = tab
            showChat = tab == .chat
            terminals.geometryActive = tab == .terminal
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
