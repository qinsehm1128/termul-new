import Foundation

struct WsErrorBody: Decodable, Sendable {
    let code: String
    let message: String
}

struct AcpAuthenticateReply: Decodable, Sendable {
    var historyMode: String?
    var runtimePolicy: AcpRuntimePolicy?
}

struct AcpRuntimePolicy: Decodable, Sendable {
    var turnTimeoutMs: Double? = nil
    var promptInactivityTimeoutMs: Double? = nil
    var permissionReconnectGraceMs: Double? = nil
    var pingIntervalMs: Double? = nil
    var pongTimeoutMs: Double? = nil
}

@MainActor
@Observable
final class AcpSocket {
    enum State: Equatable {
        case idle
        case connecting
        case connected
        case revoked
        case failed(String)
    }

    private(set) var state: State = .idle
    private(set) var historyMode = "live_only"

    var onEvent: (@MainActor (String, String?, UInt64, Data) -> Void)?

    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var heartbeat: Task<Void, Never>?
    private var runtimePolicy = AcpRuntimePolicy()
    private var pending: [String: CheckedContinuation<Data, Error>] = [:]
    private var requestSerial = 0
    private var origin: URL?
    private var openGate: HostWebSocketOpenGate?
    private var connectTask: Task<Void, Error>?
    private var connectSerial = 0
    private(set) var lastInboundAt: ContinuousClock.Instant?

    func connect(origin: URL, credentials: HostCredentials) async throws {
        guard state != .revoked else { throw HostError.deviceRevoked }
        if state == .connected, self.origin == origin { return }
        if let inFlight = connectTask, self.origin == origin {
            try await inFlight.value
            return
        }
        connectSerial &+= 1
        let serial = connectSerial
        self.origin = origin
        let task = Task<Void, Error> { @MainActor [weak self] in
            guard let self else { throw CancellationError() }
            defer {
                if self.connectSerial == serial {
                    self.connectTask = nil
                }
            }
            try await self.performConnect(origin: origin, credentials: credentials)
        }
        connectTask = task
        try await task.value
    }

    private func performConnect(origin: URL, credentials: HostCredentials) async throws {
        guard state != .revoked else { throw HostError.deviceRevoked }
        stopSocketOnly()
        state = .connecting
        runtimePolicy = AcpRuntimePolicy()
        guard let token = credentials.bearer, !token.isEmpty else {
            throw HostError.unexpected(String(localized: "This access link is missing its token. Scan the QR again."))
        }
        let wsURL = Self.wsURL(origin: origin, path: "/ws")
        var urlRequest = URLRequest(url: wsURL, timeoutInterval: HostTunnelSession.handshakeSeconds)
        urlRequest.assumesHTTP3Capable = false
        credentials.apply(to: &urlRequest)
        let gate = HostWebSocketOpenGate { [weak self] in
            Task { @MainActor in self?.handleDeviceRevoked(origin: origin) }
        }
        openGate = gate
        let session = HostTunnelSession.make(delegate: gate)
        self.session = session
        let wsTask = session.webSocketTask(with: urlRequest)
        self.task = wsTask
        wsTask.resume()
        receiveTask = Task { await self.receiveLoop(for: wsTask) }

        do {
            try await gate.waitForOpen()
            let data = try await request("authenticate", payload: ["token": token])
            if let reply = try? JSONDecoder().decode(AcpAuthenticateReply.self, from: data) {
                historyMode = reply.historyMode ?? "live_only"
                runtimePolicy = reply.runtimePolicy ?? AcpRuntimePolicy()
            }
            guard state != .revoked else { throw HostError.deviceRevoked }
            lastInboundAt = ContinuousClock.now
            state = .connected
            startHeartbeat()
        } catch {
            if case HostError.deviceRevoked = error {
                handleDeviceRevoked(origin: origin)
            } else if state != .revoked {
                state = .failed(error.localizedDescription)
            }
            stopSocketOnly()
            throw state == .revoked ? HostError.deviceRevoked : error
        }
    }

    func request(_ type: String, payload: [String: Any] = [:], timeoutSeconds: Double? = 45) async throws -> Data {
        guard state != .revoked else { throw HostError.deviceRevoked }
        requestSerial += 1
        let id = "ios-\(requestSerial)"
        let body: [String: Any] = ["id": id, "type": type, "payload": payload]
        let data = try WireJSON.data(from: body)
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            if let timeoutSeconds {
                Task { @MainActor [weak self] in
                    try? await Task.sleep(for: .seconds(timeoutSeconds))
                    guard let self, let waiting = self.pending.removeValue(forKey: id) else { return }
                    HostLog.session.error("ACP request timed out")
                    waiting.resume(throwing: HostError.network(String(localized: "The host did not finish the agent request. Try again, or start the agent on the desktop.")))
                }
            }
            task?.send(.string(String(data: data, encoding: .utf8) ?? "")) { [weak self] error in
                Task { @MainActor in
                    if let error {
                        self?.pending.removeValue(forKey: id)?.resume(throwing: HostError.network(error.localizedDescription))
                    }
                }
            }
        }
    }

    func request<T: Decodable>(
        _ type: String,
        payload: [String: Any] = [:],
        as _: T.Type,
        timeoutSeconds: Double? = 45
    ) async throws -> T {
        let data = try await request(type, payload: payload, timeoutSeconds: timeoutSeconds)
        if T.self == EmptyPayload.self {
            return EmptyPayload() as! T
        }
        if data.isEmpty {
            throw HostError.unexpected("Empty reply for \(type)")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    func sendControl(_ type: String) {
        guard let data = try? WireJSON.data(from: ["type": type]) else { return }
        task?.send(.string(String(data: data, encoding: .utf8) ?? "")) { _ in }
    }

    func handleLifecycle(isBackground: Bool) {
        sendControl(isBackground ? "background" : "foreground")
        if !isBackground, state == .connected {
            Task { try? await request("ping") }
        }
    }

    /// Transport-level liveness for a socket the app still believes is
    /// connected. Used after a resume or a path change so a stranded socket is
    /// rebuilt before the next user request has to fail.
    func isRecentlyActive(within window: Duration = .seconds(8)) -> Bool {
        guard state == .connected, task != nil, let lastInboundAt else { return false }
        return lastInboundAt.duration(to: ContinuousClock.now) < window
    }

    func probeLiveness() async -> TransportLiveness {
        guard state == .connected, let task else { return .dead }
        if isRecentlyActive(within: .seconds(6)) {
            return .alive
        }
        let result = await WebSocketLiveness.probe(task)
        if result == .alive {
            lastInboundAt = ContinuousClock.now
        }
        return result
    }

    func revoke() {
        guard state != .revoked else { return }
        state = .revoked
        openGate?.failOpen(HostError.deviceRevoked)
        failPending(HostError.deviceRevoked)
        stop()
    }

    private func handleDeviceRevoked(origin: URL) {
        guard state != .revoked else { return }
        revoke()
        HostError.deviceRevoked.publishAccessFailure(origin: origin)
    }

    func stop() {
        connectSerial &+= 1
        connectTask?.cancel()
        connectTask = nil
        stopSocketOnly()
    }

    private func stopSocketOnly() {
        heartbeat?.cancel()
        heartbeat = nil
        receiveTask?.cancel()
        receiveTask = nil
        openGate?.failOpen(HostError.network(String(localized: "Disconnected from the host.")))
        openGate = nil
        for (_, continuation) in pending {
            continuation.resume(throwing: HostError.network(String(localized: "Disconnected from the host.")))
        }
        pending.removeAll()
        let oldTask = task
        task = nil
        lastInboundAt = nil
        oldTask?.cancel(with: .goingAway, reason: nil)
        session?.invalidateAndCancel()
        session = nil
        switch state {
        case .failed, .revoked:
            break
        default:
            state = .idle
        }
    }

    private func startHeartbeat() {
        heartbeat?.cancel()
        let interval = max(5, min((runtimePolicy.pingIntervalMs ?? 30_000) / 1_000, 300))
        let timeout = max(2, min((runtimePolicy.pongTimeoutMs ?? 45_000) / 1_000, 120))
        heartbeat = Task { [weak self] in
            while let self, !Task.isCancelled {
                try? await Task.sleep(for: .seconds(interval))
                guard !Task.isCancelled else { return }
                do {
                    _ = try await self.request("ping", timeoutSeconds: timeout)
                } catch {
                    guard !Task.isCancelled else { return }
                    HostLog.session.error("ACP heartbeat failed: \(error.localizedDescription, privacy: .public)")
                    state = .failed(error.localizedDescription)
                    stop()
                    return
                }
            }
        }
    }

    private func receiveLoop(for loopTask: URLSessionWebSocketTask) async {
        while self.task === loopTask, !Task.isCancelled {
            do {
                let message = try await loopTask.receive()
                guard self.task === loopTask else { return }
                lastInboundAt = ContinuousClock.now
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
                guard self.task === loopTask, state != .revoked else { return }
                if loopTask.closeCode == .policyViolation, let origin {
                    handleDeviceRevoked(origin: origin)
                    return
                }
                if !Task.isCancelled {
                    failPending(error)
                    openGate?.failOpen(HostError.network(error.localizedDescription))
                    openGate = nil
                    if case .connected = state {
                        state = .failed(error.localizedDescription)
                    }
                }
                return
            }
        }
    }

    private func handleFrame(_ data: Data) {
        guard let object = try? WireJSON.object(from: data) else { return }
        if let id = object["id"] as? String, object["ok"] is Bool {
            handleReply(id: id, object: object, data: data)
            return
        }
        if let type = object["type"] as? String {
            let sid = object["sid"] as? String
            let seq = (object["seq"] as? NSNumber)?.uint64Value ?? 0
            let payload = (try? JSONSerialization.data(withJSONObject: object["payload"] ?? [:])) ?? Data("{}".utf8)
            if type == "auth_required" {
                return
            }
            onEvent?(type, sid, seq, payload)
        }
    }

    private func handleReply(id: String, object: [String: Any], data: Data) {
        guard let continuation = pending.removeValue(forKey: id) else { return }
        let ok = object["ok"] as? Bool ?? false
        if ok {
            if let payload = object["payload"] {
                if JSONSerialization.isValidJSONObject(payload),
                   let encoded = try? JSONSerialization.data(withJSONObject: payload) {
                    continuation.resume(returning: encoded)
                } else if payload is NSNull {
                    continuation.resume(returning: Data("{}".utf8))
                } else if let encoded = try? JSONSerialization.data(withJSONObject: [payload]) {
                    continuation.resume(returning: encoded)
                } else {
                    continuation.resume(returning: Data("{}".utf8))
                }
            } else {
                continuation.resume(returning: Data("{}".utf8))
            }
            return
        }
        let err = object["err"] as? [String: Any]
        let message = (err?["message"] as? String) ?? String(localized: "The host rejected the request.")
        let code = (err?["code"] as? String) ?? "unsupported"
        if let accessError = HostError.accessFailure(
            code: code, capability: (err?["capability"] as? String) ?? (object["capability"] as? String)
        ) {
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
        _ = data
    }

    private func failPending(_ error: Error) {
        let pending = self.pending
        self.pending.removeAll()
        for (_, continuation) in pending {
            continuation.resume(throwing: HostError.network(error.localizedDescription))
        }
    }

    private static func wsURL(origin: URL, path: String) -> URL {
        var components = URLComponents(url: origin, resolvingAgainstBaseURL: false) ?? URLComponents()
        components.scheme = origin.scheme?.lowercased() == "https" ? "wss" : "ws"
        components.path = path
        components.fragment = nil
        components.query = nil
        return components.url ?? origin
    }
}

struct EmptyPayload: Decodable {
    init() {}
    init(from decoder: Decoder) throws {
        _ = decoder
    }
}
