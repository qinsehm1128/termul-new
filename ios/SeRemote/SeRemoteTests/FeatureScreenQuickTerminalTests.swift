import Foundation
import Testing
@testable import SeRemote

struct FeatureScreenQuickTerminalTests {
    @Test func conversationNeverUsesActiveProject() {
        #expect(QuickTerminalPresentation.creationTargets(
            isProjectSession: false, projectId: "active-project", projectRoot: "/host/project"
        ) == [.workspace])
    }

    @Test func projectRequiresCompleteActiveProjectAndPreservesExactHostPath() {
        #expect(QuickTerminalPresentation.creationTargets(
            isProjectSession: true, projectId: "p1", projectRoot: "/host/project with spaces/"
        ) == [.workspace, .projectRoot(projectId: "p1", projectRoot: "/host/project with spaces/")])
        for root: String? in [nil, "", " \n "] {
            #expect(QuickTerminalPresentation.creationTargets(
                isProjectSession: true, projectId: "p1", projectRoot: root
            ) == [.workspace])
        }
        for id: String? in [nil, "", "\t"] {
            #expect(QuickTerminalPresentation.creationTargets(
                isProjectSession: true, projectId: id, projectRoot: "/host/project"
            ) == [.workspace])
        }
    }

    @Test func noCreationTargetCanCreateWorktree() {
        for isProjectSession in [true, false] {
            let targets = QuickTerminalPresentation.creationTargets(
                isProjectSession: isProjectSession, projectId: "p1", projectRoot: "/host/project"
            )
            for target in targets {
                switch target {
                case .workspace, .projectRoot: break
                case .worktree, .unknown: Issue.record("Unsafe quick terminal creation target")
                }
            }
        }
    }

    @Test func legacyIdsAreCountedOnceAndEmptyIdsIgnored() throws {
        let record = try fixture(title: " Shell ", terminalIds: ["t2", "t1", "t2", ""], terminalId: "t1")
        #expect(QuickTerminalPresentation.liveTerminalIds(record) == ["t2", "t1"])
        #expect(QuickTerminalPresentation.title(record) == "Shell")
        #expect(QuickTerminalPresentation.liveTerminalIds(try fixture(terminalId: "legacy")) == ["legacy"])
        #expect(QuickTerminalPresentation.liveTerminalIds(try fixture()).isEmpty)
    }

    @Test func absentAndWhitespaceTitleGetReadableFallback() throws {
        let absent = QuickTerminalPresentation.title(try fixture())
        #expect(!absent.isEmpty)
        #expect(QuickTerminalPresentation.title(try fixture(title: " \n\t")) == absent)
    }

    @Test func existingWorktreeAndUnknownKindsAreDisplayOnly() {
        let worktree = QuickTerminalTargetWire.worktree(projectId: "p", worktreePath: "/host/wt", worktreeBranch: "feature/x")
        #expect(!QuickTerminalPresentation.targetLabel(worktree).isEmpty)
        #expect(QuickTerminalPresentation.targetDetail(worktree) == "p · feature/x")
        #expect(QuickTerminalPresentation.targetDetail(.projectRoot(projectId: "p", projectRoot: "/host/p")) == "p")
        #expect(QuickTerminalPresentation.targetDetail(.workspace) == nil)
        #expect(!QuickTerminalPresentation.targetLabel(.unknown("future_scope")).isEmpty)
        #expect(QuickTerminalPresentation.targetDetail(.unknown("future_scope")) == "future_scope")
    }

    private func fixture(title: String? = nil, terminalIds: [String] = [], terminalId: String? = nil) throws -> QuickTerminalRecordWire {
        var json: [String: Any] = [
            "schemaVersion": 1, "id": "quick-1", "target": ["kind": "workspace"],
            "cwd": "/host/workspaces/quick-1", "createdAtUtc": "2026-11-09T00:00:00Z",
            "updatedAtUtc": "2026-11-09T00:00:00Z", "origin": "created", "terminalIds": terminalIds
        ]
        if let title { json["title"] = title }
        if let terminalId { json["terminalId"] = terminalId }
        return try JSONDecoder().decode(QuickTerminalRecordWire.self, from: JSONSerialization.data(withJSONObject: json))
    }
}
