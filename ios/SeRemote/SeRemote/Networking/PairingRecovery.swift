import Foundation

/// What the UI should offer after opening a host failed.
///
/// A saved host keeps the whole origin it was paired with
/// (`scheme://host:port`). The desktop does not promise to present the same one
/// on its next launch: a LAN listener can come back on another port and a quick
/// tunnel always comes back on another hostname. When that happens nothing the
/// phone can do reaches the host — a bare "Retry" is a dead end and the only
/// recovery is a fresh pairing scan. The same advice applies when the host *is*
/// reachable but refuses this device's credential.
///
/// Ranking matters: advice is only shown where a scan actually helps, so the
/// transient causes (no route to the network, a slow Mac) stay ordinary
/// retryable failures.
enum PairingRecovery {
    /// Whether a failed open means only a fresh scan can help.
    ///
    /// - Parameter origin: the saved origin that was dialed. A bare timeout is
    ///   ambiguous — a LAN address that stopped answering has usually moved (new
    ///   DHCP lease, another Wi-Fi) and only a scan carries the new one, while a
    ///   tunnel hostname that times out is just a slow Mac. Naming the origin
    ///   resolves that; without one, a timeout stays retryable.
    static func needsRePairing(_ error: Error, origin: URL? = nil) -> Bool {
        if let hostError = error as? HostError {
            switch hostError {
            case .rePairingRequired:
                return true
            case .deviceRevoked, .forbidden, .network, .ipc, .unexpected:
                return false
            }
        }
        guard let urlError = error as? URLError else { return false }
        switch urlError.code {
        case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost:
            return true
        case .timedOut:
            guard let origin else { return false }
            return RemoteLink.isPrivateNetworkHost(origin.host() ?? "")
        default:
            return false
        }
    }

}

/// Retry budget for opening (or re-opening) the whole host link.
///
/// The health probe used to treat a refused connection (`-1004`,
/// `cannotConnectToHost`) as a settled fact. On a phone that just resumed, or
/// whose Wi-Fi/cellular/VPN path changed, the first attempt can still land on
/// the previous path's dead connection — the device log shows
/// `nw_connection_copy_connected_remote_endpoint ... on unconnected
/// nw_connection` followed by `-1004`, and an immediate manual reconnect then
/// succeeds. A single such failure must not be presented as a hard
/// "connection lost" state, so the link retries transient transport failures a
/// few times with a short backoff and only reports the failure after the last
/// attempt.
///
/// Access failures are different: a revoked device or a rejected credential
/// cannot settle differently, so they surface immediately.
enum LinkOpenRetry {
    static let maxAttempts = 3

    /// Short, deterministic backoff: 300 ms then 700 ms. Three attempts settle
    /// within roughly one second when the failure is immediate (the common
    /// stale-path case), and inside the probe budget when it is a timeout.
    static func backoff(after attempt: Int) -> Duration {
        switch attempt {
        case 0: .milliseconds(300)
        default: .milliseconds(700)
        }
    }

    static func shouldRetry(_ error: Error) -> Bool {
        if error is CancellationError { return false }
        if let hostError = error as? HostError {
            switch hostError {
            case .network:
                // The origin answered something, or the tunnel dropped the
                // request: a second attempt can settle differently.
                return true
            case .deviceRevoked, .forbidden, .rePairingRequired, .ipc, .unexpected:
                return false
            }
        }
        guard let urlError = error as? URLError else { return false }
        switch urlError.code {
        case .timedOut, .networkConnectionLost, .cannotConnectToHost,
             .notConnectedToInternet, .cannotFindHost, .dnsLookupFailed,
             .secureConnectionFailed, .serverCertificateUntrusted:
            // A path that is still converging produces exactly these. They are
            // retried for the link, even though some of them still ask for a
            // fresh scan when the last attempt fails (see `needsRePairing`).
            return true
        default:
            return false
        }
    }

    /// Run `operation` up to `maxAttempts` times, retrying only transient link
    /// failures. `budget` bounds the whole sequence; once it is exhausted the
    /// last error is rethrown instead of starting another attempt.
    ///
    /// `operation` receives the zero-based attempt number so the caller can
    /// shrink its per-request timeout as the budget drains.
    static func run(
        budget: Duration? = nil,
        operation: @MainActor @escaping (Int) async throws -> Void
    ) async throws {
        let clock = ContinuousClock()
        let deadline = budget.map { clock.now.advanced(by: $0) }
        var lastError: (any Error)?
        for attempt in 0 ..< maxAttempts {
            if let deadline, lastError != nil, clock.now >= deadline { break }
            do {
                try await operation(attempt)
                return
            } catch {
                if error is CancellationError { throw error }
                lastError = error
                guard shouldRetry(error), attempt < maxAttempts - 1 else { throw error }
                let pause = backoff(after: attempt)
                if let deadline, clock.now.advanced(by: pause) >= deadline { throw error }
                try await Task.sleep(for: pause)
            }
        }
        throw lastError ?? HostError.network(String(localized: "The phone could not reach the host."))
    }
}
