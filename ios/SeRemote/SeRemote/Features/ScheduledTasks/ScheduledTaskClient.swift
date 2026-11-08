import Foundation

@MainActor
protocol ScheduledTaskServing {
    func list(projectId: String?) async throws -> [ScheduledTaskWire]
    func get(taskId: String) async throws -> ScheduledTaskWire
    func runs(taskId: String) async throws -> [ScheduledTaskRunWire]
    func audit(taskId: String) async throws -> [ScheduledTaskAuditEventWire]
    func run(taskId: String) async throws -> ScheduledTaskRunWire
    func pause(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire
    func resume(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire
}

/// Intentionally exposes no draft, activation, deletion, or retry routes.
@MainActor
struct ScheduledTaskClient: ScheduledTaskServing {
    let http: HostHTTP

    func list(projectId: String? = nil) async throws -> [ScheduledTaskWire] {
        try await request("list") {
            try await http.get("/scheduled-tasks", query: projectId.map { ["projectId": $0] } ?? [:])
        }
    }

    func get(taskId: String) async throws -> ScheduledTaskWire {
        try await request("get") { try await http.get(Self.taskPath(taskId)) }
    }

    func runs(taskId: String) async throws -> [ScheduledTaskRunWire] {
        try await request("runs") { try await http.get(Self.taskPath(taskId) + "/runs") }
    }

    func audit(taskId: String) async throws -> [ScheduledTaskAuditEventWire] {
        try await request("audit") { try await http.get(Self.taskPath(taskId) + "/audit") }
    }

    func run(taskId: String) async throws -> ScheduledTaskRunWire {
        try await request("run") { try await http.post(Self.taskPath(taskId) + "/run", body: [:]) }
    }

    func pause(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire {
        try await request("pause") {
            try await http.post(Self.taskPath(taskId) + "/pause", body: ["expectedRevision": expectedRevision])
        }
    }

    func resume(taskId: String, expectedRevision: UInt64) async throws -> ScheduledTaskWire {
        try await request("resume") {
            try await http.post(Self.taskPath(taskId) + "/resume", body: ["expectedRevision": expectedRevision])
        }
    }

    static func taskPath(_ taskId: String) -> String {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-._~"))
        return "/scheduled-tasks/" + (taskId.addingPercentEncoding(withAllowedCharacters: allowed) ?? "")
    }

    private func request<T>(_ operation: StaticString, _ body: @MainActor () async throws -> T) async throws -> T {
        // Never interpolate URLs, credentials, task prompts, or server error text into logs.
        HostLog.session.debug("Scheduled task request: \(operation)")
        do {
            return try await body()
        } catch {
            if !Task.isCancelled { HostLog.session.error("Scheduled task request failed: \(operation)") }
            throw error
        }
    }
}
