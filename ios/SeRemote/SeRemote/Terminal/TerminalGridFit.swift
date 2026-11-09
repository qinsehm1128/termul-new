import CoreGraphics

/// Font and grid geometry for the SwiftTerm canvas.
///
/// SwiftTerm derives its grid from the view bounds and the current font's cell
/// metrics (`cols = width / cellWidth`, `rows = height / cellHeight`). With a
/// fixed 16.5 pt phone font that reports ~35 columns on a portrait phone: big
/// glyphs, a half-empty canvas and a grid that never looked like a terminal.
///
/// Phone mode therefore picks the font size the viewport needs to show
/// `minimumPhoneColumns` at `TerminalTextScale.defaultValue`, then derives the
/// grid from the same cell ratios the view measures. Desktop mode keeps the
/// host column count and fits the font to it instead. Both modes run on every
/// bounds change, so the keyboard showing/hiding and rotation land here.
enum TerminalGridFit {
    /// Columns a portrait phone shows at the default text scale.
    static let minimumPhoneColumns = 46
    /// Floor for degenerate viewports; the host push clamps harder.
    static let minimumColumns = 8
    static let minimumRows = 1
    /// SF Mono cell metrics per point of font size, used until the view
    /// measures the real face. `W` advance and line height are linear in size.
    static let defaultAdvanceRatio: CGFloat = 0.6
    static let defaultLineHeightRatio: CGFloat = 1.25
    static let minimumFontSize: CGFloat = 6
    static let maximumFontSize: CGFloat = 22

    /// A resolved font size and the grid its metrics allow in `size`.
    nonisolated struct Fit: Equatable, Sendable {
        var fontSize: CGFloat
        var cols: Int
        var rows: Int
    }

    /// Phone fit: as many rows as the viewport holds, and a font small enough
    /// to keep `minimumPhoneColumns` at the default text scale. Zooming out
    /// (`textScale` above the default) trades columns for larger glyphs.
    static func phoneFit(
        size: CGSize,
        textScale: CGFloat,
        advanceRatio: CGFloat = defaultAdvanceRatio,
        lineHeightRatio: CGFloat = defaultLineHeightRatio,
        minimumColumns: Int = minimumPhoneColumns
    ) -> Fit {
        let advance = max(advanceRatio, 0.1)
        let lineHeight = max(lineHeightRatio, 0.5)
        let width = max(size.width, 0)
        let height = max(size.height, 0)
        let scale = max(textScale, 0.25)
        // A zoomed-in terminal keeps its columns in proportion, so the column
        // target shrinks as the text grows.
        let relative = scale / max(TerminalTextScale.defaultValue, 0.25)
        let targetColumns = max(Self.minimumColumns, Int((CGFloat(minimumColumns) / relative).rounded()))
        // Round the font down to the half point: rounding up would drop a
        // column and undo the target this computed it from.
        let raw = width > 0
            ? width / (CGFloat(targetColumns) * advance)
            : 13 * scale
        let fontSize = clamp((raw * 2).rounded(.down) / 2)
        return fit(size: size, fontSize: fontSize, cols: nil, advanceRatio: advance, lineHeightRatio: lineHeight)
    }

    /// Desktop fit: hold the host column count and size the font to it.
    static func desktopFit(
        size: CGSize,
        hostCols: Int,
        textScale: CGFloat,
        advanceRatio: CGFloat = defaultAdvanceRatio,
        lineHeightRatio: CGFloat = defaultLineHeightRatio
    ) -> Fit {
        let advance = max(advanceRatio, 0.1)
        let lineHeight = max(lineHeightRatio, 0.5)
        let cols = max(hostCols, 20)
        let width = max(size.width, 0)
        let raw = width > 0
            ? (width / (CGFloat(cols) * advance)) * max(textScale, 0.25)
            : 12
        // Desktop keeps its historical round-to-nearest: the host grid can be
        // wider than the phone and is mirrored, not reflowed.
        let fontSize = clamp((raw * 2).rounded() / 2)
        return fit(size: size, fontSize: fontSize, cols: cols, advanceRatio: advance, lineHeightRatio: lineHeight)
    }

    /// Grid that `fontSize` fits in `size`, never overflowing the viewport.
    static func fit(
        size: CGSize,
        fontSize: CGFloat,
        cols: Int?,
        advanceRatio: CGFloat = defaultAdvanceRatio,
        lineHeightRatio: CGFloat = defaultLineHeightRatio
    ) -> Fit {
        let advance = max(advanceRatio, 0.1)
        let lineHeight = max(lineHeightRatio, 0.5)
        let width = max(size.width, 0)
        let height = max(size.height, 0)
        let cellWidth = fontSize * advance
        let cellHeight = fontSize * lineHeight
        return Fit(
            fontSize: fontSize,
            cols: cols ?? max(minimumColumns, Int((width / cellWidth).rounded(.down))),
            rows: max(minimumRows, Int((height / cellHeight).rounded(.down)))
        )
    }

    private static func clamp(_ size: CGFloat) -> CGFloat {
        min(maximumFontSize, max(minimumFontSize, size))
    }
}
