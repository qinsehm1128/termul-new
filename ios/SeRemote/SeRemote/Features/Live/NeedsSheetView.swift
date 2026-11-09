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
        case .permission: String(localized: "Permission")
        case .question: String(localized: "Question")
        case .failure: String(localized: "Failure")
        case .done: String(localized: "Done")
        case .running: String(localized: "Running")
        }
    }

    var actionTitle: String {
        switch self {
        case .permission, .question: String(localized: "Handle")
        case .failure, .done, .running: String(localized: "View")
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
                title: String(localized: "Needs you"),
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

                Color.clear
                    .frame(height: 12)
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
        if permissions > 0 { parts.append(String(localized: "\(permissions) permissions")) }
        if failures > 0 { parts.append(String(localized: "\(failures) failures")) }
        if done > 0 { parts.append(String(localized: "\(done) done")) }
        return parts.isEmpty ? String(localized: "Nothing waiting") : parts.joined(separator: " · ")
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
