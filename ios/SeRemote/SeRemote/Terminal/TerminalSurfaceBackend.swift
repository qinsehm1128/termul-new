import Foundation

/// The single terminal surface talks to one of two backends.
///
/// - Workspace: `TerminalStore` + `WorkspaceSession` (project/conversation PTYs).
/// - Standalone: `TerminalPTYViewer` (quick terminals, lease-isolated).
///
/// Implementations must never kill or respawn a PTY. `park()` only releases
/// display geometry; `resume()` re-asserts it. Overlay chrome never calls
/// `resize`, so opening the panel cannot reflow the PTY.
@MainActor
protocol TerminalSurfaceBackend: AnyObject {
    var items: [TerminalSurfaceItem] { get }
    var activeID: String? { get }
    var activeTitle: String { get }
    var activeCwd: String? { get }
    var activeProjectID: String? { get }
    var agentReady: Bool { get }
    func select(_ id: String) async
    func write(_ text: String) async
    func resize(cols: Int, rows: Int)
    func park() async
    func resume() async
    /// False only for a backend that holds no live attachment (a failed open).
    /// Close semantics derive from it: no attachment, no destructive close.
    var isLiveAttached: Bool { get }
}

extension TerminalSurfaceBackend {
    /// A workspace surface always has a live PTY behind it.
    var isLiveAttached: Bool { true }
}
