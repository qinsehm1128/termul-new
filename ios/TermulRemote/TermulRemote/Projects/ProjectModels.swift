import Foundation

struct HostProject: Identifiable, Hashable, Decodable, Sendable {
    let id: String
    var name: String
    var color: String?
    var path: String?
    var isArchived: Bool
    var isDefault: Bool
}

struct ProjectListPayload: Decodable, Sendable {
    var projects: [HostProject]
    var defaultProjectId: String?
}

struct CatalogAgent: Decodable, Sendable, Identifiable {
    let id: String
    var name: String
    var status: String?
    var installed: InstalledBinary?
    var distribution: JSONValue?
    var runningAgentId: String?
}

struct InstalledBinary: Decodable, Sendable {
    var command: String
    var args: [String]?
}

struct AcpCatalog: Decodable, Sendable {
    var host: CatalogHost?
    var agents: [CatalogAgent]
}

struct DirectoryEntry: Identifiable, Hashable, Decodable, Sendable {
    var name: String
    var path: String
    var type: String
    var fileExtension: String?
    var size: Int?
    var modifiedAt: Double?

    var id: String { path }
    var isDirectory: Bool { type == "directory" }

    enum CodingKeys: String, CodingKey {
        case name, path, type, size, modifiedAt
        case fileExtension = "extension"
    }
}

struct FileContent: Decodable, Sendable {
    var content: String
    var encoding: String?
    var size: Int?
    var modifiedAt: Double?
}

struct PersistedSession: Identifiable, Decodable, Sendable {
    var storageKey: String? = nil
    var sessionId: String
    var runtimeAgentId: String? = nil
    var projectId: String? = nil
    var cwd: String? = nil
    var title: String? = nil
    var createdAt: Double? = nil
    var lastActivityAt: Double? = nil
    var status: String? = nil
    var messageCount: Int? = nil
    var lastSeq: UInt64? = nil

    var id: String { sessionId }

    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        return sessionId
    }
}

struct SessionPayload: Decodable, Sendable {
    var metadata: SessionMetadata?
    var messages: [WireChatMessage]
}

struct SessionMetadata: Decodable, Sendable {
    var id: String?
    var agentId: String?
    var title: String?
    var cwd: String?
    var projectId: String?
    var lastSeq: UInt64?
    var status: String?
}

struct WireChatMessage: Decodable, Sendable {
    var id: String?
    var role: String?
    var blocks: [ContentBlock]?
    var streaming: Bool?
    var timestamp: Double?
    var seq: UInt64?
}

struct ContentBlock: Codable, Hashable, Sendable {
    var type: String
    var text: String?
}

struct NewSessionOutcome: Decodable, Sendable {
    var sessionId: String
    var conversationId: String?
    var modes: SessionModeState?
    var models: SessionModelState?
    var configOptions: [SessionConfigOption]?

    private enum CodingKeys: String, CodingKey {
        case sessionId, conversationId, modes, models, configOptions
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try container.decode(String.self, forKey: .sessionId)
        conversationId = try container.decodeIfPresent(String.self, forKey: .conversationId)
        modes = try? container.decode(SessionModeState.self, forKey: .modes)
        models = try? container.decode(SessionModelState.self, forKey: .models)
        configOptions = SessionConfigOption.decodeList(from: container, forKey: .configOptions)
    }
}

struct SessionReopenOutcome: Decodable, Sendable {
    var modes: SessionModeState?
    var models: SessionModelState?
    var configOptions: [SessionConfigOption]?

    private enum CodingKeys: String, CodingKey {
        case modes, models, configOptions
    }

    init(modes: SessionModeState? = nil, models: SessionModelState? = nil, configOptions: [SessionConfigOption]? = nil) {
        self.modes = modes
        self.models = models
        self.configOptions = configOptions
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        modes = try? container.decode(SessionModeState.self, forKey: .modes)
        models = try? container.decode(SessionModelState.self, forKey: .models)
        configOptions = SessionConfigOption.decodeList(from: container, forKey: .configOptions)
    }
}

struct SessionModeState: Decodable, Hashable, Sendable {
    var currentModeId: String?
    var availableModes: [SessionMode]?
}

struct SessionMode: Identifiable, Decodable, Hashable, Sendable {
    var id: String
    var name: String
    var description: String?
}

struct SessionModelState: Decodable, Hashable, Sendable {
    var currentModelId: String?
    var availableModels: [SessionModel]?
}

struct SessionModel: Identifiable, Decodable, Hashable, Sendable {
    var modelId: String
    var name: String
    var description: String?
    var id: String { modelId }
}

struct SessionConfigOption: Identifiable, Hashable, Sendable {
    var id: String
    var name: String
    var category: String?
    var currentValue: String?
    var options: [SessionConfigOptionValue]?
}

struct SessionConfigOptionValue: Identifiable, Hashable, Sendable {
    var value: String
    var name: String
    var description: String?
    var group: String?
    var id: String { value }
}

extension SessionConfigOption: Decodable {
    init(from decoder: Decoder) throws {
        let value = try JSONValue(from: decoder)
        guard let decoded = SessionConfigOption(json: value) else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "SessionConfigOption")
            )
        }
        self = decoded
    }

    static func decodeList<Key: CodingKey>(
        from container: KeyedDecodingContainer<Key>,
        forKey key: Key
    ) -> [SessionConfigOption]? {
        guard let raw = try? container.decode([JSONValue].self, forKey: key) else { return nil }
        return raw.compactMap(SessionConfigOption.init(json:))
    }

    init?(json: JSONValue) {
        guard let object = json.object else { return nil }
        guard let id = object["id"]?.string ?? object["configId"]?.string, !id.isEmpty else { return nil }
        guard let name = object["name"]?.string, !name.isEmpty else { return nil }
        self.id = id
        self.name = name
        self.category = object["category"]?.string
        let current = object["currentValue"]?.scalarString.map(Self.canonicalizeClaudeModelId)
        let flattened = Self.flattenOptions(object["options"])
        if flattened.isEmpty, object["type"]?.string == "boolean" {
            let enabled = current == "true" || current == "1"
            self.currentValue = enabled ? "true" : "false"
            self.options = [
                SessionConfigOptionValue(value: "true", name: String(localized: "On")),
                SessionConfigOptionValue(value: "false", name: String(localized: "Off"))
            ]
        } else {
            self.currentValue = current
            self.options = flattened.isEmpty ? nil : flattened
        }
    }

    /// Claude ACP groups families (`claude-sonnet`) above versioned leaves.
    static func canonicalizeClaudeModelId(_ value: String) -> String {
        for family in ["claude-sonnet", "claude-opus", "claude-haiku"] {
            if value == family {
                return "\(family)-5"
            }
            if value.hasPrefix(family + "[") {
                return "\(family)-5\(value.dropFirst(family.count))"
            }
        }
        return value
    }

    static func flattenOptions(_ raw: JSONValue?, ancestorValue: String? = nil) -> [SessionConfigOptionValue] {
        guard let entries = raw?.array else { return [] }
        var flat: [SessionConfigOptionValue] = []
        for entry in entries {
            guard let object = entry.object else { continue }
            let parentValue = object["value"]?.string ?? ancestorValue
            let groupName = object["name"]?.string ?? object["group"]?.string ?? ""
            if object["options"] != nil {
                let children = flattenOptions(object["options"], ancestorValue: parentValue)
                if !children.isEmpty {
                    for child in children {
                        var next = child
                        if next.group == nil, !groupName.isEmpty {
                            next.group = groupName
                        }
                        flat.append(next)
                    }
                    continue
                }
            }
            guard var value = object["value"]?.string, let name = object["name"]?.string else { continue }
            if let ancestorValue, value.first == "[", value.last == "]" {
                value = ancestorValue + value
            }
            flat.append(
                SessionConfigOptionValue(
                    value: canonicalizeClaudeModelId(value),
                    name: name,
                    description: object["description"]?.string,
                    group: object["group"]?.string
                )
            )
        }
        return flat
    }
}

extension SessionModelState {
    static func derived(from options: [SessionConfigOption]) -> SessionModelState? {
        guard let option = options.first(where: { $0.category == "model" }),
              let values = option.options, !values.isEmpty else {
            return nil
        }
        return SessionModelState(
            currentModelId: option.currentValue,
            availableModels: values.map {
                SessionModel(modelId: $0.value, name: $0.name, description: $0.description)
            }
        )
    }
}

struct ConversationHistoryPage: Decodable, Sendable {
    var records: [ConversationHistoryRecord]
    var nextCursor: UInt64
    var complete: Bool
    var targetLastSeq: UInt64
}

struct ConversationHistoryRecord: Decodable, Sendable {
    var sessionId: String?
    var seq: UInt64
    var type: String
    var payload: JSONValue?
}

struct SpawnAgentResult: Decodable, Sendable {
    var agentId: String
    var authMethods: [AgentAuthMethod]?
    var stableNamespace: String?
}

struct AgentAuthMethod: Decodable, Sendable {
    var id: String
    var name: String
    var description: String?
}

struct SwitchProjectReply: Decodable, Sendable {
    var status: String?
    var projectId: String?
    var sessionId: String?
    var cwd: String?
}
