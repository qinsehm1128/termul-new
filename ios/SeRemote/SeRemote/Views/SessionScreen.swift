import SwiftUI
import UIKit

struct SessionScreen: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var filesVisibility: NavigationSplitViewVisibility = .automatic

    /// Plus/Max iPhones report .regular in landscape; only actual iPads get
    /// the split tree, so the phone chrome survives rotation.
    private var isWide: Bool { sizeClass == .regular && UIDevice.current.userInterfaceIdiom == .pad }

    var body: some View {
        @Bindable var chat = session.chat
        Group {
            if isWide {
                wideLayout
            } else {
                compactLayout
            }
        }
        .background(SeTheme.canvas.ignoresSafeArea())
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { notification in
            guard session.workspaceTab == .terminal || isWide else { return }
            let duration = (notification.userInfo?[UIResponder.keyboardAnimationDurationUserInfoKey] as? Double) ?? 0.25
            withAnimation(.easeOut(duration: duration)) {
                session.noteTerminalKeyboard(height: KeyboardGuard.overlapHeight(from: notification))
            }
        }
        .sheet(isPresented: $chat.showAgentSheet) {
            AgentConfigSheet(session: session)
        }
        .task(id: workspaceTaskId) {
            await session.refreshActiveTerminals()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                await session.refreshActiveTerminals()
            }
        }
        .onAppear {
            session.noteWideLayout(isWide)
            ShortcutAvailability.shared.sessionActive = true
        }
        .onDisappear {
            ShortcutAvailability.shared.sessionActive = false
        }
        .onChange(of: isWide) { _, wide in
            session.noteWideLayout(wide)
        }
        .onReceive(ShortcutCenter.shortcuts) { shortcut in
            handleShortcut(shortcut)
        }
    }

    // MARK: Wide (iPad) layout — Files sidebar + terminal/chat split

    private var wideLayout: some View {
        NavigationSplitView(columnVisibility: $filesVisibility) {
            FileBrowserView(session: session, embedded: true)
                .navigationSplitViewColumnWidth(min: 240, ideal: 300, max: 380)
                .navigationTitle(WorkspaceTab.files.title)
                .navigationBarTitleDisplayMode(.inline)
        } detail: {
            wideDetail
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button {
                            session.leaveWorkspace()
                        } label: {
                            Image(systemName: "chevron.backward")
                        }
                        .accessibilityLabel(Text("Back"))
                    }
                    ToolbarItemGroup(placement: .primaryAction) {
                        if session.workspace == .conversation {
                            Button {
                                session.chat.showAgentSheet = true
                            } label: {
                                Image(systemName: "cpu")
                            }
                            .accessibilityLabel(Text("Agent"))
                        }
                        Button {
                            Task { await session.spawnTerminal() }
                        } label: {
                            Image(systemName: "plus")
                        }
                        .accessibilityLabel(Text("New terminal"))
                    }
                }
        }
        .navigationSplitViewStyle(.balanced)
    }

    /// Terminal and chat live side by side; neither is torn down by focus
    /// changes, so PTY scrollback and chat streaming stay warm.
    private var wideDetail: some View {
        VStack(spacing: 0) {
            TerminalTabStrip(session: session)
            HStack(spacing: 0) {
                TerminalWorkspaceView(session: session)
                    .frame(minWidth: 420, maxWidth: .infinity)
                    .layoutPriority(1)
                if session.workspace == .conversation {
                    Divider()
                    ChatView(session: session, embedded: true)
                        .frame(minWidth: 320, idealWidth: 440, maxWidth: 620)
                }
            }
        }
    }

    // MARK: Compact (iPhone) layout

    private var compactLayout: some View {
        VStack(spacing: 0) {
            header
            if session.workspaceTab == .terminal {
                TerminalTabStrip(session: session)
            }
            content
            if !session.terminalKeyboardVisible {
                tabBar
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button {
                session.leaveWorkspace()
            } label: {
                Image(systemName: "chevron.backward")
                    .font(.body.bold())
                    .frame(minWidth: 44, minHeight: 44)
            }
            .accessibilityLabel(Text("Back"))

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.headline)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .onTapGesture {
                session.dismissTerminalKeyboard()
            }
            Spacer(minLength: 8)
            if session.workspace == .conversation {
                Button {
                    session.chat.showAgentSheet = true
                } label: {
                    Image(systemName: "cpu")
                        .frame(minWidth: 44, minHeight: 44)
                }
                .accessibilityLabel(Text("Agent"))
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
        .background(.ultraThinMaterial)
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
    }

    @ViewBuilder
    private var content: some View {
        ZStack {
            keepAliveWorkspace
                .opacity(session.workspaceTab == .files ? 0 : 1)
                .allowsHitTesting(session.workspaceTab != .files)
                .accessibilityHidden(session.workspaceTab == .files)
            FileBrowserView(session: session, embedded: true)
                .opacity(session.workspaceTab == .files ? 1 : 0)
                .allowsHitTesting(session.workspaceTab == .files)
                .accessibilityHidden(session.workspaceTab != .files)
        }
    }

    @ViewBuilder
    private var keepAliveWorkspace: some View {
        ZStack {
            if session.workspace == .project {
                ContentUnavailableView(
                    String(localized: "Chat"),
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text("Project view is terminals only. Open a session to talk.")
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .opacity(session.workspaceTab == .chat ? 1 : 0)
                .allowsHitTesting(session.workspaceTab == .chat)
            } else {
                ChatView(session: session, embedded: true)
                    .opacity(session.workspaceTab == .chat ? 1 : 0)
                    .allowsHitTesting(session.workspaceTab == .chat)
                    .accessibilityHidden(session.workspaceTab != .chat)
            }
            TerminalWorkspaceView(session: session)
                .opacity(session.workspaceTab == .terminal ? 1 : 0)
                .allowsHitTesting(session.workspaceTab == .terminal)
                .accessibilityHidden(session.workspaceTab != .terminal)
        }
    }

    private var tabBar: some View {
        HStack(spacing: 0) {
            ForEach(WorkspaceTab.allCases) { tab in
                Button {
                    session.setWorkspaceTab(tab)
                } label: {
                    VStack(spacing: 4) {
                        Image(systemName: tab.systemImage)
                            .font(.body.weight(.medium))
                        Text(tab.title)
                            .font(.caption2.weight(.medium))
                    }
                    .foregroundStyle(session.workspaceTab == tab ? SeTheme.accent : SeTheme.muted)
                    .frame(maxWidth: .infinity)
                    .frame(minHeight: 48)
                }
                .accessibilityLabel(Text(tab.title))
                .accessibilityAddTraits(session.workspaceTab == tab ? .isSelected : [])
            }
        }
        .padding(.top, 4)
        .background(.ultraThinMaterial)
        .overlay(alignment: .top) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
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
                filesVisibility = filesVisibility == .detailOnly ? .all : .detailOnly
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
