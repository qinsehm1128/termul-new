import Foundation

/// Lifecycle is not agent activity. The host currently promises only ready/deleted;
/// preserve future values instead of inventing a running or approval state.
struct ShellHostLifecycle: Equatable, Sendable {
    enum Kind: Equatable, Sendable { case ready, deleted, unknown }
    let kind: Kind
    let label: String

    init(_ rawValue: String) {
        switch rawValue {
        case "ready":
            kind = .ready
            label = String(localized: "Ready")
        case "deleted":
            kind = .deleted
            label = String(localized: "Deleted")
        default:
            kind = .unknown
            label = rawValue.isEmpty ? String(localized: "Unknown") : String(localized: "Unknown (\(rawValue))")
        }
    }
}

struct ShellHostProjectSection: Identifiable {
    let id: String
    let title: String
    let projects: [HostProject]
}

enum ShellHostProjects {
    static func sections(projects: [HostProject], groups: [HostProjectGroup], query: String) -> [ShellHostProjectSection] {
        let visible = projects.filter { !$0.isArchived }
        let byID = Dictionary(visible.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var claimed = Set<String>()
        var result: [ShellHostProjectSection] = []
        let running = visible.filter(\.isRunning).sorted(by: byName)
        if !running.isEmpty {
            result.append(.init(id: "running", title: String(localized: "Running"), projects: running))
            claimed.formUnion(running.map(\.id))
        }
        for group in groups {
            let items = group.projectIds.compactMap { id -> HostProject? in
                guard !claimed.contains(id), let project = byID[id] else { return nil }
                claimed.insert(id)
                return project
            }
            if !items.isEmpty {
                result.append(.init(id: "group:\(group.id)", title: group.name, projects: items))
            }
        }
        let rest = visible.filter { !claimed.contains($0.id) }.sorted(by: byName)
        if !rest.isEmpty {
            result.append(.init(id: "ungrouped", title: String(localized: "Other projects"), projects: rest))
        }
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return result.compactMap { section in
            let matches = section.projects.filter {
                trimmed.isEmpty || $0.name.localizedCaseInsensitiveContains(trimmed)
                    || ($0.path ?? "").localizedCaseInsensitiveContains(trimmed)
            }
            return matches.isEmpty ? nil : .init(id: section.id, title: section.title, projects: matches)
        }
    }

    static func terminalCount(in projects: [HostProject]) -> Int {
        projects.filter { !$0.isArchived }.reduce(0) { $0 + max(0, $1.liveTerminalCount) }
    }

    private static func byName(_ lhs: HostProject, _ rhs: HostProject) -> Bool {
        lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
    }
}

/// Pure directory filters. The view binds these strings; tests can exercise
/// the same mapping without a host.
enum HostDirectoryPresentation {
    static let all = "all"
    static let ready = "ready"
    static let ended = "ended"

    static func conversations(_ all: [HostConversation], filter: String) -> [HostConversation] {
        switch filter {
        case ready:
            return all.filter { !$0.isDeleted && $0.lifecycleState == "ready" }
        case ended:
            return all.filter(\.isDeleted)
        default:
            return all.filter { !$0.isDeleted }
        }
    }

    static func readyCount(in conversations: [HostConversation]) -> Int {
        conversations.filter { !$0.isDeleted && $0.lifecycleState == "ready" }.count
    }

    static func endedCount(in conversations: [HostConversation]) -> Int {
        conversations.filter(\.isDeleted).count
    }
}
