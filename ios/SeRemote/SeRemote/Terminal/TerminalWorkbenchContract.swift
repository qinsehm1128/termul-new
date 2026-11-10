import Foundation

/// Shared contracts for the terminal-centric workbench.
///
/// One terminal surface hosts both backends: a workspace terminal owned by
/// `TerminalStore`, and a standalone quick terminal owned by `TerminalPTYViewer`.
/// Quick terminals never enter `TerminalStore.terminals`, so their lease stays
/// isolated from catalog refreshes.

nonisolated enum TerminalAttach: Hashable, Sendable {
    case workspace(terminalID: String)
    case standalone(terminalID: String, claim: String?)

    var terminalID: String {
        switch self {
        case let .workspace(terminalID), let .standalone(terminalID, _):
            terminalID
        }
    }

    var isStandalone: Bool {
        if case .standalone = self { return true }
        return false
    }
}

nonisolated struct TerminalOpenRequest: Identifiable, Hashable, Sendable {
    let attach: TerminalAttach
    let title: String
    let cwd: String?
    let projectID: String?
    let projectName: String?

    init(
        attach: TerminalAttach,
        title: String,
        cwd: String? = nil,
        projectID: String? = nil,
        projectName: String? = nil
    ) {
        self.attach = attach
        self.title = title
        self.cwd = cwd
        self.projectID = projectID
        self.projectName = projectName
    }

    var terminalID: String { attach.terminalID }

    var id: String {
        switch attach {
        case let .workspace(terminalID): "workspace:\(terminalID)"
        case let .standalone(terminalID, _): "standalone:\(terminalID)"
        }
    }
}

nonisolated struct TerminalSurfaceItem: Identifiable, Hashable, Sendable {
    let id: String
    let title: String
    let cwd: String?
    let projectID: String?
    let hasAgent: Bool

    init(id: String, title: String, cwd: String? = nil, projectID: String? = nil, hasAgent: Bool = false) {
        self.id = id
        self.title = title
        self.cwd = cwd
        self.projectID = projectID
        self.hasAgent = hasAgent
    }
}

/// Actions reachable from the collapsed title pill's overlay panel and the
/// floating [+][keyboard] cluster.
nonisolated enum TerminalPanelAction: Hashable, Sendable {
    case files, desktop, agent, more, attach
}

nonisolated struct FilePreviewRequest: Identifiable, Hashable, Sendable {
    let path: String
    let terminalID: String?

    init(path: String, terminalID: String? = nil) {
        self.path = path
        self.terminalID = terminalID
    }

    var id: String { path }
}

/// Where a deep link or notification should land.
nonisolated struct TerminalRoute: Equatable, Sendable {
    let hostID: String?
    let terminalID: String
}
