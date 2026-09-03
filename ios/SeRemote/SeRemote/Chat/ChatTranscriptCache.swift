import Foundation

struct SessionCursor: Decodable, Sendable {
    var sessionId: String
    var watermark: UInt64
}

struct CachedTranscript: Codable, Sendable {
    var schemaVersion: Int
    var conversationId: String
    var sessionId: String
    var agentId: String?
    var cwd: String
    var watermark: UInt64
    var messages: [ChatMessage]
    var tools: [ToolCard]
}

enum ChatTranscriptCache {
    static let schemaVersion = 2
    /// First visit loads this many seqs from the host tail, not 0 → end.
    static let tailWindow: UInt64 = 60
    static let pageLimit = 80

    static func load(_ conversationId: String) -> CachedTranscript? {
        let current = fileURL(conversationId)
        let url = FileManager.default.fileExists(atPath: current.path)
            ? current
            : fileURL(conversationId, in: legacyDirectoryName)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        do {
            let data = try Data(contentsOf: url)
            var cached = try JSONDecoder().decode(CachedTranscript.self, from: data)
            guard (1 ... schemaVersion).contains(cached.schemaVersion), cached.conversationId == conversationId else {
                return nil
            }
            cached.messages = compactMessages(cached.messages)
            cached.tools = compactTools(cached.tools)
            cached.watermark = max(cached.watermark, watermark(from: cached.messages, tools: cached.tools))
            return cached
        } catch {
            HostLog.session.error("Chat cache decode failed")
            return nil
        }
    }

    static func save(_ snapshot: CachedTranscript) {
        let url = fileURL(snapshot.conversationId)
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try JSONEncoder().encode(snapshot)
            try data.write(to: url, options: .atomic)
        } catch {
            HostLog.session.error("Chat cache write failed")
        }
    }

    static func afterSeq(cachedWatermark: UInt64, hostWatermark: UInt64) -> UInt64? {
        if hostWatermark <= cachedWatermark {
            return nil
        }
        if cachedWatermark > 0 {
            return cachedWatermark
        }
        if hostWatermark > tailWindow {
            return hostWatermark - tailWindow
        }
        return 0
    }

    static func watermark(from messages: [ChatMessage], tools: [ToolCard] = []) -> UInt64 {
        let messageMark = messages.compactMap(\.seq).max() ?? 0
        let toolMark = tools.compactMap(\.seq).max() ?? 0
        return max(messageMark, toolMark)
    }

    /// Drop replay copies: same id, same seq, or the same turn text stacked again.
    static func compactMessages(_ messages: [ChatMessage]) -> [ChatMessage] {
        var seenIds = Set<String>()
        var seenSeqs = Set<UInt64>()
        var seenTurns = Set<String>()
        var result: [ChatMessage] = []
        for message in messages {
            if seenIds.contains(message.id) {
                continue
            }
            if let seq = message.seq, seq > 0 {
                if seenSeqs.contains(seq) {
                    continue
                }
                seenSeqs.insert(seq)
            }
            let trimmed = message.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty {
                let turnKey = "\(message.role.rawValue)\n\(trimmed)"
                if seenTurns.contains(turnKey) {
                    continue
                }
                seenTurns.insert(turnKey)
            }
            seenIds.insert(message.id)
            result.append(message)
        }
        return result
    }

    static func compactTools(_ tools: [ToolCard]) -> [ToolCard] {
        var seen = Set<String>()
        var result: [ToolCard] = []
        for tool in tools.reversed() {
            guard seen.insert(tool.id).inserted else { continue }
            var next = tool
            if next.isBusy {
                next.status = "completed"
            }
            result.append(next)
        }
        return result.reversed()
    }

    /// Application Support subdirectory this build reads and writes.
    private static let directoryName = "SeRemote"

    /// Pre-rename subdirectory. `load` falls back to it so transcripts cached
    /// before the rename still open; `save` only ever writes `directoryName`,
    /// so the old tree is copied forward rather than moved, and never deleted.
    private static let legacyDirectoryName = "TermulRemote"

    private static func fileURL(_ conversationId: String, in directory: String = directoryName) -> URL {
        let root = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory())
        let safe = conversationId.replacingOccurrences(of: "/", with: "_")
        return root
            .appendingPathComponent(directory, isDirectory: true)
            .appendingPathComponent("chat-cache", isDirectory: true)
            .appendingPathComponent("\(safe).json")
    }
}
