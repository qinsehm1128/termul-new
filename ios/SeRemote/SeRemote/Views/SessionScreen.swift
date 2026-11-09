import SwiftUI
import UIKit

struct SessionScreen: View {
    @Bindable var session: WorkspaceSession
    var hostDirectory: AnyView? = nil
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var containerWidth: CGFloat = 0
    @State private var filesVisibility: NavigationSplitViewVisibility = .detailOnly
    @State private var showTools = false
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
            VStack(spacing: SeSpace.sm) {
                SessionModeSwitch(session: session)
                SessionStatusRibbon(status: status, detail: modelName)
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.bottom, SeSpace.sm)
            adaptiveContent
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { containerWidth = $0 }
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .navigationBar)
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { notification in
            guard session.workspaceTab == .terminal || sideBySide else { return }
            // Keyboard/PTY resize follows the system, without animating output.
            session.noteTerminalKeyboard(height: KeyboardGuard.overlapHeight(from: notification))
        }
        .sheet(isPresented: $chat.showAgentSheet) { AgentConfigSheet(session: session) }
        .sheet(isPresented: $showTools) { ToolsView(session: session) }
        .task(id: workspaceTaskId) {
            if session.phase == .connected { await session.refreshActiveTerminals() }
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(15)) }
                catch { break }
                if session.phase == .connected { await session.refreshActiveTerminals() }
            }
        }
        .onAppear {
            session.noteWideLayout(sideBySide)
            ShortcutAvailability.shared.sessionActive = true
        }
        .onDisappear { ShortcutAvailability.shared.sessionActive = false }
        .onChange(of: sideBySide) { _, visible in session.noteWideLayout(visible) }
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
                    SeEmptyState(title: String(localized: "Chat"), systemImage: "bubble.left.and.bubble.right",
                                 message: String(localized: "Project view is terminals only. Open a session to talk."))
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

    private var modelName: String? {
        guard let models = session.chat.models,
              let id = models.currentModelId else { return nil }
        return models.availableModels?.first(where: { $0.modelId == id })?.name ?? id
    }

    private var status: NocturneSessionPresentation.Status {
        NocturneSessionPresentation.status(
            phase: session.phase,
            connected: session.workspaceTab == .terminal
                ? session.terminalSocket.state == .connected : session.acp.state == .connected,
            permissions: session.chat.permissions.count,
            questions: session.chat.questions.count,
            streaming: session.chat.messages.contains(where: \.streaming)
        )
    }

    private var workbenchHeader: some View {
        HStack(spacing: SeSpace.sm) {
            Button { dismiss() } label: {
                Image(systemName: "chevron.left")
                    .frame(width: 44, height: 44)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control).stroke(palette.strokeStrong, lineWidth: 1)
                    }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back")
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline).lineLimit(1)
                Text(NocturneSessionPresentation.subtitle(
                    host: session.origin.host ?? String(localized: "Host"),
                    agent: session.workspace == .conversation ? session.chat.agentLabel : nil,
                    model: modelName
                ))
                .font(.caption.monospaced())
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            Menu {
                if session.workspace == .conversation {
                    Button("Agent", systemImage: "cpu") { session.chat.showAgentSheet = true }
                }
                Button("Tools", systemImage: "wrench.and.screwdriver") { showTools = true }
                Button("New terminal", systemImage: "plus") {
                    Task { await session.spawnTerminal() }
                }
            } label: {
                Image(systemName: "ellipsis")
                    .frame(width: 44, height: 44)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control).stroke(palette.strokeStrong, lineWidth: 1)
                    }
            }
            .accessibilityLabel("Workspace actions")
        }
        .padding(SeSpace.md)
    }

    // MARK: Shortcuts

    private func handleShortcut(_ shortcut: ShortcutCenter.Shortcut) {
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

    // MARK: Meta

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

    private var subtitle: String? {
        switch session.workspace {
        case .conversation:
            session.conversations.active?.workspaceCwd
        case .project:
            session.projects.active?.path
        case .home:
            nil
        }
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
