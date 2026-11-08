import SwiftUI

/// Tools retain the workspace scope; navigation never creates or closes a PTY.
struct ToolsView: View {
    var session: WorkspaceSession
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
                            Text("WORKSPACE SCOPE").font(SeFont.label).foregroundStyle(SeTheme.secondaryInk)
                            Text(session.workspaceRoot ?? String(localized: "No workspace selected"))
                                .font(SeFont.support.monospaced()).textSelection(.enabled)
                        }
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), alignment: .top),
                            count: columns == .single ? 1 : 2), spacing: SeSpace.md) {
                            ForEach(ToolDestination.allCases) { tool in
                                NavigationLink(value: tool) {
                                    HStack(alignment: .top, spacing: SeSpace.sm) {
                                        Image(systemName: tool.symbol).foregroundStyle(SeTheme.accent)
                                            .frame(width: 28)
                                        VStack(alignment: .leading, spacing: SeSpace.xs) {
                                            Text(tool.title).font(SeFont.rowTitle)
                                            Text(tool.detail).font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                                        }
                                        Spacer(minLength: 0)
                                        Image(systemName: "chevron.right").foregroundStyle(SeTheme.tertiaryInk)
                                    }
                                    .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
                                    .padding(SeSpace.md).background(SeTheme.surface, in: RoundedRectangle(cornerRadius: SeRadius.content))
                                }.buttonStyle(.plain)
                            }
                        }
                    }.padding(SeSpace.screenInset)
                }.background(SeTheme.canvas)
            }
            .navigationTitle("Tools")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() }.frame(minHeight: SeSpace.hitTarget) } }
            .navigationDestination(for: ToolDestination.self) { destination in
                switch destination {
                case .git: GitFeatureView(session: session)
                case .scheduledTasks: ScheduledTasksFeatureView(session: session)
                case .quickTerminals: QuickTerminalsFeatureView(session: session)
                case .search: SearchFeatureView(session: session, onOpenFile: { dismiss() })
                case .screenText: ScreenTextFeatureView(session: session)
                }
            }
        }.tint(SeTheme.accent)
    }
}

enum ToolDestination: String, CaseIterable, Identifiable, Hashable {
    case git, scheduledTasks, quickTerminals, search, screenText
    var id: String { rawValue }
    var title: String {
        switch self {
        case .git: String(localized: "Git")
        case .scheduledTasks: String(localized: "Scheduled tasks")
        case .quickTerminals: String(localized: "Quick terminals")
        case .search: String(localized: "Search")
        case .screenText: String(localized: "Host screen text")
        }
    }
    var systemImage: String { symbol }
    var symbol: String {
        switch self {
        case .git: "point.3.connected.trianglepath.dotted"
        case .scheduledTasks: "clock"
        case .quickTerminals: "terminal"
        case .search: "magnifyingglass"
        case .screenText: "text.viewfinder"
        }
    }
    var detail: String {
        switch self {
        case .git: String(localized: "Read-only changes, diffs and history.")
        case .scheduledTasks: String(localized: "Review schedules, run history and task controls.")
        case .quickTerminals: String(localized: "Manage persistent host terminal shortcuts.")
        case .search: String(localized: "Find text in the current workspace.")
        case .screenText: String(localized: "Read a terminal text snapshot, not live screen sharing.")
        }
    }
}
