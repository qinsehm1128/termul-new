import Foundation

enum ChatTimelineItem: Identifiable, Hashable, Sendable {
    case user(ChatMessage)
    case agent(ChatMessage)
    case activity(TurnActivity)

    var id: String {
        switch self {
        case .user(let message), .agent(let message):
            message.id
        case .activity(let activity):
            activity.id
        }
    }
}

struct TurnActivity: Identifiable, Hashable, Sendable {
    let id: String
    var thoughts: [ChatMessage]
    var tools: [ToolCard]
    var active: Bool
    var attentionRequired: Bool

    var toolCount: Int { tools.count }
    var thoughtCount: Int { thoughts.count }

    var summary: String {
        if active {
            return String(localized: "Working…")
        }
        if attentionRequired {
            return String(localized: "Needs attention")
        }
        if toolCount > 0 && thoughtCount > 0 {
            return String(localized: "Worked · \(toolCount) tools")
        }
        if toolCount > 0 {
            return toolCount == 1
                ? String(localized: "Used 1 tool")
                : String(localized: "Used \(toolCount) tools")
        }
        return String(localized: "Thought")
    }
}

enum ChatTimeline {
    /// Visible chat: user and agent replies stay open. Consecutive thoughts and
    /// tool calls between turns collapse into one activity row.
    static func build(
        messages: [ChatMessage],
        tools: [ToolCard],
        activeTurn: Bool
    ) -> [ChatTimelineItem] {
        let stream = stamped(messages: messages, tools: tools)
        var items: [ChatTimelineItem] = []
        var thoughts: [ChatMessage] = []
        var groupedTools: [ToolCard] = []
        var activityIndex = 0

        func flushActivity(active: Bool) {
            guard !thoughts.isEmpty || !groupedTools.isEmpty else { return }
            let attention = groupedTools.contains { $0.needsAttention }
            items.append(
                .activity(
                    TurnActivity(
                        id: "activity:\(thoughts.first?.id ?? groupedTools.first?.id ?? "\(activityIndex)")",
                        thoughts: thoughts,
                        tools: groupedTools,
                        active: active,
                        attentionRequired: attention && !active
                    )
                )
            )
            thoughts = []
            groupedTools = []
            activityIndex += 1
        }

        for event in stream {
            switch event {
            case .user(let message):
                flushActivity(active: false)
                items.append(.user(message))
            case .agent(let message):
                flushActivity(active: false)
                if !message.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || message.streaming {
                    items.append(.agent(message))
                }
            case .thought(let message):
                thoughts.append(message)
            case .tool(let tool):
                groupedTools.append(tool)
            }
        }

        let lastActivityIsLive = activeTurn && (
            thoughts.contains { $0.streaming } || groupedTools.contains { $0.isBusy }
        )
        flushActivity(active: lastActivityIsLive)
        return items
    }

    private enum Stamped: Sendable {
        case user(ChatMessage)
        case agent(ChatMessage)
        case thought(ChatMessage)
        case tool(ToolCard)
    }

    private static func stamped(messages: [ChatMessage], tools: [ToolCard]) -> [Stamped] {
        struct Row {
            var seq: UInt64?
            var order: Int
            var event: Stamped
        }

        var rows: [Row] = []
        for (index, message) in messages.enumerated() {
            let event: Stamped = switch message.role {
            case .user: .user(message)
            case .agent: .agent(message)
            case .thought: .thought(message)
            }
            rows.append(Row(seq: message.seq, order: index, event: event))
        }
        for (index, tool) in tools.enumerated() {
            rows.append(Row(seq: tool.seq, order: 1000 + index, event: .tool(tool)))
        }
        rows.sort { left, right in
            switch (left.seq, right.seq) {
            case let (leftSeq?, rightSeq?):
                if leftSeq != rightSeq { return leftSeq < rightSeq }
            case (nil, .some):
                return false
            case (.some, nil):
                return true
            case (nil, nil):
                break
            }
            return left.order < right.order
        }
        return rows.map(\.event)
    }
}
