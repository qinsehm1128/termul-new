import Foundation

extension HostHTTP {
    /// `GET /fs/image?path=...` returns raw image bytes, not an IPC envelope.
    /// The host authorises GET file reads with the `view` scope and confines
    /// the canonical path to its project boundary.
    func fileImage(path: String) async throws -> Data {
        guard let url = Self.fileImageURL(origin: origin, path: path) else {
            throw HostError.unexpected("Invalid image URL")
        }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.httpMethod = "GET"
        request.assumesHTTP3Capable = false
        // Image previews are read-only snapshots: never serve a cached copy.
        request.cachePolicy = .reloadIgnoringLocalCacheData
        credentials.apply(to: &request)
        let (data, response) = try await HostURLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw HostError.network(String(localized: "The tunnel returned an unexpected response."))
        }
        if let failure = Self.fileResponseError(statusCode: http.statusCode, data: data) {
            failure.publishAccessFailure(origin: origin)
            throw failure
        }
        return data
    }

    /// Pure URL construction so the query encoding is testable.
    nonisolated static func fileImageURL(origin: URL, path: String) -> URL? {
        var base = origin.absoluteString
        while base.hasSuffix("/") {
            base.removeLast()
        }
        guard var components = URLComponents(string: base + "/fs/image") else { return nil }
        components.queryItems = [URLQueryItem(name: "path", value: path)]
        return components.url
    }

    /// `/fs/image` answers with `{ success: false, error, code }` on failure and
    /// raw bytes on success. Access failures keep their existing handling.
    nonisolated static func fileResponseError(statusCode: Int, data: Data) -> HostError? {
        if statusCode == 401 || statusCode == 403,
           let accessError = responseError(statusCode: statusCode, data: data) {
            return accessError
        }
        guard !(200 ..< 300).contains(statusCode) else { return nil }
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if let code = object?["code"] as? String, !code.isEmpty {
            return .ipc(object?["error"] as? String ?? code, code)
        }
        return .network("HTTP \(statusCode)")
    }
}
