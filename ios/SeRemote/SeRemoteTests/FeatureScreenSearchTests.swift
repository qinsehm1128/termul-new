import Foundation
import Testing
@testable import SeRemote

@MainActor
struct FeatureScreenSearchTests {
    @Test func groupingMergesFilesAndOrdersLines() {
        let two = FileSearchMatch(lineNumber: 2, lineText: "two")
        let one = FileSearchMatch(lineNumber: 1, lineText: "one")
        let groups = SearchResultPresentation.groups([
            FileSearchResult(filePath: "/root/b.swift", matches: [two]),
            FileSearchResult(filePath: "/root/a.swift", matches: [one]),
            FileSearchResult(filePath: "/root/b.swift", matches: [one, two])
        ])
        #expect(groups.map(\.filePath) == ["/root/b.swift", "/root/a.swift"])
        #expect(groups[0].matches.map(\.lineNumber) == [1, 2])
        #expect(groups[1].matches == [one])
    }

    @Test func highlightingIsLiteralCaseInsensitiveAndNonoverlapping() {
        let line = "a.b A.B axb a.b"
        let ranges = SearchResultPresentation.highlightRanges(in: line, query: "a.b")
        #expect(ranges.map { String(line[$0]) } == ["a.b", "A.B", "a.b"])
        let overlapping = "aaa"
        #expect(SearchResultPresentation.highlightRanges(in: overlapping, query: "aa").count == 1)
    }

    @Test func highlightingHandlesUnicodeAndWhitespaceLikeHostQuery() {
        let line = "🪴 CAFÉ café 路径"
        let ranges = SearchResultPresentation.highlightRanges(in: line, query: "  café\n")
        #expect(ranges.map { String(line[$0]) } == ["CAFÉ", "café"])
        #expect(SearchResultPresentation.highlightRanges(in: line, query: " \n").isEmpty)
        #expect(SearchResultPresentation.groups([]).isEmpty)
    }
}
