import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal hub index")
struct TerminalHubIndexTests {
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

    private func project(_ id: String, _ name: String, path: String? = nil, archived: Bool = false) throws -> HostProject {
        let pathField = path.map { ",\"path\":\"\($0)\"" } ?? ""
        return try JSONDecoder().decode(HostProject.self, from: Data("""
        {"id":"\(id)","name":"\(name)","liveTerminalCount":0,"isArchived":\(archived)\(pathField)}
        """.utf8))
    }

    private func group(_ id: String, _ name: String, _ projectIds: [String]) throws -> HostProjectGroup {
        let ids = projectIds.map { "\"\($0)\"" }.joined(separator: ",")
        return try JSONDecoder().decode(HostProjectGroup.self, from: Data("""
        {"id":"\(id)","name":"\(name)","projectIds":[\(ids)]}
        """.utf8))
    }

    private func quick(
        _ id: String,
        title: String,
        terminals: [String] = [],
        projectId: String? = nil
    ) throws -> QuickTerminalRecordWire {
        let target = projectId.map {
            "{\"kind\":\"project_root\",\"projectId\":\"\($0)\",\"projectRoot\":\"/work/\($0)\"}"
        } ?? "{\"kind\":\"workspace\"}"
        let ids = terminals.map { "\"\($0)\"" }.joined(separator: ",")
        return try JSONDecoder().decode(QuickTerminalRecordWire.self, from: Data("""
        {"schemaVersion":1,"id":"\(id)","title":"\(title)","target":\(target),"cwd":"/work",
         "createdAtUtc":"2026-11-10T00:00:00Z","updatedAtUtc":"2026-11-10T00:00:00Z",
         "terminalIds":[\(ids)],"origin":"created"}
        """.utf8))
    }

    private func fixture(query: TerminalHubIndex.Query = .init(), allowsCreate: Bool = true) throws -> TerminalHubIndex {
        TerminalHubIndex.make(
            summaries: [
                summary("t-dev", projectId: "termul", title: "pnpm tauri dev", cwd: "~/project/me/termul", branch: "main"),
                summary("t-claude", projectId: "termul", title: "claude", cwd: "~/project/me/termul", branch: "main"),
                summary("t-codex", projectId: "portal", title: "codex", cwd: "~/project/dsl/dslpayment-portal", branch: "feat/login-split"),
                summary("t-portal", projectId: "portal", title: "pnpm dev", cwd: "~/project/dsl/dslpayment-portal"),
                summary("t-dmit-1", projectId: "deploy", title: "ssh dmit"),
                summary("t-dmit-2", projectId: "deploy", title: "ssh dmit"),
                summary("t-home", title: "zsh", cwd: "~"),
            ],
            projects: [
                try project("termul", "termul", path: "/Users/qs/project/me/termul"),
                try project("portal", "dslpayment-portal"),
                try project("ns", "ns-payment-control-client"),
                try project("deploy", "se-server-deploy"),
                try project("old", "legacy", archived: true),
            ],
            projectGroups: [
                try group("g-se", "Se", ["termul", "deploy"]),
                try group("g-pay", "Payments", ["portal", "ns"]),
            ],
            quickRecords: [
                try quick("q-dev", title: "Se dev", terminals: ["t-dev"], projectId: "termul"),
                try quick("q-dmit", title: "dmit", terminals: ["t-dmit-1", "t-dmit-2"], projectId: "deploy"),
                try quick("q-logs", title: "ns logs", projectId: "ns"),
            ],
            agentTerminalIDs: ["t-claude", "t-codex"],
            lastTerminalID: "t-claude",
            allowsCreate: allowsCreate,
            query: query
        )
    }

    @Test("Shells group by host project group, then project, in host order")
    func groupsInHostOrder() throws {
        let index = try fixture()
        #expect(index.showsGroupHeaders)
        #expect(index.groups.map(\.name) == ["Se", "Payments", TerminalHubIndex.ungroupedName])
        #expect(index.groups[0].projects.map(\.name) == ["termul", "se-server-deploy"])
        #expect(index.groups[1].projects.map(\.name) == ["dslpayment-portal"])
        #expect(index.groups[2].projects.map(\.id) == [TerminalHubIndex.workspaceSectionID])
    }

    @Test("A quick terminal's shells stay in their project, marked pinned")
    func quickShellsStayListed() throws {
        let index = try fixture()
        let termul = try #require(index.groups.first?.projects.first)
        #expect(termul.shells.map(\.id) == ["t-dev", "t-claude"])
        #expect(termul.shells.first?.isPinned == true)
        #expect(termul.shells.first?.quickTitle == "Se dev")
        #expect(index.shells.count == 7)
    }

    @Test("Every owned shell is selectable under its quick terminal")
    func quickListsEachShell() throws {
        let index = try fixture()
        let dmit = try #require(index.quick.first(where: { $0.id == "q-dmit" }))
        #expect(dmit.shells.map(\.id) == ["t-dmit-1", "t-dmit-2"])
        #expect(dmit.liveCount == 2)
        let logs = try #require(index.quick.first(where: { $0.id == "q-logs" }))
        #expect(!logs.isRunning)
        #expect(logs.canOpen)
    }

    @Test("Status counts cover running, agent and not started")
    func counts() throws {
        let index = try fixture()
        #expect(index.count(.running) == 7)
        #expect(index.count(.agent) == 2)
        #expect(index.count(.idle) == 1)
        #expect(index.count(.all) == 8)
    }

    @Test("Agent filter keeps only shells with a live agent")
    func agentFilter() throws {
        let index = try fixture(query: .init(status: .agent))
        #expect(index.shells.map(\.id) == ["t-claude", "t-codex"])
        #expect(index.quick.isEmpty)
    }

    @Test("Not-started filter lists only quick terminals without a shell")
    func idleFilter() throws {
        let index = try fixture(query: .init(status: .idle))
        #expect(index.shells.isEmpty)
        #expect(index.quick.map(\.id) == ["q-logs"])
    }

    @Test("Group filter narrows shells, quick terminals and counts")
    func groupFilter() throws {
        let index = try fixture(query: .init(groupID: "g-pay"))
        #expect(index.shells.map(\.id) == ["t-codex", "t-portal"])
        #expect(index.quick.map(\.id) == ["q-logs"])
        #expect(index.count(.running) == 2)
        #expect(index.count(.agent) == 1)
        #expect(index.groupOptions.map(\.name) == ["Se", "Payments", TerminalHubIndex.ungroupedName])
        #expect(index.groupOptions.map(\.runningCount) == [4, 2, 1])
    }

    @Test("Search matches project, title, cwd, branch and group names")
    func search() throws {
        let pay = try fixture(query: .init(text: "PAY"))
        #expect(pay.shells.map(\.id) == ["t-codex", "t-portal"])
        #expect(pay.projectHits.map(\.name) == ["dslpayment-portal", "ns-payment-control-client"])
        #expect(pay.projectHits.first?.runningCount == 2)
        #expect(pay.quick.map(\.id) == ["q-logs"])

        let branch = try fixture(query: .init(text: "login-split"))
        #expect(branch.shells.map(\.id) == ["t-codex"])

        let group = try fixture(query: .init(text: "se"))
        #expect(group.projectHits.contains { $0.name == "termul" && $0.groupName == "Se" })
    }

    @Test("Searching a shell title shows only that shell under its quick terminal")
    func searchInsideQuick() throws {
        let index = try fixture(query: .init(text: "tauri"))
        let dev = try #require(index.quick.first)
        #expect(dev.id == "q-dev")
        #expect(dev.shells.map(\.id) == ["t-dev"])
    }

    @Test("Archived projects never appear as search hits")
    func archivedHidden() throws {
        let index = try fixture(query: .init(text: "legacy"))
        #expect(index.projectHits.isEmpty)
    }

    @Test("Without host groups there are no group headers or group filter")
    func noHostGroups() throws {
        let index = TerminalHubIndex.make(
            summaries: [summary("t1", projectId: "a")],
            projects: [try project("a", "Termul")],
            projectGroups: [],
            quickRecords: []
        )
        #expect(!index.showsGroupHeaders)
        #expect(index.groupOptions.isEmpty)
        #expect(index.groups.first?.projects.first?.name == "Termul")
    }

    @Test("A shell in a project the catalog does not know keeps its id")
    func unknownProject() {
        let index = TerminalHubIndex.make(
            summaries: [summary("t1", projectId: "ghost")],
            projects: [],
            projectGroups: [],
            quickRecords: []
        )
        #expect(index.groups.first?.projects.first?.name == "ghost")
    }

    @Test("Quick terminals without a shell cannot open when creating is denied")
    func quickDenied() throws {
        let index = try fixture(allowsCreate: false)
        #expect(index.quick.first(where: { $0.id == "q-logs" })?.canOpen == false)
        #expect(index.quick.first(where: { $0.id == "q-dev" })?.canOpen == true)
    }

    @Test("An empty host is an empty index")
    func empty() {
        let index = TerminalHubIndex.make(summaries: [], projects: [], projectGroups: [], quickRecords: [])
        #expect(index.isEmpty)
        #expect(index.count(.all) == 0)
    }

    @Test("A listed shell opens its workspace terminal")
    func request() throws {
        let index = try fixture()
        let shell = try #require(index.shells.first(where: { $0.id == "t-codex" }))
        let request = TerminalHubIndex.request(for: shell)
        #expect(request.attach == .workspace(terminalID: "t-codex"))
        #expect(request.projectName == "dslpayment-portal")
        #expect(shell.groupName == "Payments")
    }
}

@Suite("New terminal catalog")
struct NewTerminalCatalogTests {
    private func project(_ id: String, _ name: String, path: String? = nil, archived: Bool = false) throws -> HostProject {
        let pathField = path.map { ",\"path\":\"\($0)\"" } ?? ""
        return try JSONDecoder().decode(HostProject.self, from: Data("""
        {"id":"\(id)","name":"\(name)","liveTerminalCount":0,"isArchived":\(archived)\(pathField)}
        """.utf8))
    }

    private func group(_ id: String, _ name: String, _ projectIds: [String]) throws -> HostProjectGroup {
        let ids = projectIds.map { "\"\($0)\"" }.joined(separator: ",")
        return try JSONDecoder().decode(HostProjectGroup.self, from: Data("""
        {"id":"\(id)","name":"\(name)","projectIds":[\(ids)]}
        """.utf8))
    }

    private func projects() throws -> [HostProject] {
        [
            try project("termul", "termul", path: "/Users/qs/project/me/termul"),
            try project("portal", "dslpayment-portal", path: "/work/dsl/portal"),
            try project("ns", "ns-payment-control-client"),
            try project("misc", "scratch"),
            try project("old", "legacy", archived: true),
        ]
    }

    @Test("Recent projects lead, then host groups, then ungrouped")
    func groupedSections() throws {
        let sections = NewTerminalCatalog.sections(
            projects: try projects(),
            groups: [try group("g-pay", "Payments", ["portal", "ns"]), try group("g-se", "Se", ["termul"])],
            recentProjectIDs: ["ns", "old", "gone"],
            runningByProject: ["portal": 2]
        )
        #expect(sections.map(\.kind) == [.recent, .group, .group, .group])
        #expect(sections[0].items.map(\.id) == ["ns"])
        #expect(sections[1].title == "Payments")
        #expect(sections[1].items.map(\.id) == ["portal", "ns"])
        #expect(sections[1].items.first?.runningCount == 2)
        #expect(sections[2].items.map(\.id) == ["termul"])
        #expect(sections[3].items.map(\.id) == ["misc"])
    }

    @Test("Without host groups the projects are one flat section")
    func flat() throws {
        let sections = NewTerminalCatalog.sections(projects: try projects(), groups: [], recentProjectIDs: [])
        #expect(sections.map(\.kind) == [.all])
        #expect(sections[0].items.count == 4)
    }

    @Test("Search flattens and ranks name hits before path or group hits")
    func search() throws {
        let sections = NewTerminalCatalog.sections(
            projects: try projects(),
            groups: [try group("g-pay", "Payments", ["portal", "ns"])],
            recentProjectIDs: ["termul"],
            query: "pay"
        )
        #expect(sections.map(\.kind) == [.results])
        #expect(sections[0].items.map(\.id) == ["portal", "ns"])
        #expect(sections[0].items.first?.groupName == "Payments")

        let path = NewTerminalCatalog.sections(projects: try projects(), groups: [], recentProjectIDs: [], query: "/me/")
        #expect(path.first?.items.map(\.id) == ["termul"])

        let none = NewTerminalCatalog.sections(projects: try projects(), groups: [], recentProjectIDs: [], query: "zzz")
        #expect(none.isEmpty)
    }

    @Test("Recent projects move to the front without duplicates")
    func recentOrder() {
        let ids = TerminalRecentProjects.updated(["a", "b", "c"], with: "b")
        #expect(ids == ["b", "a", "c"])
        #expect(TerminalRecentProjects.updated(ids, with: "  ") == ids)
        let many = (0..<20).reduce([String]()) { TerminalRecentProjects.updated($0, with: "p\($1)") }
        #expect(many.count == TerminalRecentProjects.limit)
        #expect(many.first == "p19")
    }
}
