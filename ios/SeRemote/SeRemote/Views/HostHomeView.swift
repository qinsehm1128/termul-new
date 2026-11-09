import SwiftUI

enum HostHomeSection: String, CaseIterable, Identifiable {
    case sessions
    case projects

    var id: String { rawValue }

    var title: String {
        switch self {
        case .sessions: String(localized: "Sessions")
        case .projects: String(localized: "Projects")
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
                    SeNavBar(title: String(localized: "Dashboard"), subtitle: "\(link.title) · \(link.originHost)", onBack: onClose)
                }

                hostHead
                    .padding(.horizontal, 18)
                    .padding(.top, directoryOnly ? 6 : 12)

                SeSegmented(
                    segments: [
                        SeSegment(HostHomeSection.sessions.rawValue, String(localized: "Sessions"), count: conversations.count),
                        SeSegment(HostHomeSection.projects.rawValue, String(localized: "Projects"), count: projects.count)
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
                    SeGroupLabel(title: String(localized: "Sessions"), count: String(format: "%02d", filteredConversations.count))
                    if filteredConversations.isEmpty {
                        empty(String(localized: "This host has no sessions yet."))
                    } else {
                        VStack(spacing: 8) {
                            ForEach(filteredConversations) { conversation in
                                sessionRow(conversation)
                            }
                        }
                        .padding(.horizontal, 18)
                    }
                } else {
                    SeGroupLabel(title: String(localized: "Projects"), count: String(format: "%02d", projects.count))
                    if projects.isEmpty {
                        empty(String(localized: "This host has no projects yet."))
                    } else {
                        VStack(spacing: 8) {
                            ForEach(projects) { project in
                                projectRow(project)
                            }
                        }
                        .padding(.horizontal, 18)
                    }
                }

                SeGroupLabel(title: String(localized: "Tools"), count: "02")
                HStack(spacing: 8) {
                    SeToolTile(
                        icon: .terminal,
                        label: String(localized: "Terminal"),
                        title: String(localized: "Open terminal"),
                        meta: String(localized: "zsh · \(runningTerminals) connections")
                    ) {
                        Task { await session.spawnTerminal() }
                    }
                    SeToolTile(
                        icon: .folder,
                        label: String(localized: "Files"),
                        title: String(localized: "Browse files"),
                        meta: session.files.isLoading ? String(localized: "Reading") : String(localized: "Read-only")
                    ) {
                        session.setWorkspaceTab(.files)
                    }
                }
                .padding(.horizontal, 18)

                Color.clear
                    .frame(height: 28)
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
            if directoryOnly {
                SeEyebrow(text: String(localized: "Host"))
            }
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
        case .connected: state = String(localized: "Online")
        case .connecting: state = String(localized: "Connecting")
        case .idle: state = String(localized: "Not connected")
        case .revoked: state = String(localized: "This device has been removed by the host")
        case .failed: state = String(localized: "Offline")
        }
        return String(localized: "\(state) · \(conversations.count) sessions · \(runningTerminals) terminals")
    }

    private var chips: [SeChipItem] {
        if section == .sessions {
            return [
                SeChipItem(id: HostDirectoryPresentation.all, title: String(localized: "All"), count: conversations.count),
                SeChipItem(id: HostDirectoryPresentation.ready, title: String(localized: "Ready"), dot: .live,
                           count: HostDirectoryPresentation.readyCount(in: session.conversations.conversations)),
                SeChipItem(id: HostDirectoryPresentation.ended, title: String(localized: "Ended"), dot: .off,
                           count: HostDirectoryPresentation.endedCount(in: session.conversations.conversations))
            ]
        }
        return [
            SeChipItem(id: "all", title: String(localized: "All"), count: projects.count),
            SeChipItem(id: "ready", title: String(localized: "Running"), dot: .live, count: projects.filter(\.isRunning).count)
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
            meta: [project.path ?? String(localized: "Project"), project.isRunning ? String(localized: "Running") : String(localized: "Idle")].joined(separator: " · "),
            time: project.isRunning ? String(localized: "\(project.liveTerminalCount) terminals") : nil,
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
