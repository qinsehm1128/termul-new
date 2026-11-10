import Testing
@testable import SeRemote

/// Resolve a catalog key the same way the app does, so label assertions stay
/// locale-independent.
private func L(_ key: String) -> String {
    String(localized: String.LocalizationValue(stringLiteral: key))
}

@Suite("IA shell navigation")
struct IAShellNavigationTests {
    @Test func primarySectionsAreTerminalsConversationsTools() {
        // Terminal first: connecting lands on the terminal surface.
        #expect(SidebarDestination.primary.map(\.rawValue) == ["terminals", "conversations", "tools"])
        #expect(SidebarDestination.primary.map(\.title) == [L("Terminals"), L("Conversations"), L("Tools")])
        #expect(SidebarDestination.primary.map(\.icon.rawValue) == ["terminal", "chat", "folder"])
        #expect(SidebarDestination.settings.title == L("Settings"))
        #expect(!SidebarDestination.primary.contains(.settings))
    }

    @Test func toolHubKeepsThePrototypeFourTiles() {
        let hub = ToolDestination.hubCases
        // 更改 / 定时任务 / 搜索 / 屏幕, in that order.
        #expect(hub.map(\.rawValue) == ["git", "scheduledTasks", "search", "screenText"])
        #expect(hub.count == 4)
        // The grid is the whole destination list now.
        #expect(hub == ToolDestination.allCases)
        #expect(hub.map(\.title).allSatisfy { !$0.isEmpty })
        // Quick terminals merged into 终端 and the desktop is reached from the
        // screen page and the terminal panel, so neither is a tool.
        #expect(!hub.map(\.rawValue).contains("quickTerminals"))
        #expect(!hub.map(\.rawValue).contains("desktop"))
    }

    @Test func toolHubUsesNocturneGlyphs() {
        #expect(ToolDestination.hubCases.map(\.icon) == [.git, .clock, .search, .textview])
    }

    @Test func screenPageSegmentsAreSnapshotThenDesktop() {
        #expect(ScreenSegment.allCases.map(\.rawValue) == ["snapshot", "desktop"])
        #expect(ScreenSegment.allCases.map(\.title) == [L("Snapshot"), L("Desktop")])
    }

    @Test func conversationListIsACPOnly() {
        let entries = LivePresentation.conversations(
            [makeConversation(id: "a", created: "2026-11-09T05:00:00.000Z")],
            activeConversationID: nil,
            agent: nil,
            isRunning: false,
            needsAttention: false
        )
        #expect(entries.count == 1)
        #expect(entries[0].id == "a")
        // No project or terminal row id can appear: the projection only accepts
        // HostConversation values.
        #expect(!entries[0].id.hasPrefix("project:"))
        #expect(!entries[0].id.hasPrefix("agent:"))
    }

    @Test func activeOrStreamingComesFirstThenRecent() {
        let conversations = [
            makeConversation(id: "recent", created: "2026-11-09T05:30:00.000Z"),
            makeConversation(id: "older", created: "2026-11-08T05:00:00.000Z"),
            makeConversation(id: "active", created: "2026-11-07T05:00:00.000Z"),
        ]
        let entries = LivePresentation.conversations(
            conversations,
            activeConversationID: "active",
            agent: "claude-code",
            isRunning: true,
            needsAttention: false
        )
        #expect(entries.map(\.id) == ["active", "recent", "older"])
        #expect(entries[0].status == .running)
        #expect(entries[0].isActiveOrStreaming)
        #expect(entries[0].agent == "Claude Code")
    }

    @Test func needsAttentionBeatsRecentOrdering() {
        let conversations = [
            makeConversation(id: "recent", created: "2026-11-09T05:30:00.000Z"),
            makeConversation(id: "waiting", created: "2026-11-01T05:00:00.000Z"),
        ]
        let entries = LivePresentation.conversations(
            conversations,
            activeConversationID: "waiting",
            agent: "qin-code",
            isRunning: false,
            needsAttention: true
        )
        #expect(entries.map(\.id) == ["waiting", "recent"])
        #expect(entries[0].status == .needsYou)
        #expect(entries[0].needsAttention)
        #expect(entries[0].statusText == L("Waiting for you"))
    }

    @Test func deletedConversationsAreHidden() {
        var deleted = makeConversation(id: "gone", created: "2026-11-09T05:00:00.000Z")
        deleted.lifecycleState = "deleted"
        let entries = LivePresentation.conversations(
            [deleted, makeConversation(id: "keep", created: nil)],
            activeConversationID: nil,
            agent: nil,
            isRunning: false,
            needsAttention: false
        )
        #expect(entries.map(\.id) == ["keep"])
    }

    @Test func undatedConversationsKeepHostOrderAfterDatedOnes() {
        let conversations = [
            makeConversation(id: "undated-1", created: nil),
            makeConversation(id: "dated", created: "2026-11-09T05:00:00.000Z"),
            makeConversation(id: "undated-2", created: nil),
        ]
        let entries = LivePresentation.conversations(
            conversations,
            activeConversationID: nil,
            agent: nil,
            isRunning: false,
            needsAttention: false
        )
        #expect(entries.map(\.id) == ["dated", "undated-1", "undated-2"])
    }

    private func makeConversation(id: String, created: String?) -> HostConversation {
        HostConversation(
            conversationId: id,
            title: "Session \(id)",
            workspaceCwd: "/work/\(id)",
            createdAtUtc: created,
            lifecycleState: "ready",
            lastSeq: 1,
            projectAttachment: ConversationProjectAttachment(projectId: "p-\(id)", projectPathSnapshot: nil),
            executionTarget: nil
        )
    }
}
