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
                            Text("WORKSPACE SCOPE").font(SeNocturneType.monoSmall).foregroundStyle(palette.inkSecondary)
                            Text(session.workspaceRoot ?? String(localized: "No workspace selected"))
                                .font(SeNocturneType.secondary.monospaced()).textSelection(.enabled)
                        }
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), alignment: .top),
                            count: columns == .single ? 1 : 2), spacing: SeSpace.md) {
                            ForEach(ToolDestination.allCases) { tool in
                                NavigationLink(value: tool) {
                                    HStack(alignment: .top, spacing: SeSpace.sm) {
                                        Image(systemName: tool.symbol).foregroundStyle(palette.ink)
                                            .frame(width: 28)
                                        VStack(alignment: .leading, spacing: SeSpace.xs) {
                                            Text(tool.title).font(SeNocturneType.rowTitle)
                                            Text(tool.detail).font(SeNocturneType.secondary).foregroundStyle(palette.inkSecondary)
                                        }
                                        Spacer(minLength: 0)
                                        Image(systemName: "chevron.right").foregroundStyle(palette.inkTertiary)
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
            .navigationTitle("Tools")
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
            case .screenText: ScreenTextFeatureView(session: session)
            }
        }
        .toolbar(.visible, for: .navigationBar)
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
