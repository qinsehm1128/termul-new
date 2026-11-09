import SwiftUI

enum HostHomeSection: String, CaseIterable, Identifiable {
    case sessions
    case projects

    var id: String { rawValue }

    var title: String {
        switch self {
        case .sessions: "会话"
        case .projects: "项目"
        }
    }
}

/// 工作台 directory: the host's sessions and projects, plus the two tools the
/// host actually exposes. It is embedded in the session screen on iPad and
/// never owns the connection.
@MainActor
struct HostHomeView: View {
    @Bindable var session: WorkspaceSession
    @Bindable var store: ConnectionStore
    let link: RemoteLink
    var directoryOnly = false
    var onClose: (() -> Void)? = nil

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    @State private var section: HostHomeSection = .sessions
    @State private var filter = "all"

    private var conversations: [HostConversation] {
        session.conversations.conversations.filter { !$0.isDeleted }
    }

    private var filteredConversations: [HostConversation] {
        HostDirectoryPresentation.conversations(session.conversations.conversations, filter: filter)
    }

    private var projects: [HostProject] {
        session.projects.projects.filter { !$0.isArchived }
    }

    private var runningTerminals: Int {
        ShellHostProjects.terminalCount(in: session.projects.projects)
    }

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                if !directoryOnly {
                    SeNavBar(title: "工作台", subtitle: "\(link.title) · \(link.originHost)", onBack: onClose)
                }

                hostHead
                    .padding(.horizontal, 18)
                    .padding(.top, directoryOnly ? 6 : 12)

                SeSegmented(
                    segments: [
                        SeSegment(HostHomeSection.sessions.rawValue, "会话", count: conversations.count),
                        SeSegment(HostHomeSection.projects.rawValue, "项目", count: projects.count)
                    ],
                    selection: Binding(get: { section.rawValue }, set: { section = HostHomeSection(rawValue: $0) ?? .sessions }),
                    wide: true,
                    identifier: "host.section"
                )
                .padding(.horizontal, 18)
                .padding(.top, 14)

                SeChips(
                    items: chips,
                    selection: Binding(get: { filter }, set: { filter = $0 ?? "all" })
                )
                .padding(.top, 10)

                if section == .sessions {
                    SeGroupLabel(title: "会话", count: String(format: "%02d", filteredConversations.count))
                    if filteredConversations.isEmpty {
                        empty("这台主机还没有会话。")
                    } else {
                        VStack(spacing: 8) {
                            ForEach(filteredConversations) { conversation in
                                sessionRow(conversation)
                            }
                        }
                        .padding(.horizontal, 18)
                    }
                } else {
                    SeGroupLabel(title: "项目", count: String(format: "%02d", projects.count))
                    if projects.isEmpty {
                        empty("这台主机还没有项目。")
                    } else {
                        VStack(spacing: 8) {
                            ForEach(projects) { project in
                                projectRow(project)
                            }
                        }
                        .padding(.horizontal, 18)
                    }
                }

                SeGroupLabel(title: "工具", count: "02")
                HStack(spacing: 8) {
                    SeToolTile(
                        icon: .terminal,
                        label: "终端",
                        title: "打开终端",
                        meta: "zsh · \(runningTerminals) 个连接"
                    ) {
                        Task { await session.spawnTerminal() }
                    }
                    SeToolTile(
                        icon: .folder,
                        label: "文件",
                        title: "浏览文件",
                        meta: session.files.isLoading ? "读取中" : "只读"
                    ) {
                        session.setWorkspaceTab(.files)
                    }
                }
                .padding(.horizontal, 18)

                Text("会话与项目由主机提供。这里的列表只读，操作会回到对应的工作台。")
                    .font(SeNocturneType.font(10))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.top, 14)
                    .padding(.bottom, 28)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .onAppear {
            if session.workspace == .project { section = .projects }
        }
    }

    private var hostHead: some View {
        VStack(alignment: .leading, spacing: 4) {
            SeEyebrow(text: directoryOnly ? "主机" : "目录")
            Text(link.title)
                .font(SeNocturneType.font(26, weight: .semibold))
                .tracking(-0.8)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            HStack(spacing: 5) {
                SeNeedDot(kind: session.phase == .connected ? .live : .off, size: 5)
                Text(hostMeta)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
        }
    }

    private var hostMeta: String {
        let state: String
        switch session.phase {
        case .connected: state = "在线"
        case .connecting: state = "连接中"
        case .idle: state = "未连接"
        case .failed: state = "离线"
        }
        return "\(state) · \(conversations.count) 个会话 · \(runningTerminals) 个终端"
    }

    private var chips: [SeChipItem] {
        if section == .sessions {
            return [
                SeChipItem(id: HostDirectoryPresentation.all, title: "全部", count: conversations.count),
                SeChipItem(id: HostDirectoryPresentation.ready, title: "就绪", dot: .live,
                           count: HostDirectoryPresentation.readyCount(in: session.conversations.conversations)),
                SeChipItem(id: HostDirectoryPresentation.ended, title: "已结束", dot: .off,
                           count: HostDirectoryPresentation.endedCount(in: session.conversations.conversations))
            ]
        }
        return [
            SeChipItem(id: "all", title: "全部", count: projects.count),
            SeChipItem(id: "ready", title: "运行中", dot: .live, count: projects.filter(\.isRunning).count)
        ]
    }

    private func sessionRow(_ conversation: HostConversation) -> some View {
        let active = session.conversations.active?.id == conversation.id && session.workspace == .conversation
        let running = active && LivePresentation.isRunning(isSending: session.chat.isSending,
                                                           messages: session.chat.messages,
                                                           tools: session.chat.tools)
        var meta: [String] = [conversation.folderName, conversation.statusLabel]
        if let turns = conversation.countLabel { meta.append(turns) }
        return SeSessionRow(
            title: conversation.displayTitle,
            meta: meta.joined(separator: " · "),
            time: conversation.relativeCreatedLabel,
            status: active && running ? .live : .idle,
            onTap: { open(conversation) }
        )
    }

    private func projectRow(_ project: HostProject) -> some View {
        SeSessionRow(
            title: project.name,
            meta: [project.path ?? "项目", project.isRunning ? "运行中" : "空闲"].joined(separator: " · "),
            time: project.isRunning ? "\(project.liveTerminalCount) 个终端" : nil,
            status: project.isRunning ? .live : .idle,
            onTap: { open(project) }
        )
    }

    private func empty(_ text: String) -> some View {
        Text(text)
            .font(SeNocturneType.font(11))
            .foregroundStyle(palette.inkSecondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(mode.hairline, lineWidth: 1)
            }
            .padding(.horizontal, 18)
    }

    private func open(_ conversation: HostConversation) {
        Task { await session.selectConversation(conversation) }
    }

    private func open(_ project: HostProject) {
        Task { await session.selectProject(project) }
    }
}
