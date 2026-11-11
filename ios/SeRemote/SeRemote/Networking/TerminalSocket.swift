import Foundation

/// `screen_read` nests the status document that `screen_status` returns flat.
/// Never expose text when the host cannot prove a ready, synchronized observer.
struct HostScreenReadReply: Decodable, Sendable, Hashable {
    let status: HostScreenObservation
    let text: String?
    let appliedSeq: Int?
    let truncated: Bool

    private enum CodingKeys: String, CodingKey {
        case status, text, appliedSeq, truncated
    }

    init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        let observation = (try? fields.decode(HostScreenObservation.self, forKey: .status))
            ?? .unavailable(reason: .protocol)
        let sequence = try fields.decodeIfPresent(Int.self, forKey: .appliedSeq)
        // A reply-level sequence that disagrees with the status is a protocol fault.
        let consistent = sequence.map { $0 == observation.appliedSeq } ?? true
        status = consistent ? observation : .unavailable(reason: .protocol)
        appliedSeq = status.isReady ? sequence : nil
        let rawText = try fields.decodeIfPresent(String.self, forKey: .text)
        // Text is only trustworthy when its own sequence proves it matches the
        // ready observation. A missing or null sequence cannot prove that.
        let correlated = sequence != nil && sequence == status.appliedSeq
        text = status.isReady && correlated ? rawText : nil
        truncated = try fields.decode(Bool.self, forKey: .truncated)
    }
}

struct SpawnedTerminal: Decodable, Sendable {
    let id: String
    var shell: String?
    var cwd: String?
    var pid: Int?
    var cols: Int?
    var rows: Int?
    var terminalScreenEngine: HostTerminalScreenEngine?
    var claim: String?
}

struct TerminalAttachResult: Decodable, Sendable {
    let id: String
    var shell: String?
    var cwd: String?
    var pid: Int?
    var cols: Int?
    var rows: Int?
    var terminalScreenEngine: HostTerminalScreenEngine?
    var latestSeq: Int?
    var gap: Bool?
}

/// Host reply for `set_display_mode`: the authoritative live grid after the
/// adopt/park/restore decision (e.g. the parked desktop size on restore).
struct TerminalDisplayModeState: Decodable, Sendable {
    var mode: String
    var cols: Int
    var rows: Int
}

struct LiveTerminalList: Decodable, Sendable {
    var terminals: [LiveTerminalSummary]
}

struct LiveTerminalSummary: Decodable, Identifiable, Sendable {
    let id: String
    var shell: String?
    var cwd: String?
    var pid: Int?
    var cols: Int?
    var rows: Int?
    var terminalScreenEngine: HostTerminalScreenEngine?
    var conversationId: String?
    var projectId: String?
    var title: String?
    var gitBranch: String?
    var screenObservation: HostScreenObservation?
}

@MainActor
@Observable
final class TerminalSocket {
    private(set) var isConnected = false
    private(set) var isRevoked = false
    /// Bumped on every authenticated connection. A watch lives on the socket
    /// connection that took it, so a caller holding one must compare this
    /// generation before assuming the host still replays to it.
    private(set) var connectionGeneration = 0
    private var origin: URL?

    var onBytes: (@MainActor (String, Data) -> Void)?
    var onExit: (@MainActor (String) -> Void)?
    var onCatalogChanged: (@MainActor () -> Void)?
    var onDisplayModeChanged: (@MainActor (String, String, Int, Int) -> Void)?

    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var pending: [String: CheckedContinuation<Data, Error>] = [:]
    private var requestSerial = 0
    private var openGate: HostWebSocketOpenGate?

    func connect(origin: URL, credentials: HostCredentials) async throws {
        guard !isRevoked else { throw HostError.deviceRevoked }
        self.origin = origin
        stop()
        guard let token = credentials.bearer, !token.isEmpty else {
            throw HostError.unexpected(String(localized: "This access link is missing its token. Scan the QR again."))
        }
        var components = URLComponents(url: origin, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.scheme = origin.scheme?.lowercased() == "https" ? "wss" : "ws"
        components.path = "/terminal/ws"
        components.fragment = nil
        components.query = nil
        guard let wsURL = components.url else {
            throw HostError.unexpected("Invalid terminal URL")
        }
        var urlRequest = URLRequest(url: wsURL, timeoutInterval: HostTunnelSession.handshakeSeconds)
        urlRequest.assumesHTTP3Capable = false
        credentials.apply(to: &urlRequest)
        let gate = HostWebSocketOpenGate { [weak self] in
            Task { @MainActor in self?.handleDeviceRevoked(origin: origin) }
        }
        openGate = gate
        let session = HostTunnelSession.make(delegate: gate)
        self.session = session
        let task = session.webSocketTask(with: urlRequest)
        self.task = task
        task.resume()
        receiveTask = Task { await self.receiveLoop() }
        do {
            try await gate.waitForOpen()
            _ = try await self.request("authenticate", payload: ["token": token], as: EmptyPayload.self)
            guard !isRevoked else { throw HostError.deviceRevoked }
            isConnected = true
            connectionGeneration &+= 1
            HostLog.session.info("terminal WebSocket authenticated")
        } catch {
            if case HostError.deviceRevoked = error {
                handleDeviceRevoked(origin: origin)
            }
            HostLog.session.error("terminal WebSocket connect failed: \(error.localizedDescription, privacy: .public)")
            stop()
            throw isRevoked ? HostError.deviceRevoked : error
        }
    }

    func spawn(conversationId: String?, projectId: String?, cols: Int, rows: Int) async throws -> SpawnedTerminal {
        var payload: [String: Any] = [
            "cols": cols,
            "rows": rows
        ]
        if let conversationId, !conversationId.isEmpty {
            payload["conversationId"] = conversationId
            payload["cwdSource"] = "workspace"
        }
        if let projectId, !projectId.isEmpty {
            payload["projectId"] = projectId
        }
        if payload["conversationId"] == nil && payload["projectId"] == nil {
            throw HostError.unexpected(String(localized: "A conversation or project is required."))
        }
        return try await request("spawn", payload: payload, as: SpawnedTerminal.self)
    }

    func attach(terminalId: String, claim: String, lastSeq: Int) async throws -> TerminalAttachResult {
        try await request(
            "attach",
            payload: [
                "terminalId": terminalId,
                "claim": claim,
                "lastSeq": lastSeq
            ],
            as: TerminalAttachResult.self
        )
    }

    func list(conversationId: String?, projectId: String?) async throws -> [LiveTerminalSummary] {
        var payload: [String: Any] = [:]
        if let conversationId, !conversationId.isEmpty {
            payload["conversationId"] = conversationId
        }
        if let projectId, !projectId.isEmpty {
            payload["projectId"] = projectId
        }
        let reply: LiveTerminalList = try await request(
            "list",
            payload: payload,
            as: LiveTerminalList.self
        )
        return reply.terminals
    }

    /// Read-only host status. Called only by an explicit user refresh, not a poll.
    func screenStatus(terminalId: String) async throws -> HostScreenObservation {
        try await request(
            "screen_status",
            payload: ["terminalId": terminalId],
            as: HostScreenObservation.self
        )
    }

    /// Read a host-observed screen without sending input or changing ownership.
    func screenRead(terminalId: String, maxLines: Int? = nil) async throws -> HostScreenReadReply {
        var payload: [String: Any] = ["terminalId": terminalId]
        if let maxLines { payload["maxLines"] = maxLines }
        HostLog.session.debug("Host screen read request")
        do {
            return try await request("screen_read", payload: payload, as: HostScreenReadReply.self)
        } catch {
            HostLog.session.error("Host screen read failed")
            throw error
        }
    }

    func watch(terminalId: String, lastSeq: Int) async throws -> TerminalAttachResult {
        try await request(
            "watch",
            payload: [
                "terminalId": terminalId,
                "lastSeq": lastSeq
            ],
            as: TerminalAttachResult.self
        )
    }

    func detach(terminalId: String) async {
        _ = try? await request(
            "detach",
            payload: ["terminalId": terminalId],
            as: EmptyPayload.self
        )
    }

    func write(terminalId: String, data: String) async throws {
        _ = try await request(
            "write",
            payload: ["terminalId": terminalId, "data": data],
            as: EmptyPayload.self
        )
    }

    func resize(terminalId: String, cols: Int, rows: Int) async throws {
        _ = try await request(
            "resize",
            payload: ["terminalId": terminalId, "cols": cols, "rows": rows],
            as: EmptyPayload.self
        )
    }

    func setDisplayMode(
        terminalId: String,
        mode: String,
        cols: Int? = nil,
        rows: Int? = nil
    ) async throws -> TerminalDisplayModeState {
        var payload: [String: Any] = [
            "terminalId": terminalId,
            "mode": mode
        ]
        if let cols {
            payload["cols"] = cols
        }
        if let rows {
            payload["rows"] = rows
        }
        return try await request("set_display_mode", payload: payload, as: TerminalDisplayModeState.self)
    }

    /// Cancel transport work only. PTYs, mounted terminal surfaces and geometry
    /// ownership belong to the workspace and must not be torn down here.
    func revoke() {
        guard !isRevoked else { return }
        isRevoked = true
        openGate?.failOpen(HostError.deviceRevoked)
        failPending(HostError.deviceRevoked)
        stop()
    }

    private func handleDeviceRevoked(origin: URL) {
        guard !isRevoked else { return }
        revoke()
        HostError.deviceRevoked.publishAccessFailure(origin: origin)
    }

    /// Transport-level liveness for a socket the app still believes is
    /// connected. A socket that survived a network-path change reports the
    /// stale-path error here immediately, which lets the session rebuild the
    /// transport instead of surfacing a failed request to the user.
    func probeLiveness() async -> TransportLiveness {
        guard isConnected, let task else { return .dead }
        return await WebSocketLiveness.probe(task)
    }

    func stop() {
        receiveTask?.cancel()
        receiveTask = nil
        openGate?.failOpen(HostError.network(String(localized: "Disconnected from the host.")))
        openGate = nil
        failPending(HostError.network(String(localized: "Disconnected from the host.")))
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
        isConnected = false
    }

    private func request<T: Decodable>(
        _ type: String,
        payload: [String: Any],
        as _: T.Type,
        timeoutSeconds: Double = 20
    ) async throws -> T {
        guard !isRevoked else { throw HostError.deviceRevoked }
        requestSerial += 1
        let id = "term-\(requestSerial)"
        let body: [String: Any] = ["id": id, "type": type, "payload": payload]
        let data = try WireJSON.data(from: body)
        let reply = try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(timeoutSeconds))
                guard let self, let waiting = self.pending.removeValue(forKey: id) else { return }
                HostLog.session.error("Terminal request timed out type=\(type, privacy: .public)")
                waiting.resume(throwing: HostError.network(String(localized: "The terminal host did not respond. Try again.")))
            }
            guard let task else {
                pending.removeValue(forKey: id)?.resume(
                    throwing: HostError.network(String(localized: "Disconnected from the host."))
                )
                return
            }
            task.send(.string(String(data: data, encoding: .utf8) ?? "")) { [weak self] error in
                Task { @MainActor in
                    if let error {
                        // The task outlived its connection: stop claiming it is
                        // usable so the next request reconnects instead of
                        // sending into a dead socket again.
                        self?.isConnected = false
                        self?.pending.removeValue(forKey: id)?.resume(throwing: HostError.network(error.localizedDescription))
                    }
                }
            }
        }
        if T.self == EmptyPayload.self {
            return EmptyPayload() as! T
        }
        return try JSONDecoder().decode(T.self, from: reply)
    }

    private func receiveLoop() async {
        while let task, !Task.isCancelled {
            do {
                let message = try await task.receive()
                let data: Data
                switch message {
                case .data(let value):
                    data = value
                case .string(let value):
                    data = Data(value.utf8)
                @unknown default:
                    continue
                }
                handleFrame(data)
            } catch {
                guard self.task === task, !isRevoked else { return }
                if task.closeCode == .policyViolation, let origin {
                    handleDeviceRevoked(origin: origin)
                } else {
                    isConnected = false
                    failPending(HostError.network(error.localizedDescription))
                }
                return
            }
        }
    }

    private func handleFrame(_ data: Data) {
        guard let object = try? WireJSON.object(from: data) else { return }
        if let id = object["id"] as? String, object["success"] is Bool {
            handleReply(id: id, object: object)
            return
        }
        if let output = Self.output(from: object) {
            onBytes?(output.terminalId, output.data)
            return
        }
        guard let type = object["type"] as? String else { return }
        switch type {
        case "event":
            if let payload = object["payload"] as? [String: Any],
               let eventType = payload["type"] as? String {
                if eventType == "exit", let terminalId = payload["terminal_id"] as? String {
                    onExit?(terminalId)
                }
                if eventType == "display_mode_changed",
                   let terminalId = payload["terminal_id"] as? String,
                   let mode = payload["mode"] as? String {
                    let cols = Self.intValue(payload["cols"]) ?? 0
                    let rows = Self.intValue(payload["rows"]) ?? 0
                    onDisplayModeChanged?(terminalId, mode, cols, rows)
                }
                if eventType == "spawned" || eventType == "exit" {
                    HostLog.session.info("Host terminal catalog changed")
                    onCatalogChanged?()
                }
            }
        default:
            break
        }
    }

    /// Pure mapping for the two terminal-output frames the host sends over the
    /// v2 JSON protocol: `data` for live bytes and `replay` for backlog chunks.
    /// Keeping it pure lets the wire contract be tested without a socket.
    nonisolated static func output(from object: [String: Any]) -> (terminalId: String, data: Data)? {
        switch object["type"] as? String {
        case "data":
            guard let terminalId = object["terminalId"] as? String else { return nil }
            return (terminalId, bytes(from: object["data"]))
        case "replay":
            guard let terminalId = object["terminalId"] as? String,
                  let chunks = object["chunks"] as? [[String: Any]] else { return nil }
            var data = Data()
            for chunk in chunks {
                data.append(bytes(from: chunk["data"]))
            }
            return (terminalId, data)
        default:
            return nil
        }
    }

    /// Event payloads come through JSONSerialization, so numbers arrive as
    /// NSNumber; bridge both spellings.
    private static func intValue(_ any: Any?) -> Int? {
        if let number = any as? NSNumber {
            return number.intValue
        }
        return any as? Int
    }

    private func failPending(_ error: Error) {
        let pending = self.pending
        self.pending.removeAll()
        for (_, continuation) in pending {
            continuation.resume(throwing: error)
        }
    }

    private func handleReply(id: String, object: [String: Any]) {
        guard let continuation = pending.removeValue(forKey: id) else { return }
        let success = object["success"] as? Bool ?? false
        if success {
            if let payload = object["data"], JSONSerialization.isValidJSONObject(payload),
               let encoded = try? JSONSerialization.data(withJSONObject: payload) {
                continuation.resume(returning: encoded)
            } else {
                continuation.resume(returning: Data("{}".utf8))
            }
        } else {
            let message = (object["error"] as? String) ?? String(localized: "The host rejected the request.")
            let code = (object["code"] as? String) ?? "NETWORK_ERROR"
            if let accessError = HostError.accessFailure(code: code, capability: object["capability"] as? String) {
                continuation.resume(throwing: accessError)
                if let origin {
                    if case .deviceRevoked = accessError {
                        handleDeviceRevoked(origin: origin)
                    } else {
                        accessError.publishAccessFailure(origin: origin)
                    }
                }
            } else {
                continuation.resume(throwing: HostError.ipc(message, code))
            }
        }
    }

    nonisolated private static func bytes(from raw: Any?) -> Data {
        if let numbers = raw as? [Any] {
            let bytes = numbers.compactMap { item -> UInt8? in
                if let number = item as? NSNumber { return UInt8(truncating: number) }
                if let number = item as? Int { return UInt8(clamping: number) }
                return nil
            }
            return Data(bytes)
        }
        if let text = raw as? String {
            return Data(text.utf8)
        }
        return Data()
    }
}
