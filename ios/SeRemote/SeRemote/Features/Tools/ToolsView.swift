import SwiftUI

/// Entry point for the secondary host tools: Git, scheduled tasks, quick
/// terminals, content search and host screen text. Chat, Terminal and Files
/// stay the three primary destinations; Tools opens from the toolbar.
///
/// Placeholder owned by the foundation lane. The feature-screen lane replaces
/// the body with the real destinations and keeps this initializer.
struct ToolsView: View {
    let session: WorkspaceSession
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(ToolDestination.allCases) { tool in
                    Label(tool.title, systemImage: tool.systemImage)
                        .frame(minHeight: SeSpace.hitTarget)
                }
            }
            .navigationTitle(String(localized: "Tools"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(String(localized: "Done")) { dismiss() }
                }
            }
        }
    }
}

enum ToolDestination: String, CaseIterable, Identifiable, Hashable {
    case git
    case scheduledTasks
    case quickTerminals
    case search
    case screenText

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

    var systemImage: String {
        switch self {
        case .git: "arrow.triangle.branch"
        case .scheduledTasks: "calendar.badge.clock"
        case .quickTerminals: "bolt.horizontal"
        case .search: "text.magnifyingglass"
        case .screenText: "text.viewfinder"
        }
    }
}
