import Foundation

/// What the new-terminal sheet lists: recent projects first, then the host's
/// project groups (or one flat list when the host defines none). A search
/// flattens everything into one ranked list. Pure value for tests.
nonisolated enum NewTerminalCatalog {
    struct Item: Identifiable, Equatable, Sendable {
        let id: String
        let name: String
        let path: String?
        let groupName: String?
        let runningCount: Int
    }

    struct Section: Identifiable, Equatable, Sendable {
        enum Kind: Equatable, Sendable { case recent, group, all, results }
        let id: String
        let kind: Kind
        let title: String
        let items: [Item]
    }

    static let recentLimit = 3

    static func sections(
        projects: [HostProject],
        groups: [HostProjectGroup],
        recentProjectIDs: [String],
        runningByProject: [String: Int] = [:],
        query: String = ""
    ) -> [Section] {
        let live = projects.filter { !$0.isArchived }
        var groupName: [String: String] = [:]
        for group in groups {
            for id in group.projectIds where groupName[id] == nil {
                groupName[id] = group.name
            }
        }
        let item: (HostProject) -> Item = { project in
            let name = project.name.trimmingCharacters(in: .whitespacesAndNewlines)
            return Item(
                id: project.id,
                name: name.isEmpty ? project.id : name,
                path: project.path,
                groupName: groupName[project.id],
                runningCount: runningByProject[project.id] ?? 0
            )
        }

        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if !needle.isEmpty {
            let hits = live.map(item).filter { candidate in
                [candidate.name, candidate.path ?? "", candidate.groupName ?? ""]
                    .contains { $0.lowercased().contains(needle) }
            }
            // Name hits outrank path or group hits; host order breaks ties.
            let ranked = hits.enumerated().sorted { lhs, rhs in
                let l = lhs.element.name.lowercased().contains(needle)
                let r = rhs.element.name.lowercased().contains(needle)
                return l != r ? l : lhs.offset < rhs.offset
            }.map(\.element)
            return ranked.isEmpty ? [] : [Section(id: "results", kind: .results, title: "", items: ranked)]
        }

        var sections: [Section] = []
        let byID = Dictionary(live.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let recent = recentProjectIDs.compactMap { byID[$0] }.prefix(recentLimit).map(item)
        if !recent.isEmpty {
            sections.append(Section(id: "recent", kind: .recent, title: hubText("Recent"), items: Array(recent)))
        }

        if groups.isEmpty {
            if !live.isEmpty {
                sections.append(Section(id: "all", kind: .all, title: hubText("Projects"), items: live.map(item)))
            }
            return sections
        }

        var placed = Set<String>()
        for group in groups {
            let items = group.projectIds.compactMap { id -> Item? in
                guard let project = byID[id], placed.insert(id).inserted else { return nil }
                return item(project)
            }
            if !items.isEmpty {
                sections.append(Section(id: "group-\(group.id)", kind: .group, title: group.name, items: items))
            }
        }
        let rest = live.filter { !placed.contains($0.id) }.map(item)
        if !rest.isEmpty {
            sections.append(Section(id: "ungrouped", kind: .group, title: TerminalHubIndex.ungroupedName, items: rest))
        }
        return sections
    }
}

/// Projects the user recently started a terminal in, per host. Identity only:
/// names and paths are always re-read from the host catalog.
nonisolated enum TerminalRecentProjects {
    static let limit = 8

    static func key(origin: URL) -> String {
        "terminal.recentProjects.\(origin.absoluteString)"
    }

    static func load(origin: URL, defaults: UserDefaults = .standard) -> [String] {
        defaults.stringArray(forKey: key(origin: origin)) ?? []
    }

    static func record(_ projectID: String, origin: URL, defaults: UserDefaults = .standard) {
        let ids = updated(load(origin: origin, defaults: defaults), with: projectID)
        defaults.set(ids, forKey: key(origin: origin))
    }

    static func updated(_ ids: [String], with projectID: String) -> [String] {
        let trimmed = projectID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return ids }
        return Array(([trimmed] + ids.filter { $0 != trimmed }).prefix(limit))
    }
}
