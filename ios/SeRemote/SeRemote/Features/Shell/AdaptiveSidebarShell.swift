import SwiftUI

// MARK: - Sidebar model

enum SidebarDestination: String, CaseIterable, Identifiable, Sendable {
    case live
    case workspace
    case files
    case changes
    case schedules
    case quick
    case settings

    var id: String { rawValue }

    var title: LocalizedStringKey {
        switch self {
        case .live: "Live"
        case .workspace: "Workspace"
        case .files: "Files"
        case .changes: "Changes"
        case .schedules: "Schedules"
        case .quick: "Quick"
        case .settings: "Settings"
        }
    }

    var helpText: String {
        switch self {
        case .live: String(localized: "Live")
        case .workspace: String(localized: "Workspace")
        case .files: String(localized: "Files")
        case .changes: String(localized: "Changes")
        case .schedules: String(localized: "Schedules")
        case .quick: String(localized: "Quick")
        case .settings: String(localized: "Settings")
        }
    }

    var systemImage: String {
        switch self {
        case .live: "dot.radiowaves.left.and.right"
        case .workspace: "folder"
        case .files: "doc"
        case .changes: "arrow.triangle.branch"
        case .schedules: "clock"
        case .quick: "apple.terminal"
        case .settings: "slider.horizontal.3"
        }
    }
}

struct SidebarHost: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let isOnline: Bool
}

// MARK: - Adaptive shell

/// Regular-width shell for iPad. The sidebar is 250pt in landscape and 76pt in
/// portrait; both can be toggled. Compact widths are expected to use the
/// phone screens instead of this shell.
struct AdaptiveSidebarShell<Detail: View>: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let hosts: [SidebarHost]
    private let counts: [SidebarDestination: String]
    private let detail: (SidebarDestination) -> Detail

    @State private var localSelection: SidebarDestination
    private var selectionBinding: Binding<SidebarDestination>?
    private var onOpenCommand: () -> Void = {}
    private var onWidthChange: (CGFloat) -> Void = { _ in }

    private var selection: SidebarDestination { selectionBinding?.wrappedValue ?? localSelection }

    private func select(_ destination: SidebarDestination) {
        if let selectionBinding { selectionBinding.wrappedValue = destination }
        else { localSelection = destination }
    }
    @State private var collapsedOverride: Bool?

    init(
        selection: SidebarDestination = .live,
        hosts: [SidebarHost] = [],
        counts: [SidebarDestination: String] = [:],
        @ViewBuilder detail: @escaping (SidebarDestination) -> Detail
    ) {
        _localSelection = State(initialValue: selection)
        self.hosts = hosts
        self.counts = counts
        self.detail = detail
    }

    init(
        selection: Binding<SidebarDestination>,
        hosts: [SidebarHost] = [],
        counts: [SidebarDestination: String] = [:],
        onOpenCommand: @escaping () -> Void,
        onWidthChange: @escaping (CGFloat) -> Void = { _ in },
        @ViewBuilder detail: @escaping (SidebarDestination) -> Detail
    ) {
        _localSelection = State(initialValue: selection.wrappedValue)
        self.selectionBinding = selection
        self.hosts = hosts
        self.counts = counts
        self.onOpenCommand = onOpenCommand
        self.onWidthChange = onWidthChange
        self.detail = detail
    }

    var body: some View {
        GeometryReader { geometry in
            let isLandscape = geometry.size.width > geometry.size.height
            let isCollapsed = collapsedOverride ?? !isLandscape

            HStack(spacing: 0) {
                sidebar(isCollapsed: isCollapsed)
                detail(selection)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .background(palette.canvas)
            .onChange(of: isLandscape) { _, _ in
                collapsedOverride = nil
            }
        }
    }

    private func sidebar(isCollapsed: Bool) -> some View {
        VStack(alignment: isCollapsed ? .center : .leading, spacing: 0) {
            brand(isCollapsed: isCollapsed)
                .padding(.horizontal, isCollapsed ? 0 : 12)
                .padding(.top, 18)
                .padding(.bottom, 12)

            if !isCollapsed {
                searchField
                    .padding(.horizontal, 12)
                    .padding(.bottom, 6)
            }

            ScrollView {
                VStack(alignment: isCollapsed ? .center : .leading, spacing: 2) {
                    groupLabel("Work", isCollapsed: isCollapsed)
                    ForEach([SidebarDestination.live, .workspace]) { destination in
                        item(destination, isCollapsed: isCollapsed)
                    }

                    if !hosts.isEmpty {
                        groupLabel("Hosts", isCollapsed: isCollapsed)
                        ForEach(hosts) { hostRow($0, isCollapsed: isCollapsed) }
                    }

                    groupLabel("Tools", isCollapsed: isCollapsed)
                    ForEach([SidebarDestination.files, .changes, .schedules, .quick]) { destination in
                        item(destination, isCollapsed: isCollapsed)
                    }
                }
                .padding(.horizontal, isCollapsed ? 8 : 10)
                .padding(.top, 4)
            }
            .scrollIndicators(.hidden)

            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
                .padding(.horizontal, isCollapsed ? 10 : 12)

            item(.settings, isCollapsed: isCollapsed)
                .padding(.horizontal, isCollapsed ? 8 : 10)
                .padding(.vertical, 10)

            if !isCollapsed {
                Text(verbatim: ShellPresentation.versionLabel(info: Bundle.main.infoDictionary ?? [:]))
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.bottom, 12)
            }
        }
        .frame(width: isCollapsed ? 76 : 250)
        .frame(maxHeight: .infinity)
        .background(palette.card)
        .overlay(alignment: .trailing) {
            Rectangle()
                .fill(palette.stroke)
                .frame(width: 1)
        }
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion), value: isCollapsed)
    }

    private func brand(isCollapsed: Bool) -> some View {
        HStack(spacing: 9) {
            if !isCollapsed {
            Text("Se")
                .font(SeNocturneType.font(13, weight: .semibold, mono: true))
                .foregroundStyle(palette.onAccent)
                .frame(width: 32, height: 32)
                .background(palette.accent, in: RoundedRectangle(cornerRadius: 9))
            }

            if !isCollapsed {
                VStack(alignment: .leading, spacing: 2) {
                    Text("SeRemote")
                        .font(SeNocturneType.font(13, weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Text(verbatim: hosts.first?.name ?? "No host")
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                }
            }

            if !isCollapsed { Spacer(minLength: 0) }

            Button {
                collapsedOverride = !isCollapsed
            } label: {
                Image(systemName: "sidebar.left")
                    .font(.system(size: 13, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 44, height: 44)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 7))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(isCollapsed ? "Expand sidebar" : "Collapse sidebar")
        }
        .frame(maxWidth: .infinity, alignment: isCollapsed ? .center : .leading)
    }

    private var searchField: some View {
        Button(action: onOpenCommand) {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkTertiary)
            Text("Search or run a command")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 4)
            Text("⌘")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.horizontal, 9)
        .frame(height: 44)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        }
        .buttonStyle(.plain)
        .keyboardShortcut("k", modifiers: .command)
    }

    private func groupLabel(_ title: LocalizedStringKey, isCollapsed: Bool) -> some View {
        Group {
            if isCollapsed {
                Rectangle()
                    .fill(palette.stroke)
                    .frame(width: 18, height: 1)
                    .padding(.vertical, 8)
            } else {
                Text(title)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 12)
                    .padding(.bottom, 4)
                    .padding(.horizontal, 6)
            }
        }
        .frame(maxWidth: .infinity, alignment: isCollapsed ? .center : .leading)
    }

    private func item(_ destination: SidebarDestination, isCollapsed: Bool) -> some View {
        let isOn = selection == destination
        return Button {
            select(destination)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: destination.systemImage)
                    .font(.system(size: 15, weight: .regular))
                    .foregroundStyle(isOn ? palette.accent : palette.inkTertiary)
                    .frame(width: 18)
                if !isCollapsed {
                    Text(destination.title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: isOn ? .medium : .regular))
                        .foregroundStyle(isOn ? palette.ink : palette.inkSecondary)
                    Spacer(minLength: 4)
                    if let count = counts[destination] {
                        Text(verbatim: count)
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(palette.inkTertiary)
                    }
                }
            }
            .padding(.horizontal, isCollapsed ? 0 : 9)
            .frame(height: 44)
            .frame(maxWidth: .infinity, alignment: isCollapsed ? .center : .leading)
            .background(
                isOn ? palette.accentSubtle : .clear,
                in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
            )
        }
        .buttonStyle(.plain)
        .help(destination.helpText)
    }

    private func hostRow(_ host: SidebarHost, isCollapsed: Bool) -> some View {
        Button {
            select(.live)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 15, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: 18)
                if !isCollapsed {
                    Text(verbatim: host.name)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize))
                        .foregroundStyle(palette.inkSecondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                }
                Circle()
                    .fill(host.isOnline ? palette.live : palette.inkTertiary)
                    .frame(width: 6, height: 6)
            }
            .padding(.horizontal, isCollapsed ? 0 : 9)
            .frame(height: 44)
            .frame(maxWidth: .infinity, alignment: isCollapsed ? .center : .leading)
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Previews

#Preview("iPad landscape") {
    AdaptiveSidebarShell(
        hosts: [
            SidebarHost(id: "mbp", name: "MacBook Pro", isOnline: true),
            SidebarHost(id: "mini", name: "Mac mini", isOnline: false)
        ],
        counts: [.live: "3", .files: "482", .changes: "2", .schedules: "4", .quick: "4"]
    ) { destination in
        VStack(alignment: .leading, spacing: 12) {
            Text(destination.title)
                .font(SeNocturneType.screenTitle)
                .foregroundStyle(.primary)
            Text("Detail content for \(destination.rawValue).")
                .font(SeNocturneType.secondary)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(28)
    }
    .seThemeMode(.night)
    .frame(width: 1194, height: 834)
}

#Preview("iPad portrait") {
    AdaptiveSidebarShell(
        hosts: [SidebarHost(id: "mbp", name: "MacBook Pro", isOnline: true)],
        counts: [.live: "3"]
    ) { destination in
        VStack {
            Text(destination.title)
                .font(SeNocturneType.screenTitle)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    .seThemeMode(.day)
    .frame(width: 834, height: 1194)
}
