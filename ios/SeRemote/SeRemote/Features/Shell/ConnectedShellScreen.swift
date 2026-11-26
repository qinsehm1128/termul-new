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
    @State private var inspectorOpenOverride: Bool?
    @State private var railCollapsed: Bool?
    @State private var workbenchRailVisibleOverride: Bool?
    @State private var openingSession = false
    @State private var selectionTask: Task<Void, Never>?
    @State private var landingNotice: String?
    @State private var refreshingHub = false
    @State private var hubRefreshTask: Task<Void, Never>?
    @State private var hubQuery = TerminalHubIndex.Query()
    @State private var switcherQuery = TerminalHubIndex.Query()
    @State private var showsSwitcher = false
    @State private var newTerminalRoute: TerminalsHubScreen.NewTerminalRoute?
    @State private var didBootstrap = false
    /// The tab the user left when opening Settings, so Back returns there
    /// instead of always dropping them on Terminals.
    @State private var previousDestination: SidebarDestination = .terminals

    init(session: WorkspaceSession, store: ConnectionStore, link: RemoteLink) {
        self.session = session
        self.store = store
        self.link = link
        _terminalModel = State(initialValue: TerminalWorkbenchModel(session: session))
    }

    private var hostName: String { link.title }
    private var workbenchVisible: Bool { navigation.workbenchVisible }
    private var themeIcon: SeIcon { mode == .night ? .moon : .sun }

    /// Settings' Back target: the last real tab the user was on, never
    /// Settings itself, falling back to Terminals.
    static func settingsBackTarget(from remembered: SidebarDestination) -> SidebarDestination {
        SidebarDestination.primary.contains(remembered) ? remembered : .terminals
    }

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

    /// The hub, the in-terminal switcher and the rail read one merged index,
    /// so every surface lists the same shells (quick-owned ones included).
    private func hubIndex(query: TerminalHubIndex.Query = .init()) -> TerminalHubIndex {
        TerminalHubIndex.make(
            summaries: hub.summaries,
            projects: session.projects.projects,
            projectGroups: session.projects.groups,
            quickRecords: session.quickTerminals.records,
            agentTerminalIDs: Set(session.liveAgents.sessions.map(\.terminalId)),
            lastTerminalID: terminalModel.activeID,
            allowsCreate: link.allows("terminal.create"),
            query: query
        )
    }

    /// The rail keeps its compact project grouping, fed by the shared index.
    private var railGroups: [TerminalLandingModel.Group] {
        hubIndex().groups.flatMap(\.projects).map { project in
            TerminalLandingModel.Group(
                id: project.id,
                name: project.name,
                rows: project.shells.map { shell in
                    TerminalLandingModel.Row(
                        id: shell.id,
                        title: shell.title,
                        cwd: shell.cwd,
                        branch: shell.branch,
                        projectID: shell.projectID,
                        projectName: shell.projectName,
                        hasAgent: shell.hasAgent
                    )
                }
            )
        }
    }

    /// Open a shell listed by the rail, the hub or the switcher. Quick-owned
    /// shells attach through their quick terminal so the claim matches.
    private func openListed(terminalID: String) {
        guard let shell = hubIndex().shells.first(where: { $0.id == terminalID }) else { return }
        Task {
            do {
                await open(try await session.openRequest(for: shell))
            } catch {
                landingNotice = error.localizedDescription
                HostLog.session.error("Opening listed terminal failed")
            }
        }
    }

    private var fileRoot: String? {
        terminalModel.activeCwd ?? session.activeToolRoot ?? session.workspaceRoot
    }

    var body: some View {
        GeometryReader { geometry in
            let sidebar = ShellPresentation.usesSidebar(isRegular: sizeClass == .regular, width: geometry.size.width)
            let inlineInspector = ShellPresentation.usesInlineInspector(isRegular: sizeClass == .regular, width: geometry.size.width)
            let portrait = geometry.size.width <= geometry.size.height
            let defaultRailVisible = ShellPresentation.defaultWorkbenchRailVisible(
                width: geometry.size.width,
                isRegular: sizeClass == .regular
            )
            let showsWorkbenchRail = sidebar && (workbenchRailVisibleOverride ?? defaultRailVisible)
            let collapsed = railCollapsed ?? ShellPresentation.defaultRailCollapsed(
                width: geometry.size.width, height: geometry.size.height)
            let railWidth = showsWorkbenchRail
                ? (collapsed ? ShellPresentation.railCollapsedWidth : ShellPresentation.railExpandedWidth)
                : 0
            let defaultInspectorOpen = ShellPresentation.defaultInspectorOpen(
                width: geometry.size.width,
                height: geometry.size.height,
                isRegular: sizeClass == .regular
            )
            let idealInspectorWidth = ShellPresentation.inspectorColumnWidth(
                availableWidth: geometry.size.width,
                railWidth: railWidth
            )
            let inspectorOpen = inlineInspector && workbenchVisible && (inspectorOpenOverride ?? defaultInspectorOpen)
            let inspectorBinding = Binding<Bool>(
                get: { inspectorOpen },
                set: { newValue in
                    inspectorOpenOverride = newValue
                    HostLog.ui.info("Inspector toggled open=\(newValue, privacy: .public)")
                }
            )

            ZStack(alignment: .leading) {
                nativeHubTabs(sidebar: sidebar)
                    .opacity(workbenchVisible ? 0 : 1)
                    .allowsHitTesting(!workbenchVisible)
                    .accessibilityHidden(workbenchVisible)

                // Deliberately outside TabView and every layout conditional so
                // switching tabs or toggling the inspector never reconstructs
                // the mounted TerminalScreen (I1) or ends a host PTY (I5).
                // Wrapping the workbench in a stable NavigationStack lets iOS 27.1+
                // host terminal actions in iPhone Duo's trailing vertical bar,
                // while `.inspector` sits outside the leading rail so it never
                // compresses AdaptiveSidebarShell.
                ZStack(alignment: .leading) {
                    NavigationStack {
                        workbench(
                            inlineInspector: inlineInspector,
                            isInspectorOpen: inspectorOpen,
                            onToggleInspector: {
                                toggleInspector(
                                    currentlyOpen: inspectorOpen,
                                    requested: .files,
                                    inlineInspector: inlineInspector
                                )
                            },
                            showsRailToggle: sidebar,
                            isRailVisible: showsWorkbenchRail,
                            onToggleRail: {
                                workbenchRailVisibleOverride = !showsWorkbenchRail
                            }
                        )
                    }
                    .tint(palette.accent)
                    .duoBackgroundExtension()
                    .inspector(isPresented: inspectorBinding) {
                        inspector(root: fileRoot) {
                            inspectorOpenOverride = false
                            HostLog.ui.info("Inspector collapsed from header")
                        }
                        .inspectorColumnWidth(
                            min: ShellPresentation.inspectorMinWidth,
                            ideal: idealInspectorWidth,
                            max: ShellPresentation.inspectorMaxWidth
                        )
                    }
                    .padding(.leading, railWidth)

                    if showsWorkbenchRail {
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
                            onOpenTerminal: { row in openListed(terminalID: row.id) }
                        )
                        .transition(.move(edge: .leading).combined(with: .opacity))
                    }
                }
                .opacity(workbenchVisible ? 1 : 0)
                .allowsHitTesting(workbenchVisible)
                .accessibilityHidden(!workbenchVisible)

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
            .onChange(of: portrait) { _, _ in
                railCollapsed = nil
                workbenchRailVisibleOverride = nil
                inspectorOpenOverride = ShellPresentation.inspectorOverrideAfterOrientationChange(
                    currentOverride: inspectorOpenOverride,
                    newDefaultOpen: defaultInspectorOpen
                )
            }
            .onChange(of: inlineInspector) { _, inline in
                if !inline {
                    inspectorOpenOverride = nil
                }
            }
            .onReceive(ShortcutCenter.shortcuts) { shortcut in
                switch shortcut {
                case .toggleFiles:
                    guard workbenchVisible else { return }
                    toggleInspector(
                        currentlyOpen: inspectorOpen,
                        requested: .files,
                        inlineInspector: inlineInspector
                    )
                case .focusChat:
                    if workbenchVisible && inlineInspector && terminalModel.agentReady {
                        inspectorSegment = .agent
                        inspectorOpenOverride = true
                    } else {
                        navigation.select(.conversations)
                    }
                case .focusTerminal:
                    if navigation.destination != .terminals {
                        navigation.select(.terminals)
                    } else if !workbenchVisible, navigation.openRequest != nil {
                        reopenWorkbench()
                    }
                default:
                    break
                }
            }
        }
        .background(palette.canvas.ignoresSafeArea())
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
        .onChange(of: session.connectionEpoch) { _, epoch in
            // Epoch 1 is the initial connect handled by `bootstrap()`; every
            // later epoch means the link recovered from an offline/reconnect
            // round-trip and must refresh T1 + recover any mounted surface.
            guard didBootstrap, epoch > 1, session.phase == .connected else { return }
            Task { await recoverShellAfterReconnect() }
        }
        .onChange(of: store.pendingTerminalRoute) { _, route in
            guard route != nil, session.phase == .connected else { return }
            Task { await consumePendingRoute() }
        }
        .onChange(of: navigation.destination) { _, destination in
            showsDrawer = false
            // Remember the last real tab: Settings' Back returns there instead
            // of always dropping the user on Terminals.
            if destination != .settings {
                previousDestination = destination
            }
        }
        .sheet(isPresented: $showsNeeds) {
            NeedsSheetView(items: needs, onOpen: openNeed)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.hidden)
        }
        .sheet(isPresented: $showsHosts) { hostPicker }
        .sheet(isPresented: $showsSwitcher) { switcherSheet }
        .sheet(item: $newTerminalRoute) { route in
            NewTerminalSheet(
                session: session,
                allowsCreate: link.allows("terminal.create"),
                preselectedProjectID: route.projectID,
                onOpen: { request in
                    newTerminalRoute = nil
                    Task {
                        await open(request)
                        await refreshHub()
                    }
                },
                onClose: { newTerminalRoute = nil }
            )
            .presentationDetents(route.projectID == nil ? [.large, .medium] : [.medium])
            .presentationDragIndicator(.visible)
        }
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
                // A bare spinner over a frozen screen reads as a hang; the pill
                // says the tap was accepted and work is in flight.
                ProgressView()
                    .padding(16)
                    .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                    .accessibilityLabel(String(localized: "Opening terminal"))
            }
        }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: showsCommand)
        .overlay {
            // A notice raised by a deep link or a ⌘K miss while another tab is
            // showing would stay invisible until the user returns to Terminals;
            // surface it here instead, dismissible at the source.
            if let landingNotice,
               navigation.destination != .terminals,
               !showsDrawer, !showsCommand {
                // `self.landingNotice`: the shadowed binding is immutable.
                NocturneNotice(message: landingNotice, isError: false, onDismiss: { self.landingNotice = nil })
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .frame(maxWidth: 680)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                    .padding(.top, 8)
                    .transition(.opacity)
            }
        }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: landingNotice)
    }

    // MARK: - Pages (Native TabView for iPhone Duo vertical bar & iPad sidebar)

    private var destinationBinding: Binding<SidebarDestination> {
        Binding(
            get: { navigation.destination },
            set: { navigation.select($0) }
        )
    }

    private func nativeHubTabs(sidebar: Bool) -> some View {
        TabView(selection: destinationBinding) {
            Tab(
                SidebarDestination.terminals.title,
                systemImage: SidebarDestination.terminals.systemImage,
                value: SidebarDestination.terminals
            ) {
                NavigationStack {
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
                        query: $hubQuery,
                        onRefresh: { await refreshHub() },
                        onSelectHost: { showsHosts = true },
                        onToggleTheme: { settings.appearance = mode.toggled.appearance },
                        onOpenDrawer: {
                            if sidebar { railCollapsed = false } else { showsDrawer = true }
                        },
                        onOpenCommand: { showsCommand = true },
                        onOpen: { request in Task { await open(request) } },
                        onDismissNotice: { landingNotice = nil },
                        usesSystemChrome: true
                    )
                    .toolbar { hubToolbar(showsCompactDrawer: !sidebar) }
                }
            }

            Tab(
                SidebarDestination.conversations.title,
                systemImage: SidebarDestination.conversations.systemImage,
                value: SidebarDestination.conversations
            ) {
                NavigationStack {
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
                        onOpenRail: { showsDrawer = true },
                        usesSystemChrome: true
                    )
                    .toolbar { hubToolbar(showsCompactDrawer: !sidebar) }
                }
            }
            .badge(needSummary.total)

            Tab(
                SidebarDestination.tools.title,
                systemImage: SidebarDestination.tools.systemImage,
                value: SidebarDestination.tools
            ) {
                WorkspaceHubScreen(
                    session: session,
                    onOpenSession: reopenWorkbench,
                    onOpenCommand: { showsCommand = true },
                    onToggleTheme: { settings.appearance = mode.toggled.appearance },
                    onOpenDrawer: { showsDrawer = true },
                    onSelectHost: { showsHosts = true },
                    onOpenSettings: { navigation.select(.settings) },
                    usesSystemChrome: true,
                    showsCompactDrawerButton: !sidebar
                )
            }

            Tab(
                SidebarDestination.settings.title,
                systemImage: SidebarDestination.settings.systemImage,
                value: SidebarDestination.settings
            ) {
                NavigationStack {
                    SettingsScreen(
                        settings: settings,
                        store: store,
                        onBack: {
                            navigation.select(ConnectedShellScreen.settingsBackTarget(from: previousDestination))
                        },
                        usesSystemChrome: true
                    )
                    .toolbar { hubToolbar(showsCompactDrawer: !sidebar) }
                }
            }
        }
        .tabViewStyle(.sidebarAdaptable)
        .duoPreferredTabBarPlacement()
        .tint(palette.accent)
    }

    @ToolbarContentBuilder
    private func hubToolbar(showsCompactDrawer: Bool) -> some ToolbarContent {
        if showsCompactDrawer {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    showsDrawer = true
                } label: {
                    Image(systemName: "sidebar.left")
                }
                .accessibilityLabel(String(localized: "All destinations"))
            }
            .duoVisibilityPriority(.standard)
        }

        ToolbarItem(placement: .topBarLeading) {
            Button {
                showsHosts = true
            } label: {
                Label(hostName, systemImage: "server.rack")
            }
            .accessibilityLabel(String(localized: "Select host"))
        }
        .duoVisibilityPriority(.standard)

        ToolbarItem(placement: DuoToolbarPlacement.pinnedTrailing) {
            Button {
                showsCommand = true
            } label: {
                Image(systemName: "command")
            }
            .accessibilityLabel(String(localized: "Command palette"))
        }
        .duoVisibilityPriority(.high)

        ToolbarItem(placement: .topBarTrailing) {
            Button {
                settings.appearance = mode.toggled.appearance
            } label: {
                Image(systemName: mode == .night ? "moon" : "sun.max")
            }
            .accessibilityLabel(String(localized: "Toggle theme"))
        }
        .duoVisibilityPriority(.low)
    }

    // MARK: - Workbench

    /// The in-terminal switcher: the hub's own list, searchable and filterable
    /// across every project, with the current shell highlighted.
    private var switcherSheet: some View {
        TerminalHubView(
            index: hubIndex(query: switcherQuery),
            query: $switcherQuery,
            selectedTerminalID: terminalModel.activeID,
            isLoading: TerminalsHubScreen.isLoading(state: hub.state, summaryCount: hub.summaries.count),
            actions: TerminalHubActions(
                openShell: { shell in
                    showsSwitcher = false
                    guard shell.id != terminalModel.activeID else { return }
                    openListed(terminalID: shell.id)
                },
                openQuick: { quick in
                    showsSwitcher = false
                    Task {
                        do {
                            await open(try await session.openRequest(forQuick: quick))
                        } catch {
                            landingNotice = error.localizedDescription
                        }
                    }
                },
                newTerminal: {
                    showsSwitcher = false
                    newTerminalRoute = .init(projectID: nil)
                },
                newTerminalInProject: { id in
                    showsSwitcher = false
                    newTerminalRoute = .init(projectID: id)
                },
                renameQuick: { _ in },
                closeQuick: { _ in },
                deleteQuick: { _ in },
                closeShell: { _ in },
                refresh: { await refreshHub() }
            )
        ) {
            HStack {
                Text(verbatim: hubText("Switch terminal"))
                    .font(SeNocturneType.font(17, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Spacer(minLength: 0)
                Button {
                    showsSwitcher = false
                } label: {
                    SeIconView(icon: .close, size: 14, stroke: 1.7, color: palette.inkSecondary)
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(Text(hubText("Close")))
            }
            .padding(.top, 8)
        }
        .background(mode.sheetSurface.ignoresSafeArea())
        .presentationDetents([.large, .medium])
        .presentationDragIndicator(.visible)
    }

    private func workbench(
        inlineInspector: Bool,
        isInspectorOpen: Bool,
        onToggleInspector: @escaping () -> Void,
        showsRailToggle: Bool,
        isRailVisible: Bool,
        onToggleRail: @escaping () -> Void
    ) -> some View {
        TerminalWorkbenchScreen(
            session: session,
            model: terminalModel,
            isActive: workbenchVisible,
            onBack: closeWorkbench,
            onExternalAction: { action in
                handleExternalAction(
                    action,
                    inlineInspector: inlineInspector,
                    inspectorOpen: isInspectorOpen
                )
            },
            onOpenPreview: { path in
                presentPreview(path)
            },
            hasAgentInspector: inlineInspector,
            onShowAgentInspector: {
                inspectorSegment = .agent
                inspectorOpenOverride = true
            },
            onOpenSwitcher: {
                switcherQuery = .init()
                showsSwitcher = true
                Task { await refreshHub() }
            },
            showsInspectorToggle: inlineInspector,
            isInspectorOpen: isInspectorOpen,
            onToggleInspector: onToggleInspector,
            showsRailToggle: showsRailToggle,
            isRailVisible: isRailVisible,
            onToggleRail: onToggleRail,
            usesSystemToolbar: true
        )
    }

    private func inspector(root: String?, onClose: @escaping () -> Void) -> some View {
        Group {
            if let root, !root.isEmpty {
                TerminalInspector(
                    session: session,
                    rootPath: root,
                    terminalTitle: terminalModel.activeTitle,
                    agentAvailable: terminalModel.agentReady,
                    segment: $inspectorSegment,
                    onOpenFile: { presentPreview($0) },
                    onClose: onClose,
                    agentContent: AnyView(agentInspector)
                )
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .center, spacing: 8) {
                        SeEyebrow(text: "Inspector")
                        Spacer(minLength: 0)
                        Button(action: onClose) {
                            Image(systemName: "sidebar.trailing")
                                .font(.system(size: 13, weight: .medium))
                                .foregroundStyle(palette.inkSecondary)
                                .frame(width: 30, height: 30)
                                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                .overlay {
                                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                                        .strokeBorder(mode.insetStroke, lineWidth: 1)
                                }
                                .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(SePressable())
                        .accessibilityLabel(String(localized: "Collapse inspector"))
                        .accessibilityIdentifier("terminal-inspector-close")
                    }
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
                    openListed(terminalID: row.id)
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
        // The close control lives inside ChatView's top row so the status
        // ribbon truncates before it instead of being overlapped.
        ChatView(session: session, embedded: false, onClose: { showsChatCover = false })
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
            if let request = await resumeRequest() {
                await open(request)
            } else {
                navigation.connected(last: nil)
            }
            await consumePendingRoute()
            startHubPolling()
        }
    }

    /// Full shell recovery after an offline/reconnect cycle: refresh the
    /// terminal directory, re-attach/re-watch the open workbench (or resume
    /// `TerminalLastSeen` if the initial open had failed before attaching), and
    /// restart polling if needed.
    private func recoverShellAfterReconnect() async {
        await refreshHub()
        if hubRefreshTask == nil {
            startHubPolling()
        }
        if workbenchVisible {
            await terminalModel.recoverAfterSessionReconnect()
            if terminalModel.activeID == nil, let request = navigation.openRequest {
                await open(request)
            }
        } else if navigation.openRequest == nil, let request = await resumeRequest() {
            await open(request)
        }
        await consumePendingRoute()
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
    /// Only an authoritative host answer (`.loaded` or `.empty`) proves a
    /// previously seen terminal actually exited; a transient `.failed`/`.loading`
    /// refresh must NEVER erase `TerminalLastSeen`.
    nonisolated static func canClearMissingLastSeen(hubState: TerminalsHubStore.State) -> Bool {
        switch hubState {
        case .loaded, .empty:
            return true
        case .idle, .loading, .failed:
            return false
        }
    }

    nonisolated static func canClearMissingQuickLastSeen(quickState: QuickTerminalStore.State) -> Bool {
        switch quickState {
        case .loaded, .empty:
            return true
        case .idle, .loading, .failed:
            return false
        }
    }

    private func resumeRequest() async -> TerminalOpenRequest? {
        guard let saved = TerminalLastSeen.load(origin: session.origin) else { return nil }
        switch saved.attach {
        case .workspace(let terminalID):
            guard let summary = hub.summary(for: terminalID) else {
                if Self.canClearMissingLastSeen(hubState: hub.state) {
                    TerminalLastSeen.clear(origin: session.origin)
                    landingNotice = String(localized: "The terminal you left is no longer running.")
                }
                return nil
            }
            return TerminalLandingModel.request(for: summary, projects: session.projects.projects)
        case .standalone(let terminalID, _):
            guard let record = session.quickTerminals.records.first(where: {
                QuickTerminalPresentation.liveTerminalIds($0).contains(terminalID)
            }) else {
                if Self.canClearMissingQuickLastSeen(quickState: session.quickTerminals.state) {
                    TerminalLastSeen.clear(origin: session.origin)
                }
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

    private func handleExternalAction(
        _ action: TerminalPanelAction,
        inlineInspector: Bool,
        inspectorOpen: Bool
    ) {
        switch action {
        case .files:
            toggleInspector(
                currentlyOpen: inspectorOpen,
                requested: .files,
                inlineInspector: inlineInspector
            )
        case .attach:
            showsAttachment = true
        case .desktop, .agent, .more:
            break
        }
    }

    private func toggleInspector(
        currentlyOpen: Bool,
        requested: TerminalInspectorSegment,
        inlineInspector: Bool
    ) {
        if inlineInspector {
            let next = ShellPresentation.nextInspectorState(
                isOpen: currentlyOpen,
                current: inspectorSegment,
                requested: requested
            )
            inspectorSegment = next.segment
            inspectorOpenOverride = next.isOpen
            HostLog.ui.info("Inspector state open=\(next.isOpen, privacy: .public) segment=\(next.segment.rawValue, privacy: .public)")
        } else if requested == .files {
            showsFiles.toggle()
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
