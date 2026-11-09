import SwiftUI

struct OfflineSession: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let meta: String
    let badge: String
}

/// 离线: cached sessions stay readable; reconnect and wake are the only
/// actions, and neither claims success before the host answers.
struct OfflineView: View {
    var host: String
    var lastSeen: String
    var cached: [OfflineSession]

    var onBack: () -> Void = {}
    var onReconnect: () -> Void = {}
    var onWake: (() -> Void)? = nil
    var onOpenSettings: (() -> Void)? = nil
    var onOpenCached: (OfflineSession) -> Void = { _ in }

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeNavBar(
                    title: String(localized: "Offline"),
                    onBack: onBack,
                    onTrailing: onOpenSettings
                )

                hostHead

                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 2) {
                        SeEyebrow(text: String(localized: "Connection lost"))
                        SeInfoTip(text: String(localized: "Cached sessions stay readable, but new instructions cannot be sent. The cache keeps the last 7 days; reconnecting resumes the original PTY."))
                        Spacer(minLength: 0)
                    }

                    SeSolidButton(title: String(localized: "Reconnect"), action: onReconnect)
                        .padding(.top, 10)

                    if let onWake {
                        Button(action: onWake) {
                            Text("Try Wake on LAN")
                                .font(SeNocturneType.font(11.5))
                                .foregroundStyle(palette.accent)
                                .frame(maxWidth: .infinity, minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(SePressable())
                        .padding(.top, 2)
                    }
                }
                .padding(14)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .strokeBorder(mode.strongHairline.opacity(0.6), lineWidth: 1)
                }
                .padding(.horizontal, 18)
                .padding(.top, 20)

                SeGroupLabel(title: String(localized: "Cached sessions"), count: String(format: "%02d", cached.count))

                if cached.isEmpty {
                    Text("This host has no cached sessions yet.")
                        .font(SeNocturneType.font(11))
                        .foregroundStyle(palette.inkSecondary)
                        .padding(.horizontal, 18)
                        .padding(.vertical, 12)
                } else {
                    VStack(spacing: 8) {
                        ForEach(cached) { item in
                            SeSessionRow(
                                title: item.title,
                                meta: item.meta,
                                time: nil,
                                status: .idle,
                                trailing: item.badge,
                                onTap: { onOpenCached(item) }
                            )
                        }
                    }
                    .padding(.horizontal, 18)
                }

                Color.clear
                    .frame(height: 28)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
    }

    private var subtitle: String {
        lastSeen.isEmpty ? String(localized: "Offline") : String(localized: "Offline · Last seen \(lastSeen)")
    }

    private var hostHead: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(host)
                .font(SeNocturneType.font(26, weight: .semibold))
                .tracking(-0.8)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            HStack(spacing: 5) {
                SeNeedDot(kind: .off, size: 5)
                Text(subtitle)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 16)
    }
}
