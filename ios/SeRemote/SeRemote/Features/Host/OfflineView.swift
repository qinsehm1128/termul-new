import SwiftUI

// MARK: - Model

struct OfflineSession: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let meta: String
    let badge: String
}

// MARK: - Screen

/// Read-only cache is offered only when an actual transcript exists.
struct OfflineView: View {
    @Environment(\.sePalette) private var palette

    var host: String
    var lastSeen: String
    var cached: [OfflineSession]

    var onBack: () -> Void = {}
    var onReconnect: () -> Void = {}
    var onWake: (() -> Void)? = nil
    var onOpenSettings: (() -> Void)? = nil
    var onOpenCached: (OfflineSession) -> Void = { _ in }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.top, SeSpace.xs)
                title
                    .padding(.top, SeSpace.lg)
                connectionCard
                    .padding(.top, SeSpace.lg)
                if !cached.isEmpty {
                Text("Cached sessions")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, SeSpace.xl)
                    .padding(.bottom, 8)
                cachedList
                Text("Cached transcripts are read only.")
                    .font(SeNocturneType.font(9.5))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 14)
                }
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
                Text(verbatim: host)
                    .font(SeNocturneType.font(SeNocturneType.sectionTitleSize, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Text(lastSeen.isEmpty ? String(localized: "Offline") : String(localized: "Offline · last seen \(lastSeen)"))
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
            }
            Spacer(minLength: 8)
            if let onOpenSettings {
                iconButton(systemImage: "slider.horizontal.3", label: String(localized: "Settings"), action: onOpenSettings)
            }

        }
    }

    private var title: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(palette.inkTertiary)
                .frame(width: 6, height: 6)
            Text(verbatim: host)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
        }
    }

    private var connectionCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Connection lost")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
            Text(cached.isEmpty ? String(localized: "Connection lost. Reconnect to continue.") : String(localized: "Connection lost. Cached transcripts are readable, but new instructions cannot be sent."))
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 7)

            Button(action: onReconnect) {
                Text("Reconnect")
                    .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .semibold))
                    .foregroundStyle(palette.surface)
                    .frame(maxWidth: .infinity)
                    .frame(height: 44)
                    .background(palette.ink, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            }
            .buttonStyle(.plain)
            .padding(.top, 14)

            if let onWake {
            Button(action: onWake) {
                Text("Try Wake on LAN")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 10)
            }
            .buttonStyle(.plain)
            .frame(minHeight: 44)
            }
        }
        .padding(13)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                .strokeBorder(palette.strokeStrong, lineWidth: 1)
        }
    }

    private var cachedList: some View {
        VStack(spacing: 6) {
            ForEach(cached) { session in
                Button {
                    onOpenCached(session)
                } label: {
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 2)
                            .fill(palette.inkTertiary)
                            .frame(width: 3, height: 24)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: session.title)
                                .font(SeNocturneType.rowTitle)
                                .foregroundStyle(palette.ink)
                                .lineLimit(1)
                            Text(verbatim: session.meta)
                                .font(SeNocturneType.monoSmall)
                                .foregroundStyle(palette.inkTertiary)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        Text(verbatim: session.badge)
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(palette.inkTertiary)
                            .padding(.horizontal, 6)
                            .frame(height: 20)
                            .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 5))
                    }
                    .padding(11)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func iconButton(systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 44, height: 44)
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

#Preview("Offline · Night") {
    OfflineView(
        host: "Mac mini",
        lastSeen: "3h ago",
        cached: [
            OfflineSession(id: "o1", title: "优化远程连接恢复", meta: "Claude Code · read only · 3h ago", badge: "Cached"),
            OfflineSession(id: "o2", title: "检查 ACP 消息流", meta: "Terminal · read only · yesterday", badge: "Cached"),
            OfflineSession(id: "o3", title: "夜间冒烟测试", meta: "Schedule · not running", badge: "Paused")
        ]
    )
    .seThemeMode(.night)
}

#Preview("Offline · Day") {
    OfflineView(host: "Mac mini", lastSeen: "3h ago", cached: [])
        .seThemeMode(.day)
}
