import SwiftUI

/// Live-data adapter for the item-based palette. It routes to the three things
/// that own a destination: ACP conversations, projects, and tools.
///
/// The palette never guesses at a workspace surface and never opens a file
/// preview: files live in the terminal's file panel, quick terminals in the
/// terminal landing, and the desktop in the terminal panel and screen page.
/// Opening a conversation or a project is the shell's decision, so the palette
/// asks through closures the shell wires to T1's grouping or T2's workbench.
struct ConnectedCommandPalette: View {
    let session: WorkspaceSession
    /// Fallback for a shell that has not wired a specific destination yet: it
    /// only surfaces the current workspace, it never selects a tab.
    var onOpenSession: () -> Void
    /// Wired by the shell: open the conversation in T2 (or its chat cover).
    var onOpenConversation: ((String) -> Void)?
    /// Wired by the shell: open the project's terminal group in T1/T2.
    var onOpenProject: ((String) -> Void)?
    var onClose: () -> Void

    @State private var query = ""
    @State private var presentedTool: ToolDestination?
    @Environment(\.sePalette) private var palette

    init(session: WorkspaceSession,
         onOpenSession: @escaping () -> Void,
         onOpenConversation: ((String) -> Void)? = nil,
         onOpenProject: ((String) -> Void)? = nil,
         onClose: @escaping () -> Void = {}) {
        self.session = session
        self.onOpenSession = onOpenSession
        self.onOpenConversation = onOpenConversation
        self.onOpenProject = onOpenProject
        self.onClose = onClose
    }

    private var entries: [CommandPaletteEntry] {
        CommandPaletteMapping.entries(
            conversations: session.conversations.conversations,
            projects: session.projects.projects,
            query: query)
    }

    var body: some View {
        CommandPaletteView(items: entries.map(\.item), query: $query, onSelect: { item in
            guard let entry = entries.first(where: { $0.id == item.id }) else { return }
            select(entry.target)
        }, onClose: onClose)
        .sheet(item: $presentedTool) { tool in
            NavigationStack {
                WorkspaceToolDestinationView(session: session, destination: tool, onOpenFile: {
                    presentedTool = nil
                    onOpenSession()
                })
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { presentedTool = nil }
                            .frame(minHeight: SeSpace.hitTarget)
                    }
                }
            }
            .tint(palette.ink)
        }
    }

    private func select(_ target: CommandPaletteTarget) {
        switch target {
        case let .conversation(id):
            if let onOpenConversation {
                onOpenConversation(id)
            } else {
                onOpenSession()
            }
        case let .project(id):
            if let onOpenProject {
                onOpenProject(id)
            } else {
                onOpenSession()
            }
        case let .tool(tool):
            presentedTool = tool
        }
    }
}
