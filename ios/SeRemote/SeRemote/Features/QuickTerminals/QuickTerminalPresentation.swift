import Foundation

/// Pure presentation and target policy; never adopts a host PTY into a workspace.
nonisolated enum QuickTerminalPresentation {
    static func creationTargets(isProjectSession: Bool, projectId: String?, projectRoot: String?) -> [QuickTerminalTargetWire] {
        guard isProjectSession,
              let projectId, !projectId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let projectRoot, !projectRoot.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return [.workspace]
        }
        // Use the active project's exact host path, not a guessed local/worktree path.
        return [.workspace, .projectRoot(projectId: projectId, projectRoot: projectRoot)]
    }

    static func title(_ record: QuickTerminalRecordWire) -> String {
        let title = record.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? String(localized: "Untitled quick terminal") : title
    }

    static func targetLabel(_ target: QuickTerminalTargetWire) -> String {
        switch target {
        case .workspace: String(localized: "Host workspace")
        case .projectRoot: String(localized: "Project root")
        case .worktree: String(localized: "Existing worktree")
        case .unknown: String(localized: "Unknown target")
        }
    }

    static func targetDetail(_ target: QuickTerminalTargetWire) -> String? {
        switch target {
        case .workspace: nil
        case .projectRoot(let projectId, _): projectId
        case .worktree(let projectId, _, let branch): "\(projectId) · \(branch)"
        case .unknown(let kind): kind
        }
    }

    static func liveTerminalIds(_ record: QuickTerminalRecordWire) -> [String] {
        var seen = Set<String>()
        return (record.terminalIds + [record.terminalId].compactMap { $0 })
            .filter { !$0.isEmpty && seen.insert($0).inserted }
    }
}
