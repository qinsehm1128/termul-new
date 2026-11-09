import SwiftUI

struct WorkspaceView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    let link: RemoteLink
    @State private var session: WorkspaceSession
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hasConnected = false
    @State private var successFeedback = 0
    @State private var failureFeedback = 0
    @State private var startTask: Task<Void, Never>?

    init(store: ConnectionStore, settings: AppSettings, link: RemoteLink) {
        self.store = store
        self.settings = settings
        self.link = link
        _session = State(initialValue: WorkspaceSession(accessURL: link.accessURL, bearer: link.pairingToken, settings: settings))
    }

    var body: some View {
        ZStack {
            // Once connected, keep the navigation tree alive even during a
            // foreground reconnect. In particular, never remount the PTY view.
            if hasConnected {
                ConnectedShellScreen(session: session, store: store, link: link)
                    .environment(settings)
                    .environment(store)
                    .opacity(session.phase == .connected ? 1 : 0)
                    .allowsHitTesting(session.phase == .connected)
                    .accessibilityHidden(session.phase != .connected)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            }
            switch session.phase {
            case .connecting, .idle:
                connectingState
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            case .failed(let message):
                if hasConnected {
                    ConnectedOfflineScreen(session: session, store: store, link: link)
                        .environment(settings)
                        .environment(store)
                        .transition(SeTransition.entrance(reduceMotion: reduceMotion))
                } else {
                    failedState(message)
                        .transition(SeTransition.entrance(reduceMotion: reduceMotion))
                }
            case .connected:
                EmptyView()
            }
        }
        .background(SeTheme.canvas.ignoresSafeArea())
        .seAnimation(.context, value: session.phase)
        .seAnimation(.context, value: hasConnected)
        .seHaptic(.success, trigger: successFeedback)
        .seHaptic(.error, trigger: failureFeedback)
        .onChange(of: session.phase) { old, phase in
            if phase == .connected {
                // Pairing succeeded only after the host handshake, not parsing.
                ShellPairingActivity().recordConnection(link)
                if !hasConnected { successFeedback += 1 }
                hasConnected = true
            }
            if case .failed = phase, old != phase { failureFeedback += 1 }
        }
        .onDisappear {
            // Animated insertion may transiently disappear mid-connect. Only
            // an authoritative disconnect/link replacement tears down sockets.
            if ShellConnectionLifecycle.shouldStop(disappearingLinkID: link.id,
                                                   activeLinkID: store.activeLink?.id) {
                startTask?.cancel()
                startTask = nil
                session.stop()
            }
        }
        .onAppear {
            // Unlike .task, this handshake is not cancelled by an animated
            // insertion's transient disappearance. Real teardown cancels it.
            if startTask == nil {
                startTask = Task { await session.start() }
            }
        }
        .onChange(of: scenePhase) { _, phase in
            session.handleScene(isBackground: phase != .active)
            if phase == .active, case .failed = session.phase {
                Task { await session.retry() }
            }
        }
        .alert(
            String(localized: "Could not load session"),
            isPresented: alertBinding
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(session.conversations.errorMessage ?? session.projects.errorMessage ?? session.files.errorMessage ?? session.terminals.errorMessage ?? "")
        }
    }

    private var connectingState: some View {
        VStack(spacing: 16) {
            SeStatusBadge(status: .connecting)
            Text("Connecting to host…")
                .font(SeTheme.display)
            Text(link.title)
                .font(.body)
                .foregroundStyle(.secondary)
            Button {
                store.disconnect()
            } label: {
                Text("Cancel")
                    .frame(minWidth: 120, minHeight: 44)
            }
            .buttonStyle(.bordered)
            .padding(.top, 12)
            .accessibilityIdentifier("connecting-cancel")
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SeTheme.canvas.ignoresSafeArea())
        .accessibilityIdentifier("connecting")
    }

    private func failedState(_ message: String) -> some View {
        VStack(spacing: 16) {
            SeStatusBadge(status: .failed)
            Text("Could not open this desk")
                .font(SeTheme.display)
                .multilineTextAlignment(.center)
            Text(message)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            VStack(spacing: 10) {
                Button {
                    Task { await session.retry() }
                } label: {
                    Text("Retry")
                        .font(.body.bold())
                        .frame(maxWidth: .infinity)
                        .frame(minHeight: 48)
                }
                .buttonStyle(SePrimaryButtonStyle())
                Button {
                    store.disconnect()
                } label: {
                    Text("Back")
                        .frame(maxWidth: .infinity)
                        .frame(minHeight: 44)
                }
                .buttonStyle(.bordered)
            }
            .frame(maxWidth: 280)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SeTheme.canvas.ignoresSafeArea())
        .accessibilityIdentifier("connect-error")
    }

    private var workspacePresented: Binding<Bool> {
        Binding(
            get: { session.workspace != .home },
            set: { presented in
                if !presented, session.workspace != .home {
                    session.leaveWorkspace()
                }
            }
        )
    }

    private var alertBinding: Binding<Bool> {
        Binding(
            get: {
                session.conversations.errorMessage != nil
                    || session.projects.errorMessage != nil
                    || session.files.errorMessage != nil
            },
            set: { presented in
                if !presented {
                    session.conversations.errorMessage = nil
                    session.projects.errorMessage = nil
                    session.files.errorMessage = nil
                }
            }
        )
    }
}
