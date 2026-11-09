import SwiftUI

/// Settings touch this phone only: the host configuration stays on the host.
struct SettingsScreen: View {
    @Environment(\.sePalette) private var palette

    @Binding var themeMode: SeThemeMode

    var hostName: String
    var hostMeta: String
    var version: String

    var onBack: () -> Void = {}
    var onOpenHost: () -> Void = {}
    var onClearPairing: () -> Void = {}

    @State private var haptics = true
    @State private var interfaceMotion = true

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.top, SeSpace.xs)
                groupLabel("Host")
                group {
                    row(
                        title: hostName,
                        subtitle: hostMeta,
                        value: String(localized: "Online"),
                        systemImage: "desktopcomputer",
                        action: onOpenHost
                    )
                    divider
                    row(
                        title: String(localized: "Notifications"),
                        subtitle: String(localized: "Only when you are needed"),
                        value: String(localized: "On"),
                        systemImage: "bell",
                        action: {}
                    )
                }
                groupLabel("Appearance")
                group {
                    themeRow
                }
                groupLabel("Interaction")
                group {
                    toggleRow(
                        title: String(localized: "Haptics"),
                        subtitle: String(localized: "One tap when approving or denying"),
                        isOn: $haptics
                    )
                    divider
                    toggleRow(
                        title: String(localized: "Interface motion"),
                        subtitle: String(localized: "Connection state and mode switches"),
                        isOn: $interfaceMotion
                    )
                }
                groupLabel("Privacy")
                group {
                    row(
                        title: String(localized: "Credentials"),
                        subtitle: String(localized: "Keychain on this phone, never uploaded"),
                        value: "Face ID",
                        systemImage: "lock",
                        action: {}
                    )
                    divider
                    Button(action: onClearPairing) {
                        Text("Remove this host")
                            .font(SeNocturneType.font(SeNocturneType.secondarySize))
                            .foregroundStyle(palette.danger)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 14)
                            .frame(height: 46)
                    }
                    .buttonStyle(.plain)
                }
                Text(verbatim: version)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 16)
                    .padding(.horizontal, 2)
            }
            .padding(.horizontal, SeSpace.lg)
            .padding(.bottom, SeSpace.xxl)
        }
        .scrollIndicators(.hidden)
        .background(palette.canvas.ignoresSafeArea())
    }

    private var header: some View {
        HStack(spacing: 10) {
            iconButton(systemImage: "chevron.left", label: String(localized: "Back"), action: onBack)
            VStack(alignment: .leading, spacing: 2) {
                Text("Settings")
                    .font(SeNocturneType.font(SeNocturneType.sectionTitleSize, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Text(verbatim: "\(hostName) · \(String(localized: "Connected"))")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
            }
            Spacer(minLength: 8)
            iconButton(systemImage: "ellipsis", label: String(localized: "More"), action: {})
        }
    }

    private func groupLabel(_ title: LocalizedStringKey) -> some View {
        Text(title)
            .font(SeNocturneType.monoSmall)
            .foregroundStyle(palette.inkTertiary)
            .padding(.top, 18)
            .padding(.bottom, 8)
            .padding(.horizontal, 2)
    }

    private func group<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 0) {
            content()
        }
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .clipShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card))
    }

    private var divider: some View {
        Rectangle()
            .fill(palette.stroke)
            .frame(height: 1)
            .padding(.leading, 46)
    }

    private var themeRow: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Theme")
                    .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                    .foregroundStyle(palette.ink)
                Text("Day or night, this phone only")
                    .font(SeNocturneType.font(SeNocturneType.labelSize))
                    .foregroundStyle(palette.inkTertiary)
            }
            Spacer(minLength: 8)
            HStack(spacing: 3) {
                ForEach(SeThemeMode.allCases) { mode in
                    let isOn = themeMode == mode
                    Button {
                        themeMode = mode
                    } label: {
                        Text(mode.title)
                            .font(SeNocturneType.font(SeNocturneType.labelSize, weight: isOn ? .medium : .regular))
                            .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                            .padding(.horizontal, 9)
                            .frame(height: 24)
                            .background(
                                isOn ? palette.ink : .clear,
                                in: RoundedRectangle(cornerRadius: 5)
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(2)
            .overlay {
                RoundedRectangle(cornerRadius: 7)
                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 54)
    }

    private func row(
        title: String,
        subtitle: String,
        value: String,
        systemImage: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.system(size: 13, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                    Text(verbatim: subtitle)
                        .font(SeNocturneType.font(SeNocturneType.labelSize))
                        .foregroundStyle(palette.inkTertiary)
                }
                Spacer(minLength: 8)
                Text(verbatim: value)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(palette.inkTertiary)
            }
            .padding(.horizontal, 14)
            .frame(height: 54)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func toggleRow(
        title: String,
        subtitle: String,
        isOn: Binding<Bool>
    ) -> some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: title)
                    .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                    .foregroundStyle(palette.ink)
                Text(verbatim: subtitle)
                    .font(SeNocturneType.font(SeNocturneType.labelSize))
                    .foregroundStyle(palette.inkTertiary)
            }
            Spacer(minLength: 8)
            Toggle("", isOn: isOn)
                .labelsHidden()
                .tint(palette.accent)
        }
        .padding(.horizontal, 14)
        .frame(height: 54)
    }

    private func iconButton(systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

#Preview("Settings · Night") {
    SettingsScreen(
        themeMode: .constant(.night),
        hostName: "MacBook Pro",
        hostMeta: "上海 · 12 ms · 2 connections",
        version: "SeRemote 27.4 · build 189"
    )
    .seThemeMode(.night)
}

#Preview("Settings · Day") {
    SettingsScreen(
        themeMode: .constant(.day),
        hostName: "MacBook Pro",
        hostMeta: "上海 · 12 ms",
        version: "SeRemote 27.4 · build 189"
    )
    .seThemeMode(.day)
}
