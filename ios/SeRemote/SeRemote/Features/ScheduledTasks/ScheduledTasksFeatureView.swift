import SwiftUI

/// Read/manage existing host tasks. Creation and editing stay on the host.
struct ScheduledTasksFeatureView: View {
    @Environment(\.sePalette) private var p
    let session: WorkspaceSession
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicType
    @State private var selectedId: String?

    private var store: ScheduledTaskStore { session.scheduledTasks }
    private var projectId: String? { session.projects.active?.id }

    var body: some View {
        GeometryReader { geometry in
            let wide = SeLayout.columns(width: geometry.size.width,
                isRegular: sizeClass == .regular, largeText: dynamicType.isAccessibilitySize) != .single
            HStack(spacing: 0) {
                taskList(wide: wide)
                    .frame(maxWidth: wide ? SeLayout.sidebarWidth.upperBound : .infinity)
                if wide {
                    Divider()
                    if let selectedId {
                        ScheduledTaskDetailScreen(session: session, taskId: selectedId)
                            .id(selectedId)
                    } else {
                        ToolNocturneEmptyState(title: String(localized: "Select a task"), systemImage: "clock",
                            message: String(localized: "Inspect its prompt, policy and run history."))
                            .frame(maxWidth: .infinity)
                    }
                }
            }
        }
        .background(p.canvas)
        .tint(p.accent)
        .toolNocturneFont(.body)
        .foregroundStyle(p.ink)
        .navigationTitle("Scheduled tasks")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { Task { await refresh() } } label: { Label("Refresh", systemImage: "arrow.clockwise") }
                    .frame(minHeight: 44)
                    .disabled(store.state == .loading || store.actionState == .loading)
            }
        }
        .task(id: projectId) { selectedId = nil; await refresh() }
    }

    private func taskList(wide: Bool) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                ToolNocturneSectionHeader(title: String(localized: "Scheduled tasks"), detail: "\(store.tasks.count)")
                switch store.state {
                case .idle, .loading:
                    ToolNocturneProgress(String(localized: "Loading tasks…")).frame(maxWidth: .infinity, minHeight: 44)
                case .empty:
                    ToolNocturneEmptyState(title: String(localized: "No scheduled tasks"), systemImage: "clock",
                        message: String(localized: "Tasks created on the host will appear here."))
                case .failed(let message):
                    ToolNocturneErrorBanner(message: message) { Task { await refresh() } }
                case .loaded: EmptyView()
                }
                ForEach(store.tasks) { task in
                    if wide {
                        Button { selectedId = task.id } label: { ScheduledTaskRow(task: task) }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(selectedId == task.id ? [.isSelected] : [])
                    } else {
                        NavigationLink {
                            ScheduledTaskDetailScreen(session: session, taskId: task.id)
                        } label: { ScheduledTaskRow(task: task) }
                            .buttonStyle(.plain)
                    }
                }
            }.padding(20)
        }
        .refreshable { await refresh() }
    }

    private func refresh() async { await store.refresh(projectId: projectId) }
}

private struct ScheduledTaskRow: View {
    @Environment(\.sePalette) private var p
    let task: ScheduledTaskWire

    var body: some View {
        ToolNocturneCard {
            VStack(alignment: .leading, spacing: 12) {
                Text(task.name).toolNocturneFont(.rowTitle).foregroundStyle(p.ink)
                ScheduledTaskStatusLabel(taskStatus: task.status)
                Text(ScheduledTaskScheduleFormatter.summary(task.schedule))
                    .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                Text("Next run: \(ScheduledTaskScheduleFormatter.timestamp(task.nextRunAt))")
                    .toolNocturneFont(.timing).foregroundStyle(p.inkSecondary)
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}

/// Keeps exact wire status text (including future values) instead of calling an active task “Online”.
struct ScheduledTaskStatusLabel: View {
    @Environment(\.sePalette) private var p
    let title: String
    let status: SeStatus

    init(taskStatus: ScheduledTaskStatusWire) {
        title = taskStatus.rawValue
        status = switch taskStatus {
        case .active: .working
        case .paused: .paused
        case .draft: .idle
        case .unknown: .idle
        }
    }

    init(runStatus: ScheduledTaskRunStatusWire) {
        title = runStatus.rawValue
        status = switch runStatus {
        case .running: .working
        case .queued: .connecting
        case .succeeded: .completed
        case .failed, .interrupted: .failed
        case .cancelled, .skipped, .unknown: .idle
        }
    }

    var body: some View {
        ToolNocturneStatusBadge(status: status, title: title)
    }
}
