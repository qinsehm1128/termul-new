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
    @StateObject private var shortcutAvailability = ShortcutAvailability.shared

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
        .commands { appCommands }
    }

    /// Hardware-keyboard surface: View menu (focus + zoom) and a Terminal menu,
    /// matching the menu-bar guidance from WWDC25 session 208 — items stay in
    /// place and simply dim when not actionable.
    @CommandsBuilder
    private var appCommands: some Commands {
        CommandGroup(after: .newItem) {
            Button(String(localized: "New Terminal")) {
                ShortcutCenter.send(.newTerminal)
            }
            .keyboardShortcut("t", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
        }
        CommandMenu(String(localized: "View")) {
            Button(String(localized: "Chat")) {
                ShortcutCenter.send(.focusChat)
            }
            .keyboardShortcut("1", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Button(String(localized: "Terminal")) {
                ShortcutCenter.send(.focusTerminal)
            }
            .keyboardShortcut("2", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Button(String(localized: "Files")) {
                ShortcutCenter.send(.toggleFiles)
            }
            .keyboardShortcut("3", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Divider()
            Button(String(localized: "Zoom In")) {
                ShortcutCenter.send(.textScaleUp)
            }
            .keyboardShortcut("+", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Button(String(localized: "Zoom Out")) {
                ShortcutCenter.send(.textScaleDown)
            }
            .keyboardShortcut("-", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Button(String(localized: "Actual Size")) {
                ShortcutCenter.send(.textScaleReset)
            }
            .keyboardShortcut("0", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
        }
        CommandMenu(String(localized: "Terminal")) {
            Button(String(localized: "Next Terminal")) {
                ShortcutCenter.send(.nextTerminal)
            }
            .keyboardShortcut("]", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
            Button(String(localized: "Previous Terminal")) {
                ShortcutCenter.send(.previousTerminal)
            }
            .keyboardShortcut("[", modifiers: .command)
            .disabled(!shortcutAvailability.sessionActive)
        }
    }
}
