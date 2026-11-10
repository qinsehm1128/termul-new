import SwiftUI

/// The terminal is always the same child in the same ZStack slot. Changing the
/// presentation cannot remount SwiftTerm, reconnect its socket, or kill its PTY.
struct TerminalAgentChatContainer<Terminal: View>: View {
    @State private var store: AgentChatStore
    @AppStorage private var showConversation: Bool
    @Environment(\.sePalette) private var palette
    @State private var desktopSocket = DesktopSocket()
    @State private var showDesktop = false
    @State private var desktopSupported: Bool?
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
                if desktopSupported != false {
                    Button {
                        showDesktop = true
                    } label: {
                        SeIconView(icon: .desktop, size: 14, stroke: 1.7, color: palette.inkSecondary)
                            .frame(width: 30, height: 30)
                            .background(palette.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .frame(minWidth: 44, minHeight: 44)
                    .accessibilityLabel(Text("Desktop"))
                    .accessibilityHint(Text("Opens the remote desktop for this agent conversation."))
                }
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
        .sheet(isPresented: $showDesktop) {
            DesktopQuickSheet(
                hostName: String(localized: "Desktop"),
                socket: desktopSocket,
                sender: DesktopShotSender(
                    http: http,
                    draftAgentChat: { terminalID, text in
                        AgentChatDraftInbox.offer(text, terminalID: terminalID)
                    }
                ),
                target: .agentChat(id: terminalID)
            )
        }
        .task {
            guard desktopSupported == nil else { return }
            desktopSupported = (try? await http.desktopInfo())?.supported
        }
        .onReceive(NotificationCenter.default.publisher(for: .agentChatDraft)) { note in
            guard note.object as? String == terminalID,
                  let text = note.userInfo?["text"] as? String else { return }
            store.draft = text
            showConversation = true
            dismissTerminalKeyboard()
        }
    }
}
