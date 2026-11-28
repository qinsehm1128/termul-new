import Foundation

/// Count/byte budget for a retained transcript.
///
/// A conversation transcript is visible history, not a growing log. Without a
/// bound both the per-render cost and retained memory grow for the whole life of
/// a session. The policy is pure so it can be exercised without a store.
nonisolated enum TranscriptBudget {
    /// Newest items that fit the budget. Oldest go first and at least one item
    /// is always kept, so a bounded transcript can never be empty.
    static func keepingNewest<T>(
        _ items: [T],
        maxCount: Int,
        maxBytes: Int,
        size: (T) -> Int
    ) -> [T] {
        guard !items.isEmpty, maxCount > 0 else { return items }
        var bytes = items.reduce(0) { $0 + max(size($1), 0) }
        var start = 0
        while items.count - start > 1,
              (items.count - start > maxCount || bytes > maxBytes) {
            bytes -= max(size(items[start]), 0)
            start += 1
        }
        return start == 0 ? items : Array(items[start...])
    }
}
