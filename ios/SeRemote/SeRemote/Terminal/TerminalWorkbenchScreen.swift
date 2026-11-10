import SwiftUI

/// One terminal surface for workspace and quick terminals.
///
/// The canvas is the only long-lived subtree; the chrome floats above it and
/// the panel is an overlay, so nothing here resizes or remounts the PTY.
/// `.files` and `.attach` leave through `onExternalAction` for the shell to
/// present; `.desktop`, `.agent` and `.more` are presented here.
struct TerminalWorkbenchScreen: View {
    @Bindable var session: WorkspaceSession
    @Bindable var model: TerminalWorkbenchModel
    var isActive = true
    var onBack: () -> Void
    var onExternalAction: (TerminalPanelAction) -> Void
    /// A tool screen asked for a full-screen file preview. The shell owns the
    /// cover, so the workbench hands the path out after the tool cover closes.
    var onOpenPreview: ((String) -> Void)? = nil

    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette

    @State private var focusToken: UInt64 = 0
    @State private var blurToken: UInt64 = 0
    @State private var textScale = TerminalTextScale.current
    @State private var keyboardHeight: CGFloat = 0
    @State private var controlModifier = TerminalControlModifier()
    @State private var toolCover: ToolRoute?

    private var allowsInput: Bool { deviceLink?.allows("terminal.input") ?? true }
    private var allowsAgentView: Bool { deviceLink?.allows("view") ?? true }

    var body: some View {
        TerminalWorkbenchChrome(
            model: model,
            keyboardVisible: TerminalKeyboard.isVisible(keyboardHeight),
            canvas: canvas,
            onBack: {
                onBack()
                Task { await model.back() }
            },
            onFocusKeyboard: { focusToken &+= 1 },
            onToggleFit: toggleFit,
            onNudgeTextScale: nudgeTextScale,
            onSelectTab: { id in
                Task {
                    await model.select(id)
                    focusToken &+= 1
                }
            },
            onSpawnTerminal: { Task { await spawnTerminal() } }
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.terminalCanvas)
        .terminalKeyboardHeight($keyboardHeight)
        .onAppear(perform: syncCapabilities)
        .onChange(of: allowsInput) { _, _ in syncCapabilities() }
        .onChange(of: allowsAgentView) { _, _ in syncCapabilities() }
        .onChange(of: isActive) { _, active in
            if !active {
                model.panelExpanded = false
                model.dismissSheet()
                blurToken &+= 1
            }
        }
        .sheet(item: sheetRoute) { route in
            sheet(for: route.action)
        }
        .fullScreenCover(item: $toolCover) { route in
            toolScreen(route)
                .background(palette.canvas.ignoresSafeArea())
        }
        .onReceive(ShortcutCenter.shortcuts) { shortcut in
            guard isActive else { return }
            switch shortcut {
            case .textScaleUp: nudgeTextScale(1)
            case .textScaleDown: nudgeTextScale(-1)
            case .textScaleReset: applyTextScale(TerminalTextScale.defaultValue)
            case .focusTerminal:
                if allowsInput { focusToken &+= 1 }
            default:
                break
            }
        }
    }

    // MARK: - Canvas

    @ViewBuilder
    private var canvas: some View {
        if let host = model.canvasHost, let id = host.canvasTerminalID {
            TerminalScreen(
                terminalId: id,
                buffered: host.canvasBufferedOutput,
                hostCols: host.canvasCols,
                lockToHostCols: host.canvasLocksToHostCols,
                textScale: textScale,
                onSend: { text in
                    guard allowsInput else { return }
                    host.sendCanvasInput(controlModifier.apply(to: text))
                },
                onResize: { cols, rows in
                    host.canvasResized(cols: cols, rows: rows)
                },
                onReady: { feed in
                    host.installCanvasFeed(feed)
                },
                onTextScaleChange: { scale, settled in
                    textScale = scale
                    if settled { TerminalTextScale.current = scale }
                },
                focusToken: focusToken,
                blurToken: blurToken,
                inputAccessory: allowsInput ? AnyView(accessoryBar) : nil
            )
            .id(id)
            .transaction { $0.animation = nil }
            .onAppear {
                if !model.isStandalone {
                    session.terminals.pendingOutput[id] = nil
                }
            }
        } else {
            emptyState
        }
    }

    private var accessoryBar: some View {
        TerminalKeyboardAccessoryBar(
            controlModifier: $controlModifier,
            isEnabled: model.canvasHost != nil,
            onSend: { text in
                guard allowsInput, let host = model.canvasHost else { return }
                host.sendCanvasInput(text)
            },
            onAttach: {
                blurToken &+= 1
                model.present(.attach)
            },
            onHide: {
                blurToken &+= 1
                session.dismissTerminalKeyboard()
            }
        )
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            if model.isOpening {
                ProgressView()
                Text("Attaching to the host shell…")
                    .font(SeNocturneType.font(10, mono: true))
                    .foregroundStyle(palette.inkSecondary)
            } else {
                SeIconView(icon: .terminal, size: 22, stroke: 1.6, color: palette.inkTertiary)
                Text("No live terminal")
                    .font(SeNocturneType.headline)
                    .foregroundStyle(palette.ink)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.terminalCanvas)
    }

    // MARK: - Sheets

    private struct SheetRoute: Identifiable {
        let action: TerminalPanelAction
        var id: TerminalPanelAction { action }
    }

    private var sheetRoute: Binding<SheetRoute?> {
        Binding(
            get: { model.sheetAction.map(SheetRoute.init) },
            set: { route in
                if route == nil { model.dismissSheet() }
            }
        )
    }

    @ViewBuilder
    private func sheet(for action: TerminalPanelAction) -> some View {
        switch action {
        case .desktop:
            if let terminalID = model.activeID {
                DesktopQuickSheet(
                    hostName: model.agentName ?? String(localized: "Desktop"),
                    sender: desktopSender,
                    target: DesktopShotTargetResolver.terminalWorkbench(
                        terminalID: terminalID,
                        conversationShowing: model.agentReady,
                        conversationReady: model.agentReady
                    )
                )
            }
        case .agent:
            if let terminalID = model.activeID {
                AgentChatPanel(
                    session: session,
                    terminalID: terminalID,
                    hookStore: model.hookStore
                )
                .presentationDetents([.large])
                .presentationDragIndicator(.hidden)
                .presentationCornerRadius(SeNocturneRadius.sheet)
            }
        case .more:
            TerminalMoreSheet(
                presentation: model.panelPresentation,
                agentReady: model.agentReady,
                onNewTerminal: {
                    model.dismissSheet()
                    Task { await spawnTerminal() }
                },
                onScreenText: { openTool(.screenText) },
                onChanges: { openTool(.git) },
                onSearch: { openTool(.search) },
                onScheduledTasks: { openTool(.scheduledTasks) },
                onAgent: { model.present(.agent) },
                onClose: {
                    Task {
                        await model.closeActiveTerminal()
                        onBack()
                    }
                }
            )
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.hidden)
            .presentationCornerRadius(SeNocturneRadius.sheet)
        case .files, .attach:
            EmptyView()
        }
    }

    private var desktopSender: DesktopShotSender {
        DesktopShotSender(
            http: session.http,
            chat: nil,
            terminals: nil,
            conversation: nil,
            writeTerminal: { text in
                model.canvasHost?.sendCanvasInput(text)
                model.dismissSheet()
            },
            draftAgentChat: { terminalID, text in
                session.offerAgentDraft(text, terminalID: terminalID)
                model.dismissSheet()
            }
        )
    }

    // MARK: - Tools

    private enum ToolRoute: String, Identifiable {
        case git
        case screenText
        case search
        case scheduledTasks

        var id: String { rawValue }
    }

    private func openTool(_ route: ToolRoute) {
        model.dismissSheet()
        // Let the sheet finish dismissing before the cover presents; stacking
        // both in one run loop drops the cover on iOS.
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(180))
            toolCover = route
        }
    }

    @ViewBuilder
    private func toolScreen(_ route: ToolRoute) -> some View {
        ZStack(alignment: .topTrailing) {
            switch route {
            case .git:
                GitFeatureView(session: session)
            case .screenText:
                ScreenFeatureView(session: session)
            case .search:
                SearchFeatureView(session: session, onOpenFile: {
                    let path = session.files.currentPreviewPath
                    toolCover = nil
                    if let path { onOpenPreview?(path) }
                })
            case .scheduledTasks:
                ScheduledTasksFeatureView(session: session)
            }

            Button {
                toolCover = nil
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .frame(width: 32, height: 32)
                    .background(.ultraThinMaterial, in: Circle())
                    .overlay { Circle().strokeBorder(palette.stroke, lineWidth: 1) }
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.trailing, 8)
            .padding(.top, 8)
            .accessibilityLabel(Text("Close"))
        }
        .background(palette.canvas.ignoresSafeArea())
    }

    // MARK: - Actions

    private func syncCapabilities() {
        model.allowsAgentView = allowsAgentView
        model.allowsAgentInput = allowsInput
        model.onExternalAction = onExternalAction
    }

    private func toggleFit() {
        guard !model.isStandalone else { return }
        let next: TerminalDisplayMode = session.terminals.displayMode == .phone ? .desktop : .phone
        Task { await session.terminals.setDisplayMode(next) }
    }

    private func spawnTerminal() async {
        await session.spawnTerminal()
        guard let terminalID = session.terminals.activeId else { return }
        let terminal = session.terminals.terminals.first(where: { $0.id == terminalID })
        await model.open(
            TerminalOpenRequest(
                attach: .workspace(terminalID: terminalID),
                title: terminal?.title ?? String(localized: "Terminal"),
                cwd: terminal?.cwd,
                projectID: terminal?.projectId ?? session.currentTerminalProjectID,
                projectName: session.currentTerminalProjectName
            )
        )
        focusToken &+= 1
    }

    private func nudgeTextScale(_ direction: Int) {
        if direction == 0 {
            applyTextScale(TerminalTextScale.defaultValue)
        } else {
            applyTextScale(TerminalTextScale.nudge(textScale, by: direction))
        }
    }

    private func applyTextScale(_ scale: CGFloat) {
        textScale = scale
        TerminalTextScale.current = scale
    }
}

// MARK: - More sheet

/// 新建终端 / 屏幕文本 / 更改 / 搜索 / 定时任务 / 对话, then a destructive close.
/// The close row states its consequence and never pretends a workspace shell
/// can be killed from the phone.
private struct TerminalMoreSheet: View {
    let presentation: TerminalPanelPresentation
    let agentReady: Bool
    var onNewTerminal: () -> Void
    var onScreenText: () -> Void
    var onChanges: () -> Void
    var onSearch: () -> Void
    var onScheduledTasks: () -> Void
    var onAgent: () -> Void
    var onClose: () -> Void

    @Environment(\.dismiss) private var dismiss
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @State private var confirmClose = false

    var body: some View {
        VStack(spacing: 12) {
            SeSheetHeader(
                title: presentation.title,
                subtitle: presentation.subtitle,
                onClose: { dismiss() }
            )

            SeGroupCard {
                SeRowItem(icon: .plus, title: String(localized: "New Terminal"), action: onNewTerminal) {
                    EmptyView()
                }
                SeRowItem(icon: .textview, title: String(localized: "Screen Text"), action: onScreenText) {
                    EmptyView()
                }
                SeRowItem(icon: .git, title: String(localized: "Changes"), action: onChanges) {
                    EmptyView()
                }
                SeRowItem(icon: .search, title: String(localized: "Search"), action: onSearch) {
                    EmptyView()
                }
                SeRowItem(icon: .clock, title: String(localized: "Scheduled Tasks"), action: onScheduledTasks) {
                    EmptyView()
                }
                SeRowItem(
                    icon: .chat,
                    title: String(localized: "Conversation"),
                    action: agentReady ? { dismiss(); onAgent() } : nil
                ) {
                    if !agentReady {
                        SeInfoTip(text: String(localized: "Start a supported agent in this terminal."))
                    }
                }
            }

            SeGroupCard {
                SeRowItem(
                    icon: .close,
                    title: presentation.close.actionTitle,
                    destructive: true,
                    action: { confirmClose = true }
                ) {
                    EmptyView()
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .padding(.bottom, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(mode.sheetSurface)
        .confirmationDialog(
            presentation.close.confirmTitle,
            isPresented: $confirmClose,
            titleVisibility: .visible
        ) {
            Button(presentation.close.actionTitle, role: presentation.close.isDestructive ? .destructive : nil) {
                dismiss()
                onClose()
            }
            Button(String(localized: "Cancel"), role: .cancel) {}
        } message: {
            Text(presentation.close.consequence)
        }
    }
}
