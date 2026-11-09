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

    /// Whether another attempt can still change the answer.
    ///
    /// The connect probe runs inside a fixed budget so an unreachable saved
    /// origin reaches the one-tap rescan state within seconds. Retrying a refused
    /// connection or an unknown host spends that budget on a settled fact; only a
    /// stalled or dropped request can settle differently.
    static func isWorthRetrying(_ error: Error) -> Bool {
        if let hostError = error as? HostError {
            switch hostError {
            case .network:
                // The origin answered something: a tunnel 502 or a listener that
                // is still coming up can settle on a second try.
                return true
            case .deviceRevoked, .forbidden, .rePairingRequired, .ipc, .unexpected:
                return false
            }
        }
        guard let urlError = error as? URLError else { return false }
        switch urlError.code {
        case .timedOut, .networkConnectionLost:
            return true
        default:
            return false
        }
    }
}
