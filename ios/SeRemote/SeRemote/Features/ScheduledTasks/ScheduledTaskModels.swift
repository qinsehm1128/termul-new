import Foundation

/// The host may add enum values without requiring a phone upgrade.
nonisolated protocol ScheduledTaskStringWire: Decodable, Sendable, Hashable {
    init(wireValue: String)
    var rawValue: String { get }
}

extension ScheduledTaskStringWire {
    nonisolated init(from decoder: Decoder) throws {
        self.init(wireValue: try decoder.singleValueContainer().decode(String.self))
    }
}

nonisolated enum ScheduledTaskStatusWire: ScheduledTaskStringWire {
    case draft, active, paused, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "draft": self = .draft
        case "active": self = .active
        case "paused": self = .paused
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .draft: "draft"
        case .active: "active"
        case .paused: "paused"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduledTaskOverlapWire: ScheduledTaskStringWire {
    case skip, bufferOne, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "skip": self = .skip
        case "bufferOne": self = .bufferOne
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .skip: "skip"
        case .bufferOne: "bufferOne"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduledTaskCatchUpWire: ScheduledTaskStringWire {
    case skip, latestOnce, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "skip": self = .skip
        case "latestOnce": self = .latestOnce
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .skip: "skip"
        case .latestOnce: "latestOnce"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduledTaskTriggerWire: ScheduledTaskStringWire {
    case scheduled, manual, catchUp, retry, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "scheduled": self = .scheduled
        case "manual": self = .manual
        case "catchUp": self = .catchUp
        case "retry": self = .retry
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .scheduled: "scheduled"
        case .manual: "manual"
        case .catchUp: "catchUp"
        case .retry: "retry"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduledTaskRunStatusWire: ScheduledTaskStringWire {
    case queued, cancelled, running, succeeded, failed, skipped, interrupted, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "queued": self = .queued
        case "cancelled": self = .cancelled
        case "running": self = .running
        case "succeeded": self = .succeeded
        case "failed": self = .failed
        case "skipped": self = .skipped
        case "interrupted": self = .interrupted
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .queued: "queued"
        case .cancelled: "cancelled"
        case .running: "running"
        case .succeeded: "succeeded"
        case .failed: "failed"
        case .skipped: "skipped"
        case .interrupted: "interrupted"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduledTaskActorWire: ScheduledTaskStringWire {
    case human, agent, system, unknown(String)
    init(wireValue: String) {
        switch wireValue {
        case "human": self = .human
        case "agent": self = .agent
        case "system": self = .system
        default: self = .unknown(wireValue)
        }
    }
    var rawValue: String {
        switch self {
        case .human: "human"
        case .agent: "agent"
        case .system: "system"
        case .unknown(let value): value
        }
    }
}

nonisolated enum ScheduleSpecWire: Decodable, Sendable, Hashable {
    case cron(expression: String, timezone: String)
    case interval(everySeconds: UInt64, anchorAt: String)
    case at(at: String)
    case unknown(String)

    private nonisolated enum CodingKeys: String, CodingKey {
        case kind, expression, timezone, at, everySeconds, anchorAt
        case everySecondsRust = "every_seconds", anchorAtRust = "anchor_at"
    }

    nonisolated init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .kind) {
        case "cron":
            self = .cron(expression: try values.decode(String.self, forKey: .expression),
                         timezone: try values.decode(String.self, forKey: .timezone))
        case "interval":
            // Rust's enum rename_all renames variants, not the fields within a variant.
            let seconds = try values.decodeIfPresent(UInt64.self, forKey: .everySecondsRust)
                ?? values.decode(UInt64.self, forKey: .everySeconds)
            let anchor = try values.decodeIfPresent(String.self, forKey: .anchorAtRust)
                ?? values.decode(String.self, forKey: .anchorAt)
            self = .interval(everySeconds: seconds, anchorAt: anchor)
        case "at": self = .at(at: try values.decode(String.self, forKey: .at))
        case let kind: self = .unknown(kind)
        }
    }
}

nonisolated enum ExecutionTargetWire: Decodable, Sendable, Hashable {
    case workspace
    case projectRoot(projectId: String, projectRoot: String)
    case worktree(projectId: String, worktreePath: String, worktreeBranch: String)
    case unknown(String)

    private nonisolated enum CodingKeys: String, CodingKey {
        case kind, projectId, projectRoot, worktreePath, worktreeBranch
    }

    nonisolated init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .kind) {
        case "workspace": self = .workspace
        case "project_root":
            self = .projectRoot(projectId: try values.decode(String.self, forKey: .projectId),
                                projectRoot: try values.decode(String.self, forKey: .projectRoot))
        case "worktree":
            self = .worktree(projectId: try values.decode(String.self, forKey: .projectId),
                             worktreePath: try values.decode(String.self, forKey: .worktreePath),
                             worktreeBranch: try values.decode(String.self, forKey: .worktreeBranch))
        case let kind: self = .unknown(kind)
        }
    }
}

nonisolated struct ExecutionPolicyWire: Decodable, Sendable, Hashable {
    let overlap: ScheduledTaskOverlapWire
    let catchUp: ScheduledTaskCatchUpWire
    let catchUpWindowSeconds: UInt64
}

nonisolated struct ScheduledTaskWire: Decodable, Sendable, Hashable, Identifiable {
    let schemaVersion: UInt32
    let taskId: String
    let projectId: String?
    let name: String
    let description: String
    let status: ScheduledTaskStatusWire
    let schedule: ScheduleSpecWire
    let executionPolicy: ExecutionPolicyWire
    let prompt: String
    let agentConfigId: String
    let executionTarget: ExecutionTargetWire
    let executionCwd: String
    let workspaceCwd: String
    let sourceConversationId: String?
    let permissions: [String]
    let skillTemplateVersion: UInt32
    let revision: UInt64
    let draftHash: String
    let createdAt: String
    let updatedAt: String
    let nextRunAt: String?
    var id: String { taskId }
}

nonisolated struct ScheduledTaskRunWire: Decodable, Sendable, Hashable, Identifiable {
    let schemaVersion: UInt32
    let runId: String
    let taskId: String
    let projectId: String?
    let trigger: ScheduledTaskTriggerWire
    let status: ScheduledTaskRunStatusWire
    let occurrenceKey: String
    let scheduledFor: String
    let queuedAt: String
    let startedAt: String?
    let finishedAt: String?
    let conversationId: String?
    let taskSnapshotHash: String
    let retryOfRunId: String?
    let summary: String?
    let errorCode: String?
    let errorDetail: String?
    let usage: ScheduledTaskJSONWire?
    var id: String { runId }
}

/// Arbitrary host usage metadata, isolated from the shared actor-bound JSONValue type.
nonisolated enum ScheduledTaskJSONWire: Decodable, Sendable, Hashable {
    case null, bool(Bool), number(Double), string(String)
    case array([ScheduledTaskJSONWire]), object([String: ScheduledTaskJSONWire])

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let bool = try? value.decode(Bool.self) { self = .bool(bool) }
        else if let string = try? value.decode(String.self) { self = .string(string) }
        else if let number = try? value.decode(Double.self) { self = .number(number) }
        else if let array = try? value.decode([ScheduledTaskJSONWire].self) { self = .array(array) }
        else { self = .object(try value.decode([String: ScheduledTaskJSONWire].self)) }
    }

    subscript(key: String) -> ScheduledTaskJSONWire? {
        guard case .object(let values) = self else { return nil }
        return values[key]
    }

    var intValue: Int? {
        guard case .number(let number) = self else { return nil }
        return Int(exactly: number)
    }
}

nonisolated struct ScheduledTaskAuditEventWire: Decodable, Sendable, Hashable, Identifiable {
    let schemaVersion: UInt32
    let eventId: String
    let taskId: String
    let projectId: String?
    let action: String
    let actor: ScheduledTaskActorWire
    let sourceConversationId: String?
    let sourceToolCallId: String?
    let beforeHash: String?
    let afterHash: String?
    let createdAt: String
    var id: String { eventId }
}
