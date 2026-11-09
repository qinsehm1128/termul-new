import SwiftUI

// MARK: - Model

enum LiveSessionStatus: String, Sendable {
    case needsYou
    case running
    case ready
    case unknown
    case idle
}

struct LiveSession: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let agent: String
    let host: String
    let status: LiveSessionStatus
    let timing: String
    var detailStatus: String? = nil
}

struct LiveHost: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let latency: String
    let isOnline: Bool
}

struct LiveNeedsSummary: Equatable, Sendable {
    var permissions: Int = 0
    var failures: Int = 0
    var questions: Int = 0

    var total: Int { permissions + failures + questions }
}

enum LiveFilter: String, CaseIterable, Identifiable {
    case all
    case running
    case needsYou

    var id: String { rawValue }

    var title: LocalizedStringKey {
        switch self {
        case .all: "All"
        case .running: "Running"
        case .needsYou: "Needs you"
        }
    }
}

// MARK: - Screen

/// The redesigned home: live sessions across hosts, a compact needs strip,
/// and two destinations plus the command palette in the bottom bar.
struct LiveView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var hosts: [LiveHost]
    var sessions: [LiveSession]
    var needs: LiveNeedsSummary
    var showsBottomBar = true

    var onSelectHost: (LiveHost) -> Void = { _ in }
    var onOpenSession: (LiveSession) -> Void = { _ in }
    var onOpenNeeds: () -> Void = {}
    var onOpenWorkspace: () -> Void = {}
    var onOpenCommand: () -> Void = {}
    var onOpenDrawer: () -> Void = {}
    var onToggleTheme: () -> Void = {}

    @State private var filter: LiveFilter = .all
    @Namespace private var filterNamespace

    private var currentHost: LiveHost? {
        hosts.first(where: \.isOnline) ?? hosts.first
    }

    private var visibleSessions: [LiveSession] {
        switch filter {
        case .all: sessions
        case .running: sessions.filter { $0.status == .running }
        case .needsYou: sessions.filter { $0.status == .needsYou }
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.top, SeSpace.xs)
                title
                    .padding(.top, SeSpace.lg)
                if needs.total > 0 {
                    needsStrip
                        .padding(.top, SeSpace.md)
                }
                filterBar
                    .padding(.top, SeSpace.md)
                sectionLabel
                    .padding(.top, SeSpace.xl)
                sessionList
                if sessions.isEmpty {
                    emptyState
                        .padding(.top, SeSpace.xl)
                }
            }
            .padding(.horizontal, SeSpace.lg)
            .padding(.bottom, SeSpace.xxl)
        }
        .scrollIndicators(.hidden)
        .background(palette.canvas.ignoresSafeArea())
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if showsBottomBar { tabBar }
        }
    }

    private var header: some View {
        HStack(spacing: SeSpace.xs) {
            Button {
                if let currentHost { onSelectHost(currentHost) }
            } label: {
                HStack(spacing: 6) {
                    Text(currentHost?.name ?? String(localized: "No host"))
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(palette.inkTertiary)
                }
                .padding(.horizontal, 10)
                .frame(height: 44)
                .background(palette.surface, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
            }
            .buttonStyle(.plain)
            .frame(minHeight: 44)

            Text(hosts.filter(\.isOnline).count == 1 ? "1 online" : "\(hosts.filter(\.isOnline).count) online")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)

            Spacer(minLength: SeSpace.xs)

            iconButton(
                systemImage: mode == .night ? "sun.max" : "moon",
                label: String(localized: "Switch theme"),
                action: onToggleTheme
            )
            iconButton(
                systemImage: "sidebar.left",
                label: String(localized: "All destinations"),
                action: onOpenDrawer
            )
        }
    }

    private var title: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Live")
                .font(SeNocturneType.screenTitle)
                .foregroundStyle(palette.ink)
            Text("\(sessions.count) sessions · \(needs.total) need you")
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkTertiary)
        }
    }

    private var needsStrip: some View {
        Button(action: onOpenNeeds) {
            HStack(spacing: 8) {
                if needs.permissions > 0 {
                    statusDot(palette.warning)
                    Text("\(needs.permissions) permission")
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                }
                if needs.questions > 0 {
                    statusDot(palette.warning)
                    Text("\(needs.questions) question")
                        .font(SeNocturneType.secondary)
                        .foregroundStyle(palette.ink)
                }
                if (needs.permissions > 0 || needs.questions > 0) && needs.failures > 0 {
                    Rectangle()
                        .fill(palette.strokeStrong)
                        .frame(width: 1, height: 12)
                }
                if needs.failures > 0 {
                    statusDot(palette.danger)
                    Text("\(needs.failures) failure")
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                }
                Spacer(minLength: SeSpace.xs)
                Text("Needs you")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.warning)
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(palette.warning)
            }
            .padding(.horizontal, 13)
            .frame(height: 46)
            .background(palette.warningSubtle, in: RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.strip)
                    .strokeBorder(palette.warning.opacity(0.26), lineWidth: 1)
            }
        }
        .buttonStyle(.plain)
    }

    private var filterBar: some View {
        HStack(spacing: 3) {
            ForEach(LiveFilter.allCases) { item in
                let isOn = filter == item
                Button {
                    withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion)) {
                        filter = item
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text(item.title)
                            .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        Text(verbatim: "\(count(for: item))")
                            .font(SeNocturneType.monoSmall)
                            .opacity(0.62)
                    }
                    .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                    .padding(.horizontal, 11)
                    .frame(height: 44)
                    .background {
                        if isOn {
                            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                                .fill(palette.ink)
                                .matchedGeometryEffect(id: "filter", in: filterNamespace)
                        }
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var sectionLabel: some View {
        HStack {
            Text("Sessions")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
            Spacer()
            Text(verbatim: String(format: "%02d", visibleSessions.count))
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
        }
    }

    private var sessionList: some View {
        VStack(spacing: 6) {
            ForEach(visibleSessions) { session in
                Button {
                    onOpenSession(session)
                } label: {
                    sessionRow(session)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.top, 8)
    }

    private func sessionRow(_ session: LiveSession) -> some View {
        HStack(spacing: 11) {
            RoundedRectangle(cornerRadius: 2)
                .fill(statusColor(session.status))
                .frame(width: 3, height: 26)

            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: session.title)
                    .font(SeNocturneType.rowTitle)
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: [session.agent, session.detailStatus ?? statusTitle(session.status), session.host].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(SeNocturneType.font(SeNocturneType.labelSize))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            if !session.timing.isEmpty {
                Text(verbatim: session.timing)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                .strokeBorder(
                    session.status == .needsYou ? palette.warning.opacity(0.34) : palette.stroke,
                    lineWidth: 1
                )
        }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Nothing is running")
                .font(SeNocturneType.rowTitle)
                .foregroundStyle(palette.inkSecondary)
            Text("Start a session from a host, or run a quick command.")
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkTertiary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var tabBar: some View {
        HStack(spacing: 6) {
            tabItem(title: "Live", systemImage: "dot.radiowaves.left.and.right", isOn: true, action: {})
            tabItem(title: "Workspace", systemImage: "folder", isOn: false, action: onOpenWorkspace)
            Button(action: onOpenCommand) {
                Image(systemName: "command")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(palette.accent)
                    .frame(width: 52, height: 44)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, SeSpace.md)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(palette.surface)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
        }
    }

    private func tabItem(
        title: LocalizedStringKey,
        systemImage: String,
        isOn: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: systemImage)
                    .font(.system(size: 17, weight: .regular))
                Text(title)
                    .font(SeNocturneType.font(SeNocturneType.monoSmallSize))
            }
            .foregroundStyle(isOn ? palette.ink : palette.inkTertiary)
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(
                isOn ? palette.card.opacity(0.6) : .clear,
                in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
            )
        }
        .buttonStyle(.plain)
    }

    private func iconButton(
        systemImage: String,
        label: String,
        action: @escaping () -> Void
    ) -> some View {
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

    private func statusDot(_ color: Color) -> some View {
        Circle()
            .fill(color)
            .frame(width: 6, height: 6)
    }

    private func count(for filter: LiveFilter) -> Int {
        switch filter {
        case .all: sessions.count
        case .running: sessions.filter { $0.status == .running }.count
        case .needsYou: sessions.filter { $0.status == .needsYou }.count
        }
    }

    private func statusColor(_ status: LiveSessionStatus) -> Color {
        switch status {
        case .needsYou: palette.warning
        case .running: palette.live
        case .idle, .ready, .unknown: palette.inkTertiary
        }
    }

    private func statusTitle(_ status: LiveSessionStatus) -> String {
        switch status {
        case .needsYou: String(localized: "Waiting for you")
        case .running: String(localized: "Running")
        case .idle: String(localized: "Idle")
        case .ready: String(localized: "Ready")
        case .unknown: String(localized: "Unknown")
        }
    }
}

// MARK: - Previews

#Preview("Night") {
    LiveView(
        hosts: [
            LiveHost(id: "mbp", name: "MacBook Pro", latency: "12 ms", isOnline: true),
            LiveHost(id: "mini", name: "Mac mini", latency: "—", isOnline: false)
        ],
        sessions: [
            LiveSession(id: "s1", title: "优化远程连接恢复", agent: "Claude Code", host: "MacBook Pro", status: .needsYou, timing: "2m"),
            LiveSession(id: "s2", title: "检查 ACP 消息流", agent: "Terminal", host: "MacBook Pro", status: .running, timing: "15m"),
            LiveSession(id: "s3", title: "发布前检查", agent: "Files", host: "Mac mini", status: .idle, timing: "1d")
        ],
        needs: LiveNeedsSummary(permissions: 1, failures: 1)
    )
    .seThemeMode(.night)
}

#Preview("Day") {
    LiveView(
        hosts: [
            LiveHost(id: "mbp", name: "MacBook Pro", latency: "12 ms", isOnline: true)
        ],
        sessions: [
            LiveSession(id: "s1", title: "优化远程连接恢复", agent: "Claude Code", host: "MacBook Pro", status: .needsYou, timing: "2m"),
            LiveSession(id: "s2", title: "检查 ACP 消息流", agent: "Terminal", host: "MacBook Pro", status: .running, timing: "15m")
        ],
        needs: LiveNeedsSummary(permissions: 1, failures: 0)
    )
    .seThemeMode(.day)
}
