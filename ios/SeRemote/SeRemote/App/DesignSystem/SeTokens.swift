import SwiftUI

/// Spacing scale (design-spec.md section 4): 4 / 8 / 12 / 16 / 20 / 24 / 32 / 40 / 48.
enum SeSpace {
    static let xxs: CGFloat = 4
    static let xs: CGFloat = 8
    static let sm: CGFloat = 12
    static let md: CGFloat = 16
    static let lg: CGFloat = 20
    static let xl: CGFloat = 24
    static let xxl: CGFloat = 32
    static let xxxl: CGFloat = 40
    static let huge: CGFloat = 48

    /// Horizontal screen inset.
    static let screenInset: CGFloat = lg
    /// Gap between sections.
    static let section: CGFloat = xl
    /// Minimum hit target.
    static let hitTarget: CGFloat = 44
}

/// Corner radii. The count of radii is not a hierarchy signal.
enum SeRadius {
    /// Inline code tags.
    static let tag: CGFloat = 6
    /// Tool surfaces such as the terminal dock and toolbars.
    static let tool: CGFloat = 10
    /// Content containers such as cards and previews.
    static let content: CGFloat = 14
    /// Chat composer.
    static let composer: CGFloat = 18
}

/// Type scale mapped to semantic text styles so Dynamic Type applies.
/// Serif is reserved for identity and empty states.
enum SeFont {
    static let identity: Font = .system(.largeTitle, design: .serif).weight(.semibold)
    static let screenTitle: Font = .system(.title).weight(.bold)
    static let sectionTitle: Font = .system(.title2).weight(.semibold)
    static let rowTitle: Font = .headline
    static let body: Font = .body
    static let support: Font = .subheadline
    static let label: Font = .footnote.weight(.medium)
    static let caption: Font = .caption
    static let timing: Font = .footnote.monospacedDigit()

    /// Monospaced code at a Dynamic-Type-scaled size. Pair with
    /// `@ScaledMetric(relativeTo: .subheadline) var size = SeFont.codeBaseSize`.
    static let codeBaseSize: CGFloat = 14
    static func code(size: CGFloat) -> Font {
        .system(size: size, design: .monospaced)
    }
}

/// Width budgets for adaptive layout (design-spec.md section 9). These are
/// content budgets, not device checks: never branch on device names.
enum SeLayout {
    /// Two columns need a regular size class and at least this usable width.
    static let splitMinimumWidth: CGFloat = 720
    /// A persistent inspector needs this much usable width.
    static let inspectorMinimumWidth: CGFloat = 1120
    /// Split diff needs this much detail width.
    static let splitDiffMinimumWidth: CGFloat = 1000
    static let sidebarWidth: ClosedRange<CGFloat> = 280 ... 320
    static let readableColumnMaxWidth: CGFloat = 680

    enum Columns: Equatable {
        case single
        case split
        case splitWithInspector
    }

    static func columns(width: CGFloat, isRegular: Bool, largeText: Bool = false) -> Columns {
        // Larger Dynamic Type sizes need more room, so fold earlier.
        let scale: CGFloat = largeText ? 1.25 : 1
        guard isRegular, width >= splitMinimumWidth * scale else { return .single }
        return width >= inspectorMinimumWidth * scale ? .splitWithInspector : .split
    }
}
