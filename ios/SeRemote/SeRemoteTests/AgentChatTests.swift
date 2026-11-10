import Foundation
import SwiftUI
import UIKit
import Testing
@testable import SeRemote

@Suite("Hooked terminal conversation")
@MainActor
struct AgentChatTests {
    private func snapshot(
        session: String? = "session-1", cursor: UInt64 = 1,
        messages: [ChatMessage] = [], deliveries: [AgentChatDelivery] = [],
        state: String = "ready", terminal: String = "pty-1", older: UInt64? = nil, reset: Bool = false,
        tools: [AgentChatToolCall] = [], refs: [String: String] = [:], force: Bool = false, hook: AgentChatHookState? = nil
    ) -> AgentChatSnapshot {
        AgentChatSnapshot(terminalID: terminal, sessionID: session, agent: "claude", cursor: cursor,
                          messages: messages, deliveries: deliveries, state: state, reason: nil, hookState: hook, toolCalls: tools, reset: reset, olderCursor: older, messageToolIDs: refs, canForceSend: force)
    }

    private func message(_ id: String, _ text: String) -> ChatMessage {
        ChatMessage(id: id, role: .agent, text: text, streaming: false, seq: nil)
    }

    private func marker(_ id: String, _ seq: UInt64) -> ChatMessage {
        ChatMessage(id: id, role: .agent, text: "", streaming: false, seq: seq)
    }

    private func tool(_ id: String, _ seq: UInt64, _ detail: String = "result") throws -> AgentChatToolCall {
        let value: [String: Any] = ["toolCallId": id, "title": id, "status": "completed", "seq": seq,
                                   "content": [["type": "content", "content": ["type": "text", "text": detail]]]]
        return try JSONDecoder().decode(AgentChatToolCall.self, from: JSONSerialization.data(withJSONObject: value))
    }

    private func store(_ fake: AgentChatFake, canView: Bool = true, canInput: Bool = true) -> AgentChatStore {
        AgentChatStore(terminalID: "pty-1", client: AgentChatClient(
            snapshot: { terminal, cursor, before in try await fake.snapshot(terminal, cursor, before) },
            send: { terminal, text, options in try await fake.send(terminal, text, options) },
            control: { terminal, action in await fake.control(terminal, action) }
        ), canView: canView, canInput: canInput)
    }

    @Test("Wire snapshot reuses ACP message shape and includes hook metadata and receipts")
    func wireSnapshot() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":"s1","agent":"claude","cursor":42,"older_cursor":7,"can_force_send":true,"messages":[{"id":"m1","role":"agent","text":"hello","streaming":false,"seq":7}],"deliveries":[{"id":"d1","text":"prompt","status":"pending"}],"state":"ready","hook_state":{"kind":"permission_request"}}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.terminalID == "pty-1")
        #expect(value.sessionID == "s1")
        #expect(value.cursor == 42)
        #expect(value.olderCursor == 7)
        #expect(value.canForceSend)
        #expect(value.messages.first?.role == .agent)
        #expect(value.hookState?.kind == "permission_request")
        #expect(value.deliveries.first?.status == "pending")
    }

    @Test("Degraded snapshot tolerates null session and string hook state")
    func degradedWire() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":null,"agent":null,"cursor":0,"messages":[],"deliveries":[],"state":"degraded","reason":"missing_hook","hook_state":"not_installed"}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.sessionID == nil)
        #expect(value.olderCursor == nil)
        #expect(value.reason == "missing_hook")
        #expect(value.hookState?.kind == "not_installed")
    }

    @Test("Canonical ACP text blocks, thoughts and tool content decode into shared rows")
    func canonicalTranscript() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":"s1","agent":"qin","cursor":7,"messages":[{"id":"thought","role":"thought","blocks":[{"type":"text","text":"thinking"}],"seq":2},{"id":"answer","role":"agent","blocks":[{"type":"text","text":"first"},{"type":"text","text":"second"}],"seq":4}],"tool_calls":[{"toolCallId":"tool-1","title":"Read file","kind":"read","status":"completed","content":[{"type":"content","content":{"type":"text","text":"redacted result"}}]}],"deliveries":[],"state":"ready","hook_state":"agent_end","reset":true}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.reset)
        #expect(value.messages[0].role == .thought)
        #expect(value.messages[0].text == "thinking")
        #expect(value.messages[1].text == "first\nsecond")
        #expect(value.toolCalls.first?.card.id == "tool-1")
        #expect(value.toolCalls.first?.detail == "redacted result")
        let store = store(AgentChatFake(value: value))
        store.apply(value)
        #expect(store.toolCalls.count == 1)
    }

    @Test("Wire marker references are retained without modifying shared ACP messages")
    func markerWireMetadata() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":"s1","cursor":100,"messages":[{"id":"marker","role":"agent","blocks":[],"seq":1,"tool_call_id":"tool"}],"tool_calls":[{"toolCallId":"tool","title":"Read","status":"completed","seq":99,"content":[]}],"deliveries":[],"state":"ready"}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.messageToolIDs == ["marker": "tool"])
        let store = store(AgentChatFake(value: value))
        store.apply(value)
        #expect(store.toolCalls.map(\.id) == ["tool"])
    }

    @Test("Older tools merge by latest sequence and empty live deltas retain referenced values")
    func toolPageAndLiveMerge() throws {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 1000, messages: [marker("m10", 10)], tools: [try tool("shared", 100, "live")], refs: ["m10": "shared"]))
        store.applyOlder(snapshot(cursor: 1100, messages: [marker("m1", 1), marker("m2", 2)], older: 1,
                                  tools: [try tool("old", 300, "page latest"), try tool("shared", 90, "stale")], refs: ["m1": "old", "m2": "shared"]))
        #expect(store.toolCalls.map(\.id) == ["shared", "old"])
        #expect(store.toolCalls[0].detail == "live")
        #expect(store.toolCalls[1].seq == 300)
        #expect(store.cursor == 1000)
        store.apply(snapshot(cursor: 1100))
        #expect(store.toolCalls.count == 2)
        store.apply(snapshot(cursor: 1200, tools: [try tool("shared", 50, "stale live")]))
        #expect(store.toolCalls[0].detail == "live")
        store.applyOlder(snapshot(cursor: 1300, messages: [], tools: [try tool("shared", 200, "new page latest")]))
        #expect(store.toolCalls[0].detail == "new page latest")
        store.apply(snapshot(cursor: 1400, reset: true))
        #expect(store.toolCalls.isEmpty)
    }

    @Test("Evicting markers prunes unreachable tools in both latest and older windows")
    func reachableToolPruning() throws {
        let messages = (0...AgentChatStore.maximumMessages).map { marker("m\($0)", UInt64($0)) }
        let tools = try (0...AgentChatStore.maximumMessages).map { try tool("t\($0)", UInt64(10000 + $0)) }
        let refs = Dictionary((0...AgentChatStore.maximumMessages).map { ("m\($0)", "t\($0)") }, uniquingKeysWith: { _, last in last })
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 5000, messages: messages, tools: tools, refs: refs))
        #expect(store.toolCalls.count == AgentChatStore.maximumMessages)
        #expect(!store.toolCalls.contains { $0.id == "t0" })
        #expect(store.toolCalls.contains { $0.id == "t\(AgentChatStore.maximumMessages)" })
        store.applyOlder(snapshot(cursor: 6000, messages: [messages[0]], tools: [tools[0]], refs: ["m0": "t0"]))
        #expect(store.messages.first?.id == "m0")
        #expect(store.toolCalls.contains { $0.id == "t0" })
        #expect(!store.toolCalls.contains { $0.id == "t\(AgentChatStore.maximumMessages)" })
    }

    @Test("Combined message/tool bytes are bounded without truncating one oversized pair")
    func combinedToolByteBound() throws {
        let payload = String(repeating: "x", count: AgentChatStore.maximumBytes / 2 + 1024)
        let old = try tool("old", 100, payload), new = try tool("new", 200, payload)
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(messages: [marker("old", 1), marker("new", 2)], tools: [old, new], refs: ["old": "old", "new": "new"]))
        #expect(store.messages.map(\.id) == ["new"])
        #expect(store.toolCalls.map(\.id) == ["new"])
        #expect(store.olderCursor == 2)
        store.applyOlder(snapshot(messages: [marker("old", 1)], tools: [old], refs: ["old": "old"]))
        #expect(store.messages.map(\.id) == ["old"])
        #expect(store.toolCalls.first?.detail == payload)
        let oversized = String(repeating: "z", count: AgentChatStore.maximumBytes + 1)
        store.apply(snapshot(session: "oversized", cursor: 2, messages: [marker("large", 1)], tools: [try tool("large", 1000, oversized)], refs: ["large": "large"]))
        #expect(store.messages.count == 1)
        #expect(store.toolCalls.first?.detail == oversized)
    }

    @Test("Legacy marker-free and tool-only snapshots are bounded too")
    func legacyToolCompatibility() throws {
        let tools = try (0..<(AgentChatStore.maximumMessages + 2)).map { try tool("t\($0)", UInt64($0)) }
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(tools: tools))
        #expect(store.toolCalls.count == AgentChatStore.maximumMessages)
        store.apply(snapshot(session: "legacy", cursor: 2, messages: [marker("first", 2), marker("last", 4)], tools: [try tool("outside", 1), try tool("inside", 3)]))
        #expect(store.toolCalls.map(\.id) == ["inside"])
    }

    @Test("Backend reset replaces same-session transcript even if cursor increased")
    func explicitReset() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 1, messages: [message("old", "old")]))
        let reset = AgentChatSnapshot(terminalID: "pty-1", sessionID: "session-1", agent: "qin", cursor: 2,
                                      messages: [message("new", "new")], deliveries: [], state: "ready",
                                      reason: nil, hookState: nil, reset: true)
        store.apply(reset)
        #expect(store.messages.map(\.id) == ["new"])
    }

    @Test("Control accepts successful null data but never a failed IPC envelope")
    func nullControlAcknowledgement() throws {
        try AgentChatClient.validateAcknowledgement(Data(#"{"success":true,"data":null}"#.utf8))
        try AgentChatClient.validateAcknowledgement(Data(#"{"success":true,"data":{}}"#.utf8))
        #expect(throws: (any Error).self) {
            try AgentChatClient.validateAcknowledgement(Data(#"{"success":false,"data":null,"error":"denied","code":"FORBIDDEN"}"#.utf8))
        }
    }

    @Test("Toggle retains the same mounted terminal view without teardown")
    func stableTerminalMountContract() async throws {
        let origin = URL(string: "http://127.0.0.1:1")!
        let terminalID = "mount-test-\(UUID().uuidString)"
        let key = "agent-chat.presentation.\(origin.absoluteString).\(terminalID)"
        UserDefaults.standard.set(false, forKey: key)
        defer { UserDefaults.standard.removeObject(forKey: key) }
        let counter = AgentChatMountCounter()
        let container = TerminalAgentChatContainer(
            terminalID: terminalID, http: HostHTTP(origin: origin, credentials: HostCredentials(accessURL: origin, bearer: "test")),
            canView: false, canInput: false, dismissTerminalKeyboard: { counter.presentationChanges += 1 }
        ) {
            AgentChatMountedTerminal(counter: counter)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        let scene = try #require(
            UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        )
        let window = UIWindow(windowScene: scene)
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        window.rootViewController = UIHostingController(rootView: container)
        window.isHidden = false
        defer { window.isHidden = true; window.rootViewController = nil }
        try await Task.sleep(for: .milliseconds(100))
        #expect(counter.mounts == 1)
        for selection in [true, false, true, false] {
            UserDefaults.standard.set(selection, forKey: key)
            try await Task.sleep(for: .milliseconds(60))
        }
        #expect(counter.mounts == 1)
        #expect(counter.unmounts == 0)
        #expect(counter.presentationChanges == 4)
    }

    @Test("Unknown delivery states never claim successful delivery")
    func deliveryStates() {
        for status in ["queued", "pending", "delivered", "unconfirmed", "future_state"] {
            let delivery = AgentChatDelivery(id: status, text: "prompt", status: status)
            #expect(!delivery.label.isEmpty)
        }
        #expect(AgentChatDelivery(id: "d", text: "p", status: "future").label == AgentChatDelivery(id: "d", text: "p", status: "unconfirmed").label)
    }

    @Test("Terminal IDs are encoded as a single path segment")
    func pathEncoding() {
        #expect(AgentChatClient.path("pty-1") == "/agent-chat/pty-1")
        #expect(AgentChatClient.path("a/b?#%") == "/agent-chat/a%2Fb%3F%23%25")
    }

    @Test("Repeated cursor snapshots update streaming messages without duplicates")
    func mergesAndDeduplicates() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 1, messages: [message("a", "hel")]))
        store.apply(snapshot(cursor: 2, messages: [message("a", "hello"), message("b", "world")]))
        store.apply(snapshot(cursor: 2, messages: [message("b", "world")]))
        #expect(store.messages.map(\.id) == ["a", "b"])
        #expect(store.messages[0].text == "hello")
        #expect(store.cursor == 2)
    }

    @Test("Clear or resume binds new session history instead of concatenating")
    func switchesSession() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(messages: [message("old", "old session")]))
        store.apply(snapshot(session: "session-2", cursor: 2, messages: [message("new", "new session")]))
        #expect(store.sessionID == "session-2")
        #expect(store.messages.map(\.id) == ["new"])
    }

    @Test("Delayed lower cursors cannot roll back a monotonic live cursor")
    func resetsCursor() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 100, messages: [message("old", "old")]))
        store.apply(snapshot(cursor: 1, messages: [message("new", "new")]))
        #expect(store.messages.map(\.id) == ["old"])
        #expect(store.cursor == 100)
    }

    @Test("A different terminal can never poison the current transcript")
    func terminalIsolation() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(messages: [message("a", "ours")]))
        store.apply(snapshot(cursor: 2, messages: [message("b", "foreign")], terminal: "pty-2"))
        #expect(store.messages.map(\.id) == ["a"])
        #expect(store.cursor == 1)
    }

    @Test("GET uses the last applied cursor")
    func incrementalRefresh() async {
        let fake = AgentChatFake(value: snapshot(cursor: 9))
        let store = store(fake)
        await store.refresh()
        await store.refresh()
        #expect(await fake.cursors == [0, 9])
        #expect(!store.isLoading)
    }

    @Test("Paging queries use an exclusive before boundary independently from live cursor")
    func paginationQuery() {
        #expect(AgentChatClient.snapshotQuery(after: 8, before: nil) == ["after": "8"])
        #expect(AgentChatClient.snapshotQuery(after: nil, before: 4) == ["before": "4"])
        #expect(AgentChatClient.snapshotQuery(after: nil, before: nil).isEmpty)
    }

    @Test("Older pages prepend/dedupe and polling preserves history and paging cursor")
    func olderPages() async {
        let fake = AgentChatFake(value: snapshot(cursor: 100, messages: [message("recent", "new")], older: 10))
        let store = store(fake)
        await store.refresh()
        await fake.setPage(snapshot(cursor: 120, messages: [message("old", "older"), message("recent", "stale")], older: 1))
        await store.loadOlder()
        #expect(await fake.beforeCursors == [nil, 10])
        #expect(await fake.cursors == [0, nil])
        #expect(store.messages.map(\.id) == ["old", "recent"])
        #expect(store.messages.last?.text == "new")
        #expect(store.cursor == 100)
        #expect(store.olderCursor == 1)
        await store.refresh()
        #expect(await fake.cursors == [0, nil])
        #expect(store.isBrowsingOlder)
        #expect(store.messages.map(\.id) == ["old", "recent"])
        #expect(store.olderCursor == 1)
        await store.returnToLatest()
        #expect(await fake.cursors == [0, nil, nil])
        #expect(!store.isBrowsingOlder)
        #expect(store.messages.map(\.id) == ["recent"])
        store.apply(snapshot(session: "session-2", cursor: 121, messages: [message("replacement", "new session")]))
        #expect(store.messages.map(\.id) == ["replacement"])
        #expect(store.olderCursor == nil)
    }

    @Test("A same-session reset-fence page directly replaces history and tools and resumes live polling")
    func staleBeforeResetFence() async throws {
        let fake = AgentChatFake(value: snapshot(cursor: 100, messages: [marker("old", 10)], older: 10,
                                               tools: [try tool("old", 50)], refs: ["old": "old"]))
        let store = store(fake)
        await store.refresh()
        await fake.setPage(snapshot(cursor: 300, messages: [marker("new", 200)], older: 200, reset: true,
                                    tools: [try tool("new", 250)], refs: ["new": "new"]))
        await store.loadOlder()
        #expect(await fake.cursors == [0, nil])
        #expect(await fake.beforeCursors == [nil, 10])
        #expect(store.sessionID == "session-1")
        #expect(store.cursor == 300)
        #expect(store.olderCursor == 200)
        #expect(store.messages.map(\.id) == ["new"])
        #expect(store.toolCalls.map(\.id) == ["new"])
        #expect(!store.isBrowsingOlder)
        await store.refresh()
        #expect(await fake.cursors == [0, nil, 300])
        #expect(store.cursor == 300)
        #expect(store.messages.map(\.id) == ["new"])
        #expect(store.toolCalls.map(\.id) == ["new"])
    }

    @Test("Same-session resets discard an older request that was already in flight")
    func stalePageAfterReset() async {
        let fake = AgentChatFake(value: snapshot(cursor: 10, older: 5))
        let store = store(fake)
        await store.refresh()
        await fake.setPage(snapshot(cursor: 10, messages: [message("stale", "old")]), delayed: true)
        let request = Task { await store.loadOlder() }
        // Wait for the actor to suspend the request, rather than relying on a timer.
        while await fake.pageContinuation == nil { await Task.yield() }
        store.apply(snapshot(cursor: 11, messages: [message("new", "new")], reset: true))
        await fake.finishPage()
        await request.value
        #expect(store.messages.map(\.id) == ["new"])
        #expect(store.olderCursor == nil)
        #expect(store.cursor == 11)
    }

    @Test("Retained timeline evicts whole records but never truncates a full oversized record")
    func boundedHistory() {
        let store = store(AgentChatFake(value: snapshot()))
        let messages = (0..<(AgentChatStore.maximumMessages + 2)).map {
            ChatMessage(id: "m\($0)", role: .agent, text: "record-\($0)", streaming: false, seq: UInt64($0))
        }
        store.apply(snapshot(messages: messages))
        #expect(store.messages.count == AgentChatStore.maximumMessages)
        #expect(store.olderCursor == 2)
        let text = String(repeating: "x", count: AgentChatStore.maximumBytes + 1)
        store.apply(snapshot(session: "big", cursor: 2, messages: [message("big", text)]))
        #expect(store.messages.first?.text == text)
    }

    @Test("Older browsing traverses past memory capacity and return-latest restores recent window")
    func traversePastCapacity() async {
        let recent = (100..<(AgentChatStore.maximumMessages + 100)).map {
            ChatMessage(id: "m\($0)", role: .agent, text: "record-\($0)", streaming: false, seq: UInt64($0))
        }
        let fake = AgentChatFake(value: snapshot(cursor: 5000, messages: recent, older: 100))
        let store = store(fake)
        await store.refresh()
        await fake.setPage(snapshot(cursor: 6000, messages: [message("m1", "older")], older: 1))
        await store.loadOlder()
        #expect(store.messages.count == AgentChatStore.maximumMessages)
        #expect(store.messages.first?.id == "m1")
        #expect(store.messages.last?.id == "m\(AgentChatStore.maximumMessages + 98)")
        #expect(store.cursor == 5000)
        #expect(store.olderCursor == 1)
        await fake.setPage(snapshot(cursor: 6000, messages: [message("m0", "oldest")]))
        await store.loadOlder()
        #expect(store.messages.first?.id == "m0")
        #expect(store.olderCursor == nil)
        await store.returnToLatest()
        #expect(!store.isBrowsingOlder)
        #expect(store.messages.first?.id == "m100")
        #expect(store.olderCursor == 100)
    }

    @Test("Explicit Send now reuses one cold queued receipt and never submits its prompt text")
    func forceQueuedReceipt() async {
        let receipt = AgentChatDelivery(id: "cold", text: "original raw queued text", status: "queued")
        let fake = AgentChatFake(value: snapshot(deliveries: [receipt], force: true), delaySend: true)
        let store = store(fake)
        await store.refresh()
        #expect(store.canForceDelivery("cold"))
        async let first: Void = store.forceSend("cold")
        async let second: Void = store.forceSend("cold")
        _ = await (first, second)
        #expect(await fake.sent.count == 1)
        #expect(await fake.sent.first == "")
        let options = await fake.sendOptions.compactMap { $0 }
        #expect(options.first?.force == true)
        #expect(options.first?.deliveryID == "cold")
        #expect(store.deliveries == [receipt])
        #expect(store.messages.isEmpty)
    }

    @Test("Force override JSON uses force/delivery_id and no original queued prompt")
    func forceRequestBody() {
        let body = AgentChatClient.sendBody(text: "", options: AgentChatSendOptions(force: true, deliveryID: "cold"))
        #expect(body["text"] as? String == "")
        #expect(body["force"] as? Bool == true)
        #expect(body["delivery_id"] as? String == "cold")
        let normal = AgentChatClient.sendBody(text: "normal", options: nil)
        #expect(normal.count == 1)
        #expect(normal["text"] as? String == "normal")
    }

    @Test("Unavailable, busy, pending, delivered and missing queued receipts cannot be forced")
    func forceRejectedStates() async throws {
        let busy = try JSONDecoder().decode(AgentChatHookState.self, from: Data(#""busy""#.utf8))
        let queued = AgentChatDelivery(id: "cold", text: "queued", status: "queued")
        let cases = [
            snapshot(deliveries: [queued]),
            snapshot(deliveries: [queued], force: true, hook: busy),
            snapshot(deliveries: [queued, AgentChatDelivery(id: "pending", text: "in flight", status: "pending")], force: true),
            snapshot(deliveries: [AgentChatDelivery(id: "cold", text: "pending", status: "pending")], force: true),
            snapshot(deliveries: [AgentChatDelivery(id: "cold", text: "delivered", status: "delivered")], force: true),
            snapshot(deliveries: [], force: true)
        ]
        for snapshot in cases {
            let fake = AgentChatFake(value: snapshot)
            let store = store(fake)
            await store.refresh()
            #expect(!store.canForceDelivery("cold"))
            await store.forceSend("cold")
            #expect(await fake.sent.isEmpty)
        }
        let fake = AgentChatFake(value: snapshot(deliveries: [queued], force: true))
        let viewOnly = store(fake, canInput: false)
        await viewOnly.refresh()
        #expect(!viewOnly.canForceDelivery("cold"))
        await viewOnly.forceSend("cold")
        #expect(await fake.sent.isEmpty)
    }

    @Test("Failed force overrides are never retried automatically")
    func forceFailureNoRetry() async {
        let fake = AgentChatFake(value: snapshot(deliveries: [AgentChatDelivery(id: "cold", text: "prompt", status: "queued")], force: true), failSend: true)
        let store = store(fake)
        await store.refresh()
        await store.forceSend("cold")
        #expect(await fake.sent.count == 1)
        #expect(store.errorMessage != nil)
        #expect(!store.canForceDelivery("cold"))
    }

    @Test("View scope blocks reads and all mutations")
    func deniedView() async {
        let fake = AgentChatFake(value: snapshot())
        let store = store(fake, canView: false)
        store.draft = "prompt"
        await store.refresh()
        await store.send()
        await store.control(.interrupt)
        #expect(await fake.cursors.isEmpty)
        #expect(await fake.sent.isEmpty)
        #expect(await fake.controls.isEmpty)
        #expect(!store.canSend)
    }

    @Test("Read-only terminal.input scope allows transcript but denies POST")
    func readOnly() async {
        let fake = AgentChatFake(value: snapshot())
        let store = store(fake, canInput: false)
        await store.refresh()
        store.draft = "prompt"
        await store.send()
        await store.control(.stop)
        #expect(await fake.cursors == [0])
        #expect(await fake.sent.isEmpty)
        #expect(await fake.controls.isEmpty)
        #expect(!store.canControl)
    }

    @Test("Degraded state and whitespace draft cannot send or control")
    func degradedAndEmpty() async {
        let fake = AgentChatFake(value: snapshot(state: "degraded"))
        let store = store(fake)
        await store.refresh()
        store.draft = "prompt"
        await store.send()
        await store.control(.interrupt)
        store.apply(snapshot())
        store.draft = " \n "
        await store.send()
        #expect(await fake.sent.isEmpty)
        #expect(await fake.controls.isEmpty)
    }

    @Test("Successful send preserves exact text and waits for transcript confirmation")
    func successfulSend() async {
        let fake = AgentChatFake(value: snapshot())
        let store = store(fake)
        await store.refresh()
        store.draft = "  multiline\nprompt  "
        await store.send()
        #expect(await fake.sent == ["  multiline\nprompt  "])
        #expect(store.draft.isEmpty)
        #expect(store.messages.isEmpty)
        #expect(store.submissionNotice != nil)
    }

    @Test("Concurrent taps create only one POST")
    func duplicateTap() async {
        let fake = AgentChatFake(value: snapshot(), delaySend: true)
        let store = store(fake)
        await store.refresh()
        store.draft = "one"
        let first = Task { await store.send() }
        await Task.yield()
        let second = Task { await store.send() }
        await first.value
        await second.value
        #expect(await fake.sent.count == 1)
    }

    @Test("An ambiguous failed POST is not retried by polling and keeps the draft")
    func noAutomaticRetry() async {
        let fake = AgentChatFake(value: snapshot(), failSend: true)
        let store = store(fake)
        await store.refresh()
        store.draft = "do not duplicate"
        await store.send()
        #expect(store.errorMessage != nil)
        #expect(store.draft == "do not duplicate")
        await store.refresh()
        await store.refresh()
        #expect(await fake.sent.count == 1)
        #expect(store.errorMessage != nil)
        #expect(store.submissionNotice == String(localized: "Unconfirmed — not resent automatically"))
    }

    @Test("Interrupt and stop use control endpoint, never transcript or terminal input")
    func controls() async {
        let fake = AgentChatFake(value: snapshot())
        let store = store(fake)
        await store.refresh()
        await store.control(.interrupt)
        await store.control(.stop)
        #expect(await fake.controls == [.interrupt, .stop])
        #expect(await fake.sent.isEmpty)
    }

    @Test("Transient GET failures preserve transcript and disable unsafe send")
    func readFailure() async {
        let fake = AgentChatFake(value: snapshot())
        let store = store(fake)
        store.apply(snapshot(messages: [message("a", "keep")]))
        await fake.setFailRead()
        await store.refresh()
        #expect(store.messages.first?.text == "keep")
        #expect(!store.isReady)
        #expect(store.errorMessage != nil)
    }

    @Test("Scope revocation removes previously visible transcript")
    func revokedView() async {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(messages: [message("a", "secret")]))
        store.canView = false
        await store.refresh()
        #expect(store.messages.isEmpty)
        #expect(!store.isReady)
    }
}

@MainActor
private final class AgentChatMountCounter {
    var mounts = 0
    var unmounts = 0
    var presentationChanges = 0
}

private struct AgentChatMountedTerminal: UIViewRepresentable {
    let counter: AgentChatMountCounter

    func makeCoordinator() -> AgentChatMountCounter { counter }
    func makeUIView(context: Context) -> UIView {
        counter.mounts += 1
        return UIView()
    }
    func updateUIView(_ uiView: UIView, context: Context) {}
    static func dismantleUIView(_ uiView: UIView, coordinator: AgentChatMountCounter) {
        coordinator.unmounts += 1
    }
}

private actor AgentChatFake {
    let value: AgentChatSnapshot
    let failSend: Bool
    let delaySend: Bool
    var failRead = false
    var cursors: [UInt64?] = []
    var beforeCursors: [UInt64?] = []
    var page: AgentChatSnapshot?
    var pageContinuation: CheckedContinuation<AgentChatSnapshot, Never>?
    var delayPage = false
    var sent: [String] = []
    var sendOptions: [AgentChatSendOptions?] = []
    var controls: [AgentChatControl] = []

    init(value: AgentChatSnapshot, failSend: Bool = false, delaySend: Bool = false) {
        self.value = value
        self.failSend = failSend
        self.delaySend = delaySend
    }

    func setFailRead() { failRead = true }
    func setPage(_ value: AgentChatSnapshot, delayed: Bool = false) { page = value; delayPage = delayed }
    func finishPage() { if let page { pageContinuation?.resume(returning: page); pageContinuation = nil } }
    func snapshot(_ terminal: String, _ cursor: UInt64?, _ before: UInt64?) async throws -> AgentChatSnapshot {
        cursors.append(cursor)
        beforeCursors.append(before)
        if before != nil, let page {
            if delayPage { return await withCheckedContinuation { pageContinuation = $0 } }
            return page
        }
        if failRead { throw URLError(.networkConnectionLost) }
        return value
    }
    func send(_ terminal: String, _ text: String, _ options: AgentChatSendOptions?) async throws {
        sent.append(text)
        sendOptions.append(options)
        if delaySend { try await Task.sleep(for: .milliseconds(30)) }
        if failSend { throw URLError(.networkConnectionLost) }
    }
    func control(_ terminal: String, _ action: AgentChatControl) {
        controls.append(action)
    }
}
