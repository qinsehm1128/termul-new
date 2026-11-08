import Testing
@testable import SeRemote

@Suite("Workspace chat presentation")
struct WorkspaceChatPresentationTests {
    private func user(_ id: String, delivery: MessageDelivery = .sending) -> ChatTimelineItem {
        .user(ChatMessage(id: id, role: .user, text: "Prompt", streaming: false, delivery: delivery))
    }

    @Test("Only a new local message after send enters")
    func localSendEntrance() {
        let old = user("old", delivery: .accepted)
        let sent = user("turn:local")
        #expect(WorkspaceChatPresentation.sentMessageIDs(
            previous: [old], current: [old, sent], sendRequested: true, isLoading: false
        ) == ["turn:local"])
    }

    @Test("History never gets a send entrance")
    func historyEntrance() {
        #expect(WorkspaceChatPresentation.sentMessageIDs(
            previous: [], current: [user("history", delivery: .accepted)],
            sendRequested: false, isLoading: false
        ).isEmpty)
        #expect(WorkspaceChatPresentation.sentMessageIDs(
            previous: [], current: [user("history")], sendRequested: true, isLoading: true
        ).isEmpty)
        #expect(WorkspaceChatPresentation.sentMessageIDs(
            previous: [], current: [user("history", delivery: .accepted)],
            sendRequested: true, isLoading: false
        ).isEmpty)
    }

    @Test("Re-rendering or streaming tokens cannot re-enter a user row")
    func tokenUpdateEntrance() {
        let sent = user("turn:local")
        let agent = ChatTimelineItem.agent(ChatMessage(
            id: "agent", role: .agent, text: "A token", streaming: true, delivery: nil
        ))
        #expect(WorkspaceChatPresentation.sentMessageIDs(
            previous: [sent], current: [sent, agent], sendRequested: true, isLoading: false
        ).isEmpty)
    }

    @Test("Full command title and unknown status are preserved")
    func toolSummary() {
        #expect(WorkspaceChatPresentation.toolCommand("swift test --filter Reconnect") == "swift test --filter Reconnect")
        #expect(WorkspaceChatPresentation.toolCommand("  \n") == "Tool call")
        #expect(WorkspaceChatPresentation.outcome("failed") == "Failed")
        #expect(WorkspaceChatPresentation.outcome("in_progress") == "Running")
        #expect(WorkspaceChatPresentation.outcome("host_cancelled") == "Host Cancelled")
    }

    @Test("Failed execution cannot disappear behind a folded activity")
    func failureAttention() {
        let failed = ToolCard(id: "failed", title: "swift test failed", status: "failed")
        let done = ToolCard(id: "done", title: "Read", status: "completed")
        #expect(failed.needsAttention)
        #expect(!done.needsAttention)
        #expect([done, failed].filter(\.needsAttention).map(\.id) == ["failed"])
    }
}
