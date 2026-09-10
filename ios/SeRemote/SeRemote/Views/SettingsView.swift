import SwiftUI

struct SettingsView: View {
    @Bindable var settings: AppSettings
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker(String(localized: "Language"), selection: $settings.language) {
                        Text("System").tag(AppLanguage.system)
                        Text("English").tag(AppLanguage.english)
                        Text("Simplified Chinese").tag(AppLanguage.simplifiedChinese)
                    }
                    Picker(String(localized: "Appearance"), selection: $settings.appearance) {
                        Text("System").tag(AppAppearance.system)
                        Text("Dark").tag(AppAppearance.dark)
                        Text("Light").tag(AppAppearance.light)
                    }
                    Toggle(isOn: $settings.notificationsEnabled) {
                        Label(String(localized: "Background alerts"), systemImage: "bell")
                    }
                } footer: {
                    Text("Language and appearance apply to this app. Background alerts notify when the agent finishes a turn, asks for approval, or a terminal exits while the app is in the background.")
                }

                Section(String(localized: "How it works")) {
                    LabeledContent(String(localized: "Chat")) {
                        Text("Continues the desk agent over the tunnel WebSocket.")
                            .foregroundStyle(.secondary)
                    }
                    LabeledContent(String(localized: "Terminal")) {
                        Text("Watches a live host PTY in a native terminal.")
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .navigationTitle(String(localized: "Settings"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(String(localized: "Done")) { dismiss() }
                }
            }
            .onChange(of: settings.notificationsEnabled) { _, enabled in
                guard enabled else { return }
                Task { @MainActor in
                    let granted = await SeNotifications.requestAuthorization()
                    if !granted {
                        settings.notificationsEnabled = false
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
