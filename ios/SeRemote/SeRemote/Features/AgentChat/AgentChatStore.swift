import Foundation
import Observation
import OSLog

@MainActor
@Observable
final class AgentChatStore {
    let terminalID: String
    private let client: AgentChatClient
    private(set) var sessionID: String?
    private(set) var agent: String?
    private(set) var cursor: UInt64 = 0
    private(set) var messages: [ChatMessage] = []
    private(set) var deliveries: [AgentChatDelivery] = []
    private(set) var toolCalls: [AgentChatToolCall] = []
    private(set) var hookKind: String?
    private(set) var isReady = false
    private(set) var isLoading = true
    private(set) var isSubmitting = false
    private(set) var isControlling = false
    private(set) var errorMessage: String?
    private(set) var reason: String?
    private(set) var submissionNotice: String?
    var draft = ""
    var canView: Bool
    var canInput: Bool
    private var refreshing = false
    private var mutationError: String?

    init(terminalID: String, client: AgentChatClient, canView: Bool = true, canInput: Bool = true) {
        self.terminalID = terminalID
        self.client = client
        self.canView = canView
        self.canInput = canInput
    }

    var canSend: Bool {
        canView && canInput && isReady && !isSubmitting && !isControlling
            && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var canControl: Bool { canView && canInput && isReady && !isSubmitting && !isControlling }

    /// One reader owns polling. Transient failures leave the transcript intact and expose a terminal fallback.
    func follow() async {
        HostLog.ui.info("agent_chat follow started")
        defer { HostLog.ui.info("agent_chat follow ended") }
        while !Task.isCancelled {
            await refresh()
            do { try await Task.sleep(for: .seconds(1)) } catch { break }
        }
    }

    func refresh() async {
        guard canView else {
            isLoading = false
            isReady = false
            messages = []
            deliveries = []
            toolCalls = []
            reason = String(localized: "This link does not allow viewing terminal conversations.")
            return
        }
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false; isLoading = false }
        do {
            let snapshot = try await client.snapshot(terminalID, cursor)
            guard !Task.isCancelled, canView else { return }
            apply(snapshot)
        } catch is CancellationError {
            return
        } catch {
            isReady = false
            errorMessage = String(localized: "Conversation unavailable. Return to the terminal or retry.")
            // Never log the response body, prompt, terminal id, or credentials.
            HostLog.ui.error("agent_chat snapshot failed")
        }
    }

    func apply(_ snapshot: AgentChatSnapshot) {
        guard snapshot.terminalID == terminalID, canView else { return }
        // /clear, /resume and a host cursor-generation reset replace old history, not concatenate it.
        let reset = snapshot.reset || sessionID != snapshot.sessionID || snapshot.cursor < cursor
        if reset {
            messages = []
            deliveries = []
            submissionNotice = nil
            mutationError = nil
        }
        sessionID = snapshot.sessionID
        agent = snapshot.agent
        cursor = snapshot.cursor
        hookKind = snapshot.hookState?.kind
        var indices = Dictionary(messages.enumerated().map { ($0.element.id, $0.offset) }, uniquingKeysWith: { _, last in last })
        for message in snapshot.messages {
            if let index = indices[message.id] {
                messages[index] = message
            } else {
                indices[message.id] = messages.count
                messages.append(message)
            }
        }
        deliveries = snapshot.deliveries
        toolCalls = snapshot.toolCalls
        reason = snapshot.reason
        isReady = snapshot.state == "ready"
        isLoading = false
        errorMessage = mutationError
    }

    /// Never retries POST. Only a user action can create a second delivery.
    func send() async {
        guard canSend else { return }
        let text = draft
        let sendingSession = sessionID
        isSubmitting = true
        mutationError = nil
        errorMessage = nil
        defer { isSubmitting = false }
        HostLog.ui.info("agent_chat send requested")
        do {
            try await client.send(terminalID, text)
            guard sessionID == sendingSession else { return }
            if draft == text { draft = "" }
            submissionNotice = String(localized: "Sent — awaiting transcript confirmation")
            await refresh()
        } catch {
            submissionNotice = String(localized: "Unconfirmed — not resent automatically")
            mutationError = String(localized: "Send could not be confirmed. Check the terminal before sending again.")
            errorMessage = mutationError
            HostLog.ui.error("agent_chat send unconfirmed; no retry")
        }
    }

    func control(_ action: AgentChatControl) async {
        guard canControl else { return }
        isControlling = true
        mutationError = nil
        errorMessage = nil
        defer { isControlling = false }
        HostLog.ui.info("agent_chat control requested: \(action.rawValue, privacy: .public)")
        do {
            try await client.control(terminalID, action)
            await refresh()
        } catch {
            mutationError = String(localized: "Control could not be confirmed. Use the terminal instead.")
            errorMessage = mutationError
            HostLog.ui.error("agent_chat control failed; no retry")
        }
    }
}
