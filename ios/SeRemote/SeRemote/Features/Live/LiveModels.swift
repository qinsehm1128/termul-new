import Foundation

// MARK: - Shared session models

enum LiveSessionStatus: String, Sendable {
    case needsYou
    case running
    case ready
    case unknown
    case idle
}

struct LiveSession: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let agent: String
    let host: String
    let status: LiveSessionStatus
    let timing: String
    var detailStatus: String? = nil
}

struct LiveNeedsSummary: Equatable, Sendable {
    var permissions: Int = 0
    var failures: Int = 0
    var questions: Int = 0

    var total: Int { permissions + failures + questions }
}

enum LiveFilter: String, CaseIterable, Identifiable {
    case all
    case running
    case needsYou

    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: String(localized: "All")
        case .running: String(localized: "Running")
        case .needsYou: String(localized: "Need you")
        }
    }
}

/// One ACP conversation row for the 会话 list. Only host-reported values enter
/// this model: no terminals, no projects, no invented agents.
struct ConversationEntry: Identifiable, Equatable, Sendable {
    let id: String
    let conversation: HostConversation
    let title: String
    let agent: String
    let project: String
    let status: LiveSessionStatus
    let statusText: String
    let time: String?
    /// A host conversation is active/streaming when it is the open workbench
    /// conversation and its agent is sending or needs review.
    let isActiveOrStreaming: Bool
    let needsAttention: Bool
}
