import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal workbench model")
@MainActor
struct TerminalWorkbenchModelTests {
    // MARK: - Fixtures

    @MainActor
    private final class EventLog {
        private(set) var events: [String] = []

        func append(_ event: String) {
            events.append(event)
        }

        func clear() {
            events.removeAll()
        }
    }

    @MainActor
    private final class FakeBackend: TerminalSurfaceBackend {
        let name: String
        let log: EventLog
        var itemsValue: [TerminalSurfaceItem]
        var activeIDValue: String?
        var activeTitleValue: String
        var activeCwdValue: String?
        var activeProjectIDValue: String?
        var agentReadyValue = false
        var isLiveAttachedValue = true
        private(set) var writes: [String] = []
        private(set) var resizes: [(cols: Int, rows: Int)] = []

        init(
            name: String,
            log: EventLog,
            items: [TerminalSurfaceItem],
            activeTitle: String
        ) {
            self.name = name
            self.log = log
            self.itemsValue = items
            self.activeIDValue = items.first?.id
            self.activeTitleValue = activeTitle
            self.activeCwdValue = items.first?.cwd
            self.activeProjectIDValue = items.first?.projectID
        }

        var items: [TerminalSurfaceItem] { itemsValue }
        var activeID: String? { activeIDValue }
        var activeTitle: String { activeTitleValue }
        var activeCwd: String? { activeCwdValue }
        var activeProjectID: String? { activeProjectIDValue }
        var agentReady: Bool { agentReadyValue }
        var isLiveAttached: Bool { isLiveAttachedValue }

        func select(_ id: String) async {
            log.append("\(name).select")
            activeIDValue = id
            activeTitleValue = "Terminal \(id)"
            if let item = itemsValue.first(where: { $0.id == id }) {
                activeTitleValue = item.title
                activeCwdValue = item.cwd
                activeProjectIDValue = item.projectID
            }
        }

        func write(_ text: String) async {
            log.append("\(name).write")
            writes.append(text)
        }

        func resize(cols: Int, rows: Int) {
            log.append("\(name).resize")
            resizes.append((cols, rows))
        }

        func park() async {
            log.append("\(name).park")
        }

        func resume() async {
            log.append("\(name).resume")
        }
    }

    private struct Fixture {
        let model: TerminalWorkbenchModel
        let log: EventLog
        let workspace: FakeBackend
        let session: WorkspaceSession
    }

    private func makeFixture(
        watching: Bool = false,
        standaloneFails: Bool = false,
        standaloneAttached: Bool = true
    ) -> Fixture {
        let session = WorkspaceSession(
            accessURL: URL(string: "http://127.0.0.1:53290")!,
            bearer: "test"
        )
        let log = EventLog()
        let workspace = FakeBackend(
            name: "workspace",
            log: log,
            items: [
                TerminalSurfaceItem(id: "t1", title: "zsh", cwd: "/repo", projectID: "p1"),
                TerminalSurfaceItem(id: "t2", title: "dev server", cwd: "/repo", projectID: "p1"),
            ],
            activeTitle: "zsh"
        )
        let model = TerminalWorkbenchModel(
            session: session,
            workspaceBackend: { _ in workspace },
            standaloneBackend: { _, request in
                log.append("standalone.attach")
                if standaloneFails {
                    throw HostError.network("The host refused the attach.")
                }
                let backend = FakeBackend(
                    name: "standalone",
                    log: log,
                    items: [TerminalSurfaceItem(
                        id: request.terminalID,
                        title: request.title,
                        cwd: request.cwd
                    )],
                    activeTitle: request.title
                )
                backend.isLiveAttachedValue = standaloneAttached
                return backend
            },
            probesHook: false,
            isWatching: { _ in watching }
        )
        return Fixture(model: model, log: log, workspace: workspace, session: session)
    }

    private func workspaceRequest(_ id: String, title: String = "zsh") -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .workspace(terminalID: id),
            title: title,
            cwd: "/repo",
            projectID: "p1",
            projectName: "termul"
        )
    }

    private func standaloneRequest(_ id: String, title: String = "Quick") -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .standalone(terminalID: id, claim: "claim"),
            title: title,
            cwd: "/repo"
        )
    }

    // MARK: - Open and switch ordering

    @Test("Opening a workspace terminal reveals it and then resumes geometry")
    func openWorkspace() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))

        #expect(fixture.log.events == ["workspace.select", "workspace.resume"])
        #expect(fixture.model.request?.terminalID == "t1")
        #expect(fixture.model.activeID == "t1")
        #expect(fixture.model.errorMessage == nil)
    }

    @Test("Switching to a quick terminal parks the workspace surface first")
    func workspaceToStandaloneParksFirst() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        fixture.log.clear()

        await fixture.model.open(standaloneRequest("q1"))

        #expect(fixture.log.events == ["workspace.park", "standalone.attach"])
        #expect(fixture.model.isStandalone)
        #expect(fixture.model.request?.terminalID == "q1")
    }

    @Test("Switching back to a workspace terminal releases the viewer first")
    func standaloneToWorkspaceReleasesViewerFirst() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        await fixture.model.open(standaloneRequest("q1"))
        fixture.log.clear()

        await fixture.model.open(workspaceRequest("t2", title: "dev server"))

        #expect(fixture.log.events == ["standalone.park", "workspace.select", "workspace.resume"])
        #expect(!fixture.model.isStandalone)
        #expect(fixture.model.request?.terminalID == "t2")
    }

    @Test("Selecting a sibling workspace terminal does not park the surface")
    func selectSibling() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        fixture.log.clear()

        await fixture.model.select("t2")

        #expect(fixture.log.events == ["workspace.select", "workspace.resume"])
        #expect(fixture.model.request?.terminalID == "t2")
        #expect(fixture.model.request?.title == "dev server")
    }

    // MARK: - Overlay invariants

    @Test("Presenting the panel and its sheets never resizes the PTY")
    func panelNeverResizes() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        fixture.log.clear()

        fixture.model.panelExpanded = true
        fixture.model.present(.more)
        #expect(!fixture.model.panelExpanded)
        #expect(fixture.model.sheetAction == .more)

        fixture.model.present(.desktop)
        #expect(fixture.model.sheetAction == .desktop)

        fixture.model.present(.agent)
        #expect(fixture.model.sheetAction == .agent)

        #expect(fixture.workspace.resizes.isEmpty)
        #expect(!fixture.log.events.contains { $0.hasSuffix(".resize") })
    }

    @Test("Files and attach leave through the shell; the model presents nothing")
    func externalActions() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        var external: [TerminalPanelAction] = []
        fixture.model.onExternalAction = { external.append($0) }

        fixture.model.present(.files)
        fixture.model.present(.attach)

        #expect(external == [.files, .attach])
        #expect(fixture.model.sheetAction == nil)
    }

    @Test("Back only parks: the request survives and nothing is closed")
    func backNeverCloses() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        let request = fixture.model.request
        fixture.log.clear()

        await fixture.model.back()

        #expect(fixture.log.events == ["workspace.park"])
        #expect(fixture.model.request == request)
        #expect(fixture.model.activeBackend == nil)
        #expect(!fixture.log.events.contains { $0.contains("kill") || $0.contains("close") })
    }

    @Test("Returning to the parked terminal only resumes it")
    func backThenReopenResumes() async {
        let fixture = makeFixture(watching: true)
        await fixture.model.open(workspaceRequest("t1"))
        await fixture.model.back()
        fixture.log.clear()

        await fixture.model.open(workspaceRequest("t1"))

        #expect(fixture.log.events == ["workspace.resume"])
        #expect(fixture.model.request?.terminalID == "t1")
        #expect(fixture.model.activeBackend != nil)
        #expect(fixture.model.errorMessage == nil)
    }

    @Test("Returning to a different terminal still takes the full open path")
    func reopenOtherTerminalSelects() async {
        let fixture = makeFixture(watching: true)
        await fixture.model.open(workspaceRequest("t1"))
        await fixture.model.back()
        fixture.log.clear()

        await fixture.model.open(workspaceRequest("t2", title: "dev server"))

        #expect(fixture.log.events == ["workspace.select", "workspace.resume"])
        #expect(fixture.model.request?.terminalID == "t2")
    }

    @Test("A failed quick open keeps no request, surface or destructive close")
    func failedQuickOpenLeavesNoSurface() async {
        let fixture = makeFixture(standaloneFails: true)

        await fixture.model.open(standaloneRequest("q1"))

        #expect(fixture.model.request == nil)
        #expect(fixture.model.activeBackend == nil)
        #expect(fixture.model.errorMessage != nil)
        #expect(fixture.model.panelPresentation.close == .disconnect)
        #expect(await fixture.model.closeActiveTerminal() == nil)
    }

    @Test("An unattached quick backend never runs the host close route")
    func unattachedQuickCloseOnlyDisconnects() async {
        let fixture = makeFixture(standaloneAttached: false)
        await fixture.model.open(standaloneRequest("q1"))
        #expect(fixture.model.panelPresentation.close == .disconnect)
        fixture.log.clear()

        let result = await fixture.model.closeActiveTerminal()

        #expect(result == .disconnectedView)
        #expect(fixture.log.events == ["standalone.park"])
        #expect(fixture.model.request == nil)
    }

    @Test("The agent action selects the inspector when one owns the surface")
    func agentActionPrefersInspector() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        var selections = 0
        fixture.model.onSelectInspectorAgent = { selections += 1 }

        fixture.model.present(.agent)

        #expect(selections == 1)
        #expect(fixture.model.sheetAction == nil)

        fixture.model.onSelectInspectorAgent = nil
        fixture.model.present(.agent)

        #expect(fixture.model.sheetAction == .agent)
    }

    @Test("Leaving the screen stops the agent poller without touching the surface")
    func inactiveStopsProbe() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        let request = fixture.model.request
        fixture.log.clear()

        fixture.model.setActive(false)

        #expect(fixture.model.hookStore == nil)
        #expect(fixture.model.request == request)
        #expect(fixture.model.activeBackend != nil)
        #expect(fixture.log.events.isEmpty)
    }

    // MARK: - Paths

    @Test("Inserting a path writes the staged argument into the live PTY")
    func insertPath() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))

        fixture.model.insertPath("/repo/a b.png")
        try? await Task.sleep(for: .milliseconds(40))

        #expect(fixture.workspace.writes == [DesktopShellPath.stagedArgument("/repo/a b.png")])
    }

    @Test("Sending a path to the agent queues a session draft")
    func sendToAgent() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))

        fixture.model.sendToAgent("/repo/a b.png")

        #expect(
            fixture.session.takeAgentDraft(terminalID: "t1")
                == DesktopShellPath.stagedArgument("/repo/a b.png")
        )
        #expect(fixture.workspace.writes.isEmpty)
    }

    // MARK: - Close semantics

    @Test("Closing a workspace terminal only disconnects its view")
    func closeWorkspaceDisconnects() async {
        let fixture = makeFixture()
        await fixture.model.open(workspaceRequest("t1"))
        fixture.log.clear()

        let result = await fixture.model.closeActiveTerminal()

        #expect(result == .disconnectedView)
        #expect(fixture.log.events == ["workspace.park"])
        #expect(fixture.model.request != nil)
    }

    @Test("Closing a quick terminal detaches the viewer and forgets the request")
    func closeQuickDetaches() async {
        let fixture = makeFixture()
        await fixture.model.open(standaloneRequest("q1"))
        fixture.log.clear()

        let result = await fixture.model.closeActiveTerminal()

        #expect(result == .disconnectedView)
        #expect(fixture.log.events == ["standalone.park"])
        #expect(fixture.model.request == nil)
        #expect(fixture.model.activeBackend == nil)
    }
}
