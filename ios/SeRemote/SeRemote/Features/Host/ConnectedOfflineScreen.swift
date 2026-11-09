import SwiftUI

/// Offline owns no session mutation except explicit retry/disconnect. In
/// particular, reading a cached transcript cannot start an agent or attach a PTY.
struct ConnectedOfflineScreen: View {
    let session: WorkspaceSession
    let store: ConnectionStore
    let link: RemoteLink
    @Environment(AppSettings.self) private var settings
    @Environment(\.sePalette) private var palette
    @State private var transcripts: [String: CachedTranscript] = [:]
    @State private var selectedID: String?
    @State private var showsTranscript = false
    @State private var showsSettings = false

    var body: some View {
        OfflineView(host: link.title, lastSeen: "",
                    cached: OfflinePresentation.sessions(conversations: session.conversations.conversations, transcripts: transcripts),
                    onBack: { store.disconnect() }, onReconnect: { Task { await session.retry() } },
                    onOpenSettings: { showsSettings = true },
                    onOpenCached: { item in selectedID = item.id; showsTranscript = true })
        .task {
            var loaded: [String: CachedTranscript] = [:]
            for conversation in session.conversations.conversations {
                if let transcript = ChatTranscriptCache.load(conversation.id) {
                    loaded[conversation.id] = transcript
                }
            }
            transcripts = loaded
        }
        .sheet(isPresented: $showsSettings) {
            SettingsScreen(settings: settings, store: store, onBack: { showsSettings = false })
        }
        .sheet(isPresented: $showsTranscript) {
            NavigationStack {
                ScrollView {
                    if let selectedID, let transcript = transcripts[selectedID] {
                        VStack(alignment: .leading, spacing: SeSpace.lg) {
                            Text("Read only").font(SeNocturneType.secondary).foregroundStyle(palette.inkSecondary)
                            ForEach(transcript.messages) { message in
                                VStack(alignment: .leading, spacing: SeSpace.xs) {
                                    Text(message.role == .user ? String(localized: "You") : String(localized: "Agent"))
                                        .font(SeNocturneType.secondary).foregroundStyle(palette.inkSecondary)
                                    Text(verbatim: message.text).textSelection(.enabled)
                                }
                            }
                            ForEach(transcript.tools) { tool in
                                Text(verbatim: "\(tool.title) · \(tool.status)")
                                    .font(SeNocturneType.secondary).textSelection(.enabled)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(SeSpace.lg)
                    }
                }
                .background(palette.canvas)
                .foregroundStyle(palette.ink)
                .navigationTitle("Cached transcript")
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { showsTranscript = false }.frame(minHeight: 44)
                    }
                }
            }
        }
    }
}
