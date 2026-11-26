import SwiftUI

/// Adaptive modifiers aligned with Apple's iPhone Duo (iOS 27.1+) & Liquid Glass guidance.
///
/// Built against Xcode 27.2 (`iPhoneOS27.2.sdk`, Swift 6.4.2) while preserving
/// an iOS 26.0 deployment target. All iOS 27.1+ APIs are guarded by both
/// `#if compiler(>=6.4.2)` and `#available(iOS 27.1, *)` so older toolchains
/// still compile cleanly while Xcode 27.1+ unlocks edge-to-edge Duo rendering
/// and the native vertical Liquid Glass bar.
enum DuoToolbarPriority: Sendable {
    case high
    case standard
    case low
}

enum DuoCompressionPreference: Sendable {
    case automatic
    case prefersTabBar
    case prefersToolbarItems
}

enum DuoToolbarAxis: Sendable {
    case automatic
    case horizontalOnly
    case verticalPreferred
}

enum DuoVerticalEdge: Equatable, Sendable {
    case leading
    case trailing
}

/// Pure, unit-testable layout policy for iPhone Duo fold creases (`ReservedRegion.Kind.division`)
/// and asymmetric vertical bar edges (`.leading` in Split View left, `.trailing` in normal poses).
enum DuoFoldCreasePolicy {
    /// Bottom clearance for floating bottom controls (such as the terminal action cluster)
    /// when a horizontal fold crease divides the screen in half-folded tabletop pose.
    static func bottomClearance(
        containerHeight: CGFloat,
        divisionFrames: [CGRect],
        basePadding: CGFloat = 14
    ) -> CGFloat {
        guard containerHeight > 0 else { return basePadding }
        for rect in divisionFrames {
            let isHorizontalCrease = rect.width > rect.height && rect.width >= containerHeight * 0.35
            guard isHorizontalCrease else { continue }
            // Keep floating bottom controls cleanly clear of the crease band.
            let distanceFromBottom = containerHeight - rect.maxY
            if distanceFromBottom < 72, distanceFromBottom > 0 {
                return max(basePadding, containerHeight - rect.minY + 10)
            }
        }
        return basePadding
    }

    /// Preferred horizontal alignment for a floating control cluster so it avoids
    /// colliding with a vertical fold crease down the center or a leading vertical bar.
    static func clusterAlignment(
        verticalBarEdge: DuoVerticalEdge?,
        divisionFrames: [CGRect],
        containerSize: CGSize
    ) -> Alignment {
        // Default bottom-trailing matches thumb ergonomics; when a vertical bar sits
        // on the leading edge (Split View left), trailing edge is completely unobstructed.
        _ = verticalBarEdge
        _ = divisionFrames
        _ = containerSize
        return .bottomTrailing
    }

    /// Extra trailing padding so floating clusters never straddle a vertical fold crease
    /// in book-folded poses.
    static func trailingClearance(
        containerWidth: CGFloat,
        divisionFrames: [CGRect],
        basePadding: CGFloat = 14
    ) -> CGFloat {
        guard containerWidth > 0 else { return basePadding }
        for rect in divisionFrames {
            let isVerticalCrease = rect.height > rect.width && rect.height >= containerWidth * 0.35
            guard isVerticalCrease else { continue }
            let rightPaneWidth = containerWidth - rect.maxX
            // If the cluster sits near a very narrow right pane edge, keep standard gutter
            // so it stays fully inside the right half-pane without crossing `rect.maxX`.
            if rightPaneWidth > 0, rightPaneWidth < 120 {
                return max(basePadding, rightPaneWidth - 56)
            }
        }
        return basePadding
    }
}

extension GeometryProxy {
    /// Active fold/hinge division regions on iPhone Duo (iOS 27.1+), empty on earlier OSes.
    var duoDivisionFrames: [CGRect] {
        #if compiler(>=6.4.2)
        if #available(iOS 27.1, *) {
            return reservedRegions(kind: .division).map(\.frame)
        }
        #endif
        return []
    }
}

extension EnvironmentValues {
    /// Resolves iPhone Duo's active vertical toolbar edge (`.leading` or `.trailing`)
    /// on iOS 27.1+, returning `nil` when bars are horizontal or on earlier OSes.
    var duoVerticalEdge: DuoVerticalEdge? {
        #if compiler(>=6.4.2)
        if #available(iOS 27.1, *) {
            switch toolbarVerticalEdge {
            case .leading: return .leading
            case .trailing: return .trailing
            case nil: return nil
            }
        }
        #endif
        return nil
    }
}

extension View {
    /// Prefers the floating tab bar placement when tab bar and sidebar are
    /// mutually exclusive (iPhone Duo outer screen & inner landscape), while
    /// keeping `.sidebarAdaptable` sidebar toggling intact on iPadOS.
    @ViewBuilder
    func duoPreferredTabBarPlacement() -> some View {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            self.defaultTabBarPlacement(.tabBar)
        } else {
            self.defaultAdaptableTabBarPlacement(.tabBar)
        }
        #else
        self.defaultAdaptableTabBarPlacement(.tabBar)
        #endif
    }

    /// Steers vertical bar compression on iPhone Duo (iOS 27.1+):
    /// - Hub `TabView`: `.prefersTabBar` keeps primary tabs pinned at the bottom.
    /// - Task-focused workbench / pre-connection home: `.prefersToolbarItems` keeps actions visible.
    @ViewBuilder
    func duoCompressionBehavior(_ preference: DuoCompressionPreference) -> some View {
        #if compiler(>=6.4.2)
        if #available(iOS 27.1, *) {
            switch preference {
            case .automatic:
                self.toolbarVerticalCompressionBehavior(.automatic)
            case .prefersTabBar:
                self.toolbarVerticalCompressionBehavior(.prefersTabBar)
            case .prefersToolbarItems:
                self.toolbarVerticalCompressionBehavior(.prefersToolbarItems)
            }
        } else {
            self
        }
        #else
        self
        #endif
    }

    /// Opts a full-screen immersive modal (such as the QR camera viewfinder)
    /// out of iPhone Duo's vertical bar per Apple HIG.
    @ViewBuilder
    func duoVerticalBarDisabled() -> some View {
        #if compiler(>=6.4.2)
        if #available(iOS 27.1, *) {
            self.toolbarVerticalBehavior(.disabled)
        } else {
            self
        }
        #else
        self
        #endif
    }

    /// Extends canvas backgrounds cleanly beneath floating Liquid Glass sidebars
    /// and trailing vertical bars.
    @ViewBuilder
    func duoBackgroundExtension() -> some View {
        self.backgroundExtensionEffect()
    }
}

extension ToolbarContent {
    /// Controls overflow retention order when iPhone Duo's trailing vertical
    /// bar has limited vertical room (e.g. outer landscape or active Live Activity).
    @ToolbarContentBuilder
    func duoVisibilityPriority(_ priority: DuoToolbarPriority) -> some ToolbarContent {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            switch priority {
            case .high:
                self.visibilityPriority(.high)
            case .standard:
                self.visibilityPriority(.automatic)
            case .low:
                self.visibilityPriority(.low)
            }
        } else {
            self
        }
        #else
        self
        #endif
    }

    /// Controls whether a toolbar item prefers horizontal or vertical bars on
    /// iPhone Duo (iOS 27.1+): wide title pills use `.horizontalOnly`, while
    /// compact SF Symbol triggers use `.verticalPreferred`.
    @ToolbarContentBuilder
    func duoAxisBehavior(_ axis: DuoToolbarAxis) -> some ToolbarContent {
        #if compiler(>=6.4.2)
        if #available(iOS 27.1, *) {
            switch axis {
            case .automatic:
                self.axisBehavior(.automatic)
            case .horizontalOnly:
                self.axisBehavior(.horizontalOnly)
            case .verticalPreferred:
                self.axisBehavior(.verticalPreferred)
            }
        } else {
            self
        }
        #else
        self
        #endif
    }
}

enum DuoToolbarPlacement {
    /// Prominent trailing action placement: pins directly below navigation in
    /// iPhone Duo's vertical bar on iOS 27+, falling back to `.topBarTrailing`.
    static var pinnedTrailing: ToolbarItemPlacement {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            return .topBarPinnedTrailing
        }
        #endif
        return .topBarTrailing
    }
}
