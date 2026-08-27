import SwiftUI

struct ChatView: View {
    @Bindable var session: WorkspaceSession
    var embedded = false
    @State private var draft = ""
    @FocusState private var composerFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            messageList
            permissionStack
            if let error = session.chat.errorMessage, !error.isEmpty {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
            composer
        }
        .onChange(of: session.workspaceTab) { _, tab in
            if tab != .chat {
                composerFocused = false
            }
        }
        .task(id: session.chat.activeSessionId) {
            await session.chat.refreshLiveComposerSnapshot()
        }
    }

    private var messageList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if !session.chat.hasVisibleTranscript && !session.chat.isLoading {
                        ContentUnavailableView(
                            "No messages yet",
                            systemImage: "bubble.left",
                            description: Text("If this session has desktop history, pull to refresh. The phone continues the computer session instead of starting a new one.")
                        )
                        .frame(maxWidth: .infinity, minHeight: 180)
                    }
                    ForEach(session.chat.timeline) { item in
                        timelineRow(item)
                            .id(item.id)
                    }
                    if session.chat.isLoading {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    }
                }
                .padding(16)
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: session.chat.timeline.last?.id) { _, id in
                guard !session.chat.isLoading, let id else { return }
                proxy.scrollTo(id, anchor: .bottom)
            }
            .onChange(of: session.chat.isLoading) { _, loading in
                guard !loading, let id = session.chat.timeline.last?.id else { return }
                proxy.scrollTo(id, anchor: .bottom)
            }
            .refreshable {
                if let conversation = session.conversations.active {
                    let binding = await session.conversations.binding(for: conversation)
                    await session.chat.bindConversation(conversation, binding: binding)
                }
            }
        }
    }

    @ViewBuilder
    private func timelineRow(_ item: ChatTimelineItem) -> some View {
        switch item {
        case .user(let message), .agent(let message):
            MessageBubble(message: message)
        case .activity(let activity):
            ActivityDisclosure(activity: activity)
        }
    }

    @ViewBuilder
    private var permissionStack: some View {
        if !session.chat.permissions.isEmpty || !session.chat.questions.isEmpty {
            VStack(spacing: 10) {
                ForEach(session.chat.permissions) { card in
                    PermissionCardView(card: card) { option in
                        Task { await session.chat.respond(permission: card, optionId: option) }
                    }
                }
                ForEach(session.chat.questions) { card in
                    QuestionCardView(card: card) { value in
                        Task { await session.chat.answer(question: card, values: [value]) }
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 10) {
            composerChips
            HStack(alignment: .bottom, spacing: 10) {
                TextField("Message the host agent", text: $draft, axis: .vertical)
                    .textInputAutocapitalization(.sentences)
                    .lineLimit(1 ... 6)
                    .focused($composerFocused)
                    .padding(12)
                    .background(TermulTheme.surface)
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                if session.chat.isSending {
                    Button {
                        Task { await session.chat.cancel() }
                    } label: {
                        Image(systemName: "stop.fill")
                            .frame(width: 44, height: 44)
                    }
                    .accessibilityLabel(Text("Cancel"))
                } else {
                    Button {
                        let text = draft
                        draft = ""
                        composerFocused = false
                        Task { await session.chat.send(text, in: session.conversations.active) }
                    } label: {
                        Image(systemName: "arrow.up.circle.fill")
                            .font(.title)
                            .frame(width: 44, height: 44)
                    }
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .accessibilityLabel(Text("Send"))
                }
            }
        }
        .padding(16)
        .background(.ultraThinMaterial)
    }

    private var composerChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                agentChip
                if let modes = session.chat.modes?.availableModes, !modes.isEmpty {
                    Menu {
                        ForEach(modes) { mode in
                            Button {
                                Task { await session.chat.setMode(mode.id) }
                            } label: {
                                if mode.id == session.chat.modes?.currentModeId {
                                    Label(mode.name, systemImage: "checkmark")
                                } else {
                                    Text(mode.name)
                                }
                            }
                        }
                    } label: {
                        chipLabel(session.chat.modes?.availableModes?.first(where: { $0.id == session.chat.modes?.currentModeId })?.name ?? String(localized: "Mode"))
                    }
                    .accessibilityLabel(Text("Mode"))
                }
                if let models = session.chat.models?.availableModels, !models.isEmpty {
                    Menu {
                        ForEach(models) { model in
                            Button {
                                Task { await session.chat.setModel(model.modelId) }
                            } label: {
                                if model.modelId == session.chat.models?.currentModelId {
                                    Label(model.name, systemImage: "checkmark")
                                } else {
                                    Text(model.name)
                                }
                            }
                        }
                    } label: {
                        chipLabel(session.chat.models?.availableModels?.first(where: { $0.modelId == session.chat.models?.currentModelId })?.name ?? String(localized: "Model"))
                    }
                    .accessibilityLabel(Text("Model"))
                }
                if let thought = session.chat.thoughtOption, let values = thought.options, !values.isEmpty {
                    Menu {
                        ForEach(values) { value in
                            Button {
                                Task { await session.chat.setConfig(optionId: thought.id, valueId: value.value) }
                            } label: {
                                if value.value == thought.currentValue {
                                    Label(value.name, systemImage: "checkmark")
                                } else {
                                    Text(value.name)
                                }
                            }
                        }
                    } label: {
                        chipLabel(values.first(where: { $0.value == thought.currentValue })?.name ?? thought.name)
                    }
                    .accessibilityLabel(Text("Thinking"))
                }
            }
        }
    }

    private var agentChip: some View {
        Button {
            session.chat.showAgentSheet = true
        } label: {
            chipLabel(session.chat.agentLabel, systemImage: "cpu")
        }
        .accessibilityLabel(Text("Agent"))
    }

    private func chipLabel(_ title: String, systemImage: String = "chevron.up.chevron.down") -> some View {
        HStack(spacing: 6) {
            if systemImage == "cpu" {
                Image(systemName: "cpu")
            }
            Text(title)
                .lineLimit(1)
            Image(systemName: "chevron.up.chevron.down")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .font(.subheadline.weight(.medium))
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(TermulTheme.surface)
        .clipShape(Capsule())
    }
}

private struct ActivityDisclosure: View {
    let activity: TurnActivity
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                expanded.toggle()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    Text(activity.summary)
                        .font(.subheadline.weight(.medium))
                    Spacer()
                }
                .foregroundStyle(activity.attentionRequired ? Color.red : Color.secondary)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(activity.summary))
            .accessibilityHint(Text(expanded ? "Collapse" : "Expand"))

            if expanded {
                if !activity.thoughts.isEmpty {
                    ThoughtBlock(messages: activity.thoughts)
                }
                if !activity.tools.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(activity.tools) { tool in
                            HStack(spacing: 8) {
                                Image(systemName: "wrench.and.screwdriver")
                                    .font(.caption)
                                Text(tool.title)
                                    .lineLimit(1)
                                Spacer()
                                Text(tool.status.replacingOccurrences(of: "_", with: " "))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.leading, 18)
                }
            }
        }
        .padding(.vertical, 2)
    }
}

private struct ThoughtBlock: View {
    let messages: [ChatMessage]
    @State private var expanded = false

    private var text: String {
        messages.map(\.text).joined()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                expanded.toggle()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    Text("Reasoning")
                    Spacer()
                }
                .font(.footnote.weight(.medium))
                .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            if expanded {
                Text(text)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .padding(.leading, 16)
            }
        }
        .padding(.leading, 18)
    }
}

private struct MessageBubble: View {
    let message: ChatMessage

    var body: some View {
        HStack {
            if message.role == .user { Spacer(minLength: 48) }
            VStack(alignment: message.role == .user ? .trailing : .leading, spacing: 4) {
                VStack(alignment: message.role == .user ? .trailing : .leading, spacing: 6) {
                    Text(attributed)
                        .font(.body)
                        .textSelection(.enabled)
                    if message.streaming {
                        ProgressView()
                            .controlSize(.mini)
                    }
                }
                .padding(12)
                .background(background)
                .foregroundStyle(.primary)
                .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                if let receipt {
                    HStack(spacing: 4) {
                        if receipt.showsProgress {
                            ProgressView()
                                .controlSize(.mini)
                        } else {
                            Image(systemName: receipt.symbol)
                        }
                        Text(receipt.label)
                    }
                    .font(.caption2)
                    .foregroundStyle(receipt.tint)
                    .padding(.horizontal, 4)
                    .accessibilityLabel(Text(receipt.label))
                }
            }
            if message.role != .user { Spacer(minLength: 48) }
        }
    }

    private var background: Color {
        message.role == .user ? TermulTheme.accent.opacity(0.16) : TermulTheme.surface
    }

    private var receipt: Receipt? {
        guard message.role == .user, let delivery = message.delivery else { return nil }
        switch delivery {
        case .sending:
            return Receipt(
                symbol: "arrow.up.circle",
                label: String(localized: "Sending…"),
                tint: .secondary,
                showsProgress: true
            )
        case .accepted:
            return Receipt(
                symbol: "checkmark",
                label: String(localized: "Host received"),
                tint: .secondary,
                showsProgress: false
            )
        case .failed:
            return Receipt(
                symbol: "exclamationmark.circle",
                label: String(localized: "Not received"),
                tint: .red,
                showsProgress: false
            )
        }
    }

    private var attributed: AttributedString {
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .full
        options.failurePolicy = .returnPartiallyParsedIfPossible
        return (try? AttributedString(markdown: message.text, options: options))
            ?? AttributedString(message.text)
    }
}

private struct Receipt {
    var symbol: String
    var label: String
    var tint: Color
    var showsProgress: Bool
}

private struct PermissionCardView: View {
    let card: PermissionCard
    var onChoose: (String?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Need you to approve")
                .font(.headline)
            Text(card.title)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            HStack {
                ForEach(card.options) { option in
                    Button(option.name) { onChoose(option.id) }
                        .buttonStyle(.borderedProminent)
                        .tint(TermulTheme.accent)
                }
                Button("Deny") { onChoose(nil) }
                    .buttonStyle(.bordered)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(TermulTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

private struct QuestionCardView: View {
    let card: QuestionCard
    var onChoose: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(card.question)
                .font(.headline)
            ForEach(card.options) { option in
                Button(option.label) { onChoose(option.id) }
                    .buttonStyle(.bordered)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(TermulTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}
