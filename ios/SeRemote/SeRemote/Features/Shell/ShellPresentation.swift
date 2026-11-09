import Foundation

/// Shell policy has no device-name checks and never owns a connection lifecycle.
enum ShellPresentation {
    static func usesSidebar(isRegular: Bool, width: Double) -> Bool {
        isRegular && width >= 600
    }

    static func versionLabel(info: [String: Any]) -> String {
        let version = info["CFBundleShortVersionString"] as? String
        let build = info["CFBundleVersion"] as? String
        return ["SeRemote", version, build.map { "(\($0))" }].compactMap { $0 }.joined(separator: " ")
    }
}
