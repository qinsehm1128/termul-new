import Foundation
import Testing
@testable import SeRemote

@Suite("Workspace Files name filtering")
@MainActor
struct WorkspaceFilesFilterTests {
    private func entry(_ name: String, path: String? = nil, directory: Bool = false) -> DirectoryEntry {
        DirectoryEntry(
            name: name,
            path: path ?? "/workspace/\(name)",
            type: directory ? "directory" : "file"
        )
    }

    @Test("Empty and whitespace-only queries preserve all entries and order")
    func emptyQuery() {
        let listing = [entry("Sources", directory: true), entry("z.swift"), entry("a.md")]
        for query in ["", " ", "\n\t "] {
            #expect(WorkspaceFilesFilter.entries(listing, matching: query) == listing)
        }
    }

    @Test("Matches a name substring without case or diacritic sensitivity")
    func nameMatching() {
        let listing = [entry("README.md"), entry("Café.swift"), entry("notes.txt")]
        #expect(WorkspaceFilesFilter.entries(listing, matching: "readme").map(\.name) == ["README.md"])
        #expect(WorkspaceFilesFilter.entries(listing, matching: "CAFE").map(\.name) == ["Café.swift"])
        #expect(WorkspaceFilesFilter.entries(listing, matching: ".SWIFT").map(\.name) == ["Café.swift"])
    }

    @Test("Trims query edges but preserves spaces inside file names")
    func whitespace() {
        let listing = [entry("release notes.md"), entry("releasenotes.md")]
        #expect(WorkspaceFilesFilter.entries(listing, matching: " \nrelease notes\t ").map(\.name) == ["release notes.md"])
    }

    @Test("Folders and files retain their supplied order and exact paths")
    func orderingAndIdentity() {
        let listing = [
            entry("swift-tools", directory: true),
            entry("b.swift", path: "/workspace/nested/b.swift"),
            entry("a.swift"),
            entry("README.md")
        ]
        let result = WorkspaceFilesFilter.entries(listing, matching: "swift")
        #expect(result == Array(listing.prefix(3)))
        #expect(result[1].id == "/workspace/nested/b.swift")
        #expect(listing.count == 4)
    }

    @Test("Does not match ancestor paths or search outside the current listing")
    func namesOnly() {
        let listing = [entry("notes.md", path: "/workspace/secret/notes.md")]
        #expect(WorkspaceFilesFilter.entries(listing, matching: "secret").isEmpty)
        #expect(WorkspaceFilesFilter.entries(listing, matching: "workspace").isEmpty)
        #expect(WorkspaceFilesFilter.entries(listing, matching: "missing.swift").isEmpty)
    }

    @Test("Query syntax is literal, not regex or a wildcard")
    func literalQuery() {
        let listing = [entry("[draft].md"), entry("report*.md"), entry("regular.md")]
        #expect(WorkspaceFilesFilter.entries(listing, matching: "[draft]").map(\.name) == ["[draft].md"])
        #expect(WorkspaceFilesFilter.entries(listing, matching: "*").map(\.name) == ["report*.md"])
    }

    @Test("Empty and no-match listings produce an empty result")
    func noMatches() {
        #expect(WorkspaceFilesFilter.entries([], matching: "file").isEmpty)
        #expect(WorkspaceFilesFilter.entries([entry("README.md")], matching: "swift").isEmpty)
    }

    @Test("Clearing a filter restores hidden selection candidates")
    func clearingRestoresListing() {
        let listing = [entry("selected.swift"), entry("notes.md")]
        let selectedPath = listing[0].path
        #expect(!WorkspaceFilesFilter.entries(listing, matching: ".md").contains { $0.path == selectedPath })
        #expect(WorkspaceFilesFilter.entries(listing, matching: "").contains { $0.path == selectedPath })
    }
}
