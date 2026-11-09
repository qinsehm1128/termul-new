import Foundation

enum OfflinePresentation {
    static func sessions(conversations: [HostConversation], transcripts: [String: CachedTranscript]) -> [OfflineSession] {
        conversations.compactMap { conversation in
            guard let transcript = transcripts[conversation.id],
                  transcript.conversationId == conversation.id,
                  !transcript.messages.isEmpty || !transcript.tools.isEmpty else { return nil }
            return OfflineSession(id: conversation.id, title: conversation.displayTitle,
                                 meta: String(localized: "Read only"), badge: String(localized: "Cached"))
        }
    }
}
