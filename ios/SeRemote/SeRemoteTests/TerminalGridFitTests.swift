import CoreGraphics
import Testing
@testable import SeRemote

@Suite("Terminal grid fit")
struct TerminalGridFitTests {
    /// Canvas bounds inside the console's 14 pt screen gutter and 13 pt canvas
    /// padding on an iPhone 15 (393 × 852).
    private let portraitKeyboardDown = CGSize(width: 339, height: 590)
    /// Same viewport with the system keyboard (≈336 pt) covering the bottom.
    private let portraitKeyboardUp = CGSize(width: 339, height: 254)
    /// The same phone rotated: 852 × 393 with the chrome and dock removed.
    private let landscape = CGSize(width: 798, height: 190)

    @Test("A portrait phone shows a real terminal width at the default scale")
    func portraitColumns() {
        let fit = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        #expect(fit.cols >= 45)
        #expect(fit.rows >= 20)
        #expect(fit.fontSize >= 10)
        #expect(fit.fontSize <= 13)
    }

    @Test("Every phone width keeps at least 45 columns at the default scale")
    func columnsAcrossPhoneWidths() {
        for width in stride(from: 300.0, through: 440.0, by: 20) {
            let fit = TerminalGridFit.phoneFit(
                size: CGSize(width: width, height: 500),
                textScale: 1.25
            )
            #expect(fit.cols >= 45, "width \(width) gave \(fit.cols) columns")
        }
    }

    @Test("The grid never overflows the viewport it was fitted for")
    func gridFitsViewport() {
        let sizes = [portraitKeyboardDown, portraitKeyboardUp, landscape]
        for size in sizes {
            let fit = TerminalGridFit.phoneFit(size: size, textScale: 1.25)
            #expect(CGFloat(fit.cols) * fit.fontSize * TerminalGridFit.defaultAdvanceRatio <= size.width)
            #expect(CGFloat(fit.rows) * fit.fontSize * TerminalGridFit.defaultLineHeightRatio <= size.height)
        }
    }

    @Test("Hiding the keyboard grows rows and keeps the columns")
    func keyboardToggleKeepsColumns() {
        let up = TerminalGridFit.phoneFit(size: portraitKeyboardUp, textScale: 1.25)
        let down = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        #expect(up.cols == down.cols)
        #expect(up.rows < down.rows)
        // The freed keyboard height becomes terminal rows, not empty canvas.
        #expect(down.rows - up.rows >= 20)
        #expect(CGFloat(down.rows) * down.fontSize * TerminalGridFit.defaultLineHeightRatio > portraitKeyboardUp.height)
    }

    @Test("Rotation widens the grid and shortens it")
    func rotationReshapesGrid() {
        let portrait = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        let rotated = TerminalGridFit.phoneFit(size: landscape, textScale: 1.25)
        #expect(rotated.cols > portrait.cols)
        #expect(rotated.rows < portrait.rows)
        // Landscape is wide enough to stop shrinking the font.
        #expect(rotated.fontSize == TerminalGridFit.maximumFontSize)
    }

    @Test("Zooming in trades columns for text size")
    func zoomTradesColumns() {
        let small = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 0.75)
        let normal = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        let large = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 2)
        #expect(small.fontSize < normal.fontSize)
        #expect(normal.fontSize < large.fontSize)
        #expect(small.cols > normal.cols)
        #expect(normal.cols > large.cols)
        #expect(large.cols >= 25)
    }

    @Test("Desktop mode holds the host column count")
    func desktopKeepsHostColumns() {
        let fit = TerminalGridFit.desktopFit(size: portraitKeyboardDown, hostCols: 80, textScale: 1.25)
        #expect(fit.cols == 80)
        #expect(fit.fontSize <= 9)
        // Rows still come from the viewport, so the canvas is not half empty.
        #expect(fit.rows >= 30)
    }

    @Test("Desktop mode floors the host column count")
    func desktopFloorsTinyHostGrid() {
        let fit = TerminalGridFit.desktopFit(size: portraitKeyboardDown, hostCols: 12, textScale: 1.25)
        #expect(fit.cols == 20)
    }

    @Test("A collapsed viewport still yields a usable grid")
    func degenerateViewport() {
        let fit = TerminalGridFit.phoneFit(size: .zero, textScale: 1.25)
        #expect(fit.fontSize >= TerminalGridFit.minimumFontSize)
        #expect(fit.cols >= TerminalGridFit.minimumColumns)
        #expect(fit.rows >= TerminalGridFit.minimumRows)
    }
}
