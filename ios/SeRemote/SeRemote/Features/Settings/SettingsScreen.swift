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
    @State private var terminalTextScale = TerminalTextScale.current
    @State private var notificationDenied = false
    @State private var hostToForget: RemoteLink?

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeNavBar(
                    title: "设置",
                    subtitle: hostSubtitle,
                    onBack: onBack
                )

                SeGroupLabel(title: "主机", count: nil)
                SeGroupCard {
                    if let host = store?.activeLink ?? store?.savedLinks.first {
                        SeRowItem(icon: .hosts, title: host.title, detail: host.originHost, value: nil) {
                            EmptyView()
                        }
                    } else {
                        SeRowItem(icon: .hosts, title: "还没有主机", detail: "配对新主机后出现在这里") {
                            EmptyView()
                        }
                    }
                    SeRowItem(
                        icon: .clock,
                        title: "通知",
                        detail: "只在需要你处理时提醒",
                        value: settings.notificationsEnabled ? "开启" : "关闭"
                    ) {
                        SeToggle(isOn: $settings.notificationsEnabled, label: "通知")
                    }
                }
                .padding(.horizontal, 18)

                if notificationDenied {
                    Text("通知未获允许。到 iOS 设置里打开，才能收到后台提醒。")
                        .font(SeNocturneType.font(10))
                        .foregroundStyle(palette.warning)
                        .padding(.horizontal, 18)
                        .padding(.top, 8)
                }

                SeGroupLabel(title: "外观", count: nil)
                SeGroupCard {
                    SeRowItem(title: "主题", detail: "日间 / 夜间，只影响本机") {
                        SeThemePick(mode: Binding(
                            get: { mode },
                            set: { settings.appearance = $0.appearance }
                        ))
                    }
                    SeRowItem(title: "语言", detail: "跟随系统或手动选择") {
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

                SeGroupLabel(title: "交互", count: nil)
                SeGroupCard {
                    SeRowItem(title: "触觉反馈", detail: "批准与拒绝时轻点一次") {
                        SeToggle(isOn: $settings.hapticsEnabled, label: "触觉反馈")
                    }
                    SeRowItem(title: "终端字号", detail: "默认在打开终端时使用") {
                        HStack(spacing: 4) {
                            scaleButton("A−", direction: -1)
                            scaleButton("A+", direction: 1)
                        }
                    }
                }
                .padding(.horizontal, 18)

                SeGroupLabel(title: "隐私", count: nil)
                SeGroupCard {
                    SeRowItem(icon: .lock, title: "凭据", detail: "保存在本机钥匙串，不上传", value: "Face ID") {
                        EmptyView()
                    }
                    if let host = store?.activeLink ?? store?.savedLinks.first {
                        SeRowItem(title: "解除这台主机", destructive: true, action: {
                            hostToForget = host
                        }) {
                            EmptyView()
                        }
                    } else {
                        SeRowItem(title: "Git 与文件只读", detail: "查看不会修改你的桌面") {
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
        .alert("解除主机？", isPresented: Binding(
            get: { hostToForget != nil },
            set: { if !$0 { hostToForget = nil } }
        )) {
            Button("取消", role: .cancel) { hostToForget = nil }
            Button("解除", role: .destructive) {
                if let host = hostToForget {
                    ShellPairingActivity().forget(host)
                    store?.forget(host)
                }
                hostToForget = nil
            }
        } message: {
            Text("这会从本机删除主机和已保存的访问密钥。重新配对后才能连接。")
        }
    }

    private var hostSubtitle: String {
        guard let host = store?.activeLink ?? store?.savedLinks.first else { return "本机 · 未配对" }
        return "\(host.title) · 已保存"
    }

    private var versionLine: String {
        "SeRemote \(ShellPairingAppInfo.version()) · build \(ShellPairingAppInfo.build())"
    }

    private func languageTitle(_ language: AppLanguage) -> String {
        switch language {
        case .system: "跟随系统"
        case .english: "English"
        case .simplifiedChinese: "简体中文"
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
        .accessibilityLabel(direction > 0 ? "增大终端字号" : "减小终端字号")
    }
}
