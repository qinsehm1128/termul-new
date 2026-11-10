import SwiftUI

/// Tools retain the workspace scope; navigation never creates or closes a PTY.
struct ToolsView: View {
    var session: WorkspaceSession
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicType

    var body: some View {
        NavigationStack {
            GeometryReader { geometry in
                let columns = SeLayout.columns(width: geometry.size.width,
                    isRegular: sizeClass == .regular, largeText: dynamicType.isAccessibilitySize)
                ScrollView {
                    VStack(alignment: .leading, spacing: SeSpace.section) {
                        VStack(alignment: .leading, spacing: SeSpace.xs) {
                            Text("Tool scope").font(SeNocturneType.monoSmall).foregroundStyle(palette.inkSecondary)
                            Text(session.workspaceRoot ?? String(localized: "No project selected"))
                                .font(SeNocturneType.secondary.monospaced()).textSelection(.enabled)
                        }
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), alignment: .top),
                            count: columns == .single ? 1 : 2), spacing: SeSpace.md) {
                            ForEach(ToolDestination.hubCases) { tool in
                                NavigationLink(value: tool) {
                                    HStack(alignment: .top, spacing: SeSpace.sm) {
                                        SeIconView(icon: tool.icon, size: 16, stroke: 1.6, color: palette.ink)
                                            .frame(width: 28)
                                        VStack(alignment: .leading, spacing: SeSpace.xs) {
                                            Text(tool.title).font(SeNocturneType.rowTitle)
                                            Text(tool.detail).font(SeNocturneType.secondary).foregroundStyle(palette.inkSecondary)
                                        }
                                        Spacer(minLength: 0)
                                        SeIconView(icon: .chevron, size: 11, stroke: 1.6, color: palette.inkTertiary)
                                    }
                                    .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
                                    .padding(SeSpace.md)
                                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
                                    .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.card).stroke(palette.stroke, lineWidth: 1) }
                                }.buttonStyle(.plain)
                            }
                        }
                    }.padding(SeSpace.screenInset)
                }.background(palette.canvas)
            }
            .navigationTitle(String(localized: "Tools"))
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() }.frame(minHeight: SeSpace.hitTarget) } }
            .navigationDestination(for: ToolDestination.self) { destination in
                WorkspaceToolDestinationView(session: session, destination: destination, onOpenFile: { dismiss() })
            }
        }.tint(palette.ink)
    }
}

/// Shared route content used by both the hub stack and the standalone tool sheet.
struct WorkspaceToolDestinationView: View {
    let session: WorkspaceSession
    let destination: ToolDestination
    var onOpenFile: () -> Void = {}

    @ViewBuilder
    var body: some View {
        Group {
            switch destination {
            case .git: GitFeatureView(session: session)
            case .scheduledTasks: ScheduledTasksFeatureView(session: session)
            case .quickTerminals: QuickTerminalsFeatureView(session: session)
            case .search: SearchFeatureView(session: session, onOpenFile: onOpenFile)
            case .screenText: ScreenFeatureView(session: session)
            case .desktop: DesktopFeatureView(session: session)
            }
        }
        .toolbar(.visible, for: .navigationBar)
    }
}

enum ToolDestination: String, CaseIterable, Identifiable, Hashable {
    case git, scheduledTasks, quickTerminals, search, screenText, desktop
    var id: String { rawValue }

    /// 工具 lists exactly the prototype's four tiles, in prototype order:
    /// 更改 / 定时任务 / 搜索 / 屏幕.
    ///
    /// Quick terminals live in the 终端 tab, and the desktop is reached from the
    /// 屏幕 page's 桌面 segment and the ⌘ palette — both keep their destination
    /// in `allCases`, they just leave this grid.
    static var hubCases: [ToolDestination] { [.git, .scheduledTasks, .search, .screenText] }
    var title: String {
        switch self {
        case .git: String(localized: "Git")
        case .scheduledTasks: String(localized: "Scheduled tasks")
        case .quickTerminals: String(localized: "Quick terminals")
        case .search: String(localized: "Search")
        case .screenText: String(localized: "Host screen text")
        case .desktop: String(localized: "Desktop")
        }
    }
    var icon: SeIcon {
        switch self {
        case .git: .git
        case .scheduledTasks: .clock
        case .quickTerminals: .terminal
        case .search: .search
        case .screenText: .textview
        case .desktop: .desktop
        }
    }
    var detail: String {
        switch self {
        case .git: String(localized: "Read-only changes, diffs and history.")
        case .scheduledTasks: String(localized: "Schedules and run history.")
        case .quickTerminals: String(localized: "Host terminal shortcuts.")
        case .search: String(localized: "Find text in this workspace.")
        case .screenText: String(localized: "Read a terminal text snapshot.")
        case .desktop: String(localized: "View, capture and control your computer.")
        }
    }
}
