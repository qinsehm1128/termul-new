import Foundation

/// The tagged target contract uses snake_case kinds but camelCase payload keys.
nonisolated enum QuickTerminalTargetWire: Hashable, Sendable, Codable {
    case workspace
    case projectRoot(projectId: String, projectRoot: String)
    case worktree(projectId: String, worktreePath: String, worktreeBranch: String)
    case unknown(String)

    private enum CodingKeys: String, CodingKey {
        case kind, projectId, projectRoot, worktreePath, worktreeBranch
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try values.decode(String.self, forKey: .kind)
        switch kind {
        case "workspace": self = .workspace
        case "project_root":
            self = .projectRoot(
                projectId: try values.decode(String.self, forKey: .projectId),
                projectRoot: try values.decode(String.self, forKey: .projectRoot)
            )
        case "worktree":
            self = .worktree(
                projectId: try values.decode(String.self, forKey: .projectId),
                worktreePath: try values.decode(String.self, forKey: .worktreePath),
                worktreeBranch: try values.decode(String.self, forKey: .worktreeBranch)
            )
        default: self = .unknown(kind)
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .workspace:
            try values.encode("workspace", forKey: .kind)
        case let .projectRoot(projectId, projectRoot):
            try values.encode("project_root", forKey: .kind)
            try values.encode(projectId, forKey: .projectId)
            try values.encode(projectRoot, forKey: .projectRoot)
        case let .worktree(projectId, worktreePath, worktreeBranch):
            try values.encode("worktree", forKey: .kind)
            try values.encode(projectId, forKey: .projectId)
            try values.encode(worktreePath, forKey: .worktreePath)
            try values.encode(worktreeBranch, forKey: .worktreeBranch)
        case .unknown:
            // Unknown response targets can be displayed, but not safely resubmitted.
            throw EncodingError.invalidValue(self, .init(codingPath: encoder.codingPath, debugDescription: "Unknown quick terminal target"))
        }
    }
}

nonisolated enum QuickTerminalOriginWire: Hashable, Sendable, Decodable {
    case created
    case migratedConversation
    case unknown(String)

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        switch value {
        case "created": self = .created
        case "migrated_conversation": self = .migratedConversation
        default: self = .unknown(value)
        }
    }
}

nonisolated struct QuickTerminalRecordWire: Identifiable, Hashable, Sendable, Decodable {
    let schemaVersion: Int
    let id: String
    let title: String?
    let target: QuickTerminalTargetWire
    let cwd: String
    let createdAtUtc: String
    let updatedAtUtc: String
    let terminalId: String?
    let terminalIds: [String]
    let origin: QuickTerminalOriginWire

    private enum CodingKeys: String, CodingKey {
        case schemaVersion, id, title, target, cwd, createdAtUtc, updatedAtUtc, terminalId, terminalIds, origin
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try values.decode(Int.self, forKey: .schemaVersion)
        id = try values.decode(String.self, forKey: .id)
        title = try values.decodeIfPresent(String.self, forKey: .title)
        target = try values.decode(QuickTerminalTargetWire.self, forKey: .target)
        cwd = try values.decode(String.self, forKey: .cwd)
        createdAtUtc = try values.decode(String.self, forKey: .createdAtUtc)
        updatedAtUtc = try values.decode(String.self, forKey: .updatedAtUtc)
        terminalId = try values.decodeIfPresent(String.self, forKey: .terminalId)
        // Rust defaults terminalIds when reading legacy records, without inventing terminalId.
        terminalIds = try values.decodeIfPresent([String].self, forKey: .terminalIds) ?? []
        origin = try values.decode(QuickTerminalOriginWire.self, forKey: .origin)
    }
}

/// Pass terminalId + optional claim to the existing terminal attach flow.
/// A reused PTY has spawned == false and may omit claim; do not fabricate one.
nonisolated struct QuickTerminalOpenedWire: Hashable, Sendable, Decodable {
    let record: QuickTerminalRecordWire
    let terminalId: String
    let claim: String?
    let spawned: Bool
}

nonisolated struct CreateQuickTerminalBody: Encodable, Sendable {
    let target: QuickTerminalTargetWire
    var title: String? = nil
}

nonisolated struct OpenQuickTerminalBody: Encodable, Sendable {
    let id: String
    let cols: UInt16
    let rows: UInt16
    var terminalId: String? = nil
}

/// Separate from open: the server denies the terminalId key on create-terminal.
nonisolated struct CreateQuickTerminalTerminalBody: Encodable, Sendable {
    let id: String
    let cols: UInt16
    let rows: UInt16
}

nonisolated struct CloseQuickTerminalTerminalBody: Encodable, Sendable {
    let id: String
    let terminalId: String
}

nonisolated struct QuickTerminalRenameBody: Encodable, Sendable {
    let id: String
    var title: String? = nil
}

nonisolated struct QuickTerminalIdBody: Encodable, Sendable {
    let id: String
}

/// Keeps request key sets tied to Encodable DTOs while reusing HostHTTP.post.
nonisolated enum QuickTerminalRequestJSON {
    static func dictionary<T: Encodable>(_ body: T) throws -> [String: Any] {
        let data = try JSONEncoder().encode(body)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw EncodingError.invalidValue(body, .init(codingPath: [], debugDescription: "Expected a request object"))
        }
        return object
    }
}
