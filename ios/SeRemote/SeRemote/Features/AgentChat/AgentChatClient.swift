import Foundation

/// Wire types for the hooked PTY conversation, deliberately separate from ACP sessions.
struct AgentChatSnapshot: Decodable, Sendable {
    let terminalID: String
    let sessionID: String?
    let agent: String?
    let cursor: UInt64
    let messages: [ChatMessage]
    let deliveries: [AgentChatDelivery]
    let state: String
    let reason: String?
    let hookState: AgentChatHookState?
    let toolCalls: [AgentChatToolCall]
    let reset: Bool

    init(terminalID: String, sessionID: String?, agent: String?, cursor: UInt64,
         messages: [ChatMessage], deliveries: [AgentChatDelivery], state: String,
         reason: String?, hookState: AgentChatHookState?, toolCalls: [AgentChatToolCall] = [], reset: Bool = false) {
        self.terminalID = terminalID
        self.sessionID = sessionID
        self.agent = agent
        self.cursor = cursor
        self.messages = messages
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
        messages = try values.decode([AgentChatWireMessage].self, forKey: .messages).map(\.message)
        deliveries = try values.decode([AgentChatDelivery].self, forKey: .deliveries)
        state = try values.decode(String.self, forKey: .state)
        reason = try values.decodeIfPresent(String.self, forKey: .reason)
        hookState = try values.decodeIfPresent(AgentChatHookState.self, forKey: .hookState)
        toolCalls = try values.decodeIfPresent([AgentChatToolCall].self, forKey: .toolCalls) ?? []
        reset = try values.decodeIfPresent(Bool.self, forKey: .reset) ?? false
    }

    enum CodingKeys: String, CodingKey {
        case terminalID = "terminal_id", sessionID = "session_id", hookState = "hook_state", toolCalls = "tool_calls"
        case agent, cursor, messages, deliveries, state, reason, reset
    }
}

private struct AgentChatWireMessage: Decodable {
    let message: ChatMessage

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
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

    enum CodingKeys: String, CodingKey { case id, role, text, blocks, streaming, seq }
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

struct AgentChatClient: Sendable {
    var snapshot: @MainActor @Sendable (String, UInt64) async throws -> AgentChatSnapshot
    var send: @MainActor @Sendable (String, String) async throws -> Void
    var control: @MainActor @Sendable (String, AgentChatControl) async throws -> Void

    init(http: HostHTTP) {
        snapshot = { terminalID, cursor in
            try await http.get(Self.path(terminalID), query: ["after": String(cursor)])
        }
        send = { terminalID, text in
            let _: AgentChatAcknowledgement = try await http.post(Self.path(terminalID) + "/send", body: ["text": text])
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
        snapshot: @escaping @MainActor @Sendable (String, UInt64) async throws -> AgentChatSnapshot,
        send: @escaping @MainActor @Sendable (String, String) async throws -> Void,
        control: @escaping @MainActor @Sendable (String, AgentChatControl) async throws -> Void
    ) {
        self.snapshot = snapshot
        self.send = send
        self.control = control
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
