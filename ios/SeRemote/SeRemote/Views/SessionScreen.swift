import SwiftUI
import UIKit

struct SessionScreen: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        @Bindable var chat = session.chat
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
        .background(SeTheme.canvas.ignoresSafeArea())
        .modifier(TerminalKeyboardAvoidance(enabled: session.workspaceTab == .terminal))
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { notification in
            guard session.workspaceTab == .terminal else { return }
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
        if session.workspace == .project {
            ZStack {
                ContentUnavailableView(
                    String(localized: "Chat"),
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text("Project view is terminals only. Open a session to talk.")
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .opacity(session.workspaceTab == .chat ? 1 : 0)
                .allowsHitTesting(session.workspaceTab == .chat)
                TerminalWorkspaceView(session: session)
                    .opacity(session.workspaceTab == .terminal ? 1 : 0)
                    .allowsHitTesting(session.workspaceTab == .terminal)
                    .accessibilityHidden(session.workspaceTab != .terminal)
            }
        } else if sizeClass == .regular {
            HStack(spacing: 0) {
                TerminalWorkspaceView(session: session)
                Divider()
                ChatView(session: session, embedded: true)
                    .frame(minWidth: 320, idealWidth: 380, maxWidth: 420)
                    .opacity(session.workspaceTab == .chat ? 1 : 0)
                    .allowsHitTesting(session.workspaceTab == .chat)
                    .accessibilityHidden(session.workspaceTab != .chat)
            }
        } else {
            ZStack {
                ChatView(session: session, embedded: true)
                    .opacity(session.workspaceTab == .chat ? 1 : 0)
                    .allowsHitTesting(session.workspaceTab == .chat)
                    .accessibilityHidden(session.workspaceTab != .chat)
                TerminalWorkspaceView(session: session)
                    .opacity(session.workspaceTab == .terminal ? 1 : 0)
                    .allowsHitTesting(session.workspaceTab == .terminal)
                    .accessibilityHidden(session.workspaceTab != .terminal)
            }
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

private struct TerminalKeyboardAvoidance: ViewModifier {
    var enabled: Bool

    func body(content: Content) -> some View {
        if enabled {
            content.ignoresSafeArea(.keyboard, edges: .bottom)
        } else {
            content
        }
    }
}
