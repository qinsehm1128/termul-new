import SwiftUI
import UIKit

enum AppLaunchURL {
    private static let lock = NSLock()
    private static var pending: URL?

    static func set(_ url: URL) {
        lock.lock()
        pending = url
        lock.unlock()
    }

    static func take() -> URL? {
        lock.lock()
        defer { lock.unlock() }
        let url = pending
        pending = nil
        return url
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        if let url = launchOptions?[.url] as? URL {
            AppLaunchURL.set(url)
        }
        return true
    }

    func application(_ app: UIApplication, open url: URL, options: [UIApplication.OpenURLOptionsKey: Any] = [:]) -> Bool {
        AppLaunchURL.set(url)
        NotificationCenter.default.post(name: .seOpenURL, object: url)
        return true
    }
}

extension Notification.Name {
    static let seOpenURL = Notification.Name("se.openURL")
}

@main
struct SeRemoteApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var store = ConnectionStore()
    @State private var settings = AppSettings()

    var body: some Scene {
        WindowGroup {
            RootView(store: store, settings: settings)
                .environment(\.locale, settings.locale)
                .preferredColorScheme(settings.appearance.colorScheme)
                .tint(SeTheme.accent)
                .onOpenURL { store.openIncomingURL($0) }
                .onReceive(NotificationCenter.default.publisher(for: .seOpenURL)) { notification in
                    if let url = notification.object as? URL {
                        store.openIncomingURL(url)
                    }
                }
                .task {
                    store.consumePendingLaunchURL()
                }
        }
    }
}
