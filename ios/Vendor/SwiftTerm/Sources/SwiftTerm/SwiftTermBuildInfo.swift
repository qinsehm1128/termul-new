/// Static stand-in for the upstream build-tool plugin output.
/// Used only for XTVERSION / diagnostics.
public enum SwiftTermBuildInfo {
    public static let branch: String? = nil
    public static let tag: String? = "1.20.0"
    public static let commit: String? = nil
    public static let hasUncommittedChanges: Bool? = false
    public static let version: String = "1.20.0"
}
