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
        let quick: QuickTerminalRecordWire = try decode("""
        {"schemaVersion":1,"id":"q","target":{"kind":"workspace"},"cwd":"/quick","createdAtUtc":"now","updatedAtUtc":"now","origin":"created"}
        """)
        let entries = CommandPaletteMapping.entries(conversations: [conversation], projects: [project], quickTerminals: [quick], files: [], query: "")
        #expect(Array(entries.prefix(3)).map(\.id) == ["conversation:c", "project:p", "quick-terminal:q"])
        #expect(Array(entries.prefix(3)).map(\.target) == [.conversation("c"), .project("p"), .quickTerminal("q")])
        #expect(entries[0].item.shortcut == conversation.statusLabel)
        #expect(entries[2].item.title == "/quick")
    }

    @Test func contentSearchHitKeepsPathAndLineEvenWhenPathDoesNotMatchQuery() {
        let files: [FileSearchResult] = [
            .init(filePath: "/work/source.swift", matches: [.init(lineNumber: 12, lineText: "needle")]),
            .init(filePath: "/work/source.swift", matches: []),
            .init(filePath: "/work/empty.swift", matches: [])
        ]
        let entries = CommandPaletteMapping.entries(conversations: [], projects: [], quickTerminals: [], files: files, query: "needle")
        #expect(entries.map(\.target) == [.file("/work/source.swift"), .file("/work/empty.swift")])
        #expect(entries[0].item.title == "source.swift")
        #expect(entries[0].item.shortcut == "L12")
        #expect(entries[1].item.shortcut.isEmpty)
        #expect(CommandPaletteMapping.entries(conversations: [], projects: [], quickTerminals: [], files: files, query: "").allSatisfy { $0.item.kind != .file })
    }

    @Test func allToolsHaveDistinctActualDestinations() {
        let entries = CommandPaletteMapping.entries(conversations: [], projects: [], quickTerminals: [], files: [], query: "")
        #expect(entries.count == ToolDestination.allCases.count)
        #expect(Set(entries.map(\.id)).count == entries.count)
        #expect(entries.map(\.target) == ToolDestination.allCases.map { .tool($0) })
    }

    @Test func archivedDeletedAndNonmatchingEntriesAreExcluded() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work","lifecycleState":"ready","title":"Keep"},
         {"conversationId":"b","workspaceCwd":"/work","lifecycleState":"deleted","title":"Keep"}]
        """)
        let projects: [HostProject] = try decode("""
        [{"id":"p","name":"Keep","isArchived":true},{"id":"q","name":"Other"}]
        """)
        let entries = CommandPaletteMapping.entries(conversations: conversations, projects: projects, quickTerminals: [], files: [], query: "  KEEP  ")
        #expect(entries.map(\.target) == [.conversation("a")])
    }

    private func decode<T: Decodable>(_ value: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(value.utf8))
    }
}
