import SwiftUI

/// The workbench is a single stable sibling of the adaptive chrome. Changing a
/// destination, folding, or toggling the rail never reconstructs its TerminalScreen.
struct ConnectedShellScreen: View {
    @Bindable var session: WorkspaceSession
    @Bindable var store: ConnectionStore
    let link: RemoteLink
    @Environment(AppSettings.self) private var settings
    @Environment(\.seThemeMode) private var mode
    @Environment(\.sePalette) private var palette
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var destination: SidebarDestination = .live
    @State private var showsWorkbench = false
    @State private var showsNeeds = false
    @State private var showsCommand = false
    @State private var showsHosts = false
    @State private var sidebarWidth: CGFloat = 76
    @State private var openingSession = false
    @State private var selectionTask: Task<Void, Never>?

    private var hostName: String { link.title }
    private var needs: [NeedsItem] {
        LivePresentation.needs(permissions: session.chat.permissions, questions: session.chat.questions,
                               tools: session.chat.tools, context: session.conversations.active?.displayTitle ?? hostName)
    }
    private var sessions: [LiveSession] {
        LivePresentation.sessions(
            conversations: session.conversations.conversations, projects: session.projects.projects, host: hostName,
            activeConversationID: session.workspace == .conversation ? session.conversations.active?.id : nil,
            activeProjectID: session.workspace == .project ? session.projects.active?.id : nil,
            agent: session.chat.activeAgentId, isRunning: LivePresentation.isRunning(isSending: session.chat.isSending, messages: session.chat.messages, tools: session.chat.tools), needsAttention: !needs.isEmpty)
    }
    private var workbenchVisible: Bool { showsWorkbench && session.workspace != .home }

    var body: some View {
        GeometryReader { geometry in
            let sidebar = ShellPresentation.usesSidebar(isRegular: sizeClass == .regular, width: geometry.size.width)
            ZStack {
                if sidebar {
                    AdaptiveSidebarShell(
                        selection: Binding(get: { destination }, set: { selected in
                            closeWorkbench()
                            destination = selected
                        }),
                        hosts: [.init(id: link.id.uuidString, name: hostName, isOnline: session.phase == .connected)],
                        counts: [.live: "\(sessions.count)"],
                        onOpenCommand: { showsCommand = true },
                        onWidthChange: { sidebarWidth = $0 }
                    ) { selected in
                        destinationView(selected)
                    }
                } else {
                    destinationView(destination)
                        .safeAreaInset(edge: .bottom, spacing: 0) { bottomBar }
                }

                // Deliberately outside every navigation/layout conditional.
                SessionScreen(session: session, hostDirectory: AnyView(
                    HostHomeView(session: session, store: store, link: link, directoryOnly: true)
                ), onBack: closeWorkbench, isActive: showsWorkbench)
                .opacity(workbenchVisible ? 1 : 0)
                .allowsHitTesting(workbenchVisible)
                .accessibilityHidden(!workbenchVisible)
                .padding(.leading, sidebar ? sidebarWidth : 0)
            }
            .disabled(openingSession)
            .overlay {
                if openingSession { ProgressView().accessibilityLabel("Opening session") }
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $showsNeeds) {
            NeedsSheetView(items: needs, onOpen: { _ in
                showsNeeds = false
                if session.workspace != .home { session.setWorkspaceTab(.chat); showsWorkbench = true }
            })
        }
        .sheet(isPresented: $showsCommand) {
            ConnectedCommandPalette(session: session, onOpenSession: {
                showsCommand = false
                showsWorkbench = true
            })
        }
        .sheet(isPresented: $showsHosts) {
            NavigationStack {
                List {
                    ForEach(store.savedLinks) { saved in
                        Button(saved.title) {
                            showsHosts = false
                            if saved.id != link.id { store.connect(link: saved) }
                        }
                        .frame(minHeight: 44)
                    }
                }
                .navigationTitle("Hosts")
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { showsHosts = false }
                    }
                }
            }
        }
        .background {
            // Hidden commands remain registered for hardware keyboards.
            VStack {
                Button("Command palette") { showsCommand = true }.keyboardShortcut("k", modifiers: .command)
            }
            .hidden()
        }
        .onChange(of: destination) { _, _ in closeWorkbench() }
        .onDisappear { selectionTask?.cancel() }
        .onChange(of: session.workspace) { _, workspace in
            if workspace != .home { showsWorkbench = true }
        }
    }

    @ViewBuilder private func destinationView(_ selected: SidebarDestination) -> some View {
        switch selected {
        case .live:
            LiveView(
                hosts: [.init(id: link.id.uuidString, name: hostName, latency: "", isOnline: session.phase == .connected)],
                sessions: sessions, needs: LivePresentation.summary(items: needs), showsBottomBar: false,
                onSelectHost: { _ in showsHosts = true }, onOpenSession: openSession,
                onOpenNeeds: { showsNeeds = true }, onOpenWorkspace: { destination = .workspace },
                onOpenCommand: { showsCommand = true }, onOpenDrawer: { destination = .settings },
                onToggleTheme: { settings.appearance = mode.toggled.appearance })
        case .workspace:
            WorkspaceHubScreen(session: session, onOpenSession: { showsWorkbench = true },
                               onOpenCommand: { showsCommand = true })
        case .files:
            FileBrowserView(session: session, embedded: true)
        case .changes:
            WorkspaceToolDestinationView(session: session, destination: .git)
        case .schedules:
            WorkspaceToolDestinationView(session: session, destination: .scheduledTasks)
        case .quick:
            WorkspaceToolDestinationView(session: session, destination: .quickTerminals)
        case .settings:
            SettingsScreen(settings: settings, store: store, onBack: { destination = .live })
        }
    }

    private func closeWorkbench() {
        showsWorkbench = false
        if session.workspace != .home { session.leaveWorkspace() }
    }

    private func openSession(_ item: LiveSession) {
        guard !openingSession else { return }
        openingSession = true
        selectionTask = Task {
            defer { openingSession = false; selectionTask = nil }
            switch LivePresentation.destination(id: item.id) {
            case .conversation(let id):
                guard let conversation = session.conversations.conversations.first(where: { $0.id == id }) else { return }
                await session.selectConversation(conversation)
            case .project(let id):
                guard let project = session.projects.projects.first(where: { $0.id == id }) else { return }
                await session.selectProject(project)
            case nil: return
            }
            guard !Task.isCancelled else { return }
            showsWorkbench = true
        }
    }

    private var bottomBar: some View {
        HStack(spacing: 8) {
            tab("Live", icon: "cube", selected: destination == .live) { destination = .live }
            tab("Needs you", icon: "clock", selected: false) { showsNeeds = true }
            tab("Workspace", icon: "folder", selected: destination == .workspace) { destination = .workspace }
            Button { showsCommand = true } label: {
                Image(systemName: "command")
                    .frame(width: 44, height: 44)
                    .foregroundStyle(palette.accent)
                    .background(palette.surface, in: RoundedRectangle(cornerRadius: 12))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Command palette")
        }
        .padding(.horizontal, SeSpace.lg)
        .padding(.vertical, SeSpace.sm)
        .background(palette.canvas)
        .overlay(alignment: .top) { Rectangle().fill(palette.stroke).frame(height: 1) }
    }

    private func tab(_ title: LocalizedStringKey, icon: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 4) {
                Image(systemName: icon)
                Text(title).font(SeNocturneType.font(10))
            }
            .frame(maxWidth: .infinity, minHeight: 44)
            .foregroundStyle(selected ? palette.ink : palette.inkTertiary)
            .background(selected ? palette.surface : .clear, in: RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
    }
}
