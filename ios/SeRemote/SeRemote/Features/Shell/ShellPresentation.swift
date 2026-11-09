import Foundation

/// Shell policy has no device-name checks and never owns a connection lifecycle.
enum ShellPresentation {
    /// Below this width a phone/compact layout is used; above it the iPad rail.
    static let sidebarBreakpoint: Double = 600

    /// The prototype's iPad directory.
    static let railExpandedWidth: CGFloat = 250
    /// The prototype's iPad portrait rail.
    static let railCollapsedWidth: CGFloat = 76

    static func usesSidebar(isRegular: Bool, width: Double) -> Bool {
        isRegular && width >= sidebarBreakpoint
    }

    /// Landscape starts expanded, portrait starts collapsed. This is a
    /// width-vs-height rule, not a device check.
    static func defaultRailCollapsed(width: Double, height: Double) -> Bool {
        width <= height
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
