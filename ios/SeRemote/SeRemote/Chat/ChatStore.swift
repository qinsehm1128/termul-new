import Foundation
import Observation

enum ChatRole: String, Codable, Sendable {
    case user
    case agent
    case thought
}

enum MessageDelivery: String, Codable, Sendable {
    case sending
    case accepted
    case failed
}

struct ChatMessage: Identifiable, Hashable, Codable, Sendable {
    var id: String
    var role: ChatRole
    var text: String
    var streaming: Bool
    var seq: UInt64?
    var delivery: MessageDelivery? = nil
}

struct ToolCard: Identifiable, Hashable, Codable, Sendable {
    let id: String
    var title: String
    var status: String
    var seq: UInt64?

    var isBusy: Bool {
        status == "pending" || status == "in_progress"
    }

    var needsAttention: Bool {
        isBusy || status == "failed"
    }
}

struct PermissionCard: Identifiable, Hashable, Sendable {
    let id: String
    var agentId: String
    var title: String
    var options: [PermissionChoice]
}

struct PermissionChoice: Identifiable, Hashable, Sendable {
    var id: String
    var name: String
}

struct QuestionCard: Identifiable, Hashable, Sendable {
    let id: String
    var agentId: String
    var question: String
    var options: [QuestionChoice]
}

struct QuestionChoice: Identifiable, Hashable, Sendable {
    var id: String
    var label: String
}

@MainActor
@Observable
final class ChatStore {
    var sessions: [PersistedSession] = []
    var activeSessionId: String?
    var activeAgentId: String?
    var activeCwd: String = ""
    var messages: [ChatMessage] = []
    var tools: [ToolCard] = []
    var permissions: [PermissionCard] = []
    var questions: [QuestionCard] = []
    var isSending = false
    var isLoading = false
    var isSwitchingAgent = false
    var switchingStatus: String?
    var errorMessage: String?
    var lastSeq: [String: UInt64] = [:]
    var boundConversationId: String?
    var currentBinding: AgentSessionBinding?
    var catalogAgents: [CatalogAgent] = []
    var catalogHost: CatalogHost?
    var liveAgentIds: [String] = []
    var modes: SessionModeState?
    var models: SessionModelState?
    var configOptions: [SessionConfigOption] = []
    var showAgentSheet = false

    var agentLabel: String {
        if let mode = modes?.availableModes?.first(where: { $0.id == modes?.currentModeId }) {
            return mode.name
        }
        if let model = models?.availableModels?.first(where: { $0.modelId == models?.currentModelId }) {
            return model.name
        }
        if let catalog = catalogAgents.first(where: {
            AcpSpawnConfig.isCurrent($0, activeAgentId: activeAgentId, binding: currentBinding)
        }) {
            return catalog.name
        }
        return catalogAgents.first(where: { $0.status == "ready" })?.name
            ?? String(localized: "Agent")
    }

    var timeline: [ChatTimelineItem] {
        ChatTimeline.build(
            messages: messages,
            tools: tools,
            activeTurn: isSending
                || messages.contains { $0.streaming }
                || tools.contains { $0.isBusy }
        )
    }

    var hasVisibleTranscript: Bool {
        !messages.isEmpty || !tools.isEmpty
    }

    private weak var socket: AcpSocket?
    private var persistTask: Task<Void, Never>?
    var onHostConversationsChanged: (@MainActor () -> Void)?

    func attach(socket: AcpSocket) {
        self.socket = socket
        socket.onEvent = { [weak self] type, sid, seq, payload in
            self?.handle(type: type, sid: sid, seq: seq, payload: payload)
        }
    }

    func refreshSessions() async {
        guard let socket else { return }
        do {
            let data = try await socket.request("list_persisted_sessions")
            sessions = Self.decodeSessions(data)
        } catch let error as HostError {
            if case .ipc(_, let code) = error, code == "unsupported" {
                sessions = []
                return
            }
            errorMessage = error.localizedDescription
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func refreshCatalog() async {
        guard let socket else { return }
        do {
            let catalog = try await socket.request("list_acp_catalog", as: AcpCatalog.self)
            catalogAgents = catalog.agents
            catalogHost = catalog.host
        } catch {
            HostLog.session.error("ACP catalog refresh failed")
        }
    }

    func refreshLiveAgents() async {
        guard let socket else { return }
        do {
            liveAgentIds = try await socket.request("list_agents", as: [String].self)
        } catch {
            liveAgentIds = []
        }
    }

    func canSelect(_ agent: CatalogAgent) -> Bool {
        AcpSpawnConfig.canSelect(agent, liveAgentIds: liveAgentIds, host: catalogHost)
    }

    func open(_ session: PersistedSession) async {
        isLoading = true
        defer { isLoading = false }
        await attach(session: session)
    }

    func bindConversation(_ conversation: HostConversation, binding: AgentSessionBinding? = nil) async {
        let switchingConversation = boundConversationId != conversation.id
        let switchingSession = binding?.agentSessionId != nil && binding?.agentSessionId != activeSessionId
        if switchingConversation {
            persistCurrentTranscript()
        }
        boundConversationId = conversation.id
        currentBinding = binding
        activeCwd = binding?.executionCwd ?? conversation.workspaceCwd
        activeSessionId = binding?.agentSessionId
        activeAgentId = binding?.runtimeAgentId
        hydrateCache(
            conversationId: conversation.id,
            sessionId: binding?.agentSessionId,
            replacing: switchingConversation || switchingSession
        )
        let needsSpinner = !hasVisibleTranscript
        if needsSpinner {
            isLoading = true
        }
        defer { isLoading = false }
        await refreshCatalog()
        await refreshLiveAgents()
        await refreshSessions()
        var resolved = binding
        if resolved == nil {
            switch await lookupBinding(conversationId: conversation.id) {
            case .found(let value):
                resolved = value
            case .missing, .failed:
                break
            }
        }
        currentBinding = resolved
        if let resolved {
            HostLog.session.info("Bound conversation chat from current ACP binding")
            await attach(
                session: PersistedSession(
                    storageKey: conversation.id,
                    sessionId: resolved.agentSessionId,
                    runtimeAgentId: liveAgentId(for: resolved),
                    cwd: resolved.executionCwd ?? conversation.workspaceCwd,
                    lastSeq: lastSeq[resolved.agentSessionId] ?? conversation.lastSeq
                )
            )
            return
        }
        if let existing = sessions.first(where: { session in
            session.storageKey == conversation.id || session.sessionId == conversation.id
        }) {
            HostLog.session.info("Bound conversation chat to persisted session")
            hydrateCache(
                conversationId: conversation.id,
                sessionId: existing.sessionId,
                replacing: switchingConversation || existing.sessionId != activeSessionId
            )
            await attach(session: existing)
            return
        }
        HostLog.session.info("Conversation has no current ACP binding")
        persistCurrentTranscript()
    }

    func leave() {
        persistCurrentTranscript()
        persistTask?.cancel()
        resetTranscript()
        boundConversationId = nil
        currentBinding = nil
        showAgentSheet = false
        errorMessage = nil
        switchingStatus = nil
        isSwitchingAgent = false
    }

    func startInConversation(_ conversation: HostConversation) async {
        boundConversationId = conversation.id
        if activeCwd.isEmpty {
            activeCwd = conversation.workspaceCwd
        }
        switch await lookupBinding(conversationId: conversation.id) {
        case .found(let binding):
            isLoading = true
            defer { isLoading = false }
            await continueBoundSession(binding, conversationId: conversation.id)
        case .missing:
            await startNewChat(cwd: conversation.workspaceCwd, conversationId: conversation.id)
        case .failed:
            errorMessage = String(localized: "Could not confirm the computer session. Pull to refresh, then try again.")
        }
    }

    func startNewChat(cwd: String, conversationId: String?, projectId: String? = nil) async {
        guard let socket else { return }
        if let conversationId, !conversationId.isEmpty {
            switch await lookupBinding(conversationId: conversationId) {
            case .found(let binding):
                isLoading = true
                defer { isLoading = false }
                await continueBoundSession(binding, conversationId: conversationId)
                return
            case .failed:
                errorMessage = String(localized: "Could not confirm the computer session. Pull to refresh, then try again.")
                return
            case .missing:
                break
            }
        }
        isLoading = true
        defer { isLoading = false }
        do {
            let agentId = try await ensureAgent()
            var payload: [String: Any] = [
                "agentId": agentId,
                "cwd": cwd,
                "ephemeral": false
            ]
            if let conversationId, !conversationId.isEmpty {
                payload["conversationId"] = conversationId
            }
            if let projectId {
                payload["projectId"] = projectId
            }
            HostLog.session.info("Creating a conversation ACP session because the host has no current binding")
            let created = try await socket.request("create_session", payload: payload, as: NewSessionOutcome.self)
            boundConversationId = conversationId ?? created.conversationId
            apply(created, agentId: agentId, cwd: cwd)
            lastSeq[created.sessionId] = 0
            try await subscribe(sessionId: created.sessionId, lastSeq: 0)
            await refreshSessions()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func switchAgent(to agent: CatalogAgent) async {
        guard let conversationId = boundConversationId else {
            errorMessage = String(localized: "Open a session before switching agents.")
            return
        }
        switch await lookupBinding(conversationId: conversationId) {
        case .found(let binding):
            let alreadyAttached = activeSessionId == binding.agentSessionId
                && AcpSpawnConfig.isCurrent(agent, activeAgentId: activeAgentId, binding: binding)
            if alreadyAttached {
                showAgentSheet = false
                return
            }
            isSwitchingAgent = true
            switchingStatus = String(localized: "Opening the existing session on the Mac…")
            errorMessage = nil
            defer {
                isSwitchingAgent = false
                switchingStatus = nil
            }
            if !AcpSpawnConfig.matchesBinding(agent, binding: binding) {
                HostLog.session.info("Keeping the conversation ACP session instead of replacing it")
            }
            await continueBoundSession(binding, conversationId: conversationId)
            showAgentSheet = false
            return
        case .failed:
            errorMessage = String(localized: "Could not confirm the computer session. Pull to refresh, then try again.")
            return
        case .missing:
            break
        }
        guard let socket else { return }
        if activeCwd.isEmpty {
            errorMessage = String(localized: "Open a session before switching agents.")
            return
        }
        persistCurrentTranscript()
        isSwitchingAgent = true
        switchingStatus = String(localized: "Starting the agent on the Mac. First launch can take a few minutes.")
        errorMessage = nil
        defer {
            isSwitchingAgent = false
            switchingStatus = nil
        }
        do {
            let agentId = try await spawn(agent)
            switchingStatus = String(localized: "Opening a chat session…")
            let payload: [String: Any] = [
                "agentId": agentId,
                "cwd": activeCwd,
                "ephemeral": false,
                "conversationId": conversationId
            ]
            HostLog.session.info("Creating the first ACP session for this conversation")
            let created = try await socket.request(
                "create_session",
                payload: payload,
                as: NewSessionOutcome.self,
                timeoutSeconds: 75
            )
            resetTranscript()
            apply(created, agentId: agentId, cwd: activeCwd)
            lastSeq[created.sessionId] = lastSeq[created.sessionId] ?? 0
            try await subscribe(sessionId: created.sessionId, lastSeq: lastSeq[created.sessionId])
            await refreshSessions()
            await refreshLiveAgents()
            showAgentSheet = false
        } catch {
            HostLog.session.error("ACP agent switch failed")
            errorMessage = error.localizedDescription
        }
    }

    var thoughtOption: SessionConfigOption? {
        configOptions.first { option in
            option.category == "thought_level" && !(option.options?.isEmpty ?? true)
        }
    }

    func refreshLiveComposerSnapshot() async {
        await refreshCatalog()
        await refreshLiveAgents()
        _ = await fetchLiveComposerSnapshot()
    }

    func refreshComposerControls() async {
        await refreshLiveComposerSnapshot()
        // Do not call resume/load on a live Cursor session. Reopening closes
        // the agent's write stream (`WritableIterable is closed`).
    }

    func setMode(_ modeId: String) async {
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else { return }
        do {
            _ = try await socket.request(
                "set_mode",
                payload: ["agentId": agentId, "sessionId": sessionId, "modeId": modeId]
            )
            if var current = modes {
                current.currentModeId = modeId
                modes = current
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func setModel(_ modelId: String) async {
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else { return }
        do {
            let resolved = SessionConfigOption.canonicalizeClaudeModelId(modelId)
            _ = try await socket.request(
                "set_model",
                payload: ["agentId": agentId, "sessionId": sessionId, "modelId": resolved]
            )
            if var current = models {
                current.currentModelId = resolved
                models = current
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func setConfig(optionId: String, valueId: String) async {
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else { return }
        do {
            _ = try await socket.request(
                "set_config_option",
                payload: [
                    "agentId": agentId,
                    "sessionId": sessionId,
                    "configId": optionId,
                    "valueId": SessionConfigOption.canonicalizeClaudeModelId(valueId)
                ]
            )
            if let index = configOptions.firstIndex(where: { $0.id == optionId }) {
                configOptions[index].currentValue = valueId
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func send(_ text: String, in conversation: HostConversation? = nil) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        if activeSessionId == nil, let conversation {
            await startInConversation(conversation)
        }
        await prepareBoundSessionForPrompt()
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else {
            if activeAgentId == nil {
                errorMessage = String(localized: "No agent is running on the host. Start one on the desktop, then try again.")
            }
            return
        }
        isSending = true
        let turnId = UUID().uuidString
        let messageId = "turn:\(turnId)"
        let seq = (messages.compactMap(\.seq).max() ?? lastSeq[sessionId] ?? 0) + 1
        messages.append(
            ChatMessage(
                id: messageId,
                role: .user,
                text: trimmed,
                streaming: false,
                seq: seq,
                delivery: .sending
            )
        )
        schedulePersist()
        HostLog.session.info("Sending prompt to the host agent")
        do {
            _ = try await socket.request(
                "send_prompt",
                payload: [
                    "agentId": agentId,
                    "sessionId": sessionId,
                    "text": trimmed,
                    "turnId": turnId
                ],
                timeoutSeconds: nil
            )
            markDelivery(messageId, .accepted)
        } catch {
            if markDelivery(messageId, .failed) {
                errorMessage = error.localizedDescription
            } else {
                HostLog.session.info("Prompt wait ended after the host already accepted it")
            }
        }
        isSending = false
    }

    func cancel() async {
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else { return }
        _ = try? await socket.request(
            "cancel_prompt",
            payload: ["agentId": agentId, "sessionId": sessionId]
        )
        isSending = false
    }

    func respond(permission: PermissionCard, optionId: String?) async {
        permissions.removeAll { $0.id == permission.id }
        guard let socket else { return }
        var payload: [String: Any] = [
            "agentId": permission.agentId,
            "requestId": permission.id
        ]
        if let optionId {
            payload["optionId"] = optionId
        }
        _ = try? await socket.request("respond_permission", payload: payload)
    }

    func answer(question: QuestionCard, values: [String]) async {
        questions.removeAll { $0.id == question.id }
        guard let socket else { return }
        _ = try? await socket.request(
            "answer_question",
            payload: [
                "agentId": question.agentId,
                "questionId": question.id,
                "values": values
            ]
        )
    }

    private func resetTranscript() {
        messages = []
        tools = []
        permissions = []
        questions = []
        modes = nil
        models = nil
        configOptions = []
        activeSessionId = nil
        activeAgentId = nil
    }

    private func apply(_ outcome: NewSessionOutcome, agentId: String, cwd: String) {
        activeAgentId = agentId
        activeSessionId = outcome.sessionId
        activeCwd = cwd
        apply(reopen: SessionReopenOutcome(
            modes: outcome.modes,
            models: outcome.models,
            configOptions: outcome.configOptions
        ))
    }

    private func apply(reopen: SessionReopenOutcome) {
        if let modes = reopen.modes { self.modes = modes }
        if let options = reopen.configOptions { configOptions = options }
        if let models = reopen.models, models.availableModels?.isEmpty == false {
            self.models = models
        } else if let derived = SessionModelState.derived(from: configOptions) {
            self.models = derived
        }
        HostLog.session.info(
            "Composer controls modes=\(self.modes?.availableModes?.count ?? 0, privacy: .public) models=\(self.models?.availableModels?.count ?? 0, privacy: .public) options=\(self.configOptions.count, privacy: .public)"
        )
    }

    @discardableResult
    private func markDelivery(_ id: String, _ delivery: MessageDelivery) -> Bool {
        guard let index = messages.firstIndex(where: { $0.id == id }) else {
            return delivery == .failed
        }
        switch delivery {
        case .sending:
            messages[index].delivery = .sending
            return false
        case .accepted:
            if messages[index].delivery != .failed {
                messages[index].delivery = .accepted
            }
            return false
        case .failed:
            guard messages[index].delivery == .sending else { return false }
            messages[index].delivery = .failed
            return true
        }
    }

    private func markLatestSendingAccepted() {
        if let index = messages.lastIndex(where: { $0.role == .user && $0.delivery == .sending }) {
            messages[index].delivery = .accepted
            HostLog.session.info("Host accepted the prompt")
        }
    }

    private enum BindingLookup {
        case found(AgentSessionBinding)
        case missing
        case failed
    }

    private func lookupBinding(conversationId: String) async -> BindingLookup {
        if let socket {
            do {
                let snapshot = try await socket.request(
                    "get_conversation_binding",
                    payload: ["conversationId": conversationId],
                    as: ConversationBindingSnapshot.self
                )
                currentBinding = snapshot.binding
                if let binding = snapshot.binding {
                    return .found(binding)
                }
                return .missing
            } catch {
                HostLog.session.error("Conversation binding refresh failed")
            }
        }
        if let currentBinding {
            return .found(currentBinding)
        }
        return .failed
    }

    private func continueBoundSession(_ binding: AgentSessionBinding, conversationId: String) async {
        currentBinding = binding
        boundConversationId = conversationId
        if let cwd = binding.executionCwd, !cwd.isEmpty {
            activeCwd = cwd
        }
        await refreshCatalog()
        await refreshLiveAgents()
        var agentId = liveAgentId(for: binding) ?? binding.runtimeAgentId
        if let catalog = catalogAgents.first(where: { AcpSpawnConfig.matchesBinding($0, binding: binding) }),
           liveAgentId(for: binding) == nil {
            do {
                agentId = try await spawn(catalog)
            } catch {
                HostLog.session.error("Could not reuse the host ACP process; attaching the existing session")
            }
        }
        HostLog.session.info("Continuing the conversation ACP session")
        await attach(
            session: PersistedSession(
                storageKey: conversationId,
                sessionId: binding.agentSessionId,
                runtimeAgentId: agentId,
                cwd: binding.executionCwd ?? activeCwd,
                lastSeq: lastSeq[binding.agentSessionId]
            )
        )
        await refreshSessions()
        await refreshLiveAgents()
    }

    private func liveAgentId(for binding: AgentSessionBinding? = nil) -> String? {
        AcpSpawnConfig.resolveLiveAgentId(
            preferred: activeAgentId,
            binding: binding ?? currentBinding,
            catalog: catalogAgents,
            liveAgentIds: liveAgentIds
        )
    }

    private func prepareBoundSessionForPrompt() async {
        if catalogAgents.isEmpty {
            await refreshCatalog()
        }
        await refreshLiveAgents()
        if let live = liveAgentId() {
            if live != activeAgentId {
                HostLog.session.info("Remapped the conversation to the live host agent")
                activeAgentId = live
            }
            // send_prompt already reattaches on the host when the live process
            // does not own this session. Calling resume/load here replays the
            // whole transcript into the conversation store on every send.
            HostLog.session.info("Sending on the live host session without reopening it")
            return
        }
        if let binding = currentBinding,
           let catalog = catalogAgents.first(where: { AcpSpawnConfig.matchesBinding($0, binding: binding) }) {
            do {
                activeAgentId = try await spawn(catalog)
                HostLog.session.info("Started the host ACP process; the host will attach the session on send")
            } catch {
                HostLog.session.error("Could not start the host ACP process before sending")
            }
        }
    }

    private func fetchLiveComposerSnapshot() async -> Bool {
        guard let socket, let sessionId = activeSessionId, let agentId = activeAgentId else {
            return false
        }
        do {
            let snapshot = try await socket.request(
                "get_composer_controls",
                payload: ["agentId": agentId, "sessionId": sessionId],
                as: SessionReopenOutcome.self
            )
            apply(reopen: snapshot)
            return !(modes?.availableModes?.isEmpty ?? true)
                || !(models?.availableModels?.isEmpty ?? true)
                || thoughtOption != nil
        } catch let error as HostError {
            if case .ipc(_, let code) = error, code == "not_implemented" || code == "unsupported" {
                return false
            }
            HostLog.session.error("Live composer snapshot failed")
            return false
        } catch {
            HostLog.session.error("Live composer snapshot failed")
            return false
        }
    }

    private func attach(session: PersistedSession) async {
        activeSessionId = session.sessionId
        activeAgentId = liveAgentId() ?? session.runtimeAgentId ?? activeAgentId
        if let cwd = session.cwd, !cwd.isEmpty {
            activeCwd = cwd
        }
        do {
            await syncFromWatermark(sessionId: session.sessionId)
            try await subscribe(sessionId: session.sessionId, lastSeq: subscribeCursor(for: session.sessionId))
            persistCurrentTranscript()
            await refreshLiveComposerSnapshot()
            HostLog.session.info("Conversation transcript attached")
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func hydrateCache(conversationId: String, sessionId: String?, replacing: Bool) {
        if let cached = ChatTranscriptCache.load(conversationId) {
            if let sessionId, !sessionId.isEmpty, cached.sessionId != sessionId {
                if replacing { resetTranscript() }
                return
            }
            messages = ChatTranscriptCache.compactMessages(Self.settled(cached.messages))
            tools = ChatTranscriptCache.compactTools(Self.settled(cached.tools))
            activeSessionId = cached.sessionId
            activeAgentId = cached.agentId ?? activeAgentId
            if !cached.cwd.isEmpty {
                activeCwd = cached.cwd
            }
            lastSeq[cached.sessionId] = max(
                lastSeq[cached.sessionId] ?? 0,
                cached.watermark,
                ChatTranscriptCache.watermark(from: messages, tools: tools)
            )
            HostLog.session.info("Hydrated conversation chat from cache")
            return
        }
        if replacing {
            resetTranscript()
        }
    }

    private func syncFromWatermark(sessionId: String) async {
        guard let socket else { return }
        do {
            let cursor = try await socket.request(
                "get_session_cursor",
                payload: ["sessionId": sessionId],
                as: SessionCursor.self
            )
            let hostWatermark = cursor.watermark
            let cachedWatermark = lastSeq[sessionId] ?? 0
            guard let after = ChatTranscriptCache.afterSeq(
                cachedWatermark: cachedWatermark,
                hostWatermark: hostWatermark
            ) else {
                lastSeq[sessionId] = max(cachedWatermark, hostWatermark)
                HostLog.session.info("Conversation cache is at the host watermark")
                return
            }
            await loadHistoryPages(sessionId: sessionId, afterSeq: after, targetLastSeq: hostWatermark)
            lastSeq[sessionId] = max(lastSeq[sessionId] ?? 0, hostWatermark)
        } catch {
            HostLog.session.error("Session cursor lookup failed")
            if lastSeq[sessionId] == nil {
                lastSeq[sessionId] = 0
            }
        }
    }

    private func loadHistoryPages(sessionId: String, afterSeq: UInt64, targetLastSeq: UInt64?) async {
        guard let socket else { return }
        var after = afterSeq
        var target = targetLastSeq
        var loaded = 0
        do {
            while true {
                var payload: [String: Any] = [
                    "sessionId": sessionId,
                    "afterSeq": after,
                    "limit": ChatTranscriptCache.pageLimit
                ]
                if let target {
                    payload["targetLastSeq"] = target
                }
                let page = try await socket.request(
                    "get_session_payload_page",
                    payload: payload,
                    as: ConversationHistoryPage.self
                )
                target = page.targetLastSeq
                for record in page.records {
                    let payloadData = (try? JSONEncoder().encode(record.payload ?? .object([:]))) ?? Data("{}".utf8)
                    handle(type: record.type, sid: sessionId, seq: record.seq, payload: payloadData)
                    loaded += 1
                }
                lastSeq[sessionId] = max(lastSeq[sessionId] ?? 0, page.nextCursor, page.targetLastSeq)
                after = page.nextCursor
                if page.complete || page.records.isEmpty || after >= page.targetLastSeq {
                    break
                }
            }
            HostLog.session.info("Loaded conversation history pages")
        } catch {
            HostLog.session.error("Conversation history page load failed")
        }
        settleHistoricalTranscript()
        _ = loaded
    }

    private func subscribeCursor(for sessionId: String) -> UInt64? {
        let cursor = max(lastSeq[sessionId] ?? 0, ChatTranscriptCache.watermark(from: messages, tools: tools))
        lastSeq[sessionId] = cursor
        return cursor > 0 ? cursor : nil
    }

    private func settleHistoricalTranscript() {
        messages = ChatTranscriptCache.compactMessages(Self.settled(messages))
        tools = ChatTranscriptCache.compactTools(Self.settled(tools))
        isSending = false
        if let sessionId = activeSessionId {
            lastSeq[sessionId] = max(
                lastSeq[sessionId] ?? 0,
                ChatTranscriptCache.watermark(from: messages, tools: tools)
            )
        }
    }

    private static func settled(_ messages: [ChatMessage]) -> [ChatMessage] {
        messages.map { message in
            var next = message
            next.streaming = false
            if next.delivery == .sending {
                next.delivery = .accepted
            }
            return next
        }
    }

    private static func settled(_ tools: [ToolCard]) -> [ToolCard] {
        tools.map { tool in
            var next = tool
            if next.isBusy {
                next.status = "completed"
            }
            return next
        }
    }

    private func persistCurrentTranscript() {
        guard let conversationId = boundConversationId, let sessionId = activeSessionId else { return }
        let compactMessages = ChatTranscriptCache.compactMessages(Self.settled(messages))
        let compactTools = ChatTranscriptCache.compactTools(Self.settled(tools))
        ChatTranscriptCache.save(
            CachedTranscript(
                schemaVersion: ChatTranscriptCache.schemaVersion,
                conversationId: conversationId,
                sessionId: sessionId,
                agentId: activeAgentId,
                cwd: activeCwd,
                watermark: max(
                    lastSeq[sessionId] ?? 0,
                    ChatTranscriptCache.watermark(from: compactMessages, tools: compactTools)
                ),
                messages: compactMessages,
                tools: compactTools
            )
        )
    }

    private func schedulePersist() {
        persistTask?.cancel()
        persistTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            self?.persistCurrentTranscript()
        }
    }

    private func subscribe(sessionId: String, lastSeq: UInt64?) async throws {
        var payload: [String: Any] = ["sessionId": sessionId]
        if let lastSeq, lastSeq > 0 {
            payload["lastSeq"] = lastSeq
        }
        do {
            _ = try await socket?.request("subscribe", payload: payload)
        } catch let error as HostError {
            if case .ipc(_, let code) = error, code == "stale" {
                HostLog.session.info("Subscribe cursor was stale; attaching live-only")
                _ = try await socket?.request("subscribe", payload: ["sessionId": sessionId])
                return
            }
            throw error
        }
    }

    private func ensureAgent() async throws -> String {
        await refreshLiveAgents()
        if let first = liveAgentIds.first {
            return first
        }
        await refreshCatalog()
        guard let agent = catalogAgents.first(where: { canSelect($0) }) else {
            throw HostError.unexpected(String(localized: "No agent is running on the host. Start one on the desktop, then try again."))
        }
        return try await spawn(agent)
    }

    private func spawn(_ agent: CatalogAgent) async throws -> String {
        guard let socket else {
            throw HostError.unexpected(String(localized: "Not connected."))
        }
        await refreshLiveAgents()
        if let running = agent.runningAgentId, !running.isEmpty {
            HostLog.session.info("Reusing the host ACP agent already running for this catalog entry")
            return running
        }
        if let match = liveAgentIds.first(where: { AcpSpawnConfig.matches(agentId: $0, catalogId: agent.id) }) {
            return match
        }
        guard let launch = AcpSpawnConfig.derive(agent, host: catalogHost) else {
            throw HostError.unexpected(String(localized: "This agent cannot be started from the phone. Start it on the desktop, then try again."))
        }
        switchingStatus = String(localized: "Starting the agent on the Mac. First launch can take a few minutes.")
        let spawned = try await socket.request(
            "spawn_agent",
            payload: [
                "config": [
                    "configId": launch.configId,
                    "name": launch.name,
                    "command": launch.command,
                    "args": launch.args,
                    "env": launch.env
                ]
            ],
            as: SpawnAgentResult.self,
            timeoutSeconds: 180
        )
        if let method = spawned.authMethods?.first(where: { !$0.id.isEmpty }) {
            switchingStatus = String(localized: "Sign in on the computer, then wait…")
            HostLog.session.info("ACP agent requested host sign-in")
            _ = try await socket.request(
                "authenticate_agent",
                payload: [
                    "agentId": spawned.agentId,
                    "methodId": method.id
                ],
                timeoutSeconds: nil
            )
        }
        await refreshLiveAgents()
        return spawned.agentId
    }

    private func shouldIgnoreDuplicate(type: String, sid: String?, seq: UInt64) -> Bool {
        guard let sid, seq > 0 else { return false }
        guard ["user_prompt", "message_chunk", "tool_call", "tool_call_update"].contains(type) else {
            return false
        }
        return seq <= (lastSeq[sid] ?? 0)
    }

    private func handle(type: String, sid: String?, seq: UInt64, payload: Data) {
        if shouldIgnoreDuplicate(type: type, sid: sid, seq: seq) {
            return
        }
        if let sid, seq > 0 {
            lastSeq[sid] = max(lastSeq[sid] ?? 0, seq)
        }
        switch type {
        case "user_prompt":
            if let event = try? JSONDecoder().decode(UserPromptEvent.self, from: payload) {
                let id = event.turnId.map { "turn:\($0)" } ?? "user:\(seq)"
                let text = event.content.compactMap(\.text).joined()
                if let index = messages.firstIndex(where: { $0.id == id }) {
                    messages[index].text = text
                    messages[index].streaming = false
                    messages[index].seq = seq
                    if messages[index].delivery == .sending {
                        messages[index].delivery = .accepted
                        HostLog.session.info("Host accepted the prompt")
                    }
                } else if let index = messages.lastIndex(where: { $0.role == .user && $0.text == text }) {
                    messages[index].id = id
                    messages[index].streaming = false
                    messages[index].seq = seq
                    if messages[index].delivery == .sending {
                        messages[index].delivery = .accepted
                    }
                } else {
                    messages.append(ChatMessage(id: id, role: .user, text: text, streaming: false, seq: seq))
                }
            }
        case "message_chunk":
            if let event = try? JSONDecoder().decode(MessageChunkEvent.self, from: payload) {
                markLatestSendingAccepted()
                appendChunk(event, seq: seq)
            }
        case "tool_call":
            if let event = try? JSONDecoder().decode(ToolCallEvent.self, from: payload) {
                markLatestSendingAccepted()
                upsertTool(event.toolCall, seq: seq)
            }
        case "tool_call_update":
            if let event = try? JSONDecoder().decode(ToolCallUpdateEvent.self, from: payload) {
                upsertTool(event.update, seq: seq)
            }
        case "permission_request":
            if let event = try? JSONDecoder().decode(PermissionEvent.self, from: payload) {
                permissions.append(
                    PermissionCard(
                        id: event.requestId,
                        agentId: event.agentId,
                        title: event.toolCall.title ?? String(localized: "Permission required"),
                        options: event.options.map { PermissionChoice(id: $0.optionId, name: $0.name) }
                    )
                )
            }
        case "question_request":
            if let event = try? JSONDecoder().decode(QuestionEvent.self, from: payload) {
                questions.append(
                    QuestionCard(
                        id: event.questionId,
                        agentId: event.agentId,
                        question: event.question,
                        options: event.options.map { QuestionChoice(id: $0.value, label: $0.label) }
                    )
                )
            }
        case "prompt_complete":
            settleHistoricalTranscript()
            markLatestSendingAccepted()
        case "session_info_update":
            if let event = try? JSONDecoder().decode(SessionInfoEvent.self, from: payload),
               let title = event.title,
               let sid,
               let index = sessions.firstIndex(where: { $0.sessionId == sid }) {
                sessions[index].title = title
            }
        case "mode_update":
            if let event = try? JSONDecoder().decode(ModeUpdateEvent.self, from: payload) {
                var next = modes ?? SessionModeState(currentModeId: event.currentModeId, availableModes: event.availableModes)
                next.currentModeId = event.currentModeId
                if let available = event.availableModes, !available.isEmpty {
                    next.availableModes = available
                }
                modes = next
            }
        case "config_options_update":
            if let event = try? JSONDecoder().decode(ConfigOptionsUpdateEvent.self, from: payload) {
                configOptions = event.configOptions
                if let derived = SessionModelState.derived(from: event.configOptions) {
                    models = derived
                }
            }
        case "session_created", "chat_history_changed":
            Task { await refreshSessions() }
            HostLog.session.info("Host conversation catalog changed")
            onHostConversationsChanged?()
        default:
            break
        }
        if ["user_prompt", "message_chunk", "tool_call", "tool_call_update", "prompt_complete"].contains(type) {
            schedulePersist()
        }
    }

    private func appendChunk(_ event: MessageChunkEvent, seq: UInt64) {
        let role = ChatRole(rawValue: event.role) ?? .agent
        let text = event.content.text ?? ""
        if let last = messages.last, last.role == role, last.streaming {
            messages[messages.count - 1].text += text
            messages[messages.count - 1].seq = seq
            return
        }
        messages.append(
            ChatMessage(
                id: "msg-\(UUID().uuidString)",
                role: role,
                text: text,
                streaming: true,
                seq: seq
            )
        )
    }

    private func upsertTool(_ update: ToolCallPatch, seq: UInt64 = 0) {
        if let index = tools.firstIndex(where: { $0.id == update.toolCallId }) {
            if let title = update.title { tools[index].title = title }
            if let status = update.status { tools[index].status = status }
            if seq > 0 { tools[index].seq = seq }
        } else {
            tools.append(
                ToolCard(
                    id: update.toolCallId,
                    title: update.title ?? update.toolCallId,
                    status: update.status ?? "pending",
                    seq: seq > 0 ? seq : nil
                )
            )
        }
    }

    private static func message(from wire: WireChatMessage) -> ChatMessage? {
        let text = (wire.blocks ?? []).compactMap(\.text).joined()
        guard !text.isEmpty || wire.role != nil else { return nil }
        return ChatMessage(
            id: wire.id ?? UUID().uuidString,
            role: ChatRole(rawValue: wire.role ?? "agent") ?? .agent,
            text: text,
            streaming: wire.streaming ?? false,
            seq: wire.seq
        )
    }

    private static func decodeSessions(_ data: Data) -> [PersistedSession] {
        if let decoded = try? JSONDecoder().decode([PersistedSession].self, from: data) {
            return decoded
        }
        guard let raw = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            return []
        }
        return raw.compactMap { dict in
            guard let sessionId = dict["sessionId"] as? String, !sessionId.isEmpty else { return nil }
            return PersistedSession(
                storageKey: dict["storageKey"] as? String,
                sessionId: sessionId,
                runtimeAgentId: dict["runtimeAgentId"] as? String,
                cwd: dict["cwd"] as? String,
                title: dict["title"] as? String,
                lastSeq: (dict["lastSeq"] as? NSNumber)?.uint64Value
            )
        }
    }

    private static func decodePayload(_ data: Data) -> (
        messages: [ChatMessage],
        agentId: String?,
        cwd: String?,
        lastSeq: UInt64?
    ) {
        if let payload = try? JSONDecoder().decode(SessionPayload.self, from: data) {
            return (
                payload.messages.compactMap(message(from:)),
                payload.metadata?.agentId,
                payload.metadata?.cwd,
                payload.metadata?.lastSeq
            )
        }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return ([], nil, nil, nil)
        }
        let metadata = object["metadata"] as? [String: Any]
        let rawMessages = object["messages"] as? [[String: Any]] ?? []
        let messages = rawMessages.compactMap { dict -> ChatMessage? in
            let text = text(fromBlocks: dict["blocks"])
            let role = ChatRole(rawValue: dict["role"] as? String ?? "agent") ?? .agent
            guard !text.isEmpty || dict["role"] != nil else { return nil }
            return ChatMessage(
                id: dict["id"] as? String ?? UUID().uuidString,
                role: role,
                text: text,
                streaming: dict["streaming"] as? Bool ?? false,
                seq: (dict["seq"] as? NSNumber)?.uint64Value
            )
        }
        return (
            messages,
            metadata?["agentId"] as? String,
            metadata?["cwd"] as? String,
            (metadata?["lastSeq"] as? NSNumber)?.uint64Value
        )
    }

    private static func text(fromBlocks blocks: Any?) -> String {
        guard let blocks = blocks as? [[String: Any]] else { return "" }
        return blocks.compactMap { block in
            if let text = block["text"] as? String { return text }
            if let content = block["content"] as? String { return content }
            return nil
        }.joined()
    }
}

private struct UserPromptEvent: Decodable {
    var turnId: String?
    var content: [ContentBlock]
}

private struct MessageChunkEvent: Decodable {
    var role: String
    var content: ContentBlock
}

private struct ToolCallEvent: Decodable {
    var toolCall: ToolCallPatch
}

private struct ToolCallUpdateEvent: Decodable {
    var update: ToolCallPatch
}

private struct ToolCallPatch: Decodable {
    var toolCallId: String
    var title: String?
    var status: String?
}

private struct PermissionEvent: Decodable {
    var agentId: String
    var requestId: String
    var toolCall: ToolCallPatch
    var options: [PermissionOptionWire]
}

private struct PermissionOptionWire: Decodable {
    var optionId: String
    var name: String
}

private struct QuestionEvent: Decodable {
    var agentId: String
    var questionId: String
    var question: String
    var options: [QuestionOptionWire]
}

private struct QuestionOptionWire: Decodable {
    var value: String
    var label: String
}

private struct SessionInfoEvent: Decodable {
    var title: String?
}

private struct ModeUpdateEvent: Decodable {
    var currentModeId: String
    var availableModes: [SessionMode]?
}

private struct ConfigOptionsUpdateEvent: Decodable {
    var configOptions: [SessionConfigOption]

    private enum CodingKeys: String, CodingKey {
        case configOptions
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        configOptions = SessionConfigOption.decodeList(from: container, forKey: .configOptions) ?? []
    }
}
