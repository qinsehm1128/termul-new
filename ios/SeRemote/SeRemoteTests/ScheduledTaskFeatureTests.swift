import Foundation
import Testing
@testable import SeRemote

@MainActor
struct ScheduledTaskFeatureTests {
    private func decode<T: Decodable & Sendable & Hashable>(_ type: T.Type, _ value: Any) throws -> T {
        let data = try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])
        let decoded = try JSONDecoder().decode(type, from: data)
        #expect(Set([decoded]).contains(decoded))
        return decoded
    }

    @Test func taskFixtureAndMissingOptionals() throws {
        let task = try decode(ScheduledTaskWire.self, Self.taskFixture)
        #expect(task.id == "task-1")
        #expect(task.projectId == nil)
        #expect(task.nextRunAt == nil)
        #expect(task.sourceConversationId == "conversation")
        #expect(task.skillTemplateVersion == 2)
        var missingSource = Self.taskFixture
        missingSource.removeValue(forKey: "sourceConversationId")
        #expect(try decode(ScheduledTaskWire.self, missingSource).sourceConversationId == nil)
        missingSource["sourceConversationId"] = NSNull()
        #expect(try decode(ScheduledTaskWire.self, missingSource).sourceConversationId == nil)
        #expect(task.status == .active)
        #expect(task.schedule == .cron(expression: "0 9 * * *", timezone: "Asia/Shanghai"))
        #expect(task.executionTarget == .workspace)
        #expect(task.permissions == ["read"])
        #expect(task.revision == 7)
        var full = Self.taskFixture
        full["projectId"] = "project-1"
        full["nextRunAt"] = "2026-11-09T01:00:00Z"
        let populated = try decode(ScheduledTaskWire.self, full)
        #expect(populated.projectId == "project-1")
        #expect(populated.nextRunAt == "2026-11-09T01:00:00Z")
        full["projectId"] = NSNull()
        full["nextRunAt"] = NSNull()
        #expect(try decode(ScheduledTaskWire.self, full).projectId == nil)
        var future = Self.taskFixture
        future["status"] = "archived"
        future["schedule"] = ["kind": "calendar", "newField": true]
        future["executionTarget"] = ["kind": "remote", "newField": true]
        let unknown = try decode(ScheduledTaskWire.self, future)
        #expect(unknown.status == .unknown("archived"))
        #expect(unknown.schedule == .unknown("calendar"))
        #expect(unknown.executionTarget == .unknown("remote"))
    }

    @Test func taggedScheduleAndExecutionTargetFixtures() throws {
        #expect(try decode(ScheduleSpecWire.self, ["kind": "at", "at": "later"]) == .at(at: "later"))
        // Exact Rust wire spelling: enum rename_all does not rename variant fields.
        #expect(try decode(ScheduleSpecWire.self, ["kind": "interval", "every_seconds": 60, "anchor_at": "now"])
                == .interval(everySeconds: 60, anchorAt: "now"))
        #expect(try decode(ScheduleSpecWire.self, ["kind": "interval", "everySeconds": 30, "anchorAt": "now"])
                == .interval(everySeconds: 30, anchorAt: "now"))
        #expect(try decode(ExecutionTargetWire.self, ["kind": "project_root", "projectId": "p", "projectRoot": "/repo"])
                == .projectRoot(projectId: "p", projectRoot: "/repo"))
        #expect(try decode(ExecutionTargetWire.self, ["kind": "worktree", "projectId": "p", "worktreePath": "/tree", "worktreeBranch": "main"])
                == .worktree(projectId: "p", worktreePath: "/tree", worktreeBranch: "main"))
        #expect(throws: DecodingError.self) { try decode(ScheduleSpecWire.self, ["kind": "cron"]) }
        #expect(throws: DecodingError.self) { try decode(ExecutionTargetWire.self, ["kind": "project_root"]) }
        #expect(throws: DecodingError.self) { try decode(ScheduleSpecWire.self, [:]) }
    }

    @Test func allStringEnumFixtures() throws {
        #expect(try decode([ScheduledTaskStatusWire].self, ["draft", "active", "paused", "future"])
                == [.draft, .active, .paused, .unknown("future")])
        #expect(try decode([ScheduledTaskOverlapWire].self, ["skip", "bufferOne", "future"])
                == [.skip, .bufferOne, .unknown("future")])
        #expect(try decode([ScheduledTaskCatchUpWire].self, ["skip", "latestOnce", "future"])
                == [.skip, .latestOnce, .unknown("future")])
        #expect(try decode([ScheduledTaskTriggerWire].self, ["scheduled", "manual", "catchUp", "retry", "future"])
                == [.scheduled, .manual, .catchUp, .retry, .unknown("future")])
        #expect(try decode([ScheduledTaskRunStatusWire].self,
                          ["queued", "cancelled", "running", "succeeded", "failed", "skipped", "interrupted", "future"])
                == [.queued, .cancelled, .running, .succeeded, .failed, .skipped, .interrupted, .unknown("future")])
        #expect(try decode([ScheduledTaskActorWire].self, ["human", "agent", "system", "future"])
                == [.human, .agent, .system, .unknown("future")])
        #expect(ScheduledTaskStatusWire.unknown("future").rawValue == "future")
        #expect(ScheduledTaskRunStatusWire.succeeded.rawValue == "succeeded")
    }

    @Test func executionPolicyKnownAndUnknown() throws {
        let policy = try decode(ExecutionPolicyWire.self, ["overlap": "bufferOne", "catchUp": "latestOnce", "catchUpWindowSeconds": 900])
        #expect(policy.overlap == .bufferOne)
        #expect(policy.catchUp == .latestOnce)
        #expect(policy.catchUpWindowSeconds == 900)
        let future = try decode(ExecutionPolicyWire.self, ["overlap": "parallel", "catchUp": "all", "catchUpWindowSeconds": 0])
        #expect(future.overlap == .unknown("parallel"))
        #expect(future.catchUp == .unknown("all"))
    }

    @Test func runFixtureMissingNullAndPopulatedOptionals() throws {
        let run = try decode(ScheduledTaskRunWire.self, Self.runFixture)
        #expect(run.id == "run-1")
        #expect(run.trigger == .manual)
        #expect(run.status == .queued)
        #expect(run.projectId == nil && run.startedAt == nil && run.finishedAt == nil)
        #expect(run.conversationId == nil && run.retryOfRunId == nil && run.summary == nil)
        #expect(run.errorCode == nil && run.errorDetail == nil && run.usage == nil)
        var full = Self.runFixture
        for key in Self.runOptionalStrings { full[key] = key }
        full["usage"] = ["tokens": 42, "nested": ["cached": true], "items": [1, "ok", NSNull()]] as [String: Any]
        let populated = try decode(ScheduledTaskRunWire.self, full)
        #expect(populated.projectId == "projectId")
        #expect(populated.startedAt == "startedAt" && populated.finishedAt == "finishedAt")
        #expect(populated.conversationId == "conversationId" && populated.retryOfRunId == "retryOfRunId")
        #expect(populated.summary == "summary" && populated.errorCode == "errorCode" && populated.errorDetail == "errorDetail")
        #expect(populated.usage?["tokens"]?.intValue == 42)
        for key in Self.runOptionalStrings + ["usage"] { full[key] = NSNull() }
        #expect(try decode(ScheduledTaskRunWire.self, full) == run)
        full["trigger"] = "future"
        full["status"] = "future"
        #expect(try decode(ScheduledTaskRunWire.self, full).trigger == .unknown("future"))
        #expect(try decode(ScheduledTaskRunWire.self, full).status == .unknown("future"))
    }

    @Test func auditFixtureMissingNullAndPopulatedOptionals() throws {
        let event = try decode(ScheduledTaskAuditEventWire.self, Self.auditFixture)
        #expect(event.id == "event-1" && event.actor == .human && event.action == "pause")
        #expect(event.projectId == nil && event.sourceConversationId == nil && event.sourceToolCallId == nil)
        #expect(event.beforeHash == nil && event.afterHash == nil)
        var full = Self.auditFixture
        let keys = ["projectId", "sourceConversationId", "sourceToolCallId", "beforeHash", "afterHash"]
        for key in keys { full[key] = key }
        let populated = try decode(ScheduledTaskAuditEventWire.self, full)
        #expect(populated.projectId == "projectId" && populated.sourceConversationId == "sourceConversationId")
        #expect(populated.sourceToolCallId == "sourceToolCallId" && populated.beforeHash == "beforeHash" && populated.afterHash == "afterHash")
        for key in keys { full[key] = NSNull() }
        #expect(try decode(ScheduledTaskAuditEventWire.self, full) == event)
        full["actor"] = "robot"
        full["action"] = "futureAction"
        #expect(try decode(ScheduledTaskAuditEventWire.self, full).actor == .unknown("robot"))
    }

    @Test func malformedRequiredFieldsAreNotSilentlyDefaulted() throws {
        var task = Self.taskFixture
        task.removeValue(forKey: "revision")
        #expect(throws: DecodingError.self) { try decode(ScheduledTaskWire.self, task) }
        var run = Self.runFixture
        run["status"] = 42
        #expect(throws: DecodingError.self) { try decode(ScheduledTaskRunWire.self, run) }
        var audit = Self.auditFixture
        audit.removeValue(forKey: "actor")
        #expect(throws: DecodingError.self) { try decode(ScheduledTaskAuditEventWire.self, audit) }
    }

    @Test func taskIdsCannotInjectRouteSegments() {
        #expect(ScheduledTaskClient.taskPath("task-1") == "/scheduled-tasks/task-1")
        #expect(ScheduledTaskClient.taskPath("a/b?#%") == "/scheduled-tasks/a%2Fb%3F%23%25")
    }

    @Test func storeListDetailActionsAndFailures() async throws {
        let client = StubClient()
        let task = try decode(ScheduledTaskWire.self, Self.taskFixture)
        client.tasks = [task]
        client.task = task
        client.history = [try decode(ScheduledTaskRunWire.self, Self.runFixture)]
        client.events = [try decode(ScheduledTaskAuditEventWire.self, Self.auditFixture)]
        let store = ScheduledTaskStore()
        store.attach(client: client)
        #expect(store.state == .idle)
        await store.refresh(projectId: "p")
        #expect(store.state == .loaded && store.tasks == [task])
        #expect(client.lastProjectId == "p")
        await store.loadDetails(taskId: "task-1")
        #expect(store.detailState == .loaded && store.selectedTask == task)
        #expect(store.runs == client.history && store.auditEvents == client.events)
        await store.run(taskId: "task-1")
        await store.pause(taskId: "task-1", expectedRevision: 7)
        await store.resume(taskId: "task-1", expectedRevision: 8)
        #expect(client.actions == ["run:task-1", "pause:task-1:7", "resume:task-1:8"])
        #expect(store.actionState == .loaded)
        client.failure = StubError.failed
        await store.refresh()
        guard case .failed = store.state else { Issue.record("Expected failed list state"); return }
        await store.loadDetails(taskId: "task-1")
        guard case .failed = store.detailState else { Issue.record("Expected failed detail state"); return }
        #expect(store.selectedTask == nil && store.runs.isEmpty && store.auditEvents.isEmpty)
        await store.run(taskId: "task-1")
        guard case .failed = store.actionState else { Issue.record("Expected failed action state"); return }
        client.failure = nil
        client.tasks = []
        await store.refresh()
        #expect(store.state == .empty && store.tasks.isEmpty)
    }

    @Test func storeAttachmentDiscardsPendingOldHostAndShowsLoading() async {
        let old = StubClient()
        old.suspendList = true
        let store = ScheduledTaskStore()
        store.attach(client: old)
        let pending = Task { await store.refresh() }
        while old.listContinuation == nil { await Task.yield() }
        #expect(store.state == .loading)
        let new = StubClient()
        store.attach(client: new)
        old.listContinuation?.resume(returning: [])
        await pending.value
        #expect(store.state == .idle)
        await store.refresh()
        #expect(store.state == .empty)
    }

    @Test func storeUnattachedAndCancellationStates() async {
        let store = ScheduledTaskStore()
        await store.refresh()
        guard case .failed = store.state else { Issue.record("Expected missing host failure"); return }
        let client = StubClient()
        client.failure = CancellationError()
        store.attach(client: client)
        await store.refresh()
        #expect(store.state == .idle)
        await store.loadDetails(taskId: "task-1")
        #expect(store.detailState == .idle)
        await store.run(taskId: "task-1")
        #expect(store.actionState == .idle)
    }

    @Test func usageJSONHandlesEveryShape() throws {
        let value = try decode(ScheduledTaskJSONWire.self, ["null": NSNull(), "bool": true, "number": 1.5,
                                                           "string": "ok", "array": [1, "two"], "object": [:]] as [String: Any])
        #expect(value["null"] == .null)
        #expect(value["bool"] == .bool(true))
        #expect(value["number"] == .number(1.5))
        #expect(value["string"] == .string("ok"))
        #expect(value["array"] == .array([.number(1), .string("two")]))
        #expect(value["object"] == .object([:]))
        #expect(value["number"]?.intValue == nil)
        #expect(value["missing"] == nil)
    }

    @Test func latestRefreshWinsAndDuplicateActionsAreBlocked() async throws {
        let client = StubClient()
        client.task = try decode(ScheduledTaskWire.self, Self.taskFixture)
        client.history = [try decode(ScheduledTaskRunWire.self, Self.runFixture)]
        client.suspendList = true
        let store = ScheduledTaskStore()
        store.attach(client: client)
        let first = Task { await store.refresh(projectId: "old") }
        while client.listContinuation == nil { await Task.yield() }
        client.suspendList = false
        await store.refresh(projectId: "new")
        client.listContinuation?.resume(returning: [try #require(client.task)])
        await first.value
        #expect(store.state == .empty && store.tasks.isEmpty && store.projectId == "new")
        client.listContinuation = nil
        client.suspendList = true
        let action = Task { await store.run(taskId: "task-1") }
        while client.listContinuation == nil { await Task.yield() }
        #expect(store.actionState == .loading)
        await store.run(taskId: "task-1")
        #expect(client.actions == ["run:task-1"])
        client.listContinuation?.resume(returning: [])
        await action.value
        #expect(store.actionState == .loaded)
    }

    private static let taskFixture: [String: Any] = [
        "schemaVersion": 1, "taskId": "task-1", "name": "Daily summary", "description": "A task",
        "status": "active", "schedule": ["kind": "cron", "expression": "0 9 * * *", "timezone": "Asia/Shanghai"],
        "executionPolicy": ["overlap": "skip", "catchUp": "skip", "catchUpWindowSeconds": 60],
        "prompt": "Summarize", "agentConfigId": "agent", "executionTarget": ["kind": "workspace"],
        "executionCwd": "/repo", "workspaceCwd": "/repo", "sourceConversationId": "conversation",
        "permissions": ["read"], "skillTemplateVersion": 2, "revision": 7, "draftHash": "hash",
        "createdAt": "2026-11-08T00:00:00Z", "updatedAt": "2026-11-08T01:00:00Z"
    ]
    private static let runFixture: [String: Any] = [
        "schemaVersion": 1, "runId": "run-1", "taskId": "task-1", "trigger": "manual", "status": "queued",
        "occurrenceKey": "manual:1", "scheduledFor": "2026-11-08T00:00:00Z", "queuedAt": "2026-11-08T00:00:00Z",
        "taskSnapshotHash": "hash"
    ]
    private static let runOptionalStrings = ["projectId", "startedAt", "finishedAt", "conversationId", "retryOfRunId", "summary", "errorCode", "errorDetail"]
    private static let auditFixture: [String: Any] = [
        "schemaVersion": 1, "eventId": "event-1", "taskId": "task-1", "action": "pause", "actor": "human",
        "createdAt": "2026-11-08T00:00:00Z"
    ]

    private enum StubError: Error { case failed }

    @MainActor
    private final class StubClient: ScheduledTaskServing {
        var tasks: [ScheduledTaskWire] = []
        var task: ScheduledTaskWire?
        var history: [ScheduledTaskRunWire] = []
        var events: [ScheduledTaskAuditEventWire] = []
        var failure: (any Error)?
        var lastProjectId: String?
        var actions: [String] = []
        var suspendList = false
        var listContinuation: CheckedContinuation<[ScheduledTaskWire], any Error>?
        func list(projectId: String?) async throws -> [ScheduledTaskWire] {
            lastProjectId = projectId
            if suspendList { return try await withCheckedThrowingContinuation { listContinuation = $0 } }
            if let failure { throw failure }
            return tasks
        }
        func get(taskId: String) async throws -> ScheduledTaskWire {
            if let failure { throw failure }
            return try #require(task)
        }
        func runs(taskId: String) async throws -> [ScheduledTaskRunWire] {
            if let failure { throw failure }
            return history
        }
        func audit(taskId: String) async throws -> [ScheduledTaskAuditEventWire] {
            if let failure { throw failure }
            return events
        }
        func run(taskId: String) async throws -> ScheduledTaskRunWire {
            if let failure { throw failure }
            actions.append("run:\(taskId)")
            return try #require(history.first)
        }
        func pause(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire {
            if let failure { throw failure }
            actions.append("pause:\(taskId):\(expectedRevision)")
            return try #require(task)
        }
        func resume(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire {
            if let failure { throw failure }
            actions.append("resume:\(taskId):\(expectedRevision)")
            return try #require(task)
        }
    }
}
