import Foundation

/// Pure presentation inputs: nil means not reported or not loaded, never zero.
struct NocturneWorkspaceSnapshot: Equatable, Sendable {
    var gitCwd: String?
    var gitBranch: String?
    var gitChangedFileCount: Int?
    var scheduledTaskCount: Int?
    /// Host-detected agent processes per project id. Absent means the store has
    /// not refreshed yet, never zero.
    var liveAgentSessionCounts: [String: Int] = [:]
}

enum NocturneWorkspacePresentation {
    static func tool(_ destination: ToolDestination, snapshot: NocturneWorkspaceSnapshot) -> WorkspaceTool {
        let detail: String
        let note: String
        var metricIsLoaded = false
        switch destination {
        case .git:
            detail = snapshot.gitChangedFileCount.map { String(localized: "\($0) changed files") } ?? destination.detail
            note = String(localized: "Read only")
            metricIsLoaded = snapshot.gitChangedFileCount != nil
        case .scheduledTasks:
            detail = snapshot.scheduledTaskCount.map { String(localized: "\($0) tasks") } ?? destination.detail
            note = String(localized: "Host schedules")
            metricIsLoaded = snapshot.scheduledTaskCount != nil
        case .search:
            detail = destination.detail
            note = String(localized: "Read only")
        case .screenText:
            detail = destination.detail
            // The hub snapshot carries no screen state. An empty note hides the
            // line entirely instead of claiming "view only".
            note = ""
        }
        return WorkspaceTool(id: destination.rawValue, title: destination.title,
                             detail: detail, note: note, icon: destination.icon,
                             metricIsLoaded: metricIsLoaded)
    }

    static func project(_ project: HostProject, snapshot: NocturneWorkspaceSnapshot) -> WorkspaceProject {
        // GitStore has one scoped snapshot. Never reuse it for another project.
        let matchesGit = project.path != nil && snapshot.gitCwd == project.path
        let changes = matchesGit ? snapshot.gitChangedFileCount : nil
        var details = [String(localized: "\(project.liveTerminalCount) live terminals")]
        if let agents = snapshot.liveAgentSessionCounts[project.id], agents > 0 {
            details.append(agents == 1
                ? String(localized: "1 agent session")
                : String(localized: "\(agents) agent sessions"))
        }
        if let changes { details.append(String(localized: "\(changes) changed files")) }
        return WorkspaceProject(id: project.id, name: project.name,
                                branch: matchesGit ? (snapshot.gitBranch ?? "") : "",
                                detail: details.joined(separator: " · "),
                                hasChanges: (changes ?? 0) > 0, path: project.path ?? "")
    }

    static func rowText(branch: String, detail: String) -> String {
        [branch, detail].filter { !$0.isEmpty }.joined(separator: " · ")
    }
}
