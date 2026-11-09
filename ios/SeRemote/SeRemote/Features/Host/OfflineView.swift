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
                    title: host,
                    subtitle: subtitle,
                    onBack: onBack,
                    onTrailing: onOpenSettings
                )

                hostHead

                VStack(alignment: .leading, spacing: 0) {
                    SeEyebrow(text: "连接已断开")
                    Text("这台主机现在不在线。已缓存的会话仍然可以读，但不能发送新的指令。")
                        .font(SeNocturneType.font(12))
                        .foregroundStyle(palette.inkSecondary)
                        .lineSpacing(3)
                        .padding(.top, 7)

                    SeSolidButton(title: "重新连接", action: onReconnect)
                        .padding(.top, 14)

                    if let onWake {
                        Button(action: onWake) {
                            Text("尝试局域网唤醒")
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

                SeGroupLabel(title: "缓存会话", count: String(format: "%02d", cached.count))

                if cached.isEmpty {
                    Text("这台主机还没有缓存下来的会话。")
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

                Text("缓存只保留最近 7 天。重新连接后，会话会续上原来的 PTY。")
                    .font(SeNocturneType.font(10))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.top, 14)
                    .padding(.bottom, 28)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
    }

    private var subtitle: String {
        lastSeen.isEmpty ? "离线" : "离线 · 上次在线 \(lastSeen)"
    }

    private var hostHead: some View {
        VStack(alignment: .leading, spacing: 4) {
            SeEyebrow(text: "主机")
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
