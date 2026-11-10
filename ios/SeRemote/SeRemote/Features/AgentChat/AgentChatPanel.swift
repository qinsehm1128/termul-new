import SwiftUI

/// Which agent surface a terminal can show.
///
/// - `.acp`: the terminal belongs to an active ACP conversation, so the panel
///   reuses `ChatView` with its permission and question cards.
/// - `.hook`: the terminal has a hook transcript (`AgentChatView`).
/// - `.none`: no agent is attached; the AgentChip and panel row hide.
nonisolated enum TerminalAgentSurface: Equatable, Sendable {
    case acp(conversationID: String)
    case hook(terminalID: String)
    case none(AgentChatAbsence)
}

nonisolated enum AgentChatSurfaceResolver {
    static func resolve(
        conversationID: String?,
        terminalID: String?,
        hookReady: Bool
    ) -> TerminalAgentSurface {
        if let conversationID, !conversationID.isEmpty {
            return .acp(conversationID: conversationID)
        }
        guard let terminalID, !terminalID.isEmpty else { return .none(.unsupported) }
        return hookReady ? .hook(terminalID: terminalID) : .none(.noHook)
    }
}

/// The terminal's agent panel. One surface, resolved per terminal, consuming
/// the session's queued drafts so a path staged from the files panel or the
/// desktop sheet always lands in the composer.
struct AgentChatPanel: View {
    @Bindable var session: WorkspaceSession
    let terminalID: String
    /// The workbench's shared hook probe. Sharing it keeps exactly one poller
    /// per terminal; the panel only creates its own when none was handed in.
    var hookStore: AgentChatStore?

    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette
    @State private var ownStore: AgentChatStore?
    @State private var pendingDraft: String?

    private var canView: Bool { deviceLink?.allows("view") ?? true }
    private var canInput: Bool { deviceLink?.allows("terminal.input") ?? true }

    private var surface: TerminalAgentSurface {
        AgentChatSurfaceResolver.resolve(
            conversationID: session.workspace == .conversation ? session.conversations.active?.id : nil,
            terminalID: terminalID,
            hookReady: (hookStore ?? ownStore)?.isReady == true
        )
    }

    var body: some View {
        Group {
            switch surface {
            case .acp:
                acpSurface
            case .hook:
                if let store = hookStore ?? ownStore {
                    AgentChatView(store: store)
                } else {
                    ProgressView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(palette.canvas)
                }
            case let .none(absence):
                absenceView(absence)
            }
        }
        .task(id: terminalID) { await prepare() }
        .onAppear {
            (hookStore ?? ownStore)?.setConversationVisible(true)
            consumeDraft()
        }
        .onDisappear {
            (hookStore ?? ownStore)?.setConversationVisible(false)
        }
    }

    // MARK: - ACP

    private var acpSurface: some View {
        VStack(spacing: 0) {
            if let pendingDraft {
                HStack(spacing: 8) {
                    SeIconView(icon: .doc, size: 14, stroke: 1.6, color: palette.inkSecondary)
                    Text(pendingDraft)
                        .font(SeNocturneType.font(10, mono: true))
                        .foregroundStyle(palette.inkSecondary)
                        .lineLimit(2)
                    Spacer(minLength: 4)
                    Button {
                        let text = pendingDraft
                        self.pendingDraft = nil
                        Task { await session.chat.send(text) }
                    } label: {
                        Text("Send")
                            .font(SeNocturneType.font(11, weight: .semibold))
                            .foregroundStyle(palette.onAccent)
                            .padding(.horizontal, 10)
                            .frame(minHeight: 30)
                            .background(palette.accent, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .accessibilityLabel(Text("Send to agent"))
                    Button {
                        self.pendingDraft = nil
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(palette.inkTertiary)
                            .frame(width: 28, height: 28)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .accessibilityLabel(Text("Dismiss draft"))
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(palette.card)
                .overlay(alignment: .bottom) {
                    Rectangle().fill(palette.stroke).frame(height: 1)
                }
            }
            ChatView(session: session, embedded: false)
        }
    }

    // MARK: - Absence

    private func absenceView(_ absence: AgentChatAbsence) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(absence == .noHook ? "No agent conversation" : "No agent here")
                .font(SeNocturneType.headline)
                .foregroundStyle(palette.ink)
            Text(absence == .noHook
                 ? "Start a supported agent in this terminal to see its conversation."
                 : "This terminal is not bound to an agent conversation.")
                .font(SeNocturneType.body)
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(20)
        .background(palette.canvas)
        .accessibilityIdentifier("agent-panel-absence")
    }

    // MARK: - Lifecycle

    private func prepare() async {
        consumeDraft()
        guard hookStore == nil, ownStore == nil else { return }
        if case .acp = surface { return }
        let store = AgentChatStore(
            terminalID: terminalID,
            client: AgentChatClient(http: session.http),
            canView: canView,
            canInput: canInput,
            conversationVisible: true
        )
        ownStore = store
        store.setConversationVisible(true)
        consumeDraft()
        await store.follow()
    }

    private func consumeDraft() {
        guard let draft = session.takeAgentDraft(terminalID: terminalID) else { return }
        if let store = hookStore ?? ownStore {
            store.draft = draft
        } else {
            pendingDraft = draft
        }
    }
}
