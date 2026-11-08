import Foundation
import Observation

/// Local to this feature: no dependency on another feature's state machine.
enum ScheduledTaskLoadState: Equatable {
    case idle, loading, loaded, empty, failed(String)
}

@Observable
@MainActor
final class ScheduledTaskStore {
    private(set) var tasks: [ScheduledTaskWire] = []
    private(set) var selectedTask: ScheduledTaskWire?
    private(set) var runs: [ScheduledTaskRunWire] = []
    private(set) var auditEvents: [ScheduledTaskAuditEventWire] = []
    private(set) var state: ScheduledTaskLoadState = .idle
    private(set) var detailState: ScheduledTaskLoadState = .idle
    private(set) var actionState: ScheduledTaskLoadState = .idle
    private(set) var projectId: String?

    @ObservationIgnored private var client: (any ScheduledTaskServing)?
    @ObservationIgnored private var attachment = UUID()
    @ObservationIgnored private var listRequest = UUID()
    @ObservationIgnored private var detailRequest = UUID()

    func attach(http: HostHTTP) {
        attach(client: ScheduledTaskClient(http: http))
    }

    /// Also provides a feature-scoped test seam without replacing the shared transport.
    func attach(client: any ScheduledTaskServing) {
        self.client = client
        attachment = UUID()
        listRequest = UUID()
        detailRequest = UUID()
        tasks = []
        selectedTask = nil
        runs = []
        auditEvents = []
        projectId = nil
        state = .idle
        detailState = .idle
        actionState = .idle
    }

    func refresh(projectId: String? = nil) async {
        guard let client else {
            state = .failed(String(localized: "Connect to a host to load scheduled tasks."))
            return
        }
        let request = UUID()
        let host = attachment
        listRequest = request
        self.projectId = projectId
        state = .loading
        do {
            let result = try await client.list(projectId: projectId)
            guard host == attachment, request == listRequest else { return }
            try Task.checkCancellation()
            tasks = result
            state = result.isEmpty ? .empty : .loaded
        } catch {
            guard host == attachment, request == listRequest else { return }
            if Task.isCancelled || error is CancellationError {
                state = tasks.isEmpty ? .idle : .loaded
            } else {
                state = .failed(error.localizedDescription)
                HostLog.session.error("Scheduled task list refresh failed")
            }
        }
    }

    func loadDetails(taskId: String) async {
        guard let client else {
            detailState = .failed(String(localized: "Connect to a host to load scheduled tasks."))
            return
        }
        let request = UUID()
        let host = attachment
        detailRequest = request
        selectedTask = nil
        runs = []
        auditEvents = []
        detailState = .loading
        do {
            let task = try await client.get(taskId: taskId)
            let history = try await client.runs(taskId: taskId)
            let events = try await client.audit(taskId: taskId)
            guard host == attachment, request == detailRequest else { return }
            try Task.checkCancellation()
            selectedTask = task
            runs = history
            auditEvents = events
            detailState = .loaded
        } catch {
            guard host == attachment, request == detailRequest else { return }
            if Task.isCancelled || error is CancellationError {
                detailState = .idle
            } else {
                detailState = .failed(error.localizedDescription)
                HostLog.session.error("Scheduled task detail refresh failed")
            }
        }
    }

    func run(taskId: String) async {
        await performAction { client in
            _ = try await client.run(taskId: taskId)
            return nil
        }
    }

    func pause(taskId: String, expectedRevision: UInt64) async {
        await performAction { client in
            try await client.pause(taskId: taskId, expectedRevision: expectedRevision)
        }
    }

    func resume(taskId: String, expectedRevision: UInt64) async {
        await performAction { client in
            try await client.resume(taskId: taskId, expectedRevision: expectedRevision)
        }
    }

    private func performAction(_ action: @MainActor (any ScheduledTaskServing) async throws -> ScheduledTaskWire?) async {
        guard actionState != .loading else { return }
        guard let client else {
            actionState = .failed(String(localized: "Connect to a host to manage scheduled tasks."))
            return
        }
        let host = attachment
        let selectedId = selectedTask?.taskId
        actionState = .loading
        do {
            let updated = try await action(client)
            guard host == attachment else { return }
            try Task.checkCancellation()
            if let updated, let index = tasks.firstIndex(where: { $0.taskId == updated.taskId }) {
                tasks[index] = updated
            }
            if let updated, selectedTask?.taskId == updated.taskId { selectedTask = updated }
            await refresh(projectId: projectId)
            guard host == attachment else { return }
            // Do not steal selection if the user opened a different task while the action ran.
            if let selectedId, selectedTask?.taskId == selectedId {
                await loadDetails(taskId: selectedId)
            }
            guard host == attachment else { return }
            actionState = Task.isCancelled ? .idle : .loaded
        } catch {
            guard host == attachment else { return }
            if Task.isCancelled || error is CancellationError {
                actionState = .idle
            } else {
                actionState = .failed(error.localizedDescription)
                HostLog.session.error("Scheduled task action failed")
            }
        }
    }
}
