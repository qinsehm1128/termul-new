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

    /// The claim the standalone console may present to the host's `attach` op.
    ///
    /// `open` mirrors the spawn claim once and never reissues it for a reused
    /// PTY; whitespace-only is absence, not a claim.
    static func attachClaim(_ opened: QuickTerminalOpenedWire) -> String? {
        let claim = opened.claim?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return claim.isEmpty ? nil : claim
    }

    /// Copy for a shell that exists but cannot be attached from this device
    /// because the host minted no claim for it. `nil` when attach can proceed.
    static func consoleUnattachableNotice(_ opened: QuickTerminalOpenedWire) -> String? {
        guard attachClaim(opened) == nil else { return nil }
        return String(localized: "This shell is already running on the host, but this device holds no claim for it, so it cannot be attached. The shell keeps running.")
    }

    /// Failure copy for the console: name the claim limitation when that is
    /// the real cause, otherwise keep the host's own message.
    static func consoleFailureMessage(_ opened: QuickTerminalOpenedWire, error: Error) -> String {
        consoleUnattachableNotice(opened) ?? error.localizedDescription
    }
}
