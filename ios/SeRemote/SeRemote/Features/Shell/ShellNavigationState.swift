import Foundation

/// Navigation value for the connected shell.
///
/// The workbench is a single mounted surface: switching destinations only moves
/// `destination`, and leaving the workbench only flips `workbench`. `openRequest`
/// is deliberately kept across `back()` so returning to the terminal resumes
/// the same surface instead of rebuilding it (I1), and no transition here can
/// end a host PTY (I5).
struct ShellNavigationState: Equatable {
    /// The bottom bar / rail destination. Connecting always starts on terminals.
    var destination: SidebarDestination = .terminals
    /// T1 is the terminal list; it shows whenever the workbench is not visible.
    var landing = true
    /// The workbench surface is the page the user last opened. Selecting another
    /// destination keeps this flag so the terminal subtree stays mounted.
    var workbench = false
    /// The terminal the surface shows. Survives `back()`.
    var openRequest: TerminalOpenRequest?
    /// The workbench's overlay panel selection. Purely presentational.
    var panel: TerminalPanelAction?

    /// The workbench is on screen only on its own destination; the terminal
    /// subtree itself stays mounted across every destination switch.
    var workbenchVisible: Bool {
        workbench && destination == .terminals
    }

    /// Landing after a successful connect: reopen the last terminal, or show T1
    /// when there is none. A terminal that no longer exists is resolved by the
    /// caller and lands on T1 with a notice.
    mutating func connected(last: TerminalOpenRequest?) {
        destination = .terminals
        if let last {
            open(last)
        } else {
            landing = true
            workbench = false
            panel = nil
        }
    }

    /// Show a terminal. The request is kept for the mounted surface; the canvas
    /// is never remounted by this call.
    mutating func open(_ request: TerminalOpenRequest) {
        openRequest = request
        workbench = true
        landing = false
        destination = .terminals
    }

    /// Leave the workbench for T1. Only the presentation changes: the request
    /// stays so the same terminal (and its PTY) resumes.
    mutating func back() {
        workbench = false
        landing = true
        panel = nil
        destination = .terminals
    }

    /// Switch destination. `workbench` is untouched; the views only change
    /// opacity, so the terminal subtree keeps its identity.
    mutating func select(_ destination: SidebarDestination) {
        self.destination = destination
    }

    /// Select a workbench panel without touching geometry or the surface.
    mutating func present(_ action: TerminalPanelAction?) {
        panel = action
    }
}
