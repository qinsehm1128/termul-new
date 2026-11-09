import SwiftUI

// MARK: - Model

enum PaletteItemKind: String, CaseIterable, Sendable {
    case session
    case file
    case command

    var title: String {
        switch self {
        case .session: "会话"
        case .file: "文件"
        case .command: "命令"
        }
    }

    var icon: SeIcon {
        switch self {
        case .session: .clock
        case .file: .file
        case .command: .play
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

/// One search field for sessions, files and quick commands. It covers the
/// workbench instead of pushing a destination.
struct CommandPaletteView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var items: [PaletteItem]
    var onSelect: (PaletteItem) -> Void = { _ in }
    var onClose: (() -> Void)?

    @State private var localQuery = ""
    private var externalQuery: Binding<String>?
    private var filtersItemsLocally = true
    @FocusState private var isSearchFocused: Bool

    init(items: [PaletteItem], onSelect: @escaping (PaletteItem) -> Void = { _ in }, onClose: (() -> Void)? = nil) {
        self.items = items
        self.onSelect = onSelect
        self.onClose = onClose
    }

    /// Connected callers own filtering so content-search hits are not filtered
    /// again by their file name (the matching text may be inside the file).
    init(items: [PaletteItem], query: Binding<String>, onSelect: @escaping (PaletteItem) -> Void, onClose: (() -> Void)? = nil) {
        self.items = items
        self.externalQuery = query
        self.filtersItemsLocally = false
        self.onSelect = onSelect
        self.onClose = onClose
    }

    private var queryBinding: Binding<String> { externalQuery ?? $localQuery }

    private var filtered: [PaletteItem] {
        guard filtersItemsLocally else { return items }
        let trimmed = queryBinding.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return items }
        return items.filter {
            $0.title.localizedCaseInsensitiveContains(trimmed)
                || $0.detail.localizedCaseInsensitiveContains(trimmed)
        }
    }

    var body: some View {
        ZStack(alignment: .top) {
            Color(hex: mode.isNight ? 0x07080B : 0xEBF0F7)
                .opacity(mode.isNight ? 0.82 : 0.86)
                .background(.ultraThinMaterial)
                .ignoresSafeArea()
                .onTapGesture { onClose?() }

            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    searchField
                    ForEach(PaletteItemKind.allCases, id: \.rawValue) { kind in
                        let rows = filtered.filter { $0.kind == kind }
                        if !rows.isEmpty {
                            Text(kind.title)
                                .font(SeNocturneType.font(8.5, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                                .padding(.top, 16)
                                .padding(.bottom, 6)
                                .padding(.horizontal, 4)
                            ForEach(rows) { item in
                                row(item)
                            }
                        }
                    }
                    Text("命令面板替代了独立的搜索与速启入口：输一次，三种结果。")
                        .font(SeNocturneType.font(9.5))
                        .foregroundStyle(palette.inkTertiary)
                        .padding(.top, 18)
                        .padding(.horizontal, 4)
                }
                .padding(.horizontal, 16)
                .padding(.top, 74)
                .padding(.bottom, 96)
            }
        }
        .onAppear { isSearchFocused = true }
    }

    private var searchField: some View {
        HStack(spacing: 9) {
            SeIconView(icon: .search, size: 15, stroke: 1.6, color: palette.inkTertiary)
            TextField("搜索会话、文件或命令", text: queryBinding)
                .font(SeNocturneType.font(13))
                .foregroundStyle(palette.ink)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($isSearchFocused)
                .submitLabel(.search)
            Button {
                onClose?()
            } label: {
                Text("esc")
                    .font(SeNocturneType.font(8, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .overlay {
                        RoundedRectangle(cornerRadius: 4)
                            .strokeBorder(mode.insetStroke, lineWidth: 1)
                    }
                    .frame(minWidth: 32, minHeight: 32)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("关闭命令面板")
        }
        .padding(.horizontal, 12)
        .frame(height: 46)
        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(mode.insetStroke, lineWidth: 1)
        }
    }

    private func row(_ item: PaletteItem) -> some View {
        Button {
            onSelect(item)
        } label: {
            HStack(spacing: 10) {
                if item.kind == .session {
                    SeNeedDot(kind: statusDot(item.shortcut), size: 6)
                } else {
                    SeIconView(icon: item.kind.icon, size: 14, stroke: 1.6, color: palette.inkTertiary)
                }
                Text(item.title)
                    .font(SeNocturneType.font(12))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Text(item.shortcut)
                    .font(SeNocturneType.font(8.5, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Color.clear, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
        }
        .buttonStyle(SePressable())
    }

    private func statusDot(_ shortcut: String) -> SeNeedDot.Kind {
        switch shortcut {
        case "等待你": .wait
        case "运行中": .live
        default: .idle
        }
    }
}

#if DEBUG
#Preview("Palette") {
    CommandPaletteView(items: [
        PaletteItem(id: "s1", kind: .session, title: "优化远程连接恢复", detail: "Claude Code · MacBook Pro", shortcut: "等待你"),
        PaletteItem(id: "f1", kind: .file, title: "TerminalSocket.swift", detail: "src/net · 2 处命中", shortcut: "2 处命中"),
        PaletteItem(id: "c1", kind: .command, title: "swift test --filter Reconnect", detail: "速启", shortcut: "速启")
    ])
    .seThemeMode(.night)
}
#endif
