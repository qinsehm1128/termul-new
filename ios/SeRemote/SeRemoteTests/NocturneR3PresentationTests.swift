import Foundation
import Testing
@testable import SeRemote

/// R3 presentation contract: the rewritten screens only surface host data, and
/// the search/fopen guard never escapes the workspace root.
@MainActor
struct NocturneR3PresentationTests {
    @Test func toolMetricFlagIsTrueOnlyWhenHostReportedTheCount() {
        let empty = NocturneWorkspaceSnapshot()
        #expect(NocturneWorkspacePresentation.tool(.git, snapshot: empty).metricIsLoaded == false)
        #expect(NocturneWorkspacePresentation.tool(.scheduledTasks, snapshot: empty).metricIsLoaded == false)
        // Quick terminals are no longer a hub tile, so they report no metric.
        #expect(ToolDestination.allCases.count == 5)

        let loaded = NocturneWorkspaceSnapshot(gitChangedFileCount: 0, scheduledTaskCount: 4)
        #expect(NocturneWorkspacePresentation.tool(.git, snapshot: loaded).metricIsLoaded)
        #expect(NocturneWorkspacePresentation.tool(.scheduledTasks, snapshot: loaded).metricIsLoaded)
    }

    @Test func genuineZeroIsStillAHostMetric() {
        let snapshot = NocturneWorkspaceSnapshot(gitChangedFileCount: 0)
        let tile = NocturneWorkspacePresentation.tool(.git, snapshot: snapshot)
        #expect(tile.metricIsLoaded)
        #expect(tile.detail == String(localized: "\(0) changed files"))
    }

    @Test func gitBranchAndCountsStayScopedToTheMatchingProject() throws {
        let snapshot = NocturneWorkspaceSnapshot(gitCwd: "/work/app", gitBranch: "main", gitChangedFileCount: 2)
        let match = NocturneWorkspacePresentation.project(try project(path: "/work/app"), snapshot: snapshot)
        #expect(match.branch == "main")
        #expect(match.hasChanges)

        let other = NocturneWorkspacePresentation.project(try project(path: "/work/other"), snapshot: snapshot)
        #expect(other.branch.isEmpty)
        #expect(!other.hasChanges)
    }

    @Test func searchRootContainmentAcceptsRootAndChildren() {
        #expect(SearchFeatureView.isInsideRoot("/work/app", root: "/work/app"))
        #expect(SearchFeatureView.isInsideRoot("/work/app/src/main.swift", root: "/work/app"))
        #expect(SearchFeatureView.isInsideRoot("/work/app", root: "/work/app/"))
    }

    @Test func searchRootContainmentRejectsPrefixSiblingsAndParents() {
        #expect(!SearchFeatureView.isInsideRoot("/work/apple/main.swift", root: "/work/app"))
        #expect(!SearchFeatureView.isInsideRoot("/work", root: "/work/app"))
        #expect(!SearchFeatureView.isInsideRoot("/etc/hosts", root: "/work/app"))
        #expect(!SearchFeatureView.isInsideRoot("/work/app/../secret", root: "/work/app"))
    }

    @Test func diffPrefixAndCountsFollowUnifiedDiffKinds() {
        let diff = UnifiedDiff.parse("""
        diff --git a/src/a.swift b/src/a.swift
        --- a/src/a.swift
        +++ b/src/a.swift
        @@ -1,2 +1,3 @@
         context
        -removed
        +added one
        +added two
        """)
        #expect(diff.files.count == 1)
        let counts = GitDiffPresentation.counts(diff)
        #expect(counts.added == 2)
        #expect(counts.removed == 1)
        let kinds = diff.files[0].hunks[0].lines.map(\.kind)
        #expect(kinds == [.context, .removal, .addition, .addition])
        #expect(GitDiffPresentation.prefix(for: diff.files[0].hunks[0].lines[1]) == "−")
        #expect(GitDiffPresentation.prefix(for: diff.files[0].hunks[0].lines[2]) == "+")
    }

    @Test func quickTitleAndLiveIdsIgnoreBlankHostValues() throws {
        let record = try quickRecord(title: "  ", terminalIds: ["a", "a", ""], terminalId: "b")
        #expect(QuickTerminalPresentation.title(record) == String(localized: "Untitled quick terminal"))
        #expect(QuickTerminalPresentation.liveTerminalIds(record) == ["a", "b"])
    }

    private func project(path: String?) throws -> HostProject {
        var object: [String: Any] = ["id": "app", "name": "Application", "liveTerminalCount": 0]
        if let path { object["path"] = path }
        return try JSONDecoder().decode(HostProject.self, from: JSONSerialization.data(withJSONObject: object))
    }

    private func quickRecord(title: String?, terminalIds: [String], terminalId: String?) throws -> QuickTerminalRecordWire {
        var object: [String: Any] = [
            "schemaVersion": 1, "id": "q1", "target": ["kind": "workspace"],
            "cwd": "/host/q1", "createdAtUtc": "2026-11-09T00:00:00Z",
            "updatedAtUtc": "2026-11-09T00:00:00Z", "origin": "created", "terminalIds": terminalIds
        ]
        if let title { object["title"] = title }
        if let terminalId { object["terminalId"] = terminalId }
        return try JSONDecoder().decode(QuickTerminalRecordWire.self,
                                        from: JSONSerialization.data(withJSONObject: object))
    }
}
