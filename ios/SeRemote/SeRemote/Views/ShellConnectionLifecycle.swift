import Foundation

/// View transitions are not connection-lifecycle events. Only the root's
/// authoritative link selection may release the session's sockets.
enum ShellConnectionLifecycle {
    /// Sockets belong to one (saved host, origin) pair. A rescan that keeps the
    /// saved record but moves the address must tear down the session dialing the
    /// old one, so the comparison uses the link's session identity, not its id.
    static func shouldStop(disappearingSession: String, activeSession: String?) -> Bool {
        activeSession != disappearingSession
    }
}
