import SwiftUI

struct TerminalTabStrip: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(session.terminals.terminals) { item in
                    Button {
                        Task { await session.revealTerminal(item.id) }
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.title)
                                .font(.subheadline.weight(.semibold))
                                .lineLimit(1)
                            if let folder = HostTimestamp.folderName(from: item.cwd) {
                                Text(folder)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            } else if let branch = item.gitBranch, !branch.isEmpty {
                                Text(branch)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Text(session.terminals.activeId == item.id
                                 ? String(localized: "Active")
                                 : String(localized: "Idle"))
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .frame(minHeight: 44)
                        .background(
                            session.terminals.activeId == item.id && (session.workspace == .project || !session.showChat)
                                ? SeTheme.accent.opacity(0.16)
                                : SeTheme.surface
                        )
                        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .stroke(SeTheme.stroke, lineWidth: 1)
                        )
                    }
                    .buttonStyle(.plain)
                }

                Button {
                    Task { await spawn() }
                } label: {
                    Image(systemName: "plus")
                        .frame(minWidth: 44, minHeight: 44)
                }
                .accessibilityLabel(Text("New terminal"))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .background(.ultraThinMaterial)
    }

    private func spawn() async {
        switch session.workspace {
        case .conversation:
            guard let conversation = session.conversations.active else { return }
            await session.terminals.spawn(
                conversationId: conversation.id,
                projectId: conversation.projectId
            )
        case .project:
            guard let project = session.projects.active else { return }
            await session.terminals.spawn(
                conversationId: nil,
                projectId: project.id
            )
        case .home:
            return
        }
        if let id = session.terminals.activeId {
            await session.revealTerminal(id)
        }
    }
}
