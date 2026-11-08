import Foundation

struct SearchFileGroup: Identifiable, Equatable {
    let filePath: String
    let matches: [FileSearchMatch]
    var id: String { filePath }
}

/// A deterministic presentation layer: merge repeated file entries and preserve
/// host order, then sort line numbers for predictable VoiceOver navigation.
enum SearchResultPresentation {
    static func groups(_ results: [FileSearchResult]) -> [SearchFileGroup] {
        var order: [String] = []
        var matches: [String: [FileSearchMatch]] = [:]
        for result in results {
            if matches[result.filePath] == nil { order.append(result.filePath) }
            matches[result.filePath, default: []].append(contentsOf: result.matches)
        }
        return order.map { path in
            SearchFileGroup(filePath: path,
                matches: Array(Set(matches[path] ?? [])).sorted {
                    if $0.lineNumber == $1.lineNumber { return $0.lineText < $1.lineText }
                    return $0.lineNumber < $1.lineNumber
                })
        }
    }

    /// Match the host's fixed-string, case-insensitive search. String ranges
    /// remain valid for Unicode, rather than mixing byte and UTF-16 offsets.
    static func highlightRanges(in line: String, query: String) -> [Range<String.Index>] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return [] }
        var ranges: [Range<String.Index>] = []
        var start = line.startIndex
        while start < line.endIndex,
              let range = line.range(of: needle, options: [.caseInsensitive], range: start..<line.endIndex) {
            ranges.append(range)
            start = range.upperBound
        }
        return ranges
    }
}
