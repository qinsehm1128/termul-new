import SwiftUI

/// Connected entry point for the Workspace destination (tools grid and
/// projects). The shell (lane A) presents this; lane C owns its contents and
/// binds `WorkspaceHubView` to the real stores.
///
/// Contract for the shell:
/// - `onOpenSession` is called after a project was selected on `session`, so
///   the shell can present the session workbench.
/// - `onOpenCommand` opens the command palette.
struct WorkspaceHubScreen: View {
    @Bindable var session: WorkspaceSession
    var onOpenSession: () -> Void = {}
    var onOpenCommand: () -> Void = {}

    var body: some View {
        ToolsView(session: session)
    }
}
