import Foundation

/// View transitions are not connection-lifecycle events. Only the root's
/// authoritative link selection may release the session's sockets.
enum ShellConnectionLifecycle {
    static func shouldStop(disappearingLinkID: UUID, activeLinkID: UUID?) -> Bool {
        activeLinkID != disappearingLinkID
    }
}
