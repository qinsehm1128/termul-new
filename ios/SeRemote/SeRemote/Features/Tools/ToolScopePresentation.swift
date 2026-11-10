import Foundation

/// Scope of the 工具 hub, derived from the terminal the user is working in.
///
/// Tools follow the terminal: Git, search, scheduled tasks and screen text are
/// read against the project that terminal belongs to and the directory it is
/// actually in. That is `WorkspaceSession.currentTerminalProjectID` and
/// `WorkspaceSession.activeToolRoot`.
///
/// With no terminal there is no scope to read, so the hub disables every tile
/// and explains why through `SeInfoTip` instead of opening a tool against an
/// invented project.
struct ToolScope: Equatable, Sendable {
    let projectID: String?
    let projectName: String?
    let cwd: String?

    /// No terminal, no project, no directory.
    static let none = ToolScope(projectID: nil, projectName: nil, cwd: nil)

    /// A terminal scope exists only when the host reported a project or a
    /// working directory. Empty strings count as absent.
    var isEnabled: Bool {
        !(projectID ?? "").isEmpty || !(cwd ?? "").isEmpty
    }

    /// Project the tools belong to, falling back to the directory name so the
    /// header never shows a host identifier.
    var projectLabel: String {
        if let projectName, !projectName.isEmpty { return projectName }
        if let directory = directoryName { return directory }
        return String(localized: "No terminal")
    }

    /// The terminal's own directory. It can sit deeper than the project, so it
    /// stays a separate line rather than replacing the project name.
    var scopeDetail: String? {
        guard let cwd, !cwd.isEmpty else { return nil }
        return cwd
    }

    var disabledReason: String {
        String(localized: "Open a terminal to scope the tools to its project.")
    }

    private var directoryName: String? {
        guard let cwd, !cwd.isEmpty else { return nil }
        let name = (cwd as NSString).lastPathComponent
        return name.isEmpty ? cwd : name
    }
}

enum ToolScopePresentation {
    /// Pure mapping from the live terminal to the hub's scope. Every input is
    /// nil when no terminal is open, which disables the tiles.
    static func scope(terminalProjectID: String?, projectName: String?, terminalCwd: String?) -> ToolScope {
        ToolScope(projectID: terminalProjectID, projectName: projectName, cwd: terminalCwd)
    }

    /// Live derivation. An attached terminal is the gate: a workspace can hold
    /// a project without having opened a terminal yet, and tools must not
    /// pretend that project is the current scope in that case.
    @MainActor
    static func scope(for session: WorkspaceSession) -> ToolScope {
        guard session.terminals.activeId != nil else { return .none }
        return scope(
            terminalProjectID: session.currentTerminalProjectID,
            projectName: session.currentTerminalProjectName,
            terminalCwd: session.activeToolRoot)
    }
}
