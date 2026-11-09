import Foundation

/// Wire types for the hooked PTY conversation, deliberately separate from ACP sessions.
struct AgentChatSnapshot: Decodable, Sendable {
    let terminalID: String
    let sessionID: String?
    let agent: String?
    let cursor: UInt64
    let olderCursor: UInt64?
    let canForceSend: Bool
    let messages: [ChatMessage]
    let messageToolIDs: [String: String]
    let deliveries: [AgentChatDelivery]
    let state: String
    let reason: String?
    let hookState: AgentChatHookState?
    let toolCalls: [AgentChatToolCall]
    let reset: Bool

    init(terminalID: String, sessionID: String?, agent: String?, cursor: UInt64,
         messages: [ChatMessage], deliveries: [AgentChatDelivery], state: String,
         reason: String?, hookState: AgentChatHookState?, toolCalls: [AgentChatToolCall] = [], reset: Bool = false, olderCursor: UInt64? = nil, messageToolIDs: [String: String] = [:], canForceSend: Bool = false) {
        self.terminalID = terminalID
        self.sessionID = sessionID
        self.agent = agent
        self.cursor = cursor
        self.olderCursor = olderCursor
        self.canForceSend = canForceSend
        self.messages = messages
        self.messageToolIDs = messageToolIDs
        self.deliveries = deliveries
        self.state = state
        self.reason = reason
        self.hookState = hookState
        self.toolCalls = toolCalls
        self.reset = reset
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        terminalID = try values.decode(String.self, forKey: .terminalID)
        sessionID = try values.decodeIfPresent(String.self, forKey: .sessionID)
        agent = try values.decodeIfPresent(String.self, forKey: .agent)
        cursor = try values.decode(UInt64.self, forKey: .cursor)
        olderCursor = try values.decodeIfPresent(UInt64.self, forKey: .olderCursor)
        canForceSend = try values.decodeIfPresent(Bool.self, forKey: .canForceSend) ?? false
        let wireMessages = try values.decode([AgentChatWireMessage].self, forKey: .messages)
        messages = wireMessages.map(\.message)
        messageToolIDs = Dictionary(wireMessages.compactMap { wire in
            wire.toolCallID.map { (wire.message.id, $0) }
        }, uniquingKeysWith: { _, last in last })
        deliveries = try values.decode([AgentChatDelivery].self, forKey: .deliveries)
        state = try values.decode(String.self, forKey: .state)
        reason = try values.decodeIfPresent(String.self, forKey: .reason)
        hookState = try values.decodeIfPresent(AgentChatHookState.self, forKey: .hookState)
        toolCalls = try values.decodeIfPresent([AgentChatToolCall].self, forKey: .toolCalls) ?? []
        reset = try values.decodeIfPresent(Bool.self, forKey: .reset) ?? false
    }

    enum CodingKeys: String, CodingKey {
        case terminalID = "terminal_id", sessionID = "session_id", hookState = "hook_state", toolCalls = "tool_calls"
        case olderCursor = "older_cursor"
        case canForceSend = "can_force_send"
        case agent, cursor, messages, deliveries, state, reason, reset
    }
}

private struct AgentChatWireMessage: Decodable {
    let message: ChatMessage
    let toolCallID: String?

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        toolCallID = try values.decodeIfPresent(String.self, forKey: .toolCallID)
        let text = try values.decodeIfPresent(String.self, forKey: .text)
            ?? values.decodeIfPresent([AgentChatTextBlock].self, forKey: .blocks)?.compactMap(\.text).joined(separator: "\n")
            ?? ""
        message = ChatMessage(
            id: try values.decode(String.self, forKey: .id),
            role: try values.decode(ChatRole.self, forKey: .role), text: text,
            streaming: try values.decodeIfPresent(Bool.self, forKey: .streaming) ?? false,
            seq: try values.decodeIfPresent(UInt64.self, forKey: .seq)
        )
    }

    enum CodingKeys: String, CodingKey {
        case id, role, text, blocks, streaming, seq
        case toolCallID = "tool_call_id"
    }
}

private struct AgentChatTextBlock: Decodable {
    let text: String?
}

struct AgentChatToolCall: Decodable, Identifiable, Sendable {
    let id: String
    let title: String
    let status: String
    let detail: String
    let seq: UInt64?

    var card: ToolCard { ToolCard(id: id, title: title, status: status, seq: seq) }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .toolCallId)
        title = try values.decode(String.self, forKey: .title)
        status = try values.decode(String.self, forKey: .status)
        seq = try values.decodeIfPresent(UInt64.self, forKey: .seq)
        let content = try values.decodeIfPresent([Content].self, forKey: .content) ?? []
        detail = content.compactMap { $0.content?.text ?? $0.text }.joined(separator: "\n")
    }

    enum CodingKeys: String, CodingKey { case toolCallId, title, status, content, seq }
    private struct Content: Decodable {
        let text: String?
        let content: AgentChatTextBlock?
    }
}

struct AgentChatDelivery: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let text: String
    /// Keep unknown future states visible as unconfirmed rather than claiming delivery.
    let status: String

    var label: String {
        switch status {
        case "queued": String(localized: "Queued — waiting for the agent")
        case "pending": String(localized: "Sent — awaiting transcript confirmation")
        case "delivered": String(localized: "Confirmed in transcript")
        default: String(localized: "Unconfirmed — not resent automatically")
        }
    }
}

/// Hosts may expose the hook kind directly or include its metadata object.
struct AgentChatHookState: Decodable, Sendable {
    let kind: String?

    init(from decoder: Decoder) throws {
        if let value = try? decoder.singleValueContainer().decode(String.self) {
            kind = value
        } else {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            kind = try values.decodeIfPresent(String.self, forKey: .kind)
                ?? values.decodeIfPresent(String.self, forKey: .status)
        }
    }

    enum CodingKeys: String, CodingKey { case kind, status }
}

enum AgentChatControl: String, Sendable {
    case interrupt, stop
}

struct AgentChatSendOptions: Sendable {
    let force: Bool
    let deliveryID: String
}

struct AgentChatClient: Sendable {
    private var requestSnapshot: @MainActor @Sendable (String, UInt64?, UInt64?) async throws -> AgentChatSnapshot
    private var requestSend: @MainActor @Sendable (String, String, AgentChatSendOptions?) async throws -> Void
    var control: @MainActor @Sendable (String, AgentChatControl) async throws -> Void

    init(http: HostHTTP) {
        requestSnapshot = { terminalID, cursor, before in
            try await http.get(Self.path(terminalID), query: Self.snapshotQuery(after: cursor, before: before))
        }
        requestSend = { terminalID, text, options in
            let _: AgentChatAcknowledgement = try await http.post(Self.path(terminalID) + "/send", body: Self.sendBody(text: text, options: options))
        }
        control = { terminalID, action in
            // HostHTTP.post requires a non-null data payload; the control route
            // intentionally acknowledges with data:null. Keep this adapter local.
            guard let url = URL(string: http.origin.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + Self.path(terminalID) + "/control") else {
                throw HostError.unexpected("Invalid agent chat control URL")
            }
            var request = URLRequest(url: url, timeoutInterval: 20)
            request.httpMethod = "POST"
            request.assumesHTTP3Capable = false
            request.setValue("application/json", forHTTPHeaderField: "content-type")
            http.credentials.apply(to: &request)
            request.httpBody = try WireJSON.data(from: ["action": action.rawValue])
            let (data, response) = try await HostURLSession.shared.data(for: request)
            guard let response = response as? HTTPURLResponse else {
                throw HostError.network(String(localized: "The tunnel returned an unexpected response."))
            }
            if let error = HostHTTP.responseError(statusCode: response.statusCode, data: data) {
                error.publishAccessFailure(origin: http.origin)
                throw error
            }
            try Self.validateAcknowledgement(data)
        }
    }

    init(
        snapshot: @escaping @MainActor @Sendable (String, UInt64?, UInt64?) async throws -> AgentChatSnapshot,
        send: @escaping @MainActor @Sendable (String, String, AgentChatSendOptions?) async throws -> Void,
        control: @escaping @MainActor @Sendable (String, AgentChatControl) async throws -> Void
    ) {
        self.requestSnapshot = snapshot
        self.requestSend = send
        self.control = control
    }

    func snapshot(_ terminalID: String, _ after: UInt64? = nil, before: UInt64? = nil) async throws -> AgentChatSnapshot {
        try await requestSnapshot(terminalID, after, before)
    }

    func send(_ terminalID: String, _ text: String, options: AgentChatSendOptions? = nil) async throws {
        try await requestSend(terminalID, text, options)
    }

    static func sendBody(text: String, options: AgentChatSendOptions?) -> [String: Any] {
        var body: [String: Any] = ["text": text]
        if let options {
            body["force"] = options.force
            body["delivery_id"] = options.deliveryID
        }
        return body
    }

    static func snapshotQuery(after: UInt64?, before: UInt64?) -> [String: String] {
        var query: [String: String] = [:]
        if let after { query["after"] = String(after) }
        if let before { query["before"] = String(before) }
        return query
    }

    static func validateAcknowledgement(_ data: Data) throws {
        let value = try JSONDecoder().decode(IpcResult<AgentChatAcknowledgement>.self, from: data)
        guard value.success else {
            throw HostError.ipc(value.error ?? String(localized: "The host rejected the request."), value.code ?? "AGENT_CHAT_ERROR")
        }
    }

    static func path(_ terminalID: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#%")
        return "/agent-chat/" + (terminalID.addingPercentEncoding(withAllowedCharacters: allowed) ?? "")
    }
}

/// Acknowledgement payloads are intentionally not interpreted as transcript delivery.
private struct AgentChatAcknowledgement: Decodable {}
