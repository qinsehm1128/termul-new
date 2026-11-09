import SwiftUI

// MARK: - Model

enum NeedsItemKind: String, CaseIterable, Sendable {
    case permission
    case question
    case failure
    case done
    case running

    var title: String {
        switch self {
        case .permission: "许可"
        case .question: "问题"
        case .failure: "失败"
        case .done: "完成"
        case .running: "运行"
        }
    }

    var actionTitle: String {
        switch self {
        case .permission, .question: "处理"
        case .failure, .done, .running: "查看"
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

/// The contextual queue opened from the live strip. It is deliberately not a
/// destination: it appears over the live screen, and a handled row disappears.
struct NeedsSheetView: View {
    var items: [NeedsItem]
    var onOpen: (NeedsItem) -> Void = { _ in }

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SeSheetHeader(
                title: "待你处理",
                subtitle: subtitle,
                onClose: { dismiss() }
            )
            .padding(.horizontal, 18)
            .padding(.top, 10)

            ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 6) {
                    ForEach(items) { item in
                        row(item)
                    }
                }
                .padding(.top, 12)

                Text("处理后这一行会消失。没有待办时，这条带子直接隐藏。")
                    .font(SeNocturneType.font(9.5))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 12)
            }
            .padding(.horizontal, 18)
            .padding(.bottom, 24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(mode.sheetSurface)
        .foregroundStyle(palette.ink)
    }

    private var subtitle: String {
        let permissions = items.filter { $0.kind == .permission || $0.kind == .question }.count
        let failures = items.filter { $0.kind == .failure }.count
        let done = items.filter { $0.kind == .done }.count
        var parts: [String] = []
        if permissions > 0 { parts.append("\(permissions) 个许可") }
        if failures > 0 { parts.append("\(failures) 个失败") }
        if done > 0 { parts.append("\(done) 个完成") }
        return parts.isEmpty ? "没有待办" : parts.joined(separator: " · ")
    }

    private func row(_ item: NeedsItem) -> some View {
        Button {
            onOpen(item)
        } label: {
            HStack(spacing: 10) {
                badge(item.kind)
                VStack(alignment: .leading, spacing: 3) {
                    Text(item.title)
                        .font(SeNocturneType.font(11.5, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(item.meta)
                        .font(SeNocturneType.font(8.5, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                Text(item.kind.actionTitle)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.accent)
            }
            .padding(10)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .strokeBorder(mode.hairline, lineWidth: 1)
            }
            .contentShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
        }
        .buttonStyle(SePressable())
    }

    private func badge(_ kind: NeedsItemKind) -> some View {
        Text(kind.title)
            .font(SeNocturneType.font(8, mono: true))
            .foregroundStyle(foreground(kind))
            .padding(.horizontal, 6)
            .frame(height: 18)
            .background(background(kind), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
    }

    private func foreground(_ kind: NeedsItemKind) -> Color {
        switch kind {
        case .permission: palette.warning
        case .question: palette.accent
        case .failure: palette.danger
        case .done: palette.live
        case .running: palette.accent
        }
    }

    private func background(_ kind: NeedsItemKind) -> Color {
        switch kind {
        case .permission: palette.warningSubtle
        case .question: palette.accentSubtle
        case .failure: palette.dangerSubtle
        case .done: palette.liveSubtle
        case .running: palette.accentSubtle
        }
    }
}
