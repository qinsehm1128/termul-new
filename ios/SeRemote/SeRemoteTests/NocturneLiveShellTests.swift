import Foundation
import Testing
@testable import SeRemote

struct NocturneLiveShellTests {
    @Test func liveIncludesConversationsAndOnlyRunningProjects() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"ready","workspaceCwd":"/work","lifecycleState":"ready"},
         {"conversationId":"future","workspaceCwd":"/work","lifecycleState":"future"},
         {"conversationId":"deleted","workspaceCwd":"/work","lifecycleState":"deleted"}]
        """)
        let projects: [HostProject] = try decode("""
        [{"id":"idle","name":"Idle"},{"id":"live","name":"Live","liveTerminalCount":2}]
        """)
        let rows = LivePresentation.sessions(conversations: conversations, projects: projects, host: "Host",
                                             activeConversationID: nil, activeProjectID: nil,
                                             agent: nil, isRunning: false, needsAttention: false)
        #expect(rows.map(\.id) == ["conversation:ready", "conversation:future", "project:live"])
        #expect(rows.map(\.status) == [.ready, .unknown, .running])
        #expect(rows.allSatisfy { $0.timing.isEmpty && $0.agent.isEmpty })
        #expect(rows[1].detailStatus == conversations[1].statusLabel)
    }

    @Test func attentionAndRunningApplyOnlyToKnownActiveSession() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work","lifecycleState":"ready"},
         {"conversationId":"b","workspaceCwd":"/work","lifecycleState":"ready"}]
        """)
        let rows = LivePresentation.sessions(conversations: conversations, projects: [], host: "Host",
                                             activeConversationID: "b", activeProjectID: nil,
                                             agent: "agent", isRunning: true, needsAttention: true)
        #expect(rows.map(\.status) == [.ready, .needsYou])
        #expect(rows.map(\.agent) == ["", "agent"])
        let running = LivePresentation.sessions(conversations: conversations, projects: [], host: "Host",
                                                activeConversationID: "b", activeProjectID: nil,
                                                agent: nil, isRunning: true, needsAttention: false)
        #expect(running.map(\.status) == [.ready, .running])
    }

    @Test func needsCountsOnlyPermissionQuestionAndFailedTools() {
        let items = LivePresentation.needs(
            permissions: [.init(id: "p", agentId: "a", title: "Allow", options: [])],
            questions: [.init(id: "q", agentId: "a", question: "Which?", options: [])],
            tools: [.init(id: "busy", title: "Busy", status: "in_progress"),
                    .init(id: "ok", title: "Done", status: "completed"),
                    .init(id: "bad", title: "Failed", status: "failed")], context: "Active")
        #expect(items.map(\.id) == ["permission:p", "question:q", "tool:bad"])
        let summary = LivePresentation.summary(items: items)
        #expect(summary.total == 3)
        #expect(summary.permissions == 1 && summary.questions == 1 && summary.failures == 1)
        #expect(LivePresentation.summary(items: []).total == 0)
    }

    @Test func runningReflectsSendingStreamingOrBusyToolsOnly() {
        #expect(!LivePresentation.isRunning(isSending: false, messages: [], tools: []))
        #expect(LivePresentation.isRunning(isSending: true, messages: [], tools: []))
        let message = ChatMessage(id: "m", role: .agent, text: "Working", streaming: true)
        #expect(LivePresentation.isRunning(isSending: false, messages: [message], tools: []))
        #expect(LivePresentation.isRunning(isSending: false, messages: [], tools: [.init(id: "t", title: "Tool", status: "in_progress")]))
        #expect(!LivePresentation.isRunning(isSending: false, messages: [], tools: [.init(id: "t", title: "Tool", status: "completed")]))
    }

    @Test func navigationUsesNamespacedStableIdentifiers() {
        #expect(LivePresentation.destination(id: "conversation:abc:def") == .conversation("abc:def"))
        #expect(LivePresentation.destination(id: "project:p") == .project("p"))
        #expect(LivePresentation.destination(id: "unknown") == nil)
    }

    @Test func sidebarRequiresRegularClassAndSufficientWidth() {
        #expect(ShellPresentation.usesSidebar(isRegular: true, width: 600))
        #expect(!ShellPresentation.usesSidebar(isRegular: true, width: 599))
        #expect(!ShellPresentation.usesSidebar(isRegular: false, width: 1200))
        #expect(ShellPresentation.versionLabel(info: [:]) == "SeRemote")
        #expect(ShellPresentation.versionLabel(info: ["CFBundleShortVersionString": "1.2", "CFBundleVersion": "3"]) == "SeRemote 1.2 (3)")
    }

    @Test func offlineRowsRequireMatchingNonemptyTranscript() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work","lifecycleState":"ready"},
         {"conversationId":"b","workspaceCwd":"/work","lifecycleState":"ready"}]
        """)
        let empty = CachedTranscript(schemaVersion: 2, conversationId: "a", sessionId: "s", cwd: "/work", watermark: 0, messages: [], tools: [])
        #expect(OfflinePresentation.sessions(conversations: conversations, transcripts: [:]).isEmpty)
        #expect(OfflinePresentation.sessions(conversations: conversations, transcripts: ["a": empty]).isEmpty)
        var cached = empty
        cached.messages = [.init(id: "m", role: .user, text: "Hello", streaming: false)]
        #expect(OfflinePresentation.sessions(conversations: conversations, transcripts: ["a": cached]).map(\.id) == ["a"])
        cached.conversationId = "other"
        #expect(OfflinePresentation.sessions(conversations: conversations, transcripts: ["a": cached]).isEmpty)
    }

    private func decode<T: Decodable>(_ value: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(value.utf8))
    }
}
