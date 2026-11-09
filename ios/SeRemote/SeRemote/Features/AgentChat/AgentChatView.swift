import SwiftUI

/// Hooked conversations reuse the ACP transcript rows, but never its transport or session store.
struct AgentChatView: View {
    @Bindable var store: AgentChatStore
    @Environment(\.sePalette) private var palette
    @FocusState private var composerFocused: Bool
    @State private var confirmStop = false
    @State private var isNearBottom = true

    var body: some View {
        VStack(spacing: 0) {
            transcript
            if !store.isReady && !store.isLoading && !store.messages.isEmpty {
                Text("Conversation is degraded. Use the terminal to continue.")
                    .font(.footnote).foregroundStyle(palette.inkSecondary)
                    .padding(.horizontal, 16)
            }
            if let error = store.errorMessage {
                HStack {
                    Text(error).font(.footnote).foregroundStyle(palette.danger)
                    Spacer(minLength: 4)
                    Button("Retry") { Task { await store.refresh() } }
                        .frame(minWidth: 44, minHeight: 44)
                }
                .padding(.horizontal, 16)
            }
            if !store.canInput && store.canView {
                Text("Read-only link — use the terminal on the host to reply.")
                    .font(.footnote).foregroundStyle(palette.inkSecondary)
                    .padding(16)
            } else if store.canView {
                composer
            }
        }
        .background(palette.canvas)
        .confirmationDialog("Stop this agent?", isPresented: $confirmStop, titleVisibility: .visible) {
            Button("Stop agent", role: .destructive) { Task { await store.control(.stop) } }
        } message: {
            Text("This stops the agent, not the terminal. The terminal remains available.")
        }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if store.messages.isEmpty {
                        emptyState
                    }
                    ForEach(ChatTimeline.build(messages: store.messages, tools: store.toolCalls.map(\.card), activeTurn: false)) { item in
                        switch item {
                        case .user(let message), .agent(let message):
                            MessageBubble(message: message, agentName: store.agent, sourceLabel: "HOOK")
                                .id(message.id)
                        case .activity(let activity):
                            ActivityDisclosure(activity: activity)
                                .id(activity.id)
                        }
                    }
                    ForEach(store.toolCalls.filter { !$0.detail.isEmpty }) { tool in
                        DisclosureGroup(tool.title) {
                            Text(tool.detail).font(.footnote.monospaced()).textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .font(.footnote)
                        .foregroundStyle(palette.inkSecondary)
                    }
                    ForEach(store.deliveries) { delivery in
                        VStack(alignment: .trailing, spacing: 4) {
                            // Deliveries are receipts, not fabricated transcript user messages.
                            Text(delivery.text).textSelection(.enabled)
                            Text(delivery.label).font(.caption).foregroundStyle(palette.inkSecondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .padding(12)
                        .background(palette.surface, in: RoundedRectangle(cornerRadius: 12))
                    }
                    Color.clear.frame(height: 1).id("agent-chat-bottom")
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentOffset.y + geometry.containerSize.height >= geometry.contentSize.height - 80
            } action: { _, value in
                isNearBottom = value
            }
            .onChange(of: store.cursor) { _, _ in
                if isNearBottom { proxy.scrollTo("agent-chat-bottom", anchor: .bottom) }
            }
        }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            if store.isLoading {
                ProgressView("Loading conversation…")
            } else if !store.canView {
                Text("This link does not allow viewing terminal conversations.")
            } else if store.isReady {
                Text("No conversation yet").font(.headline)
                Text("Messages appear here when the agent writes its transcript.")
            } else {
                Text("Conversation unavailable").font(.headline)
                Text("Start a supported agent with hooks enabled, or return to the terminal.")
            }
            if let reason = store.reason, !reason.isEmpty {
                Text(reason).font(.footnote).textSelection(.enabled)
            }
        }
        .foregroundStyle(palette.inkSecondary)
        .frame(maxWidth: .infinity, minHeight: 160, alignment: .leading)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let kind = store.hookKind,
               kind.localizedCaseInsensitiveContains("permission") || kind.localizedCaseInsensitiveContains("approval") {
                Text("Approval required — continue in the terminal.")
                    .font(.footnote).foregroundStyle(palette.inkSecondary)
            }
            if let notice = store.submissionNotice, store.deliveries.isEmpty {
                Text(notice).font(.caption).foregroundStyle(palette.inkSecondary)
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Message agent…", text: $store.draft, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($composerFocused)
                    .disabled(!store.isReady || store.isSubmitting)
                    .padding(12)
                    .background(palette.surface, in: RoundedRectangle(cornerRadius: 12))
                    .accessibilityIdentifier("agent-chat-composer")
                Button {
                    Task { await store.send() }
                } label: {
                    Image(systemName: "arrow.up")
                        .frame(width: 44, height: 44)
                        .foregroundStyle(store.canSend ? palette.canvas : palette.inkTertiary)
                        .background(store.canSend ? palette.ink : palette.surface, in: Circle())
                }
                .disabled(!store.canSend)
                .accessibilityLabel(Text("Send message"))
                .accessibilityIdentifier("agent-chat-send")
            }
            HStack {
                Button("Interrupt", systemImage: "pause.fill") { Task { await store.control(.interrupt) } }
                    .disabled(!store.canControl)
                    .frame(minHeight: 44)
                Spacer()
                Button("Stop agent", systemImage: "stop.fill", role: .destructive) { confirmStop = true }
                    .disabled(!store.canControl)
                    .frame(minHeight: 44)
                Button {
                    composerFocused = false
                } label: {
                    Image(systemName: "keyboard.chevron.compact.down").frame(width: 44, height: 44)
                }
                .accessibilityLabel(Text("Dismiss keyboard"))
            }
            .font(.footnote)
            .foregroundStyle(palette.inkSecondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(palette.canvas)
    }
}
