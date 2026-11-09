import SwiftUI

/// 会话 — the ACP conversation list. Agent chat conversations only: terminals
/// and projects are never listed here. The 待你处理 strip keeps the existing
/// real-permission/failure behaviour and disappears when nothing needs review.
@MainActor
struct ConversationsScreen: View {
    @Bindable var session: WorkspaceSession
    let hostName: String
    let hostMeta: String
    let conversations: [ConversationEntry]
    let needs: [NeedsItem]
    let summary: LiveNeedsSummary
    let themeIcon: SeIcon
    let onSelectHost: () -> Void
    let onOpenConversation: (HostConversation) -> Void
    let onOpenWorkbench: () -> Void
    let onOpenNeeds: () -> Void
    let onOpenCommand: () -> Void
    let onToggleTheme: () -> Void
    let onOpenRail: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var renaming: HostConversation?
    @State private var renameTitle = ""
    @State private var deleting: HostConversation?
    @State private var showsNewConversation = false
    @State private var errorText: String?

    private var hasNeeds: Bool { summary.total > 0 }

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeHostSwitch(
                    title: hostName,
                    meta: hostMeta,
                    onHost: onSelectHost,
                    onTheme: onToggleTheme,
                    themeIcon: themeIcon,
                    onRail: onOpenRail,
                    onCommand: onOpenCommand
                )
                .padding(.top, 8)

                header
                    .padding(.horizontal, 18)
                    .padding(.top, 16)

                SeNeedStrip(permissions: summary.permissions, failures: summary.failures, onTap: onOpenNeeds)

                SeGroupLabel(title: "会话", count: String(format: "%02d", conversations.count))
                if conversations.isEmpty {
                    emptyState
                } else {
                    VStack(spacing: 8) {
                        ForEach(conversations) { entry in
                            row(entry)
                        }
                    }
                    .padding(.horizontal, 18)
                }

                if let errorText {
                    SeErrorBanner(message: errorText)
                        .padding(.horizontal, 18)
                        .padding(.top, 10)
                }

                Text("会话来自主机的 ACP 代理。终端和项目在「终端」与「工具」里。")
                    .font(SeNocturneType.font(10))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 18)
                    .padding(.top, 14)
                    .padding(.bottom, 28)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .alert("重命名会话", isPresented: Binding(
            get: { renaming != nil },
            set: { if !$0 { renaming = nil } }
        ), presenting: renaming) { conversation in
            TextField("标题", text: $renameTitle)
            Button("取消", role: .cancel) { renaming = nil }
            Button("保存") {
                let title = renameTitle
                renaming = nil
                Task { await rename(conversation, title: title) }
            }
            .disabled(renameTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog("删除这个会话？", isPresented: Binding(
            get: { deleting != nil },
            set: { if !$0 { deleting = nil } }
        ), titleVisibility: .visible, presenting: deleting) { conversation in
            Button("删除", role: .destructive) {
                deleting = nil
                Task { await delete(conversation) }
            }
            Button("取消", role: .cancel) { deleting = nil }
        } message: { _ in
            Text("主机保留仍被终端或代理引用的会话。")
        }
        .sheet(isPresented: $showsNewConversation) {
            NewConversationSheet(session: session) {
                showsNewConversation = false
                onOpenWorkbench()
            } onCancel: {
                showsNewConversation = false
            }
            .presentationDetents([.medium, .large])
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            SeEyebrow(text: "AGENT CHAT")
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                SeDisplayText(text: "会话")
                Spacer(minLength: 0)
                Button {
                    showsNewConversation = true
                } label: {
                    HStack(spacing: 5) {
                        SeIconView(icon: .plus, size: 11, stroke: 1.8, color: palette.accent)
                        Text("新会话")
                            .font(SeNocturneType.font(10.5, mono: true))
                            .foregroundStyle(palette.accent)
                    }
                    .padding(.horizontal, 10)
                    .frame(minHeight: SeSpace.hitTarget)
                    .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .strokeBorder(mode.insetStroke, lineWidth: 1)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel("新会话")
            }
            Text(lede)
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
        }
    }

    private var lede: String {
        let base = conversations.isEmpty ? "还没有会话" : "\(conversations.count) 个会话"
        return hasNeeds ? "\(base) · \(summary.total) 个需要你" : base
    }

    private func row(_ entry: ConversationEntry) -> some View {
        SeSessionRow(
            title: entry.title,
            meta: rowMeta(entry),
            time: entry.time,
            status: entry.needsAttention ? .wait : (entry.status == .running ? .live : .idle),
            alert: entry.needsAttention,
            trailing: entry.statusText,
            onTap: { onOpenConversation(entry.conversation) }
        )
        .contextMenu {
            Button("重命名", systemImage: "pencil") { beginRename(entry.conversation) }
            Button("删除", systemImage: "trash", role: .destructive) { deleting = entry.conversation }
        }
        .accessibilityHint(entry.agent.isEmpty ? entry.project : "\(entry.agent)，\(entry.project)")
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: entry.needsAttention)
    }

    private func rowMeta(_ entry: ConversationEntry) -> String {
        var parts: [String] = []
        if !entry.agent.isEmpty { parts.append(entry.agent) }
        parts.append(entry.project)
        parts.append(entry.statusText)
        return parts.joined(separator: " · ")
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("这台主机还没有代理会话。")
                .font(SeNocturneType.font(12, weight: .semibold))
                .foregroundStyle(palette.ink)
            Text("点「新会话」选一个项目开始，或到「终端」里启动代理。")
                .font(SeNocturneType.font(10.5))
                .foregroundStyle(palette.inkSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(mode.hairline, lineWidth: 1)
        }
        .padding(.horizontal, 18)
    }

    private func beginRename(_ conversation: HostConversation) {
        renameTitle = conversation.title ?? conversation.displayTitle
        renaming = conversation
    }

    private func rename(_ conversation: HostConversation, title: String) async {
        await session.conversations.rename(conversation, title: title)
        errorText = session.conversations.errorMessage
    }

    private func delete(_ conversation: HostConversation) async {
        await session.conversations.delete(conversation)
        errorText = session.conversations.errorMessage
    }
}

/// New conversation = a real host ACP session in a real project directory.
private struct NewConversationSheet: View {
    @Bindable var session: WorkspaceSession
    let onStarted: () -> Void
    let onCancel: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @State private var starting = false

    private var projects: [HostProject] {
        session.projects.projects.filter { !$0.isArchived }
    }

    var body: some View {
        NavigationStack {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("选一个项目目录开始新的代理会话。")
                        .font(SeNocturneType.font(11))
                        .foregroundStyle(palette.inkSecondary)
                        .padding(.top, 6)

                    if projects.isEmpty {
                        Text("主机还没有项目。先在「工具」里添加或打开一个项目。")
                            .font(SeNocturneType.font(11))
                            .foregroundStyle(palette.inkSecondary)
                            .padding(.vertical, 18)
                    } else {
                        ForEach(projects) { project in
                            Button {
                                start(in: project)
                            } label: {
                                SeSessionRow(
                                    title: project.name,
                                    meta: project.path ?? "项目",
                                    status: project.isRunning ? .live : .idle
                                ) {
                                    start(in: project)
                                }
                                .opacity(starting ? 0.6 : 1)
                            }
                            .buttonStyle(.plain)
                            .disabled(starting)
                        }
                    }
                }
                .padding(.horizontal, 18)
                .padding(.bottom, 24)
            }
            .background(palette.canvas)
            .navigationTitle("新会话")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { onCancel() }
                        .frame(minHeight: SeSpace.hitTarget)
                }
            }
            .overlay {
                if starting { ProgressView("正在开启代理会话…").padding(18) }
            }
        }
    }

    private func start(in project: HostProject) {
        guard !starting else { return }
        starting = true
        Task {
            defer { starting = false }
            let cwd = project.path ?? project.name
            await session.chat.startNewChat(cwd: cwd, conversationId: nil, projectId: project.id)
            await session.conversations.refresh()
            onStarted()
        }
    }
}
