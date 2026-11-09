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
        state: String = "ready", terminal: String = "pty-1"
    ) -> AgentChatSnapshot {
        AgentChatSnapshot(terminalID: terminal, sessionID: session, agent: "claude", cursor: cursor,
                          messages: messages, deliveries: deliveries, state: state, reason: nil, hookState: nil)
    }

    private func message(_ id: String, _ text: String) -> ChatMessage {
        ChatMessage(id: id, role: .agent, text: text, streaming: false, seq: nil)
    }

    private func store(_ fake: AgentChatFake, canView: Bool = true, canInput: Bool = true) -> AgentChatStore {
        AgentChatStore(terminalID: "pty-1", client: AgentChatClient(
            snapshot: { terminal, cursor in try await fake.snapshot(terminal, cursor) },
            send: { terminal, text in try await fake.send(terminal, text) },
            control: { terminal, action in try await fake.control(terminal, action) }
        ), canView: canView, canInput: canInput)
    }

    @Test("Wire snapshot reuses ACP message shape and includes hook metadata and receipts")
    func wireSnapshot() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":"s1","agent":"claude","cursor":42,"messages":[{"id":"m1","role":"agent","text":"hello","streaming":false,"seq":7}],"deliveries":[{"id":"d1","text":"prompt","status":"pending"}],"state":"ready","hook_state":{"kind":"permission_request"}}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.terminalID == "pty-1")
        #expect(value.sessionID == "s1")
        #expect(value.cursor == 42)
        #expect(value.messages.first?.role == .agent)
        #expect(value.hookState?.kind == "permission_request")
        #expect(value.deliveries.first?.status == "pending")
    }

    @Test("Degraded snapshot tolerates null session and string hook state")
    func degradedWire() throws {
        let data = Data(#"{"terminal_id":"pty-1","session_id":null,"agent":null,"cursor":0,"messages":[],"deliveries":[],"state":"degraded","reason":"missing_hook","hook_state":"not_installed"}"#.utf8)
        let value = try JSONDecoder().decode(AgentChatSnapshot.self, from: data)
        #expect(value.sessionID == nil)
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
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
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

    @Test("Host cursor generation reset replaces stale messages")
    func resetsCursor() {
        let store = store(AgentChatFake(value: snapshot()))
        store.apply(snapshot(cursor: 100, messages: [message("old", "old")]))
        store.apply(snapshot(cursor: 1, messages: [message("new", "new")]))
        #expect(store.messages.map(\.id) == ["new"])
        #expect(store.cursor == 1)
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
    var cursors: [UInt64] = []
    var sent: [String] = []
    var controls: [AgentChatControl] = []

    init(value: AgentChatSnapshot, failSend: Bool = false, delaySend: Bool = false) {
        self.value = value
        self.failSend = failSend
        self.delaySend = delaySend
    }

    func setFailRead() { failRead = true }
    func snapshot(_ terminal: String, _ cursor: UInt64) throws -> AgentChatSnapshot {
        cursors.append(cursor)
        if failRead { throw URLError(.networkConnectionLost) }
        return value
    }
    func send(_ terminal: String, _ text: String) async throws {
        sent.append(text)
        if delaySend { try await Task.sleep(for: .milliseconds(30)) }
        if failSend { throw URLError(.networkConnectionLost) }
    }
    func control(_ terminal: String, _ action: AgentChatControl) {
        controls.append(action)
    }
}
