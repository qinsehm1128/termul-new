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
        parts.append(item.agent.isEmpty ? "Terminal" : item.agent)
        parts.append(statusText(for: item))
        if !item.host.isEmpty { parts.append(item.host) }
        return parts.joined(separator: " · ")
    }

    /// Host lifecycle labels are preserved; no approval state is invented.
    static func statusText(for item: LiveSession) -> String {
        switch item.status {
        case .needsYou: "等待你处理"
        case .running: "运行中"
        case .ready: item.detailStatus ?? "空闲"
        case .unknown: item.detailStatus ?? "未知"
        case .idle: "空闲"
        }
    }

    static func destination(id: String) -> LiveDestination? {
        if id.hasPrefix("conversation:") { return .conversation(String(id.dropFirst("conversation:".count))) }
        if id.hasPrefix("project:") { return .project(String(id.dropFirst("project:".count))) }
        if id.hasPrefix("agent:") {
            // `agent:<projectId>:<terminalId>:<sessionId>` — only the project
            // attribution is meaningful to the current workbench.
            let rest = id.dropFirst("agent:".count)
            guard let projectId = rest.split(separator: ":", maxSplits: 1).first,
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
