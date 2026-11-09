import SwiftUI

// MARK: - Model

struct WorkspaceTool: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let detail: String
    let note: String
    let systemImage: String
    /// True only when the host actually reported the number in `detail`.
    var metricIsLoaded = false
}

struct WorkspaceProject: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let branch: String
    let detail: String
    let hasChanges: Bool
    var path: String = ""
}

enum WorkspaceSection: String, CaseIterable, Identifiable {
    case tools
    case projects

    var id: String { rawValue }

    var title: LocalizedStringKey {
        switch self {
        case .tools: "工具"
        case .projects: "项目"
        }
    }

    var label: String {
        switch self {
        case .tools: String(localized: "工具")
        case .projects: String(localized: "项目")
        }
    }
}

// MARK: - Screen

/// 工具 — the third primary destination. Tools are entry points; the section
/// below them lists the real projects the host reports as tool context.
struct WorkspaceHubView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var host: String
    var latency: String
    var project: String
    var branch: String
    var tools: [WorkspaceTool]
    var projects: [WorkspaceProject]

    var onOpenTool: (WorkspaceTool) -> Void = { _ in }
    var onOpenProject: (WorkspaceProject) -> Void = { _ in }
    var onSelectHost: () -> Void = {}
    var onOpenCommand: () -> Void = {}
    var onOpenDrawer: () -> Void = {}
    var onToggleTheme: () -> Void = {}
    var onOpenSettings: () -> Void = {}

    @State private var section: WorkspaceSection = .tools

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 8)
                title
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 10)
                sectionSwitch
                switch section {
                case .tools:
                    toolSection
                    projectSection
                case .projects:
                    projectSection
                }
                settingsSection
            }
            .padding(.bottom, 30)
        }
        .scrollIndicators(.hidden)
        .background(palette.canvas.ignoresSafeArea())
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            Button(action: onSelectHost) {
                HStack(spacing: 6) {
                    Text(verbatim: host)
                        .font(SeNocturneType.font(11, weight: .medium))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(palette.inkTertiary)
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 8))
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
                .frame(minHeight: NocturneToolMetrics.minimumTarget)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "Select host"))

            // The host protocol does not report latency or a connection count.
            // Both stay hidden instead of showing an invented value.
            Spacer(minLength: 8)

            Button(action: onOpenCommand) {
                Text("⌘")
                    .font(SeNocturneType.font(13, mono: true))
                    .foregroundStyle(palette.accent)
                    .frame(width: 34, height: 34)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                    .contentShape(Rectangle())
                    .frame(minWidth: NocturneToolMetrics.minimumTarget,
                           minHeight: NocturneToolMetrics.minimumTarget)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "命令面板"))

            NocturneIconButton(
                systemImage: "sidebar.left",
                label: String(localized: "All destinations"),
                action: onOpenDrawer
            )
        }
    }

    // MARK: Title

    private var title: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text("工具")
                .font(SeNocturneType.font(33, weight: .semibold))
                .tracking(-1.4)
                .foregroundStyle(palette.ink)
            Text(verbatim: lede)
                .font(SeNocturneType.font(12, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(2)
        }
    }

    /// Real scope only: project name, branch, and a changed-file count that the
    /// host actually reported for this workspace.
    private var lede: String {
        var parts: [String] = []
        if !project.isEmpty { parts.append(project) }
        if !branch.isEmpty { parts.append(branch) }
        if let git = tools.first(where: { $0.id == ToolDestination.git.rawValue }), git.metricIsLoaded {
            parts.append(git.detail)
        }
        return parts.joined(separator: " · ")
    }

    // MARK: Section switch

    private var sectionSwitch: some View {
        NocturneSegmented(
            segments: WorkspaceSection.allCases.map { NocturneSegment(value: $0, title: $0.label) },
            selection: $section
        )
    }

    // MARK: Tools

    private var toolSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            NocturneGroupLabel(title: String(localized: "工具"), detail: twoDigit(tools.count))
            LazyVGrid(
                columns: Array(repeating: GridItem(.flexible(), spacing: NocturneToolMetrics.cardGap),
                               count: 2),
                spacing: NocturneToolMetrics.cardGap
            ) {
                ForEach(tools) { tool in
                    toolTile(tool)
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
        }
    }

    private func toolTile(_ tool: WorkspaceTool) -> some View {
        Button {
            onOpenTool(tool)
        } label: {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: tool.systemImage)
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                    Text(verbatim: shortTitle(tool))
                        .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                Text(verbatim: tool.detail)
                    .font(SeNocturneType.font(12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(2)
                    .padding(.top, 16)
                Text(verbatim: tool.note)
                    .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(2)
                    .padding(.top, 3)
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(12)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 14))
            .overlay {
                RoundedRectangle(cornerRadius: 14)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(NocturnePressStyle(reduceMotion: reduceMotion))
        .accessibilityElement(children: .combine)
    }

    private func shortTitle(_ tool: WorkspaceTool) -> String {
        switch tool.id {
        case ToolDestination.git.rawValue: String(localized: "Changes")
        case ToolDestination.scheduledTasks.rawValue: String(localized: "Scheduled")
        case ToolDestination.quickTerminals.rawValue: String(localized: "Quick")
        case ToolDestination.search.rawValue: String(localized: "Search")
        case ToolDestination.screenText.rawValue: String(localized: "Screen")
        default: tool.title
        }
    }

    // MARK: Projects

    private var projectSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            NocturneGroupLabel(
                title: section == .tools ? String(localized: "最近项目") : String(localized: "项目"),
                detail: twoDigit(projects.count)
            )
            VStack(spacing: NocturneToolMetrics.cardGap) {
                if projects.isEmpty {
                    NocturneEmptyState(
                        systemImage: "folder",
                        title: String(localized: "No projects"),
                        message: String(localized: "The host has not reported a project for this workspace.")
                    )
                } else {
                    ForEach(projects) { project in
                        NocturneSessionCard(
                            title: project.name,
                            subtitle: NocturneWorkspacePresentation.rowText(branch: project.branch, detail: project.detail),
                            spine: project.hasChanges ? .live : .none,
                            trailingSystemImage: "arrow.right",
                            action: { onOpenProject(project) }
                        )
                    }
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
        }
    }

    // MARK: Settings

    private var settingsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            NocturneGroupLabel(title: String(localized: "设置"), detail: "")
            Button(action: onOpenSettings) {
                HStack(spacing: 10) {
                    Image(systemName: "gearshape")
                        .font(.system(size: 14, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                    Text("设置")
                        .font(SeNocturneType.font(12, weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Spacer(minLength: 0)
                    Image(systemName: "arrow.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(palette.inkTertiary)
                }
                .padding(.horizontal, 12)
                .frame(minHeight: 52)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 14))
                .overlay {
                    RoundedRectangle(cornerRadius: 14)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(NocturnePressStyle(reduceMotion: reduceMotion))
            .accessibilityLabel(String(localized: "设置"))
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, NocturneToolMetrics.cardGap)
        }
    }

    private func twoDigit(_ count: Int) -> String {
        String(format: "%02d", count)
    }
}

// MARK: - Previews

#Preview("Workspace · Night") {
    WorkspaceHubView(
        host: "MacBook Pro",
        latency: "",
        project: "Termul",
        branch: "main",
        tools: [
            WorkspaceTool(id: "git", title: "Git", detail: "2 changed files", note: "Read only", systemImage: "point.3.connected.trianglepath.dotted", metricIsLoaded: true),
            WorkspaceTool(id: "scheduledTasks", title: "Scheduled tasks", detail: "4 tasks", note: "Host schedules", systemImage: "clock", metricIsLoaded: true),
            WorkspaceTool(id: "quickTerminals", title: "Quick terminals", detail: "2 terminals", note: "Persistent host terminals", systemImage: "terminal", metricIsLoaded: true),
            WorkspaceTool(id: "search", title: "Search", detail: "Sessions & files", note: "Read only", systemImage: "magnifyingglass"),
            WorkspaceTool(id: "screenText", title: "Host screen text", detail: "Terminal snapshot", note: "Matching snapshot required", systemImage: "text.viewfinder")
        ],
        projects: [
            WorkspaceProject(id: "p1", name: "Termul", branch: "main", detail: "2 changed files", hasChanges: true),
            WorkspaceProject(id: "p2", name: "se-server", branch: "dev", detail: "0 live terminals", hasChanges: false)
        ]
    )
    .seThemeMode(.night)
}

#Preview("Workspace · Day") {
    WorkspaceHubView(
        host: "MacBook Pro",
        latency: "",
        project: "Termul",
        branch: "main",
        tools: [
            WorkspaceTool(id: "git", title: "Git", detail: "2 changed files", note: "Read only", systemImage: "point.3.connected.trianglepath.dotted", metricIsLoaded: true)
        ],
        projects: []
    )
    .seThemeMode(.day)
}
