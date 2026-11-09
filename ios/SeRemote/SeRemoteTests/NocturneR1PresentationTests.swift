import Foundation
import Testing

@testable import SeRemote

// MARK: - Shell layout

@Suite("Nocturne R1 shell")
struct NocturneR1ShellPresentationTests {
    @Test("Rail widths are the prototype's 250 / 76 pt")
    func railWidths() {
        #expect(ShellPresentation.railExpandedWidth == 250)
        #expect(ShellPresentation.railCollapsedWidth == 76)
        #expect(ShellPresentation.sidebarBreakpoint == 600)
    }

    @Test("Sidebar appears only in a regular width class at or above 600 pt")
    func sidebarPolicy() {
        #expect(!ShellPresentation.usesSidebar(isRegular: false, width: 1024))
        #expect(!ShellPresentation.usesSidebar(isRegular: true, width: 599))
        #expect(ShellPresentation.usesSidebar(isRegular: true, width: 600))
        #expect(ShellPresentation.usesSidebar(isRegular: true, width: 1194))
    }

    @Test("Landscape starts expanded; portrait starts collapsed")
    func defaultRailCollapse() {
        #expect(!ShellPresentation.defaultRailCollapsed(width: 1194, height: 834))
        #expect(ShellPresentation.defaultRailCollapsed(width: 834, height: 1194))
        // A square window uses the rail: no orientation signal exists.
        #expect(ShellPresentation.defaultRailCollapsed(width: 900, height: 900))
    }

    @Test("Version label drops parts the bundle omits")
    func versionLabel() {
        #expect(ShellPresentation.versionLabel(info: [
            "CFBundleShortVersionString": "27.4",
            "CFBundleVersion": "189"
        ]) == "SeRemote 27.4 (189)")
        #expect(ShellPresentation.versionLabel(info: [:]) == "SeRemote")
        #expect(ShellPresentation.versionLabel(info: ["CFBundleShortVersionString": "27.4"]) == "SeRemote 27.4")
    }
}

// MARK: - Live projection

@Suite("Nocturne R1 live presentation")
struct NocturneR1LivePresentationTests {
    @Test("Deleted conversations never reach 现场")
    func deletedConversationsExcluded() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work/a","lifecycleState":"ready","title":"Alpha"},
         {"conversationId":"b","workspaceCwd":"/work/b","lifecycleState":"deleted","title":"Beta"}]
        """)
        let sessions = LivePresentation.sessions(
            conversations: conversations, projects: [], host: "MacBook Pro",
            activeConversationID: nil, activeProjectID: nil,
            agent: nil, isRunning: false, needsAttention: false)
        #expect(sessions.map(\.id) == ["conversation:a"])
        #expect(sessions[0].title == "Alpha")
        #expect(sessions[0].status == .ready)
    }

    @Test("The active running conversation becomes running and then needs you")
    func activeStatus() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/work/a","lifecycleState":"ready","title":"Alpha"}]
        """)
        let running = LivePresentation.sessions(
            conversations: conversations, projects: [], host: "H",
            activeConversationID: "a", activeProjectID: nil,
            agent: "Claude Code", isRunning: true, needsAttention: false)
        #expect(running[0].status == .running)
        #expect(running[0].agent == "Claude Code")
        #expect(LivePresentation.statusText(for: running[0]) == "运行中")

        let waiting = LivePresentation.sessions(
            conversations: conversations, projects: [], host: "H",
            activeConversationID: "a", activeProjectID: nil,
            agent: "Claude Code", isRunning: true, needsAttention: true)
        #expect(waiting[0].status == .needsYou)
        #expect(LivePresentation.statusText(for: waiting[0]) == "等待你处理")
    }

    @Test("Running projects appear as terminal sessions; idle projects do not")
    func runningProjects() throws {
        let projects: [HostProject] = try decode("""
        [{"id":"p1","name":"Live","liveTerminalCount":2},
         {"id":"p2","name":"Idle","liveTerminalCount":0}]
        """)
        let sessions = LivePresentation.sessions(
            conversations: [], projects: projects, host: "H",
            activeConversationID: nil, activeProjectID: "p1",
            agent: nil, isRunning: false, needsAttention: false)
        #expect(sessions.map(\.id) == ["project:p1"])
        #expect(sessions[0].status == .running)
    }

    @Test("Row meta keeps agent, state and host without inventing latency")
    func rowMeta() {
        let session = LiveSession(id: "conversation:a", title: "A", agent: "Claude Code",
                                  host: "MacBook Pro", status: .needsYou, timing: "")
        #expect(LivePresentation.rowMeta(for: session) == "Claude Code · 等待你处理 · MacBook Pro")
        let terminal = LiveSession(id: "project:p", title: "P", agent: "", host: "Mac mini",
                                   status: .running, timing: "")
        #expect(LivePresentation.rowMeta(for: terminal) == "Terminal · 运行中 · Mac mini")
        let ready = LiveSession(id: "conversation:b", title: "B", agent: "Codex", host: "H",
                                status: .ready, timing: "", detailStatus: "就绪")
        #expect(LivePresentation.statusText(for: ready) == "就绪")
    }

    @Test("Needs map only permissions, questions and failed tools")
    func needsMapping() {
        let permission = PermissionCard(id: "p", agentId: "a", title: "修改文件",
                                        options: [PermissionChoice(id: "allow", name: "允许")])
        let question = QuestionCard(id: "q", agentId: "a", question: "继续吗？", options: [])
        let failed = ToolCard(id: "t1", title: "测试失败", status: "failed")
        let running = ToolCard(id: "t2", title: "正在跑", status: "running")
        let items = LivePresentation.needs(
            permissions: [permission], questions: [question], tools: [failed, running], context: "会话 A")
        #expect(items.map(\.id) == ["permission:p", "question:q", "tool:t1"])
        #expect(items.map(\.kind) == [.permission, .question, .failure])
        #expect(items.allSatisfy { $0.meta == "会话 A" })
        let summary = LivePresentation.summary(items: items)
        #expect(summary.permissions == 1)
        #expect(summary.failures == 1)
        #expect(summary.questions == 1)
        #expect(summary.total == 3)
    }

    @Test("Session ids round-trip to the right destination")
    func destinations() {
        #expect(LivePresentation.destination(id: "conversation:abc") == .conversation("abc"))
        #expect(LivePresentation.destination(id: "project:p1") == .project("p1"))
        #expect(LivePresentation.destination(id: "unknown") == nil)
    }

    @Test("Busy means a send, a streaming message or a busy tool")
    func running() {
        #expect(LivePresentation.isRunning(isSending: true, messages: [], tools: []))
        let streaming = ChatMessage(id: "m", role: .agent, text: "", streaming: true, seq: nil)
        #expect(LivePresentation.isRunning(isSending: false, messages: [streaming], tools: []))
        let busy = ToolCard(id: "t", title: "x", status: "in_progress")
        #expect(LivePresentation.isRunning(isSending: false, messages: [], tools: [busy]))
        #expect(!LivePresentation.isRunning(isSending: false, messages: [], tools: []))
    }

    private func decode<T: Decodable>(_ value: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(value.utf8))
    }
}

// MARK: - Host directory

@Suite("Nocturne R1 host directory")
struct NocturneR1HostDirectoryTests {
    @Test("All hides deleted; ready and ended are honest lifecycle filters")
    func filters() throws {
        let conversations: [HostConversation] = try decode("""
        [{"conversationId":"a","workspaceCwd":"/a","lifecycleState":"ready","title":"Ready"},
         {"conversationId":"b","workspaceCwd":"/b","lifecycleState":"ended","title":"Ended"},
         {"conversationId":"c","workspaceCwd":"/c","lifecycleState":"deleted","title":"Deleted"}]
        """)
        #expect(HostDirectoryPresentation.conversations(conversations, filter: HostDirectoryPresentation.all)
            .map(\.id) == ["a", "b"])
        #expect(HostDirectoryPresentation.conversations(conversations, filter: HostDirectoryPresentation.ready)
            .map(\.id) == ["a"])
        #expect(HostDirectoryPresentation.conversations(conversations, filter: HostDirectoryPresentation.ended)
            .map(\.id) == ["c"])
        #expect(HostDirectoryPresentation.readyCount(in: conversations) == 1)
        #expect(HostDirectoryPresentation.endedCount(in: conversations) == 1)
    }

    private func decode<T: Decodable>(_ value: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(value.utf8))
    }
}

// MARK: - Static labels

@Suite("Nocturne R1 labels")
struct NocturneR1LabelTests {
    @Test("Needs badges use the prototype's four words and two actions")
    func needsLabels() {
        #expect(NeedsItemKind.permission.title == "许可")
        #expect(NeedsItemKind.question.title == "问题")
        #expect(NeedsItemKind.failure.title == "失败")
        #expect(NeedsItemKind.done.title == "完成")
        #expect(NeedsItemKind.running.title == "运行")
        #expect(NeedsItemKind.permission.actionTitle == "处理")
        #expect(NeedsItemKind.failure.actionTitle == "查看")
    }

    @Test("Palette groups and live filters use sentence-case Chinese")
    func paletteAndFilterLabels() {
        #expect(PaletteItemKind.session.title == "会话")
        #expect(PaletteItemKind.file.title == "文件")
        #expect(PaletteItemKind.command.title == "命令")
        #expect(LiveFilter.all.title == "全部")
        #expect(LiveFilter.running.title == "运行中")
        #expect(LiveFilter.needsYou.title == "需要你")
    }

    @Test("Rail destinations keep the two primary entries and settings")
    func railDestinations() {
        #expect(SidebarDestination.live.title == "现场")
        #expect(SidebarDestination.workspace.title == "工作区")
        #expect(SidebarDestination.settings.title == "设置")
    }
}
