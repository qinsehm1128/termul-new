import SwiftUI

struct ChatView: View {
    @Bindable var session: WorkspaceSession
    var embedded = false
    @State private var draft = ""
    @State private var isAtBottom = true
    @State private var unreadCount = 0
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
        .onReceive(ShortcutCenter.shortcuts) { shortcut in
            if shortcut == .focusChat {
                composerFocused = true
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
            .onScrollGeometryChange(for: Bool.self) { geometry in
                let bottomDistance = geometry.contentSize.height + geometry.contentOffset.y - geometry.visibleRect.height
                return bottomDistance < 60
            } action: { _, atBottom in
                if atBottom && !isAtBottom {
                    unreadCount = 0
                }
                isAtBottom = atBottom
            }
            .onChange(of: session.chat.timeline.last?.id) { _, id in
                guard !session.chat.isLoading, let id else { return }
                if isAtBottom {
                    proxy.scrollTo(id, anchor: .bottom)
                } else {
                    unreadCount += 1
                }
            }
            .onChange(of: session.chat.isLoading) { _, loading in
                guard !loading, let id = session.chat.timeline.last?.id else { return }
                if isAtBottom {
                    proxy.scrollTo(id, anchor: .bottom)
                }
            }
            .refreshable {
                if let conversation = session.conversations.active {
                    let binding = await session.conversations.binding(for: conversation)
                    await session.chat.bindConversation(conversation, binding: binding)
                }
            }
            .overlay {
                jumpToLatest(proxy: proxy)
            }
        }
    }

    /// Appears once the reader scrolls away from the newest message; the badge
    /// counts timeline items appended while away.
    private func jumpToLatest(proxy: ScrollViewProxy) -> some View {
        Button {
            if let id = session.chat.timeline.last?.id {
                withAnimation {
                    proxy.scrollTo(id, anchor: .bottom)
                }
            }
            unreadCount = 0
        } label: {
            HStack(spacing: 6) {
                if unreadCount > 0 {
                    Text("\(unreadCount)")
                        .font(.caption.weight(.semibold))
                        .monospacedDigit()
                }
                Image(systemName: "arrow.down")
                    .font(.subheadline.weight(.semibold))
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .foregroundStyle(SeTheme.ink)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(
                Capsule().stroke(SeTheme.stroke, lineWidth: 1)
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
        .padding(16)
        .accessibilityLabel(Text("Jump to latest"))
        .opacity(isAtBottom ? 0 : 1)
        .allowsHitTesting(isAtBottom ? false : true)
        .animation(.easeOut(duration: 0.15), value: isAtBottom)
    }

    @ViewBuilder
    private func timelineRow(_ item: ChatTimelineItem) -> some View {
        switch item {
        case .user(let message):
            MessageBubble(
                message: message,
                onEdit: {
                    draft = message.text
                    composerFocused = true
                },
                onRetry: {
                    Task { await session.chat.retry(message, in: session.conversations.active) }
                }
            )
        case .agent(let message):
            MessageBubble(
                message: message,
                onRetry: {
                    Task { await session.chat.regenerateLastTurn(in: session.conversations.active) }
                }
            )
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
                    .background(SeTheme.surface)
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
        .background(SeTheme.surface)
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
    var onEdit: (() -> Void)?
    var onRetry: (() -> Void)?

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
                    if message.delivery == .failed, let onRetry {
                        Button(action: onRetry) {
                            receiptView(receipt)
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint(Text("Tap to resend"))
                    } else {
                        receiptView(receipt)
                    }
                }
            }
            if message.role != .user { Spacer(minLength: 48) }
        }
        .contextMenu { messageActions }
    }

    @ViewBuilder
    private var messageActions: some View {
        Button {
            UIPasteboard.general.string = message.text
        } label: {
            Label(String(localized: "Copy"), systemImage: "doc.on.doc")
        }
        if message.role == .user {
            if let onEdit {
                Button(action: onEdit) {
                    Label(String(localized: "Edit & Resend"), systemImage: "square.and.pencil")
                }
            }
            if message.delivery == .failed, let onRetry {
                Button(action: onRetry) {
                    Label(String(localized: "Resend"), systemImage: "arrow.up.circle")
                }
            }
        } else if let onRetry {
            Button(action: onRetry) {
                Label(String(localized: "Regenerate"), systemImage: "arrow.clockwise")
            }
        }
        ShareLink(item: message.text) {
            Label(String(localized: "Share"), systemImage: "square.and.arrow.up")
        }
    }

    private func receiptView(_ receipt: Receipt) -> some View {
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

    private var background: Color {
        message.role == .user ? SeTheme.accent.opacity(0.16) : SeTheme.surface
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
                        .tint(SeTheme.accent)
                }
                Button("Deny") { onChoose(nil) }
                    .buttonStyle(.bordered)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SeTheme.surface)
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
        .background(SeTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}
