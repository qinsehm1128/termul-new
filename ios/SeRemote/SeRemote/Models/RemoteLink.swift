import Darwin
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
    var hostId: String? = nil
    var deviceId: String? = nil
    var deviceName: String? = nil
    var scopes: [String]? = nil
    var isRevoked = false
    /// One-time code: never encoded or saved to Keychain.
    var pairCode: String? = nil
    var createdAt: Date

    init(id: UUID = UUID(), title: String? = nil, accessURL: URL, bearer: String? = nil, createdAt: Date = .now) {
        self.id = id
        self.title = title ?? RemoteLink.displayTitle(for: accessURL)
        self.accessURL = RemoteLink.attaching(token: bearer ?? RemoteLink.accessToken(in: accessURL), to: accessURL)
        self.bearer = bearer ?? RemoteLink.accessToken(in: self.accessURL)
        self.createdAt = createdAt
        self.pairCode = Self.fragmentValue("pair", in: accessURL.fragment)
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

    /// What the saved-host record keeps: the origin, port included, without the
    /// bearer (the secret lives in the keychain). The port belongs to the host
    /// process that issued the QR, so the desktop has to present the same one on
    /// its next launch for this record to keep working.
    var savedOrigin: String {
        originURL.absoluteString
    }

    /// Stable, non-secret host identity, normalized for UUID casing. Missing
    /// identities retain the older origin / same-credential matching policy.
    var stableHostID: String? {
        guard let value = hostId?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else { return nil }
        return UUID(uuidString: value)?.uuidString.lowercased() ?? value
    }

    var hostIdentity: String? {
        if let stableHostID { return "host:\(stableHostID)" }
        guard let token = pairingToken, !token.isEmpty else { return nil }
        return "credential:\(token)"
    }

    /// A known identity mismatch must never be hidden by a recycled address.
    func refersToSameHost(as other: RemoteLink) -> Bool {
        if let mine = stableHostID, let theirs = other.stableHostID {
            return mine == theirs
        }
        if let mine = pairingToken, !mine.isEmpty, mine == other.pairingToken { return true }
        return originHost.caseInsensitiveCompare(other.originHost) == .orderedSame
    }

    /// Identity of the live session this link opens: the saved record plus the
    /// origin it dials. Keeping the record across a rescan must still replace the
    /// session that dials the dead origin, so the address belongs in the key.
    var sessionIdentity: String {
        "\(id.uuidString)#\(savedOrigin)"
    }

    /// Fold a fresh scan of this same host in: the record keeps its identity
    /// (`id`, so the keychain account and the home-screen activity timestamps stay
    /// attached) and a name the user gave it, while the origin and credential
    /// become the ones that were just scanned.
    func adoptingRescan(_ fresh: RemoteLink) -> RemoteLink {
        let userNamed = title != RemoteLink.displayTitle(for: accessURL)
        var merged = RemoteLink(
            id: id,
            title: userNamed && !fresh.title.isEmpty ? title : fresh.title,
            accessURL: fresh.accessURL,
            bearer: fresh.deviceId != nil ? fresh.pairingToken : (deviceId != nil ? pairingToken : fresh.pairingToken ?? pairingToken),
            createdAt: createdAt
        )
        merged.hostId = fresh.stableHostID ?? stableHostID
        merged.deviceId = fresh.deviceId ?? deviceId
        merged.deviceName = fresh.deviceName ?? deviceName
        merged.scopes = fresh.scopes ?? scopes
        merged.isRevoked = fresh.deviceId != nil ? false : isRevoked
        merged.pairCode = fresh.pairCode
        return merged
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
        case id, title, accessURL, accessURLString, bearer, createdAt, hostId, deviceId, deviceName, scopes, isRevoked
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
        hostId = try container.decodeIfPresent(String.self, forKey: .hostId)
        deviceId = try container.decodeIfPresent(String.self, forKey: .deviceId)
        deviceName = try container.decodeIfPresent(String.self, forKey: .deviceName)
        scopes = try container.decodeIfPresent([String].self, forKey: .scopes)
        isRevoked = try container.decodeIfPresent(Bool.self, forKey: .isRevoked) ?? false
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(title, forKey: .title)
        var components = URLComponents(url: accessURL, resolvingAgainstBaseURL: false)
        if let fragment = components?.fragment {
            let retained = fragment.split(separator: "&").filter { !$0.hasPrefix("pair=") }.joined(separator: "&")
            components?.fragment = retained.isEmpty ? nil : retained
        }
        let storedURL = components?.url ?? originURL
        try container.encode(storedURL, forKey: .accessURL)
        try container.encode(storedURL.absoluteString, forKey: .accessURLString)
        try container.encodeIfPresent(pairingToken, forKey: .bearer)
        try container.encode(createdAt, forKey: .createdAt)
        try container.encodeIfPresent(stableHostID, forKey: .hostId)
        try container.encodeIfPresent(deviceId, forKey: .deviceId)
        try container.encodeIfPresent(deviceName, forKey: .deviceName)
        try container.encodeIfPresent(scopes, forKey: .scopes)
        try container.encode(isRevoked, forKey: .isRevoked)
    }

    /// Deep-link schemes this build owns, read out of its own `CFBundleURLTypes`
    /// registration rather than typed here a second time.
    ///
    /// The registration is the single source on purpose. The system routes a URL
    /// to this app *because* that entry exists, so a scheme spelled again as a
    /// literal here can drift from it, and the drift fails in the worst
    /// direction: the app is launched with a URL its own parser then rejects, and
    /// `RemoteLinkError.invalidURL` names no cause a user or a log could act on.
    ///
    /// The pre-rename `termul` scheme is deliberately absent, not forgotten.
    /// Dropping it is a locked decision: a pre-rename link saved outside the app
    /// — a Safari bookmark, a message thread — stops opening. Nothing the desktop
    /// hands out is affected, because pairing has never produced a deep link; the
    /// QR and the copy button both carry an `http` or `https` access URL, which reaches
    /// ``parseAccessURL(_:)`` without consulting this set at all.
    private static let deepLinkSchemes: Set<String> = {
        guard let types = Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]] else {
            HostLog.session.error("Bundle registers no CFBundleURLTypes; deep links cannot be parsed")
            return []
        }
        let schemes = Set(types.flatMap { ($0["CFBundleURLSchemes"] as? [String]) ?? [] }.map { $0.lowercased() })
        if schemes.isEmpty {
            HostLog.session.error("CFBundleURLTypes registers no scheme; deep links cannot be parsed")
        }
        return schemes
    }()

    static func parse(_ raw: String) throws -> RemoteLink {
        try parse(raw, fallbackToken: nil)
    }

    private static func parse(_ raw: String, fallbackToken: String?, fallbackPair: String? = nil) throws -> RemoteLink {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), let scheme = url.scheme?.lowercased() else {
            throw RemoteLinkError.invalidURL
        }
        if deepLinkSchemes.contains(scheme) {
            return try parseDeepLink(url)
        }
        return try parseAccessURL(url, fallbackToken: fallbackToken, fallbackPair: fallbackPair)
    }

    private static func parseDeepLink(_ url: URL) throws -> RemoteLink {
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let embedded = components?.queryItems?.first(where: { $0.name == "url" })?.value
        let queryToken = components?.queryItems?.first(where: { $0.name == "access_token" })?.value
        let outerToken = accessToken(inFragment: url.fragment)
        let outerPair = fragmentValue("pair", in: url.fragment)
        guard let embedded, !embedded.isEmpty else {
            throw RemoteLinkError.invalidURL
        }
        // A deep link may carry the bearer outside the embedded URL
        // (`se://open?url=…&access_token=…`). Hand it to the access-URL parser so
        // the embedded URL is still validated for scheme and host, and so the
        // token survives without rewriting the embedded fragment.
        let fallbackToken = [queryToken, outerToken]
            .compactMap { $0 }
            .first { !$0.isEmpty }
        var link = try parse(embedded, fallbackToken: fallbackToken, fallbackPair: outerPair)
        if link.pairingToken == nil, let token = queryToken ?? outerToken, !token.isEmpty {
            link = RemoteLink(id: link.id, title: link.title, accessURL: link.accessURL, bearer: token, createdAt: link.createdAt)
        }
        guard link.pairCode != nil || link.pairingToken?.isEmpty == false else {
            throw RemoteLinkError.missingToken
        }
        return link
    }

    private static func parseAccessURL(_ url: URL, fallbackToken: String? = nil, fallbackPair: String? = nil) throws -> RemoteLink {
        guard let scheme = url.scheme?.lowercased(), let host = url.host(), !host.isEmpty else {
            throw RemoteLinkError.invalidURL
        }
        guard scheme == "http" || scheme == "https" else {
            throw RemoteLinkError.httpsRequired
        }
        let embeddedToken = accessToken(in: url).flatMap { $0.isEmpty ? nil : $0 }
        let token = embeddedToken ?? fallbackToken
        let pair = fragmentValue("pair", in: url.fragment).flatMap { $0.isEmpty ? nil : $0 } ?? fallbackPair
        guard pair?.isEmpty == false || token?.isEmpty == false else {
            throw RemoteLinkError.missingToken
        }
        // HTTP is allowed for operator-shared pairing URLs (LAN or an HTTP
        // reverse tunnel). The bearer in the fragment is the access secret;
        // scheme is not a substitute for it.
        var link = RemoteLink(accessURL: url, bearer: token)
        link.pairCode = pair
        return link
    }

    static func accessToken(in url: URL) -> String? {
        if let token = accessToken(inFragment: url.fragment) {
            return token
        }
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        return items?.first(where: { $0.name == "access_token" })?.value
    }

    static func accessToken(inFragment fragment: String?) -> String? {
        fragmentValue("access_token", in: fragment)
    }

    static func fragmentValue(_ key: String, in fragment: String?) -> String? {
        guard let fragment, !fragment.isEmpty else { return nil }
        let pairs = fragment.split(separator: "&")
        for pair in pairs {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2, parts[0] == Substring(key) else { continue }
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

    /// True when the host IPv4 sits inside one of this device's own on-link
    /// subnets AND the range itself is safe for cleartext (RFC1918, CGNAT
    /// RFC 6598, loopback, link-local). "Same Wi-Fi" establishes
    /// reachability, but only non-routable numbering keeps a sniffed bearer
    /// from being useful off-segment — campus-public L2 must pair over HTTPS.
    static func isOnLinkIPv4Host(_ host: String) -> Bool {
        let octets = host.split(separator: ".").compactMap { UInt8($0) }
        guard octets.count == 4, isCleartextSafeOctets(octets),
              let target = Self.ipv4(host) else { return false }
        let targetAddress = target.s_addr
        var interfaceList: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&interfaceList) == 0, let first = interfaceList else { return false }
        defer { freeifaddrs(interfaceList) }
        var cursor: UnsafeMutablePointer<ifaddrs>? = first
        while let current = cursor {
            defer { cursor = current.pointee.ifa_next }
            guard let name = current.pointee.ifa_name,
                  String(cString: name).hasPrefix("en"),
                  let addressSockaddr = current.pointee.ifa_addr,
                  addressSockaddr.pointee.sa_family == UInt8(AF_INET),
                  let maskSockaddr = current.pointee.ifa_netmask,
                  maskSockaddr.pointee.sa_family == UInt8(AF_INET)
            else { continue }
            let address = addressSockaddr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                $0.pointee.sin_addr.s_addr
            }
            let mask = maskSockaddr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                $0.pointee.sin_addr.s_addr
            }
            guard address != 0, mask != 0 else { continue }
            if address & mask == targetAddress & mask {
                return true
            }
        }
        return false
    }

    private static func ipv4(_ string: String) -> in_addr? {
        var address = in_addr()
        let result = string.withCString { inet_pton(AF_INET, $0, &address) }
        return result == 1 ? address : nil
    }

    /// Cleartext HTTP pairing is confined to ranges that are not globally
    /// routable: RFC1918, CGNAT (100.64.0.0/10), loopback, link-local.
    /// Mirrors the host-side publish allow-list (src-tauri lan.rs) — keep the
    /// two policies in step.
    private static func isCleartextSafeOctets(_ octets: [UInt8]) -> Bool {
        guard octets.count == 4 else { return false }
        switch octets[0] {
        case 10, 127:
            return true
        case 169:
            return octets[1] == 254
        case 172:
            return (16 ... 31).contains(octets[1])
        case 192:
            return octets[1] == 168
        case 100:
            return (64 ... 127).contains(octets[1])
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
            String(localized: "Use an http or https access link from the desktop QR.")
        case .missingToken:
            String(localized: "This link is missing the access secret. Copy or scan the full QR from the desktop.")
        }
    }
}
