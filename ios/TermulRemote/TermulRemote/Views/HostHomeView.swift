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

struct HostHomeView: View {
    @Bindable var session: WorkspaceSession
    @Bindable var store: ConnectionStore
    let link: RemoteLink
    @State private var section: HostHomeSection = .sessions

    var body: some View {
        VStack(spacing: 0) {
            header
            Picker(String(localized: "Workspace"), selection: $section) {
                ForEach(HostHomeSection.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(minHeight: 44)

            switch section {
            case .sessions:
                sessionList
            case .projects:
                projectList
            }
        }
        .background(SeTheme.canvas)
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button {
                store.disconnect()
            } label: {
                Image(systemName: "chevron.backward")
                    .font(.body.bold())
                    .frame(minWidth: 44, minHeight: 44)
            }
            .accessibilityLabel(Text("Back to home"))

            VStack(alignment: .leading, spacing: 2) {
                Text(deskTitle)
                    .font(SeTheme.display)
                    .lineLimit(1)
                Text(section == .sessions
                     ? String(localized: "Independent chats, not project terminals")
                     : String(localized: "Projects already open on the computer"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
    }

    private var deskTitle: String {
        link.title.split(separator: "·").first.map { String($0).trimmingCharacters(in: .whitespaces) }
            ?? link.title
    }

    @ViewBuilder
    private var sessionList: some View {
        if session.conversations.conversations.isEmpty {
            ContentUnavailableView(
                "No sessions yet",
                systemImage: "bubble.left.and.bubble.right",
                description: Text("Open a chat on the desktop. These sessions are not projects.")
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(session.conversations.conversations) { conversation in
                Button {
                    Task { await session.selectConversation(conversation) }
                } label: {
                    HostListRow(
                        title: conversation.displayTitle,
                        preview: conversation.previewText,
                        previewMono: true,
                        meta: conversation.countLabel,
                        status: .idle,
                        time: conversation.relativeCreatedLabel,
                        glyph: "bubble.left.and.bubble.right"
                    )
                }
                .listRowBackground(SeTheme.canvas)
                .listRowSeparatorTint(SeTheme.stroke)
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
    }

    @ViewBuilder
    private var projectList: some View {
        let projects = session.projects.projects.filter { !$0.isArchived }
        if projects.isEmpty {
            ContentUnavailableView(
                "No projects",
                systemImage: "square.stack",
                description: Text("Open a project on the desktop to watch its terminals here.")
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(projects) { project in
                Button {
                    Task { await session.selectProject(project) }
                } label: {
                    HostListRow(
                        title: project.name,
                        preview: project.path,
                        previewMono: true,
                        meta: HostTimestamp.folderName(from: project.path),
                        glyph: "folder",
                        showsChevron: true
                    )
                }
                .listRowBackground(SeTheme.canvas)
                .listRowSeparatorTint(SeTheme.stroke)
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
    }
}
