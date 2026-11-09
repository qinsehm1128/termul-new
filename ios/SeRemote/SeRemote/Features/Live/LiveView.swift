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

    var title: String {
        switch self {
        case .all: "全部"
        case .running: "运行中"
        case .needsYou: "需要你"
        }
    }
}

// MARK: - Screen

/// 现场: host switch, one live session list, and the 46 pt needs strip.
/// Everything here is projected from the host stores; nothing is invented.
struct LiveView: View {
    let hostName: String
    var hostMeta: String?
    let sessions: [LiveSession]
    let needs: [NeedsItem]
    let summary: LiveNeedsSummary
    let themeIcon: SeIcon
    var onSelectHost: () -> Void = {}
    var onOpenSession: (LiveSession) -> Void = { _ in }
    var onOpenNeeds: () -> Void = {}
    var onToggleTheme: () -> Void = {}
    var onOpenRail: () -> Void = {}

    @Environment(\.sePalette) private var palette
    @State private var filter: LiveFilter = .all
    @State private var hostFilter: String?

    private var hostNames: [String] {
        Array(Set(sessions.map(\.host))).sorted()
    }

    private var filteredSessions: [LiveSession] {
        sessions.filter { session in
            let matchesHost = hostFilter.map { session.host == $0 } ?? true
            let matchesFilter: Bool
            switch filter {
            case .all: matchesFilter = true
            case .running: matchesFilter = session.status == .running
            case .needsYou: matchesFilter = session.status == .needsYou
            }
            return matchesHost && matchesFilter
        }
    }

    private func count(for filter: LiveFilter) -> Int {
        sessions.filter { item in
            switch filter {
            case .all: true
            case .running: item.status == .running
            case .needsYou: item.status == .needsYou
            }
        }.count
    }

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeHostSwitch(
                    title: hostName,
                    meta: hostMeta,
                    onHost: onSelectHost,
                    onTheme: onToggleTheme,
                    themeIcon: themeIcon,
                    onRail: onOpenRail
                )

                VStack(alignment: .leading, spacing: 6) {
                    SeDisplayText(text: "现场")
                    lede
                }
                .padding(.horizontal, 18)
                .padding(.top, 10)

                SeNeedStrip(
                    permissions: summary.permissions,
                    failures: summary.failures,
                    onTap: onOpenNeeds
                )

                SeSegmented(
                    segments: [
                        SeSegment("all", "全部", count: count(for: .all)),
                        SeSegment("running", "运行中", count: count(for: .running)),
                        SeSegment("needsYou", "需要你", count: count(for: .needsYou))
                    ],
                    selection: Binding(get: { filter.rawValue }, set: { filter = LiveFilter(rawValue: $0) ?? .all }),
                    identifier: "live.filter"
                )
                .padding(.horizontal, 18)
                .padding(.top, 14)

                if hostNames.count > 1 {
                    SeChips(
                        items: hostNames.map { name in
                            SeChipItem(id: name, title: name,
                                       dot: sessions.contains { $0.host == name && $0.status == .running } ? .live : .off,
                                       count: sessions.filter { $0.host == name }.count)
                        },
                        selection: $hostFilter
                    )
                    .padding(.top, 10)
                }

                SeGroupLabel(title: "会话", count: String(format: "%02d", filteredSessions.count))

                if filteredSessions.isEmpty {
                    emptyState
                } else {
                    VStack(spacing: 8) {
                        ForEach(filteredSessions) { item in
                            SeSessionRow(
                                title: item.title,
                                meta: meta(for: item),
                                time: item.timing.isEmpty ? nil : item.timing,
                                status: rowStatus(item.status),
                                alert: item.status == .needsYou,
                                onTap: { onOpenSession(item) }
                            )
                        }
                    }
                    .padding(.horizontal, 18)
                }

                Text(needs.isEmpty
                     ? "没有待你处理的事项。"
                     : "处理后这一行会消失。没有待办时，这条带子直接隐藏。")
                    .font(SeNocturneType.font(10))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.top, 16)
                    .padding(.bottom, 24)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
    }

    private var lede: some View {
        HStack(spacing: 4) {
            Text("\(sessions.count) 个").fontWeight(.semibold).foregroundStyle(palette.ink)
            Text("会话").foregroundStyle(palette.inkSecondary)
            Text("·").foregroundStyle(palette.inkTertiary)
            Text("\(summary.total) 个").fontWeight(.semibold).foregroundStyle(palette.ink)
            Text("需要你").foregroundStyle(palette.inkSecondary)
        }
        .font(SeNocturneType.font(12))
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("暂时没有会话")
                .font(SeNocturneType.font(13, weight: .semibold))
                .foregroundStyle(palette.ink)
            Text("从工作区新开一个会话，或回到主机的工作台。")
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .padding(.horizontal, 18)
    }

    private func rowStatus(_ status: LiveSessionStatus) -> SeSessionRow.Status {
        switch status {
        case .needsYou: .wait
        case .running: .live
        case .ready, .unknown, .idle: .idle
        }
    }

    private func meta(for item: LiveSession) -> String {
        LivePresentation.rowMeta(for: item)
    }
}
