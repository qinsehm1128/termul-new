import SwiftUI

/// Tools retain the terminal scope; navigation never creates or closes a PTY.
struct ToolsView: View {
    var session: WorkspaceSession
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicType

    private var scope: ToolScope { ToolScopePresentation.scope(for: session) }

    var body: some View {
        NavigationStack {
            GeometryReader { geometry in
                let columns = SeLayout.columns(width: geometry.size.width,
                    isRegular: sizeClass == .regular, largeText: dynamicType.isAccessibilitySize)
                ScrollView {
                    VStack(alignment: .leading, spacing: SeSpace.section) {
                        HStack(alignment: .top, spacing: SeSpace.xs) {
                            VStack(alignment: .leading, spacing: SeSpace.xs) {
                                Text("Tool scope").font(SeNocturneType.monoSmall).foregroundStyle(palette.inkSecondary)
                                Text(verbatim: scope.projectLabel)
                                    .font(SeNocturneType.secondary.monospaced()).textSelection(.enabled)
                                if let detail = scope.scopeDetail {
                                    Text(verbatim: detail)
                                        .font(SeNocturneType.monoSmall)
                                        .foregroundStyle(palette.inkTertiary)
                                        .lineLimit(2)
                                        .truncationMode(.middle)
                                        .textSelection(.enabled)
                                }
                            }
                            Spacer(minLength: 0)
                            if !scope.isEnabled {
                                SeInfoTip(text: scope.disabledReason,
                                          accessibilityLabel: String(localized: "Tools unavailable"))
                            }
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
                                }
                                .buttonStyle(.plain)
                                .disabled(!scope.isEnabled)
                                .opacity(scope.isEnabled ? 1 : 0.55)
                                .accessibilityHint(scope.isEnabled ? "" : scope.disabledReason)
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
            case .search: SearchFeatureView(session: session, onOpenFile: onOpenFile)
            case .screenText: ScreenFeatureView(session: session)
            }
        }
        .toolbar(.visible, for: .navigationBar)
    }
}

enum ToolDestination: String, CaseIterable, Identifiable, Hashable {
    case git, scheduledTasks, search, screenText
    var id: String { rawValue }

    /// 工具 lists exactly the prototype's four tiles, in prototype order:
    /// 更改 / 定时任务 / 搜索 / 屏幕.
    ///
    /// Quick terminals live in 终端 and the desktop is reached from the 屏幕
    /// page and the terminal panel, so neither is a tool destination any more.
    static var hubCases: [ToolDestination] { allCases }

    var title: String {
        switch self {
        case .git: String(localized: "Git")
        case .scheduledTasks: String(localized: "Scheduled tasks")
        case .search: String(localized: "Search")
        case .screenText: String(localized: "Host screen text")
        }
    }
    var icon: SeIcon {
        switch self {
        case .git: .git
        case .scheduledTasks: .clock
        case .search: .search
        case .screenText: .textview
        }
    }
    var detail: String {
        switch self {
        case .git: String(localized: "Read-only changes, diffs and history.")
        case .scheduledTasks: String(localized: "Schedules and run history.")
        case .search: String(localized: "Find text in this workspace.")
        case .screenText: String(localized: "Read a terminal text snapshot.")
        }
    }
}
