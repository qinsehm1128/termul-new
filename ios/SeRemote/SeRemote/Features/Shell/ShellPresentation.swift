import Foundation

/// Shell policy has no device-name checks and never owns a connection lifecycle.
enum ShellPresentation {
    /// Below this width a phone/compact layout is used; above it the iPad rail.
    static let sidebarBreakpoint: Double = 600

    /// The prototype's iPad directory.
    static let railExpandedWidth: CGFloat = 250
    /// The prototype's iPad portrait rail.
    static let railCollapsedWidth: CGFloat = 76

    /// Trailing file/agent inspector column budgets (unified with `SeLayout`).
    static let inspectorMinWidth: CGFloat = SeLayout.sidebarWidth.lowerBound
    static let inspectorIdealWidth: CGFloat = SeLayout.sidebarWidth.upperBound
    static let inspectorMaxWidth: CGFloat = 380
    /// Auto-open the inspector only on roomy landscape canvases (e.g. iPad landscape),
    /// keeping iPhone Duo inner screen (669×951 / 951×669) and iPad portrait
    /// collapsed by default so the terminal canvas is never crushed.
    static let inspectorAutoOpenMinimumWidth: Double = Double(SeLayout.inspectorMinimumWidth)
    static let minimumCanvasWidth: Double = 360

    static func usesSidebar(isRegular: Bool, width: Double) -> Bool {
        isRegular && width >= sidebarBreakpoint
    }

    /// Whether the trailing file/agent inspector is available alongside the
    /// workbench instead of presenting as a compact-only sheet.
    static func usesInlineInspector(isRegular: Bool, width: Double) -> Bool {
        usesSidebar(isRegular: isRegular, width: width)
    }

    /// Whether the leading terminal rail starts visible inside the workbench.
    /// Roomy regular widths (iPad portrait/landscape >= 744pt and iPhone Duo inner
    /// landscape 951pt) show the collapsible rail by default; narrower regular
    /// canvases like iPhone Duo inner portrait (669pt) start full-width and let
    /// the user toggle the rail on demand.
    static func defaultWorkbenchRailVisible(width: Double, isRegular: Bool) -> Bool {
        isRegular && width >= 744
    }

    /// Landscape starts expanded, portrait starts collapsed. This is a
    /// width-vs-height rule, not a device check.
    static func defaultRailCollapsed(width: Double, height: Double) -> Bool {
        width <= height
    }

    /// Wide landscape canvases open the trailing file/agent inspector by default
    /// while remaining collapsible; narrower regular canvases (iPad portrait,
    /// iPhone Duo inner screen, Split View) start collapsed to preserve canvas room.
    static func defaultInspectorOpen(width: Double, height: Double, isRegular: Bool) -> Bool {
        isRegular && width >= inspectorAutoOpenMinimumWidth && width > height
    }

    /// Orientation-change rule for the trailing inspector:
    /// preserve an explicit user collapse (`false`) across rotations so a closed
    /// file tree never pops back open unexpectedly, while clearing an explicit
    /// open (`true`) when rotating into a narrower posture that defaults closed.
    static func inspectorOverrideAfterOrientationChange(
        currentOverride: Bool?,
        newDefaultOpen: Bool
    ) -> Bool? {
        guard let currentOverride else { return nil }
        if !currentOverride { return false }
        return newDefaultOpen ? true : nil
    }

    /// Toggle/segment transition for the trailing inspector:
    /// - closed -> opens on `requested`
    /// - open on same segment -> collapses
    /// - open on other segment -> switches segment without collapsing
    static func nextInspectorState(
        isOpen: Bool,
        current: TerminalInspectorSegment,
        requested: TerminalInspectorSegment
    ) -> (isOpen: Bool, segment: TerminalInspectorSegment) {
        guard isOpen else {
            return (true, requested)
        }
        if current == requested {
            return (false, current)
        }
        return (true, requested)
    }

    /// Clamps the inline inspector width so the live terminal canvas always keeps
    /// at least `minimumCanvasWidth` when presented side-by-side.
    static func inspectorColumnWidth(availableWidth: Double, railWidth: Double) -> CGFloat {
        let budget = availableWidth - railWidth - minimumCanvasWidth
        guard budget >= Double(inspectorMinWidth) else {
            return inspectorMinWidth
        }
        return min(inspectorIdealWidth, max(inspectorMinWidth, CGFloat(budget)))
    }

    /// Apple HIG for iPhone Duo: prefer even column counts in card grids so the
    /// fold crease sits cleanly between columns instead of splitting a card.
    static func evenGridColumnCount(width: Double, isRegular: Bool) -> Int {
        guard isRegular, width >= 1080 else { return 2 }
        return 4
    }

    /// Size-class-driven desktop interaction default (never branch on
    /// `UIDevice.userInterfaceIdiom`, since iPhone Duo inner screen is regular
    /// while keeping `.phone` idiom).
    static func defaultDesktopInteractionMode(isRegular: Bool) -> DesktopInteractionMode {
        isRegular ? .direct : .trackpad
    }

    /// The brand block's subtitle. The prototype hard-codes `iPad · Nocturne`
    /// in the phone drawer too (U-5); here the form factor comes from the
    /// layout size class, so a phone never claims to be an iPad.
    static func railSubtitle(isPad: Bool) -> String {
        isPad
            ? String(localized: "iPad · Nocturne")
            : String(localized: "iPhone · Nocturne")
    }

    /// The collapsed iPad rail drops the host name for a single initial
    /// (prototype `index.html:728`, `.rhost .ini`).
    static func hostInitial(_ name: String) -> String {
        guard let first = name.trimmingCharacters(in: .whitespacesAndNewlines).first else {
            return "?"
        }
        return String(first).uppercased()
    }

    static func versionLabel(info: [String: Any]) -> String {
        let version = info["CFBundleShortVersionString"] as? String
        let build = info["CFBundleVersion"] as? String
        return ["SeRemote", version, build.map { "(\($0))" }].compactMap { $0 }.joined(separator: " ")
    }

    static var appVersion: String {
        versionLabel(info: Bundle.main.infoDictionary ?? [:])
    }
}
