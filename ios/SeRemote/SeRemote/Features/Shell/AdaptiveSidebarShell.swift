import SwiftUI

/// The three places the connected shell exposes. Terminals lead: that is where
/// the host lands, and the rail lists them before anything else.
enum SidebarDestination: String, CaseIterable, Identifiable, Sendable {
    case conversations
    case terminals
    case tools
    case settings

    var id: String { rawValue }

    /// The three primary sections, in bar/rail order.
    static let primary: [SidebarDestination] = [.terminals, .conversations, .tools]

    var title: String {
        switch self {
        case .conversations: String(localized: "Conversations")
        case .terminals: String(localized: "Terminals")
        case .tools: String(localized: "Tools")
        case .settings: String(localized: "Settings")
        }
    }

    var icon: SeIcon {
        switch self {
        case .conversations: .chat
        case .terminals: .terminal
        case .tools: .folder
        case .settings: .gear
        }
    }
}

/// iPad directory: 250 pt expanded, 76 pt rail collapsed. The width is the
/// only input; orientation decides the default, and the user can flip it.
struct AdaptiveSidebarShell: View {
    @Binding var selection: SidebarDestination
    @Binding var collapsed: Bool
    let hostName: String
    let hostOnline: Bool
    let counts: [SidebarDestination: String]
    let version: String
    let onOpenCommand: () -> Void
    /// Terminals grouped by project. The iPad rail is the terminal list, so it
    /// reads the same grouped model T1 renders.
    var terminalGroups: [TerminalLandingModel.Group] = []
    var selectedTerminalID: String? = nil
    var onOpenTerminal: ((TerminalLandingModel.Row) -> Void)? = nil
    var expandedWidth: CGFloat = ShellPresentation.railExpandedWidth
    /// The phone drawer passes this so the header's trailing button closes the
    /// drawer (prototype `.railclose`) instead of folding a nil-effect rail.
    var onClose: (() -> Void)? = nil

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass

    private var width: CGFloat {
        collapsed ? ShellPresentation.railCollapsedWidth : expandedWidth
    }

    /// The brand subtitle follows the size class. The prototype hard-codes
    /// `iPad · Nocturne` in the phone drawer too (U-5) — never copy that.
    private var subtitle: String {
        ShellPresentation.railSubtitle(isPad: horizontalSizeClass != .compact)
    }

    var body: some View {
        VStack(alignment: collapsed ? .center : .leading, spacing: 0) {
            header
            if !collapsed {
                search
                    .padding(.top, 4)
                    .padding(.bottom, 8)
            }
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: collapsed ? .center : .leading, spacing: 0) {
                    if !collapsed {
                        groupLabel(String(localized: "Terminals"))
                    }
                    if terminalGroups.isEmpty {
                        if !collapsed {
                            Text("No running terminals")
                                .font(SeNocturneType.font(10))
                                .foregroundStyle(palette.inkTertiary)
                                .padding(.horizontal, 9)
                                .padding(.vertical, 8)
                        }
                    } else {
                        ForEach(terminalGroups) { group in
                            if !collapsed {
                                railGroup(group)
                            }
                            ForEach(group.rows) { row in
                                terminalNav(row)
                            }
                        }
                    }
                    if !collapsed {
                        groupLabel(String(localized: "Sections"))
                    }
                    nav(.terminals)
                    nav(.conversations)
                    nav(.tools)
                    if !collapsed {
                        groupLabel(String(localized: "Host"))
                    }
                    hostRow
                }
                .padding(.horizontal, collapsed ? 9 : 10)
                .padding(.top, collapsed ? 8 : 0)
            }
            Spacer(minLength: 0)
            footer
        }
        .frame(width: width)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(mode.railSurface)
        .overlay(alignment: .trailing) {
            Rectangle().fill(mode.hairline).frame(width: 1)
        }
        .ignoresSafeArea(edges: .leading)
        .animation(reduceMotion ? nil : SeNocturneMotion.panel, value: collapsed)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(String(localized: "Sidebar"))
    }

    private var header: some View {
        VStack(alignment: collapsed ? .center : .leading, spacing: 0) {
            HStack(spacing: 10) {
                SeRailLogo()
                if !collapsed {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("SeRemote")
                            .font(SeNocturneType.font(13.5, weight: .semibold))
                            .tracking(-0.2)
                            .foregroundStyle(palette.ink)
                        Text(subtitle)
                            .font(SeNocturneType.font(8.5, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                    }
                    Spacer(minLength: 0)
                    headerAction
                }
            }
            .padding(.horizontal, collapsed ? 0 : 14)
            .padding(.top, 20)
            .padding(.bottom, collapsed ? 14 : 12)
            if collapsed {
                collapseButton.padding(.top, 10)
            }
        }
    }

    /// Trailing header button: an X that closes the phone drawer, or the
    /// fold control for the iPad rail.
    private var headerAction: some View {
        Group {
            if let onClose {
                Button(action: onClose) {
                    headerActionLabel(icon: .close, label: String(localized: "Close"))
                }
            } else {
                collapseButton
            }
        }
        .buttonStyle(SePressable())
    }

    private var collapseButton: some View {
        Button {
            collapsed.toggle()
        } label: {
            headerActionLabel(
                icon: collapsed ? .chevron : .sidebar,
                label: collapsed ? String(localized: "Expand sidebar") : String(localized: "Collapse sidebar")
            )
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(collapsed ? String(localized: "Expand sidebar") : String(localized: "Collapse sidebar"))
    }

    private func headerActionLabel(icon: SeIcon, label: String) -> some View {
        SeIconView(icon: icon, size: 13, stroke: 1.6, color: palette.inkSecondary)
            .frame(width: 28, height: 28)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .contentShape(Rectangle())
            .frame(width: 44, height: 44)
            .accessibilityLabel(label)
    }

    private var search: some View {
        Button(action: onOpenCommand) {
            HStack(spacing: 8) {
                SeIconView(icon: .search, size: 14, stroke: 1.6, color: palette.inkTertiary)
                Text("Search sessions, files or commands")
                    .font(SeNocturneType.font(11.5))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
                Spacer(minLength: 0)
                Text("⌘")
                    .font(SeNocturneType.font(8, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .overlay {
                        RoundedRectangle(cornerRadius: 4)
                            .strokeBorder(mode.insetStroke, lineWidth: 1)
                    }
            }
            .padding(.horizontal, 10)
            .frame(height: 38)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .strokeBorder(mode.insetStroke, lineWidth: 1)
            }
            .padding(.horizontal, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
    }

    private func groupLabel(_ text: String) -> some View {
        SeEyebrow(text: text)
            .padding(.horizontal, 8)
            .padding(.top, 10)
            .padding(.bottom, 5)
    }

    /// A project heading inside the rail's terminal list.
    private func railGroup(_ group: TerminalLandingModel.Group) -> some View {
        HStack(spacing: 6) {
            Text(verbatim: group.name)
                .font(SeNocturneType.font(10, weight: .semibold))
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(1)
            Text(verbatim: String(format: "%02d", group.count))
                .font(SeNocturneType.font(8.5, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .padding(.top, 10)
        .padding(.bottom, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("\(group.name), \(group.count) terminals"))
    }

    /// One terminal row in the rail. Selecting it opens the workbench; the row
    /// is not a separate destination.
    @ViewBuilder
    private func terminalNav(_ row: TerminalLandingModel.Row) -> some View {
        let on = selectedTerminalID == row.id
        Button {
            onOpenTerminal?(row)
        } label: {
            HStack(spacing: collapsed ? 0 : 9) {
                SeIconView(icon: .terminal,
                           size: 14,
                           stroke: 1.6,
                           color: on ? palette.accent : palette.inkSecondary)
                if !collapsed {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: row.title)
                            .font(SeNocturneType.font(11.5))
                            .foregroundStyle(on ? palette.ink : palette.inkSecondary)
                            .lineLimit(1)
                        if row.hasAgent {
                            Text("Agent")
                                .font(SeNocturneType.font(8, mono: true))
                                .foregroundStyle(palette.live)
                        }
                    }
                    Spacer(minLength: 4)
                }
            }
            .padding(.horizontal, collapsed ? 0 : 8)
            .frame(maxWidth: collapsed ? nil : .infinity, alignment: collapsed ? .center : .leading)
            .frame(width: collapsed ? 44 : nil, height: 36)
            .background(on ? palette.accentSubtle : Color.clear,
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(SePressable())
        .disabled(onOpenTerminal == nil)
        .accessibilityLabel(Text(row.title))
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func nav(_ destination: SidebarDestination) -> some View {
        let on = selection == destination
        return Button {
            selection = destination
        } label: {
            HStack(spacing: collapsed ? 0 : 10) {
                SeIconView(icon: destination.icon,
                           size: 17,
                           stroke: 1.6,
                           color: on ? palette.accent : palette.inkSecondary)
                if !collapsed {
                    Text(destination.title)
                        .font(SeNocturneType.font(12.5))
                        .foregroundStyle(on ? palette.ink : palette.inkSecondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if let count = counts[destination], !count.isEmpty {
                        Text(count)
                            .font(SeNocturneType.font(9, mono: true))
                            .foregroundStyle(destination == .conversations ? palette.accent : palette.inkTertiary)
                    }
                }
            }
            .padding(.horizontal, collapsed ? 0 : 9)
            .frame(maxWidth: collapsed ? nil : .infinity, alignment: collapsed ? .center : .leading)
            .frame(width: collapsed ? 44 : nil, height: 38)
            .background(on ? palette.accentSubtle : Color.clear,
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(destination.title)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// Expanded: live dot + host name. Collapsed (iPad portrait): a 44×34
    /// chip with the host's initial, no status dot — prototype `index.html:726-728`.
    private var hostRow: some View {
        HStack(spacing: collapsed ? 0 : 8) {
            if collapsed {
                Text(ShellPresentation.hostInitial(hostName))
                    .font(SeNocturneType.font(12.5, weight: .bold))
                    .foregroundStyle(palette.ink)
            } else {
                SeNeedDot(kind: hostOnline ? .live : .off, size: 5)
                Text(hostName)
                    .font(SeNocturneType.font(12.5))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
        }
        .padding(.horizontal, collapsed ? 0 : 9)
        .padding(.vertical, collapsed ? 0 : 9)
        .frame(width: collapsed ? 44 : nil, height: collapsed ? 34 : nil)
        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .padding(.top, collapsed ? 6 : 2)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(hostName)
        .accessibilityValue(hostOnline ? String(localized: "Connected") : String(localized: "Offline"))
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 0) {
            if collapsed {
                Button {
                    selection = .settings
                } label: {
                    SeIconView(icon: .gear,
                               size: 17,
                               stroke: 1.6,
                               color: selection == .settings ? palette.accent : palette.inkSecondary)
                        .frame(width: 44, height: 38)
                        .background(selection == .settings ? palette.accentSubtle : Color.clear,
                                    in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(String(localized: "Settings"))
            } else {
                nav(.settings)
                Text(version)
                    .font(SeNocturneType.font(8, mono: true))
                    .foregroundStyle(palette.inkTertiary.opacity(0.75))
                    .padding(.horizontal, 9)
                    .padding(.top, 8)
            }
        }
        .padding(.horizontal, collapsed ? 9 : 10)
        .padding(.top, collapsed ? 8 : 10)
        .padding(.bottom, 14)
        .frame(maxWidth: .infinity, alignment: collapsed ? .center : .leading)
        .overlay(alignment: .top) {
            Rectangle().fill(mode.hairline).frame(height: 1)
        }
    }
}

struct SeRailLogo: View {
    @Environment(\.sePalette) private var palette

    var body: some View {
        Text("S")
            .font(SeNocturneType.font(14, weight: .bold))
            .foregroundStyle(palette.canvas)
            .frame(width: 34, height: 34)
            .background(palette.ink, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .accessibilityHidden(true)
    }
}
