import Foundation
import Testing
@testable import SeRemote

@MainActor
struct NocturneWorkspacePresentationTests {
    @Test func unknownCountsUseDescriptionsInsteadOfFakeZero() {
        let snapshot = NocturneWorkspaceSnapshot()
        for destination in ToolDestination.allCases {
            let tile = NocturneWorkspacePresentation.tool(destination, snapshot: snapshot)
            #expect(tile.id == destination.rawValue)
            #expect(tile.title == destination.title)
            #expect(tile.detail == destination.detail)
            #expect(tile.icon == destination.icon)
            // Only the screen tool may leave the note blank: the hub snapshot
            // carries no screen state to summarize.
            #expect(tile.note.isEmpty == (destination == .screenText))
        }
    }

    @Test func noToolInventsAViewOnlyState() {
        let snapshot = NocturneWorkspaceSnapshot()
        let viewOnly = String(localized: "View only")
        for destination in ToolDestination.allCases {
            let tile = NocturneWorkspacePresentation.tool(destination, snapshot: snapshot)
            #expect(tile.note != viewOnly)
        }
    }

    @Test func loadedCountsIncludeGenuineZero() {
        let snapshot = NocturneWorkspaceSnapshot(gitChangedFileCount: 0, scheduledTaskCount: 4)
        #expect(NocturneWorkspacePresentation.tool(.git, snapshot: snapshot).detail == String(localized: "\(0) changed files"))
        #expect(NocturneWorkspacePresentation.tool(.scheduledTasks, snapshot: snapshot).detail == String(localized: "\(4) tasks"))
    }

    @Test func projectUsesReportedNamePathAndLiveCount() throws {
        let project = try makeProject(path: "/work/app", liveCount: 3)
        let row = NocturneWorkspacePresentation.project(project, snapshot: NocturneWorkspaceSnapshot())
        #expect(row.id == project.id)
        #expect(row.name == project.name)
        #expect(row.path == "/work/app")
        #expect(row.detail == String(localized: "\(3) live terminals"))
        #expect(row.branch.isEmpty)
        #expect(!row.hasChanges)
    }

    @Test func gitMetadataOnlyAppearsForMatchingProject() throws {
        let snapshot = NocturneWorkspaceSnapshot(gitCwd: "/work/app", gitBranch: "feature", gitChangedFileCount: 2)
        let matching = NocturneWorkspacePresentation.project(try makeProject(path: "/work/app"), snapshot: snapshot)
        #expect(matching.branch == "feature")
        #expect(matching.hasChanges)
        #expect(matching.detail.contains(String(localized: "\(2) changed files")))
        let other = NocturneWorkspacePresentation.project(try makeProject(path: "/work/other"), snapshot: snapshot)
        #expect(other.branch.isEmpty)
        #expect(!other.hasChanges)
        #expect(other.detail == String(localized: "\(0) live terminals"))
    }

    @Test func missingPathNeverMatchesMissingGitScope() throws {
        let snapshot = NocturneWorkspaceSnapshot(gitBranch: "wrong", gitChangedFileCount: 7)
        let row = NocturneWorkspacePresentation.project(try makeProject(path: nil), snapshot: snapshot)
        #expect(row.path.isEmpty)
        #expect(row.branch.isEmpty)
        #expect(!row.hasChanges)
    }

    @Test func rowTextOmitsUnknownBranchWithoutDanglingSeparator() {
        #expect(NocturneWorkspacePresentation.rowText(branch: "", detail: "3 live terminals") == "3 live terminals")
        #expect(NocturneWorkspacePresentation.rowText(branch: "main", detail: "3 live terminals") == "main · 3 live terminals")
        #expect(NocturneWorkspacePresentation.rowText(branch: "", detail: "") == "")
    }

    private func makeProject(path: String?, liveCount: Int = 0) throws -> HostProject {
        var object: [String: Any] = ["id": "app", "name": "Application", "liveTerminalCount": liveCount]
        if let path { object["path"] = path }
        return try JSONDecoder().decode(HostProject.self, from: JSONSerialization.data(withJSONObject: object))
    }
}
