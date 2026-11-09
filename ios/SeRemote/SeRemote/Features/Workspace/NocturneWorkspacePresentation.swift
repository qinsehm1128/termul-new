import Foundation

/// Pure presentation inputs: nil means not reported or not loaded, never zero.
struct NocturneWorkspaceSnapshot: Equatable, Sendable {
    var gitCwd: String?
    var gitBranch: String?
    var gitChangedFileCount: Int?
    var scheduledTaskCount: Int?
    var quickTerminalCount: Int?
}

enum NocturneWorkspacePresentation {
    static func tool(_ destination: ToolDestination, snapshot: NocturneWorkspaceSnapshot) -> WorkspaceTool {
        let detail: String
        let note: String
        switch destination {
        case .git:
            detail = snapshot.gitChangedFileCount.map { String(localized: "\($0) changed files") } ?? destination.detail
            note = String(localized: "Read only")
        case .scheduledTasks:
            detail = snapshot.scheduledTaskCount.map { String(localized: "\($0) tasks") } ?? destination.detail
            note = String(localized: "Host schedules")
        case .quickTerminals:
            detail = snapshot.quickTerminalCount.map { String(localized: "\($0) terminals") } ?? destination.detail
            note = String(localized: "Persistent host terminals")
        case .search:
            detail = destination.detail
            note = String(localized: "Read only")
        case .screenText:
            detail = destination.detail
            note = String(localized: "Matching terminal snapshot required")
        }
        return WorkspaceTool(id: destination.rawValue, title: destination.title,
                             detail: detail, note: note, systemImage: destination.symbol)
    }

    static func project(_ project: HostProject, snapshot: NocturneWorkspaceSnapshot) -> WorkspaceProject {
        // GitStore has one scoped snapshot. Never reuse it for another project.
        let matchesGit = project.path != nil && snapshot.gitCwd == project.path
        let changes = matchesGit ? snapshot.gitChangedFileCount : nil
        var details = [String(localized: "\(project.liveTerminalCount) live terminals")]
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
