import Foundation

/// Presentation-only routes: building or filtering these never starts a PTY.
enum CommandPaletteTarget: Hashable {
    case conversation(String)
    case project(String)
    case file(String)
    case quickTerminal(String)
    case tool(ToolDestination)
}

struct CommandPaletteEntry: Identifiable {
    let item: PaletteItem
    let target: CommandPaletteTarget
    var id: String { item.id }
}

enum CommandPaletteMapping {
    static func entries(
        conversations: [HostConversation], projects: [HostProject],
        quickTerminals: [QuickTerminalRecordWire], files: [FileSearchResult], query: String
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
        for record in quickTerminals {
            let title = record.title.flatMap { $0.isEmpty ? nil : $0 } ?? record.cwd
            guard matches(title, record.cwd, record.id) else { continue }
            entries.append(CommandPaletteEntry(item: .init(
                id: "quick-terminal:\(record.id)", kind: .command, title: title, detail: record.cwd,
                shortcut: String(localized: "Details")),
                target: .quickTerminal(record.id)))
        }
        // SearchStore already matched file CONTENT. Do not require the query in
        // the path, nor show duplicate rows if a host repeats a result path.
        var seenPaths = Set<String>()
        if !query.isEmpty {
            for file in files where seenPaths.insert(file.filePath).inserted {
                entries.append(CommandPaletteEntry(item: .init(
                    id: "file:\(file.filePath)", kind: .file, title: URL(fileURLWithPath: file.filePath).lastPathComponent,
                    detail: file.filePath, shortcut: file.matches.first.map { "L\($0.lineNumber)" } ?? ""),
                    target: .file(file.filePath)))
            }
        }
        for tool in ToolDestination.allCases where matches(tool.title, tool.detail, tool.rawValue) {
            entries.append(CommandPaletteEntry(item: .init(
                id: "tool:\(tool.rawValue)", kind: .command, title: tool.title, detail: tool.detail, shortcut: ""), target: .tool(tool)))
        }
        return entries
    }
}
