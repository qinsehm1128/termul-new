import SwiftUI

// MARK: - Model

enum NeedsItemKind: String, CaseIterable, Sendable {
    case permission
    case failure
    case done
    case running

    var title: String {
        switch self {
        case .permission: String(localized: "Permission")
        case .failure: String(localized: "Failure")
        case .done: String(localized: "Done")
        case .running: String(localized: "Running")
        }
    }
}

struct NeedsItem: Identifiable, Equatable, Sendable {
    let id: String
    let kind: NeedsItemKind
    let title: String
    let meta: String
    let actionTitle: String
}

// MARK: - Screen

/// Contextual queue opened from the live strip. It is deliberately not a
/// destination: it appears over the live screen, and a handled row disappears.
struct NeedsSheetView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss

    var items: [NeedsItem]
    var onOpen: (NeedsItem) -> Void = { _ in }

    private var permissions: Int { items.filter { $0.kind == .permission }.count }
    private var failures: Int { items.filter { $0.kind == .failure }.count }
    private var done: Int { items.filter { $0.kind == .done }.count }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            grabber
                .padding(.top, 10)
            header
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 14)
            Text(summary)
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkTertiary)
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 4)
            list
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 12)
            Text("Handled items disappear. When nothing needs you, the strip hides itself.")
                .font(.system(size: 9.5))
                .foregroundStyle(palette.inkTertiary)
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 12)
                .padding(.bottom, SeSpace.lg)
        }
        .background(palette.surface)
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(SeNocturneRadius.sheet)
    }

    private var grabber: some View {
        Capsule()
            .fill(palette.strokeStrong)
            .frame(width: 36, height: 4)
            .frame(maxWidth: .infinity)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Text("Needs you")
                .font(SeNocturneType.sheetTitle)
                .foregroundStyle(palette.ink)
            Spacer()
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 32, height: 32)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "Close"))
        }
    }

    private var summary: String {
        var parts: [String] = []
        if permissions > 0 { parts.append("\(permissions) permission") }
        if failures > 0 { parts.append("\(failures) failure") }
        if done > 0 { parts.append("\(done) done") }
        if parts.isEmpty { return String(localized: "Nothing needs you") }
        return parts.joined(separator: " · ")
    }

    private var list: some View {
        VStack(spacing: 6) {
            ForEach(items) { item in
                Button {
                    onOpen(item)
                } label: {
                    row(item)
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func row(_ item: NeedsItem) -> some View {
        HStack(spacing: 10) {
            Text(item.kind.title)
                .font(SeNocturneType.font(8.5, weight: .medium, mono: true))
                .foregroundStyle(badgeColor(item.kind))
                .padding(.horizontal, 6)
                .frame(height: 20)
                .background(badgeBackground(item.kind), in: RoundedRectangle(cornerRadius: 5))

            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: item.title)
                    .font(SeNocturneType.rowTitle)
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: item.meta)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Text(verbatim: item.actionTitle)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.accent)
        }
        .padding(10)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func badgeColor(_ kind: NeedsItemKind) -> Color {
        switch kind {
        case .permission: palette.warning
        case .failure: palette.danger
        case .done: palette.live
        case .running: palette.accent
        }
    }

    private func badgeBackground(_ kind: NeedsItemKind) -> Color {
        switch kind {
        case .permission: palette.warningSubtle
        case .failure: palette.dangerSubtle
        case .done: palette.liveSubtle
        case .running: palette.accentSubtle
        }
    }
}

// MARK: - Previews

#Preview("Needs · Night") {
    NeedsSheetView(items: [
        NeedsItem(id: "n1", kind: .permission, title: "修改 TerminalSocket.swift", meta: "Claude Code · MacBook Pro · 2 分钟前", actionTitle: "Review"),
        NeedsItem(id: "n2", kind: .failure, title: "夜间冒烟测试失败", meta: "Schedule · Mac mini · 09:30", actionTitle: "Open"),
        NeedsItem(id: "n3", kind: .done, title: "恢复连接巡检通过", meta: "Schedule · MacBook Pro · 09:00", actionTitle: "Open"),
        NeedsItem(id: "n4", kind: .running, title: "检查 ACP 消息流", meta: "Terminal · MacBook Pro · 15 分钟前", actionTitle: "Open")
    ])
    .seThemeMode(.night)
}

#Preview("Needs · Day") {
    NeedsSheetView(items: [
        NeedsItem(id: "n1", kind: .permission, title: "修改 TerminalSocket.swift", meta: "Claude Code · MacBook Pro · 2 分钟前", actionTitle: "Review")
    ])
    .seThemeMode(.day)
}
