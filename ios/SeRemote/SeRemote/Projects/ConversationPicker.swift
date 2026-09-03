import SwiftUI

struct ConversationPicker: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List(session.conversations.conversations) { conversation in
                Button {
                    Task {
                        await session.selectConversation(conversation)
                        dismiss()
                    }
                } label: {
                    HStack {
                        HostListRow(
                            title: conversation.displayTitle,
                            preview: conversation.previewText,
                            previewMono: true,
                            meta: conversation.countLabel,
                            status: .idle,
                            time: conversation.relativeCreatedLabel
                        )
                        if session.conversations.active?.id == conversation.id {
                            Image(systemName: "checkmark")
                                .foregroundStyle(SeTheme.accent)
                        }
                    }
                }
            }
            .navigationTitle(String(localized: "Sessions"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .overlay {
                if session.conversations.conversations.isEmpty {
                    ContentUnavailableView(
                        "No running sessions",
                        systemImage: "bubble.left.and.bubble.right",
                        description: Text("Open a chat or terminal on the desktop, then pull to refresh.")
                    )
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
