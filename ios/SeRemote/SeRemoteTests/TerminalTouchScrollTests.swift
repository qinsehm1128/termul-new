import CoreGraphics
import SwiftTerm
import Testing
import UIKit
@testable import SeRemote

@Suite("Terminal touch & vertical scroll adaptation")
struct TerminalTouchScrollTests {
    @Test("Scroll routing distinguishes normal shell, mouse-reporting TUI, alt-screen TUI, selection and read-only links")
    func scrollRouteMatrix() {
        #expect(
            TerminalScrollController.route(
                isAlternateBuffer: false,
                mouseModeActive: false,
                allowsInput: true,
                selectionActive: false
            ) == .nativeScrollView
        )
        #expect(
            TerminalScrollController.route(
                isAlternateBuffer: true,
                mouseModeActive: true,
                allowsInput: true,
                selectionActive: false
            ) == .mouseWheel
        )
        #expect(
            TerminalScrollController.route(
                isAlternateBuffer: true,
                mouseModeActive: false,
                allowsInput: true,
                selectionActive: false
            ) == .alternateCursorKeys
        )
        #expect(
            TerminalScrollController.route(
                isAlternateBuffer: true,
                mouseModeActive: true,
                allowsInput: false,
                selectionActive: false
            ) == .none
        )
        #expect(
            TerminalScrollController.route(
                isAlternateBuffer: false,
                mouseModeActive: true,
                allowsInput: true,
                selectionActive: true
            ) == .none
        )
    }

    @Test("Swipe-down keyboard dismissal never fights scrollback or TUI scrolling")
    func keyboardDismissGuard() {
        #expect(
            TerminalScrollController.allowsSwipeDownKeyboardDismiss(
                isFirstResponder: true,
                route: .nativeScrollView,
                hasNormalScrollback: false
            ) == true
        )
        #expect(
            TerminalScrollController.allowsSwipeDownKeyboardDismiss(
                isFirstResponder: true,
                route: .nativeScrollView,
                hasNormalScrollback: true
            ) == false
        )
        #expect(
            TerminalScrollController.allowsSwipeDownKeyboardDismiss(
                isFirstResponder: true,
                route: .mouseWheel,
                hasNormalScrollback: false
            ) == false
        )
        #expect(
            TerminalScrollController.allowsSwipeDownKeyboardDismiss(
                isFirstResponder: false,
                route: .nativeScrollView,
                hasNormalScrollback: false
            ) == false
        )
    }

    @Test("Natural vertical pan maps finger-down to scroll-up and preserves sub-row remainder")
    func naturalPanAccumulation() {
        var accumulator = VerticalPanAccumulator()
        // cellHeight = 16 -> stepHeight = 18.4 pt
        #expect(accumulator.consume(deltaY: 10, cellHeight: 16) == nil)
        let upBatch = accumulator.consume(deltaY: 10, cellHeight: 16)
        #expect(upBatch == .init(up: true, count: 1))

        // Reversing finger upward resets opposite remainder immediately
        #expect(accumulator.consume(deltaY: -10, cellHeight: 16) == nil)
        let downBatch = accumulator.consume(deltaY: -12, cellHeight: 16)
        #expect(downBatch == .init(up: false, count: 1))
    }

    @Test("Fast flick produces bounded decaying momentum batches")
    func momentumFlingBatches() {
        #expect(VerticalPanAccumulator.momentumTicks(velocityY: 120, cellHeight: 16).isEmpty)
        let upBatches = VerticalPanAccumulator.momentumTicks(velocityY: 900, cellHeight: 16)
        #expect(!upBatches.isEmpty)
        let allUp = upBatches.allSatisfy { $0.up }
        #expect(allUp)
        let total = upBatches.reduce(0) { $0 + $1.count }
        #expect(total >= 2 && total <= 18)
    }

    @Test("Accessory catalog exposes repeatable PageUp and PageDown keys")
    func accessoryCatalogHasPageKeys() {
        let pageUp = TerminalAccessoryCatalog.keys.first { $0.id == "pageUp" }
        let pageDown = TerminalAccessoryCatalog.keys.first { $0.id == "pageDown" }
        #expect(pageUp?.bytes == "\u{1b}[5~")
        #expect(pageUp?.repeatable == true)
        #expect(pageDown?.bytes == "\u{1b}[6~")
        #expect(pageDown?.repeatable == true)
    }

    @Test("PhoneTerminalView disables tap-to-focus and emits SGR 64/65 mouse wheel sequences in TUI mode")
    @MainActor
    func phoneTerminalViewEmitsSGRWheelAndRespectsInputPermission() {
        final class CapturingDelegate: NSObject, TerminalViewDelegate {
            var sent: [String] = []
            func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {}
            func setTerminalTitle(source: TerminalView, title: String) {}
            func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
            func send(source: TerminalView, data: ArraySlice<UInt8>) {
                sent.append(String(decoding: data, as: UTF8.self))
            }
            func scrolled(source: TerminalView, position: Double) {}
            func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {}
            func bell(source: TerminalView) {}
            func clipboardCopy(source: TerminalView, content: Data) {}
            func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
            func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
        }

        let view = PhoneTerminalView(frame: CGRect(x: 0, y: 0, width: 390, height: 640))
        view.configureMobileModeIfNeeded()
        #expect(view.tapFocusesKeyboardWhenUnfocused == false)

        let delegate = CapturingDelegate()
        view.terminalDelegate = delegate
        view.layoutIfNeeded()

        // Enter alternate screen + DECSET 1000 button tracking + DECSET 1006 SGR encoding
        view.feed(text: "\u{1b}[?1049h\u{1b}[?1000h\u{1b}[?1006h")
        #expect(view.isShowingAlternateBuffer == true)
        #expect(view.currentScrollRoute == .mouseWheel)

        view.sendMouseWheel(up: true, at: CGPoint(x: 40, y: 40))
        view.sendMouseWheel(up: false, at: CGPoint(x: 40, y: 40))
        #expect(delegate.sent.count == 2)
        #expect(delegate.sent[0].hasPrefix("\u{1b}[<64;"))
        #expect(delegate.sent[1].hasPrefix("\u{1b}[<65;"))

        // Read-only link blocks mouse wheel emission
        delegate.sent.removeAll()
        view.allowsInput = false
        #expect(view.canSendMouseEvents() == false)
        #expect(view.currentScrollRoute == .none)
        view.sendMouseWheel(up: true, at: CGPoint(x: 40, y: 40))
        #expect(delegate.sent.isEmpty)
    }
}
