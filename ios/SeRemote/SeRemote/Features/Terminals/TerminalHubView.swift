import SwiftUI

/// Callbacks the hub list forwards. The screen that owns host access decides
/// what each one does; the list itself never talks to the host.
struct TerminalHubActions {
    var openShell: (TerminalHubIndex.Shell) -> Void
    var openQuick: (TerminalHubIndex.Quick) -> Void
    var newTerminal: () -> Void
    var newTerminalInProject: (String) -> Void
    var renameQuick: (String) -> Void
    var closeQuick: (String) -> Void
    var deleteQuick: (String) -> Void
    var closeShell: (TerminalHubIndex.Shell) -> Void
    var refresh: () async -> Void
}

/// Direction one of the terminal hub ("搜索即操作").
///
/// Search and filters sit at the bottom, in thumb reach. Above them one list:
/// quick terminals (each expandable to its shells), then host project
/// groups → projects → shells. The same view is the in-terminal switcher.
struct TerminalHubView<Header: View>: View {
    let index: TerminalHubIndex
    @Binding var query: TerminalHubIndex.Query
    var selectedTerminalID: String?
    var isLoading = false
    var notice: String?
    var errorMessage: String?
    var onDismissNotice: (() -> Void)?
    var onDismissError: (() -> Void)?
    let actions: TerminalHubActions
    @ViewBuilder var header: Header

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var expandedQuick: Set<String> = []
    @State private var collapsedGroups: Set<String> = []
    @FocusState private var searchFocused: Bool

    private static var gutter: CGFloat { 16 }

    var body: some View {
        List {
            header
                .hubRow()

            if let notice {
                NocturneNotice(message: notice, isError: false, onDismiss: onDismissNotice)
                    .hubRow(top: 8)
            }
            if let errorMessage {
                NocturneNotice(message: errorMessage, isError: true, onDismiss: onDismissError)
                    .hubRow(top: 8)
            }

            if index.isEmpty {
                emptyState.hubRow(top: 24)
            } else {
                if !index.projectHits.isEmpty {
                    sectionLabel(hubText("Projects"), count: index.projectHits.count)
                    ForEach(index.projectHits) { hit in
                        projectHitRow(hit).hubRow()
                    }
                }
                if !index.quick.isEmpty {
                    sectionLabel(hubText("Quick"), count: index.quick.count)
                    ForEach(index.quick) { quick in
                        quickRow(quick).hubRow()
                        if isExpanded(quick) {
                            ForEach(quick.shells) { shell in
                                shellRow(shell, inQuick: true).hubRow()
                            }
                        }
                    }
                }
                ForEach(index.groups) { group in
                    if index.showsGroupHeaders {
                        groupHeader(group).hubRow(top: 12)
                    }
                    if !collapsedGroups.contains(group.id) {
                        ForEach(group.projects) { project in
                            projectHeader(project)
                            ForEach(project.shells) { shell in
                                shellRow(shell, inQuick: false).hubRow()
                            }
                        }
                    }
                }
            }

            Color.clear.frame(height: 12).hubRow()
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .scrollDismissesKeyboard(.interactively)
        .environment(\.defaultMinListRowHeight, 0)
        .refreshable { await actions.refresh() }
        .background(SeVelvetBackdrop())
        .safeAreaInset(edge: .bottom, spacing: 0) { controls }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: expandedQuick)
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: collapsedGroups)
    }

    // MARK: - Sections

    private func sectionLabel(_ title: String, count: Int) -> some View {
        HStack(spacing: 8) {
            Text(verbatim: title)
                .font(SeNocturneType.font(13, weight: .semibold))
                .foregroundStyle(palette.inkSecondary)
            Text(verbatim: "\(count)")
                .font(SeNocturneType.font(12, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 0)
        }
        .padding(.top, 20)
        .padding(.bottom, 4)
        .accessibilityAddTraits(.isHeader)
        .hubRow()
    }

    private func groupHeader(_ group: TerminalHubIndex.GroupSection) -> some View {
        let collapsed = collapsedGroups.contains(group.id)
        return Button {
            if collapsed { collapsedGroups.remove(group.id) } else { collapsedGroups.insert(group.id) }
        } label: {
            HStack(spacing: 8) {
                Text(verbatim: group.name)
                    .font(SeNocturneType.font(13, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .lineLimit(1)
                Text(verbatim: "\(group.count)")
                    .font(SeNocturneType.font(12, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                Spacer(minLength: 0)
                SeIconView(icon: .chevronDown, size: 12, stroke: 1.6, color: palette.inkTertiary)
                    .rotationEffect(.degrees(collapsed ? -90 : 0))
            }
            .frame(minHeight: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(Text(verbatim: group.name))
        .accessibilityValue(Text(collapsed ? hubText("Collapsed") : hubText("Expanded")))
        .accessibilityAddTraits(.isHeader)
    }

    private func projectHeader(_ project: TerminalHubIndex.ProjectSection) -> some View {
        HStack(spacing: 8) {
            Text(verbatim: project.name)
                .font(SeNocturneType.font(13, weight: .medium))
                .foregroundStyle(palette.ink.opacity(0.82))
                .lineLimit(1)
            Text(verbatim: "\(project.shells.count)")
                .font(SeNocturneType.font(12, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 0)
            if index.canCreate, let projectID = project.projectID {
                Button {
                    actions.newTerminalInProject(projectID)
                } label: {
                    SeIconView(icon: .plus, size: 14, stroke: 1.6, color: palette.inkSecondary)
                        .frame(width: 44, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(Text(hubText("New terminal in \(project.name)")))
            }
        }
        .padding(.top, 8)
        .frame(minHeight: 36)
        .hubRow()
    }

    // MARK: - Rows

    private func shellRow(_ shell: TerminalHubIndex.Shell, inQuick: Bool) -> some View {
        let selected = shell.id == selectedTerminalID
        return Button {
            actions.openShell(shell)
        } label: {
            HStack(alignment: .center, spacing: 12) {
                Circle()
                    .fill(shell.hasAgent ? palette.accent : palette.live)
                    .frame(width: 8, height: 8)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(verbatim: shell.title)
                            .font(SeNocturneType.font(15, weight: .medium))
                            .foregroundStyle(palette.ink)
                            .lineLimit(1)
                        if shell.isPinned, !inQuick {
                            SeIconView(icon: .bolt, size: 12, stroke: 1.6, color: palette.inkTertiary)
                        }
                    }
                    if let meta = shellMeta(shell, inQuick: inQuick) {
                        Text(verbatim: meta)
                            .font(SeNocturneType.font(12, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                            .truncationMode(.head)
                    }
                }
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 2) {
                    if let branch = shell.branch {
                        Text(verbatim: branch)
                            .font(SeNocturneType.font(12, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                    }
                    if shell.hasAgent {
                        Text(verbatim: hubText("Agent"))
                            .font(SeNocturneType.font(12, weight: .medium))
                            .foregroundStyle(palette.accent)
                    }
                }
            }
            .padding(.vertical, 10)
            .padding(.leading, inQuick ? 40 : 0)
            .padding(.horizontal, selected ? 8 : 0)
            .frame(minHeight: 52)
            .background(selected ? palette.accentSubtle : Color.clear,
                        in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if shell.quickID != nil {
                Button(role: .destructive) {
                    actions.closeShell(shell)
                } label: {
                    Text(verbatim: hubText("Close"))
                }
            }
        }
        .contextMenu {
            if shell.quickID != nil {
                Button(role: .destructive) {
                    actions.closeShell(shell)
                } label: {
                    Text(verbatim: hubText("Close shell"))
                }
            }
        }
    }

    private func shellMeta(_ shell: TerminalHubIndex.Shell, inQuick: Bool) -> String? {
        let path = TerminalHubIndex.displayPath(shell.cwd)
        if inQuick, let project = shell.projectName {
            return [project, path].compactMap { $0 }.joined(separator: "  ")
        }
        return path ?? shell.projectName
    }

    private func quickRow(_ quick: TerminalHubIndex.Quick) -> some View {
        Button {
            if quick.liveCount > 1 {
                toggle(quick)
            } else if let shell = quick.shells.first {
                actions.openShell(shell)
            } else {
                actions.openQuick(quick)
            }
        } label: {
            HStack(spacing: 12) {
                SeIconView(icon: .bolt, size: 14, stroke: 1.6, color: quick.isRunning ? palette.ink : palette.inkTertiary)
                    .frame(width: 28, height: 28)
                    .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .strokeBorder(mode.insetStroke, lineWidth: 1)
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: quick.title)
                        .font(SeNocturneType.font(15, weight: .medium))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(verbatim: quickMeta(quick))
                        .font(SeNocturneType.font(12, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
                Spacer(minLength: 8)
                quickTrailing(quick)
            }
            .padding(.vertical, 8)
            .frame(minHeight: 52)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .disabled(!quick.canOpen && quick.liveCount == 0)
        .accessibilityElement(children: .combine)
        .accessibilityHint(Text(quick.liveCount > 1 ? hubText("Shows its shells") : hubText("Opens the terminal")))
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(role: .destructive) {
                actions.deleteQuick(quick.id)
            } label: {
                Text(verbatim: hubText("Delete"))
            }
            if quick.isRunning {
                Button {
                    actions.closeQuick(quick.id)
                } label: {
                    Text(verbatim: hubText("Close"))
                }
                .tint(palette.warning)
            }
            Button {
                actions.renameQuick(quick.id)
            } label: {
                Text(verbatim: hubText("Rename"))
            }
            .tint(palette.accent)
        }
        .contextMenu {
            Button { actions.renameQuick(quick.id) } label: { Text(verbatim: hubText("Rename")) }
            if quick.isRunning {
                Button(role: .destructive) { actions.closeQuick(quick.id) } label: { Text(verbatim: hubText("Close shells")) }
            }
            Button(role: .destructive) { actions.deleteQuick(quick.id) } label: { Text(verbatim: hubText("Delete")) }
        }
    }

    private func quickMeta(_ quick: TerminalHubIndex.Quick) -> String {
        if quick.liveCount == 0 { return hubText("Not started") }
        if quick.liveCount == 1, let shell = quick.shells.first { return shell.title }
        return TerminalHubIndex.displayPath(quick.cwd) ?? quick.cwd
    }

    @ViewBuilder
    private func quickTrailing(_ quick: TerminalHubIndex.Quick) -> some View {
        if quick.liveCount == 0 {
            Text(verbatim: hubText("Start"))
                .font(SeNocturneType.font(13, weight: .medium))
                .foregroundStyle(quick.canOpen ? palette.accent : palette.inkTertiary)
        } else if quick.liveCount > 1 {
            HStack(spacing: 6) {
                Text(verbatim: "\(quick.liveCount)")
                    .font(SeNocturneType.font(12, mono: true))
                    .foregroundStyle(palette.live)
                SeIconView(icon: .chevronDown, size: 12, stroke: 1.6, color: palette.inkTertiary)
                    .rotationEffect(.degrees(isExpanded(quick) ? 0 : -90))
            }
        } else {
            SeIconView(icon: .chevron, size: 12, stroke: 1.6, color: palette.inkTertiary)
        }
    }

    private func projectHitRow(_ hit: TerminalHubIndex.ProjectHit) -> some View {
        HStack(spacing: 12) {
            SeIconView(icon: .folder, size: 16, stroke: 1.6, color: palette.inkSecondary)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 2) {
                HighlightedText(text: hit.name, needle: query.trimmedText, palette: palette)
                    .font(SeNocturneType.font(15, weight: .medium))
                    .lineLimit(1)
                let meta = [hit.groupName, TerminalHubIndex.displayPath(hit.path)].compactMap { $0 }.joined(separator: "  ")
                if !meta.isEmpty {
                    Text(verbatim: meta)
                        .font(SeNocturneType.font(12, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 8)
            if hit.runningCount > 0 {
                Button {
                    query.text = hit.name
                } label: {
                    Text(verbatim: hubText("\(hit.runningCount) running"))
                        .font(SeNocturneType.font(12, mono: true))
                        .foregroundStyle(palette.live)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
            }
            if index.canCreate {
                Button {
                    actions.newTerminalInProject(hit.id)
                } label: {
                    Text(verbatim: hubText("New"))
                        .font(SeNocturneType.font(13, weight: .semibold))
                        .foregroundStyle(palette.onAccent)
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(palette.accent, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(Text(hubText("New terminal in \(hit.name)")))
            }
        }
        .frame(minHeight: 52)
    }

    // MARK: - Empty

    @ViewBuilder
    private var emptyState: some View {
        VStack(spacing: 12) {
            if isLoading {
                ProgressView().controlSize(.small)
                Text(verbatim: hubText("Loading terminals…"))
                    .font(SeNocturneType.font(15))
                    .foregroundStyle(palette.inkSecondary)
            } else if query.isFiltering {
                SeIconView(icon: .search, size: 20, stroke: 1.6, color: palette.inkTertiary)
                Text(verbatim: hubText("No matching terminals"))
                    .font(SeNocturneType.font(15, weight: .medium))
                    .foregroundStyle(palette.ink)
                Button {
                    query = .init()
                } label: {
                    Text(verbatim: hubText("Clear filters"))
                        .font(SeNocturneType.font(13, weight: .semibold))
                        .foregroundStyle(palette.accent)
                        .padding(.horizontal, 16)
                        .frame(height: 36)
                        .overlay {
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .strokeBorder(palette.accent.opacity(0.5), lineWidth: 1)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
            } else {
                SeIconView(icon: .terminal, size: 20, stroke: 1.6, color: palette.inkTertiary)
                Text(verbatim: hubText("No running terminals"))
                    .font(SeNocturneType.font(15, weight: .medium))
                    .foregroundStyle(palette.ink)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 32)
        .accessibilityElement(children: .combine)
    }

    // MARK: - Bottom controls

    private var controls: some View {
        VStack(spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(TerminalHubIndex.StatusFilter.allCases) { filter in
                        statusChip(filter)
                    }
                    if !index.groupOptions.isEmpty {
                        groupMenu
                    }
                }
                .padding(.horizontal, Self.gutter)
            }
            .scrollClipDisabled()

            HStack(spacing: 8) {
                searchField
                Button(action: actions.newTerminal) {
                    SeIconView(icon: .plus, size: 18, stroke: 1.8, color: palette.onAccent)
                        .frame(width: 44, height: 44)
                        .background(palette.accent, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .disabled(!index.canCreate)
                .opacity(index.canCreate ? 1 : 0.4)
                .accessibilityLabel(Text(hubText("New terminal")))
            }
            .padding(.horizontal, Self.gutter)
        }
        .padding(.top, 8)
        .padding(.bottom, 8)
        .background(palette.canvas.ignoresSafeArea(edges: .bottom))
        .overlay(alignment: .top) {
            Rectangle().fill(mode.hairline).frame(height: 1)
        }
    }

    private func statusChip(_ filter: TerminalHubIndex.StatusFilter) -> some View {
        let on = query.status == filter
        return Button {
            query.status = on && filter != .all ? .all : filter
        } label: {
            HStack(spacing: 6) {
                Text(verbatim: filter.title)
                    .font(SeNocturneType.font(13, weight: on ? .semibold : .regular))
                Text(verbatim: "\(index.count(filter))")
                    .font(SeNocturneType.font(12, mono: true))
                    .opacity(0.7)
            }
            .foregroundStyle(on ? palette.accent : palette.inkSecondary)
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(on ? palette.accentSubtle : mode.insetFill, in: Capsule(style: .continuous))
            .overlay {
                Capsule(style: .continuous)
                    .strokeBorder(on ? palette.accent.opacity(0.45) : mode.insetStroke, lineWidth: 1)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private var groupMenu: some View {
        let selected = index.groupOptions.first(where: { $0.id == query.groupID })
        return Menu {
            Button {
                query.groupID = nil
            } label: {
                if selected == nil {
                    Label(hubText("All groups"), systemImage: "checkmark")
                } else {
                    Text(verbatim: hubText("All groups"))
                }
            }
            ForEach(index.groupOptions) { option in
                Button {
                    query.groupID = option.id
                } label: {
                    let title = "\(option.name)  \(option.runningCount)"
                    if option.id == query.groupID {
                        Label(title, systemImage: "checkmark")
                    } else {
                        Text(verbatim: title)
                    }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Text(verbatim: selected?.name ?? hubText("Groups"))
                    .font(SeNocturneType.font(13, weight: selected == nil ? .regular : .semibold))
                    .lineLimit(1)
                SeIconView(icon: .chevronUpDown, size: 12, stroke: 1.6,
                           color: selected == nil ? palette.inkTertiary : palette.accent)
            }
            .foregroundStyle(selected == nil ? palette.inkSecondary : palette.accent)
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(selected == nil ? mode.insetFill : palette.accentSubtle, in: Capsule(style: .continuous))
            .overlay {
                Capsule(style: .continuous)
                    .strokeBorder(selected == nil ? mode.insetStroke : palette.accent.opacity(0.45), lineWidth: 1)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .accessibilityLabel(Text(hubText("Group filter")))
        .accessibilityValue(Text(verbatim: selected?.name ?? hubText("All groups")))
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            SeIconView(icon: .search, size: 16, stroke: 1.6, color: palette.inkTertiary)
            TextField(text: $query.text) {
                Text(verbatim: hubText("Search projects, terminals, branches"))
            }
            .font(SeNocturneType.font(15))
            .foregroundStyle(palette.ink)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .submitLabel(.search)
            .focused($searchFocused)
            if !query.text.isEmpty {
                Button {
                    query.text = ""
                } label: {
                    SeIconView(icon: .close, size: 12, stroke: 1.7, color: palette.inkSecondary)
                        .frame(width: 32, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(Text(hubText("Clear search")))
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, query.text.isEmpty ? 12 : 4)
        .frame(height: 44)
        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(searchFocused ? palette.accent.opacity(0.6) : mode.insetStroke, lineWidth: 1)
        }
    }

    // MARK: - Expansion

    private func isExpanded(_ quick: TerminalHubIndex.Quick) -> Bool {
        guard quick.shells.count > 1 || (quick.shells.count == 1 && query.isFiltering) else { return false }
        return query.isFiltering || expandedQuick.contains(quick.id)
    }

    private func toggle(_ quick: TerminalHubIndex.Quick) {
        if expandedQuick.contains(quick.id) {
            expandedQuick.remove(quick.id)
        } else {
            expandedQuick.insert(quick.id)
        }
    }
}

/// Bolds the part of a name that matches the search text.
private struct HighlightedText: View {
    let text: String
    let needle: String
    let palette: SePalette

    var body: some View {
        Text(attributed)
    }

    private var attributed: AttributedString {
        var value = AttributedString(text)
        value.foregroundColor = palette.ink
        guard !needle.isEmpty,
              let range = text.range(of: needle, options: .caseInsensitive),
              let lower = AttributedString.Index(range.lowerBound, within: value),
              let upper = AttributedString.Index(range.upperBound, within: value) else {
            return value
        }
        value[lower..<upper].foregroundColor = palette.accent
        return value
    }
}

private extension View {
    /// A list row without List chrome: no separator, no background, the
    /// hub's own gutter.
    func hubRow(top: CGFloat = 0) -> some View {
        self
            .listRowInsets(EdgeInsets(top: top, leading: 16, bottom: 0, trailing: 16))
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
    }
}
