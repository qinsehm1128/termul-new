import Foundation
import Testing
@testable import SeRemote

@MainActor
struct DesktopSocketTests {
    @Test func rapidDisplaySwitchFencesOldFramesUntilFinalMeta() {
        var fence = DesktopDisplayFence()
        fence.request("A")
        #expect(!fence.accepts("A"))
        let initial = fence.acknowledge("A")
        #expect(initial)
        #expect(fence.accepts("A"))
        fence.request("B")
        fence.request("A")
        #expect(!fence.accepts("A"))
        let intermediate = fence.acknowledge("B")
        #expect(!intermediate)
        #expect(!fence.accepts("A"))
        let final = fence.acknowledge("A")
        #expect(final)
        #expect(fence.accepts("A"))
        #expect(!fence.accepts("B"))
    }

    @Test func displayFenceIgnoresUnrequestedMetaAndResetsOnDisconnect() {
        var fence = DesktopDisplayFence()
        fence.request("B")
        let unsolicited = fence.acknowledge("A")
        #expect(!unsolicited)
        #expect(fence.pending == ["B"])
        let matching = fence.acknowledge("B")
        #expect(matching)
        fence.reset()
        #expect(!fence.accepts("B"))
        let stale = fence.acknowledge("B")
        #expect(!stale)
    }

    /// The quick sheet streams on its own socket. Closing it must leave the
    /// Screen page's live desktop alone (regression guard for one shared socket).
    @Test func closingOneOwnerLeavesTheOtherSocketOpen() async {
        let origin = URL(string: "http://localhost:53240")!
        let credentials = HostCredentials(accessURL: origin, bearer: "fixture")
        let screen = DesktopSocket()
        let sheet = DesktopSocket()
        for socket in [screen, sheet] {
            socket.configure(http: HostHTTP(origin: origin, credentials: credentials))
            await socket.open()
        }
        #expect(screen.isOpen)
        #expect(sheet.isOpen)

        sheet.close()

        #expect(!sheet.isOpen)
        #expect(screen.isOpen)
        for _ in 0 ..< 100 where screen.state == .idle {
            try? await Task.sleep(for: .milliseconds(20))
        }
        #expect(screen.state != .idle)
        screen.close()
        #expect(!screen.isOpen)
    }

    @Test func revokedDesktopNeverReconnectsOrResumes() async {
        let socket = DesktopSocket()
        let origin = URL(string: "http://localhost:53238")!
        socket.configure(http: HostHTTP(origin: origin, credentials: HostCredentials(accessURL: origin, bearer: "fixture")))
        socket.revoke()
        socket.close()
        socket.handleLifecycle(isBackground: true)
        socket.handleLifecycle(isBackground: false)
        await socket.open()
        #expect(socket.state == .revoked)
        #expect(socket.image == nil)
        #expect(socket.displays.isEmpty)
        #expect(!socket.controlActive)
    }

    @Test func controlCannotBeEnabledBeforeStreamOrWithoutPermission() {
        let socket = DesktopSocket()
        socket.setControlActive(true)
        socket.send(.pointer(display: "1", u: 0.5, v: 0.5))
        socket.send(.text("fixture"))
        #expect(!socket.controlActive)
        #expect(socket.state == .idle)
    }

    @Test func backgroundPausesAnIdleDesktopWithoutStartingATransport() {
        let socket = DesktopSocket()
        socket.handleLifecycle(isBackground: true)
        #expect(socket.state == .paused)
        #expect(!socket.controlActive)
        socket.handleLifecycle(isBackground: false)
        // The tool was not visible: foreground must not create a desktop stream.
        #expect(socket.state == .paused)
        socket.close()
        #expect(socket.state == .idle)
    }

    @Test(arguments: ["DESKTOP_DISABLED", "CONTROL_DISABLED", "SCREEN_PERMISSION_REQUIRED", "INPUT_PERMISSION_REQUIRED", "SCREEN_UNSUPPORTED", "DISPLAY_NOT_FOUND"])
    func desktopHTTPPolicyFailuresRetainTheirStableCode(_ code: String) throws {
        let data = try WireJSON.data(from: ["code": code, "error": "fixture"])
        let error = HostHTTP.desktopResponseError(statusCode: 403, data: data)
        guard case .ipc(_, let actualCode) = error else {
            Issue.record("A desktop policy failure became a credential failure")
            return
        }
        #expect(actualCode == code)
    }

    @Test func desktopOnlyStopMapsToDisconnectedNotRevocation() {
        #expect(DesktopIssue(code: "DESKTOP_STOPPED") == .disconnected)
        let failure = HostHTTP.desktopResponseError(statusCode: 403, data: Data(#"{"code":"DESKTOP_STOPPED"}"#.utf8))
        if case .ipc(_, let code) = failure { #expect(code == "DESKTOP_STOPPED") }
        else { Issue.record("Desktop-only stop must not be a credential failure") }
    }

    @Test func desktopHTTPAccessFailuresStillUseSharedRevocationAndScopeMapping() {
        let revoked = HostHTTP.desktopResponseError(statusCode: 401, data: Data(#"{"code":"DEVICE_REVOKED"}"#.utf8))
        if case .deviceRevoked = revoked {} else { Issue.record("Revocation identity lost") }
        let forbidden = HostHTTP.desktopResponseError(statusCode: 403, data: Data(#"{"code":"FORBIDDEN","capability":"screen.control"}"#.utf8))
        if case .forbidden(let scope) = forbidden { #expect(scope == "screen.control") }
        else { Issue.record("Scope denial identity lost") }
        let rejected = HostHTTP.desktopResponseError(statusCode: 401, data: Data())
        if case .rePairingRequired = rejected {} else { Issue.record("Unauthorized rejection must require pairing") }
    }

    @Test func workspaceRevocationLatchesDesktopWithoutChangingTerminalGeometryOrPty() async {
        let session = WorkspaceSession(accessURL: URL(string: "http://localhost:53239")!, bearer: "fixture")
        session.workspaceTab = .terminal
        session.terminals.geometryActive = true
        let pty = LiveTerminal(id: "desktop-existing-pty", claim: "existing-claim", lastSeq: 17,
                               cols: 83, rows: 25, title: "running", owned: true)
        session.terminals.terminals = [pty]
        session.terminals.activeId = pty.id
        HostError.deviceRevoked.publishAccessFailure(origin: session.origin)
        #expect(session.phase == .revoked)
        #expect(session.desktopSocket.state == .revoked)
        session.stop()
        session.handleScene(isBackground: true)
        session.handleScene(isBackground: false)
        await session.desktopSocket.open()
        #expect(session.desktopSocket.state == .revoked)
        #expect(session.workspaceTab == .terminal)
        #expect(session.terminals.geometryActive)
        #expect(session.terminals.terminals == [pty])
        #expect(session.terminals.activeId == pty.id)
    }
}
