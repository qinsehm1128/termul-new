import Foundation
import SwiftTerm
import Testing
import UIKit
@testable import SeRemote

/// A takeover replays the host's whole output log. That log is raw bytes laid
/// out for the grid the desktop was using, so the phone cannot paint it as if
/// it were a screen: SwiftTerm's alternate buffer is neither reflowed nor
/// trimmed when the canvas shrinks, and the program's next incremental repaint
/// only overwrites part of the old frame.
///
/// These tests drive the real canvas coordinator, because the defect lived in
/// the order between installing the feed, fitting the grid, and painting.
@Suite("Terminal takeover replay")
@MainActor
struct TerminalTakeoverReplayTests {
    /// 390x700 phone viewport: the fitted grid is narrower and taller than
    /// SwiftTerm's 80x25 startup grid.
    private let phoneBounds = CGRect(x: 0, y: 0, width: 390, height: 700)
    private let desktopGrid = (cols: 160, rows: 40, key: "DESKTOWFRAME")
    private let desktopRows = 40
    private let desktopColumns = 120

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

    /// A full-screen program drawing at the desktop grid, in the alternate
    /// buffer: the same shape a TUI emits on the host.
    private var desktopFrame: Data {
        var bytes = "\u{1b}[?1049h\u{1b}[H\u{1b}[2J"
        for row in 1...desktopRows {
            bytes += "\u{1b}[\(row);1H"
            bytes += "\(desktopGrid.key) r" + String(format: "%02d", row) + " "
            bytes += String(repeating: "=", count: desktopColumns)
        }
        return Data(bytes.utf8)
    }

    /// What a running TUI sends after the takeover resize: only the lines it
    /// considers changed. Everything else keeps whatever the canvas holds.
    private var incrementalRepaint: Data {
        var bytes = ""
        for row in [34, 35, 36] {
            bytes += "\u{1b}[\(row);1H"
            bytes += "phone status \(row) ".padding(toLength: 44, withPad: ".", startingAt: 0)
        }
        return Data(bytes.utf8)
    }

    private var shellHistory: Data {
        Data("shell history line one\r\nshell history line two\r\n".utf8)
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

    /// Let the canvas lay out and SwiftTerm deliver its (main-queue) size
    /// report, then flush the held replay.
    private func fit(_ view: PhoneTerminalView, _ coordinator: TerminalScreen.Coordinator) async {
        view.frame = phoneBounds
        view.layoutSubviews()
        await Task.yield()
        await Task.yield()
        coordinator.flushFeedIfFitted(view)
    }

    @Test("A takeover replay is not painted at the pre-layout startup grid")
    func replayWaitsForTheCanvasGrid() async {
        let (view, coordinator) = makeCanvas()
        coordinator.replayGrid = TerminalReplayGrid(cols: desktopGrid.cols, rows: desktopGrid.rows)

        coordinator.receive(desktopFrame, into: view)

        // Mount time: the view has no size yet, so SwiftTerm is still at 80x25
        // and painting the replay here would author the screen at the wrong
        // grid. Nothing may reach the emulator.
        #expect(coordinator.didApplyReplay == false)
        #expect(view.getTerminal().isCurrentBufferAlternate == false)

        await fit(view, coordinator)

        #expect(coordinator.didApplyReplay == true)
        // The replay was interpreted at the grid the user sees, not the
        // 80-column startup grid it was installed with.
        #expect(view.getTerminal().cols != 80 || view.getTerminal().rows != 25)
    }

    @Test("A takeover leaves no desktop frame under the program's incremental repaint")
    func takeoverDropsTheDesktopFrame() async {
        let (view, coordinator) = makeCanvas()
        coordinator.replayGrid = TerminalReplayGrid(cols: desktopGrid.cols, rows: desktopGrid.rows)
        coordinator.receive(desktopFrame, into: view)

        await fit(view, coordinator)
        #expect(view.getTerminal().isCurrentBufferAlternate)

        // The program repaints the lines it changed after the resize.
        coordinator.receive(incrementalRepaint, into: view)

        let rows = visibleRows(view)
        let leftovers = rows.filter { $0.contains(desktopGrid.key) || $0.contains("===") }
        #expect(leftovers.isEmpty, "visible screen kept the desktop frame: \(leftovers)")
        #expect(rows.contains { $0.contains("phone status 35") })
    }

    @Test("A takeover keeps shell scrollback, which reflows instead of being dropped")
    func takeoverKeepsNormalBufferHistory() async {
        let (view, coordinator) = makeCanvas()
        coordinator.replayGrid = TerminalReplayGrid(cols: desktopGrid.cols, rows: desktopGrid.rows)
        coordinator.receive(shellHistory, into: view)

        await fit(view, coordinator)

        #expect(view.getTerminal().isCurrentBufferAlternate == false)
        let rows = visibleRows(view)
        #expect(rows.contains { $0.contains("shell history line one") })
        #expect(rows.contains { $0.contains("shell history line two") })
    }

    @Test("A replay authored at the canvas grid is painted as-is")
    func matchingGridReplayIsNotCleared() async {
        let (view, coordinator) = makeCanvas()
        coordinator.receive(desktopFrame, into: view)
        await fit(view, coordinator)

        // Without a host-reported grid there is nothing to compare against, so
        // the replay is trusted: a resume must not blank the screen.
        coordinator.replayGrid = nil
        let rows = visibleRows(view)
        #expect(rows.contains { $0.contains(desktopGrid.key) })
    }

    @Test("Replay geometry from an attach reply is only taken when both axes are sane")
    func replayGridValidation() {
        #expect(TerminalReplayGrid(cols: 160, rows: 40) != nil)
        #expect(TerminalReplayGrid(cols: nil, rows: 40) == nil)
        #expect(TerminalReplayGrid(cols: 160, rows: nil) == nil)
        #expect(TerminalReplayGrid(cols: 0, rows: 40) == nil)
        #expect(TerminalReplayGrid(cols: 160, rows: 1) == nil)
    }
}
