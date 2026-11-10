import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal surface lease")
@MainActor
struct TerminalSurfaceLeaseTests {
    // MARK: - Helpers

    private func makeSession(port: Int = 1) -> WorkspaceSession {
        WorkspaceSession(
            accessURL: URL(string: "http://127.0.0.1:\(port)")!,
            bearer: "test"
        )
    }

    private func liveTerminal(
        _ id: String,
        claim: String? = nil,
        cols: Int = 80,
        rows: Int = 24,
        owned: Bool = false,
        projectId: String? = nil,
        conversationId: String? = nil
    ) -> LiveTerminal {
        LiveTerminal(
            id: id,
            claim: claim,
            lastSeq: 0,
            cols: cols,
            rows: rows,
            title: "zsh",
            cwd: "/repo",
            gitBranch: nil,
            owned: owned,
            projectId: projectId,
            conversationId: conversationId
        )
    }

    // MARK: - Catalog isolation

    @Test("A quick terminal never enters TerminalStore.terminals")
    func quickTerminalStaysOutOfCatalog() async {
        let session = makeSession()
        session.terminals.terminals = [liveTerminal("catalog-1", projectId: "p1")]
        session.terminals.activeId = "catalog-1"

        let model = TerminalWorkbenchModel(
            session: session,
            probesHook: false
        )
        await model.open(
            TerminalOpenRequest(
                attach: .standalone(terminalID: "quick-1", claim: "claim"),
                title: "Quick shell",
                cwd: "/repo"
            )
        )

        // The attach fails without a host, but the lease rule is what matters:
        // the standalone path only ever talks to `attachViewer`, never to the
        // catalog the workbench refreshes and evicts.
        #expect(model.errorMessage != nil)
        #expect(session.terminals.terminals.map(\.id) == ["catalog-1"])
        #expect(session.terminals.activeId == "catalog-1")
    }

    @Test("Catalog mapping keeps owned claims and carries the host binding")
    func catalogMapping() {
        let owned = liveTerminal("t1", claim: "claim-1", cols: 100, rows: 40, owned: true)
        let summary = LiveTerminalSummary(
            id: "t1",
            shell: "zsh",
            cwd: "/repo",
            pid: 12,
            cols: nil,
            rows: nil,
            terminalScreenEngine: nil,
            conversationId: "conversation-1",
            projectId: "project-1",
            title: "zsh",
            gitBranch: "main",
            screenObservation: nil
        )

        let mapped = TerminalStore.liveTerminals(
            from: [summary],
            ownedById: ["t1": owned]
        )

        #expect(mapped.count == 1)
        #expect(mapped[0].claim == "claim-1")
        #expect(mapped[0].cols == 100)
        #expect(mapped[0].rows == 40)
        #expect(mapped[0].owned)
        #expect(mapped[0].projectId == "project-1")
        #expect(mapped[0].conversationId == "conversation-1")
        #expect(mapped[0].title == "zsh · main")
    }

    // MARK: - Park and resume

    @Test("Parking releases geometry and resuming re-asserts it")
    func parkAndResume() async {
        let session = makeSession()
        session.workspace = .project
        session.terminals.terminals = [liveTerminal("t1")]
        session.terminals.activeId = "t1"
        session.terminals.geometryActive = true
        session.terminals.displayMode = .phone

        await session.parkTerminalSurface()

        #expect(!session.terminals.geometryActive)
        #expect(session.terminals.displayMode == .phone)
        // The PTY and the selection survive the park.
        #expect(session.terminals.terminals.map(\.id) == ["t1"])
        #expect(session.terminals.activeId == "t1")

        await session.resumeTerminalSurface()

        #expect(session.terminals.geometryActive)
    }

    @Test("Resume is a no-op without a live terminal")
    func resumeWithoutTerminal() async {
        let session = makeSession()
        session.workspace = .project
        session.terminals.activeId = nil
        session.terminals.geometryActive = false

        await session.resumeTerminalSurface()

        #expect(!session.terminals.geometryActive)
    }

    // MARK: - Scope

    @Test("Tool scope follows the active terminal cwd")
    func activeToolRoot() {
        let session = makeSession()
        session.workspace = .project
        session.terminals.terminals = [liveTerminal("t1")]
        session.terminals.activeId = "t1"

        #expect(session.currentTerminalCwd == "/repo")
        #expect(session.activeToolRoot == "/repo")
    }

    @Test("Agent drafts are queued per terminal and taken once")
    func agentDrafts() {
        let session = makeSession()
        session.offerAgentDraft("'/repo/a.png' ", terminalID: "t1")

        #expect(session.takeAgentDraft(terminalID: "t1") == "'/repo/a.png' ")
        #expect(session.takeAgentDraft(terminalID: "t1") == nil)
        #expect(session.takeAgentDraft(terminalID: "t2") == nil)
    }
}
