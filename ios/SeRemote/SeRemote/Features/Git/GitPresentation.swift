import Foundation

/// The current host sends words ("modified"); older hosts may send porcelain
/// columns. Preserve unfamiliar codes instead of silently calling them modified.
struct GitChangePresentation: Equatable, Sendable {
    let symbol: String
    let title: String

    static func status(_ raw: String, staged: Bool) -> Self {
        let word = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        switch word {
        case "added": return .init(symbol: "plus.circle", title: "Added")
        case "modified": return .init(symbol: "pencil.circle", title: "Modified")
        case "deleted": return .init(symbol: "minus.circle", title: "Deleted")
        case "renamed": return .init(symbol: "arrow.right.circle", title: "Renamed")
        case "copied": return .init(symbol: "doc.on.doc", title: "Copied")
        case "untracked": return .init(symbol: "questionmark.circle", title: "Untracked")
        case "unmerged", "conflicted": return .init(symbol: "exclamationmark.triangle", title: "Conflict")
        default: break
        }
        let columns = Array(raw)
        let conflicts = ["DD", "AU", "UD", "UA", "DU", "AA", "UU"]
        if conflicts.contains(raw) || raw == "U" {
            return .init(symbol: "exclamationmark.triangle", title: "Conflict")
        }
        if raw == "??" { return .init(symbol: "questionmark.circle", title: "Untracked") }
        if raw == "!!" { return .init(symbol: "eye.slash", title: "Ignored") }
        let code = columns.count == 2 ? columns[staged ? 0 : 1] : columns.first
        switch code {
        case "A": return .init(symbol: "plus.circle", title: "Added")
        case "M": return .init(symbol: "pencil.circle", title: "Modified")
        case "D": return .init(symbol: "minus.circle", title: "Deleted")
        case "R": return .init(symbol: "arrow.right.circle", title: "Renamed")
        case "C": return .init(symbol: "doc.on.doc", title: "Copied")
        case "T": return .init(symbol: "arrow.triangle.swap", title: "Type changed")
        case "U": return .init(symbol: "exclamationmark.triangle", title: "Conflict")
        case "?": return .init(symbol: "questionmark.circle", title: "Untracked")
        default: return .init(symbol: "questionmark.square", title: word.isEmpty ? "Unknown change" : "Unknown (\(raw))")
        }
    }
}

/// Formatting and alignment are independent of SwiftUI and never invent gutters.
enum GitDiffPresentation {
    struct SplitRow: Equatable {
        let old: UnifiedDiffLine?
        let new: UnifiedDiffLine?
    }

    static func usesSplit(detailWidth: CGFloat, minimumWidth: CGFloat, largeText: Bool) -> Bool {
        detailWidth >= minimumWidth && !largeText
    }

    static func prefix(for line: UnifiedDiffLine) -> String {
        switch line.kind {
        case .addition: "+"
        case .removal: "−"
        case .context: " "
        case .header: ""
        }
    }

    static func gutter(_ number: Int?) -> String { number.map(String.init) ?? "" }

    static func counts(_ diff: UnifiedDiff) -> (added: Int, removed: Int) {
        let lines = diff.files.flatMap(\.hunks).flatMap(\.lines)
        return (lines.filter { $0.kind == .addition }.count,
                lines.filter { $0.kind == .removal }.count)
    }

    static func splitRows(_ lines: [UnifiedDiffLine]) -> [SplitRow] {
        var result: [SplitRow] = []
        var removed: [UnifiedDiffLine] = []
        var added: [UnifiedDiffLine] = []
        func flush() {
            for index in 0..<max(removed.count, added.count) {
                result.append(SplitRow(old: index < removed.count ? removed[index] : nil,
                                       new: index < added.count ? added[index] : nil))
            }
            removed.removeAll(keepingCapacity: true)
            added.removeAll(keepingCapacity: true)
        }
        for line in lines {
            switch line.kind {
            case .removal:
                // A second replacement block after additions must not get paired
                // back into the preceding block.
                if !added.isEmpty { flush() }
                removed.append(line)
            case .addition: added.append(line)
            case .context:
                flush()
                result.append(SplitRow(old: line, new: line))
            case .header:
                flush()
                result.append(SplitRow(old: line, new: line))
            }
        }
        flush()
        return result
    }

    static func date(_ raw: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: raw) ?? ISO8601DateFormatter().date(from: raw)
    }
}
