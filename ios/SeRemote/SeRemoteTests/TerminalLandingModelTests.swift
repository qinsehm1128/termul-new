import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal landing model")
struct TerminalLandingModelTests {
    private func summary(
        _ id: String,
        projectId: String? = nil,
        title: String? = nil,
        cwd: String? = nil,
        conversationId: String? = nil
    ) -> LiveTerminalSummary {
        LiveTerminalSummary(
            id: id,
            shell: "/bin/zsh",
            cwd: cwd,
            pid: 1,
            cols: 80,
            rows: 24,
            terminalScreenEngine: nil,
            conversationId: conversationId,
            projectId: projectId,
            title: title,
            gitBranch: nil,
            screenObservation: nil
        )
    }

    private func project(_ id: String, _ name: String) throws -> HostProject {
        try JSONDecoder().decode(HostProject.self, from: Data("""
        {"id":"\(id)","name":"\(name)","liveTerminalCount":1}
        """.utf8))
    }

    private func quickRecord(_ id: String, terminal: String?) throws -> QuickTerminalRecordWire {
        try JSONDecoder().decode(QuickTerminalRecordWire.self, from: Data("""
        {"schemaVersion":1,"id":"\(id)","title":"\(id) title",
         "target":{"kind":"workspace"},"cwd":"/home/me",
         "createdAtUtc":"2026-11-10T00:00:00Z","updatedAtUtc":"2026-11-10T00:00:00Z",
         "terminalId":"\(terminal ?? "")","terminalIds":[\(terminal.map { "\"\($0)\"" } ?? "")],
         "origin":"created"}
        """.utf8))
    }

    @Test("The hero is the last terminal, and only when the host still reports it")
    func hero() throws {
        let model = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a"), summary("t2", projectId: "a")],
            projects: [try project("a", "Termul")],
            quickRecords: [],
            lastTerminalID: "t2"
        )
        #expect(model.hero?.id == "t2")
        #expect(model.hero?.projectName == "Termul")

        let none = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a")],
            projects: [try project("a", "Termul")],
            quickRecords: [],
            lastTerminalID: "gone"
        )
        #expect(none.hero == nil)
    }

    @Test("Terminals group by project; quick terminals are not listed twice")
    func grouping() throws {
        let record = try quickRecord("q", terminal: "quick-1")
        let model = TerminalLandingModel.make(
            summaries: [
                summary("t1", projectId: "a"),
                summary("t2", projectId: "a"),
                summary("t3", projectId: "b"),
                summary("quick-1", projectId: "a"),
            ],
            projects: [try project("a", "Termul"), try project("b", "se-server")],
            quickRecords: [record]
        )
        #expect(model.groups.map(\.name) == ["Termul", "se-server"])
        #expect(model.groups.map(\.count) == [2, 1])
        #expect(model.groups.flatMap(\.rows).map(\.id) == ["t1", "t2", "t3"])
        #expect(model.terminalCount == 3)
        #expect(model.quick.map(\.id) == ["q"])
        #expect(model.quick.first?.liveCount == 1)
        #expect(model.quick.first?.terminalID == "quick-1")
        #expect(model.isEmpty == false)
    }

    @Test("An empty host is an empty landing, never a fabricated terminal")
    func empty() {
        let model = TerminalLandingModel.make(
            summaries: [], projects: [], quickRecords: [], lastTerminalID: nil
        )
        #expect(model.isEmpty)
        #expect(model.hero == nil)
        #expect(model.groups.isEmpty)
        #expect(model.quick.isEmpty)
    }

    @Test("New terminal is disabled without a project to pick")
    func newTerminalDisabled() throws {
        let noProjects = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a")],
            projects: [],
            quickRecords: [],
            allowsCreate: true
        )
        #expect(!noProjects.canCreate)

        let enabled = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a")],
            projects: [try project("a", "Termul")],
            quickRecords: [],
            allowsCreate: true
        )
        #expect(enabled.canCreate)

        let denied = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a")],
            projects: [try project("a", "Termul")],
            quickRecords: [],
            allowsCreate: false
        )
        #expect(!denied.canCreate)
    }

    @Test("A row opens its workspace terminal with the project name")
    func rowRequest() throws {
        let model = TerminalLandingModel.make(
            summaries: [summary("t1", projectId: "a", title: "dev server", cwd: "/work/termul")],
            projects: [try project("a", "Termul")],
            quickRecords: []
        )
        let row = try #require(model.groups.first?.rows.first)
        let request = TerminalLandingModel.request(for: row)
        #expect(request.attach == .workspace(terminalID: "t1"))
        #expect(request.title == "dev server")
        #expect(request.cwd == "/work/termul")
        #expect(request.projectID == "a")
        #expect(request.projectName == "Termul")
    }
}

@Suite("Conversation open policy")
struct ConversationOpenPolicyTests {
    private func conversation(projectId: String? = nil) throws -> HostConversation {
        var json = """
        {"conversationId":"c1","workspaceCwd":"/work/termul","lifecycleState":"ready","title":"Fix reconnect"
        """
        if let projectId {
            json += ",\"projectAttachment\":{\"projectId\":\"" + projectId + "\"}"
        }
        json += "}"
        return try JSONDecoder().decode(HostConversation.self, from: Data(json.utf8))
    }

    private func terminal(
        _ id: String,
        conversationId: String? = nil,
        projectId: String? = nil
    ) -> LiveTerminalSummary {
        LiveTerminalSummary(
            id: id,
            shell: "/bin/zsh",
            cwd: "/work/termul",
            pid: 1,
            cols: 80,
            rows: 24,
            terminalScreenEngine: nil,
            conversationId: conversationId,
            projectId: projectId,
            title: nil,
            gitBranch: nil,
            screenObservation: nil
        )
    }

    @Test("A conversation with a live terminal opens the workbench")
    func opensTerminal() throws {
        let destination = ConversationOpenPolicy.destination(
            conversation: try conversation(),
            terminals: [terminal("t1", conversationId: "c1")]
        )
        guard case let .terminal(request) = destination else {
            Issue.record("Expected a terminal destination")
            return
        }
        #expect(request.terminalID == "t1")
        #expect(request.attach == .workspace(terminalID: "t1"))
    }

    @Test("A project terminal is the fallback for its conversation")
    func projectFallback() throws {
        let destination = ConversationOpenPolicy.destination(
            conversation: try conversation(projectId: "p1"),
            terminals: [terminal("t2", projectId: "p1")]
        )
        guard case let .terminal(request) = destination else {
            Issue.record("Expected a terminal destination")
            return
        }
        #expect(request.terminalID == "t2")
        #expect(request.projectID == "p1")
    }

    @Test("Without a live shell the conversation opens chat")
    func opensChat() throws {
        let destination = ConversationOpenPolicy.destination(
            conversation: try conversation(projectId: "p1"),
            terminals: [terminal("other", projectId: "p2")]
        )
        #expect(destination == .chat(conversationID: "c1"))
    }
}
