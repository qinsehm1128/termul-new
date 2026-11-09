import SwiftUI

/// Preferences affect this phone. Host identity comes from the connection store;
/// a selected host is not advertised as online before its handshake succeeds.
@MainActor
struct SettingsScreen: View {
    @Bindable var settings: AppSettings
    var store: ConnectionStore? = nil
    var onBack: () -> Void = {}

    @Environment(\.sePalette) private var palette
    @State private var terminalTextScale = TerminalTextScale.current
    @State private var notificationDenied = false
    @State private var hostToForget: RemoteLink?
    @ScaledMetric(relativeTo: .subheadline) private var secondarySize = SeNocturneType.secondarySize
    @ScaledMetric(relativeTo: .title3) private var titleSize = SeNocturneType.sheetTitleSize
    @ScaledMetric(relativeTo: .body) private var rowSize = SeNocturneType.rowTitleSize
    @ScaledMetric(relativeTo: .caption) private var labelSize = SeNocturneType.labelSize
    @ScaledMetric(relativeTo: .caption) private var monoSize = SeNocturneType.monoSize

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.md) {
                header
                sectionLabel("Host")
                group {
                    if let host = store?.activeLink {
                        HStack(spacing: SeSpace.md) {
                            Image(systemName: "desktopcomputer")
                                .foregroundStyle(palette.inkSecondary)
                            VStack(alignment: .leading, spacing: SeSpace.xxs) {
                                Text(verbatim: host.title).font(SeNocturneType.font(rowSize, weight: .semibold))
                                Text(verbatim: host.originHost)
                                    .font(SeNocturneType.font(monoSize, mono: true))
                                    .foregroundStyle(palette.inkSecondary)
                            }
                            Spacer(minLength: SeSpace.sm)
                            Text("Current host")
                                .font(SeNocturneType.font(labelSize))
                                .foregroundStyle(palette.inkSecondary)
                        }
                        .padding(SeSpace.md)
                        Divider().overlay(palette.stroke)
                        Button(role: .destructive) { hostToForget = host } label: {
                            Text("Remove this host")
                                .font(SeNocturneType.font(secondarySize))
                                .foregroundStyle(palette.danger)
                                .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
                                .padding(.horizontal, SeSpace.md)
                        }
                        .buttonStyle(.plain)
                    } else {
                        Label("No active host", systemImage: "desktopcomputer")
                            .font(SeNocturneType.font(secondarySize))
                            .foregroundStyle(palette.inkSecondary)
                            .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
                            .padding(SeSpace.md)
                    }
                }

                sectionLabel("Appearance")
                group {
                    Picker("Language", selection: $settings.language) {
                        Text("System").tag(AppLanguage.system)
                        Text("English").tag(AppLanguage.english)
                        Text("Simplified Chinese").tag(AppLanguage.simplifiedChinese)
                    }
                    .accessibilityIdentifier("settings.language")
                    .settingsControl()
                    Divider().overlay(palette.stroke)
                    Picker("Appearance", selection: $settings.appearance) {
                        Text("System").tag(AppAppearance.system)
                        Text("Dark").tag(AppAppearance.dark)
                        Text("Light").tag(AppAppearance.light)
                    }
                    .accessibilityIdentifier("settings.appearance")
                    .settingsControl()
                }
                note("Language and appearance apply to this app.")

                sectionLabel("Feedback")
                group {
                    Toggle(isOn: $settings.hapticsEnabled) {
                        Label("Haptic feedback", systemImage: "hand.tap")
                    }
                    .accessibilityIdentifier("settings.haptics")
                    .settingsControl()
                    Divider().overlay(palette.stroke)
                    Toggle(isOn: $settings.notificationsEnabled) {
                        Label("Background alerts", systemImage: "bell")
                    }
                    .accessibilityIdentifier("settings.notifications")
                    .settingsControl()
                }
                if notificationDenied {
                    Label("Notifications are not allowed. Enable them in iOS Settings to receive background alerts.", systemImage: "bell.slash")
                        .font(SeNocturneType.font(secondarySize))
                        .foregroundStyle(palette.warning)
                        .accessibilityIdentifier("settings.notificationsDenied")
                }
                note("Background alerts notify when an agent finishes a turn, asks for approval, or a terminal exits while the app is in the background. Haptics confirm actions and outcomes.")

                sectionLabel("Terminal")
                group {
                    Picker("Default text size", selection: $terminalTextScale) {
                        ForEach(TerminalTextScale.presets, id: \.self) { scale in
                            Text(scale, format: .percent.precision(.fractionLength(0))).tag(scale)
                        }
                    }
                    .settingsControl()
                    .accessibilityIdentifier("settings.terminalTextScale")
                }
                note("Used when opening a terminal. You can adjust the text size again in the terminal toolbar.")

                sectionLabel("Privacy")
                group {
                    Label {
                        Text("Pairing secrets are stored in the Keychain on this phone.")
                            .fixedSize(horizontal: false, vertical: true)
                    } icon: { Image(systemName: "lock") }
                    .font(SeNocturneType.font(secondarySize))
                    .padding(SeSpace.md)
                    Divider().overlay(palette.stroke)
                    Text("Git and Files are read-only. View changes and browse files without modifying your desktop.")
                        .font(SeNocturneType.font(secondarySize))
                        .foregroundStyle(palette.inkSecondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(SeSpace.md)
                }
                HStack {
                    LabeledContent("Version", value: ShellPairingAppInfo.version())
                    LabeledContent("Build", value: ShellPairingAppInfo.build())
                }
                .font(SeNocturneType.font(monoSize, mono: true))
                .foregroundStyle(palette.inkSecondary)
            }
            .padding(SeSpace.lg)
            .frame(maxWidth: SeLayout.readableColumnMaxWidth)
            .frame(maxWidth: .infinity)
        }
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .tint(palette.accent)
        .onAppear { terminalTextScale = TerminalTextScale.current }
        .onChange(of: terminalTextScale) { _, scale in TerminalTextScale.current = scale }
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
        .seHaptic(.selection, trigger: settings.language)
        .seHaptic(.selection, trigger: settings.appearance)
        .alert("Forget host?", isPresented: Binding(
            get: { hostToForget != nil },
            set: { if !$0 { hostToForget = nil } }
        )) {
            Button("Cancel", role: .cancel) { hostToForget = nil }
            Button("Forget", role: .destructive) {
                if let host = hostToForget {
                    ShellPairingActivity().forget(host)
                    store?.forget(host)
                }
                hostToForget = nil
            }
        } message: {
            Text("This removes the host and its saved access secret from this phone. Pair again to reconnect.")
        }
    }

    private var header: some View {
        HStack(spacing: SeSpace.md) {
            Button(action: onBack) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 14, weight: .semibold))
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            Text("Settings")
                .font(SeNocturneType.font(titleSize, weight: .semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer()
        }
    }

    private func sectionLabel(_ title: LocalizedStringKey) -> some View {
        Text(title)
            .font(SeNocturneType.font(labelSize))
            .foregroundStyle(palette.inkSecondary)
            .padding(.top, SeSpace.sm)
            .accessibilityAddTraits(.isHeader)
    }

    private func note(_ text: LocalizedStringKey) -> some View {
        Text(text)
            .font(SeNocturneType.font(secondarySize))
            .foregroundStyle(palette.inkSecondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func group<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0, content: content)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
    }
}

private struct SettingsControlModifier: ViewModifier {
    @ScaledMetric(relativeTo: .subheadline) private var textSize = SeNocturneType.secondarySize

    func body(content: Content) -> some View {
        content
            .font(SeNocturneType.font(textSize))
            .frame(minHeight: SeSpace.hitTarget)
            .padding(.horizontal, SeSpace.md)
            .padding(.vertical, SeSpace.xs)
    }
}

private extension View {
    func settingsControl() -> some View { modifier(SettingsControlModifier()) }
}
