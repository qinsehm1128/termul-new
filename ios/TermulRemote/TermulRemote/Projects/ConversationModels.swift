import Foundation

struct HostConversation: Identifiable, Hashable, Decodable, Sendable {
    var conversationId: String
    var title: String?
    var workspaceCwd: String
    var createdAtUtc: String?
    var lifecycleState: String
    var lastSeq: UInt64?
    var projectAttachment: ConversationProjectAttachment?
    var executionTarget: ConversationExecutionTarget?

    var id: String { conversationId }

    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        return HostTimestamp.folderName(from: workspaceCwd) ?? conversationId
    }

    var folderName: String {
        HostTimestamp.folderName(from: workspaceCwd) ?? workspaceCwd
    }

    var relativeCreatedLabel: String? {
        HostTimestamp.relative(from: createdAtUtc)
    }

    var statusLabel: String {
        switch lifecycleState {
        case "ready": String(localized: "Ready")
        case "deleted": String(localized: "Deleted")
        default: lifecycleState
        }
    }

    /// Path-secondary preview until the host ships last-turn text.
    var previewText: String { folderName }

    var countLabel: String? {
        guard let lastSeq, lastSeq > 0 else { return nil }
        return String.localizedStringWithFormat(
            String(localized: "%lld turns"),
            Int64(lastSeq)
        )
    }

    var projectId: String? {
        projectAttachment?.projectId ?? executionTarget?.projectId
    }

    var isDeleted: Bool { lifecycleState == "deleted" }
}

struct ConversationProjectAttachment: Hashable, Decodable, Sendable {
    var projectId: String
    var projectPathSnapshot: String?
}

struct ConversationExecutionTarget: Hashable, Decodable, Sendable {
    var kind: String
    var projectId: String?
    var projectRoot: String?
    var worktreePath: String?
}

struct ConversationOpenOutcome: Decodable, Sendable {
    var conversation: HostConversation?
    var workspace: SessionWorkspaceLoadOutcome?
}

struct ConversationBindingSnapshot: Decodable, Sendable {
    var conversationId: String
    var binding: AgentSessionBinding?
}

struct AgentSessionBinding: Hashable, Decodable, Sendable {
    var agentSessionId: String
    var runtimeAgentId: String
    var stableAgentNamespace: String?
    var executionCwd: String?
    var state: String?
}

struct SessionWorkspaceLoadOutcome: Decodable, Sendable {
    var status: String
    var workspace: SessionWorkspacePayload?
}

struct SessionWorkspacePayload: Decodable, Sendable {
    var resources: [SessionWorkspaceResource]?
}

struct SessionWorkspaceResource: Decodable, Sendable {
    var kind: String
    var terminalId: String?
}
