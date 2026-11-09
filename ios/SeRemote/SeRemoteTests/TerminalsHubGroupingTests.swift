import Foundation
import Testing
@testable import SeRemote

/// Pure grouping/title rules for the 终端 hub. No session, no socket.
@Suite("TerminalsHub grouping")
struct TerminalsHubGroupingTests {
    private func terminal(
        _ id: String,
        title: String = "shell",
        projectId: String? = nil,
        branch: String? = nil,
        cwd: String? = nil,
        agent: Bool = false
    ) -> TerminalsHubTerminal {
        TerminalsHubTerminal(
            id: id,
            title: title,
            cwd: cwd,
            branch: branch,
            projectId: projectId,
            hasRunningAgent: agent
        )
    }

    private func summary(
        _ id: String,
        projectId: String? = nil,
        title: String? = nil,
        cwd: String? = nil,
        branch: String? = nil
    ) -> LiveTerminalSummary {
        LiveTerminalSummary(
            id: id,
            shell: "/bin/zsh",
            cwd: cwd,
            pid: 1,
            cols: 80,
            rows: 24,
            terminalScreenEngine: nil,
            conversationId: nil,
            projectId: projectId,
            title: title,
            gitBranch: branch,
            screenObservation: nil
        )
    }

    @Test("TerminalsHub groups terminals by project in catalog order")
    func terminalsHubGroupsByProjectInCatalogOrder() {
        let groups = TerminalsHubPresentation.groups(
            terminals: [
                terminal("t1", projectId: "b"),
                terminal("t2", projectId: "a"),
                terminal("t3", projectId: "a"),
            ],
            projects: [
                TerminalsHubProject(id: "a", name: "Termul"),
                TerminalsHubProject(id: "b", name: "box-aggregator"),
                TerminalsHubProject(id: "c", name: "Qscode"),
            ]
        )
        #expect(groups.map(\.name) == ["Termul", "box-aggregator"])
        #expect(groups.map(\.count) == [2, 1])
        #expect(groups.allSatisfy { !$0.isOther })
        // Catalog order wins, and terminal order inside a group is host order.
        #expect(groups[0].terminals.map(\.id) == ["t2", "t3"])
    }

    @Test("TerminalsHub puts project-less terminals into Other last")
    func terminalsHubPutsProjectlessTerminalsInOtherLast() {
        let groups = TerminalsHubPresentation.groups(
            terminals: [
                terminal("t1", projectId: "a"),
                terminal("orphan-1"),
                terminal("orphan-2", projectId: "   "),
            ],
            projects: [TerminalsHubProject(id: "a", name: "Termul")]
        )
        #expect(groups.count == 2)
        #expect(groups[0].name == "Termul")
        #expect(groups[0].count == 1)
        #expect(groups[1].id == TerminalsHubPresentation.otherGroupId)
        #expect(groups[1].isOther)
        #expect(groups[1].name == TerminalsHubPresentation.otherGroupName)
        #expect(groups[1].terminals.map(\.id) == ["orphan-1", "orphan-2"])
    }

    @Test("TerminalsHub labels a project missing from the catalog with its host id")
    func terminalsHubLabelsUnknownProjectWithHostId() {
        let groups = TerminalsHubPresentation.groups(
            terminals: [terminal("t1", projectId: "unlisted")],
            projects: [TerminalsHubProject(id: "a", name: "Termul")]
        )
        #expect(groups.map(\.id) == ["unlisted"])
        #expect(groups[0].name == "unlisted")
        #expect(groups[0].count == 1)
    }

    @Test("TerminalsHub omits empty groups and never invents one")
    func terminalsHubOmitsEmptyGroups() {
        let groups = TerminalsHubPresentation.groups(
            terminals: [],
            projects: [
                TerminalsHubProject(id: "a", name: "Termul"),
                TerminalsHubProject(id: "b", name: "Qscode"),
            ]
        )
        #expect(groups.isEmpty)
    }

    @Test("TerminalsHub excludes quick-terminal shells from project groups")
    func terminalsHubExcludesQuickTerminalShells() {
        let groups = TerminalsHubPresentation.groups(
            terminals: [
                terminal("pty-quick", projectId: "a"),
                terminal("pty-project", projectId: "a"),
            ],
            projects: [TerminalsHubProject(id: "a", name: "Termul")],
            excluding: ["pty-quick"]
        )
        #expect(groups.count == 1)
        #expect(groups[0].terminals.map(\.id) == ["pty-project"])
    }

    @Test("TerminalsHub marks only terminals the host reports an agent on")
    func terminalsHubMarksRunningAgentsByTerminalId() {
        let running: Set<String> = ["t2"]
        let mapped = [
            summary("t1", projectId: "a"),
            summary("t2", projectId: "a"),
        ].map {
            TerminalsHubPresentation.terminal(from: $0, runningAgentTerminalIds: running)
        }
        #expect(mapped[0].hasRunningAgent == false)
        #expect(mapped[1].hasRunningAgent == true)
    }

    @Test("TerminalsHub maps host attribution and branch onto the row")
    func terminalsHubMapsSummaryAttribution() {
        let mapped = TerminalsHubPresentation.terminal(
            from: summary("t1", projectId: "p1", title: "termul", cwd: "/repo/termul", branch: "main"),
            runningAgentTerminalIds: []
        )
        #expect(mapped.id == "t1")
        #expect(mapped.title == "termul")
        #expect(mapped.projectId == "p1")
        #expect(mapped.branch == "main")
        #expect(mapped.cwd == "/repo/termul")
        #expect(mapped.hasRunningAgent == false)
    }

    @Test("TerminalsHub title falls back from host title to cwd tail to shell")
    func terminalsHubTitleFallbacks() {
        #expect(TerminalsHubPresentation.title(hostTitle: "host-title", cwd: "/repo/x", shell: "zsh") == "host-title")
        #expect(TerminalsHubPresentation.title(hostTitle: "   ", cwd: "/repo/x/", shell: "zsh") == "x")
        #expect(TerminalsHubPresentation.title(hostTitle: nil, cwd: nil, shell: "zsh") == "zsh")
        #expect(TerminalsHubPresentation.title(hostTitle: nil, cwd: "/", shell: nil) == String(localized: "Terminal"))
    }

    @Test("TerminalsHub merges per-project pages without duplicate ids")
    func terminalsHubMergesPagesWithoutDuplicates() {
        let merged = TerminalsHubPresentation.merged([
            [summary("a"), summary("b")],
            [summary("b"), summary("c")],
        ])
        #expect(merged.map(\.id) == ["a", "b", "c"])
    }

    @Test("TerminalsHub falls back only on the legacy VALIDATION_ERROR list")
    func terminalsHubFallbackIsValidationOnly() {
        #expect(
            TerminalsHubPresentation.shouldFallBackToListPerProject(
                HostError.ipc("list requires conversationId or projectId", "VALIDATION_ERROR")
            )
        )
        #expect(
            !TerminalsHubPresentation.shouldFallBackToListPerProject(
                HostError.ipc("Unauthorized", "UNAUTHORIZED")
            )
        )
        #expect(
            !TerminalsHubPresentation.shouldFallBackToListPerProject(
                HostError.network("offline")
            )
        )
    }

    @Test("TerminalsHub fallback targets only projects the host says are running")
    func terminalsHubFallbackTargetsRunningProjects() throws {
        let projects = try JSONDecoder().decode([HostProject].self, from: Data(#"""
        [
          {"id":"p1","name":"A","liveTerminalCount":2},
          {"id":"p2","name":"B","liveTerminalCount":0},
          {"id":"p3","name":"C","liveTerminalCount":1,"isArchived":true}
        ]
        """#.utf8))
        #expect(TerminalsHubPresentation.fallbackProjectIds(projects) == ["p1"])
    }

    @Test("TerminalsHub reads quick-terminal shell ids from host records")
    func terminalsHubReadsQuickTerminalShellIds() throws {
        let records = try JSONDecoder().decode([QuickTerminalRecordWire].self, from: Data(#"""
        [
          {"schemaVersion":2,"id":"qt-1","target":{"kind":"workspace"},"cwd":"/w","createdAtUtc":"t1","updatedAtUtc":"t2",
           "terminalId":"pty-1","terminalIds":["pty-1","pty-2"],"origin":"created"},
          {"schemaVersion":2,"id":"qt-2","target":{"kind":"workspace"},"cwd":"/w","createdAtUtc":"t1","updatedAtUtc":"t2",
           "terminalId":"pty-2","origin":"created"}
        ]
        """#.utf8))
        #expect(TerminalsHubPresentation.quickTerminalIds(records) == ["pty-1", "pty-2"])
    }
}
