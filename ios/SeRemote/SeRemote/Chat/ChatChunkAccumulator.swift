import Foundation

/// Buffers consecutive streaming `message_chunk` payloads so one transcript
/// mutation covers a frame budget instead of one per token.
///
/// ACP streams an agent reply as many small chunks. Applying each chunk the
/// moment it arrives makes SwiftUI re-diff the whole transcript and re-parse
/// the growing bubble's Markdown once per token, so the cost of a reply grows
/// with its own length. Chunks are therefore held for at most one frame and
/// applied in arrival order.
///
/// Merging adjacent same-role chunks is exactly the result of applying them one
/// by one: `appendChunk` only ever appends to the trailing message while its
/// role matches and it is still streaming. A role change starts a new message,
/// so it starts a new segment. The highest host cursor seen for a segment is
/// kept, so the flushed message records the same resume watermark a per-chunk
/// apply would have written.
nonisolated struct ChatChunkAccumulator: Equatable, Sendable {
    struct Segment: Equatable, Sendable {
        var role: String
        var text: String
        var seq: UInt64
    }

    private(set) var segments: [Segment] = []

    var isEmpty: Bool { segments.isEmpty }

    /// Retained text, for diagnostics and tests.
    var characterCount: Int {
        segments.reduce(0) { $0 + $1.text.count }
    }

    mutating func append(role: String, text: String, seq: UInt64 = 0) {
        if var last = segments.last, last.role == role {
            last.text += text
            last.seq = max(last.seq, seq)
            segments[segments.count - 1] = last
            return
        }
        segments.append(Segment(role: role, text: text, seq: seq))
    }

    /// Take every buffered segment in arrival order.
    mutating func drain() -> [Segment] {
        let drained = segments
        segments.removeAll(keepingCapacity: true)
        return drained
    }

    /// Drop buffered text without applying it (the transcript it belonged to is
    /// being replaced).
    mutating func discard() {
        segments.removeAll(keepingCapacity: true)
    }
}
