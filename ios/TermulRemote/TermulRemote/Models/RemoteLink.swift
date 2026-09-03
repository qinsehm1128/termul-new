import Foundation

enum WorkspaceSurface: String, Hashable, CaseIterable, Identifiable {
    case chat
    case terminal

    var id: String { rawValue }
}

struct RemoteLink: Identifiable, Hashable, Codable {
    let id: UUID
    var title: String
    var accessURL: URL
    var bearer: String?
    var createdAt: Date

    init(id: UUID = UUID(), title: String? = nil, accessURL: URL, bearer: String? = nil, createdAt: Date = .now) {
        self.id = id
        self.title = title ?? RemoteLink.displayTitle(for: accessURL)
        self.accessURL = RemoteLink.attaching(token: bearer ?? RemoteLink.accessToken(in: accessURL), to: accessURL)
        self.bearer = bearer ?? RemoteLink.accessToken(in: self.accessURL)
        self.createdAt = createdAt
    }

    var originHost: String {
        accessURL.host() ?? accessURL.absoluteString
    }

    var pairingToken: String? {
        if let bearer, !bearer.isEmpty { return bearer }
        return RemoteLink.accessToken(in: accessURL)
    }

    var originURL: URL {
        var components = URLComponents(url: accessURL, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.fragment = nil
        components.query = nil
        components.path = ""
        return components.url ?? accessURL
    }

    func url(for surface: WorkspaceSurface) -> URL {
        var base = accessURL.absoluteString
        if let hash = base.firstIndex(of: "#") {
            base = String(base[..<hash])
        }
        while base.hasSuffix("/") {
            base.removeLast()
        }
        switch surface {
        case .chat:
            return accessURL
        case .terminal:
            return URL(string: "\(base)/#/terminal") ?? accessURL
        }
    }

    enum CodingKeys: String, CodingKey {
        case id, title, accessURL, accessURLString, bearer, createdAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UUID.self, forKey: .id)
        title = try container.decode(String.self, forKey: .title)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        let storedString = try container.decodeIfPresent(String.self, forKey: .accessURLString)
        let storedURL = try container.decodeIfPresent(URL.self, forKey: .accessURL)
        let raw = storedString.flatMap(URL.init(string:)) ?? storedURL
        guard let raw else {
            throw DecodingError.dataCorruptedError(forKey: .accessURL, in: container, debugDescription: "Missing access URL")
        }
        let storedBearer = try container.decodeIfPresent(String.self, forKey: .bearer)
        accessURL = RemoteLink.attaching(token: storedBearer ?? RemoteLink.accessToken(in: raw), to: raw)
        bearer = storedBearer ?? RemoteLink.accessToken(in: accessURL)
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(title, forKey: .title)
        try container.encode(accessURL, forKey: .accessURL)
        try container.encode(accessURL.absoluteString, forKey: .accessURLString)
        try container.encodeIfPresent(pairingToken, forKey: .bearer)
        try container.encode(createdAt, forKey: .createdAt)
    }

    static func parse(_ raw: String) throws -> RemoteLink {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), let scheme = url.scheme?.lowercased() else {
            throw RemoteLinkError.invalidURL
        }
        if scheme == "termul" {
            return try parseDeepLink(url)
        }
        return try parseAccessURL(url)
    }

    private static func parseDeepLink(_ url: URL) throws -> RemoteLink {
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let embedded = components?.queryItems?.first(where: { $0.name == "url" })?.value
        let queryToken = components?.queryItems?.first(where: { $0.name == "access_token" })?.value
        let outerToken = accessToken(inFragment: url.fragment)
        guard let embedded, !embedded.isEmpty else {
            throw RemoteLinkError.invalidURL
        }
        var link = try parse(embedded)
        if link.pairingToken == nil, let token = queryToken ?? outerToken, !token.isEmpty {
            link = RemoteLink(id: link.id, title: link.title, accessURL: link.accessURL, bearer: token, createdAt: link.createdAt)
        }
        guard let token = link.pairingToken, !token.isEmpty else {
            throw RemoteLinkError.missingToken
        }
        return link
    }

    private static func parseAccessURL(_ url: URL) throws -> RemoteLink {
        guard let scheme = url.scheme?.lowercased(), let host = url.host(), !host.isEmpty else {
            throw RemoteLinkError.invalidURL
        }
        if scheme == "http" {
            guard isPrivateNetworkHost(host) else {
                throw RemoteLinkError.httpsRequired
            }
        } else if scheme != "https" {
            throw RemoteLinkError.httpsRequired
        }
        guard let token = accessToken(in: url), !token.isEmpty else {
            throw RemoteLinkError.missingToken
        }
        return RemoteLink(accessURL: url, bearer: token)
    }

    static func accessToken(in url: URL) -> String? {
        if let token = accessToken(inFragment: url.fragment) {
            return token
        }
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        return items?.first(where: { $0.name == "access_token" })?.value
    }

    static func accessToken(inFragment fragment: String?) -> String? {
        guard let fragment, !fragment.isEmpty else { return nil }
        let pairs = fragment.split(separator: "&")
        for pair in pairs {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2, parts[0] == "access_token" else { continue }
            let raw = String(parts[1])
            return raw.removingPercentEncoding ?? raw
        }
        return nil
    }

    static func attaching(token: String?, to url: URL) -> URL {
        guard let token, !token.isEmpty else { return url }
        if accessToken(in: url) == token { return url }
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.fragment = "access_token=\(token)"
        return components.url ?? url
    }

    static func isPrivateNetworkHost(_ host: String) -> Bool {
        let lowered = host.lowercased()
        if lowered == "localhost" || lowered.hasSuffix(".local") {
            return true
        }
        if lowered == "::1" || lowered.hasPrefix("fe80:") {
            return true
        }
        let parts = lowered.split(separator: ".").compactMap { UInt8($0) }
        guard parts.count == 4 else { return false }
        switch parts[0] {
        case 10, 127:
            return true
        case 169:
            return parts[1] == 254
        case 172:
            return (16 ... 31).contains(parts[1])
        case 192:
            return parts[1] == 168
        default:
            return false
        }
    }

    private static func displayTitle(for url: URL) -> String {
        url.host() ?? String(localized: "Saved connection")
    }
}

enum RemoteLinkError: LocalizedError {
    case invalidURL
    case httpsRequired
    case missingToken

    var errorDescription: String? {
        switch self {
        case .invalidURL:
            String(localized: "That does not look like a Se access link.")
        case .httpsRequired:
            String(localized: "Public hosts need HTTPS. A LAN address such as 192.168.x.x is allowed over HTTP.")
        case .missingToken:
            String(localized: "This link is missing the access secret. Copy or scan the full QR from the desktop.")
        }
    }
}
