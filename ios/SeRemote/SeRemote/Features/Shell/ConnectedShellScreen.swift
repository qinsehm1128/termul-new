import SwiftUI

/// The connected shell.
///
/// One terminal surface plus a directory. The workbench is a stable opacity
/// sibling of the pages, so switching destinations, opening the drawer or
/// presenting a sheet never reconstructs its `TerminalScreen` (I1) and never
/// ends a host PTY (I5). T1 lists terminals; T2 is the workbench; the bottom
/// bar is the only place the three destinations live.
struct ConnectedShellScreen: View {
    @Bindable var session: WorkspaceSession
    @Bindable var store: ConnectionStore
    let link: RemoteLink

    @Environment(AppSettings.self) private var settings
    @Environment(\.seThemeMode) private var mode
    @Environment(\.sePalette) private var palette
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var navigation = ShellNavigationState()
    @State private var hub = TerminalsHubStore()
    @State private var terminalModel: TerminalWorkbenchModel

    @State private var showsNeeds = false
    @State private var showsCommand = false
    @State private var showsDrawer = false
    @State private var showsHosts = false
    @State private var showsFiles = false
    @State private var showsAttachment = false
    @State private var showsChatCover = false
    @State private var preview: FilePreviewRequest?
    @State private var inspectorSegment: TerminalInspectorSegment = .files
    @State private var railCollapsed: Bool?
    @State private var openingSession = false
    @State private var selectionTask: Task<Void, Never>?
    @State private var landingNotice: String?
    @State private var refreshingHub = false
    @State private var hubRefreshTask: Task<Void, Never>?
    @State private var didBootstrap = false

    init(session: WorkspaceSession, store: ConnectionStore, link: RemoteLink) {
        self.session = session
        self.store = store
        self.link = link
        _terminalModel = State(initialValue: TerminalWorkbenchModel(session: session))
    }

    private var hostName: String { link.title }
    private var workbenchVisible: Bool { navigation.workbenchVisible }
    private var themeIcon: SeIcon { mode == .night ? .moon : .sun }

    private var needs: [NeedsItem] {
        LivePresentation.needs(
            permissions: session.chat.permissions,
            questions: session.chat.questions,
            tools: session.chat.tools,
            context: session.conversations.active?.displayTitle ?? hostName
        )
    }

    private var conversationEntries: [ConversationEntry] {
        LivePresentation.conversations(
            session.conversations.conversations,
            activeConversationID: session.workspace == .conversation ? session.conversations.active?.id : nil,
            agent: session.chat.activeAgentId,
            isRunning: LivePresentation.isRunning(isSending: session.chat.isSending,
                                                  messages: session.chat.messages,
                                                  tools: session.chat.tools),
            needsAttention: !needs.isEmpty
        )
    }

    private var needSummary: LiveNeedsSummary { LivePresentation.summary(items: needs) }

    /// The iPad rail is the terminal list, so it reads the same grouped model
    /// T1 renders.
    private var railGroups: [TerminalLandingModel.Group] {
        TerminalLandingModel.make(
            summaries: hub.summaries,
            projects: session.projects.projects,
            quickRecords: [],
            runningAgentTerminalIDs: Set(session.liveAgents.sessions.map(\.terminalId)),
            lastTerminalID: nil,
            allowsCreate: false
        ).groups
    }

    private var fileRoot: String? {
        terminalModel.activeCwd ?? session.activeToolRoot ?? session.workspaceRoot
    }

    var body: some View {
        GeometryReader { geometry in
            let sidebar = ShellPresentation.usesSidebar(isRegular: sizeClass == .regular, width: geometry.size.width)
            let portrait = geometry.size.width <= geometry.size.height
            let collapsed = railCollapsed ?? ShellPresentation.defaultRailCollapsed(
                width: geometry.size.width, height: geometry.size.height)
            let railWidth = collapsed ? ShellPresentation.railCollapsedWidth : ShellPresentation.railExpandedWidth
            let showsInspector = sidebar && workbenchVisible

            HStack(spacing: 0) {
                ZStack(alignment: .leading) {
                    page(sidebar: sidebar)
                        .padding(.leading, sidebar ? railWidth : 0)
                        .safeAreaInset(edge: .bottom, spacing: 0) {
                            if !sidebar && !workbenchVisible { bottomBar }
                        }
                        .opacity(workbenchVisible ? 0 : 1)
                        .allowsHitTesting(!workbenchVisible)
                        .accessibilityHidden(workbenchVisible)

                    // Deliberately outside every navigation/layout conditional.
                    workbench(sidebar: sidebar)
                        .padding(.leading, sidebar ? railWidth : 0)
                        .opacity(workbenchVisible ? 1 : 0)
                        .allowsHitTesting(workbenchVisible)
                        .accessibilityHidden(!workbenchVisible)

                    if sidebar {
                        AdaptiveSidebarShell(
                            selection: Binding(
                                get: { navigation.destination },
                                set: { navigation.select($0) }
                            ),
                            collapsed: Binding(get: { collapsed }, set: { railCollapsed = $0 }),
                            hostName: hostName,
                            hostOnline: session.phase == .connected,
                            counts: [.conversations: "\(conversationEntries.count)"],
                            version: ShellPresentation.appVersion,
                            onOpenCommand: { showsCommand = true },
                            terminalGroups: railGroups,
                            selectedTerminalID: terminalModel.activeID,
                            onOpenTerminal: { row in
                                Task { await open(TerminalLandingModel.request(for: row)) }
                            }
                        )
                    }

                    if showsDrawer {
                        phoneDrawer
                    }

                    if showsCommand {
                        ConnectedCommandPalette(
                            session: session,
                            onOpenSession: {
                                showsCommand = false
                                reopenWorkbench()
                            },
                            onOpenConversation: { conversationID in
                                showsCommand = false
                                openConversation(id: conversationID)
                            },
                            onOpenProject: { projectID in
                                showsCommand = false
                                openProject(projectID)
                            },
                            onClose: { showsCommand = false }
                        )
                        .transition(.opacity)
                        .zIndex(20)
                    }
                }

                if showsInspector {
                    inspector(root: fileRoot)
                        .frame(width: 320)
                        .transition(.move(edge: .trailing))
                }
            }
            .onChange(of: portrait) { _, _ in railCollapsed = nil }
        }
        .background(palette.canvas.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .onAppear(perform: bootstrap)
        .onDisappear {
            selectionTask?.cancel()
            hubRefreshTask?.cancel()
        }
        .onChange(of: workbenchVisible) { _, visible in
            // The workbench stays mounted while hidden; drop the keyboard with
            // it so no first responder survives in an opacity-0 view.
            session.terminalSurfaceVisible = visible
            if !visible { session.dismissTerminalKeyboard() }
        }
        .onChange(of: session.phase) { _, phase in
            guard phase == .connected else { return }
            Task { await consumePendingRoute() }
        }
        .onChange(of: store.pendingTerminalRoute) { _, route in
            guard route != nil, session.phase == .connected else { return }
            Task { await consumePendingRoute() }
        }
        .onChange(of: navigation.destination) { _, _ in
            showsDrawer = false
        }
        .sheet(isPresented: $showsNeeds) {
            NeedsSheetView(items: needs, onOpen: openNeed)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.hidden)
        }
        .sheet(isPresented: $showsHosts) { hostPicker }
        .sheet(isPresented: $showsFiles) { filesSheet }
        .sheet(isPresented: $showsAttachment) { attachmentSheet }
        .fullScreenCover(item: $preview) { request in
            TerminalFilePreviewCover(
                session: session,
                request: request,
                agentAvailable: terminalModel.agentReady,
                onInsertPath: { path in
                    terminalModel.insertPath(path)
                    preview = nil
                },
                onSendToAgent: { path in
                    terminalModel.sendToAgent(path)
                    preview = nil
                },
                onClose: { preview = nil }
            )
            .background(palette.canvas.ignoresSafeArea())
        }
        .fullScreenCover(isPresented: $showsChatCover) { chatCover }
        .background {
            VStack {
                Button("Command palette") { showsCommand = true }
                    .keyboardShortcut("k", modifiers: .command)
            }
            .hidden()
        }
        .disabled(openingSession)
        .overlay {
            if openingSession {
                ProgressView().accessibilityLabel(String(localized: "Opening terminal"))
            }
        }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: showsCommand)
    }

    // MARK: - Pages

    @ViewBuilder
    private func page(sidebar: Bool) -> some View {
        switch navigation.destination {
        case .terminals:
            if sidebar {
                iPadTerminalEmptyState
            } else {
                TerminalsHubScreen(
                    session: session,
                    store: hub,
                    hostName: hostName,
                    hostMeta: session.phase == .connected
                        ? String(localized: "Connected")
                        : String(localized: "Connecting"),
                    themeIcon: themeIcon,
                    lastTerminalID: navigation.openRequest?.terminalID,
                    allowsCreate: link.allows("terminal.create"),
                    isRefreshing: refreshingHub,
                    notice: landingNotice,
                    onRefresh: { Task { await refreshHub() } },
                    onSelectHost: { showsHosts = true },
                    onToggleTheme: { settings.appearance = mode.toggled.appearance },
                    onOpenDrawer: { showsDrawer = true },
                    onOpenCommand: { showsCommand = true },
                    onOpen: { request in Task { await open(request) } }
                )
            }
        case .conversations:
            ConversationsScreen(
                session: session,
                hostName: hostName,
                hostMeta: session.phase == .connected ? String(localized: "Connected") : String(localized: "Connecting"),
                conversations: conversationEntries,
                needs: needs,
                summary: needSummary,
                themeIcon: themeIcon,
                onSelectHost: { showsHosts = true },
                onOpenConversation: openConversation,
                onOpenWorkbench: { showsChatCover = true },
                onOpenNeeds: { showsNeeds = true },
                onOpenCommand: { showsCommand = true },
                onToggleTheme: { settings.appearance = mode.toggled.appearance },
                onOpenRail: { showsDrawer = true }
            )
        case .tools:
            WorkspaceHubScreen(
                session: session,
                onOpenSession: reopenWorkbench,
                onOpenCommand: { showsCommand = true },
                onToggleTheme: { settings.appearance = mode.toggled.appearance },
                onOpenDrawer: { showsDrawer = true },
                onSelectHost: { showsHosts = true },
                onOpenSettings: { navigation.select(.settings) }
            )
        case .settings:
            SettingsScreen(settings: settings, store: store, onBack: { navigation.select(.terminals) })
        }
    }

    /// iPad centre pane while the workbench is closed. Back only parks the
    /// surface, so the placeholder keeps the way back into it.
    private var iPadTerminalEmptyState: some View {
        VStack(alignment: .leading, spacing: 12) {
            SeDisplayText(text: String(localized: "Terminals"))
            Text("Pick a terminal from the sidebar.")
                .font(SeNocturneType.font(11.5))
                .foregroundStyle(palette.inkSecondary)
            if let request = navigation.openRequest {
                Button(action: reopenWorkbench) {
                    HStack(spacing: 10) {
                        SeIconView(icon: .terminal, size: 15, stroke: 1.6, color: palette.ink)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(String(localized: "Resume"))
                                .font(SeNocturneType.font(12, weight: .semibold))
                                .foregroundStyle(palette.ink)
                            Text(request.title)
                                .font(SeNocturneType.font(10, mono: true))
                                .foregroundStyle(palette.inkSecondary)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 0)
                        SeIconView(icon: .arrowRight, size: 13, stroke: 1.6, color: palette.inkTertiary)
                    }
                    .padding(.horizontal, 14)
                    .frame(minHeight: 52)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(Text(String(localized: "Resume")))
                .accessibilityValue(Text(request.title))
            }
            Spacer(minLength: 0)
        }
        .padding(NocturneToolMetrics.gutter)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(palette.canvas)
    }

    // MARK: - Workbench

    private func workbench(sidebar: Bool) -> some View {
        TerminalWorkbenchScreen(
            session: session,
            model: terminalModel,
            isActive: workbenchVisible,
            onBack: closeWorkbench,
            onExternalAction: { action in
                handleExternalAction(action, sidebar: sidebar)
            },
            onOpenPreview: { path in
                presentPreview(path)
            },
            hasAgentInspector: sidebar,
            onShowAgentInspector: { inspectorSegment = .agent }
        )
    }

    private func inspector(root: String?) -> some View {
        Group {
            if let root, !root.isEmpty {
                TerminalInspector(
                    session: session,
                    rootPath: root,
                    terminalTitle: terminalModel.activeTitle,
                    agentAvailable: terminalModel.agentReady,
                    segment: $inspectorSegment,
                    onOpenFile: { presentPreview($0) },
                    agentContent: AnyView(agentInspector)
                )
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    SeEyebrow(text: "Inspector")
                    NocturneEmptyState(
                        systemImage: "terminal",
                        title: String(localized: "No live terminal"),
                        message: String(localized: "Open a terminal to browse its files.")
                    )
                    Spacer(minLength: 0)
                }
                .padding(NocturneToolMetrics.gutter)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            }
        }
        .background(palette.canvas)
        .overlay(alignment: .leading) {
            Rectangle().fill(mode.hairline).frame(width: 1)
        }
    }

    @ViewBuilder
    private var agentInspector: some View {
        if let terminalID = terminalModel.activeID {
            AgentChatPanel(
                session: session,
                terminalID: terminalID,
                hookStore: terminalModel.hookStore
            )
        } else {
            NocturneEmptyState(
                systemImage: "bubble.left.and.bubble.right",
                title: String(localized: "No agent"),
                message: String(localized: "Open a terminal with an agent to chat.")
            )
            .padding(NocturneToolMetrics.gutter)
        }
    }

    // MARK: - Phone chrome

    private var bottomBar: some View {
        SeBottomBar(
            selection: navigation.destination,
            onTerminals: { navigation.select(.terminals) },
            onConversations: { navigation.select(.conversations) },
            onTools: { navigation.select(.tools) }
        )
    }

    private var phoneDrawer: some View {
        ZStack(alignment: .leading) {
            Color.black.opacity(mode.isNight ? 0.5 : 0.22)
                .ignoresSafeArea()
                .onTapGesture { showsDrawer = false }
            AdaptiveSidebarShell(
                selection: Binding(
                    get: { navigation.destination },
                    set: { navigation.select($0) }
                ),
                collapsed: .constant(false),
                hostName: hostName,
                hostOnline: session.phase == .connected,
                counts: [.conversations: "\(conversationEntries.count)"],
                version: ShellPresentation.appVersion,
                onOpenCommand: {
                    showsDrawer = false
                    showsCommand = true
                },
                terminalGroups: railGroups,
                selectedTerminalID: terminalModel.activeID,
                onOpenTerminal: { row in
                    showsDrawer = false
                    Task { await open(TerminalLandingModel.request(for: row)) }
                },
                expandedWidth: 288,
                onClose: { showsDrawer = false }
            )
            .transition(.move(edge: .leading).combined(with: .opacity))
        }
        .zIndex(30)
        .animation(reduceMotion ? nil : SeNocturneMotion.sheet, value: showsDrawer)
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
                            if saved.id == link.id { Text("Current").font(SeNocturneType.font(9, mono: true)) }
                        }
                        .frame(minHeight: 44)
                    }
                    .listRowBackground(palette.card)
                }
            }
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .navigationTitle(String(localized: "Host"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { showsHosts = false }
                }
            }
        }
        .presentationDetents([.medium])
    }

    // MARK: - Files and attachments

    private var filesSheet: some View {
        Group {
            if let root = fileRoot, !root.isEmpty {
                TerminalFilePanel(
                    session: session,
                    rootPath: root,
                    terminalTitle: terminalModel.activeTitle,
                    agentAvailable: terminalModel.agentReady,
                    onOpenFile: { path in presentPreview(path) },
                    onClose: { showsFiles = false }
                )
            } else {
                VStack(spacing: 10) {
                    Text("No live terminal")
                        .font(SeNocturneType.headline)
                        .foregroundStyle(palette.ink)
                    Button("Close") { showsFiles = false }
                        .frame(minHeight: SeSpace.hitTarget)
                }
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(palette.canvas)
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.hidden)
    }

    private var attachmentSheet: some View {
        AttachmentSheet(
            session: session,
            terminalID: terminalModel.activeID ?? "",
            terminalTitle: terminalModel.activeTitle,
            agentReady: terminalModel.agentReady,
            onWriteTerminal: { text in
                terminalModel.canvasHost?.sendCanvasInput(text)
            },
            onDraftAgent: { terminalID, text in
                session.offerAgentDraft(text, terminalID: terminalID)
            },
            onClose: { showsAttachment = false }
        )
    }

    private var chatCover: some View {
        ZStack(alignment: .topTrailing) {
            ChatView(session: session, embedded: false)
                .background(palette.canvas.ignoresSafeArea())
            Button {
                showsChatCover = false
            } label: {
                SeIconView(icon: .close, size: 13, stroke: 1.7, color: palette.inkSecondary)
                    .frame(width: 32, height: 32)
                    .background(.ultraThinMaterial, in: Circle())
                    .overlay { Circle().strokeBorder(palette.stroke, lineWidth: 1) }
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.trailing, 8)
            .padding(.top, 8)
            .accessibilityLabel(String(localized: "Close"))
        }
        .background(palette.canvas.ignoresSafeArea())
    }

    // MARK: - Bootstrap and routing

    private func bootstrap() {
        guard !didBootstrap else { return }
        didBootstrap = true
        hub.attach(socket: session.terminalSocket) {
            try await session.terminals.ensureConnected()
        }
        Task {
            await session.ensureTerminalTransport()
            await refreshHub()
            await session.quickTerminals.refresh()
            if let request = await resumeRequest() {
                await open(request)
            } else {
                navigation.connected(last: nil)
            }
            await consumePendingRoute()
            startHubPolling()
        }
    }

    /// 15 s cadence for the rail and T1; the first tick is skipped because the
    /// bootstrap already refreshed. Cancelled with the shell.
    private func startHubPolling() {
        hubRefreshTask?.cancel()
        hubRefreshTask = Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                guard !Task.isCancelled else { return }
                await refreshHub()
            }
        }
    }

    /// Refresh everything T1 and the rail read. Quick records go first: their
    /// live shell ids are the exclusion set that keeps those shells out of the
    /// project groups, and a shell spawned on the host or by `open` must be
    /// picked up by the same poll (and by the manual refresh button).
    private func refreshHub() async {
        guard session.phase == .connected else { return }
        refreshingHub = true
        await session.quickTerminals.refresh()
        await hub.refresh(projects: session.projects.projects)
        refreshingHub = false
    }

    /// Reopen the terminal that was open when the app last left this host. A
    /// terminal the host no longer reports lands on T1 with a notice instead of
    /// opening an empty surface.
    private func resumeRequest() async -> TerminalOpenRequest? {
        guard let saved = TerminalLastSeen.load(origin: session.origin) else { return nil }
        switch saved.attach {
        case .workspace(let terminalID):
            guard let summary = hub.summary(for: terminalID) else {
                TerminalLastSeen.clear(origin: session.origin)
                landingNotice = String(localized: "The terminal you left is no longer running.")
                return nil
            }
            return TerminalLandingModel.request(for: summary, projects: session.projects.projects)
        case .standalone(let terminalID, _):
            guard let record = session.quickTerminals.records.first(where: {
                QuickTerminalPresentation.liveTerminalIds($0).contains(terminalID)
            }) else {
                TerminalLastSeen.clear(origin: session.origin)
                return nil
            }
            guard let opened = try? await session.quickTerminals.open(id: record.id, cols: 80, rows: 24) else {
                return nil
            }
            return TerminalOpenRequest(
                attach: .standalone(
                    terminalID: opened.terminalId,
                    claim: QuickTerminalPresentation.attachClaim(opened)
                ),
                title: QuickTerminalPresentation.title(record),
                cwd: opened.record.cwd
            )
        }
    }

    /// A deep link or notification asked for one terminal. The host list is the
    /// source of truth: a missing terminal lands on T1 with a notice.
    private func consumePendingRoute() async {
        guard let route = store.takePendingTerminalRoute() else { return }
        await refreshHub()
        guard let summary = hub.summary(for: route.terminalID) else {
            landingNotice = String(localized: "That terminal is not running on this host.")
            showsChatCover = false
            preview = nil
            navigation.back()
            return
        }
        landingNotice = nil
        await open(TerminalLandingModel.request(for: summary, projects: session.projects.projects))
    }

    /// Show a terminal: scope the session to its project or conversation first
    /// so `TerminalStore` lists the PTY, then open the mounted surface.
    private func open(_ request: TerminalOpenRequest, presentingAgent: Bool = false) async {
        preview = nil
        // A tap is the first request after a resume on a phone. Verify the
        // socket that claims to be connected before the watch rides it, so a
        // stale-path failure never becomes the user-visible result of the tap.
        await session.ensureTerminalTransport()
        await scopeSession(for: request)
        navigation.open(request)
        await terminalModel.open(request)
        TerminalLastSeen.save(request, origin: session.origin)
        session.terminalSurfaceVisible = true
        if presentingAgent { terminalModel.present(.agent) }
    }

    private func scopeSession(for request: TerminalOpenRequest) async {
        guard case let .workspace(terminalID) = request.attach else { return }
        if let projectID = request.projectID,
           session.currentTerminalProjectID != projectID,
           let project = session.projects.projects.first(where: { $0.id == projectID }) {
            await session.selectProject(project)
            return
        }
        if session.workspace == .home,
           let conversationID = hub.summary(for: terminalID)?.conversationId,
           let conversation = session.conversations.conversations.first(where: { $0.id == conversationID }) {
            await session.selectConversation(conversation)
            return
        }
        if session.workspace != .home {
            await session.refreshActiveTerminals()
        }
    }

    private func closeWorkbench() {
        navigation.back()
        session.terminalSurfaceVisible = false
        preview = nil
    }

    private func reopenWorkbench() {
        if let request = navigation.openRequest {
            Task { await open(request) }
        } else {
            navigation.back()
        }
    }

    private func presentPreview(_ path: String) {
        let request = FilePreviewRequest(path: path, terminalID: terminalModel.activeID)
        showsFiles = false
        // Let the panel finish dismissing before the cover presents; stacking
        // both in one run loop drops the cover on iOS.
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(160))
            preview = request
        }
    }

    private func handleExternalAction(_ action: TerminalPanelAction, sidebar: Bool) {
        switch action {
        case .files:
            if sidebar {
                inspectorSegment = .files
            } else {
                showsFiles = true
            }
        case .attach:
            showsAttachment = true
        case .desktop, .agent, .more:
            break
        }
    }

    // MARK: - Conversations and needs

    private func openConversation(_ conversation: HostConversation) {
        guard !openingSession else { return }
        let destination = ConversationOpenPolicy.destination(
            conversation: conversation,
            terminals: hub.summaries,
            projects: session.projects.projects
        )
        openingSession = true
        selectionTask = Task {
            defer { openingSession = false; selectionTask = nil }
            await session.selectConversation(conversation)
            guard !Task.isCancelled else { return }
            switch destination {
            case .terminal(let request):
                await open(request, presentingAgent: true)
            case .chat:
                showsChatCover = true
            }
        }
    }

    /// ⌘K conversation row. The palette carries the id, the shell resolves it
    /// against the list it already shows.
    private func openConversation(id: String) {
        guard let conversation = session.conversations.conversations.first(where: { $0.id == id }) else {
            landingNotice = String(localized: "That conversation is no longer available.")
            return
        }
        openConversation(conversation)
    }

    /// ⌘K project row: land on T1 with the project scoped. A project that
    /// already has a live terminal opens that terminal instead of an empty list.
    private func openProject(_ projectID: String) {
        guard let project = session.projects.projects.first(where: { $0.id == projectID }) else {
            landingNotice = String(localized: "That project is no longer available.")
            return
        }
        if let summary = hub.summaries.first(where: { $0.projectId == projectID }) {
            Task { await open(TerminalLandingModel.request(for: summary, projects: session.projects.projects)) }
            return
        }
        leaveWorkbench()
        Task {
            await session.selectProject(project)
            await refreshHub()
        }
    }

    /// Return to T1 without closing anything: the surface only parks its lease.
    private func leaveWorkbench() {
        closeWorkbench()
        Task { await terminalModel.back() }
    }

    private func openNeed(_ item: NeedsItem) {
        showsNeeds = false
        if let request = navigation.openRequest {
            Task {
                await open(request)
                if item.kind == .permission || item.kind == .question {
                    terminalModel.present(.agent)
                }
            }
        } else if session.workspace == .conversation, session.conversations.active != nil {
            showsChatCover = true
        }
    }
}
