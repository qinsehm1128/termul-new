import SwiftUI

// MARK: - Model

enum PaletteItemKind: String, CaseIterable, Sendable {
    case session
    case file
    case command

    var title: LocalizedStringKey {
        switch self {
        case .session: "Sessions"
        case .file: "Files"
        case .command: "Commands"
        }
    }

    var systemImage: String {
        switch self {
        case .session: "bubble.left.and.text.bubble.right"
        case .file: "doc"
        case .command: "apple.terminal"
        }
    }
}

struct PaletteItem: Identifiable, Equatable, Sendable {
    let id: String
    let kind: PaletteItemKind
    let title: String
    let detail: String
    let shortcut: String
}

// MARK: - Screen

/// One search field for sessions, files and quick commands. This replaces the
/// separate search and quick-command screens from the previous IA.
struct CommandPaletteView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss

    var items: [PaletteItem]
    var onSelect: (PaletteItem) -> Void = { _ in }

    @State private var query: String = ""
    @FocusState private var isSearchFocused: Bool

    private var filtered: [PaletteItem] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return items }
        return items.filter {
            $0.title.localizedCaseInsensitiveContains(trimmed)
                || $0.detail.localizedCaseInsensitiveContains(trimmed)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            searchField
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 64)

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(PaletteItemKind.allCases, id: \.rawValue) { kind in
                        let rows = filtered.filter { $0.kind == kind }
                        if !rows.isEmpty {
                            Text(kind.title)
                                .font(SeNocturneType.monoSmall)
                                .foregroundStyle(palette.inkTertiary)
                                .padding(.top, 18)
                                .padding(.bottom, 6)
                            ForEach(rows) { item in
                                row(item)
                            }
                        }
                    }

                    if filtered.isEmpty {
                        Text("No match. Try a session title, a file name, or a command.")
                            .font(SeNocturneType.secondary)
                            .foregroundStyle(palette.inkTertiary)
                            .padding(.top, 24)
                    }
                }
                .padding(.horizontal, SeSpace.md)
                .padding(.bottom, SeSpace.xxl)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(.ultraThinMaterial)
        .background(palette.canvas.opacity(0.82))
        .onAppear { isSearchFocused = true }
        .overlay(alignment: .topTrailing) {
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
            .padding(.top, 24)
            .padding(.trailing, SeSpace.md)
            .accessibilityLabel(String(localized: "Close"))
        }
    }

    private var searchField: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkTertiary)
            TextField("Search sessions, files or commands", text: $query)
                .font(SeNocturneType.font(SeNocturneType.sectionTitleSize))
                .foregroundStyle(palette.ink)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($isSearchFocused)
                .submitLabel(.go)
                .onSubmit {
                    if let first = filtered.first { onSelect(first) }
                }
            Text("esc")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
                .padding(.horizontal, 5)
                .frame(height: 18)
                .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 4))
        }
        .padding(.horizontal, 12)
        .frame(height: 46)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.strip)
                .strokeBorder(palette.strokeStrong, lineWidth: 1)
        }
    }

    private func row(_ item: PaletteItem) -> some View {
        Button {
            onSelect(item)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: item.kind.systemImage)
                    .font(.system(size: 13, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: item.title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize + 0.5))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(verbatim: item.detail)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(verbatim: item.shortcut)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 10)
            .background(palette.card.opacity(0.6), in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
        }
        .buttonStyle(.plain)
        .padding(.bottom, 5)
    }
}

// MARK: - Previews

#Preview("Palette · Night") {
    CommandPaletteView(items: [
        PaletteItem(id: "s1", kind: .session, title: "优化远程连接恢复", detail: "Claude Code · MacBook Pro", shortcut: "waiting"),
        PaletteItem(id: "s2", kind: .session, title: "检查 ACP 消息流", detail: "Terminal · MacBook Pro", shortcut: "running"),
        PaletteItem(id: "f1", kind: .file, title: "TerminalSocket.swift", detail: "src/net · 2 matches", shortcut: "open"),
        PaletteItem(id: "f2", kind: .file, title: "ReconnectPolicy.swift", detail: "src/net · 1 match", shortcut: "open"),
        PaletteItem(id: "c1", kind: .command, title: "swift test --filter Reconnect", detail: "Quick command", shortcut: "run"),
        PaletteItem(id: "c2", kind: .command, title: "npm run dev:web", detail: "Quick command", shortcut: "run")
    ])
    .seThemeMode(.night)
}

#Preview("Palette · Day") {
    CommandPaletteView(items: [
        PaletteItem(id: "s1", kind: .session, title: "优化远程连接恢复", detail: "Claude Code · MacBook Pro", shortcut: "waiting")
    ])
    .seThemeMode(.day)
}
