import Foundation

enum JSONValue: Sendable, Codable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }

    var string: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    /// String form of a JSON scalar (`true` / `1` / `"opus"`). Objects and arrays are ignored.
    var scalarString: String? {
        switch self {
        case .string(let value):
            value
        case .bool(let value):
            value ? "true" : "false"
        case .number(let value):
            value.rounded() == value ? String(Int(value)) : String(value)
        default:
            nil
        }
    }

    var object: [String: JSONValue]? {
        if case .object(let value) = self { return value }
        return nil
    }

    var array: [JSONValue]? {
        if case .array(let value) = self { return value }
        return nil
    }

    var stringArray: [String] {
        array?.compactMap(\.string) ?? []
    }

    var stringMap: [String: String] {
        guard let object else { return [:] }
        return object.compactMapValues(\.string)
    }

    func decode<T: Decodable>(_ type: T.Type) throws -> T {
        let data = try JSONEncoder().encode(self)
        return try JSONDecoder().decode(T.self, from: data)
    }

    static func encode(_ value: some Encodable) throws -> Data {
        try JSONEncoder().encode(value)
    }
}

enum WireJSON {
    static func data(from object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object, options: [])
    }

    static func object(from data: Data) throws -> [String: Any] {
        let raw = try JSONSerialization.jsonObject(with: data)
        guard let object = raw as? [String: Any] else {
            throw HostError.unexpected("Expected a JSON object")
        }
        return object
    }
}

enum HostError: LocalizedError {
    case network(String)
    case ipc(String, String)
    case unexpected(String)

    var errorDescription: String? {
        switch self {
        case .network(let message), .unexpected(let message):
            message
        case .ipc(let message, _):
            message
        }
    }
}
