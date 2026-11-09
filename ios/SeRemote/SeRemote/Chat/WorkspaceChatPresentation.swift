import Foundation

/// View-only presentation decisions. Transcript loading and token updates must
/// never be mistaken for a local send.
enum WorkspaceChatPresentation {
    /// Absence of an acknowledgement never produces a completion haptic.
    static func acknowledgedPermissionFeedback(
        optionID: String?, optionName: String?, acknowledged: Bool
    ) -> PermissionFeedback? {
        guard acknowledged else { return nil }
        return permissionFeedback(optionID: optionID, optionName: optionName, acknowledged: true)
    }

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

    enum PermissionFeedback: Equatable {
        case approval
        case denial
        case failure
    }

    /// Provider option IDs carry semantics such as `reject_once`; the name is
    /// a fallback for providers with opaque IDs. A failed request never emits
    /// either success or denial feedback.
    static func permissionFeedback(
        optionID: String?, optionName: String?, acknowledged: Bool
    ) -> PermissionFeedback {
        guard acknowledged else { return .failure }
        guard let optionID else { return .denial }
        let words = (optionID + " " + (optionName ?? ""))
            .lowercased().split { !$0.isLetter }
        let denialWords = ["deny", "denied", "reject", "decline", "cancel"]
        return words.contains { word in
            denialWords.contains { word.hasPrefix($0) }
        } ? .denial : .approval
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
