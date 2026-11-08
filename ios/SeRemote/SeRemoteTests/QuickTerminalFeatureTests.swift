import Foundation
import Testing
@testable import SeRemote

@Suite("Quick terminal feature")
@MainActor
struct QuickTerminalFeatureTests {
    private static let recordJSON = #"""
    {"schemaVersion":2,"id":"qt-1","title":"Shell","target":{"kind":"project_root","projectId":"p1","projectRoot":"/repo"},"cwd":"/repo","createdAtUtc":"2026-11-08T12:00:00Z","updatedAtUtc":"2026-11-08T12:01:00Z","terminalId":"pty-1","terminalIds":["pty-1","pty-2"],"origin":"created"}
    """#

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    private func record() throws -> QuickTerminalRecordWire {
        try decode(QuickTerminalRecordWire.self, Self.recordJSON)
    }

    private func assertKeys<T: Encodable>(_ body: T, _ expected: Set<String>) throws -> [String: Any] {
        let object = try QuickTerminalRequestJSON.dictionary(body)
        #expect(Set(object.keys) == expected)
        return object
    }

    @Test("All target tags and exact payload keys match Rust")
    func targetVariants() throws {
        let fixtures: [(QuickTerminalTargetWire, String, Set<String>)] = [
            (.workspace, "workspace", ["kind"]),
            (.projectRoot(projectId: "p1", projectRoot: "/repo"), "project_root", ["kind", "projectId", "projectRoot"]),
            (.worktree(projectId: "p1", worktreePath: "/tree", worktreeBranch: "topic"), "worktree", ["kind", "projectId", "worktreePath", "worktreeBranch"]),
        ]
        for (target, kind, keys) in fixtures {
            let object = try assertKeys(target, keys)
            #expect(object["kind"] as? String == kind)
            let data = try JSONEncoder().encode(target)
            #expect(try JSONDecoder().decode(QuickTerminalTargetWire.self, from: data) == target)
            #expect(Set([target, target]).count == 1)
        }
    }

    @Test("Unknown enum values remain identifiable and are not submitted as targets")
    func unknownEnums() throws {
        let target = try decode(QuickTerminalTargetWire.self, #"{"kind":"future_target","futureField":true}"#)
        #expect(target == .unknown("future_target"))
        #expect(throws: EncodingError.self) { try JSONEncoder().encode(target) }
        #expect(try decode(QuickTerminalOriginWire.self, #""future_origin""#) == .unknown("future_origin"))
        #expect(try decode(QuickTerminalOriginWire.self, #""created""#) == .created)
        // record.rs uses snake_case, not migratedConversation.
        #expect(try decode(QuickTerminalOriginWire.self, #""migrated_conversation""#) == .migratedConversation)
        #expect(throws: DecodingError.self) { try decode(QuickTerminalTargetWire.self, #"{"kind":"project_root"}"#) }
    }

    @Test("Record decodes every field, is Hashable and Sendable")
    func recordModel() throws {
        let value = try record()
        #expect(value.schemaVersion == 2)
        #expect(value.id == "qt-1")
        #expect(value.title == "Shell")
        #expect(value.target == .projectRoot(projectId: "p1", projectRoot: "/repo"))
        #expect(value.cwd == "/repo")
        #expect(value.createdAtUtc == "2026-11-08T12:00:00Z")
        #expect(value.updatedAtUtc == "2026-11-08T12:01:00Z")
        #expect(value.terminalId == "pty-1")
        #expect(value.terminalIds == ["pty-1", "pty-2"])
        #expect(value.origin == .created)
        #expect(Set([value, value]).count == 1)
        requireSendable(value)
        requireSendable(value.target)
        requireSendable(value.origin)
    }

    @Test("Omitted and null optionals stay nil; legacy terminalIds defaults empty")
    func recordOptionals() throws {
        for fields in ["", #", "title":null,"terminalId":null"#] {
            let value = try decode(QuickTerminalRecordWire.self, """
            {"schemaVersion":1,"id":"old","target":{"kind":"workspace"},"cwd":"/w","createdAtUtc":"t1","updatedAtUtc":"t2","origin":"migrated_conversation"\(fields)}
            """)
            #expect(value.title == nil)
            #expect(value.terminalId == nil)
            #expect(value.terminalIds.isEmpty)
            #expect(value.origin == .migratedConversation)
        }
    }

    @Test("Opened result preserves claim, spawned, terminalId and nested record")
    func openedModel() throws {
        let value = try decode(QuickTerminalOpenedWire.self, """
        {"record":\(Self.recordJSON),"terminalId":"pty-2","claim":"one-use-claim","spawned":true}
        """)
        #expect(value.record == (try record()))
        #expect(value.terminalId == "pty-2")
        #expect(value.claim == "one-use-claim")
        #expect(value.spawned)
        #expect(Set([value, value]).count == 1)
        requireSendable(value)
        for extra in ["", #", "claim":null"#] {
            let reused = try decode(QuickTerminalOpenedWire.self, """
            {"record":\(Self.recordJSON),"terminalId":"pty-1","spawned":false\(extra)}
            """)
            #expect(reused.claim == nil)
            #expect(!reused.spawned)
        }
    }

    @Test("Create request exact keys with and without title")
    func createRequest() throws {
        let minimal = try assertKeys(CreateQuickTerminalBody(target: .workspace), ["target"])
        let target = try #require(minimal["target"] as? [String: Any])
        #expect(Set(target.keys) == ["kind"])
        #expect(target["kind"] as? String == "workspace")
        let named = try assertKeys(CreateQuickTerminalBody(target: .projectRoot(projectId: "p", projectRoot: "/p"), title: "Shell"), ["target", "title"])
        #expect(named["title"] as? String == "Shell")
        let project = try #require(named["target"] as? [String: Any])
        #expect(Set(project.keys) == ["kind", "projectId", "projectRoot"])
        #expect(project["kind"] as? String == "project_root")
    }

    @Test("Open and create-terminal bodies have distinct exact key sets")
    func openRequests() throws {
        let open = try assertKeys(OpenQuickTerminalBody(id: "q", cols: 120, rows: 40), ["id", "cols", "rows"])
        #expect(open["id"] as? String == "q")
        #expect(open["cols"] as? Int == 120)
        #expect(open["rows"] as? Int == 40)
        let selected = try assertKeys(OpenQuickTerminalBody(id: "q", cols: 120, rows: 40, terminalId: "pty"), ["id", "cols", "rows", "terminalId"])
        #expect(selected["terminalId"] as? String == "pty")
        let create = try assertKeys(CreateQuickTerminalTerminalBody(id: "q", cols: 80, rows: 24), ["id", "cols", "rows"])
        #expect(create["cols"] as? Int == 80)
        #expect(create["rows"] as? Int == 24)
    }

    @Test("Close-terminal, rename, delete and close exact key sets")
    func mutationRequests() throws {
        let closeTerminal = try assertKeys(CloseQuickTerminalTerminalBody(id: "q", terminalId: "pty"), ["id", "terminalId"])
        #expect(closeTerminal["terminalId"] as? String == "pty")
        let named = try assertKeys(QuickTerminalRenameBody(id: "q", title: "New"), ["id", "title"])
        #expect(named["title"] as? String == "New")
        _ = try assertKeys(QuickTerminalRenameBody(id: "q"), ["id"])
        let idOnly = try assertKeys(QuickTerminalIdBody(id: "q"), ["id"])
        #expect(idOnly["id"] as? String == "q")
    }

    @Test("Delete really uses POST and shared credentials, not a GET envelope workaround")
    func deleteRequest() throws {
        let origin = try #require(URL(string: "https://host.example/base/"))
        let credentials = HostCredentials(accessURL: origin, bearer: "test-token")
        let http = HostHTTP(origin: origin, credentials: credentials)
        let request = try QuickTerminalClient.deleteRequest(http: http, body: .init(id: "q"))
        #expect(request.httpMethod == "POST")
        #expect(request.url?.absoluteString == "https://host.example/base/quick-terminals/delete")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer test-token")
        #expect(request.value(forHTTPHeaderField: "Origin") == "https://host.example")
        #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
        #expect(request.timeoutInterval == 20)
        #expect(!request.assumesHTTP3Capable)
        let data = try #require(request.httpBody)
        let object = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(Set(object.keys) == ["id"])
        #expect(object["id"] as? String == "q")
    }

    @Test("Null delete acknowledgement succeeds; IPC and HTTP failures still throw")
    func deleteAcknowledgement() throws {
        let url = try #require(URL(string: "https://host.example/quick-terminals/delete"))
        let ok = try #require(HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil))
        try QuickTerminalClient.validateDeleteResponse(data: Data(#"{"success":true,"data":null}"#.utf8), response: ok)
        try QuickTerminalClient.validateDeleteResponse(data: Data(#"{"success":true}"#.utf8), response: ok)
        do {
            try QuickTerminalClient.validateDeleteResponse(data: Data(#"{"success":false,"data":null,"error":"Missing","code":"QUICK_TERMINAL_NOT_FOUND"}"#.utf8), response: ok)
            Issue.record("Expected an IPC error")
        } catch HostError.ipc(let message, let code) {
            #expect(message == "Missing")
            #expect(code == "QUICK_TERMINAL_NOT_FOUND")
        }
        for status in [401, 403, 422, 500] {
            let response = try #require(HTTPURLResponse(url: url, statusCode: status, httpVersion: nil, headerFields: nil))
            #expect(throws: HostError.self) {
                try QuickTerminalClient.validateDeleteResponse(data: Data(), response: response)
            }
        }
        #expect(throws: DecodingError.self) {
            try QuickTerminalClient.validateDeleteResponse(data: Data("not json".utf8), response: ok)
        }
    }

    @Test("Refresh transitions through loading, empty, loaded and failed, and recovers")
    func storeRefreshStates() async throws {
        let fake = QuickTerminalFakeClient(record: try record())
        let store = QuickTerminalStore(client: fake)
        #expect(store.state == .idle)
        fake.onList = { #expect(store.isLoading) }
        await store.refresh()
        #expect(store.state == .loaded)
        #expect(store.records == [fake.record])
        fake.listResult = []
        await store.refresh()
        #expect(store.state == .empty)
        fake.error = HostError.ipc("No access", "FORBIDDEN")
        await store.refresh()
        #expect(store.errorMessage != nil)
        #expect(!store.isLoading)
        fake.error = nil
        await store.refresh()
        #expect(store.state == .empty)
        #expect(store.errorMessage == nil)
    }

    @Test("Store mutations update records and return the unchanged attach result")
    func storeMutations() async throws {
        let fake = QuickTerminalFakeClient(record: try record())
        let store = QuickTerminalStore(client: fake)
        _ = try await store.create(target: .workspace)
        #expect(store.records == [fake.record])
        let opened = try await store.open(id: "qt-1", cols: 80, rows: 24, terminalId: "pty-2")
        #expect(opened.record == fake.record)
        #expect(opened.terminalId == "pty-2")
        #expect(opened.claim == "attach-claim")
        #expect(opened.spawned)
        _ = try await store.createTerminal(id: "qt-1", cols: 80, rows: 24)
        _ = try await store.rename(id: "qt-1", title: nil)
        _ = try await store.closeTerminal(id: "qt-1", terminalId: "pty-2")
        _ = try await store.close(id: "qt-1")
        #expect(store.records.count == 1)
        try await store.delete(id: "qt-1")
        #expect(store.records.isEmpty)
        #expect(store.state == .empty)
        #expect(fake.calls == ["create", "open", "create-terminal", "rename", "close-terminal", "close", "delete"])
        fake.error = HostError.ipc("Missing", "QUICK_TERMINAL_NOT_FOUND")
        await #expect(throws: HostError.self) { try await store.close(id: "qt-1") }
        #expect(store.errorMessage != nil)
    }

    @Test("Unattached store does not issue requests and fails mutations explicitly")
    func unattachedStore() async throws {
        let store = QuickTerminalStore()
        await store.refresh()
        #expect(store.state == .idle)
        await #expect(throws: HostError.self) { try await store.delete(id: "q") }
        let origin = try #require(URL(string: "https://host.example"))
        store.attach(http: HostHTTP(origin: origin, credentials: HostCredentials(accessURL: origin)))
        #expect(store.state == .idle)
        #expect(store.records.isEmpty)
    }

    private func requireSendable<T: Sendable>(_: T) {}
}

@MainActor
private final class QuickTerminalFakeClient: QuickTerminalServing {
    let record: QuickTerminalRecordWire
    var listResult: [QuickTerminalRecordWire]
    var error: HostError?
    var calls: [String] = []
    var onList: (() -> Void)?

    init(record: QuickTerminalRecordWire) {
        self.record = record
        self.listResult = [record]
    }

    func list() async throws -> [QuickTerminalRecordWire] {
        onList?()
        if let error { throw error }
        return listResult
    }
    private func result(_ call: String) throws -> QuickTerminalRecordWire {
        calls.append(call)
        if let error { throw error }
        return record
    }
    private func opened(_ call: String) throws -> QuickTerminalOpenedWire {
        let record = try result(call)
        return QuickTerminalOpenedWire(record: record, terminalId: "pty-2", claim: "attach-claim", spawned: true)
    }
    func create(_ body: CreateQuickTerminalBody) async throws -> QuickTerminalRecordWire { try result("create") }
    func open(_ body: OpenQuickTerminalBody) async throws -> QuickTerminalOpenedWire { try opened("open") }
    func createTerminal(_ body: CreateQuickTerminalTerminalBody) async throws -> QuickTerminalOpenedWire { try opened("create-terminal") }
    func closeTerminal(_ body: CloseQuickTerminalTerminalBody) async throws -> QuickTerminalRecordWire { try result("close-terminal") }
    func rename(_ body: QuickTerminalRenameBody) async throws -> QuickTerminalRecordWire { try result("rename") }
    func delete(_ body: QuickTerminalIdBody) async throws { _ = try result("delete") }
    func close(_ body: QuickTerminalIdBody) async throws -> QuickTerminalRecordWire { try result("close") }
}
