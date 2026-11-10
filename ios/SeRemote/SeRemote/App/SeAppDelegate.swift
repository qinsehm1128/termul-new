import UIKit
import UserNotifications

/// User-info keys shared by the notification poster and the delegate.
nonisolated enum SeNotificationKey {
    static let hostID = "hostID"
    static let terminalID = "terminalID"
}

extension Notification.Name {
    /// A local-notification tap asked for a destination. `userInfo` carries
    /// `SeNotificationKey.hostID` and, for terminal events, `terminalID`.
    static let seOpenTerminal = Notification.Name("seOpenTerminal")
}

/// Turns a local-notification tap into an in-app destination.
///
/// `RootView` owns the routing decision (`ConnectionStore`), so a tap while the
/// app is cold or already open takes the same path. A denormalized payload is
/// harmless: a missing terminal id only asks for the host.
@MainActor
final class SeAppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        var route: [String: String] = [:]
        if let hostID = info[SeNotificationKey.hostID] as? String, !hostID.isEmpty {
            route[SeNotificationKey.hostID] = hostID
        }
        if let terminalID = info[SeNotificationKey.terminalID] as? String, !terminalID.isEmpty {
            route[SeNotificationKey.terminalID] = terminalID
        }
        await MainActor.run {
            NotificationCenter.default.post(name: .seOpenTerminal, object: nil, userInfo: route)
        }
    }

    /// Banner while the app is foregrounded. The poster only fires while
    /// backgrounded, so this is a safety net rather than the normal path.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }
}
