import SwiftUI

struct ChatView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Bindable var session: WorkspaceSession
    var embedded = false
    @State private var draft = ""
    @State private var isAtBottom = true
    @State private var unreadCount = 0
    @FocusState private var composerFocused: Bool
    @State private var renderedTimeline: [ChatTimelineItem] = []
    @State private var entranceIDs: Set<String> = []
    @State private var sendRequested = false
    @State private var pendingSentIDs: Set<String> = []
    @State private var sendHaptic = 0
    @State private var acknowledgedSendIDs: Set<String> = []
    @State private var cardHapticSerial = 0
    @State private var cardHaptic: SeHaptic = .success
    @State private var renderedPermissions: [PermissionCard] = []
    @State private var containerWidth: CGFloat = 0

    private var columnWidth: CGFloat? {
        let columns = SeLayout.columns(
            width: containerWidth, isRegular: sizeClass == .regular,
            largeText: dynamicTypeSize.isAccessibilitySize
        )
        switch columns {
        case .single:
            return sizeClass == .regular ? SeLayout.readableColumnMaxWidth : nil
        case .split, .splitWithInspector:
            return SeLayout.readableColumnMaxWidth
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            messageList
            permissionStack
            if let error = session.chat.errorMessage, !error.isEmpty {
                SeErrorBanner(message: error)
                    .frame(maxWidth: columnWidth)
                    .padding(.horizontal, SeSpace.md)
                    .padding(.bottom, SeSpace.xs)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { composer }
        .background(palette.canvas)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { containerWidth = $0 }
        .seHaptic(.send, trigger: sendHaptic)
        .seHaptic(trigger: cardHapticSerial) { _, _ in cardHaptic }
        .onChange(of: session.chat.timeline, initial: true) { _, items in
            updateTimeline(items)
        }
        .onChange(of: session.chat.permissions, initial: true) { _, _ in
            updatePermissions()
        }
        .onChange(of: session.workspaceTab) { _, tab in
            if tab != .chat {
                composerFocused = false
            }
        }
        .onChange(of: composerFocused) { _, focused in
            session.chat.composerActive = focused
        }
        .onDisappear {
            // Focus-state changes do not always fire on unmount; the session
            // must not keep a sticky composer claim on the keyboard.
            session.chat.composerActive = false
        }
        .onChange(of: session.chat.activeSessionId) { _, _ in
            isAtBottom = true
            unreadCount = 0
            sendRequested = false
            entranceIDs = []
            pendingSentIDs = []
            acknowledgedSendIDs = []
            renderedPermissions = session.chat.permissions
            renderedTimeline = session.chat.timeline
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
            // Single timeline materialization per body: rows must not each
            // re-walk (and the store's timeline getter re-build) the array.
            let items = renderedTimeline
            let lastAgentId = latestAgentId(in: items)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if !session.chat.hasVisibleTranscript && !session.chat.isLoading {
                        SeEmptyState(
                            title: String(localized: "Continue the conversation"),
                            systemImage: "bubble.left.and.bubble.right",
                            message: String(localized: "Pull to refresh desktop history. Your phone continues the same host session.")
                        )
                        .frame(maxWidth: .infinity, minHeight: 180)
                    }
                    ForEach(items) { item in
                        timelineRow(item, lastAgentId: lastAgentId)
                            .modifier(WorkspaceMessageEntrance(enabled: entranceIDs.contains(item.id)))
                            .id(item.id)
                    }
                    if session.chat.isLoading {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    }
                }
                .padding(SeSpace.md)
                .frame(maxWidth: columnWidth)
                .frame(maxWidth: .infinity)
            }
            .background(palette.canvas)
            // History sessions open pinned to the newest message instead of
            // relying on scroll-to-bottom racing the first layout pass.
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                // visibleRect lives in content space, so its maxY is the
                // bottom edge of what is on screen.
                let bottomDistance = geometry.contentSize.height - geometry.visibleRect.maxY
                return bottomDistance < 60
            } action: { _, atBottom in
                if atBottom && !isAtBottom {
                    unreadCount = 0
                }
                isAtBottom = atBottom
            }
            .onChange(of: session.chat.timeline.last?.id) { _, id in
                guard !session.chat.isLoading, let id else { return }
                // Activity/tool tail churn is not a user-visible message.
                if case .activity = session.chat.timeline.last {
                    return
                }
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
                sendRequested = false
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
                if reduceMotion {
                    // Reduce Motion: jump instantly. Never animate viewport travel.
                    var transaction = Transaction()
                    transaction.disablesAnimations = true
                    withTransaction(transaction) {
                        proxy.scrollTo(id, anchor: .bottom)
                    }
                } else {
                    withAnimation(SeMotion.context.animation) {
                        proxy.scrollTo(id, anchor: .bottom)
                    }
                }
            }
            unreadCount = 0
        } label: {
            HStack(spacing: 6) {
                if unreadCount > 0 {
                    Text("\(unreadCount) new messages")
                        .font(.caption.weight(.semibold))
                        .monospacedDigit()
                }
                Image(systemName: "arrow.down")
                    .font(.subheadline.weight(.semibold))
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .foregroundStyle(palette.ink)
            .frame(minHeight: 44)
            .background(palette.cardAlt, in: Capsule())
            .overlay(
                Capsule().stroke(palette.strokeStrong, lineWidth: 1)
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
        .padding(16)
        .accessibilityLabel(Text("Jump to latest"))
        .accessibilityValue(unreadCount > 0 ? Text("\(unreadCount) new") : Text(""))
        .accessibilityHidden(isAtBottom)
        .opacity(isAtBottom ? 0 : 1)
        .allowsHitTesting(isAtBottom ? false : true)
        .seAnimation(.fade, value: isAtBottom)
        .seAnimation(.fade, value: unreadCount)
    }

    @ViewBuilder
    private func timelineRow(_ item: ChatTimelineItem, lastAgentId: String?) -> some View {
        switch item {
        case .user(let message):
            MessageBubble(
                message: message,
                onEdit: {
                    draft = message.text
                    composerFocused = true
                },
                onRetry: {
                    performSend { await session.chat.retry(message, in: session.conversations.active) }
                }
            )
        case .agent(let message):
            MessageBubble(
                message: message,
                onRetry: message.id == lastAgentId
                    ? { performSend { await session.chat.regenerateLastTurn(in: session.conversations.active) } }
                    : nil
            )
        case .activity(let activity):
            ActivityDisclosure(activity: activity)
        }
    }

    /// Regenerate re-sends the latest user turn, so only the newest agent
    /// bubble offers it — an older bubble cannot regenerate its own turn.
    private func latestAgentId(in items: [ChatTimelineItem]) -> String? {
        for item in items.reversed() {
            if case .agent(let message) = item {
                return message.id
            }
        }
        return nil
    }

    @ViewBuilder
    private var permissionStack: some View {
        if !renderedPermissions.isEmpty || !session.chat.questions.isEmpty {
            ScrollView {
                VStack(spacing: SeSpace.sm) {
                    ForEach(renderedPermissions) { card in
                        PermissionCardView(card: card, submission: session.chat.permissionSubmissions[card.id]) { option in
                            guard session.chat.permissionSubmissions[card.id] != .submitting else { return }
                            Task {
                                let acknowledged = await session.chat.respond(permission: card, optionId: option)
                                guard let feedback = WorkspaceChatPresentation.acknowledgedPermissionFeedback(
                                    optionID: option,
                                    optionName: card.options.first(where: { $0.id == option })?.name,
                                    acknowledged: acknowledged
                                ) else { return }
                                switch feedback {
                                case .approval: cardHaptic = .success
                                case .denial: cardHaptic = .warning
                                case .failure: cardHaptic = .error
                                }
                                cardHapticSerial += 1
                            }
                        }
                        .seEntrance()
                    }
                    ForEach(session.chat.questions) { card in
                        QuestionCardView(card: card, submission: session.chat.questionSubmissions[card.id]) { value in
                            guard session.chat.questionSubmissions[card.id] != .submitting else { return }
                            Task {
                                if await session.chat.answer(question: card, values: [value]) {
                                    cardHaptic = .success
                                    cardHapticSerial += 1
                                }
                            }
                        }
                    }
                }
                .padding(.horizontal, SeSpace.md)
                .padding(.bottom, SeSpace.xs)
                .frame(maxWidth: columnWidth)
                .frame(maxWidth: .infinity)
            }
            .frame(maxHeight: 300)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func updatePermissions() {
        seWithAnimation(.disclosure, reduceMotion: reduceMotion) {
            renderedPermissions = session.chat.permissions
        }
    }

    private func updateTimeline(_ items: [ChatTimelineItem]) {
        let sent = WorkspaceChatPresentation.sentMessageIDs(
            previous: renderedTimeline, current: items,
            sendRequested: sendRequested, isLoading: session.chat.isLoading
        )
        if !sent.isEmpty {
            entranceIDs.formUnion(sent)
            pendingSentIDs.formUnion(sent)
            sendRequested = false
            withAnimation(SeMotion.control.animation(reduceMotion: reduceMotion), completionCriteria: .logicallyComplete) {
                renderedTimeline = items
            } completion: {
                entranceIDs.subtract(sent)
            }
        } else {
            // No implicit animation on history, reasoning text or streaming tokens.
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction) { renderedTimeline = items }
        }
        for item in items {
            guard case .user(let message) = item,
                  pendingSentIDs.contains(message.id) else { continue }
            if message.delivery == .accepted {
                pendingSentIDs.remove(message.id)
                acknowledgeSend(message.id)
            } else if message.delivery == .failed {
                pendingSentIDs.remove(message.id)
            }
        }
    }

    private func acknowledgeSend(_ id: String) {
        guard acknowledgedSendIDs.insert(id).inserted else { return }
        sendHaptic += 1
    }

    private func performSend(_ action: @escaping @MainActor () async -> Void) {
        let existing = Set(session.chat.messages.map(\.id))
        let conversationID = session.chat.activeSessionId
        sendRequested = true
        Task {
            await action()
            guard session.chat.activeSessionId == conversationID else { return }
            // A quick host reply may coalesce .sending and .accepted into one
            // render. This completion path still confirms acceptance once.
            if let sent = session.chat.messages.last(where: { $0.role == .user }),
               !existing.contains(sent.id), sent.id.hasPrefix("turn:"), sent.delivery == .accepted {
                acknowledgeSend(sent.id)
            }
            sendRequested = false
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
                    .background(palette.cardAlt)
                    .clipShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.card).stroke(palette.strokeStrong, lineWidth: 1)
                    }
                if session.chat.isSending {
                    Button {
                        Task { await session.chat.cancel() }
                    } label: {
                        Label("Stop", systemImage: "stop.fill")
                            .font(.subheadline.weight(.semibold))
                            .frame(minWidth: 44, minHeight: 44)
                    }
                    .buttonStyle(NocturneSessionButtonStyle())
                    .accessibilityLabel(Text("Stop response"))
                } else {
                    Button {
                        let text = draft
                        draft = ""
                        composerFocused = false
                        performSend { await session.chat.send(text, in: session.conversations.active) }
                    } label: {
                        Label("Send", systemImage: "arrow.up")
                            .font(.subheadline.weight(.semibold))
                            .frame(minWidth: 44, minHeight: 44)
                    }
                    .buttonStyle(NocturneSessionButtonStyle(primary: true))
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                              || session.chat.isLoading || session.chat.isSwitchingAgent)
                    .accessibilityLabel(Text("Send"))
                }
            }
        }
        .padding(SeSpace.md)
        .frame(maxWidth: columnWidth)
        .frame(maxWidth: .infinity)
        .background(palette.canvas)
        .overlay(alignment: .top) { Rectangle().fill(palette.strokeStrong).frame(height: 1) }
        .seAnimation(.control, value: session.chat.isSending)
    }

    private var composerChips: some View {
        ViewThatFits(in: .horizontal) {
            chipRow
            ScrollView(.horizontal, showsIndicators: false) { chipRow }
        }
    }

    private var chipRow: some View {
        HStack(spacing: SeSpace.xs) {
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

    private var agentChip: some View {
        Button {
            session.chat.showAgentSheet = true
        } label: {
            chipLabel(session.chat.agentLabel, systemImage: "cpu")
        }
        .accessibilityLabel(Text("Agent"))
    }

    private func chipLabel(_ title: String, systemImage: String = "chevron.down") -> some View {
        SeChip(title: title, systemImage: systemImage)
            .frame(minHeight: SeSpace.hitTarget)
    }
}
