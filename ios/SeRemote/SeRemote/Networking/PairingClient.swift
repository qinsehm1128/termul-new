import Foundation

/// Exchanges the fragment-only, one-time code without sending any host bearer.
struct PairingClient {
    var session: URLSession = .shared

    func exchange(link: RemoteLink, name: String) async throws -> RemoteLink {
        guard let code = link.pairCode else {
            var identified = link
            if identified.stableHostID == nil {
                identified.hostId = await HostIdentityClient(session: session).fetch(origin: link.originURL)
            }
            return identified
        }
        var request = URLRequest(url: link.originURL.appendingPathComponent("pair"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body = PairRequest(code: code, name: name, platform: "ios")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw PairingExchangeError.failed }
        guard (200..<300).contains(http.statusCode) else {
            let error = try? JSONDecoder().decode(PairFailure.self, from: data)
            switch error?.code {
            case "INVALID_PAIRING_CODE": throw PairingExchangeError.expired
            case "DEVICE_LIMIT_REACHED": throw PairingExchangeError.limit
            default: throw PairingExchangeError.failed
            }
        }
        let grant = try JSONDecoder().decode(PairExchange.self, from: data)
        guard !grant.deviceId.isEmpty, !grant.deviceToken.isEmpty else { throw PairingExchangeError.failed }
        var paired = RemoteLink(id: link.id, title: link.title, accessURL: link.originURL,
                                bearer: grant.deviceToken, createdAt: link.createdAt)
        paired.hostId = grant.hostId
        paired.deviceId = grant.deviceId
        paired.deviceName = name
        paired.scopes = grant.scopes
        if paired.stableHostID == nil {
            paired.hostId = await HostIdentityClient(session: session).fetch(origin: link.originURL)
        }
        return paired
    }

    private struct PairRequest: Encodable {
        let code: String
        let name: String
        let platform: String
    }
    private struct PairFailure: Decodable { let code: String }
}

enum PairingExchangeError: LocalizedError {
    case expired, limit, failed
    var errorDescription: String? {
        switch self {
        case .expired: String(localized: "Pairing code expired. Scan a new QR code on the host.")
        case .limit: String(localized: "The host device limit has been reached.")
        case .failed: String(localized: "Pairing failed. Try a new QR code on the host.")
        }
    }
}
