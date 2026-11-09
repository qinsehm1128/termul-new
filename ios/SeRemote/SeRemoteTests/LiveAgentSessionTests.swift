import Foundation
import Testing
@testable import SeRemote

@Suite("Live agent sessions")
struct LiveAgentSessionTests {
    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    @Test("Host live-session payload decodes camelCase and absent project attribution")
    func decodesHostPayload() throws {
        let payload: HostLiveAgentSessionList = try decode(HostLiveAgentSessionList.self, """
        {"scannedAt":"2026-11-09T05:21:27Z","sessions":[
          {"terminalId":"terminal-1","projectId":"p1","rootPid":100,
           "agentId":"claude-code","sessionId":"session-a","cwd":"/work/termul"},
          {"terminalId":"terminal-2","rootPid":200,
           "agentId":"pi","sessionId":"session-b"}
        ]}
        """)
        #expect(payload.sessions.count == 2)
        #expect(payload.sessions[0].projectId == "p1")
        #expect(payload.sessions[0].destinationProjectId == "p1")
        #expect(payload.sessions[1].projectId == nil)
        #expect(payload.sessions[1].destinationProjectId == nil)
        #expect(payload.sessions[1].cwd == nil)
    }

    @Test("Only host-detected agent processes become running Live rows")
    func detectedAgentsBecomeRunningRows() throws {
        let sessions: [HostLiveAgentSession] = try decode([HostLiveAgentSession].self, """
        [{"terminalId":"terminal-1","projectId":"p1","rootPid":100,
          "agentId":"claude-code","sessionId":"session-a","cwd":"/work/termul"}]
        """)
        let rows = LivePresentation.sessions(
            conversations: [], projects: [], host: "Mac",
            activeConversationID: nil, activeProjectID: nil,
            agent: nil, isRunning: false, needsAttention: false,
            agentSessions: sessions)
        #expect(rows.count == 1)
        #expect(rows[0].status == .running)
        #expect(rows[0].agent == "Claude Code")
        #expect(rows[0].title == "termul")
        #expect(rows[0].id == "agent:p1:terminal-1:session-a")
        #expect(LivePresentation.rowMeta(for: rows[0]) == "Claude Code · 运行中 · Mac")
    }

    @Test("Agent rows open their project; an unattributed agent has no destination")
    func agentDestinations() {
        #expect(LivePresentation.destination(id: "agent:p1:terminal-1:session-a") == .project("p1"))
        #expect(LivePresentation.destination(id: "agent::terminal-1:session-a") == nil)
        #expect(LivePresentation.destination(id: "unknown") == nil)
    }

    @Test("Folderless agent sessions fall back to the agent label, never a fake title")
    func titleFallsBackToAgent() throws {
        let sessions: [HostLiveAgentSession] = try decode([HostLiveAgentSession].self, """
        [{"terminalId":"terminal-2","rootPid":200,"agentId":"qin-code","sessionId":"s2"}]
        """)
        #expect(LivePresentation.agentTitle(for: sessions[0]) == "qin-code")
        #expect(LivePresentation.agentLabel("codex") == "Codex")
        #expect(LivePresentation.agentLabel("gemini-cli") == "Gemini CLI")
    }

    @Test("Workspace project rows add real agent counts to the real terminal count")
    func workspaceProjectRow() throws {
        let projects: [HostProject] = try decode([HostProject].self, """
        [{"id":"p1","name":"Termul","liveTerminalCount":2}]
        """)
        let without = NocturneWorkspacePresentation.project(
            projects[0], snapshot: NocturneWorkspaceSnapshot())
        #expect(without.detail == "2 live terminals")

        let with = NocturneWorkspacePresentation.project(
            projects[0],
            snapshot: NocturneWorkspaceSnapshot(liveAgentSessionCounts: ["p1": 1]))
        #expect(with.detail == "2 live terminals · 1 agent sessions")
    }

    @Test("Idle projects never appear as running just because agent history exists")
    func idleProjectStaysStopped() throws {
        let projects: [HostProject] = try decode([HostProject].self, """
        [{"id":"p1","name":"Idle","liveTerminalCount":0}]
        """)
        let rows = LivePresentation.sessions(
            conversations: [], projects: projects, host: "Mac",
            activeConversationID: nil, activeProjectID: nil,
            agent: nil, isRunning: false, needsAttention: false,
            agentSessions: [])
        #expect(rows.isEmpty)
    }
}
