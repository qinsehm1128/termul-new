import Foundation

/// Public, non-secret identity discovery. Old hosts may return 404 or have no
/// identity field; this optional request must never become a connection gate.
struct HostIdentityClient: Sendable {
    var session: URLSession = .shared

    nonisolated func fetch(origin: URL) async -> String? {
        var request = URLRequest(url: origin.appendingPathComponent("info"), cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 1.5)
        request.httpMethod = "GET"
        request.assumesHTTP3Capable = false
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else { return nil }
            return Self.decode(data)
        } catch {
            return nil
        }
    }

    nonisolated static func decode(_ data: Data) -> String? {
        guard let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let raw = body["hostId"] as? String else { return nil }
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        return UUID(uuidString: value)?.uuidString.lowercased() ?? value
    }
}
