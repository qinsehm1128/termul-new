import SwiftUI

/// One scheduled task in the prototype's detail language. Every action is a
/// real host request; the screen never claims a run succeeded before the host
/// reports it, and Run now plays no success haptic.
struct ScheduledTaskDetailScreen: View {
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession
    let taskId: String

    @State private var busy = false

    private var store: ScheduledTaskStore { session.scheduledTasks }

    private var task: ScheduledTaskWire? {
        store.tasks.first(where: { $0.taskId == taskId })
            ?? (store.selectedTask?.taskId == taskId ? store.selectedTask : nil)
    }

    var body: some View {
        NocturneToolScreen(
            title: task?.name ?? String(localized: "Scheduled task"),
            subtitle: task.map { ScheduledTaskScheduleFormatter.summary($0.schedule) } ?? "",
            onRefresh: refresh
        ) {
            content
                .task { await store.loadDetails(taskId: taskId) }
        }
    }

    @ViewBuilder
    private var content: some View {
        if let error = actionError {
            NocturneNotice(message: error, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 16)
        }

        if store.detailState == .loading && task == nil {
            HStack(spacing: 8) {
                ProgressView()
                Text("Loading from the host…")
                    .font(SeNocturneType.font(11, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
            .frame(minHeight: NocturneToolMetrics.minimumTarget)
        } else if let task {
            statusCard(task)
            actions(task)

            NocturneGroupLabel(title: String(localized: "Schedule"), detail: "")
            NocturneCard {
                VStack(alignment: .leading, spacing: 8) {
                    Text(verbatim: ScheduledTaskScheduleFormatter.summary(task.schedule))
                        .font(SeNocturneType.font(11, weight: .regular, mono: true))
                        .foregroundStyle(palette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    if let next = task.nextRunAt?.nocturneNonEmpty {
                        Text("Next run · \(ScheduledTaskScheduleFormatter.timestamp(next))")
                            .font(SeNocturneType.font(10, weight: .regular))
                            .foregroundStyle(palette.inkSecondary)
                    }
                    if !task.description.isEmpty {
                        Text(verbatim: task.description)
                            .font(SeNocturneType.font(10.5, weight: .regular))
                            .foregroundStyle(palette.inkSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)

            if !task.prompt.isEmpty {
                NocturneGroupLabel(title: String(localized: "Prompt"), detail: "")
                NocturneCard {
                    Text(verbatim: task.prompt)
                        .font(SeNocturneType.font(10.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
            }

            if !task.executionCwd.isEmpty || !task.workspaceCwd.isEmpty {
                NocturneGroupLabel(title: String(localized: "Working folder"), detail: "")
                NocturneCard {
                    VStack(alignment: .leading, spacing: 5) {
                        if !task.executionCwd.isEmpty {
                            Text(verbatim: task.executionCwd)
                                .font(SeNocturneType.font(10, weight: .regular, mono: true))
                                .foregroundStyle(palette.ink)
                                .textSelection(.enabled)
                        }
                        if !task.workspaceCwd.isEmpty, task.workspaceCwd != task.executionCwd {
                            Text(verbatim: task.workspaceCwd)
                                .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                                .textSelection(.enabled)
                        }
                    }
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
            }

            runsSection
            auditSection(task)
        } else {
            NocturneEmptyState(
                systemImage: "clock",
                title: String(localized: "Task unavailable"),
                message: String(localized: "The host did not return this task. Pull to refresh.")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
        }
    }

    // MARK: Status

    private func statusCard(_ task: ScheduledTaskWire) -> some View {
        NocturneCard {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Status")
                        .font(SeNocturneType.font(10, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                    Text(verbatim: statusWord(task.status))
                        .font(SeNocturneType.font(19, weight: .semibold))
                        .foregroundStyle(palette.ink)
                }
                Spacer(minLength: 8)
                NocturnePill(text: statusWord(task.status), tone: statusTone(task.status))
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 18)
    }

    // MARK: Actions

    private func actions(_ task: ScheduledTaskWire) -> some View {
        HStack(spacing: 8) {
            if task.status == .active || task.status == .paused {
                NocturneRunButton(title: String(localized: "Run now"), systemImage: "play.fill") {
                    Task { await store.run(taskId: task.taskId) }
                }
                .disabled(isWorking || !(deviceLink?.allows("agent.run") ?? true))
            }
            if task.status == .active {
                NocturneRunButton(title: String(localized: "Pause"), systemImage: "pause.fill") {
                    Task { await store.pause(taskId: task.taskId, expectedRevision: task.revision) }
                }
                .disabled(isWorking || !(deviceLink?.allows("agent.run") ?? true))
                .accessibilityHint(String(localized: "Keeps run history."))
            } else if task.status == .paused {
                NocturneRunButton(title: String(localized: "Resume"), systemImage: "play.circle") {
                    Task { await store.resume(taskId: task.taskId, expectedRevision: task.revision) }
                }
                .disabled(isWorking || !(deviceLink?.allows("agent.run") ?? true))
                .accessibilityHint(String(localized: "Continues from the next scheduled occurrence."))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 12)
    }

    // MARK: Runs

    @ViewBuilder
    private var runsSection: some View {
        NocturneGroupLabel(
            title: String(localized: "Recent runs"),
            detail: store.runs.isEmpty ? "" : String(format: "%02d", min(store.runs.count, 20))
        )
        VStack(spacing: NocturneToolMetrics.cardGap) {
            if store.runs.isEmpty {
                NocturneNotice(message: String(localized: "The host reports no runs for this task."))
            } else {
                ForEach(store.runs.prefix(20)) { run in
                    runRow(run)
                }
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
    }

    private func runRow(_ run: ScheduledTaskRunWire) -> some View {
        NocturneCard {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    NocturnePill(text: runStatusWord(run.status), tone: runStatusTone(run.status))
                    Text(verbatim: triggerWord(run.trigger))
                        .font(SeNocturneType.font(9, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                    Spacer(minLength: 8)
                    Text(verbatim: ScheduledTaskScheduleFormatter.timestamp(run.finishedAt ?? run.scheduledFor))
                        .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                if let summary = run.summary?.nocturneNonEmpty {
                    Text(verbatim: summary)
                        .font(SeNocturneType.font(10.5, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let error = (run.errorDetail ?? run.errorCode)?.nocturneNonEmpty {
                    Text(verbatim: error)
                        .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.danger)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    // MARK: Audit

    private func auditSection(_ task: ScheduledTaskWire) -> some View {
        Group {
            NocturneGroupLabel(
                title: String(localized: "Audit"),
                detail: store.auditEvents.isEmpty ? "" : String(format: "%02d", store.auditEvents.count)
            )
            VStack(spacing: NocturneToolMetrics.cardGap) {
                if store.auditEvents.isEmpty {
                    NocturneNotice(message: String(localized: "The host reports no audit events for this task."))
                } else {
                    ForEach(store.auditEvents.prefix(20)) { event in
                        HStack(spacing: 10) {
                            Text(verbatim: event.action)
                                .font(SeNocturneType.font(10.5, weight: .medium))
                                .foregroundStyle(palette.ink)
                            Spacer(minLength: 8)
                            Text(verbatim: ScheduledTaskScheduleFormatter.timestamp(event.createdAt))
                                .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 11)
                        .frame(minHeight: NocturneToolMetrics.minimumTarget)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 12))
                        .overlay {
                            RoundedRectangle(cornerRadius: 12)
                                .strokeBorder(palette.stroke, lineWidth: 1)
                        }
                    }
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
        }
    }

    // MARK: Labels

    private var actionError: String? {
        if case let .failed(message) = store.actionState { return message }
        return nil
    }

    private var isWorking: Bool { store.actionState == .loading }

    private func statusWord(_ status: ScheduledTaskStatusWire) -> String {
        switch status {
        case .draft: String(localized: "Draft")
        case .active: String(localized: "Enabled")
        case .paused: String(localized: "Paused")
        case .unknown(let value): value
        }
    }

    private func statusTone(_ status: ScheduledTaskStatusWire) -> NocturnePillTone {
        switch status {
        case .active: .live
        case .paused: .wait
        case .draft: .off
        case .unknown: .off
        }
    }

    private func runStatusWord(_ status: ScheduledTaskRunStatusWire) -> String {
        switch status {
        case .queued: String(localized: "Queued")
        case .cancelled: String(localized: "Cancelled")
        case .running: String(localized: "Running")
        case .succeeded: String(localized: "Succeeded")
        case .failed: String(localized: "Failed")
        case .skipped: String(localized: "Skipped")
        case .interrupted: String(localized: "Interrupted")
        case .unknown(let value): value
        }
    }

    private func runStatusTone(_ status: ScheduledTaskRunStatusWire) -> NocturnePillTone {
        switch status {
        case .succeeded: .live
        case .failed, .interrupted: .wait
        case .running, .queued: .blue
        default: .off
        }
    }

    private func triggerWord(_ trigger: ScheduledTaskTriggerWire) -> String {
        switch trigger {
        case .scheduled: String(localized: "scheduled")
        case .manual: String(localized: "manual")
        case .catchUp: String(localized: "catch-up")
        case .retry: String(localized: "retry")
        case .unknown(let value): value
        }
    }

    // MARK: Actions

    private func refresh() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        await store.loadDetails(taskId: taskId)
    }
}
