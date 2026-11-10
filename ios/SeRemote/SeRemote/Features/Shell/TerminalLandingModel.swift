import Foundation

/// What T1 (the terminal landing page) renders.
///
/// Pure projection of host state: live terminals grouped by project, the quick
/// terminal shortcuts, and the "continue" hero for the terminal that was open
/// last. Nothing here spawns, attaches or fabricates a terminal — an empty host
/// response is an empty landing page.
nonisolated struct TerminalLandingModel: Equatable, Sendable {
    struct Row: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        let cwd: String?
        let branch: String?
        let projectID: String?
        let projectName: String?
        let hasAgent: Bool
    }

    struct Group: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let rows: [Row]

        var count: Int { rows.count }
    }

    struct QuickRow: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        let cwd: String
        let targetLabel: String
        let liveCount: Int
        /// The first live shell this shortcut owns, or `nil` when it has none.
        let terminalID: String?

        var canOpen: Bool { terminalID != nil }
    }

    /// Last terminal the user had open, when the host still reports it.
    var hero: Row?
    var groups: [Group] = []
    var quick: [QuickRow] = []
    /// The new-terminal entry is only offered when the device may create a
    /// shell and the host has a project to start it in.
    var canCreate = false

    var isEmpty: Bool { hero == nil && groups.isEmpty && quick.isEmpty }
    var terminalCount: Int { groups.reduce(0) { $0 + $1.count } }

    static func make(
        summaries: [LiveTerminalSummary],
        projects: [HostProject],
        quickRecords: [QuickTerminalRecordWire],
        runningAgentTerminalIDs: Set<String> = [],
        lastTerminalID: String? = nil,
        allowsCreate: Bool = false
    ) -> TerminalLandingModel {
        let names = Dictionary(projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
        let hubTerminals = summaries.map { summary in
            TerminalsHubTerminal(
                id: summary.id,
                title: TerminalsHubPresentation.title(
                    hostTitle: summary.title,
                    cwd: summary.cwd,
                    shell: summary.shell
                ),
                cwd: summary.cwd,
                branch: summary.gitBranch?.trimmingCharacters(in: .whitespacesAndNewlines),
                projectId: summary.projectId?.trimmingCharacters(in: .whitespacesAndNewlines),
                hasRunningAgent: runningAgentTerminalIDs.contains(summary.id)
            )
        }
        let excluded = TerminalsHubPresentation.quickTerminalIds(quickRecords)
        let groups = TerminalsHubPresentation.groups(
            terminals: hubTerminals,
            projects: projects.map { TerminalsHubProject(id: $0.id, name: $0.name) },
            excluding: excluded
        ).map { group in
            Group(
                id: group.id,
                name: group.name,
                rows: group.terminals.map { row(from: $0, projectName: names[$0.projectId ?? ""]) }
            )
        }

        let rowsByID = Dictionary(
            groups.flatMap(\.rows).map { ($0.id, $0) },
            uniquingKeysWith: { first, _ in first }
        )
        let hero = lastTerminalID.flatMap { rowsByID[$0] }

        let quick = quickRecords.map { record in
            let live = QuickTerminalPresentation.liveTerminalIds(record)
            return QuickRow(
                id: record.id,
                title: QuickTerminalPresentation.title(record),
                cwd: record.cwd,
                targetLabel: QuickTerminalPresentation.targetLabel(record.target),
                liveCount: live.count,
                terminalID: live.first
            )
        }

        return TerminalLandingModel(
            hero: hero,
            groups: groups,
            quick: quick,
            canCreate: allowsCreate && !projects.isEmpty
        )
    }

    /// Request that opens one listed terminal.
    static func request(for row: Row) -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .workspace(terminalID: row.id),
            title: row.title,
            cwd: row.cwd,
            projectID: row.projectID,
            projectName: row.projectName
        )
    }

    /// Request for a host summary, used by deep links and the last-terminal
    /// resume path before T1 has rendered.
    static func request(for summary: LiveTerminalSummary, projects: [HostProject]) -> TerminalOpenRequest {
        let projectID = summary.projectId?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenBlank
        return TerminalOpenRequest(
            attach: .workspace(terminalID: summary.id),
            title: TerminalsHubPresentation.title(
                hostTitle: summary.title,
                cwd: summary.cwd,
                shell: summary.shell
            ),
            cwd: summary.cwd,
            projectID: projectID,
            projectName: projectID.flatMap { id in projects.first(where: { $0.id == id })?.name }
        )
    }

    private static func row(from terminal: TerminalsHubTerminal, projectName: String?) -> Row {
        Row(
            id: terminal.id,
            title: terminal.title,
            cwd: terminal.cwd,
            branch: terminal.branch,
            projectID: terminal.projectId,
            projectName: projectName,
            hasAgent: terminal.hasRunningAgent
        )
    }
}

/// What terminal a conversation row opens.
///
/// A conversation with a live terminal goes to the workbench with its agent
/// panel; a conversation without one opens the chat cover. The decision is a
/// pure value so both paths stay testable without a session.
nonisolated enum ConversationOpenPolicy {
    enum Destination: Equatable, Sendable {
        case terminal(TerminalOpenRequest)
        case chat(conversationID: String)
    }

    static func destination(
        conversation: HostConversation,
        terminals: [LiveTerminalSummary],
        projects: [HostProject] = []
    ) -> Destination {
        if let terminal = terminal(for: conversation, in: terminals) {
            return .terminal(TerminalLandingModel.request(for: terminal, projects: projects))
        }
        return .chat(conversationID: conversation.conversationId)
    }

    /// A terminal bound to this conversation wins; otherwise one in the same
    /// project. Never guess: a conversation with no live shell opens chat.
    static func terminal(
        for conversation: HostConversation,
        in terminals: [LiveTerminalSummary]
    ) -> LiveTerminalSummary? {
        if let bound = terminals.first(where: { $0.conversationId == conversation.conversationId }) {
            return bound
        }
        let projectID = conversation.projectAttachment?.projectId
            ?? conversation.executionTarget?.projectId
        guard let projectID else { return nil }
        return terminals.first(where: { $0.projectId == projectID })
    }
}

/// Last terminal per host, so a connect can resume it and T1 can show a
/// "continue" hero. Only identity is stored: titles, cwds and project names are
/// re-read from the host catalog, never replayed from disk.
nonisolated enum TerminalLastSeen {
    static func key(origin: URL) -> String {
        "terminal.last.\(origin.absoluteString)"
    }

    static func save(_ request: TerminalOpenRequest, origin: URL, defaults: UserDefaults = .standard) {
        let stored = Stored(
            kind: request.attach.isStandalone ? "standalone" : "workspace",
            terminalID: request.terminalID,
            title: request.title,
            cwd: request.cwd,
            projectID: request.projectID,
            projectName: request.projectName
        )
        guard let data = try? JSONEncoder().encode(stored) else { return }
        defaults.set(data, forKey: key(origin: origin))
    }

    static func load(origin: URL, defaults: UserDefaults = .standard) -> TerminalOpenRequest? {
        guard let data = defaults.data(forKey: key(origin: origin)),
              let stored = try? JSONDecoder().decode(Stored.self, from: data),
              !stored.terminalID.isEmpty else {
            return nil
        }
        let attach: TerminalAttach = stored.kind == "standalone"
            ? .standalone(terminalID: stored.terminalID, claim: nil)
            : .workspace(terminalID: stored.terminalID)
        return TerminalOpenRequest(
            attach: attach,
            title: stored.title,
            cwd: stored.cwd,
            projectID: stored.projectID,
            projectName: stored.projectName
        )
    }

    static func clear(origin: URL, defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: key(origin: origin))
    }

    private struct Stored: Codable {
        var kind: String
        var terminalID: String
        var title: String
        var cwd: String?
        var projectID: String?
        var projectName: String?
    }
}

private extension String {
    nonisolated var nilWhenBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
