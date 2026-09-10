import Foundation
import Observation

@MainActor
@Observable
final class ConversationStore {
    var conversations: [HostConversation] = []
    var active: HostConversation?
    var isLoading = false
    var errorMessage: String?

    private var http: HostHTTP?

    func attach(http: HostHTTP) {
        self.http = http
    }

    func refresh() async {
        guard let http else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let listed: [HostConversation] = try await http.get("conversations")
            conversations = listed.filter { !$0.isDeleted }
            if let current = active {
                active = conversations.first(where: { $0.id == current.id })
            }
        } catch {
            HostLog.session.error("Conversation list refresh failed")
            errorMessage = error.localizedDescription
        }
    }

    func select(_ conversation: HostConversation) {
        active = conversation
    }

    /// Rename over the shared web contract. The host returns the updated
    /// record, which replaces the row in place.
    func rename(_ conversation: HostConversation, title: String) async {
        guard let http else { return }
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        do {
            let updated: HostConversation = try await http.post(
                "conversations/\(conversation.id)/rename",
                body: ["title": trimmed]
            )
            if let index = conversations.firstIndex(where: { $0.id == updated.id }) {
                conversations[index] = updated
            }
            if active?.id == updated.id {
                active = updated
            }
            HostLog.session.info("Conversation renamed")
        } catch {
            HostLog.session.error("Conversation rename failed")
            errorMessage = error.localizedDescription
        }
    }

    /// Soft-delete via the lifecycle contract; `lastSeq` doubles as the
    /// optimistic-concurrency revision the host validates. The list snapshot
    /// goes stale as turns land, so the record is re-read first; a `blocked`
    /// outcome keeps the row and surfaces why.
    func delete(_ conversation: HostConversation) async {
        guard let http else { return }
        do {
            var target = conversation
            if let fresh: HostConversation = try? await http.get("conversations/\(conversation.id)") {
                target = fresh
            }
            let outcome: ConversationLifecycleOutcome = try await http.post(
                "conversations/\(conversation.id)/lifecycle/delete",
                body: [
                    "expectedRevision": Int(target.lastSeq ?? 0),
                    "removeWorkspace": false,
                ]
            )
            guard outcome.isUpdated else {
                HostLog.session.info("Conversation delete blocked by host")
                errorMessage = String(localized: "The host kept this session: live terminals or an active agent still reference it.")
                return
            }
            conversations.removeAll { $0.id == conversation.id }
            if active?.id == conversation.id {
                clearSelection()
            }
            HostLog.session.info("Conversation deleted")
        } catch {
            HostLog.session.error("Conversation delete failed")
            errorMessage = error.localizedDescription
        }
    }

    func clearSelection() {
        active = nil
    }

    func open(_ conversation: HostConversation) async -> ConversationOpenOutcome? {
        guard let http else { return nil }
        do {
            let opened: ConversationOpenOutcome = try await http.post(
                "conversations/\(conversation.id)/open",
                body: [:]
            )
            if let record = opened.conversation {
                select(record)
            } else {
                select(conversation)
            }
            return opened
        } catch {
            errorMessage = error.localizedDescription
            select(conversation)
            return nil
        }
    }

    func binding(for conversation: HostConversation) async -> AgentSessionBinding? {
        guard let http else { return nil }
        do {
            let snapshot: ConversationBindingSnapshot = try await http.get(
                "conversations/\(conversation.id)/binding"
            )
            return snapshot.binding
        } catch {
            HostLog.session.error("Conversation binding lookup failed")
            return nil
        }
    }
}
