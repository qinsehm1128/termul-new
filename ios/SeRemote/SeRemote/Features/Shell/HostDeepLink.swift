import Foundation

/// The `se://` links this app accepts.
///
/// - `se://open?url=…` keeps the pre-existing pairing behaviour and is handed
///   back unchanged, so QR codes and shared access links never change meaning.
/// - `se://terminal?host=<hostUUID>&terminal=<terminalID>` lands on one
///   terminal. `host` is optional: without it the active host is used.
/// - Anything else is unknown and must not be treated as a pairing link.
nonisolated enum HostDeepLink: Equatable {
    case pairing(URL)
    case terminal(TerminalRoute)
    case unknown

    static let scheme = "se"
    static let pairingHost = "open"
    static let terminalHost = "terminal"

    static func parse(_ url: URL) -> HostDeepLink {
        guard url.scheme?.lowercased() == scheme else { return .unknown }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let host = (url.host() ?? "").lowercased()
        let path = (components?.path ?? "")
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            .lowercased()

        if host == pairingHost || path == pairingHost {
            return .pairing(url)
        }
        guard host == terminalHost || path == terminalHost else { return .unknown }

        let items = components?.queryItems ?? []
        guard let terminalID = value("terminal", in: items), !terminalID.isEmpty else {
            return .unknown
        }
        let hostID = value("host", in: items)
        return .terminal(TerminalRoute(
            hostID: hostID?.isEmpty == true ? nil : hostID,
            terminalID: terminalID
        ))
    }

    private static func value(_ name: String, in items: [URLQueryItem]) -> String? {
        items.first(where: { $0.name == name })?.value?
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
