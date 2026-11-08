import Foundation

/// View-only presentation decisions. Transcript loading and token updates must
/// never be mistaken for a local send.
enum WorkspaceChatPresentation {
    static func sentMessageIDs(
        previous: [ChatTimelineItem],
        current: [ChatTimelineItem],
        sendRequested: Bool,
        isLoading: Bool
    ) -> Set<String> {
        guard sendRequested, !isLoading else { return [] }
        let known = Set(previous.map(\.id))
        return Set(current.compactMap { item in
            guard case .user(let message) = item,
                  !known.contains(message.id), message.delivery == .sending else { return nil }
            return message.id
        })
    }

    static func outcome(_ status: String) -> String {
        switch status {
        case "pending": String(localized: "Queued")
        case "in_progress": String(localized: "Running")
        case "completed", "success": String(localized: "Completed")
        case "failed": String(localized: "Failed")
        case "cancelled": String(localized: "Cancelled")
        default: status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    /// The host currently sends a title and outcome, not structured execution
    /// telemetry. Keep the entire title visible rather than inventing metadata.
    static func toolCommand(_ title: String) -> String {
        title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? String(localized: "Tool call") : title
    }
}
