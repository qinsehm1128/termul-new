import Foundation
import SwiftTerm
import Testing
import UIKit
@testable import SeRemote

/// Two replay defects share one symptom: the canvas paints a stream that is
/// not the one it thinks it has.
///
/// 1. The host caps its output log (256 KiB). When the client's cursor fell
///    behind the oldest retained chunk it replays from there and flags the
///    stream as gapped. Those bytes begin mid-sequence, so they cannot be
///    painted onto whatever state the emulator already holds.
/// 2. Switching tabs installs the new terminal's feed only after its canvas
///    remounts. A replay that arrives in between must not be dropped by the
///    previous tab's feed.
@Suite("Terminal replay continuity")
@MainActor
struct TerminalReplayContinuityTests {
    /// 390x700 phone viewport: the fitted grid differs from SwiftTerm's 80x25
    /// startup grid, so the replay exercises the takeover path.
    private let phoneBounds = CGRect(x: 0, y: 0, width: 390, height: 700)

    private func makeCanvas() -> (PhoneTerminalView, TerminalScreen.Coordinator) {
        let view = PhoneTerminalView(frame: .zero)
        let coordinator = TerminalScreen.Coordinator(
            onSend: { _ in },
            onResize: { _, _ in },
            onTextScaleChange: { _, _ in }
        )
        coordinator.attach(view)
        return (view, coordinator)
    }

    private func fit(_ view: PhoneTerminalView, _ coordinator: TerminalScreen.Coordinator) async {
        view.frame = phoneBounds
        view.layoutSubviews()
        await Task.yield()
        await Task.yield()
        coordinator.flushFeedIfFitted(view)
    }

    private func visibleRows(_ view: PhoneTerminalView) -> [String] {
        let terminal = view.getTerminal()
        return (0..<terminal.rows).map { row in
            terminal.getText(
                start: Position(col: 0, row: row),
                end: Position(col: terminal.cols - 1, row: row)
            )
        }
    }

    private func liveTerminal(_ id: String) -> LiveTerminal {
        LiveTerminal(
            id: id,
            claim: nil,
            lastSeq: 0,
            cols: 80,
            rows: 24,
            title: "zsh",
            cwd: "/repo",
            gitBranch: nil,
            owned: false
        )
    }

    // MARK: - Gapped replay

    @Test("A gapped replay resets the canvas before feeding it")
    func gappedReplayResetsTheCanvas() async {
        let (view, coordinator) = makeCanvas()
        // Dirty the canvas with output from before the gap, including a scroll
        // region: a replay starting mid-sequence would inherit it.
        let preGap = Data("PRE-GAP-BANNER\r\n\u{1b}[1;1r\u{1b}[31m".utf8)
        coordinator.receive(preGap, into: view)
        await fit(view, coordinator)
        #expect(visibleRows(view).contains { $0.contains("PRE-GAP-BANNER") })

        coordinator.receive(Data("GAPPED-STREAM\r\n".utf8), reset: true, into: view)

        let rows = visibleRows(view)
        #expect(rows.contains { $0.contains("GAPPED-STREAM") },
                "the gapped replay's own bytes must survive the reset")
        #expect(!rows.contains { $0.contains("PRE-GAP-BANNER") },
                "pre-gap content must not survive the reset")
        let terminal = view.getTerminal()
        #expect(terminal.buffer.scrollTop == 0)
        #expect(terminal.buffer.scrollBottom == terminal.rows - 1,
                "the pre-gap scroll region must be gone")
    }

    @Test("A gapped replay that arrives before layout discards the pre-gap buffer")
    func pendingGappedReplayDiscardsItsPrefix() async {
        let (view, coordinator) = makeCanvas()
        // Neither frame has reached the emulator yet: both are held for the
        // first layout pass. The gap flag means the prefix belongs to the
        // truncated stream and must not be painted at all.
        coordinator.receive(Data("PRE-GAP-BANNER\r\n".utf8), into: view)
        coordinator.receive(Data("GAPPED-STREAM\r\n".utf8), reset: true, into: view)

        await fit(view, coordinator)

        let rows = visibleRows(view)
        #expect(rows.contains { $0.contains("GAPPED-STREAM") })
        #expect(!rows.contains { $0.contains("PRE-GAP-BANNER") })
    }

    @Test("A non-gapped replay is not reset")
    func nonGappedReplayKeepsTheCanvas() async {
        let (view, coordinator) = makeCanvas()
        coordinator.receive(Data("FIRST-FRAME\r\n".utf8), into: view)
        await fit(view, coordinator)

        coordinator.receive(Data("SECOND-FRAME\r\n".utf8), reset: false, into: view)

        let rows = visibleRows(view)
        #expect(rows.contains { $0.contains("FIRST-FRAME") })
        #expect(rows.contains { $0.contains("SECOND-FRAME") })
    }

    // MARK: - Tab-switch replay

    @Test("A replay for a hidden tab is delivered when that tab's canvas installs")
    func bufferedReplayFollowsItsTerminal() {
        let store = TerminalStore()
        store.terminals = [liveTerminal("A"), liveTerminal("B")]
        store.activeId = "A"

        var aBytes = Data()
        store.installFeed(for: "A") { data, _ in aBytes.append(data) }

        // B's watch replay lands while A's canvas is still the installed feed.
        // It must not be dropped on the floor.
        let replay = Data("B-REPLAY".utf8)
        store.deliverOutput(terminalId: "B", data: replay)
        #expect(aBytes.isEmpty, "A's feed must never receive B's bytes")

        var bBytes = Data()
        var bReset: Bool?
        store.installFeed(for: "B") { data, reset in
            bBytes.append(data)
            bReset = reset
        }

        #expect(bBytes == replay)
        #expect(bReset == false)
        #expect(store.pendingOutput["B"] == nil)
    }

    @Test("A gapped replay surfaces its reset through the store")
    func gappedReplaySurfacesReset() {
        let store = TerminalStore()
        store.terminals = [liveTerminal("A")]

        var bytes = Data()
        var resetSeen: Bool?
        store.installFeed(for: "A") { data, reset in
            bytes.append(data)
            resetSeen = reset
        }

        store.deliverOutput(terminalId: "A", data: Data("MID-STREAM".utf8), gap: true)

        #expect(resetSeen == true)
        #expect(bytes == Data("MID-STREAM".utf8))
    }

    @Test("A hidden tab's buffered output is bounded and drops the oldest bytes")
    func pendingOutputIsBounded() {
        let store = TerminalStore()
        store.terminals = [liveTerminal("B")]
        let limit = TerminalStore.pendingOutputLimit

        store.deliverOutput(terminalId: "B", data: Data(repeating: 0x41, count: limit + 64))
        store.deliverOutput(terminalId: "B", data: Data("TAIL".utf8))

        #expect(store.pendingOutput["B"]?.count == limit)

        var captured = Data()
        var capturedReset = false
        store.installFeed(for: "B") { data, reset in
            captured = data
            capturedReset = reset
        }
        #expect(captured.count == limit)
        #expect(captured.suffix(4) == Data("TAIL".utf8), "newest bytes win")
        #expect(capturedReset, "a truncated buffer starts mid-sequence and must reset")
    }

    // MARK: - Wire mapping

    @Test("A replay frame carries the host's gap verdict")
    func replayFrameCarriesGap() {
        let gapped = TerminalSocket.output(from: [
            "type": "replay",
            "terminalId": "t1",
            "gap": true,
            "chunks": [["seq": 3, "data": [65, 66]]]
        ])
        #expect(gapped?.gap == true)
        #expect(gapped?.data == Data([65, 66]))

        let live = TerminalSocket.output(from: [
            "type": "data",
            "terminalId": "t1",
            "data": [67]
        ])
        #expect(live?.gap == false)
    }
}
