import SwiftUI

/// Preferences affect this phone. Host identity comes from the connection store;
/// a selected host is not advertised as online before its handshake succeeds.
@MainActor
struct SettingsScreen: View {
    @Bindable var settings: AppSettings
    var store: ConnectionStore? = nil
    var onBack: () -> Void = {}

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.remoteDeviceLink) private var deviceLink
    private var permissionLink: RemoteLink? { deviceLink ?? store?.activeLink ?? store?.savedLinks.first }
    @State private var terminalTextScale = TerminalTextScale.current
    @State private var notificationDenied = false
    @State private var hostToForget: RemoteLink?

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeNavBar(
                    title: String(localized: "Settings"),
                    subtitle: hostSubtitle,
                    onBack: onBack
                )

                SeGroupLabel(title: String(localized: "Hosts"), count: nil)
                SeGroupCard {
                    if let host = store?.activeLink ?? store?.savedLinks.first {
                        SeRowItem(icon: .hosts, title: host.title, detail: host.originHost, value: nil) {
                            EmptyView()
                        }
                    } else {
                        SeRowItem(icon: .hosts, title: String(localized: "No hosts yet"), detail: String(localized: "Pair a new host and it appears here")) {
                            EmptyView()
                        }
                    }
                    SeRowItem(
                        icon: .clock,
                        title: String(localized: "Notifications"),
                        detail: String(localized: "Notifies only when you need to act"),
                        value: settings.notificationsEnabled ? String(localized: "On") : String(localized: "Off")
                    ) {
                        SeToggle(isOn: $settings.notificationsEnabled, label: String(localized: "Notifications"))
                    }
                }
                .padding(.horizontal, 18)

                if notificationDenied {
                    Text("Notifications are not allowed. Enable them in iOS Settings to receive background alerts.")
                        .font(SeNocturneType.font(10))
                        .foregroundStyle(palette.warning)
                        .padding(.horizontal, 18)
                        .padding(.top, 8)
                }

                if let link = permissionLink {
                    SeGroupLabel(
                        title: String(localized: "Device access"),
                        count: nil,
                        info: String(localized: "Permissions are managed on the host.")
                    )
                    SeGroupCard {
                        SeRowItem(icon: .lock, title: String(localized: "Device name"),
                                  detail: link.deviceName ?? String(localized: "This device")) {
                            EmptyView()
                        }
                        if let scopes = link.scopes {
                            if scopes.isEmpty {
                                SeRowItem(title: String(localized: "No permissions granted")) { EmptyView() }
                            } else {
                                ForEach(Array(Set(scopes)).sorted(), id: \.self) { scope in
                                    SeRowItem(title: DeviceScope.label(for: scope)) { EmptyView() }
                                }
                            }
                        } else {
                            SeRowItem(title: String(localized: "Legacy pairing"),
                                      detail: String(localized: "No scope restrictions")) { EmptyView() }
                        }
                    }
                    .padding(.horizontal, 18)
                }

                SeGroupLabel(title: String(localized: "Appearance"), count: nil)
                SeGroupCard {
                    SeRowItem(title: String(localized: "Theme")) {
                        SeThemePick(mode: Binding(
                            get: { mode },
                            set: { settings.appearance = $0.appearance }
                        ))
                    }
                    SeRowItem(title: String(localized: "Language")) {
                        Menu {
                            ForEach(AppLanguage.allCases) { language in
                                Button(languageTitle(language)) { settings.language = language }
                            }
                        } label: {
                            Text(languageTitle(settings.language))
                                .font(SeNocturneType.font(9, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                                .frame(minWidth: 44, minHeight: 44, alignment: .trailing)
                        }
                        .accessibilityIdentifier("settings.language")
                    }
                }
                .padding(.horizontal, 18)

                SeGroupLabel(title: String(localized: "Interaction"), count: nil)
                SeGroupCard {
                    SeRowItem(title: String(localized: "Haptics")) {
                        SeToggle(isOn: $settings.hapticsEnabled, label: String(localized: "Haptics"))
                    }
                    SeRowItem(title: String(localized: "Terminal font size")) {
                        HStack(spacing: 4) {
                            scaleButton("A−", direction: -1)
                            scaleButton("A+", direction: 1)
                        }
                    }
                }
                .padding(.horizontal, 18)

                SeGroupLabel(
                    title: String(localized: "Privacy"),
                    count: nil,
                    info: String(localized: "Git and files are read-only")
                )
                SeGroupCard {
                    SeRowItem(icon: .lock, title: String(localized: "Credentials"), detail: String(localized: "Stored in this device keychain, never uploaded"), value: "Face ID") {
                        EmptyView()
                    }
                    if let host = store?.activeLink ?? store?.savedLinks.first {
                        SeRowItem(title: String(localized: "Unpair this host"), destructive: true, action: {
                            hostToForget = host
                        }) {
                            EmptyView()
                        }
                    }
                }
                .padding(.horizontal, 18)

                Text(versionLine)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.top, 16)
                    .padding(.bottom, 28)
            }
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
        .alert("Unpair host?", isPresented: Binding(
            get: { hostToForget != nil },
            set: { if !$0 { hostToForget = nil } }
        )) {
            Button("Cancel", role: .cancel) { hostToForget = nil }
            Button("Unpair", role: .destructive) {
                if let host = hostToForget {
                    ShellPairingActivity().forget(host)
                    store?.forget(host)
                }
                hostToForget = nil
            }
        } message: {
            Text("This deletes the host and saved access key from this device. Re-pair to connect again.")
        }
    }

    private var hostSubtitle: String {
        guard let host = store?.activeLink ?? store?.savedLinks.first else {
            return String(localized: "This device · Unpaired")
        }
        return String(localized: "\(host.title) · Saved")
    }

    private var versionLine: String {
        "SeRemote \(ShellPairingAppInfo.version()) · build \(ShellPairingAppInfo.build())"
    }

    private func languageTitle(_ language: AppLanguage) -> String {
        switch language {
        case .system: String(localized: "Follow system")
        case .english: "English"
        case .simplifiedChinese: String(localized: "Simplified Chinese")
        }
    }

    private func scaleButton(_ title: String, direction: Int) -> some View {
        Button {
            terminalTextScale = TerminalTextScale.nudge(terminalTextScale, by: direction)
        } label: {
            Text(title)
                .font(SeNocturneType.font(9, mono: true))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 30, height: 30)
                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .strokeBorder(mode.insetStroke, lineWidth: 1)
                }
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(direction > 0 ? String(localized: "Increase terminal font size") : String(localized: "Decrease terminal font size"))
    }
}
