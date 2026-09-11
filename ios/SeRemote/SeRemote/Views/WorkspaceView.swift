import SwiftUI

struct WorkspaceView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    let link: RemoteLink
    @State private var session: WorkspaceSession
    @Environment(\.scenePhase) private var scenePhase

    init(store: ConnectionStore, settings: AppSettings, link: RemoteLink) {
        self.store = store
        self.settings = settings
        self.link = link
        _session = State(initialValue: WorkspaceSession(accessURL: link.accessURL, bearer: link.pairingToken, settings: settings))
    }

    var body: some View {
        Group {
            switch session.phase {
            case .connecting, .idle:
                connectingState
            case .failed(let message):
                failedState(message)
            case .connected:
                switch session.workspace {
                case .home:
                    HostHomeView(session: session, store: store, link: link)
                case .conversation, .project:
                    SessionScreen(session: session)
                }
            }
        }
        .background(SeTheme.canvas.ignoresSafeArea())
        .onDisappear {
            // Leaving the workspace view (cancel, back, disconnect) closes
            // the sockets; retry keeps the view mounted so this only fires
            // on real teardown.
            session.stop()
        }
        .task(id: link.id) {
            await session.start()
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
            ProgressView()
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
                .buttonStyle(.borderedProminent)
                .tint(SeTheme.accent)
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
