import Foundation
import Testing
@testable import SeRemote

/// Bug #1 regression suite: backgrounding the app triggered spurious
/// full-screen reconnects, and after an auto-reconnect every surface showed
/// the raw untranslated `"Unauthorized"` ("Un…") string.
///
/// Three independent defects compose here:
/// 1. Transient `.inactive` scene phases were treated as lifecycle events.
/// 2. A reconnected `TerminalSocket` is a fresh server-side connection whose
///    authorization map is empty until `watch`/`attach` re-runs, so
///    `set_display_mode`/`write` answered `("UNAUTHORIZED", "Unauthorized")`.
/// 3. That raw ipc string reached the UI untranslated.
@Suite("Reconnect authorization and scene lifecycle")
struct ReconnectAuthorizationTests {
    // MARK: - Scene lifecycle classification

    @Test("A transient inactive phase is not a lifecycle event")
    func inactiveIsNeverALifecycleEvent() {
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: false, isBackground: false, wasBackgrounded: false
        ) == .none)
        // Returning to active after mere chrome (Control Center, banners,
        // app-switcher peek) is equally inert.
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: true, isBackground: false, wasBackgrounded: false
        ) == .none)
    }

    @Test("Only a real background stay enters background")
    func backgroundEntersOnce() {
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: false, isBackground: true, wasBackgrounded: false
        ) == .enterBackground)
        // Repeated background samples while already backgrounded are not new.
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: false, isBackground: true, wasBackgrounded: true
        ) == .none)
    }

    @Test("Active after a background stay is the foreground return")
    func activeAfterBackgroundReturnsToForeground() {
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: true, isBackground: false, wasBackgrounded: true
        ) == .enterForeground)
    }

    @Test("Active after a transient inactive stays inert")
    func activeAfterInactiveIsStillNone() {
        #expect(ShellConnectionLifecycle.sceneAction(
            isActive: true, isBackground: false, wasBackgrounded: false
        ) == .none)
    }

    // MARK: - Output frame sequence cursor

    @Test("A live data frame carries its host seq")
    func liveFrameExposesSeq() {
        let frame = TerminalSocket.outputFrame(from: [
            "type": "data",
            "terminalId": "terminal-1",
            "seq": 41,
            "data": [65, 66, 10]
        ])
        #expect(frame?.terminalId == "terminal-1")
        #expect(frame?.data == Data([65, 66, 10]))
        #expect(frame?.gap == false)
        #expect(frame?.latestSeq == 41)
    }

    @Test("A replay frame exposes latestSeq")
    func replayFrameExposesLatestSeq() {
        let frame = TerminalSocket.outputFrame(from: [
            "type": "replay",
            "terminalId": "terminal-2",
            "gap": false,
            "latestSeq": 12,
            "chunks": [
                ["seq": 11, "data": [104, 105]],
                ["seq": 12, "data": [10]]
            ]
        ])
        #expect(frame?.latestSeq == 12)
        #expect(frame?.data == Data([104, 105, 10]))
        #expect(frame?.gap == false)
    }

    @Test("A replay frame without latestSeq falls back to the last chunk seq")
    func replayFrameFallsBackToLastChunkSeq() {
        let frame = TerminalSocket.outputFrame(from: [
            "type": "replay",
            "terminalId": "terminal-3",
            "gap": true,
            "chunks": [
                ["seq": 7, "data": [120]],
                ["seq": 9, "data": [121]]
            ]
        ])
        #expect(frame?.latestSeq == 9)
        #expect(frame?.gap == true)
    }

    @Test("Non-output frames and seq-less frames map to nil")
    func nonOutputFramesMapToNil() {
        #expect(TerminalSocket.outputFrame(from: ["type": "event"]) == nil)
        #expect(TerminalSocket.outputFrame(from: ["type": "data"]) == nil)
        #expect(TerminalSocket.outputFrame(from: ["type": "replay", "chunks": []]) == nil)
        // A frame without a cursor still maps (older hosts), just seq-less.
        let legacy = TerminalSocket.outputFrame(from: [
            "type": "data",
            "terminalId": "terminal-4",
            "data": [10]
        ])
        #expect(legacy != nil)
        #expect(legacy?.latestSeq == nil)
    }

    @Test("The legacy tuple mapping stays byte-identical")
    func legacyTupleMappingUnchanged() {
        let output = TerminalSocket.output(from: [
            "type": "data",
            "terminalId": "terminal-5",
            "seq": 99,
            "data": [111, 107]
        ])
        #expect(output?.terminalId == "terminal-5")
        #expect(output?.data == Data([111, 107]))
        #expect(output?.gap == false)
        // The tuple API has no cursor position to fill.
        #expect(Mirror(reflecting: output!).children.count == 3)
    }

    // MARK: - UNAUTHORIZED localization

    @Test("UNAUTHORIZED never surfaces the raw Unauthorized literal")
    func unauthorizedCodeIsLocalized() {
        let byCode = HostError.ipc("Unauthorized", "UNAUTHORIZED")
        #expect(byCode.localizedDescription != "Unauthorized")
        #expect(!byCode.localizedDescription.isEmpty)

        let byMessage = HostError.ipc("Unauthorized", "NETWORK_ERROR")
        #expect(byMessage.localizedDescription != "Unauthorized")

        let lowercase = HostError.ipc("Unauthorized", "unauthorized")
        #expect(lowercase.localizedDescription != "Unauthorized")
    }

    @Test("Other ipc errors keep their host message")
    func otherIpcErrorsPassThrough() {
        let error = HostError.ipc("Terminal Core owns this shell", "VALIDATION_ERROR")
        #expect(error.localizedDescription == "Terminal Core owns this shell")
    }

    // MARK: - TerminalStore cursor tracking

    @Test("Output frames advance the catalog cursor to the host seq")
    @MainActor
    func storeTracksHostCursor() {
        let store = TerminalStore()
        store.terminals = [LiveTerminal(
            id: "T1", claim: nil, lastSeq: 0, cols: 80, rows: 24,
            title: "zsh", cwd: nil, gitBranch: nil, owned: false
        )]

        // A frame authored at host seq 7 sets the cursor to 7, not to 1.
        store.deliverOutput(terminalId: "T1", data: Data("ls\n".utf8), latestSeq: 7)
        #expect(store.terminals.first?.lastSeq == 7)

        // The cursor never regresses to an older frame's seq.
        store.deliverOutput(terminalId: "T1", data: Data("ok".utf8), latestSeq: 4)
        #expect(store.terminals.first?.lastSeq == 7)

        // It advances with the host.
        store.deliverOutput(terminalId: "T1", data: Data("\n".utf8), latestSeq: 12)
        #expect(store.terminals.first?.lastSeq == 12)

        // Hosts without a seq on the frame keep the monotonic +1 fallback.
        store.deliverOutput(terminalId: "T1", data: Data("!".utf8))
        #expect(store.terminals.first?.lastSeq == 13)
    }

    @Test("The resume cursor survives a catalog refresh that drops the row")
    @MainActor
    func resumeCursorSurvivesCatalogRefresh() {
        let store = TerminalStore()
        store.terminals = [LiveTerminal(
            id: "T1", claim: nil, lastSeq: 0, cols: 80, rows: 24,
            title: "zsh", cwd: nil, gitBranch: nil, owned: false
        )]
        store.deliverOutput(terminalId: "T1", data: Data("build".utf8), latestSeq: 55)
        #expect(store.lastWatchedSeq(for: "T1") == 55)

        // A mid-reconnect catalog refresh (or an eviction) must not lose the
        // cursor the re-watch needs: the side channel outlives the rows.
        store.terminals = []
        #expect(store.lastWatchedSeq(for: "T1") == 55)

        // Unknown terminals resume from zero.
        #expect(store.lastWatchedSeq(for: "unknown") == 0)
    }

    @Test("The socket frame hook delivers each frame exactly once")
    @MainActor
    func socketFrameHookDeliversOnce() async {
        let socket = TerminalSocket()
        let store = TerminalStore()
        store.attach(
            socket: socket,
            origin: URL(string: "http://192.168.6.105:53231")!,
            credentials: HostCredentials(accessURL: URL(string: "http://192.168.6.105:53231")!, bearer: "tok")
        )
        store.terminals = [LiveTerminal(
            id: "T1", claim: nil, lastSeq: 0, cols: 80, rows: 24,
            title: "zsh", cwd: nil, gitBranch: nil, owned: false
        )]
        store.activeId = "T1"

        var deliveries: [Data] = []
        store.installFeed(for: "T1") { data, _ in
            deliveries.append(data)
        }

        guard let frame = TerminalSocket.outputFrame(from: [
            "type": "data", "terminalId": "T1", "seq": 21, "data": [104, 105]
        ]) else {
            Issue.record("frame should map")
            return
        }
        socket.onOutputFrame?(frame)
        // The store coalesces output through a 16 ms MainActor window; under
        // full-suite parallel execution MainActor hops can queue >80 ms.
        for _ in 0 ..< 40 where deliveries.isEmpty {
            try? await Task.sleep(for: .milliseconds(25))
        }
        // Give any accidental duplicate delivery a beat to land as well.
        try? await Task.sleep(for: .milliseconds(30))

        #expect(deliveries.count == 1, "one frame must be delivered exactly once")
        #expect(deliveries.first == Data([104, 105]))
        #expect(store.terminals.first?.lastSeq == 21)
    }

    @Test("A reconnect hook with no live socket is a clean no-op")
    @MainActor
    func recoveryWithoutLiveSocketIsSafe() async {
        let socket = TerminalSocket()
        let store = TerminalStore()
        store.attach(
            socket: socket,
            origin: URL(string: "http://192.168.6.105:53231")!,
            credentials: HostCredentials(accessURL: URL(string: "http://192.168.6.105:53231")!, bearer: "tok")
        )
        store.errorMessage = "previous failure"

        // No connection, no watch, no viewer: recovery must neither crash nor
        // invent requests on a dead socket.
        await store.recoverAfterReconnect()
        #expect(store.errorMessage == "previous failure")
    }

    @Test("Recoverable write classifier retries stale transport and UNAUTHORIZED but never revoked or forbidden")
    func recoverableWriteClassifier() {
        #expect(TerminalStore.isRecoverableWriteFailure(HostError.network("timed out")) == true)
        #expect(TerminalStore.isRecoverableWriteFailure(HostError.ipc("Unauthorized", "UNAUTHORIZED")) == true)
        #expect(TerminalStore.isRecoverableWriteFailure(HostError.ipc("gone", "TERMINAL_NOT_FOUND")) == false)
        #expect(TerminalStore.isRecoverableWriteFailure(HostError.deviceRevoked) == false)
        #expect(TerminalStore.isRecoverableWriteFailure(HostError.forbidden("terminal.input")) == false)
    }
}
