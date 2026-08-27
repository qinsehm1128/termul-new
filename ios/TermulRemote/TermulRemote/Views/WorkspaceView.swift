import SwiftUI

struct WorkspaceView: View {
    @Bindable var store: ConnectionStore
    let link: RemoteLink
    @State private var session: WorkspaceSession
    @Environment(\.scenePhase) private var scenePhase

    init(store: ConnectionStore, link: RemoteLink) {
        self.store = store
        self.link = link
        _session = State(initialValue: WorkspaceSession(accessURL: link.accessURL, bearer: link.pairingToken))
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
        .background(TermulTheme.canvas.ignoresSafeArea())
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
                .font(TermulTheme.display)
            Text(link.title)
                .font(.body)
                .foregroundStyle(.secondary)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(TermulTheme.canvas.ignoresSafeArea())
        .accessibilityIdentifier("connecting")
    }

    private func failedState(_ message: String) -> some View {
        VStack(spacing: 16) {
            Text("Could not open this desk")
                .font(TermulTheme.display)
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
                .tint(TermulTheme.accent)
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
        .background(TermulTheme.canvas.ignoresSafeArea())
        .accessibilityIdentifier("connect-error")
    }

    private var alertBinding: Binding<Bool> {
        Binding(
            get: {
                session.conversations.errorMessage != nil
                    || session.projects.errorMessage != nil
                    || session.files.errorMessage != nil
                    || session.terminals.errorMessage != nil
            },
            set: { presented in
                if !presented {
                    session.conversations.errorMessage = nil
                    session.projects.errorMessage = nil
                    session.files.errorMessage = nil
                    session.terminals.errorMessage = nil
                }
            }
        )
    }
}
