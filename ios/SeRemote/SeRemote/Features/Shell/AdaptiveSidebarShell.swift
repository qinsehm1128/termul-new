import SwiftUI

/// The three places the workbench exposes. Feature destinations still live in
/// the workspace hub; the rail keeps the prototype's two primary entries.
enum SidebarDestination: String, CaseIterable, Identifiable, Sendable {
    case live
    case workspace
    case files
    case changes
    case schedules
    case quick
    case settings

    var id: String { rawValue }

    var title: String {
        switch self {
        case .live: "现场"
        case .workspace: "工作区"
        case .files: "文件"
        case .changes: "改动"
        case .schedules: "定时"
        case .quick: "速启"
        case .settings: "设置"
        }
    }

    var icon: SeIcon {
        switch self {
        case .live: .hosts
        case .workspace: .folder
        case .files: .file
        case .changes: .branch
        case .schedules: .clock
        case .quick: .terminal
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
    var expandedWidth: CGFloat = ShellPresentation.railExpandedWidth

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var width: CGFloat {
        collapsed ? ShellPresentation.railCollapsedWidth : expandedWidth
    }

    var body: some View {
        VStack(alignment: collapsed ? .center : .leading, spacing: 0) {
            header
            if !collapsed {
                search
                    .padding(.top, 4)
            }
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: collapsed ? .center : .leading, spacing: 0) {
                    if !collapsed {
                        groupLabel("工作")
                    }
                    nav(.live)
                    nav(.workspace)
                    if !collapsed {
                        groupLabel("主机")
                        hostRow
                        groupLabel("工具")
                    }
                    nav(.files)
                    nav(.changes)
                    nav(.schedules)
                    nav(.quick)
                }
                .padding(.horizontal, collapsed ? 9 : 10)
                .padding(.top, collapsed ? 8 : 4)
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
        .accessibilityLabel("侧栏")
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
                        Text("iPad · Nocturne")
                            .font(SeNocturneType.font(8.5, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                    }
                    Spacer(minLength: 0)
                    collapseButton
                }
            }
            .padding(.horizontal, collapsed ? 0 : 14)
            .padding(.top, 20)
            .padding(.bottom, 12)
            if collapsed {
                collapseButton.padding(.top, 10)
            }
        }
    }

    private var collapseButton: some View {
        Button {
            collapsed.toggle()
        } label: {
            SeIconView(icon: collapsed ? .chevron : .sidebar,
                       size: 13,
                       stroke: 1.6,
                       color: palette.inkSecondary)
                .frame(width: 28, height: 28)
                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .contentShape(Rectangle())
                .frame(width: 44, height: 44)
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(collapsed ? "展开侧栏" : "收起侧栏")
    }

    private var search: some View {
        Button(action: onOpenCommand) {
            HStack(spacing: 8) {
                SeIconView(icon: .search, size: 14, stroke: 1.6, color: palette.inkTertiary)
                Text("搜索会话、文件或命令")
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
                            .foregroundStyle(destination == .live ? palette.accent : palette.inkTertiary)
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

    private var hostRow: some View {
        HStack(spacing: 8) {
            SeNeedDot(kind: hostOnline ? .live : .off, size: 5)
            Text(hostName)
                .font(SeNocturneType.font(12.5))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 8)
        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .padding(.top, 2)
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
                .accessibilityLabel("设置")
            } else {
                nav(.settings)
                Text(version)
                    .font(SeNocturneType.font(8, mono: true))
                    .foregroundStyle(palette.inkTertiary.opacity(0.75))
                    .padding(.horizontal, 9)
                    .padding(.top, 8)
                    .padding(.bottom, 4)
            }
        }
        .padding(.horizontal, collapsed ? 9 : 10)
        .padding(.top, 10)
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
            .font(SeNocturneType.font(15, weight: .bold))
            .foregroundStyle(palette.canvas)
            .frame(width: 34, height: 34)
            .background(palette.ink, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .accessibilityHidden(true)
    }
}
