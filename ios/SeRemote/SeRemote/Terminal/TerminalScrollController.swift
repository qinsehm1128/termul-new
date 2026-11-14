import CoreGraphics
import Foundation

/// Programmatic scroll commands issued by the floating chrome or accessory bar.
enum TerminalScrollCommand: Equatable, Sendable {
    case stepUp
    case stepDown
    case pageUp
    case pageDown
    case bottom
}

/// How a vertical touch pan on the mobile terminal canvas should be routed.
enum TerminalScrollRoute: Equatable, Sendable {
    /// Normal screen buffer without mouse reporting: let `UIScrollView` scroll
    /// local scrollback natively with full iOS momentum.
    case nativeScrollView
    /// Mouse reporting is enabled (e.g. `qin-code`, `claude`, `codex`, `tmux`,
    /// `vim`, `htop`, `less --mouse`): translate vertical drag into xterm/SGR
    /// mouse wheel events (`64` up / `65` down) at the touch coordinate.
    case mouseWheel
    /// Alternate screen buffer without mouse reporting (e.g. `less`, `man`,
    /// `git log` under DECSET 1007): translate vertical drag into cursor up/down
    /// sequences.
    case alternateCursorKeys
    /// Active selection or read-only TUI where touch drag must not emit input.
    case none
}

enum TerminalScrollController {
    static func route(
        isAlternateBuffer: Bool,
        mouseModeActive: Bool,
        allowsInput: Bool,
        selectionActive: Bool
    ) -> TerminalScrollRoute {
        if selectionActive {
            return .none
        }
        if mouseModeActive {
            return allowsInput ? .mouseWheel : .none
        }
        if isAlternateBuffer {
            return allowsInput ? .alternateCursorKeys : .none
        }
        return .nativeScrollView
    }

    static func cursorSequence(up: Bool, applicationCursor: Bool) -> String {
        if up {
            return applicationCursor ? "\u{1b}OA" : "\u{1b}[A"
        }
        return applicationCursor ? "\u{1b}OB" : "\u{1b}[B"
    }

    /// Whether swipe-down-to-dismiss-keyboard may claim a vertical drag without
    /// fighting terminal scrolling.
    static func allowsSwipeDownKeyboardDismiss(
        isFirstResponder: Bool,
        route: TerminalScrollRoute,
        hasNormalScrollback: Bool
    ) -> Bool {
        guard isFirstResponder, route == .nativeScrollView else { return false }
        return !hasNormalScrollback
    }
}

/// Converts continuous vertical touch deltas into discrete terminal scroll ticks
/// while preserving sub-row remainder and natural iOS scroll direction:
/// - Finger moves DOWN (`deltaY > 0`): pull older lines down into view → `up = true`.
/// - Finger moves UP (`deltaY < 0`): push content up toward newer lines → `up = false`.
struct VerticalPanAccumulator: Equatable, Sendable {
    struct TickBatch: Equatable, Sendable {
        let up: Bool
        let count: Int
    }

    private(set) var remainder: CGFloat = 0
    var maxTicksPerStep: Int = 8

    static func stepHeight(forCellHeight cellHeight: CGFloat) -> CGFloat {
        min(max(cellHeight * 1.15, 15), 28)
    }

    mutating func reset() {
        remainder = 0
    }

    mutating func consume(deltaY: CGFloat, cellHeight: CGFloat) -> TickBatch? {
        guard deltaY != 0, deltaY.isFinite else { return nil }
        let step = Self.stepHeight(forCellHeight: cellHeight)
        // Reversing finger direction drops stale opposite remainder immediately
        // so changing scroll direction feels crisp rather than spongy.
        if (remainder > 0 && deltaY < 0) || (remainder < 0 && deltaY > 0) {
            remainder = 0
        }
        remainder += deltaY
        let rawTicks = Int((abs(remainder) / step).rounded(.down))
        guard rawTicks > 0 else { return nil }
        let up = remainder > 0
        let consumed = CGFloat(rawTicks) * step
        if up {
            remainder -= consumed
        } else {
            remainder += consumed
        }
        return TickBatch(up: up, count: min(rawTicks, max(1, maxTicksPerStep)))
    }

    /// Short decaying inertia burst on finger lift for fast flicks in TUI views.
    static func momentumTicks(velocityY: CGFloat, cellHeight: CGFloat) -> [TickBatch] {
        guard velocityY.isFinite else { return [] }
        let speed = abs(velocityY)
        guard speed >= 280 else { return [] }
        let up = velocityY > 0
        let step = stepHeight(forCellHeight: cellHeight)
        let totalLines = min(max(Int((speed * 0.16 / step).rounded()), 2), 18)
        var remaining = totalLines
        var batches: [TickBatch] = []
        let weights: [Double] = [0.35, 0.25, 0.18, 0.12, 0.10]
        for (index, weight) in weights.enumerated() {
            guard remaining > 0 else { break }
            let count: Int
            if index == weights.count - 1 {
                count = remaining
            } else {
                count = max(1, min(remaining, Int((Double(totalLines) * weight).rounded())))
            }
            batches.append(TickBatch(up: up, count: count))
            remaining -= count
        }
        return batches
    }
}
