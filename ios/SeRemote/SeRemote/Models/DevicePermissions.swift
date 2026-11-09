import Foundation

/// The wire scope vocabulary and its single user-facing translation boundary.
enum DeviceScope {
    nonisolated static func label(for scope: String) -> String {
        switch scope {
        case "view": String(localized: "View host")
        case "terminal.input": String(localized: "Terminal input")
        case "terminal.create": String(localized: "Create terminals")
        case "agent.run": String(localized: "Run agents")
        case "files.write": String(localized: "Write files")
        case "git.write": String(localized: "Git write")
        case "screen.view": String(localized: "View screen")
        case "screen.control": String(localized: "Control screen")
        case "manage": String(localized: "Manage host")
        default: String(localized: "Additional permission")
        }
    }
}

extension RemoteLink {
    /// Legacy host bearers predate scopes; absence is distinct from an empty grant.
    func allows(_ scope: String) -> Bool {
        !isRevoked && (scopes.map { $0.contains(scope) } ?? true)
    }
}

struct PairExchange: Decodable, Sendable {
    let deviceId: String
    let deviceToken: String
    let scopes: [String]
}
