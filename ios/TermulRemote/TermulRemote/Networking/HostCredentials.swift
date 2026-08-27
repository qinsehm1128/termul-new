import Foundation

struct HostCredentials: Sendable {
    let origin: URL
    let bearer: String?

    init(accessURL: URL, bearer storedBearer: String? = nil) {
        origin = HostCredentials.origin(from: accessURL)
        bearer = storedBearer ?? RemoteLink.accessToken(in: accessURL)
    }

    func apply(to request: inout URLRequest) {
        if let bearer, !bearer.isEmpty {
            request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        }
        // Desktop `/ws` and `/terminal/ws` reject upgrades without an allowed
        // browser Origin. URLSession does not send one unless we set it.
        request.setValue(originString, forHTTPHeaderField: "Origin")
    }

    var originString: String {
        var components = URLComponents(url: origin, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.path = ""
        components.query = nil
        components.fragment = nil
        if let host = components.host, let scheme = components.scheme {
            if let port = components.port {
                return "\(scheme)://\(host):\(port)"
            }
            return "\(scheme)://\(host)"
        }
        return origin.absoluteString
    }

    private static func origin(from url: URL) -> URL {
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.fragment = nil
        components.query = nil
        components.path = ""
        return components.url ?? url
    }

}
