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
    static func needsRePairing(_ error: Error) -> Bool {
        if let hostError = error as? HostError {
            switch hostError {
            case .rePairingRequired:
                return true
            case .network, .ipc, .unexpected:
                return false
            }
        }
        guard let urlError = error as? URLError else { return false }
        switch urlError.code {
        case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost:
            return true
        default:
            return false
        }
    }
}
