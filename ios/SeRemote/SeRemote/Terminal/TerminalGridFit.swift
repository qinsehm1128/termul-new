import CoreGraphics

/// Font and grid geometry for the SwiftTerm canvas.
///
/// SwiftTerm derives its grid from the view bounds and the current font's cell
/// metrics (`cols = width / cellWidth`, `rows = height / cellHeight`). Phone
/// mode therefore fixes the glyph size from a *reference* portrait width —
/// never from the live width — so rotating the phone adds columns instead of
/// magnifying the same grid. The grid itself is still derived from the real
/// cell metrics the view measures (`W` advance and line height per point), so
/// the font and the columns always agree.
///
/// Desktop mode keeps the host column count and fits the font to it instead.
/// Both modes run on every bounds change, so the keyboard showing/hiding and
/// rotation land here.
enum TerminalGridFit {
    /// Columns a reference portrait phone shows at the default text scale.
    /// ~420 pt / (11 pt × 0.6) resolves to 63 columns.
    static let minimumPhoneColumns = 62
    /// The portrait width the font is sized against. A wider (landscape)
    /// viewport then holds proportionally more columns instead of growing the
    /// glyphs past a phone-legible size.
    static let referencePhoneWidth: CGFloat = 420
    /// Floor for degenerate viewports; the host push clamps harder.
    static let minimumColumns = 8
    static let minimumRows = 1
    /// SF Mono cell metrics per point of font size, used until the view
    /// measures the real face. `W` advance and line height are linear in size.
    static let defaultAdvanceRatio: CGFloat = 0.6
    static let defaultLineHeightRatio: CGFloat = 1.25
    static let minimumFontSize: CGFloat = 6
    static let maximumFontSize: CGFloat = 22
    /// The default-scale font on a portrait phone must stay readable.
    static let minimumLegibleFontSize: CGFloat = 9.5

    /// A resolved font size and the grid its metrics allow in `size`.
    nonisolated struct Fit: Equatable, Sendable {
        var fontSize: CGFloat
        var cols: Int
        var rows: Int
    }

    /// Phone fit: as many rows as the viewport holds, and a font sized so the
    /// reference portrait width keeps `minimumPhoneColumns`. Zooming in
    /// (`textScale` above the default) grows the glyphs and trades columns;
    /// zooming out does the reverse.
    static func phoneFit(
        size: CGSize,
        textScale: CGFloat,
        advanceRatio: CGFloat = defaultAdvanceRatio,
        lineHeightRatio: CGFloat = defaultLineHeightRatio,
        minimumColumns: Int = minimumPhoneColumns,
        referenceWidth: CGFloat = referencePhoneWidth
    ) -> Fit {
        let advance = max(advanceRatio, 0.1)
        let lineHeight = max(lineHeightRatio, 0.5)
        let height = max(size.height, 0)
        let scale = max(textScale, 0.25)
        // Zoom is relative to the persisted default, so changing the design
        // default does not silently re-scale an existing user's grid.
        let relative = scale / max(TerminalTextScale.defaultValue, 0.25)
        let columns = max(minimumColumns, Self.minimumColumns)
        // Glyph size follows the reference portrait width, never the live
        // bounds: a landscape viewport must gain columns, not magnification.
        let raw = referenceWidth > 0
            ? referenceWidth * relative / (CGFloat(columns) * advance)
            : 13 * scale
        // Round the font down to the half point so the reference width still
        // holds the target column count at the default scale.
        let fontSize = clamp((raw * 2).rounded(.down) / 2)
        return fit(
            size: CGSize(width: max(size.width, 0), height: height),
            fontSize: fontSize,
            cols: nil,
            advanceRatio: advance,
            lineHeightRatio: lineHeight
        )
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
