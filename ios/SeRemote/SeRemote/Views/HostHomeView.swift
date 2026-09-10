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
    @State private var selectedId: String?
    @State private var searchText = ""
    @State private var renaming: HostConversation?
    @State private var renameDraft = ""
    @State private var deleting: HostConversation?
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        Group {
            if sizeClass == .regular {
                wideLayout
            } else {
                compactLayout
            }
        }
        .background(SeTheme.canvas)
    }

    // MARK: Wide (iPad) layout — sidebar list + detail preview

    private var wideLayout: some View {
        NavigationSplitView {
            sidebar
                .navigationTitle(deskTitle)
                .navigationBarTitleDisplayMode(.inline)
        } detail: {
            detail
        }
        .navigationSplitViewStyle(.balanced)
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            Picker(String(localized: "Workspace"), selection: $section) {
                ForEach(HostHomeSection.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)

            List(filteredRows) { row in
                Button {
                    selectedId = row.id
                } label: {
                    HostListRow(
                        title: row.title,
                        preview: row.preview,
                        previewMono: true,
                        meta: row.meta,
                        status: .idle,
                        time: row.time,
                        glyph: row.glyph
                    )
                }
                .listRowBackground(SeTheme.canvas)
                .listRowSeparatorTint(SeTheme.stroke)
                .contextMenu { rowActions(for: row) }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .searchable(text: $searchText, prompt: Text("Search"))
        }
    }

    @ViewBuilder
    private var detail: some View {
        VStack {
            if let row = selectedRow {
                VStack(alignment: .leading, spacing: 18) {
                    HStack(spacing: 12) {
                        Image(systemName: row.glyph)
                            .font(.title2)
                            .foregroundStyle(SeTheme.accent)
                        Text(row.title)
                            .font(SeTheme.display)
                            .lineLimit(2)
                    }
                    Text(row.preview ?? "")
                        .font(.body.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(6)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(14)
                        .background(SeTheme.surface)
                        .clipShape(RoundedRectangle(cornerRadius: SeTheme.radius, style: .continuous))
                    Text(row.meta ?? "")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                    Button {
                        open(row)
                    } label: {
                        Text(row.openTitle)
                            .font(.body.bold())
                            .frame(minHeight: 48)
                            .frame(maxWidth: 240)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(SeTheme.accent)
                }
                .padding(32)
                .frame(maxWidth: 560, maxHeight: .infinity, alignment: .topLeading)
            } else {
                ContentUnavailableView(
                    String(localized: "Choose from the sidebar"),
                    systemImage: section == .sessions ? "bubble.left.and.bubble.right" : "folder",
                    description: Text(section == .sessions
                        ? "Pick a session to continue it here."
                        : "Pick a project to watch its terminals.")
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle(selectedRow?.title ?? String(localized: "Workspace"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    store.disconnect()
                } label: {
                    Image(systemName: "chevron.backward")
                }
                .accessibilityLabel(Text("Back to home"))
            }
        }
    }

    // MARK: Compact (iPhone) layout

    private var compactLayout: some View {
        NavigationStack {
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
            .navigationBarHidden(true)
        }
        .onChange(of: section) { _, _ in
            selectedId = nil
        }
        .alert(
            String(localized: "Rename session"),
            isPresented: Binding(
                get: { renaming != nil },
                set: { if !$0 { renaming = nil } }
            )
        ) {
            TextField(String(localized: "Session name"), text: $renameDraft)
            Button(String(localized: "Save")) {
                if let conversation = renaming {
                    Task { await session.conversations.rename(conversation, title: renameDraft) }
                }
                renaming = nil
            }
            Button(String(localized: "Cancel"), role: .cancel) {
                renaming = nil
            }
        }
        .confirmationDialog(
            String(localized: "Delete this session?"),
            isPresented: Binding(
                get: { deleting != nil },
                set: { if !$0 { deleting = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button(String(localized: "Delete"), role: .destructive) {
                if let conversation = deleting {
                    if session.conversations.active?.id == conversation.id {
                        session.leaveConversation()
                    }
                    Task { await session.conversations.delete(conversation) }
                }
                deleting = nil
            }
            Button(String(localized: "Cancel"), role: .cancel) {
                deleting = nil
            }
        } message: {
            Text("The desktop keeps its files; the chat is removed from the list.")
        }
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

    @ViewBuilder
    private var sessionList: some View {
        if filteredConversations.isEmpty {
            ContentUnavailableView(
                "No sessions yet",
                systemImage: "bubble.left.and.bubble.right",
                description: Text("Open a chat on the desktop. These sessions are not projects.")
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(filteredConversations) { conversation in
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
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    Button(role: .destructive) {
                        deleting = conversation
                    } label: {
                        Label(String(localized: "Delete"), systemImage: "trash")
                    }
                    Button {
                        renameDraft = conversation.displayTitle
                        renaming = conversation
                    } label: {
                        Label(String(localized: "Rename"), systemImage: "pencil")
                    }
                    .tint(SeTheme.lamp)
                }
                .contextMenu {
                    conversationActions(for: conversation, openAction: {
                        Task { await session.selectConversation(conversation) }
                    })
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .searchable(text: $searchText, prompt: Text("Search"))
        }
    }

    @ViewBuilder
    private var projectList: some View {
        if filteredProjects.isEmpty {
            ContentUnavailableView(
                "No projects",
                systemImage: "square.stack",
                description: Text("Open a project on the desktop to watch its terminals here.")
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(filteredProjects) { project in
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
                .contextMenu {
                    Button {
                        Task { await session.selectProject(project) }
                    } label: {
                        Label(String(localized: "Open project"), systemImage: "arrow.up.forward.app")
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .searchable(text: $searchText, prompt: Text("Search"))
        }
    }

    private var filteredConversations: [HostConversation] {
        let conversations = session.conversations.conversations
        guard !searchText.isEmpty else { return conversations }
        return conversations.filter {
            $0.displayTitle.localizedCaseInsensitiveContains(searchText)
                || $0.previewText.localizedCaseInsensitiveContains(searchText)
        }
    }

    private var filteredProjects: [HostProject] {
        let projects = session.projects.projects.filter { !$0.isArchived }
        guard !searchText.isEmpty else { return projects }
        return projects.filter {
            $0.name.localizedCaseInsensitiveContains(searchText)
                || ($0.path ?? "").localizedCaseInsensitiveContains(searchText)
        }
    }

    private var filteredRows: [HostHomeRow] {
        section == .sessions
            ? filteredConversations.map(HostHomeRow.init)
            : filteredProjects.map(HostHomeRow.init)
    }

    private var selectedRow: HostHomeRow? {
        filteredRows.first { $0.id == selectedId }
    }

    private func open(_ row: HostHomeRow) {
        if let conversation = filteredConversations.first(where: { $0.id == row.id }) {
            Task { await session.selectConversation(conversation) }
        } else if let project = filteredProjects.first(where: { $0.id == row.id }) {
            Task { await session.selectProject(project) }
        }
    }

    /// Sidebar rows route through the id back to the concrete record so the
    /// actions operate on the live model, not the projection.
    @ViewBuilder
    private func rowActions(for row: HostHomeRow) -> some View {
        if let conversation = filteredConversations.first(where: { $0.id == row.id }) {
            conversationActions(for: conversation, openAction: { open(row) })
        } else {
            Button {
                open(row)
            } label: {
                Label(String(localized: "Open project"), systemImage: "arrow.up.forward.app")
            }
        }
    }

    @ViewBuilder
    private func conversationActions(for conversation: HostConversation, openAction: @escaping () -> Void) -> some View {
        Button(action: openAction) {
            Label(String(localized: "Open session"), systemImage: "arrow.up.forward.app")
        }
        Button {
            renameDraft = conversation.displayTitle
            renaming = conversation
        } label: {
            Label(String(localized: "Rename"), systemImage: "pencil")
        }
        Button(role: .destructive) {
            deleting = conversation
        } label: {
            Label(String(localized: "Delete"), systemImage: "trash")
        }
    }

    private var deskTitle: String {
        link.title.split(separator: "·").first.map { String($0).trimmingCharacters(in: .whitespaces) }
            ?? link.title
    }
}

/// Sidebar/detail projection of a session or project entry.
private struct HostHomeRow: Identifiable {
    let id: String
    let title: String
    let preview: String?
    let meta: String?
    let time: String?
    let glyph: String
    let openTitle: String

    init(conversation: HostConversation) {
        id = conversation.id
        title = conversation.displayTitle
        preview = conversation.previewText
        meta = conversation.countLabel
        time = conversation.relativeCreatedLabel
        glyph = "bubble.left.and.bubble.right"
        openTitle = String(localized: "Open session")
    }

    init(project: HostProject) {
        id = project.id
        title = project.name
        preview = project.path
        meta = HostTimestamp.folderName(from: project.path)
        time = ""
        glyph = "folder"
        openTitle = String(localized: "Open project")
    }
}
