import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal panel presentation")
struct TerminalPanelPresentationTests {
    private let items = [
        TerminalSurfaceItem(id: "t1", title: "zsh", cwd: "/repo", projectID: "p1"),
        TerminalSurfaceItem(id: "t2", title: "dev server", cwd: "/repo", projectID: "p1"),
        TerminalSurfaceItem(id: "t3", title: "test", cwd: "/repo", projectID: "p1"),
    ]

    private func workspaceRequest(active: String = "t2") -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .workspace(terminalID: active),
            title: "dev server",
            cwd: "/repo",
            projectID: "p1",
            projectName: "termul"
        )
    }

    private func standaloneRequest() -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .standalone(terminalID: "q1", claim: "claim"),
            title: "Quick shell",
            cwd: "/repo",
            projectID: "p1"
        )
    }

    @Test("Workspace counters follow the active tab")
    func workspaceCounter() {
        let presentation = TerminalPanelPresentation.make(
            request: workspaceRequest(),
            items: items,
            activeID: "t2",
            cols: 120,
            rows: 30,
            agentName: "Claude",
            agentReady: true,
            permissionCount: 2,
            subtitle: "termul"
        )

        #expect(presentation.counter == "2/3")
        #expect(presentation.tabs.count == 3)
        #expect(presentation.tabs.map(\.index) == [0, 1, 2])
        #expect(presentation.activeTabID == "t2")
        #expect(presentation.showsTabs)
        #expect(presentation.subtitle == "termul")
        #expect(presentation.title == "dev server")
    }

    @Test("Workspace PTY row offers FIT and exact dimensions")
    func workspacePTYRow() {
        let presentation = TerminalPanelPresentation.make(
            request: workspaceRequest(),
            items: items,
            activeID: "t2",
            cols: 120,
            rows: 30,
            agentName: "Claude",
            agentReady: true,
            permissionCount: 0
        )

        #expect(presentation.pty.showsFit)
        #expect(presentation.pty.fitActive)
        #expect(presentation.pty.isReady)
        #expect(presentation.pty.dimensions == "120 × 30")
        #expect(presentation.pty.textScaleLabel == "A+")
        #expect(presentation.close == .disconnect)
        #expect(!presentation.isStandalone)
    }

    @Test("Quick terminals hide tabs and FIT and close their shell")
    func standalonePanel() {
        let presentation = TerminalPanelPresentation.make(
            request: standaloneRequest(),
            items: [TerminalSurfaceItem(id: "q1", title: "Quick shell", cwd: "/repo")],
            activeID: "q1",
            cols: 80,
            rows: 24,
            agentName: nil,
            agentReady: false,
            permissionCount: 0
        )

        #expect(!presentation.showsTabs)
        #expect(!presentation.pty.showsFit)
        #expect(!presentation.pty.fitActive)
        #expect(presentation.close == .closeShell)
        #expect(presentation.isStandalone)
        #expect(presentation.agent == nil)
    }

    @Test("The agent row appears only when an agent is ready")
    func agentRowVisibility() {
        let hidden = TerminalPanelPresentation.make(
            request: workspaceRequest(),
            items: items,
            activeID: "t2",
            cols: 80,
            rows: 24,
            agentName: nil,
            agentReady: false,
            permissionCount: 0
        )
        #expect(hidden.agent == nil)

        let visible = TerminalPanelPresentation.make(
            request: workspaceRequest(),
            items: items,
            activeID: "t2",
            cols: 80,
            rows: 24,
            agentName: "Claude Code",
            agentReady: true,
            permissionCount: 1
        )
        #expect(visible.agent?.name == "Claude Code")
        #expect(visible.agent?.permissionCount == 1)
        #expect(visible.agent?.status.isEmpty == false)
    }

    @Test("Unknown dimensions read as connecting")
    func missingDimensions() {
        let presentation = TerminalPanelPresentation.make(
            request: workspaceRequest(),
            items: items,
            activeID: "t2",
            cols: nil,
            rows: nil,
            agentName: nil,
            agentReady: false,
            permissionCount: 0
        )

        #expect(!presentation.pty.isReady)
        #expect(presentation.pty.dimensions == "—")
    }

    @Test("An empty catalog still counts one surface")
    func emptyCatalog() {
        let presentation = TerminalPanelPresentation.make(
            request: nil,
            items: [],
            activeID: nil,
            cols: nil,
            rows: nil,
            agentName: nil,
            agentReady: false,
            permissionCount: 0
        )

        #expect(presentation.counter == "1/1")
        #expect(presentation.tabs.isEmpty)
        #expect(presentation.close == .disconnect)
    }

    @Test("Close policies state their real consequence")
    func closePolicies() {
        #expect(TerminalClosePolicy.closeShell.isDestructive)
        #expect(!TerminalClosePolicy.disconnect.isDestructive)
        #expect(TerminalClosePolicy.closeShell.consequence.isEmpty == false)
        #expect(TerminalClosePolicy.disconnect.consequence.isEmpty == false)
        #expect(TerminalClosePolicy.closeShell.consequence != TerminalClosePolicy.disconnect.consequence)
        #expect(TerminalClosePolicy.closeShell.confirmTitle != TerminalClosePolicy.disconnect.confirmTitle)
    }
}
