import SwiftUI

@MainActor
struct SettingsView: View {
    @Bindable var settings: AppSettings
    @Environment(\.dismiss) private var dismiss
    @State private var terminalTextScale = TerminalTextScale.current
    @State private var notificationDenied = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("Language", selection: $settings.language) {
                        Text("System").tag(AppLanguage.system)
                        Text("English").tag(AppLanguage.english)
                        Text("Simplified Chinese").tag(AppLanguage.simplifiedChinese)
                    }
                    Picker("Appearance", selection: $settings.appearance) {
                        Text("System").tag(AppAppearance.system)
                        Text("Dark").tag(AppAppearance.dark)
                        Text("Light").tag(AppAppearance.light)
                    }
                } header: {
                    Text("Appearance")
                } footer: {
                    Text("Language and appearance apply to this app.")
                }

                Section {
                    Toggle(isOn: $settings.hapticsEnabled) {
                        Label("Haptic feedback", systemImage: "hand.tap")
                    }
                    Toggle(isOn: $settings.notificationsEnabled) {
                        Label("Background alerts", systemImage: "bell")
                    }
                    if notificationDenied {
                        Label(
                            "Notifications are not allowed. Enable them in iOS Settings to receive background alerts.",
                            systemImage: "bell.slash"
                        )
                        .font(SeFont.support)
                        .foregroundStyle(SeTheme.secondaryInk)
                        .accessibilityIdentifier("settings.notificationsDenied")
                    }
                } header: {
                    Text("Feedback")
                } footer: {
                    Text(
                        "Background alerts notify when an agent finishes a turn, asks for approval, or a terminal exits while the app is in the background. Haptics confirm actions and outcomes."
                    )
                }

                Section {
                    Picker("Default text size", selection: $terminalTextScale) {
                        ForEach(TerminalTextScale.presets, id: \.self) { scale in
                            Text(scale, format: .percent.precision(.fractionLength(0)))
                                .tag(scale)
                        }
                    }
                    .accessibilityIdentifier("settings.terminalTextScale")
                } header: {
                    Text("Terminal")
                } footer: {
                    Text("Used when opening a terminal. You can adjust the text size again in the terminal toolbar.")
                }

                Section {
                    LabeledContent("Version", value: ShellPairingAppInfo.version())
                    LabeledContent("Build", value: ShellPairingAppInfo.build())
                    Label {
                        Text(
                            "Git and Files are read-only. View changes and browse files without modifying your desktop."
                        )
                        .fixedSize(horizontal: false, vertical: true)
                    } icon: {
                        Image(systemName: "lock")
                    }
                    .font(SeFont.support)
                    .foregroundStyle(SeTheme.secondaryInk)
                } header: {
                    Text("About")
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .background(SeTheme.canvas)
            .foregroundStyle(SeTheme.ink)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear { terminalTextScale = TerminalTextScale.current }
            .onChange(of: terminalTextScale) { _, scale in
                TerminalTextScale.current = scale
            }
            .onChange(of: settings.notificationsEnabled) { _, enabled in
                guard enabled else { return }
                notificationDenied = false
                Task { @MainActor in
                    let granted = await SeNotifications.requestAuthorization()
                    if !granted {
                        settings.notificationsEnabled = false
                        notificationDenied = true
                    }
                }
            }
        }
        .tint(SeTheme.accent)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
    }
}
