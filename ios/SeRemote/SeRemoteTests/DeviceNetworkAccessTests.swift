import Foundation
import Testing
@testable import SeRemote

@MainActor
struct DeviceNetworkAccessTests {
    @Test func httpRevocationIsNotLegacyPairingOrNetworkFailure() throws {
        let data = Data(#"{"success":false,"code":"DEVICE_REVOKED","error":"Device revoked"}"#.utf8)
        let error = try #require(HostHTTP.responseError(statusCode: 401, data: data))
        guard case .deviceRevoked = error else { Issue.record("Expected deviceRevoked"); return }
        #expect(!PairingRecovery.needsRePairing(error))
        #expect(!PairingRecovery.isWorthRetrying(error))
    }

    @Test func httpScopeDenialRetainsCapabilityAndDoesNotRequestPairing() throws {
        let data = Data(#"{"success":false,"code":"FORBIDDEN","capability":"files","error":"Denied"}"#.utf8)
        let error = try #require(HostHTTP.responseError(statusCode: 403, data: data))
        guard case .forbidden(let capability) = error else { Issue.record("Expected forbidden"); return }
        #expect(capability == "files")
        #expect(error.localizedDescription.contains(DeviceScope.label(for: "files")))
        #expect(!PairingRecovery.needsRePairing(error))
        #expect(!PairingRecovery.isWorthRetrying(error))
    }

    @Test func legacyAuthFailureStillRequestsPairingAndSuccessIsNotAnError() throws {
        let data = Data(#"{"success":false,"code":"UNAUTHORIZED"}"#.utf8)
        let error = try #require(HostHTTP.responseError(statusCode: 401, data: data))
        #expect(PairingRecovery.needsRePairing(error))
        #expect(HostHTTP.responseError(statusCode: 200, data: data) == nil)
    }

    @Test func policyCloseBeforeOpenIsCachedAndPublishedOnce() async {
        await confirmation("one revocation callback") { confirm in
            let gate = HostWebSocketOpenGate { confirm() }
            let error = gate.handleClose(.policyViolation)
            gate.handleClose(.policyViolation)
            #expect(!PairingRecovery.isWorthRetrying(error))
            do {
                try await gate.waitForOpen(seconds: 0.5)
                Issue.record("A 1008 close must not open")
            } catch HostError.deviceRevoked {
                // Expected even when the close arrived before waitForOpen.
            } catch {
                Issue.record("Expected deviceRevoked, got \(error)")
            }
        }
    }

    @Test func policyCloseAfterOpenStillPublishesRevocation() async throws {
        await confirmation("revocation after handshake") { confirm in
            let gate = HostWebSocketOpenGate { confirm() }
            let task = URLSession.shared.webSocketTask(with: URL(string: "ws://localhost:53231/ws")!)
            gate.urlSession(URLSession.shared, webSocketTask: task, didOpenWithProtocol: nil)
            gate.handleClose(.policyViolation)
            task.cancel()
        }
    }

    @Test func nonPolicyCloseIsRetryableAndDoesNotPublishRevocation() async {
        await confirmation("no revocation on network close", expectedCount: 0) { confirm in
            let gate = HostWebSocketOpenGate { confirm() }
            #expect(PairingRecovery.isWorthRetrying(gate.handleClose(.goingAway)))
        }
    }

    @Test func revokedSocketsCannotReconnectEvenAfterStop() async {
        let origin = URL(string: "http://localhost:53232")!
        let credentials = HostCredentials(accessURL: origin, bearer: "test-device-token")
        let terminal = TerminalSocket()
        terminal.revoke()
        terminal.stop()
        do {
            try await terminal.connect(origin: origin, credentials: credentials)
            Issue.record("Revoked terminal must not reconnect")
        } catch HostError.deviceRevoked {} catch { Issue.record("Unexpected error: \(error)") }
        #expect(terminal.isRevoked)
        #expect(!terminal.isConnected)

        let acp = AcpSocket()
        acp.revoke()
        acp.stop()
        do {
            try await acp.connect(origin: origin, credentials: credentials)
            Issue.record("Revoked ACP must not reconnect")
        } catch HostError.deviceRevoked {} catch { Issue.record("Unexpected error: \(error)") }
        #expect(acp.state == .revoked)
    }

    @Test func revocationNotificationLatchesWorkspaceWithoutUnmountingOrChangingGeometry() async {
        let session = WorkspaceSession(accessURL: URL(string: "http://localhost:53233")!, bearer: "test")
        session.workspace = .project
        session.terminalSurfaceVisible = true
        session.terminals.geometryActive = true
        let pty = LiveTerminal(id: "existing-pty", claim: "existing-claim", lastSeq: 12,
                               cols: 83, rows: 25, title: "running", owned: true)
        session.terminals.terminals = [pty]
        session.terminals.activeId = pty.id

        HostError.deviceRevoked.publishAccessFailure(origin: session.origin)
        #expect(session.phase == .revoked)
        session.stop()
        session.handleScene(isBackground: true)
        session.handleScene(isBackground: false)
        await session.start()
        #expect(session.phase == .revoked)
        #expect(session.acp.state == .revoked)
        #expect(session.terminalSocket.isRevoked)
        #expect(session.workspace == .project)
        #expect(session.terminalSurfaceVisible)
        #expect(session.terminals.geometryActive)
        #expect(session.terminals.terminals == [pty])
        #expect(session.terminals.activeId == pty.id)
    }

    @Test func permissionNoticeCarriesOriginAndDoesNotRevokeWorkspace() async {
        let session = WorkspaceSession(accessURL: URL(string: "http://localhost:53234")!, bearer: "test")
        let originString = session.origin.absoluteString
        await confirmation("scope notice") { confirm in
            let observer = NotificationCenter.default.addObserver(
                forName: .hostCapabilityDenied, object: nil, queue: .main
            ) { notification in
                #expect(notification.object as? String == "files")
                #expect(notification.userInfo?["origin"] as? String == originString)
                confirm()
            }
            defer { NotificationCenter.default.removeObserver(observer) }
            HostError.forbidden("files").publishAccessFailure(origin: session.origin)
        }
        #expect(session.phase == .idle)
        #expect(!session.terminalSocket.isRevoked)
        #expect(session.acp.state == .idle)
    }

    @Test func anotherOriginsRevocationDoesNotAffectWorkspace() {
        let session = WorkspaceSession(accessURL: URL(string: "http://localhost:53235")!, bearer: "test")
        HostError.deviceRevoked.publishAccessFailure(origin: URL(string: "http://localhost:53236")!)
        #expect(session.phase == .idle)
        #expect(!session.terminalSocket.isRevoked)
    }
}
