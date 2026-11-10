import SwiftUI

/// Real adapter for the 工具 tab. The shell owns the three primary destinations.
struct WorkspaceHubScreen: View {
    let session: WorkspaceSession
    let onOpenSession: () -> Void
    let onOpenCommand: () -> Void
    var onToggleTheme: () -> Void = {}
    var onOpenDrawer: () -> Void = {}
    var onSelectHost: () -> Void = {}
    var onOpenSettings: () -> Void = {}
    /// The shell wires this to T1's project grouping. When it is absent (the
    /// old call sites, previews) the hub falls back to selecting the project.
    var onOpenProject: ((String) -> Void)?

    @Environment(ConnectionStore.self) private var connections
    @Environment(\.sePalette) private var palette
    @State private var path: [ToolDestination] = []
    @State private var selectionTask: Task<Void, Never>?
    @State private var selectingProject = false

    private var scope: ToolScope { ToolScopePresentation.scope(for: session) }

    private var scopedProject: HostProject? {
        guard let id = scope.projectID else { return nil }
        return session.projects.projects.first(where: { $0.id == id })
    }

    private var snapshot: NocturneWorkspaceSnapshot {
        let git = session.git
        let gitReady = git.state == .loaded || git.state == .empty
        let tasks = session.scheduledTasks
        let tasksReady = (tasks.state == .loaded || tasks.state == .empty) && tasks.projectId == nil
        return NocturneWorkspaceSnapshot(
            gitCwd: gitReady ? git.cwd : nil,
            gitBranch: gitReady ? git.context?.branch : nil,
            gitChangedFileCount: gitReady ? Set(git.status.map(\.path)).count : nil,
            scheduledTaskCount: tasksReady ? tasks.tasks.count : nil,
            liveAgentSessionCounts: session.liveAgents.countsByProject)
    }

    private var hostName: String {
        if let link = connections.activeLink, link.originURL == session.origin { return link.title }
        return session.origin.host() ?? String(localized: "Host")
    }

    private var branch: String {
        guard let project = scopedProject else { return "" }
        return NocturneWorkspacePresentation.project(project, snapshot: snapshot).branch
    }

    var body: some View {
        NavigationStack(path: $path) {
            WorkspaceHubView(
                host: hostName,
                latency: "", // The host protocol does not report latency.
                project: scope.projectLabel,
                branch: branch,
                tools: ToolDestination.hubCases.map { NocturneWorkspacePresentation.tool($0, snapshot: snapshot) },
                projects: session.projects.projects.map { NocturneWorkspacePresentation.project($0, snapshot: snapshot) },
                toolScope: scope,
                onOpenTool: { tool in
                    guard scope.isEnabled, let destination = ToolDestination(rawValue: tool.id) else { return }
                    path.append(destination)
                },
                onOpenProject: openProject,
                onSelectHost: onSelectHost,
                onOpenCommand: onOpenCommand,
                onOpenDrawer: onOpenDrawer,
                onToggleTheme: onToggleTheme,
                onOpenSettings: onOpenSettings
            )
            .disabled(selectingProject)
            .overlay { if selectingProject { ProgressView().accessibilityLabel("Opening project") } }
            .navigationDestination(for: ToolDestination.self) { destination in
                // Route directly so each tool keeps the prototype's custom nav.
                // WorkspaceToolDestinationView forces the system bar visible,
                // which would draw a second bar above the Nocturne header.
                toolDestination(destination)
            }
            .toolbar(.hidden, for: .navigationBar)
            .task(id: scope.cwd) {
                // These stores retain their own results and cancel stale requests.
                await session.projects.refresh()
                guard !Task.isCancelled else { return }
                await session.liveAgents.refresh()
                guard !Task.isCancelled else { return }
                // Git is read-only and scoped to the terminal's own directory.
                if let cwd = scope.scopeDetail { await session.git.refresh(cwd: cwd) }
                guard !Task.isCancelled else { return }
                await session.scheduledTasks.refresh()
            }
            .onDisappear {
                selectionTask?.cancel()
                selectionTask = nil
                selectingProject = false
            }
        }
        .tint(palette.ink)
    }

    @ViewBuilder
    private func toolDestination(_ destination: ToolDestination) -> some View {
        switch destination {
        case .git:
            GitFeatureView(session: session)
        case .scheduledTasks:
            ScheduledTasksFeatureView(session: session)
        case .search:
            SearchFeatureView(session: session, onOpenFile: {
                path.removeAll()
                onOpenSession()
            })
        case .screenText:
            ScreenFeatureView(session: session)
        }
    }

    private func openProject(_ row: WorkspaceProject) {
        if let onOpenProject {
            onOpenProject(row.id)
            return
        }
        // Legacy fallback: the old shell has no landing grouping to route to.
        guard !selectingProject,
              let project = session.projects.projects.first(where: { $0.id == row.id }) else { return }
        selectingProject = true
        selectionTask = Task { @MainActor in
            defer { selectingProject = false; selectionTask = nil }
            await session.selectProject(project)
            guard !Task.isCancelled else { return }
            onOpenSession()
        }
    }
}
