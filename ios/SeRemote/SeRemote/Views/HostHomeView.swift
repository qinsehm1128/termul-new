import SwiftUI

enum HostHomeSection: String, CaseIterable, Identifiable {
    case sessions, projects
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
    /// Embedded in the workspace sidebar, where the parent owns sizing and chrome.
    var directoryOnly: Bool = false
    @State private var section: HostHomeSection = .sessions
    @State private var selectedSessionID: String?
    @State private var selectedProjectID: String?
    @State private var initializedSelection = false
    @State private var searchText = ""
    @State private var renaming: HostConversation?
    @State private var renameDraft = ""
    @State private var deleting: HostConversation?
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        Group {
            if directoryOnly {
                directory(previewSelection: false)
            } else {
                GeometryReader { geometry in
                    let columns = SeLayout.columns(
                        width: geometry.size.width,
                        isRegular: sizeClass == .regular,
                        largeText: dynamicTypeSize.isAccessibilitySize
                    )
                    if columns == .single {
                        directory(previewSelection: false)
                    } else {
                        HStack(spacing: 0) {
                            directory(previewSelection: true)
                                .frame(width: min(SeLayout.sidebarWidth.upperBound,
                                                  max(SeLayout.sidebarWidth.lowerBound, geometry.size.width * 0.3)))
                            Divider().overlay(SeTheme.stroke)
                            detail
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                        }
                    }
                }
                .navigationTitle(deskTitle)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            store.disconnect()
                        } label: {
                            Label(String(localized: "Disconnect"), systemImage: "eject")
                        }
                    }
                }
            }
        }
        .background(SeTheme.canvas)
        .onAppear {
            // A fold may reappear this view; do not reset a browsing selection.
            guard !initializedSelection else { return }
            initializedSelection = true
            selectedSessionID = session.conversations.active?.id
            selectedProjectID = session.projects.active?.id
            switch session.workspace {
            case .conversation: section = .sessions
            case .project: section = .projects
            case .home:
                if selectedSessionID != nil { section = .sessions }
                else if selectedProjectID != nil { section = .projects }
            }
        }
        .onChange(of: session.conversations.active?.id) { _, id in
            selectedSessionID = id
            if directoryOnly, id != nil { section = .sessions }
        }
        .onChange(of: session.projects.active?.id) { _, id in
            selectedProjectID = id
            if directoryOnly, id != nil { section = .projects }
        }
        // Both dialogs and both selection IDs survive width-class changes.
        .alert(
            String(localized: "Rename session"),
            isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } }),
            presenting: renaming
        ) { conversation in
            TextField(String(localized: "Session name"), text: $renameDraft)
            Button(String(localized: "Save")) {
                Task { await session.conversations.rename(conversation, title: renameDraft) }
            }
            Button(String(localized: "Cancel"), role: .cancel) {}
        }
        .confirmationDialog(
            String(localized: "Delete this session?"),
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible,
            presenting: deleting
        ) { conversation in
            Button(String(localized: "Delete"), role: .destructive) {
                if session.conversations.active?.id == conversation.id { session.leaveConversation() }
                Task { await session.conversations.delete(conversation) }
            }
            Button(String(localized: "Cancel"), role: .cancel) {}
        } message: { _ in
            Text("The desktop keeps its files; the chat is removed from the list.")
        }
    }

    private func directory(previewSelection: Bool) -> some View {
        VStack(spacing: 0) {
            Picker(String(localized: "Workspace"), selection: $section) {
                ForEach(HostHomeSection.allCases) { item in
                    Text(item.title).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, SeSpace.md)
            .padding(.vertical, SeSpace.sm)

            HStack(spacing: SeSpace.xs) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(SeTheme.secondaryInk)
                TextField(String(localized: "Search sessions or projects"), text: $searchText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityIdentifier("host-home-search")
                if !searchText.isEmpty {
                    Button { searchText = "" } label: {
                        Image(systemName: "xmark.circle.fill")
                            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    }
                    .accessibilityLabel(String(localized: "Clear search"))
                }
            }
            .padding(.horizontal, SeSpace.md)
            .frame(minHeight: SeSpace.hitTarget)

            // Always keep a scroll container, even when empty, so pull-to-refresh
            // is available without requiring an already populated directory.
            List {
                if section == .sessions {
                    Section {
                        ForEach(filteredConversations) { conversation in
                            Button {
                                selectedSessionID = conversation.id
                                if !previewSelection { open(conversation) }
                            } label: {
                                conversationRow(conversation)
                            }
                            .listRowBackground(rowBackground(selectedSessionID == conversation.id))
                            .listRowSeparatorTint(SeTheme.stroke)
                            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                Button(role: .destructive) { deleting = conversation } label: {
                                    Label(String(localized: "Delete"), systemImage: "trash")
                                }
                                Button { beginRename(conversation) } label: {
                                    Label(String(localized: "Rename"), systemImage: "pencil")
                                }
                                .tint(SeTheme.accent)
                            }
                            .contextMenu { conversationActions(conversation) }
                        }
                        if filteredConversations.isEmpty { emptyState }
                    } header: {
                        SeSectionHeader(title: String(localized: "Sessions"), detail: "\(filteredConversations.count)")
                    }
                } else {
                    ForEach(projectSections) { group in
                        Section {
                            ForEach(group.projects) { project in
                                Button {
                                    selectedProjectID = project.id
                                    if !previewSelection { open(project) }
                                } label: {
                                    projectRow(project)
                                }
                                .listRowBackground(rowBackground(selectedProjectID == project.id))
                                .listRowSeparatorTint(SeTheme.stroke)
                                .contextMenu {
                                    Button { open(project) } label: {
                                        Label(String(localized: "Open project"), systemImage: "arrow.up.forward.app")
                                    }
                                }
                            }
                        } header: {
                            SeSectionHeader(title: group.title, detail: "\(group.projects.count)")
                        }
                    }
                    if projectSections.isEmpty {
                        Section { emptyState } header: {
                            SeSectionHeader(title: String(localized: "Projects"), detail: "0")
                        }
                    }
                }
                Section {
                    HStack {
                        Label(String(localized: "Running terminals"), systemImage: "terminal")
                        Spacer()
                        Text("\(ShellHostProjects.terminalCount(in: session.projects.projects))")
                            .monospacedDigit()
                    }
                    .font(SeFont.support)
                    .foregroundStyle(SeTheme.secondaryInk)
                    .listRowBackground(SeTheme.canvas)
                } header: {
                    SeSectionHeader(title: String(localized: "Workspace"))
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .scrollBounceBehavior(.always)
            .refreshable { await refreshDirectory() }

        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if listIsLoading {
            HStack { Spacer(); ProgressView(); Spacer() }
                .padding(.vertical, SeSpace.lg)
                .listRowBackground(SeTheme.canvas)
        } else if !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            ContentUnavailableView.search(text: searchText)
                .listRowBackground(SeTheme.canvas)
        } else {
            ContentUnavailableView(
                section == .sessions ? String(localized: "No sessions") : String(localized: "No projects"),
                systemImage: section == .sessions ? "bubble.left.and.bubble.right" : "square.stack",
                description: Text(section == .sessions
                    ? String(localized: "Create a session on the desktop to continue it here.")
                    : String(localized: "Open a project on the desktop to watch its terminals here."))
            )
            .listRowBackground(SeTheme.canvas)
        }
    }

    private var detail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.md) {
                if section == .sessions,
                   let conversation = session.conversations.conversations.first(where: { $0.id == selectedSessionID }) {
                    SeSectionHeader(title: String(localized: "Session"))
                    Text(conversation.displayTitle).font(SeFont.screenTitle)
                    conversationRow(conversation)
                    Button { open(conversation) } label: {
                        Label(String(localized: "Open session"), systemImage: "arrow.up.forward.app")
                    }
                    .buttonStyle(.borderedProminent)
                    .contextMenu { conversationActions(conversation) }
                } else if section == .projects,
                          let project = session.projects.projects.first(where: { $0.id == selectedProjectID }) {
                    SeSectionHeader(title: String(localized: "Project"))
                    Text(project.name).font(SeFont.screenTitle)
                    projectRow(project)
                    VStack(alignment: .leading, spacing: SeSpace.xxs) {
                        LabeledContent(String(localized: "Host terminal screen engine")) {
                            Text(project.terminalScreenEngine.rawValue)
                                .font(SeFont.support.monospaced())
                        }
                        Text("Read-only host preference. iOS rendering is unchanged.")
                            .font(SeFont.caption)
                            .foregroundStyle(SeTheme.secondaryInk)
                    }
                    Button { open(project) } label: {
                        Label(String(localized: "Open project"), systemImage: "arrow.up.forward.app")
                    }
                    .buttonStyle(.borderedProminent)
                } else {
                    ContentUnavailableView(
                        String(localized: "Choose a session or project"),
                        systemImage: "sidebar.left",
                        description: Text("Your sessions stay on the host. Choose one to continue working.")
                    )
                }
            }
            .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
            .padding(SeSpace.lg)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }

    private func conversationRow(_ conversation: HostConversation) -> HostListRow {
        HostListRow(
            title: conversation.displayTitle,
            preview: conversation.previewText,
            previewMono: true,
            meta: conversation.countLabel,
            lifecycle: ShellHostLifecycle(conversation.lifecycleState),
            time: conversation.relativeCreatedLabel,
            showsChevron: true
        )
    }

    private func projectRow(_ project: HostProject) -> HostListRow {
        HostListRow(
            title: project.name,
            preview: HostTimestamp.folderName(from: project.path),
            meta: String(localized: "\(max(0, project.liveTerminalCount)) live terminals"),
            status: project.isRunning ? .working : .idle,
            glyph: project.isRunning ? "bolt.horizontal.circle.fill" : "folder",
            showsChevron: true
        )
    }

    private func rowBackground(_ selected: Bool) -> Color {
        selected ? SeTheme.accentSubtle : SeTheme.canvas
    }

    private var filteredConversations: [HostConversation] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return session.conversations.conversations.filter {
            query.isEmpty || $0.displayTitle.localizedCaseInsensitiveContains(query)
                || $0.previewText.localizedCaseInsensitiveContains(query)
        }
    }

    private var projectSections: [ShellHostProjectSection] {
        ShellHostProjects.sections(projects: session.projects.projects, groups: session.projects.groups, query: searchText)
    }

    private func refreshDirectory() async {
        // Sequential actor-isolated refresh avoids moving the observable stores
        // across executors under strict Swift 6 checking. Both are always fetched.
        await session.conversations.refresh()
        await session.projects.refresh()
    }

    private func open(_ conversation: HostConversation) {
        Task { await session.selectConversation(conversation) }
    }

    private func open(_ project: HostProject) {
        Task { await session.selectProject(project) }
    }

    private func beginRename(_ conversation: HostConversation) {
        renameDraft = conversation.displayTitle
        renaming = conversation
    }

    @ViewBuilder
    private func conversationActions(_ conversation: HostConversation) -> some View {
        Button { open(conversation) } label: {
            Label(String(localized: "Open session"), systemImage: "arrow.up.forward.app")
        }
        Button { beginRename(conversation) } label: {
            Label(String(localized: "Rename"), systemImage: "pencil")
        }
        Button(role: .destructive) { deleting = conversation } label: {
            Label(String(localized: "Delete"), systemImage: "trash")
        }
    }

    private var deskTitle: String {
        link.title.split(separator: "·").first.map { String($0).trimmingCharacters(in: .whitespaces) } ?? link.title
    }

    private var listIsLoading: Bool {
        section == .sessions ? session.conversations.isLoading : session.projects.isLoading
    }
}
