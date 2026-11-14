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

/// Input that was handed to the socket but never acknowledged. The host may
/// already have applied it (its log shows `write` succeeding while the reply
/// was still in transit), so it is never resent automatically: a duplicated
/// Enter runs a command twice.
nonisolated struct TerminalWriteUnconfirmed: LocalizedError {
    var errorDescription: String? {
        String(localized: "Send could not be confirmed. Check the terminal before sending again.")
    }
}

/// The frame never reached the transport, so the host cannot have applied it.
nonisolated struct TerminalFrameNotSent: Error {
    let message: String
}

/// What to do with a request whose reply has not arrived yet.
nonisolated enum TerminalRequestTimeoutVerdict: Equatable, Sendable {
    /// Frames are still arriving: the reply is queued behind them.
    case keepWaiting(Duration)
    /// The socket is delivering but this request was never answered.
    case failKeepingSocket
    /// Nothing arrived for a whole timeout: the socket is presumed dead.
    case failAndResetSocket
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
    /// Monotonic timestamp of the most recent authenticated handshake or
    /// inbound WebSocket frame on the current connection. Real inbound traffic
    /// proves the TCP/WebSocket stream is alive without sending a control Ping
    /// (which some tunnels buffer or drop right after handshake).
    private(set) var lastInboundAt: ContinuousClock.Instant?
    private var origin: URL?

    /// `gap` is true on the first replay frame when the host could not replay
    /// every byte since the client's cursor: the stream starts mid-sequence and
    /// the canvas must reset before painting it.
    var onBytes: (@MainActor (String, Data, Bool) -> Void)?
    /// Same frames as `onBytes`, plus the host's sequence cursor. Recovery
    /// re-authorization needs the true cursor to resume replay without a gap
    /// or a full-scrollback refetch.
    var onOutputFrame: (@MainActor (OutputFrame) -> Void)?
    /// Fired once per authenticated connection, right after
    /// `connectionGeneration` is bumped. Authorization is per-connection, so
    /// this is the store's cue to re-watch/re-attach everything it holds.
    var onConnected: (@MainActor (Int) -> Void)?
    /// Fired when a live connection drops or fails its ping/request watchdog so
    /// higher layers can proactively reconnect without waiting for user input.
    var onDisconnected: (@MainActor () -> Void)?
    var onExit: (@MainActor (String) -> Void)?
    var onCatalogChanged: (@MainActor () -> Void)?
    var onDisplayModeChanged: (@MainActor (String, String, Int, Int) -> Void)?

    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var heartbeatTask: Task<Void, Never>?
    private var pending: [String: CheckedContinuation<Data, Error>] = [:]
    private var timeoutTasks: [String: Task<Void, Never>] = [:]
    private var requestSerial = 0
    private var openGate: HostWebSocketOpenGate?
    /// Single-flight handshake task so concurrent `ensureConnected()` callers
    /// await the same connection attempt instead of calling `stop()` on each
    /// other and throwing spurious "Disconnected from the host." errors.
    private var connectTask: Task<Void, Error>?
    private var connectSerial = 0
    /// Cumulative inbound counters, reported when a request times out so the
    /// device log shows whether the link was silent or busy during the wait.
    @ObservationIgnored private var inboundFrameCount = 0
    @ObservationIgnored private var inboundByteCount = 0

    /// True while a handshake is in flight. Stopping the socket now would fail
    /// every caller awaiting it with "Disconnected from the host."
    var isConnecting: Bool { connectTask != nil }

    /// A reply is only declared lost after `timeout` of inbound silence. While
    /// frames keep arriving the reply is queued behind them (replies and PTY
    /// output share one ordered stream), so the wait continues up to four
    /// timeouts in total before the request alone is failed.
    nonisolated static func timeoutVerdict(
        waited: Duration,
        silentFor: Duration,
        timeout: Duration
    ) -> TerminalRequestTimeoutVerdict {
        if silentFor >= timeout { return .failAndResetSocket }
        let hardCap = timeout * 4
        if waited >= hardCap { return .failKeepingSocket }
        return .keepWaiting(min(timeout - silentFor, hardCap - waited))
    }

    func connect(origin: URL, credentials: HostCredentials) async throws {
        guard !isRevoked else { throw HostError.deviceRevoked }
        if isConnected, self.origin == origin { return }
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
        guard !isRevoked else { throw HostError.deviceRevoked }
        stopSocketOnly()
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
        let wsTask = session.webSocketTask(with: urlRequest)
        // The default 1 MiB limit fails the whole socket with "Message too
        // long": PTY bytes travel as a JSON number array (up to 4 characters
        // per byte), so a 256 KiB replay alone can exceed it.
        wsTask.maximumMessageSize = HostTunnelSession.maximumMessageBytes
        self.task = wsTask
        wsTask.resume()
        receiveTask = Task { await self.receiveLoop(for: wsTask) }
        do {
            try await gate.waitForOpen()
            _ = try await self.request("authenticate", payload: ["token": token], as: EmptyPayload.self)
            guard !isRevoked else { throw HostError.deviceRevoked }
            isConnected = true
            lastInboundAt = ContinuousClock.now
            connectionGeneration &+= 1
            startHeartbeat()
            HostLog.session.info("terminal WebSocket authenticated")
            // Authorization is per-connection: everything watched/attached on
            // the previous generation must re-authorize on this one.
            onConnected?(connectionGeneration)
        } catch {
            if case HostError.deviceRevoked = error {
                handleDeviceRevoked(origin: origin)
            }
            HostLog.session.error("terminal WebSocket connect failed: \(error.localizedDescription, privacy: .public)")
            stopSocketOnly()
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

    /// Throws `HostError.network` only when the input never left this device
    /// (no live socket, or the transport refused the frame), which is safe to
    /// resend after a reconnect. Any failure after the frame was handed to
    /// the transport is `TerminalWriteUnconfirmed`: the host may have applied it.
    func write(terminalId: String, data: String) async throws {
        guard task != nil, isConnected else {
            throw HostError.network(String(localized: "Disconnected from the host."))
        }
        do {
            _ = try await request(
                "write",
                payload: ["terminalId": terminalId, "data": data],
                as: EmptyPayload.self,
                timeoutSeconds: 10,
                unsentAsNetwork: false
            )
        } catch let unsent as TerminalFrameNotSent {
            throw HostError.network(unsent.message)
        } catch HostError.network {
            HostLog.session.error("Terminal write unconfirmed; not resending")
            throw TerminalWriteUnconfirmed()
        }
    }

    func resize(terminalId: String, cols: Int, rows: Int) async throws {
        _ = try await request(
            "resize",
            payload: ["terminalId": terminalId, "cols": cols, "rows": rows],
            as: EmptyPayload.self,
            timeoutSeconds: 10
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
        return try await request(
            "set_display_mode",
            payload: payload,
            as: TerminalDisplayModeState.self,
            timeoutSeconds: 10
        )
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
    /// True when the socket is connected and received an authenticated reply or
    /// inbound frame within `window` — proving liveness without a control Ping.
    func isRecentlyActive(within window: Duration = .seconds(10)) -> Bool {
        guard isConnected, task != nil, let lastInboundAt else { return false }
        return lastInboundAt.duration(to: ContinuousClock.now) < window
    }

    func probeLiveness() async -> TransportLiveness {
        guard isConnected, let task else { return .dead }
        if isRecentlyActive(within: .seconds(6)) {
            return .alive
        }
        let result = await WebSocketLiveness.probe(task)
        if result == .alive {
            lastInboundAt = ContinuousClock.now
        }
        return result
    }

    func stop() {
        connectSerial &+= 1
        connectTask?.cancel()
        connectTask = nil
        stopSocketOnly()
    }

    private func stopSocketOnly() {
        heartbeatTask?.cancel()
        heartbeatTask = nil
        receiveTask?.cancel()
        receiveTask = nil
        for (_, timer) in timeoutTasks {
            timer.cancel()
        }
        timeoutTasks.removeAll()
        openGate?.failOpen(HostError.network(String(localized: "Disconnected from the host.")))
        openGate = nil
        failPending(HostError.network(String(localized: "Disconnected from the host.")))
        let oldTask = task
        task = nil
        isConnected = false
        lastInboundAt = nil
        oldTask?.cancel(with: .goingAway, reason: nil)
        session?.invalidateAndCancel()
        session = nil
    }

    /// Periodic WebSocket ping watchdog so a half-open TCP/tunnel link is torn
    /// down before user typing or scrolling hangs waiting for a reply.
    private func startHeartbeat() {
        heartbeatTask?.cancel()
        heartbeatTask = Task { @MainActor [weak self] in
            var consecutiveUnknowns = 0
            while let self, !Task.isCancelled {
                try? await Task.sleep(for: .seconds(25))
                guard !Task.isCancelled, self.isConnected, let currentTask = self.task else { return }
                if self.isRecentlyActive(within: .seconds(20)) {
                    consecutiveUnknowns = 0
                    continue
                }
                let liveness = await WebSocketLiveness.probe(currentTask, timeout: .seconds(5))
                guard !Task.isCancelled, self.task === currentTask, self.isConnected else { return }
                switch liveness {
                case .alive:
                    self.lastInboundAt = ContinuousClock.now
                    consecutiveUnknowns = 0
                case .unknown:
                    if self.isRecentlyActive(within: .seconds(20)) {
                        consecutiveUnknowns = 0
                        continue
                    }
                    consecutiveUnknowns += 1
                    if consecutiveUnknowns >= 3 {
                        HostLog.session.error("Terminal WebSocket ping timed out 3x with no traffic; resetting stale socket")
                        self.stopSocketOnly()
                        self.onDisconnected?()
                        return
                    }
                case .dead:
                    HostLog.session.error("Terminal WebSocket ping failed; tearing down dead socket")
                    self.stopSocketOnly()
                    self.onDisconnected?()
                    return
                }
            }
        }
    }

    private func request<T: Decodable>(
        _ type: String,
        payload: [String: Any],
        as _: T.Type,
        timeoutSeconds: Double = 20,
        unsentAsNetwork: Bool = true
    ) async throws -> T {
        guard !isRevoked else { throw HostError.deviceRevoked }
        requestSerial += 1
        let id = "term-\(requestSerial)"
        let body: [String: Any] = ["id": id, "type": type, "payload": payload]
        let data = try WireJSON.data(from: body)
        let reply: Data = try await withCheckedThrowingContinuation { continuation in
            guard let currentTask = task else {
                continuation.resume(
                    throwing: HostError.network(String(localized: "Disconnected from the host."))
                )
                return
            }
            pending[id] = continuation
            let timeout = Duration.seconds(timeoutSeconds)
            let started = ContinuousClock.now
            let framesAtStart = inboundFrameCount
            let bytesAtStart = inboundByteCount
            timeoutTasks[id] = Task { @MainActor [weak self] in
                var wait = timeout
                while true {
                    try? await Task.sleep(for: wait)
                    guard let self, !Task.isCancelled, self.pending[id] != nil else { return }
                    let now = ContinuousClock.now
                    let lastInbound = self.task === currentTask ? self.lastInboundAt : nil
                    let quietSince = max(lastInbound ?? started, started)
                    let waited = started.duration(to: now)
                    let silentFor = quietSince.duration(to: now)
                    let verdict = Self.timeoutVerdict(waited: waited, silentFor: silentFor, timeout: timeout)
                    if case .keepWaiting(let more) = verdict {
                        wait = more
                        continue
                    }
                    guard let waiting = self.pending.removeValue(forKey: id) else { return }
                    self.timeoutTasks.removeValue(forKey: id)
                    // A missing reply alone does not prove the socket is dead:
                    // replies share one ordered stream with PTY output. Only
                    // a full timeout of inbound silence resets the socket.
                    let reset = verdict == .failAndResetSocket
                    let frames = self.inboundFrameCount &- framesAtStart
                    let bytes = self.inboundByteCount &- bytesAtStart
                    HostLog.session.error("Terminal request timed out type=\(type, privacy: .public) waitedMs=\(Int(waited / .milliseconds(1)), privacy: .public) silentMs=\(Int(silentFor / .milliseconds(1)), privacy: .public) inboundFrames=\(frames, privacy: .public) inboundBytes=\(bytes, privacy: .public) resetSocket=\(reset, privacy: .public)")
                    waiting.resume(throwing: HostError.network(String(localized: "The terminal host did not respond. Try again.")))
                    if reset, self.task === currentTask {
                        let wasConnected = self.isConnected
                        self.stopSocketOnly()
                        if wasConnected {
                            self.onDisconnected?()
                        }
                    }
                    return
                }
            }
            currentTask.send(.string(String(data: data, encoding: .utf8) ?? "")) { [weak self] error in
                Task { @MainActor in
                    if let error {
                        guard let self, self.task === currentTask else { return }
                        self.timeoutTasks.removeValue(forKey: id)?.cancel()
                        let wasConnected = self.isConnected
                        let unsent: Error = unsentAsNetwork
                            ? HostError.network(error.localizedDescription)
                            : TerminalFrameNotSent(message: error.localizedDescription)
                        self.pending.removeValue(forKey: id)?.resume(throwing: unsent)
                        self.stopSocketOnly()
                        if wasConnected {
                            self.onDisconnected?()
                        }
                    }
                }
            }
        }
        if T.self == EmptyPayload.self {
            return EmptyPayload() as! T
        }
        return try JSONDecoder().decode(T.self, from: reply)
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
                inboundFrameCount &+= 1
                inboundByteCount &+= data.count
                handleFrame(data)
            } catch {
                guard self.task === loopTask, !isRevoked else { return }
                if loopTask.closeCode == .policyViolation, let origin {
                    handleDeviceRevoked(origin: origin)
                } else {
                    let wasConnected = isConnected
                    stopSocketOnly()
                    if wasConnected {
                        onDisconnected?()
                    }
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
        if let frame = Self.outputFrame(from: object) {
            onOutputFrame?(frame)
            onBytes?(frame.terminalId, frame.data, frame.gap)
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
    ///
    /// A replay whose host cursor fell behind the oldest retained chunk carries
    /// `gap: true` on its first frame. Concatenating those chunks without that
    /// flag would replay a stream that starts mid-escape at an unknown screen
    /// state — the canvas has to reset first.
    nonisolated static func output(from object: [String: Any]) -> (terminalId: String, data: Data, gap: Bool)? {
        switch object["type"] as? String {
        case "data":
            guard let terminalId = object["terminalId"] as? String else { return nil }
            return (terminalId, bytes(from: object["data"]), false)
        case "replay":
            guard let terminalId = object["terminalId"] as? String,
                  let chunks = object["chunks"] as? [[String: Any]] else { return nil }
            var data = Data()
            for chunk in chunks {
                data.append(bytes(from: chunk["data"]))
            }
            let gap = object["gap"] as? Bool ?? false
            return (terminalId, data, gap)
        default:
            return nil
        }
    }

    /// One terminal-output frame with the host's authoritative sequence cursor.
    /// `latestSeq` is what recovery replays resume from: `data` frames carry it
    /// flat as `seq`, `replay` frames carry `latestSeq` (falling back to the
    /// last chunk's own `seq`).
    struct OutputFrame: Equatable, Sendable {
        let terminalId: String
        let data: Data
        let gap: Bool
        let latestSeq: Int?
    }

    /// `output(from:)` plus the sequence cursor, for callers that re-authorize
    /// after a reconnect and must resume replay at the host's true cursor.
    nonisolated static func outputFrame(from object: [String: Any]) -> OutputFrame? {
        guard let output = output(from: object) else { return nil }
        let latestSeq: Int?
        switch object["type"] as? String {
        case "data":
            latestSeq = intValue(object["seq"])
        case "replay":
            if let chunks = object["chunks"] as? [[String: Any]] {
                latestSeq = intValue(object["latestSeq"])
                    ?? chunks.compactMap { intValue($0["seq"]) }.last
            } else {
                latestSeq = intValue(object["latestSeq"])
            }
        default:
            latestSeq = nil
        }
        return OutputFrame(
            terminalId: output.terminalId,
            data: output.data,
            gap: output.gap,
            latestSeq: latestSeq
        )
    }

    /// Event payloads come through JSONSerialization, so numbers arrive as
    /// NSNumber; bridge both spellings.
    nonisolated private static func intValue(_ any: Any?) -> Int? {
        if let number = any as? NSNumber {
            return number.intValue
        }
        return any as? Int
    }

    private func failPending(_ error: Error) {
        for (_, timer) in timeoutTasks {
            timer.cancel()
        }
        timeoutTasks.removeAll()
        let pending = self.pending
        self.pending.removeAll()
        for (_, continuation) in pending {
            continuation.resume(throwing: error)
        }
    }

    private func handleReply(id: String, object: [String: Any]) {
        timeoutTasks.removeValue(forKey: id)?.cancel()
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
