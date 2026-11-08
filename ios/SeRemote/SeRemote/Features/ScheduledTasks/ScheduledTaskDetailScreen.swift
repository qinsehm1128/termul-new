import SwiftUI

struct ScheduledTaskDetailScreen: View {
    let session: WorkspaceSession
    let taskId: String
    @State private var pendingAction: Action?
    @State private var successTick = 0
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
            VStack(alignment: .leading, spacing: SeSpace.section) {
                if case .failed(let message) = store.actionState {
                    SeErrorBanner(message: message)
                }
                if let actionNotice {
                    Label(actionNotice, systemImage: "checkmark.circle")
                        .font(SeFont.support).foregroundStyle(SeTheme.success)
                }
                if store.selectedTaskId == taskId || store.selectedTaskId == nil {
                    switch store.detailState {
                    case .idle, .loading:
                        ProgressView("Loading task details…").frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget)
                    case .failed(let message):
                        SeErrorBanner(message: message) { Task { await reload() } }
                    case .empty:
                        SeEmptyState(title: String(localized: "Task unavailable"), systemImage: "clock.badge.exclamationmark")
                    case .loaded: EmptyView()
                    }
                } else {
                    ProgressView("Loading task details…")
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
            .padding(SeSpace.screenInset)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        .background(SeTheme.canvas)
        .navigationTitle(task?.name ?? String(localized: "Task details"))
        .refreshable { guard !busy else { return }; await reload() }
        .task(id: taskId) { await reload() }
        .seHaptic(.success, trigger: successTick)
        .seHaptic(.error, trigger: errorTick)
        .seHaptic(.selection, trigger: selectionTick)
    }

    private func overview(_ task: ScheduledTaskWire) -> some View {
        SeCard {
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                Text(task.name).font(SeFont.sectionTitle)
                ScheduledTaskStatusLabel(taskStatus: task.status)
                if !task.description.isEmpty { Text(task.description).font(SeFont.support) }
                Text(ScheduledTaskScheduleFormatter.summary(task.schedule)).font(SeFont.support)
                field(String(localized: "Next run"), ScheduledTaskScheduleFormatter.timestamp(task.nextRunAt))
                field(String(localized: "Revision"), String(task.revision))
            }
        }
    }

    private func controls(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: SeSpace.sm) { actionButtons(task) }
                VStack(alignment: .leading, spacing: SeSpace.sm) { actionButtons(task) }
            }
            if busy { ProgressView("Waiting for host acknowledgement…").font(SeFont.support) }
        }
    }

    @ViewBuilder private func actionButtons(_ task: ScheduledTaskWire) -> some View {
        Button { perform(.run, task: task) } label: { Label("Run now", systemImage: "play.fill") }
            .buttonStyle(SePrimaryButtonStyle())
            .disabled(busy || store.detailState != .loaded)
        if task.status == .active {
            Button { perform(.pause, task: task) } label: { Label("Pause", systemImage: "pause") }
                .buttonStyle(SeSecondaryButtonStyle())
                .disabled(busy || store.detailState != .loaded)
        } else if task.status == .paused {
            Button { perform(.resume, task: task) } label: { Label("Resume", systemImage: "play") }
                .buttonStyle(SeSecondaryButtonStyle())
                .disabled(busy || store.detailState != .loaded)
        }
    }

    private func prompt(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            SeSectionHeader(title: String(localized: "Prompt"))
            SeCard { Text(task.prompt).font(SeFont.body).textSelection(.enabled) }
        }
    }

    private func target(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            SeSectionHeader(title: String(localized: "Execution target"))
            SeCard {
                VStack(alignment: .leading, spacing: SeSpace.sm) {
                    switch task.executionTarget {
                    case .workspace: Text("Workspace").font(SeFont.rowTitle)
                    case .projectRoot(let id, let root):
                        Text("Project root").font(SeFont.rowTitle)
                        field(String(localized: "Project"), id)
                        field(String(localized: "Root"), root)
                    case .worktree(let id, let path, let branch):
                        Text("Worktree").font(SeFont.rowTitle)
                        field(String(localized: "Project"), id)
                        field(String(localized: "Path"), path)
                        field(String(localized: "Branch"), branch)
                    case .unknown(let kind): Text(kind).font(SeFont.support)
                    }
                    field(String(localized: "Working directory"), task.executionCwd)
                    field(String(localized: "Workspace"), task.workspaceCwd)
                    field(String(localized: "Agent configuration"), task.agentConfigId)
                }
            }
        }
    }

    private func policy(_ task: ScheduledTaskWire) -> some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            SeSectionHeader(title: String(localized: "Execution policy"))
            SeCard {
                VStack(alignment: .leading, spacing: SeSpace.sm) {
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
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            SeSectionHeader(title: String(localized: "Runs"), detail: "\(store.runs.count)")
            if store.runs.isEmpty { Text("No runs yet.").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk) }
            ForEach(store.runs) { run in
                SeCard {
                    VStack(alignment: .leading, spacing: SeSpace.sm) {
                        ScheduledTaskStatusLabel(runStatus: run.status)
                        field(String(localized: "Trigger"), run.trigger.rawValue)
                        field(String(localized: "Scheduled for"), ScheduledTaskScheduleFormatter.timestamp(run.scheduledFor))
                        field(String(localized: "Queued"), ScheduledTaskScheduleFormatter.timestamp(run.queuedAt))
                        field(String(localized: "Started"), run.startedAt.map(ScheduledTaskScheduleFormatter.timestamp)
                            ?? String(localized: "Not started"))
                        field(String(localized: "Finished"), run.finishedAt.map(ScheduledTaskScheduleFormatter.timestamp)
                            ?? String(localized: "Not finished"))
                        Text(run.summary ?? String(localized: "No summary available.")).font(SeFont.support).textSelection(.enabled)
                        if let errorCode = run.errorCode { field(String(localized: "Error code"), errorCode) }
                        if let errorDetail = run.errorDetail { SeErrorBanner(message: errorDetail) }
                    }
                }
            }
        }
    }

    private var auditHistory: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            SeSectionHeader(title: String(localized: "Audit"), detail: "\(store.auditEvents.count)")
            if store.auditEvents.isEmpty { Text("No audit events.").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk) }
            ForEach(store.auditEvents) { event in
                SeCard {
                    VStack(alignment: .leading, spacing: SeSpace.sm) {
                        Text(event.action).font(SeFont.rowTitle).textSelection(.enabled)
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
        VStack(alignment: .leading, spacing: SeSpace.xxs) {
            Text(label).font(SeFont.label).foregroundStyle(SeTheme.secondaryInk)
            Text(value).font(SeFont.support).textSelection(.enabled)
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
                    successTick += 1
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
