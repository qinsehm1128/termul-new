import Foundation
import SwiftUI

/// View transitions are not connection-lifecycle events. Only the root's
/// authoritative link selection may release the session's sockets.
enum ShellConnectionLifecycle {
    /// Sockets belong to one (saved host, origin) pair. A rescan that keeps the
    /// saved record but moves the address must tear down the session dialing the
    /// old one, so the comparison uses the link's session identity, not its id.
    static func shouldStop(disappearingSession: String, activeSession: String?) -> Bool {
        activeSession != disappearingSession
    }

    /// The one connection-lifecycle decision a scene-phase change produces.
    /// Transient phases must never reach the session: Control Center, the
    /// notification shade, an app-switcher peek and system banners all pass
    /// through `.inactive`, and each of those used to fire a full-screen
    /// reconnect of a session that was still healthy.
    enum SceneLifecycleAction: Equatable {
        /// Nothing happened that touches the transports.
        case none
        /// The scene left the foreground for real: park polling and sockets.
        case enterBackground
        /// The scene returned after a real background stay: revalidate.
        case enterForeground
    }

    /// Pure classification of one scene-phase sample.
    ///
    /// - `.background` while not already backgrounded is the only entry into
    ///   background; the OS delivers it after `.inactive`, so the `.inactive`
    ///   step itself is not a background event.
    /// - `.active` only matters as a *return*: after a real background stay.
    ///   Becoming active after a transient `.inactive` (banners, Control
    ///   Center) returns `.none`, which is what stops the spurious
    ///   foreground-reconnect storms.
    static func sceneAction(isActive: Bool, isBackground: Bool, wasBackgrounded: Bool) -> SceneLifecycleAction {
        if isBackground {
            return wasBackgrounded ? .none : .enterBackground
        }
        if isActive {
            return wasBackgrounded ? .enterForeground : .none
        }
        // `.inactive` in every spelling: transient chrome or an in-flight
        // background/active transition whose own terminal state decides.
        return .none
    }

    /// Convenience for SwiftUI's three-state `ScenePhase`.
    @MainActor
    static func sceneAction(for phase: ScenePhase, wasBackgrounded: Bool) -> SceneLifecycleAction {
        sceneAction(
            isActive: phase == .active,
            isBackground: phase == .background,
            wasBackgrounded: wasBackgrounded
        )
    }
}
