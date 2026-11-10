import Foundation
import Testing
@testable import SeRemote

@Suite("Shell navigation state")
struct ShellNavigationStateTests {
    private func request(_ id: String = "t1", project: String? = "p1") -> TerminalOpenRequest {
        TerminalOpenRequest(
            attach: .workspace(terminalID: id),
            title: "zsh",
            cwd: "/work",
            projectID: project,
            projectName: "Termul"
        )
    }

    @Test("Opening a terminal shows the workbench and leaves the landing list")
    func openShowsWorkbench() {
        var state = ShellNavigationState()
        let request = request()
        state.open(request)

        #expect(state.workbench)
        #expect(state.workbenchVisible)
        #expect(!state.landing)
        #expect(state.destination == .terminals)
        #expect(state.openRequest == request)
    }

    @Test("Back returns to the terminal list and keeps the request")
    func backKeepsRequest() {
        var state = ShellNavigationState()
        let request = request()
        state.open(request)
        state.present(.files)
        state.back()

        #expect(!state.workbench)
        #expect(state.landing)
        #expect(state.panel == nil)
        #expect(state.destination == .terminals)
        // I1/I5: the surface keeps its request, so the same PTY resumes.
        #expect(state.openRequest == request)
    }

    @Test("Selecting another destination hides the surface without dropping it")
    func selectKeepsWorkbench() {
        var state = ShellNavigationState()
        let request = request()
        state.open(request)
        state.select(.conversations)

        #expect(state.destination == .conversations)
        #expect(state.workbench)
        // Hidden only because another destination is on screen.
        #expect(!state.workbenchVisible)
        #expect(state.openRequest == request)

        state.select(.tools)
        #expect(state.destination == .tools)
        #expect(state.openRequest == request)

        state.select(.terminals)
        #expect(state.workbenchVisible)
    }

    @Test("Presenting a panel never changes the surface")
    func presentKeepsGeometry() {
        var state = ShellNavigationState()
        state.open(request())
        state.present(.more)

        #expect(state.panel == .more)
        #expect(state.workbench)
        #expect(state.workbenchVisible)
    }

    @Test("Connecting resumes the last terminal or lands on T1")
    func connected() {
        var resumed = ShellNavigationState(destination: .conversations, landing: true, workbench: false)
        let last = request("last")
        resumed.connected(last: last)
        #expect(resumed.workbenchVisible)
        #expect(resumed.destination == .terminals)
        #expect(resumed.openRequest == last)

        var fresh = ShellNavigationState(destination: .conversations)
        fresh.connected(last: nil)
        #expect(!fresh.workbench)
        #expect(fresh.landing)
        #expect(fresh.destination == .terminals)
        #expect(fresh.openRequest == nil)
    }
}
