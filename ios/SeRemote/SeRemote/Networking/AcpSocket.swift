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
    var turnTimeoutMs: Double?
    var promptInactivityTimeoutMs: Double?
    var permissionReconnectGraceMs: Double?
    var pingIntervalMs: Double?
    var pongTimeoutMs: Double?
}

@MainActor
@Observable
final class AcpSocket {
    enum State: Equatable {
        case idle
        case connecting
        case connected
        case failed(String)
    }

    private(set) var state: State = .idle
    private(set) var historyMode = "live_only"

    var onEvent: (@MainActor (String, String?, UInt64, Data) -> Void)?

    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var heartbeat: Task<Void, Never>?
    private var pending: [String: CheckedContinuation<Data, Error>] = [:]
    private var requestSerial = 0
    private var origin: URL?
    private var openGate: HostWebSocketOpenGate?

    func connect(origin: URL, credentials: HostCredentials) async throws {
        stop()
        self.origin = origin
        state = .connecting
        guard let token = credentials.bearer, !token.isEmpty else {
            throw HostError.unexpected(String(localized: "This access link is missing its token. Scan the QR again."))
        }
        let wsURL = Self.wsURL(origin: origin, path: "/ws")
        var urlRequest = URLRequest(url: wsURL, timeoutInterval: HostTunnelSession.handshakeSeconds)
        urlRequest.assumesHTTP3Capable = false
        credentials.apply(to: &urlRequest)
        let gate = HostWebSocketOpenGate()
        openGate = gate
        let session = HostTunnelSession.make(delegate: gate)
        self.session = session
        let task = session.webSocketTask(with: urlRequest)
        self.task = task
        task.resume()
        receiveTask = Task { await self.receiveLoop() }

        do {
            try await gate.waitForOpen()
            let data = try await request("authenticate", payload: ["token": token])
            if let reply = try? JSONDecoder().decode(AcpAuthenticateReply.self, from: data) {
                historyMode = reply.historyMode ?? "live_only"
            }
            state = .connected
            startHeartbeat()
        } catch {
            state = .failed(error.localizedDescription)
            stop()
            throw error
        }
    }

    func request(_ type: String, payload: [String: Any] = [:], timeoutSeconds: Double? = 45) async throws -> Data {
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

    func stop() {
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
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
        if case .failed = state {
            /* keep */
        } else {
            state = .idle
        }
    }

    private func startHeartbeat() {
        heartbeat?.cancel()
        heartbeat = Task { [weak self] in
            while let self, !Task.isCancelled {
                try? await Task.sleep(for: .seconds(30))
                guard !Task.isCancelled else { return }
                _ = try? await self.request("ping")
            }
        }
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
        continuation.resume(throwing: HostError.ipc(message, code))
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
