import SwiftUI

/// Scheduled tasks in the prototype's list language. Run, pause and resume send
/// real host requests; every row reports the host outcome instead of optimistic
/// state. Run now deliberately plays no success haptic.
struct ScheduledTasksFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession

    @State private var filter: TaskFilter = .all
    @State private var busy = false

    private var store: ScheduledTaskStore { session.scheduledTasks }

    var body: some View {
        NocturneToolScreen(
            title: String(localized: "Scheduled tasks"),
            subtitle: navSubtitle,
            onRefresh: refresh
        ) {
            content
                .task { await refreshIfNeeded() }
        }
    }

    // MARK: Data

    private var hostName: String {
        session.origin.host() ?? String(localized: "Host")
    }

    private var tasksLoaded: Bool {
        store.state == .loaded || store.state == .empty
    }

    private var navSubtitle: String {
        guard tasksLoaded else { return hostName }
        return "\(hostName) · \(String(localized: "\(store.tasks.count) tasks"))"
    }

    private var activeCount: Int { store.tasks.filter { $0.status == .active }.count }
    private var pausedCount: Int { store.tasks.filter { $0.status == .paused }.count }

    private var visibleTasks: [ScheduledTaskWire] {
        switch filter {
        case .all: store.tasks
        case .active: store.tasks.filter { $0.status == .active }
        case .paused: store.tasks.filter { $0.status == .paused }
        }
    }

    private var actionError: String? {
        if case let .failed(message) = store.actionState { return message }
        return nil
    }

    private var loadError: String? {
        if case let .failed(message) = store.state { return message }
        return nil
    }

    // MARK: Body

    @ViewBuilder
    private var content: some View {
        NocturneSegmented(
            segments: [
                NocturneSegment(value: TaskFilter.all, title: String(localized: "All"), count: tasksLoaded ? store.tasks.count : nil),
                NocturneSegment(value: TaskFilter.active, title: String(localized: "Enabled"), count: tasksLoaded ? activeCount : nil),
                NocturneSegment(value: TaskFilter.paused, title: String(localized: "Paused"), count: tasksLoaded ? pausedCount : nil)
            ],
            selection: $filter
        )

        if let actionError {
            NocturneNotice(message: actionError, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 16)
        }

        if let loadError {
            NocturneNotice(message: loadError, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 16)
        }

        NocturneGroupLabel(
            title: dateLabel,
            detail: tasksLoaded ? String(format: "%02d", visibleTasks.count) : ""
        )

        VStack(spacing: NocturneToolMetrics.cardGap) {
            if store.state == .loading && store.tasks.isEmpty {
                loadingRow
            } else if visibleTasks.isEmpty {
                NocturneEmptyState(
                    systemImage: "clock",
                    title: String(localized: "No scheduled tasks")
                )
            } else {
                ForEach(visibleTasks) { task in
                    taskRow(task)
                }
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.bottom, 18)
    }

    private var loadingRow: some View {
        HStack(spacing: 8) {
            ProgressView()
            Text("Loading from the host…")
                .font(SeNocturneType.font(11, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
        }
        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
    }

    // MARK: Row

    private func taskRow(_ task: ScheduledTaskWire) -> some View {
        HStack(spacing: 11) {
            NavigationLink {
                ScheduledTaskDetailScreen(session: session, taskId: task.taskId)
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: task.name)
                        .font(SeNocturneType.font(12.5, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        Text(verbatim: ScheduledTaskScheduleFormatter.summary(task.schedule))
                            .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                        Text(verbatim: statusWord(task.status))
                            .font(SeNocturneType.font(8.5, weight: .regular, mono: true))
                            .foregroundStyle(statusTint(task.status))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if task.status == .active || task.status == .paused {
                NocturneRunButton(
                    title: String(localized: "Run"),
                    systemImage: "play.fill"
                ) {
                    Task { await store.run(taskId: task.taskId) }
                }
                .disabled(isWorking)
            }

            toggle(for: task)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func toggle(for task: ScheduledTaskWire) -> some View {
        let isOn = task.status == .active
        let canToggle = !isWorking && (task.status == .active || task.status == .paused)
        return Button {
            Task {
                switch task.status {
                case .active: await store.pause(taskId: task.taskId, expectedRevision: task.revision)
                case .paused: await store.resume(taskId: task.taskId, expectedRevision: task.revision)
                default: break
                }
            }
        } label: {
            Capsule()
                .fill(isOn ? palette.accent : palette.cardAlt)
                .frame(width: 38, height: 22)
                .overlay {
                    Capsule().strokeBorder(isOn ? palette.accent : palette.strokeStrong, lineWidth: 1)
                }
                .overlay(alignment: .leading) {
                    Circle()
                        .fill(isOn ? palette.onAccent : palette.inkTertiary)
                        .frame(width: 16, height: 16)
                        .padding(2)
                        .offset(x: isOn ? 16 : 0)
                }
                .frame(width: NocturneToolMetrics.minimumTarget, height: NocturneToolMetrics.minimumTarget)
                .contentShape(Rectangle())
                .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: isOn)
        }
        .buttonStyle(.plain)
        .disabled(!canToggle)
        .accessibilityLabel(isOn
                            ? String(localized: "Pause \(task.name)")
                            : String(localized: "Resume \(task.name)"))
        .accessibilityValue(isOn ? String(localized: "Enabled") : String(localized: "Paused"))
    }

    private var isWorking: Bool {
        store.actionState == .loading
    }

    // MARK: Labels

    private var dateLabel: String {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.setLocalizedDateFormatFromTemplate("EEEE")
        return "\(String(localized: "Today")) · \(formatter.string(from: .now))"
    }

    private func statusWord(_ status: ScheduledTaskStatusWire) -> String {
        switch status {
        case .draft: String(localized: "draft")
        case .active: String(localized: "enabled")
        case .paused: String(localized: "paused")
        case .unknown(let value): value
        }
    }

    private func statusTint(_ status: ScheduledTaskStatusWire) -> Color {
        switch status {
        case .active: palette.live
        case .paused: palette.warning
        default: palette.inkTertiary
        }
    }

    // MARK: Actions

    private func refreshIfNeeded() async {
        if store.state == .idle {
            await store.refresh()
        }
    }

    private func refresh() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        await store.refresh()
    }
}

enum TaskFilter: Hashable {
    case all, active, paused
}
