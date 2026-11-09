import SwiftUI

/// The terminal is always the same child in the same ZStack slot. Changing the
/// presentation cannot remount SwiftTerm, reconnect its socket, or kill its PTY.
struct TerminalAgentChatContainer<Terminal: View>: View {
    @State private var store: AgentChatStore
    @AppStorage private var showConversation: Bool
    @Environment(\.sePalette) private var palette
    private let terminalID: String
    private let http: HostHTTP
    private let canView: Bool
    private let canInput: Bool
    private let dismissTerminalKeyboard: () -> Void
    private let terminal: Terminal

    init(
        terminalID: String,
        http: HostHTTP,
        canView: Bool,
        canInput: Bool,
        dismissTerminalKeyboard: @escaping () -> Void,
        @ViewBuilder terminal: () -> Terminal
    ) {
        _store = State(initialValue: AgentChatStore(
            terminalID: terminalID, client: AgentChatClient(http: http),
            canView: canView, canInput: canInput
        ))
        _showConversation = AppStorage(wrappedValue: false, "agent-chat.presentation.\(http.origin.absoluteString).\(terminalID)")
        self.terminalID = terminalID
        self.http = http
        self.canView = canView
        self.canInput = canInput
        self.dismissTerminalKeyboard = dismissTerminalKeyboard
        self.terminal = terminal()
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Picker("Terminal presentation", selection: $showConversation) {
                    Text("Terminal").tag(false)
                    Text("Conversation").tag(true)
                }
                .pickerStyle(.segmented)
                .frame(maxWidth: 300)
                .accessibilityIdentifier("terminal-agent-chat-toggle")
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .padding(.horizontal, 12)
            .background(palette.canvas)
            ZStack {
                VStack(spacing: 0) { terminal }
                    .opacity(showConversation ? 0 : 1)
                    .allowsHitTesting(!showConversation)
                    .accessibilityHidden(showConversation)
                if showConversation {
                    SessionChatView(agentChat: store)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .task(id: "\(http.origin.absoluteString)|\(terminalID)") {
            let current = AgentChatStore(
                terminalID: terminalID, client: AgentChatClient(http: http),
                canView: canView, canInput: canInput
            )
            store = current
            await current.follow()
        }
        .onAppear { if showConversation { dismissTerminalKeyboard() } }
        .onChange(of: showConversation) { _, _ in dismissTerminalKeyboard() }
        .onChange(of: canView) { _, value in
            store.canView = value
            if !value { Task { await store.refresh() } }
        }
        .onChange(of: canInput) { _, value in store.canInput = value }
    }
}
