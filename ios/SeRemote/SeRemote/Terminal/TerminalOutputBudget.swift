import Foundation

/// Bounds the output held for terminals whose canvas is not mounted.
///
/// Each terminal's backlog is already capped by
/// `TerminalStore.pendingOutputLimit`, but nothing bounded their sum: a user who
/// visits many chatty shells keeps one backlog per shell alive until each one is
/// reopened. Dropping a stale backlog is safe because the store only ever has
/// one watched terminal; reopening any other terminal re-watches it from the
/// start, so the host replays that terminal's retained log again.
nonisolated enum TerminalOutputBudget {
    /// Ids to drop so the summed buffer sizes fit `limit`. `order` is the
    /// least-recently-touched-first eviction order and `protecting` holds the
    /// terminals whose bytes are still being fed or are about to be.
    static func evictedIDs(
        buffers: [String: Data],
        limit: Int,
        protecting protected: Set<String>,
        order: [String]
    ) -> [String] {
        var total = buffers.values.reduce(0) { $0 + $1.count }
        guard total > limit else { return [] }
        var evicted: [String] = []
        for id in order where total > limit {
            guard !protected.contains(id), let data = buffers[id] else { continue }
            total -= data.count
            evicted.append(id)
        }
        return evicted
    }

    /// Buffers for terminals the host no longer lists. A protected id (the live
    /// feed, the watched PTY) is never pruned, even if the catalog is mid
    /// refresh and has temporarily dropped it.
    static func staleIDs(
        buffers: [String: Data],
        listed: Set<String>,
        protecting protected: Set<String>
    ) -> [String] {
        buffers.keys.filter { !listed.contains($0) && !protected.contains($0) }
    }
}
