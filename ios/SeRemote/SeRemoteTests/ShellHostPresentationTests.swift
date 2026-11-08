import Foundation
import Testing
@testable import SeRemote

struct ShellHostPresentationTests {
    @Test func readyIsNotRunningOrAwaitingApproval() {
        let state = ShellHostLifecycle("ready")
        #expect(state.kind == .ready)
        #expect(state.label == String(localized: "Ready"))
    }

    @Test func deletedAndUnknownRemainHonest() {
        #expect(ShellHostLifecycle("deleted").kind == .deleted)
        #expect(ShellHostLifecycle("deleted").label == String(localized: "Deleted"))
        for raw in ["running", "awaiting_approval", "future-state", "READY"] {
            #expect(ShellHostLifecycle(raw).kind == .unknown)
            #expect(ShellHostLifecycle(raw).label == String(localized: "Unknown (\(raw))"))
        }
        #expect(ShellHostLifecycle("").label == String(localized: "Unknown"))
    }

    @Test func groupsPrioritizeRunningAndDoNotDuplicateProjects() throws {
        let payload = try decode("""
        {"projects":[
          {"id":"a","name":"Alpha","liveTerminalCount":2},
          {"id":"b","name":"Beta"},
          {"id":"c","name":"Other","path":"/work/other"},
          {"id":"d","name":"Archived","isArchived":true,"liveTerminalCount":8}
        ],"groups":[
          {"id":"g1","name":"Team","projectIds":["a","b","b","d","missing"]},
          {"id":"g2","name":"Duplicate","projectIds":["b"]}
        ]}
        """)
        let groups = ShellHostProjects.sections(projects: payload.projects, groups: payload.groups, query: "")
        #expect(groups.map(\.id) == ["running", "group:g1", "ungrouped"])
        #expect(groups.flatMap(\.projects).map(\.id) == ["a", "b", "c"])
        #expect(ShellHostProjects.terminalCount(in: payload.projects) == 2)
    }

    @Test func searchIncludesPathAndTrimsWhitespace() throws {
        let payload = try decode("""
        {"projects":[
          {"id":"a","name":"Alpha","path":"/Work/Client"},
          {"id":"b","name":"Beta"}
        ],"groups":[]}
        """)
        let groups = ShellHostProjects.sections(projects: payload.projects, groups: [], query: "  CLIENT\n")
        #expect(groups.flatMap(\.projects).map(\.id) == ["a"])
        #expect(ShellHostProjects.sections(projects: payload.projects, groups: [], query: "no match").isEmpty)
    }

    @Test func emptyAndMalformedCountsDoNotInventTerminals() throws {
        #expect(ShellHostProjects.sections(projects: [], groups: [], query: "").isEmpty)
        #expect(ShellHostProjects.terminalCount(in: []) == 0)
        let payload = try decode("""
        {"projects":[
          {"id":"a","name":"Missing count"},
          {"id":"b","name":"Bad count","liveTerminalCount":-5},
          {"id":"c","name":"Running","liveTerminalCount":3}
        ],"groups":[]}
        """)
        #expect(ShellHostProjects.terminalCount(in: payload.projects) == 3)
    }

    private func decode(_ json: String) throws -> ProjectListPayload {
        try JSONDecoder().decode(ProjectListPayload.self, from: Data(json.utf8))
    }
}
