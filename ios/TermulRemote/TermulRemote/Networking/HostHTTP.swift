import Foundation

struct IpcResult<T: Decodable>: Decodable {
    let success: Bool
    let data: T?
    let error: String?
    let code: String?
}

struct HostHTTP: Sendable {
    let origin: URL
    let credentials: HostCredentials

    init(origin: URL, credentials: HostCredentials) {
        self.origin = origin
        self.credentials = credentials
    }

    func get<T: Decodable>(_ path: String, query: [String: String] = [:]) async throws -> T {
        let result: IpcResult<T> = try await getEnvelope(path, query: query)
        return try unwrap(result)
    }

    func getEnvelope<T: Decodable>(_ path: String, query: [String: String] = [:]) async throws -> IpcResult<T> {
        var components = URLComponents(url: url(for: path), resolvingAgainstBaseURL: false)
        if !query.isEmpty {
            components?.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) }
        }
        guard let url = components?.url else {
            throw HostError.unexpected("Invalid URL for \(path)")
        }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.httpMethod = "GET"
        request.assumesHTTP3Capable = false
        credentials.apply(to: &request)
        return try await decode(request)
    }

    func post<T: Decodable>(_ path: String, body: [String: Any]) async throws -> T {
        let result: IpcResult<T> = try await sendJSON(path, method: "POST", body: body)
        return try unwrap(result)
    }

    func probeHealth() async throws {
        var lastError: Error?
        for attempt in 0 ..< 5 {
            do {
                try await probeHealthOnce()
                return
            } catch {
                lastError = error
                if attempt < 4 {
                    try await Task.sleep(for: .milliseconds(400 * (1 << attempt)))
                }
            }
        }
        throw lastError ?? HostError.network(String(localized: "The phone could not reach the host."))
    }

    private func probeHealthOnce() async throws {
        var request = URLRequest(url: origin.appendingPathComponent("health"), timeoutInterval: 12)
        request.httpMethod = "GET"
        request.assumesHTTP3Capable = false
        credentials.apply(to: &request)
        let (data, response) = try await HostURLSession.shared.data(for: request)
        let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard let http = response as? HTTPURLResponse, (200 ..< 300).contains(http.statusCode), text == "OK" || text.hasPrefix("OK") else {
            if RemoteLink.isPrivateNetworkHost(origin.host() ?? "") {
                throw HostError.network(String(localized: "The phone could not reach the Mac on this Wi-Fi. Confirm remote access is on and both devices are on the same network."))
            }
            throw HostError.network(String(localized: "The phone could not reach the host tunnel. Confirm remote access is still on, then retry."))
        }
    }

    private func sendJSON<T: Decodable>(_ path: String, method: String, body: [String: Any]) async throws -> IpcResult<T> {
        var request = URLRequest(url: url(for: path), timeoutInterval: 20)
        request.httpMethod = method
        request.assumesHTTP3Capable = false
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        credentials.apply(to: &request)
        request.httpBody = try WireJSON.data(from: body)
        return try await decode(request)
    }

    private func url(for path: String) -> URL {
        var base = origin.absoluteString
        while base.hasSuffix("/") {
            base.removeLast()
        }
        let suffix = path.hasPrefix("/") ? path : "/\(path)"
        return URL(string: base + suffix) ?? origin.appending(path: path)
    }

    private func decode<T: Decodable>(_ request: URLRequest) async throws -> IpcResult<T> {
        do {
            let (data, response) = try await HostURLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                throw HostError.network(String(localized: "The tunnel returned an unexpected response."))
            }
            if http.statusCode == 401 || http.statusCode == 403 {
                throw HostError.network(String(localized: "The access token was rejected. Scan the desktop QR again."))
            }
            if !(200 ..< 300).contains(http.statusCode) {
                throw HostError.network("HTTP \(http.statusCode)")
            }
            return try JSONDecoder().decode(IpcResult<T>.self, from: data)
        } catch let error as HostError {
            throw error
        } catch let error as DecodingError {
            throw HostError.unexpected(error.localizedDescription)
        } catch {
            throw HostError.network(Self.describe(error))
        }
    }

    private func unwrap<T: Decodable>(_ result: IpcResult<T>) throws -> T {
        if result.success, let data = result.data {
            return data
        }
        throw HostError.ipc(result.error ?? String(localized: "The host rejected the request."), result.code ?? "NETWORK_ERROR")
    }

    private static func describe(_ error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .notConnectedToInternet:
                return String(localized: "This iPhone is offline.")
            case .timedOut:
                return String(localized: "The tunnel timed out. Check that remote access is still on.")
            case .cannotFindHost, .dnsLookupFailed:
                return String(localized: "The phone could not resolve the tunnel hostname. If the Mac uses a VPN, the iPhone needs a working path to Cloudflare too.")
            case .cannotConnectToHost, .networkConnectionLost:
                return String(localized: "The phone could not reach the host tunnel. Confirm remote access is still on, then retry.")
            case .secureConnectionFailed, .serverCertificateUntrusted:
                return String(localized: "The HTTPS tunnel failed its certificate check.")
            default:
                break
            }
        }
        return error.localizedDescription
    }
}
