import SwiftUI

/// Tools retain the terminal scope; navigation never creates or closes a PTY.
struct ToolsView: View {
    var session: WorkspaceSession
    @Environment(\.sePalette) private var palette
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.dismiss) private var dismiss
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicType
    @State private var pluginsCover = false

    private var scope: ToolScope { ToolScopePresentation.scope(for: session) }

    private func isEnabled(_ tool: ToolDestination) -> Bool {
        tool == .plugins ? tool.browserEntryEnabled(for: deviceLink) : scope.isEnabled
    }

    private func disabledReason(_ tool: ToolDestination) -> String {
        guard !isEnabled(tool) else { return "" }
        if tool == .plugins {
            return String(localized: "Plugins are not available for this connection.")
        }
        return scope.disabledReason
    }

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
                                Group {
                                    if tool == .plugins {
                                        Button { pluginsCover = true } label: { toolTile(tool) }
                                            .buttonStyle(.plain)
                                    } else {
                                        NavigationLink(value: tool) { toolTile(tool) }
                                            .buttonStyle(.plain)
                                    }
                                }
                                .disabled(!isEnabled(tool))
                                .opacity(isEnabled(tool) ? 1 : 0.55)
                                .accessibilityHint(disabledReason(tool))
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
            .fullScreenCover(isPresented: $pluginsCover) {
                PluginBrowserScreen(
                    origin: session.origin,
                    token: deviceLink?.pairingToken ?? session.credentials.bearer,
                    http: session.http
                )
                .background(palette.canvas.ignoresSafeArea())
            }
        }.tint(palette.ink)
    }

    private func toolTile(_ tool: ToolDestination) -> some View {
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
}

/// Shared route content used by both the hub stack and the standalone tool sheet.
struct WorkspaceToolDestinationView: View {
    let session: WorkspaceSession
    let destination: ToolDestination
    var onOpenFile: () -> Void = {}

    @Environment(\.remoteDeviceLink) private var deviceLink

    @ViewBuilder
    var body: some View {
        Group {
            switch destination {
            case .git: GitFeatureView(session: session)
            case .scheduledTasks: ScheduledTasksFeatureView(session: session)
            case .search: SearchFeatureView(session: session, onOpenFile: onOpenFile)
            case .screenText: ScreenFeatureView(session: session)
            case .plugins:
                PluginBrowserScreen(
                    origin: session.origin,
                    token: deviceLink?.pairingToken ?? session.credentials.bearer,
                    http: session.http
                )
            }
        }
        .toolbar(.visible, for: .navigationBar)
    }
}

enum ToolDestination: String, CaseIterable, Identifiable, Hashable {
    case git, scheduledTasks, search, screenText, plugins
    var id: String { rawValue }

    /// 工具 lists the prototype's four terminal-scoped tiles followed by the
    /// host-scoped 插件 entry:
    /// 更改 / 定时任务 / 搜索 / 屏幕 / 插件.
    ///
    /// Quick terminals live in 终端 and the desktop is reached from the 屏幕
    /// page and the terminal panel, so neither is a tool destination any more.
    /// Plugins is the one entry that does not follow a terminal: it runs the
    /// desktop's plugin host in the remote-access web app.
    static var hubCases: [ToolDestination] { allCases }

    var title: String {
        switch self {
        case .git: String(localized: "Git")
        case .scheduledTasks: String(localized: "Scheduled tasks")
        case .search: String(localized: "Search")
        case .screenText: String(localized: "Host screen text")
        case .plugins: String(localized: "Plugins")
        }
    }
    var icon: SeIcon {
        switch self {
        case .git: .git
        case .scheduledTasks: .clock
        case .search: .search
        case .screenText: .textview
        case .plugins: .plugins
        }
    }
    var detail: String {
        switch self {
        case .git: String(localized: "Read-only changes, diffs and history.")
        case .scheduledTasks: String(localized: "Schedules and run history.")
        case .search: String(localized: "Find text in this workspace.")
        case .screenText: String(localized: "Read a terminal text snapshot.")
        case .plugins: String(localized: "Desktop plugins")
        }
    }
}
