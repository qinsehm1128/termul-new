import Foundation

/// The one list the 终端 hub, the in-terminal switcher and the iPad rail read.
///
/// Live shells, quick-terminal records, live agents, projects and host project
/// groups are merged into a single index, then narrowed by a search text, a
/// status filter and a group filter. Pure value: no host calls, no SwiftUI,
/// so every rule is unit-testable.
///
/// A shell a quick terminal owns is listed under that quick terminal *and* in
/// its project, marked as pinned. It never disappears from either place.
nonisolated struct TerminalHubIndex: Equatable, Sendable {
    enum StatusFilter: String, CaseIterable, Identifiable, Sendable {
        case all
        case running
        case agent
        case idle

        var id: String { rawValue }

        var title: String {
            switch self {
            case .all: hubText("All")
            case .running: hubText("Running")
            case .agent: hubText("Agent")
            case .idle: hubText("Not started")
            }
        }
    }

    struct Query: Equatable, Sendable {
        var text = ""
        var status: StatusFilter = .all
        /// `nil` shows every group.
        var groupID: String?

        var trimmedText: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
        var isFiltering: Bool { !trimmedText.isEmpty || status != .all || groupID != nil }
    }

    struct Shell: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        let cwd: String?
        let branch: String?
        let projectID: String?
        let projectName: String?
        let groupID: String
        let groupName: String
        let hasAgent: Bool
        /// The quick terminal that owns this shell, when one does.
        let quickID: String?
        let quickTitle: String?
        let isLast: Bool

        var isPinned: Bool { quickID != nil }
    }

    struct Quick: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        let cwd: String
        let projectID: String?
        let groupID: String
        /// Live shells the record owns, filtered to the current query.
        let shells: [Shell]
        /// Live shells the record owns before filtering.
        let liveCount: Int
        let canOpen: Bool

        var isRunning: Bool { liveCount > 0 }
    }

    struct ProjectSection: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let projectID: String?
        let shells: [Shell]
    }

    struct GroupSection: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let projects: [ProjectSection]

        var count: Int { projects.reduce(0) { $0 + $1.shells.count } }
    }

    struct GroupOption: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let runningCount: Int
    }

    struct ProjectHit: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let path: String?
        let groupName: String?
        let runningCount: Int
    }

    static let ungroupedID = "__hub_ungrouped__"
    static let workspaceSectionID = "__hub_workspace__"

    static var ungroupedName: String { hubText("Ungrouped") }
    static var workspaceName: String { hubText("Workspace") }

    /// One chip of the workbench's top strip: every live shell across every
    /// project, in index order, with the project as its compact label.
    nonisolated struct StripItem: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        /// Compact project label; `nil` for a workspace (project-less) shell.
        let projectLabel: String?
        let hasAgent: Bool
        let isActive: Bool
    }

    var query = Query()
    var quick: [Quick] = []
    var groups: [GroupSection] = []
    var projectHits: [ProjectHit] = []
    var counts: [StatusFilter: Int] = [:]
    var groupOptions: [GroupOption] = []
    /// Group headers only help when the host actually defines groups.
    var showsGroupHeaders = false
    var canCreate = false

    /// Every listed shell, in display order, without the quick duplicates.
    var shells: [Shell] { groups.flatMap { $0.projects.flatMap(\.shells) } }
    var isEmpty: Bool { quick.isEmpty && groups.isEmpty && projectHits.isEmpty }
    var totalRunning: Int { counts[.running] ?? 0 }

    func count(_ filter: StatusFilter) -> Int { counts[filter] ?? 0 }

    // MARK: - Strip

    /// The workbench strip for this index: the live shells in display order,
    /// the given terminal highlighted. Built from an unfiltered index so the
    /// strip never inherits the hub's or the switcher's search text.
    func strip(activeTerminalID: String?) -> [StripItem] {
        Self.strip(from: self, activeTerminalID: activeTerminalID)
    }

    static func strip(from index: TerminalHubIndex, activeTerminalID: String?) -> [StripItem] {
        index.shells.map { shell in
            StripItem(
                id: shell.id,
                title: stripTitle(shell),
                projectLabel: stripProjectLabel(shell),
                hasAgent: shell.hasAgent,
                isActive: shell.id == activeTerminalID
            )
        }
    }

    /// The chip shows the shell's own title; a title that is just a cwd falls
    /// back to the directory name, which is what tells two shells apart.
    nonisolated static func stripTitle(_ shell: Shell) -> String {
        let title = shell.title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !title.isEmpty { return title }
        if let cwd = shell.cwd, let name = cwd.split(separator: "/").last, !name.isEmpty {
            return String(name)
        }
        return hubText("Terminal")
    }

    /// Shells of one project share a label, so the strip prints it once per
    /// run instead of on every chip (see `stripShowsProjectLabel`).
    nonisolated static func stripProjectLabel(_ shell: Shell) -> String? {
        shell.projectName?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenBlank
    }

    /// Pure layout rule for the strip: a chip prints its project label only
    /// when it starts a new project run, so a project with five shells shows
    /// its name once and five compact chips.
    nonisolated static func stripShowsProjectLabel(
        at index: Int,
        items: [StripItem]
    ) -> Bool {
        guard items.indices.contains(index) else { return false }
        let label = items[index].projectLabel
        if label == nil { return false }
        guard index > 0 else { return true }
        return items[index - 1].projectLabel != label
    }

    // MARK: - Build

    static func make(
        summaries: [LiveTerminalSummary],
        projects: [HostProject],
        projectGroups: [HostProjectGroup],
        quickRecords: [QuickTerminalRecordWire],
        agentTerminalIDs: Set<String> = [],
        lastTerminalID: String? = nil,
        allowsCreate: Bool = false,
        query: Query = Query()
    ) -> TerminalHubIndex {
        let catalog = Catalog(projects: projects, groups: projectGroups)

        // Quick ownership first, so every shell knows whether it is pinned.
        var ownerByShell: [String: QuickTerminalRecordWire] = [:]
        for record in quickRecords {
            for id in QuickTerminalPresentation.liveTerminalIds(record) where ownerByShell[id] == nil {
                ownerByShell[id] = record
            }
        }

        let allShells: [Shell] = summaries.map { summary in
            let projectID = summary.projectId?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenBlank
            let group = catalog.group(for: projectID)
            let owner = ownerByShell[summary.id]
            return Shell(
                id: summary.id,
                title: TerminalsHubPresentation.title(
                    hostTitle: summary.title,
                    cwd: summary.cwd,
                    shell: summary.shell
                ),
                cwd: summary.cwd?.nilWhenBlank,
                branch: summary.gitBranch?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenBlank,
                projectID: projectID,
                projectName: projectID.map { catalog.projectName($0) },
                groupID: group.id,
                groupName: group.name,
                hasAgent: agentTerminalIDs.contains(summary.id),
                quickID: owner?.id,
                quickTitle: owner.map(QuickTerminalPresentation.title),
                isLast: summary.id == lastTerminalID
            )
        }
        let shellsByID = Dictionary(allShells.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })

        let needle = query.trimmedText.lowercased()
        let inGroup: (String) -> Bool = { groupID in
            query.groupID == nil || query.groupID == groupID
        }

        // Text + group narrowing; counts are computed here, before status.
        let visibleShells = allShells.filter { inGroup($0.groupID) && matches($0, needle) }

        struct QuickCandidate {
            let record: QuickTerminalRecordWire
            let projectID: String?
            let groupID: String
            let liveShells: [Shell]
            let titleMatches: Bool
        }
        let quickCandidates: [QuickCandidate] = quickRecords.compactMap { record in
            let projectID = quickProjectID(record.target)
            let group = catalog.group(for: projectID)
            guard inGroup(group.id) else { return nil }
            let live = QuickTerminalPresentation.liveTerminalIds(record).compactMap { shellsByID[$0] }
            let titleMatches = needle.isEmpty || [
                QuickTerminalPresentation.title(record),
                record.cwd,
                projectID.map { catalog.projectName($0) } ?? "",
                group.name,
            ].contains { $0.lowercased().contains(needle) }
            let anyShellMatches = live.contains { matches($0, needle) }
            guard titleMatches || anyShellMatches else { return nil }
            return QuickCandidate(
                record: record,
                projectID: projectID,
                groupID: group.id,
                liveShells: live,
                titleMatches: titleMatches
            )
        }

        var counts: [StatusFilter: Int] = [:]
        counts[.running] = visibleShells.count
        counts[.agent] = visibleShells.filter(\.hasAgent).count
        counts[.idle] = quickCandidates.filter { $0.liveShells.isEmpty }.count
        counts[.all] = (counts[.running] ?? 0) + (counts[.idle] ?? 0)

        // Status narrowing.
        let shellPasses: (Shell) -> Bool = { shell in
            switch query.status {
            case .all, .running: true
            case .agent: shell.hasAgent
            case .idle: false
            }
        }

        let quick: [Quick] = quickCandidates.compactMap { candidate in
            let textShells = candidate.titleMatches
                ? candidate.liveShells
                : candidate.liveShells.filter { matches($0, needle) }
            let shown: [Shell]
            switch query.status {
            case .all:
                shown = textShells
            case .running, .agent:
                shown = textShells.filter(shellPasses)
                if shown.isEmpty { return nil }
            case .idle:
                guard candidate.liveShells.isEmpty else { return nil }
                shown = []
            }
            return Quick(
                id: candidate.record.id,
                title: QuickTerminalPresentation.title(candidate.record),
                cwd: candidate.record.cwd,
                projectID: candidate.projectID,
                groupID: candidate.groupID,
                shells: shown,
                liveCount: candidate.liveShells.count,
                canOpen: !candidate.liveShells.isEmpty || allowsCreate
            )
        }

        let listed = visibleShells.filter(shellPasses)
        let groups = catalog.sections(for: listed)

        var projectHits: [ProjectHit] = []
        if !needle.isEmpty {
            projectHits = projects.compactMap { project in
                guard !project.isArchived else { return nil }
                let group = catalog.group(for: project.id)
                guard inGroup(group.id) else { return nil }
                let haystack = [project.name, project.path ?? "", catalog.hasHostGroups ? group.name : ""]
                guard haystack.contains(where: { $0.lowercased().contains(needle) }) else { return nil }
                let running = allShells.filter { $0.projectID == project.id }.count
                return ProjectHit(
                    id: project.id,
                    name: catalog.projectName(project.id),
                    path: project.path,
                    groupName: catalog.hasHostGroups ? group.name : nil,
                    runningCount: running
                )
            }
        }

        let groupOptions: [GroupOption] = catalog.orderedGroups.compactMap { group in
            let running = allShells.filter { $0.groupID == group.id }.count
            if group.id == ungroupedID && running == 0 && !catalog.hasUngroupedProjects {
                return nil
            }
            return GroupOption(id: group.id, name: group.name, runningCount: running)
        }

        return TerminalHubIndex(
            query: query,
            quick: quick,
            groups: groups,
            projectHits: projectHits,
            counts: counts,
            groupOptions: catalog.hasHostGroups ? groupOptions : [],
            showsGroupHeaders: catalog.hasHostGroups,
            canCreate: allowsCreate
        )
    }

    /// The project a quick terminal targets; workspace targets have none.
    static func quickProjectID(_ target: QuickTerminalTargetWire) -> String? {
        switch target {
        case .projectRoot(let projectId, _), .worktree(let projectId, _, _):
            projectId.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenBlank
        case .workspace, .unknown:
            nil
        }
    }

    /// A host path with the home directory folded to `~`, so rows show the
    /// part that tells projects apart.
    static func displayPath(_ path: String?) -> String? {
        guard let path = path?.trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty else {
            return nil
        }
        for prefix in ["/Users/", "/home/"] where path.hasPrefix(prefix) {
            let rest = path.dropFirst(prefix.count)
            guard let slash = rest.firstIndex(of: "/") else { return "~" }
            return "~" + rest[slash...]
        }
        return path
    }

    private static func matches(_ shell: Shell, _ needle: String) -> Bool {
        guard !needle.isEmpty else { return true }
        return [
            shell.title,
            shell.cwd ?? "",
            shell.branch ?? "",
            shell.projectName ?? "",
            shell.groupName,
            shell.quickTitle ?? "",
        ].contains { $0.lowercased().contains(needle) }
    }

    // MARK: - Request

    /// Request that opens one listed shell.
    ///
    /// A shell a quick terminal owns is still a project/workspace PTY on the
    /// host, so it attaches like any other listed terminal.
    static func request(for shell: Shell) -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .workspace(terminalID: shell.id),
            title: shell.title,
            cwd: shell.cwd,
            projectID: shell.projectID,
            projectName: shell.projectName
        )
    }
}

// MARK: - Catalog

/// Project and group lookup in host order.
nonisolated private struct Catalog {
    struct GroupRef {
        let id: String
        let name: String
        let projectIDs: [String]
    }

    let projects: [HostProject]
    let orderedGroups: [GroupRef]
    let hasHostGroups: Bool
    let hasUngroupedProjects: Bool
    private let groupByProject: [String: GroupRef]
    private let names: [String: String]
    private let ungrouped: GroupRef

    init(projects: [HostProject], groups: [HostProjectGroup]) {
        self.projects = projects
        var names: [String: String] = [:]
        for project in projects where names[project.id] == nil {
            let name = project.name.trimmingCharacters(in: .whitespacesAndNewlines)
            names[project.id] = name.isEmpty ? project.id : name
        }
        self.names = names

        var byProject: [String: GroupRef] = [:]
        var refs: [GroupRef] = []
        for group in groups {
            let name = group.name.trimmingCharacters(in: .whitespacesAndNewlines)
            let ref = GroupRef(id: group.id, name: name.isEmpty ? group.id : name, projectIDs: group.projectIds)
            refs.append(ref)
            // A project listed in two groups belongs to the first one.
            for id in group.projectIds where byProject[id] == nil {
                byProject[id] = ref
            }
        }
        let ungroupedProjectIDs = projects.map(\.id).filter { byProject[$0] == nil }
        let ungrouped = GroupRef(
            id: TerminalHubIndex.ungroupedID,
            name: TerminalHubIndex.ungroupedName,
            projectIDs: ungroupedProjectIDs
        )
        self.ungrouped = ungrouped
        self.groupByProject = byProject
        self.hasHostGroups = !groups.isEmpty
        self.hasUngroupedProjects = !ungroupedProjectIDs.isEmpty
        self.orderedGroups = refs + [ungrouped]
    }

    func group(for projectID: String?) -> GroupRef {
        guard let projectID, let group = groupByProject[projectID] else { return ungrouped }
        return group
    }

    func projectName(_ id: String) -> String {
        names[id] ?? id
    }

    /// Group → project → shells, in host order. Unknown project ids follow
    /// the catalog; shells with no project close the ungrouped bucket.
    func sections(for shells: [TerminalHubIndex.Shell]) -> [TerminalHubIndex.GroupSection] {
        var byProject: [String: [TerminalHubIndex.Shell]] = [:]
        var workspace: [TerminalHubIndex.Shell] = []
        for shell in shells {
            if let projectID = shell.projectID {
                byProject[projectID, default: []].append(shell)
            } else {
                workspace.append(shell)
            }
        }

        let catalogOrder = projects.map(\.id)
        var sections: [TerminalHubIndex.GroupSection] = []
        var placed = Set<String>()
        for group in orderedGroups {
            var order = group.projectIDs
            if group.id == TerminalHubIndex.ungroupedID {
                // Projects the catalog does not know still have shells.
                let unknown = byProject.keys.filter { names[$0] == nil }.sorted()
                order = catalogOrder.filter { groupByProject[$0] == nil } + unknown
            }
            var projectSections: [TerminalHubIndex.ProjectSection] = []
            for projectID in order where !placed.contains(projectID) {
                guard self.group(for: projectID).id == group.id,
                      let list = byProject[projectID], !list.isEmpty else { continue }
                placed.insert(projectID)
                projectSections.append(.init(
                    id: projectID,
                    name: projectName(projectID),
                    projectID: projectID,
                    shells: list
                ))
            }
            if group.id == TerminalHubIndex.ungroupedID, !workspace.isEmpty {
                projectSections.append(.init(
                    id: TerminalHubIndex.workspaceSectionID,
                    name: TerminalHubIndex.workspaceName,
                    projectID: nil,
                    shells: workspace
                ))
            }
            if !projectSections.isEmpty {
                sections.append(.init(id: group.id, name: group.name, projects: projectSections))
            }
        }
        return sections
    }
}

/// Copy for the terminal hub lives in its own string table, so this surface
/// ships its translations without touching the shared catalog.
nonisolated func hubText(_ key: String.LocalizationValue) -> String {
    String(localized: key, table: "TerminalHub")
}

private extension String {
    nonisolated var nilWhenBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
