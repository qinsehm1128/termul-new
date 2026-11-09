import Foundation

/// One live host terminal, reduced to what the 终端 hub renders.
///
/// `conversationId` is deliberately not stored: a project terminal carries a
/// process-local scope on the host, and that is not an ownership identity the
/// phone may show.
nonisolated struct TerminalsHubTerminal: Identifiable, Hashable, Sendable {
    let id: String
    var title: String
    var cwd: String?
    var branch: String?
    var projectId: String?
    var hasRunningAgent: Bool
}

/// A project the host catalog knows about, minus decoding concerns.
nonisolated struct TerminalsHubProject: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
}

/// One rendered group: a project, an unknown project id, or 其他.
nonisolated struct TerminalsHubGroup: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    var terminals: [TerminalsHubTerminal]
    var isOther: Bool

    var count: Int { terminals.count }
}

/// Pure grouping and title policy. No host state and no SwiftUI, so the rules
/// are unit-testable without a session.
nonisolated enum TerminalsHubPresentation {
    /// Terminals with no project never fabricate one; they are grouped here.
    static let otherGroupId = "__terminals_hub_other__"

    static var otherGroupName: String { String(localized: "Other") }

    /// Map a host summary. `title` is the host's cwd-basename (or shell)
    /// fallback; when an older host omits it, the cwd tail is used instead.
    static func terminal(
        from summary: LiveTerminalSummary,
        runningAgentTerminalIds: Set<String>
    ) -> TerminalsHubTerminal {
        TerminalsHubTerminal(
            id: summary.id,
            title: title(
                hostTitle: summary.title,
                cwd: summary.cwd,
                shell: summary.shell
            ),
            cwd: summary.cwd,
            branch: summary.gitBranch?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenEmpty,
            projectId: summary.projectId?.trimmingCharacters(in: .whitespacesAndNewlines).nilWhenEmpty,
            hasRunningAgent: runningAgentTerminalIds.contains(summary.id)
        )
    }

    /// Group by project id in catalog order, unknown project ids after the
    /// catalog, and 其他 last. `excluded` ids are terminals the quick-terminal
    /// list already renders (the same PTY must not appear twice).
    static func groups(
        terminals: [TerminalsHubTerminal],
        projects: [TerminalsHubProject],
        excluding excluded: Set<String> = []
    ) -> [TerminalsHubGroup] {
        var buckets: [String: [TerminalsHubTerminal]] = [:]
        var other: [TerminalsHubTerminal] = []
        for terminal in terminals where !excluded.contains(terminal.id) {
            guard let projectId = terminal.projectId, !projectId.isEmpty else {
                other.append(terminal)
                continue
            }
            buckets[projectId, default: []].append(terminal)
        }

        var groups: [TerminalsHubGroup] = []
        var known = Set<String>()
        for project in projects {
            known.insert(project.id)
            guard let terminals = buckets.removeValue(forKey: project.id), !terminals.isEmpty else {
                continue
            }
            let name = project.name.trimmingCharacters(in: .whitespacesAndNewlines)
            groups.append(
                TerminalsHubGroup(
                    id: project.id,
                    name: name.nilWhenEmpty ?? project.id,
                    terminals: terminals,
                    isOther: false
                )
            )
        }

        // A live terminal whose project is missing from the catalog still has
        // a project; label it with the id the host reported, never a guess.
        for projectId in buckets.keys.sorted() where !known.contains(projectId) {
            guard let terminals = buckets[projectId], !terminals.isEmpty else { continue }
            groups.append(
                TerminalsHubGroup(
                    id: projectId,
                    name: projectId,
                    terminals: terminals,
                    isOther: false
                )
            )
        }

        if !other.isEmpty {
            groups.append(
                TerminalsHubGroup(
                    id: otherGroupId,
                    name: otherGroupName,
                    terminals: other,
                    isOther: true
                )
            )
        }
        return groups
    }

    /// Display title: the host title, then the cwd tail, then the shell.
    static func title(hostTitle: String?, cwd: String?, shell: String?) -> String {
        if let hostTitle = hostTitle?.trimmingCharacters(in: .whitespacesAndNewlines),
           !hostTitle.isEmpty {
            return hostTitle
        }
        if let tail = cwdTail(cwd) {
            return tail
        }
        if let shell = shell?.trimmingCharacters(in: .whitespacesAndNewlines), !shell.isEmpty {
            return shell
        }
        return String(localized: "Terminal")
    }

    /// The last path component, never the empty string and never `/`.
    static func cwdTail(_ cwd: String?) -> String? {
        guard var path = cwd?.trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty else {
            return nil
        }
        while path.count > 1 && path.hasSuffix("/") {
            path.removeLast()
        }
        let tail = (path as NSString).lastPathComponent
        guard !tail.isEmpty, tail != "/" else { return nil }
        return tail
    }

    /// Old hosts reject an unfiltered `list` with VALIDATION_ERROR; the hub
    /// then lists once per project that reports live terminals.
    static func shouldFallBackToListPerProject(_ error: Error) -> Bool {
        if case HostError.ipc(_, let code) = error {
            return code == "VALIDATION_ERROR"
        }
        return false
    }

    /// Projects the fallback may ask for: only those the host says are running.
    static func fallbackProjectIds(_ projects: [HostProject]) -> [String] {
        projects
            .filter { $0.liveTerminalCount > 0 && !$0.isArchived }
            .map(\.id)
    }

    /// Merge per-project pages without duplicating a terminal id.
    static func merged(_ pages: [[LiveTerminalSummary]]) -> [LiveTerminalSummary] {
        var seen = Set<String>()
        var merged: [LiveTerminalSummary] = []
        for page in pages {
            for summary in page where seen.insert(summary.id).inserted {
                merged.append(summary)
            }
        }
        return merged
    }

    /// Live-terminal ids a quick-terminal record owns, so the hub's quick
    /// group and project groups never show the same shell twice.
    static func quickTerminalIds(_ records: [QuickTerminalRecordWire]) -> Set<String> {
        Set(records.flatMap(QuickTerminalPresentation.liveTerminalIds))
    }
}

private extension String {
    nonisolated var nilWhenEmpty: String? {
        isEmpty ? nil : self
    }
}
