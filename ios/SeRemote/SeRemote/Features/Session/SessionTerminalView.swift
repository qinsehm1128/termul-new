import SwiftUI

/// The Nocturne session surface uses the existing, identity-stable SwiftTerm owner.
/// The parent keeps this mounted while switching session modes.
struct SessionTerminalView: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        TerminalWorkspaceView(session: session)
    }
}

/// Display values are host snapshots; missing dimensions are never fabricated.
enum SessionTerminalPresentation {
    nonisolated static func dimensions(cols: Int?, rows: Int?) -> String? {
        guard let cols, let rows, cols > 0, rows > 0 else { return nil }
        return "\(cols) × \(rows)"
    }
}
