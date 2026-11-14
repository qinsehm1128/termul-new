import SwiftUI

/// 会话 — the ACP conversation list. Agent chat conversations only: terminals
/// and projects are never listed here. The 待你处理 strip keeps the existing
/// real-permission/failure behaviour and disappears when nothing needs review.
@MainActor
struct ConversationsScreen: View {
    @Environment(\.remoteDeviceLink) private var deviceLink
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

                SeGroupLabel(title: String(localized: "Conversations"), count: String(format: "%02d", conversations.count))
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
                    // `self.errorText`: the shadowed binding is immutable.
                    NocturneNotice(message: errorText, isError: true, onDismiss: { self.errorText = nil })
                        .padding(.horizontal, 18)
                        .padding(.top, 10)
                }

                Color.clear
                    .frame(height: 28)
            }
        }
        .refreshable {
            await session.conversations.refresh()
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .alert("Rename session", isPresented: Binding(
            get: { renaming != nil },
            set: { if !$0 { renaming = nil } }
        ), presenting: renaming) { conversation in
            TextField("Title", text: $renameTitle)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Save") {
                let title = renameTitle
                renaming = nil
                Task { await rename(conversation, title: title) }
            }
            .disabled(renameTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog("Delete this session?", isPresented: Binding(
            get: { deleting != nil },
            set: { if !$0 { deleting = nil } }
        ), titleVisibility: .visible, presenting: deleting) { conversation in
            Button("Delete", role: .destructive) {
                deleting = nil
                Task { await delete(conversation) }
            }
            Button("Cancel", role: .cancel) { deleting = nil }
        } message: { _ in
            Text("The host keeps sessions still referenced by a terminal or agent.")
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
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                SeDisplayText(text: String(localized: "Conversations"))
                Spacer(minLength: 0)
                Button {
                    showsNewConversation = true
                } label: {
                    HStack(spacing: 5) {
                        SeIconView(icon: .plus, size: 11, stroke: 1.8, color: palette.accent)
                        Text("New session")
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
                .accessibilityLabel(String(localized: "New session"))
                .disabled(!(deviceLink?.allows("agent.run") ?? true))
            }
        }
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
            if (deviceLink?.allows("agent.run") ?? true) {
                Button("Rename", systemImage: "pencil") { beginRename(entry.conversation) }
                Button("Delete", systemImage: "trash", role: .destructive) { deleting = entry.conversation }
            }
        }
        .accessibilityHint(entry.agent.isEmpty
            ? entry.project
            : String(localized: "\(entry.agent), \(entry.project)"))
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: entry.needsAttention)
    }

    /// Static and status-free: `SeSessionRow.trailing` already renders
    /// `entry.statusText`, so the meta line must never repeat it.
    nonisolated static func rowMeta(agent: String, project: String) -> String {
        var parts: [String] = []
        let trimmedAgent = agent.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedAgent.isEmpty { parts.append(trimmedAgent) }
        parts.append(project)
        return parts.joined(separator: " · ")
    }

    private func rowMeta(_ entry: ConversationEntry) -> String {
        ConversationsScreen.rowMeta(agent: entry.agent, project: entry.project)
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("This host has no agent sessions yet.")
                .font(SeNocturneType.font(12, weight: .semibold))
                .foregroundStyle(palette.ink)
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
    @Environment(\.remoteDeviceLink) private var deviceLink
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
                    if projects.isEmpty {
                        Text("Add a project in Tools first.")
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
                                    meta: project.path ?? String(localized: "Project"),
                                    status: project.isRunning ? .live : .idle
                                ) {
                                    start(in: project)
                                }
                                .opacity(starting ? 0.6 : 1)
                            }
                            .buttonStyle(.plain)
                            .disabled(starting || !(deviceLink?.allows("agent.run") ?? true))
                        }
                    }
                }
                .padding(.horizontal, 18)
                .padding(.bottom, 24)
            }
            .background(palette.canvas)
            .navigationTitle(String(localized: "New session"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { onCancel() }
                        .frame(minHeight: SeSpace.hitTarget)
                }
            }
            .overlay {
                if starting { ProgressView("Starting agent session…").padding(18) }
            }
        }
    }

    private func start(in project: HostProject) {
        guard (deviceLink?.allows("agent.run") ?? true), !starting else { return }
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
