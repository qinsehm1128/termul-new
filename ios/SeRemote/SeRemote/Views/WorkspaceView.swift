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
        _session = State(initialValue: WorkspaceSession(
            accessURL: link.accessURL,
            bearer: link.pairingToken,
            settings: settings,
            hostID: link.stableHostID
        ))
    }

    /// The mounted shell stays on screen (and interactive) once the session
    /// has connected at least once — including while a foreground/network
    /// recovery re-runs its handshakes. Blanking the live terminal/chat
    /// canvas behind a full-screen "Connecting…" spinner for a link that
    /// already worked is the bug this guards against.
    private var shellIsPresentable: Bool {
        session.hasEstablishedConnection
            && (session.phase == .connected
                || (session.isRecovering && session.phase == .connecting))
    }

    var body: some View {
        ZStack {
            // Once connected, keep the navigation tree alive even during a
            // foreground reconnect. In particular, never remount the PTY view.
            if hasConnected {
                ConnectedShellScreen(session: session, store: store, link: link)
                    .environment(settings)
                    .environment(store)
                    .opacity(shellIsPresentable ? 1 : 0)
                    .allowsHitTesting(shellIsPresentable)
                    .accessibilityHidden(!shellIsPresentable)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            }
            switch session.phase {
            case .connecting, .idle:
                // A recovery pass keeps the live shell on screen; the opaque
                // full-screen "Connecting…" overlay must not stack on top of
                // it (the ZStack draws this above the shell).
                if !shellIsPresentable {
                    connectingState
                        .transition(SeTransition.entrance(reduceMotion: reduceMotion))
                }
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
            case .connected, .revoked:
                // Root owns revoked-device presentation; retain the mounted shell.
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
            if ShellConnectionLifecycle.shouldStop(disappearingSession: link.sessionIdentity,
                                                   activeSession: store.activeLink?.sessionIdentity) {
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
            // Transient `.inactive` samples (Control Center, notification
            // shade, app-switcher peek, system banners) are not lifecycle
            // events; only a real background stay and its return are.
            switch ShellConnectionLifecycle.sceneAction(for: phase, wasBackgrounded: session.isBackgrounded) {
            case .enterBackground:
                session.handleScene(isBackground: true)
            case .enterForeground:
                // handleScene(false) -> revalidateTransports already retries a
                // `.failed` link after proving the route settled; a second
                // concurrent `retry()` here raced it and re-blanked the shell.
                session.handleScene(isBackground: false)
            case .none:
                break
            }
        }
        .alert(
            String(localized: "Could not load session"),
            isPresented: alertBinding
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(session.conversations.errorMessage ?? session.projects.errorMessage ?? session.files.errorMessage ?? "")
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
        let needsRePairing = session.needsRePairing
        return VStack(spacing: 16) {
            SeStatusBadge(status: .failed)
            Text(needsRePairing
                ? String(localized: "Pairing needed")
                : String(localized: "Could not open this desk"))
                .font(SeTheme.display)
                .multilineTextAlignment(.center)
            Text(needsRePairing
                ? String(localized: "This host answers at a different address or no longer accepts this device. Scan its QR code once to pair again.")
                : message)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            VStack(spacing: 10) {
                if needsRePairing {
                    // A retry re-dials the same dead origin with the same
                    // rejected credential. The scan is the recovery, so it is
                    // the primary action, not a footnote.
                    Button {
                        store.requestRePairing()
                    } label: {
                        Text("Scan the QR code again")
                            .font(.body.bold())
                            .frame(maxWidth: .infinity)
                            .frame(minHeight: 48)
                    }
                    .buttonStyle(SePrimaryButtonStyle())
                    .accessibilityIdentifier("connect-repair")
                } else {
                    Button {
                        Task { await session.retry() }
                    } label: {
                        Text("Retry")
                            .font(.body.bold())
                            .frame(maxWidth: .infinity)
                            .frame(minHeight: 48)
                    }
                    .buttonStyle(SePrimaryButtonStyle())
                }
                Button {
                    if needsRePairing {
                        Task { await session.retry() }
                    } else {
                        store.disconnect()
                    }
                } label: {
                    Text(needsRePairing ? String(localized: "Retry anyway") : String(localized: "Back"))
                        .frame(maxWidth: .infinity)
                        .frame(minHeight: 44)
                }
                .buttonStyle(.bordered)
                if needsRePairing {
                    Button {
                        store.disconnect()
                    } label: {
                        Text("Back")
                            .frame(maxWidth: .infinity)
                            .frame(minHeight: 44)
                    }
                    .buttonStyle(.bordered)
                }
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
                // Recovery windows heal their own failures; an alert raised
                // mid-reconnect would only show a stale, already-fixed error.
                guard !session.isRecovering else { return false }
                return session.conversations.errorMessage != nil
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
