import Foundation
import Testing
@testable import SeRemote

struct NocturneCommandShellTests {
    @Test func entriesKeepRealTargetsAndLifecycle() throws {
        let conversation: HostConversation = try decode("""
        {"conversationId":"c","workspaceCwd":"/work","lifecycleState":"ready","title":"Chat"}
        """)
        let project: HostProject = try decode("""
        {"id":"p","name":"Project","path":"/project"}
        """)
        let entries = CommandPaletteMapping.entries(conversations: [conversation], projects: [project], query: "")
        // Conversations and projects come first; the four tools always follow.
        #expect(entries.count == 2 + ToolDestination.allCases.count)
        #expect(Array(entries.prefix(2)).map(\.id) == ["conversation:c", "project:p"])
        #expect(Array(entries.prefix(2)).map(\.target) == [.conversation("c"), .project("p")])
        #expect(entries.dropFirst(2).map(\.target) == ToolDestination.allCases.map { .tool($0) })
        #expect(entries[0].item.shortcut == conversation.statusLabel)
        #expect(entries[1].item.detail == "/project")
        #expect(entries[1].item.shortcut == String(localized: "Project"))
    }

    @Test func queryMatchesTheConversationPathAndToolDetail() throws {
        let conversation: HostConversation = try decode("""
        {"conversationId":"c","workspaceCwd":"/work/needle","lifecycleState":"ready","title":"Chat"}
        """)
        let byPath = CommandPaletteMapping.entries(
            conversations: [conversation], projects: [], query: "needle")
        #expect(byPath.map(\.target) == [.conversation("c")])
        // Tool details stay searchable: the palette still reaches them by what
        // they do, not only by their title. The detail is localized, so query
        // with the same value the mapping sees.
        let byDetail = CommandPaletteMapping.entries(
            conversations: [], projects: [], query: ToolDestination.git.detail)
        #expect(byDetail.map(\.target) == [.tool(.git)])
        // No palette row can be a file: file search belongs to the terminal's
        // file panel.
        #expect(byPath.allSatisfy { $0.item.kind != .file })
        #expect(byDetail.allSatisfy { $0.item.kind != .file })
    }

    @Test func allToolsHaveDistinctActualDestinations() {
        let entries = CommandPaletteMapping.entries(conversations: [], projects: [], query: "")
        #expect(entries.count == ToolDestination.allCases.count)
        #expect(Set(entries.map(\.id)).count == entries.count)
        #expect(entries.map(\.target) == ToolDestination.allCases.map { .tool($0) })
        #expect(entries.map(\.id) == ["tool:git", "tool:scheduledTasks", "tool:search", "tool:screenText"])
        #expect(entries.allSatisfy { $0.item.kind == .command })
        // The merged quick-terminal and desktop destinations are gone.
        #expect(!entries.map(\.id).contains("tool:quickTerminals"))
        #expect(!entries.map(\.id).contains("tool:desktop"))
    }

    @Test func archivedDeletedAndNonmatchingEntriesAreExcluded() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work","lifecycleState":"ready","title":"Keep"},
         {"conversationId":"b","workspaceCwd":"/work","lifecycleState":"deleted","title":"Keep"}]
        """)
        let projects: [HostProject] = try decode("""
        [{"id":"p","name":"Keep","isArchived":true},{"id":"q","name":"Other"}]
        """)
        let entries = CommandPaletteMapping.entries(conversations: conversations, projects: projects, query: "  KEEP  ")
        #expect(entries.map(\.target) == [.conversation("a")])
    }

    private func decode<T: Decodable>(_ value: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(value.utf8))
    }
}
