import SwiftUI

@main
struct SeRemoteApp: App {
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
