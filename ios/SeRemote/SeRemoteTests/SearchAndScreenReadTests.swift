import Foundation
import Testing
@testable import SeRemote

@MainActor
struct SearchFeatureTests {
    @Test func decodesSearchModels() throws {
        let json = #"{"results":[{"filePath":"/p/a.swift","matches":[{"lineNumber":3,"lineText":"Hello"}]}],"truncated":true,"scannedFiles":0,"failedFiles":0}"#
        let reply = try JSONDecoder().decode(FileSearchResponse.self, from: Data(json.utf8))
        #expect(reply.truncated)
        #expect(reply.scannedFiles == 0)
        #expect(reply.failedFiles == 0)
        #expect(reply.results.first?.filePath == "/p/a.swift")
        #expect(reply.results.first?.matches == [FileSearchMatch(lineNumber: 3, lineText: "Hello")])
        let info = try JSONDecoder().decode(RgInfo.self, from: Data(#"{"sidecarBinaryName":"rg","resolvedPath":"/bin/rg","source":"sidecar","exists":true}"#.utf8))
        #expect(info.exists)
        #expect(info.source == "sidecar")
        #expect(info.resolvedPath == "/bin/rg")
    }

    @Test func searchBodyHasOnlyContractKeys() throws {
        let body = SearchContentBody(scopeRoot: "/p", rootPath: "/p/src", query: "hello")
        let data = try JSONEncoder().encode(body)
        let fields = try #require(JSONSerialization.jsonObject(with: data) as? [String: String])
        #expect(Set(fields.keys) == ["scopeRoot", "rootPath", "query"])
        #expect(try JSONDecoder().decode(SearchContentBody.self, from: data) == body)
    }

    @Test func validatesHostScalarLimit() throws {
        try SearchQuery.validate("")
        try SearchQuery.validate(" \n ")
        try SearchQuery.validate(String(repeating: "x", count: 500))
        try SearchQuery.validate(" \n" + String(repeating: "x", count: 500) + "\n ")
        try SearchQuery.validate(String(repeating: "🦀", count: 500))
        #expect(throws: SearchQueryError.tooLong) {
            try SearchQuery.validate(String(repeating: "x", count: 501))
        }
        // Two Unicode scalars per grapheme. Rust chars() sees 502, not 251.
        #expect(throws: SearchQueryError.tooLong) {
            try SearchQuery.validate(String(repeating: "e\u{301}", count: 251))
        }
    }

    @Test func emptyAndInvalidSearchNeverNeedAHost() async {
        let store = SearchStore()
        await store.refresh()
        #expect(store.state == .empty)
        store.setQuery(String(repeating: "q", count: 501))
        #expect(store.errorMessage != nil)
        #expect(!store.isLoading)
        store.setQuery("   ")
        #expect(store.state == .empty)
        #expect(store.results.isEmpty)
    }

    @Test func canceledRefreshSettlesWhenResponseAlreadyArrived() async throws {
        let client = SuspendingSearchClient()
        let store = SearchStore()
        store.attach(client: client)
        store.configure(scopeRoot: "/p", rootPath: "/p")
        store.setQuery("needle", debounce: .seconds(3600))
        let refresh = Task { await store.refresh() }
        while client.continuation == nil { await Task.yield() }
        #expect(store.isLoading)
        // Cancel first, then let the already-received response complete.
        refresh.cancel()
        let reply = FileSearchResponse(results: [], truncated: false, scannedFiles: 0, failedFiles: 0)
        client.continuation?.resume(returning: reply)
        await refresh.value
        #expect(!store.isLoading)
        #expect(store.state == .empty)
    }

    @Test func latestRefreshWinsOverLateResponse() async throws {
        let client = SuspendingSearchClient()
        let store = SearchStore()
        store.attach(client: client)
        store.configure(scopeRoot: "/p", rootPath: "/p")
        store.setQuery("old", debounce: .seconds(3600))
        let first = Task { await store.refresh() }
        while client.continuation == nil { await Task.yield() }
        let stale = try #require(client.continuation)
        client.continuation = nil
        client.suspend = false
        client.reply = FileSearchResponse(
            results: [FileSearchResult(filePath: "/p/new.swift", matches: [])],
            truncated: false, scannedFiles: 0, failedFiles: 0)
        await store.refresh()
        stale.resume(returning: FileSearchResponse(
            results: [FileSearchResult(filePath: "/p/old.swift", matches: [])],
            truncated: false, scannedFiles: 0, failedFiles: 0))
        await first.value
        #expect(store.results.map(\.filePath) == ["/p/new.swift"])
        #expect(store.state == .loaded)
    }
}

@MainActor
private final class SuspendingSearchClient: SearchServing {
    var suspend = true
    var reply = FileSearchResponse(results: [], truncated: false, scannedFiles: 0, failedFiles: 0)
    var continuation: CheckedContinuation<FileSearchResponse, any Error>?

    func content(_ body: SearchContentBody) async throws -> FileSearchResponse {
        if suspend { return try await withCheckedThrowingContinuation { continuation = $0 } }
        return reply
    }
}

@MainActor
struct HostScreenReadReplyTests {
    private let ready = #"{"terminalScreenEngine":"ghostty-vt","state":"ready","reason":null,"capturedSeq":8,"appliedSeq":8,"observerBuildId":"host-test"}"#

    private func decode(_ json: String) throws -> HostScreenReadReply {
        try JSONDecoder().decode(HostScreenReadReply.self, from: Data(json.utf8))
    }

    @Test func decodesReadyAndOptionalFields() throws {
        let reply = try decode("{\"status\":\(ready),\"text\":\"hello\\n\",\"appliedSeq\":8,\"truncated\":true}")
        #expect(reply.status.isReady)
        #expect(reply.text == "hello\n")
        #expect(reply.appliedSeq == 8)
        #expect(reply.truncated)
        let minimal = try decode("{\"status\":\(ready),\"truncated\":false}")
        #expect(minimal.status.isReady)
        #expect(minimal.text == nil)
        #expect(minimal.appliedSeq == nil)
    }

    @Test func missingAndInvalidStatusFailClosed() throws {
        for status in [nil, "null", "{}", "42", "\"ready\"",
                       ready.replacingOccurrences(of: "ghostty-vt", with: "future-engine"),
                       ready.replacingOccurrences(of: "ready", with: "future-state"),
                       ready.replacingOccurrences(of: "\"capturedSeq\":8", with: "\"capturedSeq\":9")] as [String?] {
            let prefix = status.map { "\"status\":\($0)," } ?? ""
            let reply = try decode("{\(prefix)\"text\":\"unsafe\",\"appliedSeq\":8,\"truncated\":false}")
            #expect(!reply.status.isReady)
            #expect(reply.status.reason == .protocol)
            #expect(reply.text == nil)
            #expect(reply.appliedSeq == nil)
        }
    }

    @Test func readyTextWithoutCorrelatedSequenceIsHidden() throws {
        for sequence in ["", ",\"appliedSeq\":null"] {
            let reply = try decode("{\"status\":\(ready),\"text\":\"unsafe\"\(sequence),\"truncated\":false}")
            // The status itself is still a valid ready report.
            #expect(reply.status.isReady)
            #expect(reply.text == nil)
            #expect(reply.appliedSeq == nil)
        }
    }

    @Test func mismatchedSequenceAndNonReadyHideText() throws {
        let mismatch = try decode("{\"status\":\(ready),\"text\":\"unsafe\",\"appliedSeq\":7,\"truncated\":false}")
        #expect(!mismatch.status.isReady)
        #expect(mismatch.text == nil)
        let starting = ready.replacingOccurrences(of: "ready", with: "starting")
        let reply = try decode("{\"status\":\(starting),\"text\":\"unsafe\",\"truncated\":false}")
        #expect(!reply.status.isReady)
        #expect(reply.text == nil)
    }
}
