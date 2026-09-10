import Foundation
import UserNotifications

/// Local notifications for host events that land while the app is
/// backgrounded: turn completion, permission/question requests, terminal
/// exit. Mirrors the browser client's Web Notifications behavior when remote.
@MainActor
enum SeNotifications {
    static func requestAuthorization() async -> Bool {
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral:
            return true
        case .denied:
            HostLog.ui.info("Notification authorization previously denied")
            return false
        default:
            break
        }
        do {
            let granted = try await center.requestAuthorization(options: [.alert, .sound])
            HostLog.ui.info("Notification authorization \(granted ? "granted" : "declined", privacy: .public)")
            return granted
        } catch {
            HostLog.ui.error("Notification authorization failed: \(error.localizedDescription)")
            return false
        }
    }

    static func post(title: String, body: String) async {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let request = UNNotificationRequest(
            identifier: "se.event.\(UUID().uuidString)",
            content: content,
            trigger: nil
        )
        do {
            try await UNUserNotificationCenter.current().add(request)
        } catch {
            HostLog.ui.error("Local notification failed: \(error.localizedDescription)")
        }
    }
}
