import SwiftUI

struct ScheduledTaskDetailScreen: View {
    @Environment(\.sePalette) private var p
    let session: WorkspaceSession
    let taskId: String
    @State private var pendingAction: Action?
    @State private var errorTick = 0
    @State private var selectionTick = 0
    @State private var actionNotice: String?

    private enum Action { case run, pause, resume }
    private var store: ScheduledTaskStore { session.scheduledTasks }
    private var busy: Bool { pendingAction != nil || store.actionState == .loading }
    private var task: ScheduledTaskWire? {
        guard store.selectedTaskId == taskId, store.selectedTask?.id == taskId else { return nil }
        return store.selectedTask
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if case .failed(let message) = store.actionState {
                    ToolNocturneErrorBanner(message: message)
                }
                if let actionNotice {
                    Label(actionNotice, systemImage: "checkmark.circle")
                        .toolNocturneFont(.support).foregroundStyle(p.live)
                }
                if store.selectedTaskId == taskId || store.selectedTaskId == nil {
                    switch store.detailState {
                    case .idle, .loading:
                        ToolNocturneProgress(String(localized: "Loading task details…")).frame(maxWidth: .infinity, minHeight: 44)
                    case .failed(let message):
                        ToolNocturneErrorBanner(message: message) { Task { await reload() } }
                    case .empty:
                        ToolNocturneEmptyState(title: String(localized: "Task unavailable"), systemImage: "clock.badge.exclamationmark")
                    case .loaded: EmptyView()
                    }
                } else {
                    ToolNocturneProgress(String(localized: "Loading task details…"))
                }
                if let task {
                    overview(task)
                    controls(task)
                    prompt(task)
                    target(task)
                    policy(task)
                    runHistory
                    auditHistory
                }
            }
            .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .background(p.canvas)
        .tint(p.accent)
        .toolNocturneFont(.body)
        .foregroundStyle(p.ink)
        .navigationTitle(task?.name ?? String(localized: "Task details"))
        .refreshable { guard !busy else { return }; await reload() }
        .task(id: taskId) { await reload() }
        .seHaptic(.error, trigger: errorTick)
        .seHaptic(.selection, trigger: selectionTick)
    }

    private func overview(_ task: ScheduledTaskWire) -> some View {
        ToolNocturneCard {
            VStack(alignment: .leading, spacing: 12) {
                Text(task.name).toolNocturneFont(.sectionTitle)
                ScheduledTaskStatusLabel(taskStatus: task.status)
                if !task.description.isEmpty { Text(task.description).toolNocturneFont(.support) }
                Text(ScheduledTaskScheduleFormatter.summary(task.schedule)).toolNocturneFont(.support)
                field(String(localized: "Next run"), ScheduledTaskScheduleFormatter.timestamp(task.nextRunAt))
                field(String(localized: "Revision"), String(task.revision))
            }
        }
    }

    private func controls(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) { actionButtons(task) }
                VStack(alignment: .leading, spacing: 12) { actionButtons(task) }
            }
            if busy { ToolNocturneProgress(String(localized: "Waiting for host acknowledgement…")).toolNocturneFont(.support) }
        }
    }

    @ViewBuilder private func actionButtons(_ task: ScheduledTaskWire) -> some View {
        Button { perform(.run, task: task) } label: { Label("Run now", systemImage: "play.fill") }
            .buttonStyle(ToolNocturneButtonStyle(primary: true))
            .disabled(busy || store.detailState != .loaded)
        if task.status == .active {
            Button { perform(.pause, task: task) } label: { Label("Pause", systemImage: "pause") }
                .buttonStyle(ToolNocturneButtonStyle())
                .disabled(busy || store.detailState != .loaded)
        } else if task.status == .paused {
            Button { perform(.resume, task: task) } label: { Label("Resume", systemImage: "play") }
                .buttonStyle(ToolNocturneButtonStyle())
                .disabled(busy || store.detailState != .loaded)
        }
    }

    private func prompt(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ToolNocturneSectionHeader(title: String(localized: "Prompt"))
            ToolNocturneCard { Text(task.prompt).toolNocturneFont(.body).textSelection(.enabled) }
        }
    }

    private func target(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ToolNocturneSectionHeader(title: String(localized: "Execution target"))
            ToolNocturneCard {
                VStack(alignment: .leading, spacing: 12) {
                    switch task.executionTarget {
                    case .workspace: Text("Workspace").toolNocturneFont(.rowTitle)
                    case .projectRoot(let id, let root):
                        Text("Project root").toolNocturneFont(.rowTitle)
                        field(String(localized: "Project"), id)
                        field(String(localized: "Root"), root)
                    case .worktree(let id, let path, let branch):
                        Text("Worktree").toolNocturneFont(.rowTitle)
                        field(String(localized: "Project"), id)
                        field(String(localized: "Path"), path)
                        field(String(localized: "Branch"), branch)
                    case .unknown(let kind): Text(kind).toolNocturneFont(.support)
                    }
                    field(String(localized: "Working directory"), task.executionCwd)
                    field(String(localized: "Workspace"), task.workspaceCwd)
                    field(String(localized: "Agent configuration"), task.agentConfigId)
                }
            }
        }
    }

    private func policy(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ToolNocturneSectionHeader(title: String(localized: "Execution policy"))
            ToolNocturneCard {
                VStack(alignment: .leading, spacing: 12) {
                    field(String(localized: "Overlap"), task.executionPolicy.overlap.rawValue)
                    field(String(localized: "Catch up"), task.executionPolicy.catchUp.rawValue)
                    field(String(localized: "Catch-up window"), ScheduledTaskScheduleFormatter.duration(task.executionPolicy.catchUpWindowSeconds))
                    field(String(localized: "Permissions"), task.permissions.isEmpty
                        ? String(localized: "None") : task.permissions.joined(separator: ", "))
                }
            }
        }
    }

    private var runHistory: some View {
        VStack(alignment: .leading, spacing: 12) {
            ToolNocturneSectionHeader(title: String(localized: "Runs"), detail: "\(store.runs.count)")
            if store.runs.isEmpty { Text("No runs yet.").toolNocturneFont(.support).foregroundStyle(p.inkSecondary) }
            ForEach(store.runs) { run in
                ToolNocturneCard {
                    VStack(alignment: .leading, spacing: 12) {
                        ScheduledTaskStatusLabel(runStatus: run.status)
                        field(String(localized: "Trigger"), run.trigger.rawValue)
                        field(String(localized: "Scheduled for"), ScheduledTaskScheduleFormatter.timestamp(run.scheduledFor))
                        field(String(localized: "Queued"), ScheduledTaskScheduleFormatter.timestamp(run.queuedAt))
                        field(String(localized: "Started"), run.startedAt.map(ScheduledTaskScheduleFormatter.timestamp)
                            ?? String(localized: "Not started"))
                        field(String(localized: "Finished"), run.finishedAt.map(ScheduledTaskScheduleFormatter.timestamp)
                            ?? String(localized: "Not finished"))
                        Text(run.summary ?? String(localized: "No summary available.")).toolNocturneFont(.support).textSelection(.enabled)
                        if let errorCode = run.errorCode { field(String(localized: "Error code"), errorCode) }
                        if let errorDetail = run.errorDetail { ToolNocturneErrorBanner(message: errorDetail) }
                    }
                }
            }
        }
    }

    private var auditHistory: some View {
        VStack(alignment: .leading, spacing: 12) {
            ToolNocturneSectionHeader(title: String(localized: "Audit"), detail: "\(store.auditEvents.count)")
            if store.auditEvents.isEmpty { Text("No audit events.").toolNocturneFont(.support).foregroundStyle(p.inkSecondary) }
            ForEach(store.auditEvents) { event in
                ToolNocturneCard {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(event.action).toolNocturneFont(.rowTitle).textSelection(.enabled)
                        field(String(localized: "Actor"), event.actor.rawValue)
                        field(String(localized: "Time"), ScheduledTaskScheduleFormatter.timestamp(event.createdAt))
                        if let hash = event.beforeHash { field(String(localized: "Before"), hash) }
                        if let hash = event.afterHash { field(String(localized: "After"), hash) }
                    }
                }
            }
        }
    }

    private func field(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).toolNocturneFont(.label).foregroundStyle(p.inkSecondary)
            Text(value).toolNocturneFont(.support).textSelection(.enabled)
        }.accessibilityElement(children: .combine)
    }

    private func reload() async { await store.loadDetails(taskId: taskId) }

    private func perform(_ action: Action, task: ScheduledTaskWire) {
        guard !busy, task.id == taskId, store.detailState == .loaded else { return }
        // Capture the displayed current revision, never a list row's stale revision.
        pendingAction = action
        actionNotice = nil
        Task { @MainActor in
            defer { pendingAction = nil }
            switch action {
            case .run: await store.run(taskId: task.id)
            case .pause: await store.pause(taskId: task.id, expectedRevision: task.revision)
            case .resume: await store.resume(taskId: task.id, expectedRevision: task.revision)
            }
            switch store.actionState {
            case .loaded:
                if action == .run {
                    // The host only accepted the request; the run's outcome is
                    // not known yet. Report acceptance in text, not as success.
                    actionNotice = String(localized: "Host accepted the run request. Check run history for its outcome.")
                } else {
                    selectionTick += 1
                    actionNotice = String(localized: "Host acknowledged the status change.")
                }
            case .failed: errorTick += 1
            case .idle, .loading, .empty: break
            }
        }
    }
}
