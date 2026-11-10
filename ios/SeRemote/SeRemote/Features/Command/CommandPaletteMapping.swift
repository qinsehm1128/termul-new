import Foundation

/// Presentation-only routes: building or filtering these never starts a PTY.
enum CommandPaletteTarget: Hashable {
    case conversation(String)
    case project(String)
    case tool(ToolDestination)
}

struct CommandPaletteEntry: Identifiable {
    let item: PaletteItem
    let target: CommandPaletteTarget
    var id: String { item.id }
}

/// The palette only offers what has a destination of its own: ACP
/// conversations, projects, and tools. Files belong to the terminal's file
/// panel, quick terminals to the terminal landing, and the desktop to the
/// terminal panel and the screen page.
enum CommandPaletteMapping {
    static func entries(
        conversations: [HostConversation], projects: [HostProject], query: String
    ) -> [CommandPaletteEntry] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        func matches(_ values: String...) -> Bool {
            query.isEmpty || values.contains { $0.localizedCaseInsensitiveContains(query) }
        }
        var entries: [CommandPaletteEntry] = []
        for conversation in conversations where !conversation.isDeleted {
            guard matches(conversation.displayTitle, conversation.workspaceCwd, conversation.conversationId) else { continue }
            entries.append(CommandPaletteEntry(item: .init(
                id: "conversation:\(conversation.id)", kind: .session, title: conversation.displayTitle,
                detail: conversation.workspaceCwd, shortcut: conversation.statusLabel),
                target: .conversation(conversation.id)))
        }
        for project in projects where !project.isArchived {
            guard matches(project.name, project.path ?? "", project.id) else { continue }
            entries.append(CommandPaletteEntry(item: .init(
                id: "project:\(project.id)", kind: .session, title: project.name,
                detail: project.path ?? String(localized: "Project"), shortcut: String(localized: "Project")),
                target: .project(project.id)))
        }
        for tool in ToolDestination.allCases where matches(tool.title, tool.detail, tool.rawValue) {
            entries.append(CommandPaletteEntry(item: .init(
                id: "tool:\(tool.rawValue)", kind: .command, title: tool.title, detail: tool.detail, shortcut: ""),
                target: .tool(tool)))
        }
        return entries
    }
}
