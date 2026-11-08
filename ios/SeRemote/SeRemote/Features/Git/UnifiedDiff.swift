import Foundation

struct UnifiedDiff: Decodable, Sendable, Hashable {
    let files: [UnifiedDiffFile]

    static func parse(_ text: String) -> UnifiedDiff {
        UnifiedDiffParser.parse(text)
    }
}

struct UnifiedDiffFile: Decodable, Sendable, Hashable {
    var oldPath: String?
    var newPath: String?
    var headers: [UnifiedDiffLine] = []
    var hunks: [UnifiedDiffHunk] = []

    var path: String { newPath ?? oldPath ?? "" }
    var isNew: Bool { oldPath == nil && newPath != nil }
    var isDeleted: Bool { oldPath != nil && newPath == nil }
    var isBinary: Bool {
        headers.contains { $0.content.hasPrefix("Binary files ") || $0.content == "GIT binary patch" }
    }
}

struct UnifiedDiffHunk: Decodable, Sendable, Hashable {
    let header: UnifiedDiffLine
    let oldStart: Int
    let oldCount: Int
    let newStart: Int
    let newCount: Int
    var lines: [UnifiedDiffLine] = []
}

struct UnifiedDiffLine: Decodable, Sendable, Hashable {
    enum Kind: String, Decodable, Sendable, Hashable {
        case context, addition, removal, header
    }

    let kind: Kind
    /// Content without the unified-diff prefix; headers retain their full text.
    let content: String
    let oldLineNumber: Int?
    let newLineNumber: Int?
    var noNewlineAtEnd = false

    var oldNoNewline: Bool { noNewlineAtEnd && (kind == .context || kind == .removal) }
    var newNoNewline: Bool { noNewlineAtEnd && (kind == .context || kind == .addition) }

    static func header(_ content: String) -> Self {
        Self(kind: .header, content: content, oldLineNumber: nil, newLineNumber: nil)
    }
}

/// Parses ordinary two-sided unified diffs. Binary/rename metadata and unknown
/// headers are retained rather than misrepresented as changed source lines.
/// Combined (`@@@`) merge diffs are retained as headers, not numbered hunks.
enum UnifiedDiffParser {
    static func parse(_ text: String) -> UnifiedDiff {
        var files: [UnifiedDiffFile] = []
        var oldLine = 0
        var newLine = 0
        var oldRemaining = 0
        var newRemaining = 0
        var rows = text.components(separatedBy: "\n")
        if rows.last == "" { rows.removeLast() } // Do not invent a context line at EOF.

        for (index, row) in rows.enumerated() {
            if row.hasPrefix("diff --git ") {
                let paths = gitPaths(String(row.dropFirst(11)))
                files.append(UnifiedDiffFile(oldPath: paths.0, newPath: paths.1))
                oldRemaining = 0
                newRemaining = 0
            } else if row.hasPrefix("--- "), oldRemaining == 0, newRemaining == 0,
                      index + 1 < rows.count, rows[index + 1].hasPrefix("+++ "),
                      files.last?.hunks.isEmpty == false {
                // Plain unified diffs need not include a `diff --git` separator.
                files.append(UnifiedDiffFile())
            }
            if files.isEmpty {
                guard !row.isEmpty else { continue }
                files.append(UnifiedDiffFile())
            }
            let file = files.count - 1
            if let hunk = hunkHeader(row) {
                files[file].hunks.append(hunk)
                oldLine = hunk.oldStart
                newLine = hunk.newStart
                oldRemaining = hunk.oldCount
                newRemaining = hunk.newCount
                continue
            }
            if row == "\\ No newline at end of file", let lastHunk = files[file].hunks.indices.last,
               let lastLine = files[file].hunks[lastHunk].lines.indices.last {
                files[file].hunks[lastHunk].lines[lastLine].noNewlineAtEnd = true
                continue
            }
            if let hunk = files[file].hunks.indices.last {
                let kind: UnifiedDiffLine.Kind?
                switch row.first {
                case " " where oldRemaining > 0 && newRemaining > 0: kind = .context
                case "-" where oldRemaining > 0: kind = .removal
                case "+" where newRemaining > 0: kind = .addition
                default: kind = nil
                }
                if let kind {
                    let hasOld = kind != .addition
                    let hasNew = kind != .removal
                    files[file].hunks[hunk].lines.append(UnifiedDiffLine(
                        kind: kind, content: String(row.dropFirst()),
                        oldLineNumber: hasOld ? oldLine : nil,
                        newLineNumber: hasNew ? newLine : nil
                    ))
                    if hasOld { oldLine += 1; oldRemaining -= 1 }
                    if hasNew { newLine += 1; newRemaining -= 1 }
                    continue
                }
            }
            if row.hasPrefix("--- ") {
                files[file].oldPath = path(String(row.dropFirst(4)), prefix: "a/")
            } else if row.hasPrefix("+++ ") {
                files[file].newPath = path(String(row.dropFirst(4)), prefix: "b/")
            } else if row.hasPrefix("new file mode ") {
                files[file].oldPath = nil
            } else if row.hasPrefix("deleted file mode ") {
                files[file].newPath = nil
            }
            files[file].headers.append(.header(row))
        }
        return UnifiedDiff(files: files)
    }

    private static func hunkHeader(_ row: String) -> UnifiedDiffHunk? {
        let fields = row.split(separator: " ", omittingEmptySubsequences: true)
        guard fields.count >= 4, fields[0] == "@@", fields[3] == "@@",
              let old = range(fields[1], sign: "-"), let new = range(fields[2], sign: "+") else { return nil }
        return UnifiedDiffHunk(header: .header(row), oldStart: old.0, oldCount: old.1,
                               newStart: new.0, newCount: new.1)
    }

    private static func range(_ field: Substring, sign: Character) -> (Int, Int)? {
        guard field.first == sign else { return nil }
        let numbers = field.dropFirst().split(separator: ",", omittingEmptySubsequences: false)
        guard (1...2).contains(numbers.count), let start = Int(numbers[0]), start >= 0,
              let count = numbers.count == 2 ? Int(numbers[1]) : 1, count >= 0,
              start <= Int.max - count else { return nil }
        return (start, count)
    }

    private static func gitPaths(_ header: String) -> (String?, String?) {
        if header.hasPrefix("\"") {
            let end = quotedEnd(header)
            let first = String(header[...end])
            let second = String(header[header.index(after: end)...]).trimmingCharacters(in: .whitespaces)
            return (path(first, prefix: "a/"), path(second, prefix: "b/"))
        }
        // Git leaves spaces unquoted; file markers below are authoritative.
        guard let split = header.range(of: " b/", options: .backwards) else { return (nil, nil) }
        return (path(String(header[..<split.lowerBound]), prefix: "a/"),
                path(String(header[header.index(after: split.lowerBound)...]), prefix: "b/"))
    }

    private static func path(_ raw: String, prefix: String) -> String? {
        // Timestamps in plain unified headers are tab separated.
        let value: String
        if raw.hasPrefix("\"") {
            value = unquote(String(raw[...quotedEnd(raw)]))
        } else {
            value = String(raw.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)[0])
        }
        if value == "/dev/null" { return nil }
        return value.hasPrefix(prefix) ? String(value.dropFirst(2)) : value
    }

    private static func quotedEnd(_ value: String) -> String.Index {
        var escaped = false
        for index in value.indices.dropFirst() {
            if escaped { escaped = false; continue }
            if value[index] == "\\" { escaped = true }
            else if value[index] == "\"" { return index }
        }
        return value.index(before: value.endIndex)
    }

    /// Git core.quotePath uses C escapes and octal UTF-8 bytes.
    private static func unquote(_ value: String) -> String {
        let bytes = Array(value.utf8)
        guard bytes.count >= 2, bytes.first == 34, bytes.last == 34 else { return value }
        var result: [UInt8] = []
        var index = 1
        while index < bytes.count - 1 {
            let byte = bytes[index]
            index += 1
            guard byte == 92, index < bytes.count - 1 else { result.append(byte); continue }
            let escaped = bytes[index]
            index += 1
            if (48...55).contains(escaped) {
                var octal = Int(escaped - 48)
                var count = 1
                while count < 3, index < bytes.count - 1, (48...55).contains(bytes[index]) {
                    octal = octal * 8 + Int(bytes[index] - 48)
                    index += 1
                    count += 1
                }
                result.append(UInt8(truncatingIfNeeded: octal))
            } else {
                let escapes: [UInt8: UInt8] = [97: 7, 98: 8, 102: 12, 110: 10, 114: 13, 116: 9, 118: 11]
                result.append(escapes[escaped] ?? escaped)
            }
        }
        return String(decoding: result, as: UTF8.self)
    }
}
