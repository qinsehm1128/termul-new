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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var destination: SidebarDestination = .live
    @State private var showsWorkbench = false
    @State private var showsNeeds = false
    @State private var showsCommand = false
    @State private var showsDrawer = false
    @State private var showsHosts = false
    @State private var showsDirectory = false
    @State private var railCollapsed: Bool?
    @State private var openingSession = false
    @State private var selectionTask: Task<Void, Never>?

    private var hostName: String { link.title }

    private var needs: [NeedsItem] {
        LivePresentation.needs(
            permissions: session.chat.permissions,
            questions: session.chat.questions,
            tools: session.chat.tools,
            context: session.conversations.active?.displayTitle ?? hostName
        )
    }

    private var sessions: [LiveSession] {
        LivePresentation.sessions(
            conversations: session.conversations.conversations,
            projects: session.projects.projects,
            host: hostName,
            activeConversationID: session.workspace == .conversation ? session.conversations.active?.id : nil,
            activeProjectID: session.workspace == .project ? session.projects.active?.id : nil,
            agent: session.chat.activeAgentId,
            isRunning: LivePresentation.isRunning(isSending: session.chat.isSending,
                                                  messages: session.chat.messages,
                                                  tools: session.chat.tools),
            needsAttention: !needs.isEmpty,
            agentSessions: session.liveAgents.sessions
        )
    }

    private var needSummary: LiveNeedsSummary { LivePresentation.summary(items: needs) }
    private var workbenchVisible: Bool { showsWorkbench && session.workspace != .home }
    private var themeIcon: SeIcon { mode == .night ? .moon : .sun }

    var body: some View {
        GeometryReader { geometry in
            let sidebar = ShellPresentation.usesSidebar(isRegular: sizeClass == .regular, width: geometry.size.width)
            let portrait = geometry.size.width <= geometry.size.height
            let collapsed = railCollapsed ?? ShellPresentation.defaultRailCollapsed(
                width: geometry.size.width, height: geometry.size.height)
            let railWidth = collapsed ? ShellPresentation.railCollapsedWidth : ShellPresentation.railExpandedWidth

            ZStack(alignment: .leading) {
                page
                    .padding(.leading, sidebar ? railWidth : 0)
                    .safeAreaInset(edge: .bottom, spacing: 0) {
                        if !sidebar && !workbenchVisible { bottomBar }
                    }
                    .opacity(workbenchVisible ? 0 : 1)
                    .allowsHitTesting(!workbenchVisible)
                    .accessibilityHidden(workbenchVisible)

                // Deliberately outside every navigation/layout conditional.
                SessionScreen(
                    session: session,
                    hostDirectory: AnyView(
                        HostHomeView(session: session, store: store, link: link, directoryOnly: true)
                    ),
                    onBack: closeWorkbench,
                    isActive: showsWorkbench || session.workspace != .home
                )
                .padding(.leading, sidebar ? railWidth : 0)
                .opacity(workbenchVisible ? 1 : 0)
                .allowsHitTesting(workbenchVisible)
                .accessibilityHidden(!workbenchVisible)

                if sidebar {
                    AdaptiveSidebarShell(
                        selection: $destination,
                        collapsed: Binding(get: { collapsed }, set: { railCollapsed = $0 }),
                        hostName: hostName,
                        hostOnline: session.phase == .connected,
                        counts: [.live: "\(sessions.count)"],
                        version: ShellPresentation.appVersion,
                        onOpenCommand: { showsCommand = true }
                    )
                }

                if showsDrawer {
                    phoneDrawer(selection: destination)
                }

                if showsCommand {
                    ConnectedCommandPalette(session: session, onOpenSession: {
                        showsCommand = false
                        showsWorkbench = true
                    }, onClose: { showsCommand = false })
                    .transition(.opacity)
                    .zIndex(20)
                }
            }
            .onChange(of: portrait) { _, _ in railCollapsed = nil }
            .onChange(of: destination) { _, _ in closeWorkbench() }
        }
        .background(palette.canvas.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .onChange(of: session.workspace) { _, workspace in
            if workspace != .home { showsWorkbench = true }
        }
        .onDisappear { selectionTask?.cancel() }
        .sheet(isPresented: $showsNeeds) {
            NeedsSheetView(items: needs, onOpen: openNeed)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.hidden)
        }
        .sheet(isPresented: $showsHosts) { hostPicker }
        .fullScreenCover(isPresented: $showsDirectory) {
            HostHomeView(session: session, store: store, link: link, directoryOnly: false,
                         onClose: { showsDirectory = false })
                .background(palette.canvas.ignoresSafeArea())
        }
        .background {
            VStack {
                Button("命令面板") { showsCommand = true }
                    .keyboardShortcut("k", modifiers: .command)
            }
            .hidden()
        }
        .disabled(openingSession)
        .overlay {
            if openingSession { ProgressView().accessibilityLabel("正在打开会话") }
        }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: showsCommand)
    }

    // MARK: - Pages

    @ViewBuilder private var page: some View {
        switch destination {
        case .live:
            LiveView(
                hostName: hostName,
                hostMeta: session.phase == .connected ? "已连接" : "连接中",
                sessions: sessions,
                needs: needs,
                summary: needSummary,
                themeIcon: themeIcon,
                onSelectHost: { showsDirectory = true },
                onOpenSession: openSession,
                onOpenNeeds: { showsNeeds = true },
                onToggleTheme: { settings.appearance = mode.toggled.appearance },
                onOpenRail: { showsDrawer = true }
            )
        case .workspace:
            WorkspaceHubScreen(
                session: session,
                onOpenSession: { showsWorkbench = true },
                onOpenCommand: { showsCommand = true },
                onToggleTheme: { settings.appearance = mode.toggled.appearance },
                onOpenDrawer: { showsDrawer = true },
                onSelectHost: { showsHosts = true }
            )
        case .settings:
            SettingsScreen(settings: settings, store: store, onBack: { destination = .live })
        case .files, .changes, .schedules, .quick:
            WorkspaceHubScreen(
                session: session,
                onOpenSession: { showsWorkbench = true },
                onOpenCommand: { showsCommand = true },
                onToggleTheme: { settings.appearance = mode.toggled.appearance },
                onOpenDrawer: { showsDrawer = true },
                onSelectHost: { showsHosts = true }
            )
        }
    }

    // MARK: - Phone chrome

    private var bottomBar: some View {
        SeBottomBar(
            selection: destination,
            onLive: { destination = .live },
            onWorkspace: { destination = .workspace },
            onCommand: { showsCommand = true }
        )
    }

    private func phoneDrawer(selection current: SidebarDestination) -> some View {
        ZStack(alignment: .leading) {
            Color.black.opacity(mode.isNight ? 0.5 : 0.22)
                .ignoresSafeArea()
                .onTapGesture { showsDrawer = false }
            AdaptiveSidebarShell(
                selection: $destination,
                collapsed: .constant(false),
                hostName: hostName,
                hostOnline: session.phase == .connected,
                counts: [.live: "\(sessions.count)"],
                version: ShellPresentation.appVersion,
                onOpenCommand: {
                    showsDrawer = false
                    showsCommand = true
                },
                expandedWidth: 288
            )
            .transition(.move(edge: .leading).combined(with: .opacity))
        }
        .zIndex(30)
        .animation(reduceMotion ? nil : SeNocturneMotion.sheet, value: showsDrawer)
        .onChange(of: destination) { _, _ in showsDrawer = false }
    }

    private var hostPicker: some View {
        NavigationStack {
            List {
                ForEach(store.savedLinks) { saved in
                    Button {
                        showsHosts = false
                        if saved.id != link.id { store.connect(link: saved) }
                    } label: {
                        HStack {
                            Text(saved.title).foregroundStyle(palette.ink)
                            Spacer()
                            if saved.id == link.id { Text("当前").font(SeNocturneType.font(9, mono: true)) }
                        }
                        .frame(minHeight: 44)
                    }
                    .listRowBackground(palette.card)
                }
                Button {
                    showsHosts = false
                    showsWorkbench = true
                } label: {
                    Text("查看这台主机的工作台").frame(minHeight: 44)
                }
                .listRowBackground(palette.card)
            }
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .navigationTitle("主机")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { showsHosts = false }
                }
            }
        }
        .presentationDetents([.medium])
    }

    // MARK: - Actions

    private func closeWorkbench() {
        showsWorkbench = false
        showsDrawer = false
        if session.workspace != .home { session.leaveWorkspace() }
    }

    private func openSession(_ item: LiveSession) {
        guard !openingSession else { return }
        openingSession = true
        selectionTask = Task {
            defer { openingSession = false; selectionTask = nil }
            switch LivePresentation.destination(id: item.id) {
            case let .conversation(id):
                guard let conversation = session.conversations.conversations.first(where: { $0.id == id }) else { return }
                await session.selectConversation(conversation)
            case let .project(id):
                guard let project = session.projects.projects.first(where: { $0.id == id }) else { return }
                await session.selectProject(project)
            case nil:
                return
            }
            guard !Task.isCancelled else { return }
            showsWorkbench = true
        }
    }

    private func openNeed(_ item: NeedsItem) {
        showsNeeds = false
        guard session.workspace != .home else { return }
        switch item.kind {
        case .permission, .question:
            session.setWorkspaceTab(.chat)
        case .failure:
            session.setWorkspaceTab(.terminal)
        case .done, .running:
            break
        }
        showsWorkbench = true
    }
}
