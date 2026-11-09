import CoreGraphics
import Testing
@testable import SeRemote

@Suite("Terminal grid fit")
struct TerminalGridFitTests {
    /// The canvas on a 420 pt portrait phone (iPhone Air class) after the
    /// full-bleed bar (44), dock (≈108) and home indicator are removed.
    private let portraitKeyboardDown = CGSize(width: 420, height: 685)
    /// Same canvas with the system keyboard (≈336 pt) covering the bottom.
    private let portraitKeyboardUp = CGSize(width: 420, height: 349)
    /// The same phone rotated, still full-bleed.
    private let landscape = CGSize(width: 912, height: 260)

    @Test("A portrait phone shows 60-64 legible columns at the default scale")
    func portraitColumns() {
        let fit = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        #expect((60 ... 64).contains(fit.cols), "got \(fit.cols) columns")
        #expect(fit.rows >= 20)
        #expect(fit.fontSize >= TerminalGridFit.minimumLegibleFontSize)
        #expect(fit.fontSize <= 13)
    }

    @Test("Every phone width keeps a usable column count at the default scale")
    func columnsAcrossPhoneWidths() {
        for width in stride(from: 300.0, through: 440.0, by: 20) {
            let fit = TerminalGridFit.phoneFit(
                size: CGSize(width: width, height: 500),
                textScale: 1.25
            )
            #expect(fit.cols >= 45, "width \(width) gave \(fit.cols) columns")
            #expect(fit.fontSize >= TerminalGridFit.minimumLegibleFontSize)
        }
    }

    @Test("The column target is derived from the measured cell metrics")
    func measuredMetricsKeepColumnTarget() {
        // The view measures the real face: its advance per point can vary a
        // little across fonts and OS versions. The grid must stay in the
        // 60-64 column band instead of depending on an assumed ratio.
        for advance in [CGFloat(0.58), 0.60, 0.62, 0.66, 0.70] {
            let fit = TerminalGridFit.phoneFit(
                size: portraitKeyboardDown,
                textScale: 1.25,
                advanceRatio: advance
            )
            #expect((60 ... 64).contains(fit.cols), "advance \(advance) gave \(fit.cols) columns")
            #expect(fit.fontSize >= TerminalGridFit.minimumLegibleFontSize)
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

    @Test("Rotation adds columns instead of magnifying the glyphs")
    func rotationReshapesGrid() {
        let portrait = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 1.25)
        let rotated = TerminalGridFit.phoneFit(size: landscape, textScale: 1.25)
        #expect(rotated.cols >= 110, "landscape showed \(rotated.cols) columns")
        #expect(rotated.cols > portrait.cols)
        #expect(rotated.rows < portrait.rows)
        // The font is sized from the reference portrait width, so rotating
        // reveals more columns rather than enlarging text.
        #expect(rotated.fontSize == portrait.fontSize)
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

    @Test("Zoom stays inside the font bounds")
    func zoomBounds() {
        let tiny = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 0.01)
        let huge = TerminalGridFit.phoneFit(size: portraitKeyboardDown, textScale: 20)
        #expect(tiny.fontSize == TerminalGridFit.minimumFontSize)
        #expect(huge.fontSize == TerminalGridFit.maximumFontSize)
        // Zoom out still yields a usable row count for a very small font.
        #expect(tiny.rows > huge.rows)
    }

    @Test("Desktop mode holds the host column count")
    func desktopKeepsHostColumns() {
        let fit = TerminalGridFit.desktopFit(size: portraitKeyboardDown, hostCols: 80, textScale: 1.25)
        #expect(fit.cols == 80)
        #expect(fit.fontSize <= 12)
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
