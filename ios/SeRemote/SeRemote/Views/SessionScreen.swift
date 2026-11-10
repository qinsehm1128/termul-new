import SwiftUI
import UIKit

/// The session workbench (prototype `data-screen="chat|terminal|files"`).
///
/// One navbar and one mode switch frame three identity-stable panes. The
/// SwiftTerm host and the chat/files panes stay mounted across mode switches,
/// rotation and size-class changes; only opacity/offset changes.
struct SessionScreen: View {
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Bindable var session: WorkspaceSession
    var hostDirectory: AnyView? = nil
    var onBack: (() -> Void)? = nil
    var isActive: Bool = true
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var containerWidth: CGFloat = 0
    @State private var filesVisibility: NavigationSplitViewVisibility = .detailOnly
    @State private var showTools = false
    @State private var showDesktopSheet = false
    @State private var tabForward = true

    private var columns: SeLayout.Columns {
        SeLayout.columns(width: containerWidth, isRegular: sizeClass == .regular,
                         largeText: dynamicTypeSize.isAccessibilitySize)
    }
    private var isWide: Bool { columns != .single }
    private var sideBySide: Bool { columns == .splitWithInspector }
    private var showingFiles: Bool {
        session.workspaceTab == .files || (isWide && filesVisibility == .all)
    }

    var body: some View {
        @Bindable var chat = session.chat
        VStack(spacing: 0) {
            workbenchHeader
            SessionModeSwitch(session: session)
                .padding(.horizontal, 18)
                .padding(.top, 12)
            adaptiveContent
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { containerWidth = $0 }
        .background(palette.surface.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .navigationBar)
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { notification in
            guard isActive, session.workspaceTab == .terminal || sideBySide else { return }
            // Keyboard/PTY resize follows the system, without animating output.
            session.noteTerminalKeyboard(height: KeyboardGuard.overlapHeight(from: notification))
        }
        .sheet(isPresented: $chat.showAgentSheet) { AgentConfigSheet(session: session) }
        .sheet(isPresented: $showTools) { ToolsView(session: session) }
        .sheet(isPresented: $showDesktopSheet) {
            DesktopQuickSheet(
                hostName: deviceLink?.title ?? String(localized: "Desktop"),
                sender: DesktopShotSender(
                    http: session.http,
                    chat: session.chat,
                    terminals: session.terminals,
                    conversation: session.conversations.active
                ),
                target: DesktopShotTargetResolver.resolve(
                    activeTerminalID: session.terminals.activeId,
                    activeConversationID: session.conversations.active?.id,
                    agentChatTerminalID: nil,
                    terminalTabActive: session.workspaceTab == .terminal
                ) ?? .chat(conversationID: session.conversations.active?.id)
            )
        }
        .task(id: workspaceTaskId) {
            if session.phase == .connected { await session.refreshActiveTerminals() }
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(15)) }
                catch { break }
                if session.phase == .connected { await session.refreshActiveTerminals() }
            }
        }
        .onAppear {
            if isActive { session.noteWideLayout(sideBySide) }
            ShortcutAvailability.shared.sessionActive = isActive
        }
        .onDisappear { ShortcutAvailability.shared.sessionActive = false }
        .onChange(of: sideBySide) { _, visible in
            if isActive { session.noteWideLayout(visible) }
        }
        .onChange(of: isActive) { _, active in
            ShortcutAvailability.shared.sessionActive = active
            if active {
                session.noteWideLayout(sideBySide)
            } else {
                // The screen stays mounted at opacity 0. A terminal that keeps
                // first responder here leaves the keyboard attached to an
                // invisible view (KeyboardCoordinator conversion / snapshot
                // warnings), so hand focus back before the next presentation.
                session.dismissTerminalKeyboard()
            }
        }
        .onChange(of: session.workspaceTab) { old, new in
            tabForward = (WorkspaceTab.allCases.firstIndex(of: new) ?? 0)
                > (WorkspaceTab.allCases.firstIndex(of: old) ?? 0)
        }
        .seHaptic(.selection, trigger: session.workspaceTab)
        .onReceive(ShortcutCenter.shortcuts) { handleShortcut($0) }
    }

    /// AnyLayout changes arrangement, not child identity. File tree, chat,
    /// terminal, drafts and scroll anchors survive tab switches AND folding.
    private var adaptiveContent: some View {
        let layout = isWide ? AnyLayout(HStackLayout(spacing: 0)) : AnyLayout(ZStackLayout())
        return layout {
            ZStack {
                if let hostDirectory {
                    hostDirectory
                        .opacity(isWide && !showingFiles ? 1 : 0)
                        .allowsHitTesting(isWide && !showingFiles)
                        .accessibilityHidden(!isWide || showingFiles)
                }
                FileBrowserView(session: session, embedded: true)
                    .modifier(ShellTabVisibility(visible: showingFiles, forward: tabForward,
                                                 reduceMotion: reduceMotion))
            }
            .frame(width: isWide ? 300 : nil)
            .frame(maxWidth: isWide ? nil : .infinity, maxHeight: .infinity)
            .overlay(alignment: .trailing) {
                if isWide { Rectangle().fill(SeTheme.stroke).frame(width: 1) }
            }
            keepAliveWorkspace
                .modifier(ShellTabVisibility(visible: isWide || !showingFiles, forward: tabForward,
                                             reduceMotion: reduceMotion))
        }
    }

    private var keepAliveWorkspace: some View {
        let layout = sideBySide ? AnyLayout(HStackLayout(spacing: 0)) : AnyLayout(ZStackLayout())
        let chatVisible = sideBySide || session.workspaceTab == .chat
            || (isWide && session.workspaceTab == .files)
        let terminalVisible = sideBySide || session.workspaceTab == .terminal
        return layout {
            Group {
                if session.workspace == .project {
                    SeEmptyState(title: String(localized: "Terminals only"),
                                 systemImage: "bubble.left.and.bubble.right",
                                 message: String(localized: "Open an agent session to chat."))
                } else {
                    SessionChatView(session: session)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .modifier(ShellTabVisibility(visible: chatVisible, forward: tabForward,
                                         reduceMotion: reduceMotion))
            VStack(spacing: 0) {
                TerminalTabStrip(session: session)
                TerminalWorkspaceView(session: session)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .modifier(ShellTabVisibility(visible: terminalVisible, forward: tabForward,
                                         reduceMotion: reduceMotion))
        }
    }

    // MARK: - Navbar (prototype `.navbar`)

    private var workbenchHeader: some View {
        HStack(spacing: 10) {
            Button {
                if let onBack { onBack() } else { dismiss() }
            } label: {
                SeIconView(icon: .back, size: 15, stroke: 1.8, color: palette.inkSecondary)
                    .frame(width: 32, height: 32)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .stroke(palette.stroke, lineWidth: 1)
                    }
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Back"))

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(SeNocturneType.font(15, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(headerSubtitle)
                    .font(SeNocturneType.font(10, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)

            if session.desktopSupported != false {
                Button {
                    showDesktopSheet = true
                } label: {
                    SeIconView(icon: .desktop, size: 15, stroke: 1.7, color: palette.inkSecondary)
                        .frame(width: 32, height: 32)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 9, style: .continuous)
                                .stroke(palette.stroke, lineWidth: 1)
                        }
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("Desktop"))
                .accessibilityHint(Text("Opens the remote desktop."))
            }

            Menu {
                if session.workspace == .conversation {
                    Button("Agent") { session.chat.showAgentSheet = true }
                        .disabled(!(deviceLink?.allows("agent.run") ?? true))
                }
                Button("Tools") { showTools = true }
                Button("New terminal") {
                    Task { await session.spawnTerminal() }
                }
                .disabled(!(deviceLink?.allows("terminal.create") ?? true))
            } label: {
                SeIconView(icon: .more, size: 15, stroke: 1.8, color: palette.inkSecondary)
                    .frame(width: 32, height: 32)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .stroke(palette.stroke, lineWidth: 1)
                    }
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(Text("Workspace actions"))
        }
        .padding(.horizontal, 18)
        .padding(.top, 8)
    }

    // MARK: - Shortcuts

    private func handleShortcut(_ shortcut: ShortcutCenter.Shortcut) {
        guard NocturneSessionPresentation.acceptsShortcut(isActive: isActive, enabled: ShortcutAvailability.shared.sessionActive) else { return }
        switch shortcut {
        case .focusChat:
            session.setWorkspaceTab(.chat)
        case .focusTerminal:
            session.setWorkspaceTab(.terminal)
        case .toggleFiles:
            if isWide {
                if showingFiles {
                    filesVisibility = .detailOnly
                    if session.workspaceTab == .files {
                        session.setWorkspaceTab(session.workspace == .project ? .terminal : .chat)
                    }
                } else {
                    filesVisibility = .all
                }
            } else {
                session.setWorkspaceTab(.files)
            }
        case .newTerminal:
            guard (deviceLink?.allows("terminal.create") ?? true) else { return }
            Task { await session.spawnTerminal() }
        case .nextTerminal:
            cycleTerminal(forward: true)
        case .previousTerminal:
            cycleTerminal(forward: false)
        case .textScaleUp, .textScaleDown, .textScaleReset:
            break  // TerminalWorkspaceView owns text scale.
        }
    }

    private func cycleTerminal(forward: Bool) {
        let list = session.terminals.terminals
        guard !list.isEmpty else { return }
        let index: Int
        if let activeId = session.terminals.activeId,
           let current = list.firstIndex(where: { $0.id == activeId }) {
            index = (current + (forward ? 1 : list.count - 1)) % list.count
        } else {
            index = 0
        }
        Task { await session.revealTerminal(list[index].id) }
    }

    // MARK: - Meta

    private var title: String {
        switch session.workspace {
        case .conversation:
            session.conversations.active?.displayTitle ?? String(localized: "Session")
        case .project:
            session.projects.active?.name ?? String(localized: "Project")
        case .home:
            String(localized: "Workspace")
        }
    }

    private var headerSubtitle: String {
        let host = session.origin.host ?? String(localized: "Host")
        switch session.workspaceTab {
        case .chat:
            return NocturneSessionPresentation.subtitle(
                host: host,
                agent: session.chat.agentLabel,
                model: modelName
            )
        case .terminal:
            let terminal = session.terminals.terminals.first(where: { $0.id == session.terminals.activeId })
            let folder = terminal?.cwd.map { URL(fileURLWithPath: $0).lastPathComponent }
                ?? session.workspaceRoot.map { URL(fileURLWithPath: $0).lastPathComponent }
            return NocturneSessionPresentation.subtitle(
                host: host,
                agent: folder,
                model: terminal?.gitBranch
            )
        case .files:
            let folder = session.files.crumbs.last?.name
                ?? session.workspaceRoot.map { URL(fileURLWithPath: $0).lastPathComponent }
            return NocturneSessionPresentation.subtitle(
                host: host,
                agent: folder,
                model: String(localized: "Read only")
            )
        }
    }

    private var modelName: String? {
        guard let models = session.chat.models,
              let id = models.currentModelId else { return nil }
        return models.availableModels?.first(where: { $0.modelId == id })?.name ?? id
    }

    private var workspaceTaskId: String {
        switch session.workspace {
        case .conversation:
            "conversation:\(session.conversations.active?.id ?? "")"
        case .project:
            "project:\(session.projects.active?.id ?? "")"
        case .home:
            "home"
        }
    }
}

/// Stable panes use the same fade/travel as SeTransition.tabContent without
/// removing a hidden terminal. The transition also handles first insertion;
/// explicit opacity is what makes subsequent keep-alive swaps animate.
private struct ShellTabVisibility: ViewModifier {
    let visible: Bool
    let forward: Bool
    let reduceMotion: Bool

    func body(content: Content) -> some View {
        content
            .opacity(visible ? 1 : 0)
            .offset(x: visible || reduceMotion ? 0 : (forward ? -SeMotion.entranceTravel : SeMotion.entranceTravel))
            .allowsHitTesting(visible)
            .accessibilityHidden(!visible)
            .transition(SeTransition.tabContent(reduceMotion: reduceMotion, forward: forward))
            .animation(SeMotion.fade.animation(reduceMotion: reduceMotion), value: visible)
    }
}
