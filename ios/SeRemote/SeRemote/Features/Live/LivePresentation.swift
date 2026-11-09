import Foundation

/// Pure host-to-Live projection. No clock, network or preview values enter this mapping.
enum LivePresentation {
    static func isRunning(isSending: Bool, messages: [ChatMessage], tools: [ToolCard]) -> Bool {
        isSending || messages.contains { $0.streaming } || tools.contains { $0.isBusy }
    }

    static func sessions(
        conversations: [HostConversation], projects: [HostProject], host: String,
        activeConversationID: String?, activeProjectID: String?,
        agent: String?, isRunning: Bool, needsAttention: Bool,
        agentSessions: [HostLiveAgentSession] = []
    ) -> [LiveSession] {
        let chats = conversations.filter { !$0.isDeleted }.map { conversation in
            let active = conversation.id == activeConversationID
            return LiveSession(
                id: "conversation:\(conversation.id)", title: conversation.displayTitle,
                agent: active ? (agent ?? "") : "", host: host,
                status: active && needsAttention ? .needsYou : (active && isRunning ? .running : (conversation.lifecycleState == "ready" ? .ready : .unknown)),
                timing: "", detailStatus: active && (needsAttention || isRunning) ? nil : conversation.statusLabel
            )
        }
        let terminals = projects.filter(\.isRunning).map { project in
            LiveSession(
                id: "project:\(project.id)", title: project.name, agent: "", host: host,
                status: project.id == activeProjectID && needsAttention ? .needsYou : .running,
                timing: ""
            )
        }
        // The host only returns processes it found under a live terminal, so
        // these rows are running evidence, not session files on disk.
        let agents = agentSessions.map { session in
            LiveSession(
                id: "agent:\(session.destinationProjectId ?? ""):\(session.terminalId):\(session.sessionId)",
                title: agentTitle(for: session),
                agent: agentLabel(session.agentId),
                host: host,
                status: .running,
                timing: ""
            )
        }
        return chats + terminals + agents
    }

    /// ACP conversations only, ordered for the 会话 tab: the active/streaming
    /// conversation first, then the most recent host conversations. Terminals,
    /// projects and `/cli-sessions/live` agents never enter this list.
    static func conversations(
        _ conversations: [HostConversation],
        activeConversationID: String?,
        agent: String?,
        isRunning: Bool,
        needsAttention: Bool
    ) -> [ConversationEntry] {
        let rows = conversations.filter { !$0.isDeleted }.map { conversation in
            let active = conversation.id == activeConversationID
            let waiting = active && needsAttention
            let streaming = active && isRunning
            let status: LiveSessionStatus = waiting ? .needsYou : (streaming ? .running : (conversation.lifecycleState == "ready" ? .ready : .unknown))
            return ConversationEntry(
                id: conversation.id,
                conversation: conversation,
                title: conversation.displayTitle,
                agent: active ? agentLabel(agent ?? "") : "",
                project: conversation.folderName,
                status: status,
                statusText: statusText(status: status, detail: conversation.statusLabel),
                time: conversation.relativeCreatedLabel,
                isActiveOrStreaming: active && (waiting || streaming),
                needsAttention: waiting
            )
        }
        return rows.enumerated().sorted { lhs, rhs in
            if lhs.element.isActiveOrStreaming != rhs.element.isActiveOrStreaming {
                return lhs.element.isActiveOrStreaming
            }
            let lhsDate = conversationDate(lhs.element.conversation)
            let rhsDate = conversationDate(rhs.element.conversation)
            switch (lhsDate, rhsDate) {
            case let (l?, r?):
                if l != r { return l > r }
            case (nil, _?):
                return false
            case (_?, nil):
                return true
            case (nil, nil):
                break
            }
            return lhs.offset < rhs.offset
        }.map(\.element)
    }

    private static func statusText(status: LiveSessionStatus, detail: String?) -> String {
        switch status {
        case .needsYou: String(localized: "Waiting for you")
        case .running: String(localized: "Running")
        case .ready: detail ?? String(localized: "Idle")
        case .unknown: detail ?? String(localized: "Unknown")
        case .idle: String(localized: "Idle")
        }
    }

    /// Host timestamps are ISO-8601 with optional fractional seconds. Sorting is
    /// the only use; an unparseable value simply sorts last and keeps its order.
    private static func conversationDate(_ conversation: HostConversation) -> Date? {
        guard let raw = conversation.createdAtUtc, !raw.isEmpty else { return nil }
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = withFraction.date(from: raw) { return date }
        return ISO8601DateFormatter().date(from: raw)
    }

    /// Display name for the agent the host detected.
    static func agentLabel(_ agentId: String) -> String {
        switch agentId {
        case "claude-code": "Claude Code"
        case "qin-code": "qin-code"
        case "codex": "Codex"
        case "gemini-cli": "Gemini CLI"
        case "cursor": "Cursor"
        case "opencode": "OpenCode"
        case "pi": "Pi"
        default: agentId
        }
    }

    /// A real title: the session's working folder when the host reported one,
    /// otherwise the agent name. Never a fabricated conversation title.
    static func agentTitle(for session: HostLiveAgentSession) -> String {
        if let cwd = session.cwd,
           let folder = HostTimestamp.folderName(from: cwd),
           !folder.isEmpty {
            return folder
        }
        return agentLabel(session.agentId)
    }

    static func needs(permissions: [PermissionCard], questions: [QuestionCard], tools: [ToolCard], context: String) -> [NeedsItem] {
        permissions.map {
            NeedsItem(id: "permission:\($0.id)", kind: .permission, title: $0.title,
                      meta: context, actionTitle: String(localized: "Review"))
        } + questions.map {
            NeedsItem(id: "question:\($0.id)", kind: .question, title: $0.question,
                      meta: context, actionTitle: String(localized: "Answer"))
        } + tools.filter { $0.status == "failed" }.map {
            NeedsItem(id: "tool:\($0.id)", kind: .failure, title: $0.title,
                      meta: context, actionTitle: String(localized: "Open"))
        }
    }

    static func summary(items: [NeedsItem]) -> LiveNeedsSummary {
        LiveNeedsSummary(permissions: items.filter { $0.kind == .permission }.count,
                         failures: items.filter { $0.kind == .failure }.count,
                         questions: items.filter { $0.kind == .question }.count)
    }

    /// The single line under a session title: agent · state · host.
    static func rowMeta(for item: LiveSession) -> String {
        var parts: [String] = []
        parts.append(item.agent.isEmpty ? String(localized: "Terminal") : item.agent)
        parts.append(statusText(for: item))
        if !item.host.isEmpty { parts.append(item.host) }
        return parts.joined(separator: " · ")
    }

    /// Host lifecycle labels are preserved; no approval state is invented.
    static func statusText(for item: LiveSession) -> String {
        switch item.status {
        case .needsYou: String(localized: "Waiting for you")
        case .running: String(localized: "Running")
        case .ready: item.detailStatus ?? String(localized: "Idle")
        case .unknown: item.detailStatus ?? String(localized: "Unknown")
        case .idle: String(localized: "Idle")
        }
    }

    static func destination(id: String) -> LiveDestination? {
        if id.hasPrefix("conversation:") { return .conversation(String(id.dropFirst("conversation:".count))) }
        if id.hasPrefix("project:") { return .project(String(id.dropFirst("project:".count))) }
        if id.hasPrefix("agent:") {
            // `agent:<projectId>:<terminalId>:<sessionId>` — only the project
            // attribution is meaningful to the current workbench.
            let rest = id.dropFirst("agent:".count)
            // Keep the empty component: `agent::<terminal>:<session>` has no
            // project attribution, and dropping the empty field would promote
            // the terminal id to a project.
            guard let projectId = rest.split(
                separator: ":", maxSplits: 1, omittingEmptySubsequences: false
            ).first,
                  !projectId.isEmpty else { return nil }
            return .project(String(projectId))
        }
        return nil
    }
}

enum LiveDestination: Equatable {
    case conversation(String)
    case project(String)
}
