import Foundation
import Testing
@testable import SeRemote

@Suite("Git feature")
@MainActor
struct GitFeatureTests {
    @Test("Git wire payloads use camelCase and preserve unknown porcelain codes")
    func wirePayloads() throws {
        let decoder = JSONDecoder()
        let status = try decoder.decode([GitStatusDetailWire].self, from: Data(#"[{"path":"src/a.swift","status":"??","staged":false,"future":1}]"#.utf8))
        #expect(status == [GitStatusDetailWire(path: "src/a.swift", status: "??", staged: false)])
        let commits = try decoder.decode([GitCommitWire].self, from: Data(#"[{"hash":"abcdef","shortHash":"abc","parents":[],"refs":["HEAD -> main"],"author":"A","date":"2026-11-08","subject":"hello"}]"#.utf8))
        #expect(commits.first?.shortHash == "abc")
        #expect(commits.first?.parents == [])
        #expect(commits.first?.refs == ["HEAD -> main"])
        let stashes = try decoder.decode([GitStashInfoWire].self, from: Data(#"[{"index":0,"name":"stash@{0}","message":"WIP"}]"#.utf8))
        #expect(stashes.first?.name == "stash@{0}")
        #expect(Set(status).count == 1)
    }

    @Test("Detached/unborn context tolerates null or omitted optional branch", arguments: ["null", "omitted"])
    func contextOptionalBranch(form: String) throws {
        let branch = form == "null" ? #""branch":null,"# : ""
        let data = Data(("{" + branch + #""hasUpstream":false,"ahead":0,"behind":0,"stagedCount":0,"hasHead":false,"lastSubject":"","lastBody":"","future":"ignored"}"#).utf8)
        let context = try JSONDecoder().decode(GitCommitContextWire.self, from: data)
        #expect(context.branch == nil)
        #expect(!context.hasHead)
        #expect(context.lastBody.isEmpty)
    }

    @Test("Diff envelope contains a JSON string, not a nested DTO")
    func diffEnvelope() throws {
        let result = try JSONDecoder().decode(IpcResult<String>.self, from: Data(#"{"success":true,"data":"--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n"}"#.utf8))
        #expect(result.success)
        #expect(UnifiedDiff.parse(try #require(result.data)).files.count == 1)
    }

    @Test("Branch list decodes raw local, remote, symbolic and detached lines")
    func rawBranches() throws {
        let branches = try JSONDecoder().decode([GitBranch].self, from: Data(#"["* main","  remotes/origin/main","  remotes/origin/HEAD -> origin/main","+ topic","* (HEAD detached at abc123)"]"#.utf8))
        #expect(branches[0].name == "main")
        #expect(branches[0].isCurrent)
        #expect(!branches[0].isRemote)
        #expect(branches[1].isRemote)
        #expect(branches[2].name == "remotes/origin/HEAD")
        #expect(branches[2].symbolicTarget == "origin/main")
        #expect(branches[3].isCheckedOutElsewhere)
        #expect(!branches[3].isCurrent)
        #expect(branches[4].name == "(HEAD detached at abc123)")
        #expect(branches[2].raw == "  remotes/origin/HEAD -> origin/main")
    }

    @Test("Multi-file multi-hunk diff numbers context, removals and additions independently")
    func multipleHunks() throws {
        let diff = UnifiedDiff.parse("""
        diff --git a/a.swift b/a.swift
        index 111..222 100644
        --- a/a.swift
        +++ b/a.swift
        @@ -2,3 +2,4 @@ function
         same
        -old
        +new
        +extra
         tail
        @@ -10 +11 @@
        -before
        +after
        diff --git a/b.swift b/b.swift
        --- a/b.swift
        +++ b/b.swift
        @@ -1 +1 @@
        -x
        +y
        """)
        #expect(diff.files.count == 2)
        let file = try #require(diff.files.first)
        #expect(file.path == "a.swift")
        #expect(file.headers.allSatisfy { $0.kind == .header })
        #expect(file.hunks.count == 2)
        let hunk = file.hunks[0]
        #expect(hunk.header.kind == .header)
        #expect(hunk.header.content == "@@ -2,3 +2,4 @@ function")
        #expect(hunk.lines.map(\.kind) == [.context, .removal, .addition, .addition, .context])
        #expect(hunk.lines.map(\.oldLineNumber) == [2, 3, nil, nil, 4])
        #expect(hunk.lines.map(\.newLineNumber) == [2, nil, 3, 4, 5])
        #expect(hunk.lines.map(\.content) == ["same", "old", "new", "extra", "tail"])
        #expect(file.hunks[1].oldCount == 1)
        #expect(file.hunks[1].newCount == 1)
        #expect(file.hunks[1].lines.first?.oldLineNumber == 10)
        #expect(file.hunks[1].lines.last?.newLineNumber == 11)
        #expect(diff.files[1].hunks[0].lines.last?.newLineNumber == 1)
    }

    @Test("New and deleted files use /dev/null and zero-length ranges")
    func newAndDeleted() {
        let diff = UnifiedDiff.parse("""
        diff --git a/new.txt b/new.txt
        new file mode 100644
        --- /dev/null
        +++ b/new.txt
        @@ -0,0 +1,2 @@
        +hello
        +
        diff --git a/gone.txt b/gone.txt
        deleted file mode 100644
        --- a/gone.txt
        +++ /dev/null
        @@ -1,2 +0,0 @@
        -goodbye
        -
        """)
        #expect(diff.files.count == 2)
        #expect(diff.files[0].isNew)
        #expect(diff.files[0].oldPath == nil)
        #expect(diff.files[0].hunks[0].lines.map(\.newLineNumber) == [1, 2])
        #expect(diff.files[0].hunks[0].lines.allSatisfy { $0.oldLineNumber == nil })
        #expect(diff.files[1].isDeleted)
        #expect(diff.files[1].newPath == nil)
        #expect(diff.files[1].hunks[0].lines.map(\.oldLineNumber) == [1, 2])
        #expect(diff.files[1].hunks[0].lines.allSatisfy { $0.newLineNumber == nil })
    }

    @Test("No-newline markers annotate their previous line without consuming line numbers")
    func noNewline() throws {
        let diff = UnifiedDiff.parse("""
        --- a/a
        +++ b/a
        @@ -1 +1 @@
        -old
        \\ No newline at end of file
        +new
        \\ No newline at end of file
        """)
        let lines = try #require(diff.files.first?.hunks.first?.lines)
        #expect(lines.count == 2)
        #expect(lines[0].noNewlineAtEnd && lines[0].oldNoNewline && !lines[0].newNoNewline)
        #expect(lines[1].noNewlineAtEnd && !lines[1].oldNoNewline && lines[1].newNoNewline)
        #expect(lines[0].oldLineNumber == 1)
        #expect(lines[1].newLineNumber == 1)
        let context = UnifiedDiff.parse("--- a/a\n+++ b/a\n@@ -1 +1 @@\n same\n\\ No newline at end of file\n")
        #expect(context.files[0].hunks[0].lines[0].oldNoNewline)
        #expect(context.files[0].hunks[0].lines[0].newNoNewline)
    }

    @Test("File-like content inside a hunk is not mistaken for a header")
    func headerLikeContent() {
        let diff = UnifiedDiff.parse("--- a/a\n+++ b/a\n@@ -1 +1 @@\n--- old\n+++ new\n")
        #expect(diff.files.count == 1)
        #expect(diff.files[0].hunks[0].lines.map(\.content) == ["-- old", "++ new"])
        #expect(diff.files[0].oldPath == "a")
    }

    @Test("Plain unified files, quoted Git paths, binary and rename-only headers remain usable")
    func pathsAndMetadata() {
        let plain = UnifiedDiff.parse("--- a/one\n+++ b/one\n@@ -1 +1 @@\n-a\n+b\n--- a/two\n+++ b/two\n@@ -1 +1 @@\n-c\n+d\n")
        #expect(plain.files.map(\.path) == ["one", "two"])
        let quoted = UnifiedDiff.parse(#"diff --git "a/\303\251 file\t.txt" "b/\303\251 file\t.txt""# + "\n" + #"--- "a/\303\251 file\t.txt""# + "\n" + #"+++ "b/\303\251 file\t.txt""#)
        #expect(quoted.files[0].path == "é file\t.txt")
        let metadata = UnifiedDiff.parse("diff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\ndiff --git a/icon b/icon\nBinary files a/icon and b/icon differ\n")
        #expect(metadata.files[0].oldPath == "old name")
        #expect(metadata.files[0].newPath == "new name")
        #expect(metadata.files[0].hunks.isEmpty)
        #expect(metadata.files[1].isBinary)
        let binaryModes = UnifiedDiff.parse("diff --git a/new.png b/new.png\nnew file mode 100644\nBinary files /dev/null and b/new.png differ\ndiff --git a/gone.png b/gone.png\ndeleted file mode 100644\nBinary files a/gone.png and /dev/null differ\n")
        #expect(binaryModes.files[0].isNew && binaryModes.files[0].isBinary)
        #expect(binaryModes.files[1].isDeleted && binaryModes.files[1].isBinary)
    }

    @Test("Empty, malformed and combined diffs do not invent numbered lines")
    func emptyAndMalformed() {
        #expect(UnifiedDiff.parse("").files.isEmpty)
        #expect(UnifiedDiff.parse("\n").files.isEmpty)
        let malformed = UnifiedDiff.parse("diff --git a/a b/a\n@@ -bad +1 @@\n@@@ -1 -1 +1 @@@\n+x\n")
        #expect(malformed.files[0].hunks.isEmpty)
        #expect(malformed.files[0].headers.allSatisfy { $0.oldLineNumber == nil && $0.newLineNumber == nil })
    }

    @Test("Store transitions through loading, loaded, clean worktree and failed states")
    func storeRefresh() async {
        let fake = GitFakeReader()
        let store = GitStore()
        store.attach(client: fake)
        #expect(store.state == .idle)
        fake.onStatus = { _ in
            #expect(store.state == .loading)
            return [GitStatusDetailWire(path: "a", status: "M", staged: true)]
        }
        await store.refresh(cwd: "/repo")
        #expect(store.state == .loaded)
        #expect(store.status.count == 1)
        #expect(store.context?.branch == "main")
        #expect(fake.calls == ["status:/repo", "log:/repo", "context:/repo", "branches:/repo", "stashes:/repo"])
        fake.onStatus = nil
        await store.refresh()
        #expect(store.state == .empty)
        #expect(store.status.isEmpty)
        fake.onStatus = { _ in throw GitTestError.unavailable }
        await store.refresh()
        guard case .failed = store.state else { Issue.record("Expected refresh failure"); return }
    }

    @Test("Selected diff receives cwd/path/staged and clears stale content on failure")
    func storeDiff() async {
        let fake = GitFakeReader()
        let store = GitStore()
        store.attach(client: fake)
        await store.refresh(cwd: "/repo")
        fake.diffText = "--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n"
        await store.loadDiff(path: "a", staged: true)
        #expect(store.diffState == .loaded)
        #expect(store.diff.files.count == 1)
        #expect(store.rawDiff == fake.diffText)
        #expect(store.diffPath == "a")
        #expect(store.diffStaged)
        #expect(fake.lastDiff?.cwd == "/repo")
        #expect(fake.lastDiff?.path == "a")
        #expect(fake.lastDiff?.staged == true)
        fake.diffText = ""
        await store.loadDiff(path: "b")
        #expect(store.diffState == .empty)
        #expect(store.diff.files.isEmpty)
        fake.diffError = GitTestError.unavailable
        await store.loadDiff(path: "c")
        guard case .failed = store.diffState else { Issue.record("Expected diff failure"); return }
        #expect(store.rawDiff.isEmpty)
        #expect(store.diff.files.isEmpty)
    }

    @Test("An old workspace response cannot overwrite a newer refresh")
    func staleRefresh() async {
        let fake = GitFakeReader()
        let store = GitStore()
        store.attach(client: fake)
        var suspended: CheckedContinuation<[GitStatusDetailWire], Never>?
        fake.onStatus = { cwd in
            if cwd == "/old" {
                return await withCheckedContinuation { suspended = $0 }
            }
            return []
        }
        let old = Task { await store.refresh(cwd: "/old") }
        while suspended == nil { await Task.yield() }
        await store.refresh(cwd: "/new")
        suspended?.resume(returning: [GitStatusDetailWire(path: "old", status: "M", staged: false)])
        await old.value
        #expect(store.cwd == "/new")
        #expect(store.status.isEmpty)
        #expect(store.state == .empty)
    }

    @Test("Reattaching a host clears snapshot and ignores in-flight diffs")
    func attachInvalidatesDiff() async {
        let fake = GitFakeReader()
        let store = GitStore()
        store.attach(client: fake)
        await store.refresh(cwd: "/repo")
        var suspended: CheckedContinuation<String, Never>?
        fake.onDiff = { await withCheckedContinuation { suspended = $0 } }
        let pending = Task { await store.loadDiff(path: "a") }
        while suspended == nil { await Task.yield() }
        #expect(store.diffState == .loading)
        store.attach(client: GitFakeReader())
        suspended?.resume(returning: "old host diff")
        await pending.value
        #expect(store.cwd == nil)
        #expect(store.context == nil)
        #expect(store.state == .idle)
        #expect(store.diffState == .idle)
        #expect(store.rawDiff.isEmpty)
    }
}

private enum GitTestError: Error { case unavailable }

@MainActor
private final class GitFakeReader: GitReading {
    var calls: [String] = []
    var onStatus: ((String) async throws -> [GitStatusDetailWire])?
    var onDiff: (() async throws -> String)?
    var diffText = ""
    var diffError: (any Error)?
    var lastDiff: (cwd: String, path: String, staged: Bool)?

    func status(cwd: String) async throws -> [GitStatusDetailWire] {
        calls.append("status:\(cwd)")
        return try await onStatus?(cwd) ?? []
    }

    func log(cwd: String, limit: Int?) async throws -> [GitCommitWire] {
        calls.append("log:\(cwd)")
        return []
    }

    func commitContext(cwd: String) async throws -> GitCommitContextWire {
        calls.append("context:\(cwd)")
        return GitCommitContextWire(branch: "main", hasUpstream: false, ahead: 0, behind: 0,
                                    stagedCount: 0, hasHead: false, lastSubject: "", lastBody: "")
    }

    func branches(cwd: String) async throws -> [GitBranch] {
        calls.append("branches:\(cwd)")
        return []
    }

    func stashes(cwd: String) async throws -> [GitStashInfoWire] {
        calls.append("stashes:\(cwd)")
        return []
    }

    func diff(cwd: String, path: String, staged: Bool) async throws -> String {
        lastDiff = (cwd, path, staged)
        if let diffError { throw diffError }
        return try await onDiff?() ?? diffText
    }
}
