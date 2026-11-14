import SwiftUI

/// Nocturne session chat (prototype `data-screen="chat"`).
///
/// Layout mirrors the prototype: a mono status ribbon, an open transcript with
/// right-aligned user pills and agent rows, inline tool/reasoning chips, and a
/// floating composer with quick prompts, a model chip and a send button. All
/// transcript, streaming, permission acknowledgement and retry behavior stays
/// owned by ChatStore; this view only renders and forwards intent.
struct ChatView: View {
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Bindable var session: WorkspaceSession
    private var allowsAgentRun: Bool { (deviceLink?.allows("agent.run") ?? true) }
    var embedded = false
    /// Optional close control in the top row (full-screen cover usage). The
    /// ribbon truncates before it instead of being overlaid by a floating
    /// ZStack button.
    var onClose: (() -> Void)? = nil
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

    private var status: NocturneSessionPresentation.Status {
        NocturneSessionPresentation.status(
            phase: session.phase,
            connected: session.acp.state == .connected,
            permissions: session.chat.permissions.count,
            questions: session.chat.questions.count,
            streaming: session.chat.messages.contains(where: \.streaming)
        )
    }

    private var modelName: String? {
        guard let models = session.chat.models,
              let id = models.currentModelId else { return nil }
        return models.availableModels?.first(where: { $0.modelId == id })?.name ?? id
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                SessionStatusRibbon(status: status, detail: ribbonDetail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let onClose {
                    Button(action: onClose) {
                        SeIconView(icon: .close, size: 13, stroke: 1.7, color: palette.inkSecondary)
                            .frame(width: 32, height: 32)
                            .background(.ultraThinMaterial, in: Circle())
                            .overlay { Circle().strokeBorder(palette.stroke, lineWidth: 1) }
                            .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(String(localized: "Close"))
                    .fixedSize()
                }
            }
            .padding(.horizontal, 18)
            .padding(.top, 12)
            messageList
            if let error = session.chat.errorMessage, !error.isEmpty {
                SeErrorBanner(message: error)
                    .frame(maxWidth: columnWidth)
                    .padding(.horizontal, SeSpace.md)
                    .padding(.bottom, SeSpace.xs)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if allowsAgentRun { composer }
        }
        .background(palette.surface)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { containerWidth = $0 }
        .seHaptic(.send, trigger: sendHaptic)
        .seHaptic(trigger: cardHapticSerial) { _, _ in cardHaptic }
        .onChange(of: session.chat.timeline, initial: true) { _, items in
            updateTimeline(items)
        }
        .onChange(of: session.chat.permissions, initial: true) { _, _ in
            updatePermissions()
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
            if shortcut == .focusChat && allowsAgentRun {
                composerFocused = true
            }
        }
        .task(id: session.chat.activeSessionId) {
            await session.chat.refreshLiveComposerSnapshot()
        }
    }

    private var ribbonDetail: String? {
        NocturneR2Presentation.ribbonDetail(
            agent: session.chat.agentLabel,
            model: modelName,
            readableFiles: NocturneR2Presentation.readableFileCount(session.files.entries)
        )
    }

    private var messageList: some View {
        ScrollViewReader { proxy in
            // Single timeline materialization per body: rows must not each
            // re-walk (and the store's timeline getter re-build) the array.
            let items = renderedTimeline
            let lastAgentId = latestAgentId(in: items)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !session.chat.hasVisibleTranscript && !session.chat.isLoading {
                        SeEmptyState(
                            title: String(localized: "Ready for your message"),
                            systemImage: "bubble.left.and.bubble.right",
                            message: String(localized: "Send a prompt below to start.")
                        )
                        .frame(maxWidth: .infinity, minHeight: 180)
                    }
                    ForEach(items) { item in
                        timelineRow(item, lastAgentId: lastAgentId)
                            .modifier(WorkspaceMessageEntrance(enabled: entranceIDs.contains(item.id)))
                            .id(item.id)
                    }
                    permissionCards
                        .disabled(!allowsAgentRun)
                    if session.chat.isLoading {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, SeSpace.lg)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 24)
                .frame(maxWidth: columnWidth)
                .frame(maxWidth: .infinity)
            }
            .background(palette.surface)
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
                SeIconView(icon: .down, size: 14, stroke: 1.8, color: palette.ink)
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
        .padding(.bottom, 120)
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
                onEdit: allowsAgentRun ? {
                    draft = message.text
                    composerFocused = true
                } : nil,
                onRetry: allowsAgentRun ? {
                    performSend { await session.chat.retry(message, in: session.conversations.active) }
                } : nil
            )
        case .agent(let message):
            MessageBubble(
                message: message,
                agentName: session.chat.agentLabel,
                onRetry: allowsAgentRun && message.id == lastAgentId
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
    private var permissionCards: some View {
        if !renderedPermissions.isEmpty || !session.chat.questions.isEmpty {
            VStack(spacing: 12) {
                ForEach(renderedPermissions) { card in
                    PermissionCardView(card: card, submission: session.chat.permissionSubmissions[card.id]) { option in
                        guard allowsAgentRun, session.chat.permissionSubmissions[card.id] != .submitting else { return }
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
                        guard allowsAgentRun, session.chat.questionSubmissions[card.id] != .submitting else { return }
                        Task {
                            if await session.chat.answer(question: card, values: [value]) {
                                cardHaptic = .success
                                cardHapticSerial += 1
                            }
                        }
                    }
                }
            }
            .padding(.top, 14)
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
        guard allowsAgentRun else { return }
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

    // MARK: - Composer (prototype `.composer`)

    private var composer: some View {
        VStack(alignment: .leading, spacing: 8) {
            quickRow
            composerBox
            HStack(spacing: 0) {
                Spacer(minLength: 0)
                SeInfoTip(
                    text: String(localized: "Only text is sent. Running commands still needs your permission."),
                    size: 22
                )
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 14)
        .padding(.bottom, 18)
        .frame(maxWidth: columnWidth)
        .frame(maxWidth: .infinity)
        .background(alignment: .top) {
            LinearGradient(
                colors: [palette.surface.opacity(0), palette.surface.opacity(0.86), palette.surface],
                startPoint: .top,
                endPoint: .center
            )
            .ignoresSafeArea()
        }
        .seAnimation(.control, value: session.chat.isSending)
    }

    private var quickRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(quickPrompts, id: \.self) { prompt in
                    Button {
                        draft = prompt
                        composerFocused = true
                    } label: {
                        Text(prompt)
                            .font(.system(size: 11))
                            .foregroundStyle(palette.inkSecondary)
                            .padding(.horizontal, 9)
                            .frame(minHeight: 30)
                            .background(palette.card, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                            .overlay {
                                RoundedRectangle(cornerRadius: 7, style: .continuous)
                                    .stroke(palette.stroke, lineWidth: 1)
                            }
                    }
                    .buttonStyle(.plain)
                    .frame(minHeight: SeSpace.hitTarget)
                }
            }
            .padding(.horizontal, 2)
        }
    }

    private var quickPrompts: [String] {
        [
            String(localized: "Keep fixing"),
            String(localized: "Run tests"),
            String(localized: "Show diff"),
            String(localized: "Try another angle")
        ]
    }

    private var composerBox: some View {
        VStack(spacing: 6) {
            TextField("Continue this session…", text: $draft, axis: .vertical)
                .textInputAutocapitalization(.sentences)
                .lineLimit(1 ... 6)
                .font(SeNocturneType.font(13))
                .foregroundStyle(palette.ink)
                .focused($composerFocused)
                .padding(.horizontal, 12)
                .padding(.top, 9)
                .frame(minHeight: 32, alignment: .topLeading)
            HStack(spacing: 6) {
                modelChip
                Spacer(minLength: 0)
                if session.chat.isSending {
                    Button {
                        Task { await session.chat.cancel() }
                    } label: {
                        SeIconView(icon: .stop, size: 13, stroke: 1.9, color: palette.onAccent)
                            .frame(width: 31, height: 31)
                            .background(palette.danger, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                            .frame(minWidth: 44, minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text("Stop response"))
                } else {
                    Button {
                        let text = draft
                        draft = ""
                        composerFocused = false
                        performSend { await session.chat.send(text, in: session.conversations.active) }
                    } label: {
                        SeIconView(icon: .send, size: 14, stroke: 1.9, color: palette.onAccent)
                            .frame(width: 31, height: 31)
                            .background(palette.accent, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                            .frame(minWidth: 44, minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                              || session.chat.isLoading || session.chat.isSwitchingAgent)
                    .opacity(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.45 : 1)
                    .accessibilityLabel(Text("Send"))
                }
            }
            .padding(.horizontal, 9)
            .padding(.bottom, 8)
        }
        .background(palette.card, in: RoundedRectangle(cornerRadius: 15, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 15, style: .continuous)
                .stroke(composerFocused ? palette.accent : palette.strokeStrong, lineWidth: 1)
        }
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: composerFocused)
    }

    @ViewBuilder
    private var modelChip: some View {
        let title = modelName ?? session.chat.agentLabel
        Button {
            session.chat.showAgentSheet = true
        } label: {
            HStack(spacing: 5) {
                Circle()
                    .fill(palette.accent)
                    .frame(width: 4, height: 4)
                Text(title)
                    .font(SeNocturneType.font(10, mono: true))
                    .lineLimit(1)
            }
            .foregroundStyle(palette.inkSecondary)
            .padding(.horizontal, 7)
            .frame(height: 24)
            .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .stroke(palette.stroke, lineWidth: 1)
            }
            .frame(minHeight: SeSpace.hitTarget)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text("Agent and model"))
    }
}
