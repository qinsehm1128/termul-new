import Foundation

/// Entry gating and navigation policy for the remote-access web app shown inside
/// the app's plugins browser.
///
/// The browser is the one place in the app that loads arbitrary author markup
/// from the host, so the policy is a pure value type: the view can be thin and
/// the rules are exercised without a web view.
enum PluginBrowserPolicy {
    /// Route the renderer understands next to the credential fragment:
    /// `#access_token=<token>&route=/plugins`.
    nonisolated static let pluginsRoute = "/plugins"

    nonisolated enum Decision: Equatable, Sendable {
        case allow
        case openExternally(URL)
        case cancel
    }

    // MARK: - Bootstrap URL

    /// The first URL the browser loads: the paired origin's root with the
    /// credential and the plugins route in the fragment.
    ///
    /// The token is percent-encoded with unreserved characters only. The fragment
    /// grammar splits on `&` and `=`, and the renderer reads it with
    /// `URLSearchParams` (where a bare `+` means a space), so `&`, `=`, `#`, `+`,
    /// `%` and spaces must never reach it raw. `nil` means the caller has no usable
    /// credential or origin and should show the pairing state instead of loading.
    nonisolated static func bootstrapURL(origin: URL, token: String?) -> URL? {
        guard let token, !token.isEmpty else { return nil }
        guard let scheme = origin.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return nil }
        guard let host = origin.host(), !host.isEmpty else { return nil }
        guard var components = URLComponents(url: origin, resolvingAgainstBaseURL: false) else { return nil }
        // The bootstrap is an origin, never the path/query/fragment of the saved link.
        components.path = ""
        components.query = nil
        components.fragment = nil
        components.user = nil
        guard var base = components.url?.absoluteString else { return nil }
        while base.hasSuffix("/") { base.removeLast() }
        let fragment = "access_token=\(encodedToken(token))&route=\(pluginsRoute)"
        // Build the string by hand: assigning `fragment` via URLComponents
        // would re-encode the already valid `%XX` escapes.
        return URL(string: "\(base)/#\(fragment)")
    }

    /// RFC 3986 unreserved characters: letters, digits, `-._~`.
    nonisolated static func encodedToken(_ token: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return token.addingPercentEncoding(withAllowedCharacters: allowed) ?? token
    }

    // MARK: - Entry gating

    /// Plugins are host-scoped, not terminal-scoped: the hub must offer this
    /// entry without an open terminal. It is available only on a live link that
    /// grants the `view` scope. Legacy bearers without scopes stay allowed, like
    /// every other feature's `allows(_:)` fallback.
    nonisolated static func entryEnabled(isRevoked: Bool, scopes: [String]?) -> Bool {
        !isRevoked && (scopes?.contains("view") ?? true)
    }

    // MARK: - Navigation

    /// Decide a navigation action.
    ///
    /// Main frame: stay on the paired origin (scheme, host and normalized port),
    /// allow the initial `about:blank`, send other web pages to the system
    /// browser and refuse every other scheme.
    ///
    /// Subframes: the plugin host is expected to sandbox plugin content in
    /// same-origin iframes, and its CSP governs what those frames load, so
    /// web/data/blob frames are allowed here.
    nonisolated static func decide(url: URL?, isMainFrame: Bool, origin: URL) -> Decision {
        guard let url else { return .cancel }
        let scheme = url.scheme?.lowercased()
        if isMainFrame {
            if scheme == "about", url.absoluteString.lowercased() == "about:blank" { return .allow }
            guard scheme == "http" || scheme == "https" else { return .cancel }
            return sameOrigin(url, origin) ? .allow : .openExternally(url)
        }
        switch scheme {
        case "http", "https", "about", "data", "blob":
            return .allow
        default:
            return .cancel
        }
    }

    /// Same scheme + host + effective port, with `http`/`https` default ports
    /// normalized so `https://host` and `https://host:443` match.
    nonisolated static func sameOrigin(_ lhs: URL, _ rhs: URL) -> Bool {
        guard let left = normalizedOrigin(lhs), let right = normalizedOrigin(rhs) else { return false }
        return left == right
    }

    nonisolated private static func normalizedOrigin(_ url: URL) -> String? {
        guard let scheme = url.scheme?.lowercased(), let host = url.host(), !host.isEmpty else { return nil }
        let defaultPort: Int? = switch scheme {
        case "http": 80
        case "https": 443
        default: nil
        }
        let port = url.port ?? defaultPort
        return "\(scheme)://\(host.lowercased()):\(port ?? 0)"
    }
}

extension ToolDestination {
    /// Whether this hub entry is usable for the active link. Plugins do not
    /// depend on the terminal scope; every other tool keeps its existing gate.
    func browserEntryEnabled(for link: RemoteLink?) -> Bool {
        guard self == .plugins else { return true }
        guard let link else { return true }
        return PluginBrowserPolicy.entryEnabled(isRevoked: link.isRevoked, scopes: link.scopes)
    }
}
