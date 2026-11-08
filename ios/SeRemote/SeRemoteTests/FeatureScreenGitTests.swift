import Foundation
import Testing
@testable import SeRemote

@Suite("Git screen presentation")
struct FeatureScreenGitTests {
    @Test("Current host word statuses have both semantic text and a symbol", arguments: [
        ("modified", "Modified", "pencil.circle"),
        ("added", "Added", "plus.circle"),
        ("deleted", "Deleted", "minus.circle"),
        ("renamed", "Renamed", "arrow.right.circle"),
        ("copied", "Copied", "doc.on.doc"),
        ("untracked", "Untracked", "questionmark.circle"),
        ("unmerged", "Conflict", "exclamationmark.triangle")
    ])
    func normalizedStatus(value: (String, String, String)) {
        let result = GitChangePresentation.status(value.0, staged: false)
        #expect(result.title == value.1)
        #expect(result.symbol == value.2)
    }

    @Test("Porcelain columns select index versus worktree independently")
    func indexAndWorktree() {
        #expect(GitChangePresentation.status("AM", staged: true).title == "Added")
        #expect(GitChangePresentation.status("AM", staged: false).title == "Modified")
        #expect(GitChangePresentation.status(" D", staged: false).title == "Deleted")
        #expect(GitChangePresentation.status("R ", staged: true).title == "Renamed")
        #expect(GitChangePresentation.status("T", staged: false).title == "Type changed")
    }

    @Test("All conflict pairs remain explicit", arguments: ["DD", "AU", "UD", "UA", "DU", "AA", "UU"])
    func conflicts(code: String) {
        #expect(GitChangePresentation.status(code, staged: true).title == "Conflict")
        #expect(GitChangePresentation.status(code, staged: false).title == "Conflict")
    }

    @Test("Untracked ignored empty and unknown codes do not become modified")
    func unknownStatuses() {
        #expect(GitChangePresentation.status("??", staged: false).title == "Untracked")
        #expect(GitChangePresentation.status("!!", staged: false).title == "Ignored")
        #expect(GitChangePresentation.status("", staged: false).title == "Unknown change")
        #expect(GitChangePresentation.status("future-status", staged: false).title == "Unknown (future-status)")
    }

    @Test("Replacement alignment preserves real line gutters and missing counterparts")
    func splitGutters() throws {
        let diff = UnifiedDiff.parse("""
        --- a/source.swift
        +++ b/source.swift
        @@ -10,3 +10,4 @@
         context
        -before
        +after
        +extra
         tail
        """)
        let hunk = try #require(diff.files.first?.hunks.first)
        let rows = GitDiffPresentation.splitRows(hunk.lines)
        #expect(rows.count == 4)
        #expect(rows[0].old?.oldLineNumber == 10)
        #expect(rows[0].new?.newLineNumber == 10)
        #expect(rows[1].old?.content == "before")
        #expect(rows[1].old?.oldLineNumber == 11)
        #expect(rows[1].old?.newLineNumber == nil)
        #expect(rows[1].new?.content == "after")
        #expect(rows[1].new?.newLineNumber == 11)
        #expect(rows[2].old == nil)
        #expect(rows[2].new?.newLineNumber == 12)
        #expect(rows[3].old?.oldLineNumber == 12)
        #expect(rows[3].new?.newLineNumber == 13)
        let counts = GitDiffPresentation.counts(diff)
        #expect(counts.added == 2)
        #expect(counts.removed == 1)
    }

    @Test("New files have no old gutter and empty split input stays empty")
    func newFile() throws {
        let diff = UnifiedDiff.parse("""
        --- /dev/null
        +++ b/new.txt
        @@ -0,0 +1,2 @@
        +first
        +second
        """)
        let hunk = try #require(diff.files.first?.hunks.first)
        let rows = GitDiffPresentation.splitRows(hunk.lines)
        #expect(rows.count == 2)
        #expect(rows.allSatisfy { $0.old == nil })
        #expect(rows.map { $0.new?.newLineNumber } == [1, 2])
        #expect(GitDiffPresentation.splitRows([]).isEmpty)
        #expect(GitDiffPresentation.gutter(nil).isEmpty)
        #expect(GitDiffPresentation.gutter(1204) == "1204")
    }

    @Test("Deleted files have no new gutter and no-newline annotations stay on source")
    func deletionAndNewline() throws {
        let diff = UnifiedDiff.parse("""
        --- a/gone.txt
        +++ /dev/null
        @@ -7 +0,0 @@
        -last
        \\ No newline at end of file
        """)
        let hunk = try #require(diff.files.first?.hunks.first)
        let row = try #require(GitDiffPresentation.splitRows(hunk.lines).first)
        #expect(row.new == nil)
        #expect(row.old?.oldLineNumber == 7)
        #expect(row.old?.noNewlineAtEnd == true)
        #expect(row.old.map { GitDiffPresentation.prefix(for: $0) } == "−")
        #expect(GitDiffPresentation.counts(diff).removed == 1)
    }

    @Test("Binary and rename metadata do not produce changed-line counts or fake gutters")
    func metadata() {
        let diff = UnifiedDiff.parse("""
        diff --git a/old.png b/new.png
        similarity index 100%
        rename from old.png
        rename to new.png
        Binary files a/old.png and b/new.png differ
        """)
        #expect(GitDiffPresentation.counts(diff).added == 0)
        #expect(GitDiffPresentation.counts(diff).removed == 0)
        #expect(diff.files.first?.hunks.isEmpty == true)
        let header = UnifiedDiffLine.header("@@ -1 +1 @@")
        #expect(GitDiffPresentation.gutter(header.oldLineNumber).isEmpty)
        #expect(GitDiffPresentation.prefix(for: header).isEmpty)
    }

    @Test("Split requires the actual detail budget and falls back for accessibility text")
    func splitBudget() {
        #expect(!GitDiffPresentation.usesSplit(detailWidth: 984, minimumWidth: 1000, largeText: false))
        #expect(!GitDiffPresentation.usesSplit(detailWidth: 999, minimumWidth: 1000, largeText: false))
        #expect(GitDiffPresentation.usesSplit(detailWidth: 1000, minimumWidth: 1000, largeText: false))
        #expect(GitDiffPresentation.usesSplit(detailWidth: 1200, minimumWidth: 1000, largeText: false))
        #expect(!GitDiffPresentation.usesSplit(detailWidth: 1200, minimumWidth: 1000, largeText: true))
    }

    @Test("ISO dates support both host timestamp precisions and invalid fallback")
    func dates() {
        #expect(GitDiffPresentation.date("2026-11-08T12:00:00+08:00") != nil)
        #expect(GitDiffPresentation.date("2026-11-08T12:00:00.123Z") != nil)
        #expect(GitDiffPresentation.date("not-a-date") == nil)
    }
}
