import Foundation
import CoreGraphics
import Observation

enum DesktopConnectionState: Equatable {
    case idle, connecting, streaming, reconnecting, paused, revoked
}

/// Ordered meta acknowledgments fence frames from earlier display subscriptions.
/// In particular A→B→A must not accept the old A's in-flight sequence as the new stream.
nonisolated struct DesktopDisplayFence {
    private(set) var pending: [String] = []
    private(set) var acknowledged: String?

    mutating func request(_ display: String) { pending.append(display); acknowledged = nil }
    mutating func acknowledge(_ display: String) -> Bool {
        guard pending.first == display else { return false }
        pending.removeFirst()
        guard pending.isEmpty else { return false }
        acknowledged = display
        return true
    }
    func accepts(_ display: String) -> Bool { pending.isEmpty && acknowledged == display }
    mutating func reset() { pending.removeAll(); acknowledged = nil }
}

/// One desktop transport per workspace. It never owns or changes a terminal PTY.
@MainActor
@Observable
final class DesktopSocket {
    private(set) var state: DesktopConnectionState = .idle
    private(set) var info: DesktopInfo?
    private(set) var displays: [DesktopDisplay] = []
    private(set) var selectedDisplay: String?
    private(set) var image: CGImage?
    /// When `image` was decoded, so a capture can tell a live frame from a
    /// stream that stopped hours ago. `nil` whenever `image` is `nil`.
    private(set) var imageUpdatedAt: Date?
    /// Codec requested by the latest `start` / confirmed by `meta`.
    private(set) var activeCodec: DesktopCodec = .jpeg
    private(set) var issue: DesktopIssue?
    private(set) var controlEnabled = false
    private(set) var controlActive = false

    @ObservationIgnored private var http: HostHTTP?
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private var urlSession: URLSession?
    @ObservationIgnored private var gate: HostWebSocketOpenGate?
    @ObservationIgnored private let frameWorker = DesktopFrameWorker()
    @ObservationIgnored private var frameTask: Task<Void, Never>?
    @ObservationIgnored private var frameConfiguration: Task<Void, Never>?
    @ObservationIgnored private var frameGeneration = 0
    @ObservationIgnored private var connectionTask: Task<Void, Never>?
    @ObservationIgnored private var sendTask: Task<Void, Never>?
    @ObservationIgnored private var heartbeatTask: Task<Void, Never>?
    @ObservationIgnored private var pointerTask: Task<Void, Never>?
    @ObservationIgnored private var pointerCoalescer = DesktopPointerCoalescer()
    @ObservationIgnored private var heldKeys: [String: DesktopModifiers] = [:]
    @ObservationIgnored private var heldButtons: Set<String> = []
    @ObservationIgnored private var authenticated = false
    @ObservationIgnored private var h264Disabled = false
    @ObservationIgnored private var lastServerError: (code: String, capability: String?)?
    private var visible = false
    private var backgrounded = false
    private var generation = 0

    func configure(http: HostHTTP) { self.http = http }

    /// Whether an owner has asked this socket to stream. `close()` clears it,
    /// and only for this instance: sockets never share lifecycle state.
    var isOpen: Bool { visible }

    private func publish(image: CGImage?) {
        self.image = image
        imageUpdatedAt = image == nil ? nil : Date()
    }

    deinit { frameWorker.finish() }

    private func configureFrames(display: String?, newStream: Bool = false) {
        if frameTask == nil {
            let worker = frameWorker
            frameTask = Task { [weak self] in
                await worker.run { [weak self] update in
                    guard let self, self.frameGeneration == update.generation,
                          self.selectedDisplay == update.display, self.authenticated else { return }
                    if update.restartWithJPEG {
                        self.restartWithJPEG()
                    } else if update.needsKeyframe {
                        self.enqueue(["type": "keyframe"])
                    } else if let image = update.image {
                        self.publish(image: image)
                        self.state = .streaming
                    }
                }
            }
        }
        // Advance the publication fence synchronously, before the actor hop.
        frameGeneration += 1
        let token = frameGeneration
        let previous = frameConfiguration
        let worker = frameWorker
        frameConfiguration = Task { [weak self] in
            // Preserve every A→B→A request for ordered meta acknowledgments.
            await previous?.value
            let preview = await worker.configure(generation: token, display: display, newStream: newStream)
            guard let self, self.frameGeneration == token, display != nil else { return }
            self.publish(image: preview)
        }
    }

    func open() async {
        guard state != .revoked else { return }
        visible = true
        guard !backgrounded, connectionTask == nil else { return }
        generation += 1
        let epoch = generation
        connectionTask = Task { [weak self] in
            guard let self else { return }
            await self.run(epoch: epoch)
            if self.generation == epoch { self.connectionTask = nil }
        }
    }

    func close() {
        visible = false
        disconnect()
        if state != .revoked { state = .idle }
    }

    func revoke() {
        visible = false
        disconnect()
        state = .revoked
        publish(image: nil)
        displays = []
    }

    func handleLifecycle(isBackground: Bool) {
        backgrounded = isBackground
        if isBackground {
            disconnect()
            if state != .revoked { state = .paused }
        } else if visible, state != .revoked {
            Task { await open() }
        }
    }

    func selectDisplay(_ display: String) {
        guard displays.contains(where: { $0.id == display }), display != selectedDisplay else { return }
        setControlActive(false)
        selectedDisplay = display
        publish(image: nil)
        configureFrames(display: display)
        issue = nil
        if authenticated {
            state = .connecting
            enqueue(["type": "switch_display", "display": display])
            enqueue(["type": "keyframe"])
        }
    }

    func setControlActive(_ enabled: Bool) {
        guard enabled != controlActive else { return }
        if !enabled {
            pointerCoalescer.reset()
            pointerTask?.cancel()
            pointerTask = nil
            for button in heldButtons { enqueue(DesktopInputMessage.button(button: button, down: false).json) }
            for (code, modifiers) in heldKeys {
                enqueue(DesktopInputMessage.key(code: code, down: false, modifiers: modifiers).json)
            }
            heldButtons.removeAll()
            heldKeys.removeAll()
        }
        controlActive = enabled && controlEnabled && state == .streaming
    }

    func send(_ message: DesktopInputMessage) {
        guard controlActive, controlEnabled, state == .streaming else { return }
        switch message {
        case .pointer:
            pointerCoalescer.offer(message)
            guard pointerTask == nil else { return }
            pointerTask = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(34))
                guard !Task.isCancelled, let self else { return }
                self.flushPointer()
                self.pointerTask = nil
            }
        case .button(let button, let down):
            flushPointer()
            if down { heldButtons.insert(button) } else { heldButtons.remove(button) }
            enqueue(message.json)
        case .key(let code, let down, let modifiers):
            flushPointer()
            if down { heldKeys[code] = modifiers } else { heldKeys.removeValue(forKey: code) }
            enqueue(message.json)
        default:
            // The click/drag must never overtake its final pointer position.
            flushPointer()
            enqueue(message.json)
        }
    }

    private func flushPointer() {
        if let pointer = pointerCoalescer.take() {
            enqueue(pointer.json)
        }
    }

    private func disconnect() {
        generation += 1
        connectionTask?.cancel()
        connectionTask = nil
        invalidateTransport()
    }

    private func invalidateTransport() {
        authenticated = false
        configureFrames(display: nil, newStream: true)
        controlActive = false
        pointerCoalescer.reset()
        heldKeys.removeAll()
        heldButtons.removeAll()
        pointerTask?.cancel()
        pointerTask = nil
        sendTask?.cancel()
        sendTask = nil
        heartbeatTask?.cancel()
        heartbeatTask = nil
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
        gate = nil
        urlSession?.invalidateAndCancel()
        urlSession = nil
    }

    private func run(epoch: Int) async {
        var attempt = 0
        while visible, !backgrounded, generation == epoch, !Task.isCancelled {
            state = attempt == 0 && image == nil ? .connecting : .reconnecting
            do {
                try await connect(epoch: epoch)
                guard generation == epoch, let socket else { return }
                while !Task.isCancelled, generation == epoch {
                    let message = try await socket.receive()
                    guard generation == epoch else { return }
                    try await receive(message, epoch: epoch)
                }
                return
            } catch {
                guard generation == epoch, !Task.isCancelled else { return }
                if socket?.closeCode == .policyViolation {
                    gate?.handleClose(.policyViolation)
                    publishRevocation()
                    return
                }
                if case HostError.deviceRevoked = error { publishRevocation(); return }
                if (socket?.closeCode == .normalClosure || socket?.closeCode == .goingAway),
                   let lastServerError {
                    issue = DesktopIssue(code: lastServerError.code, capability: lastServerError.capability)
                    invalidateTransport()
                    state = .idle
                    return
                }
                if case HostError.rePairingRequired = error {
                    issue = DesktopIssue(code: "UNAUTHORIZED")
                    invalidateTransport()
                    state = .idle
                    return
                }
                if case HostError.forbidden(let capability) = error {
                    issue = DesktopIssue(code: "FORBIDDEN", capability: capability)
                    invalidateTransport()
                    state = .idle
                    return
                }
                if case HostError.ipc(_, let code) = error {
                    issue = DesktopIssue(code: code)
                    invalidateTransport()
                    state = .idle
                    return
                }
                invalidateTransport()
                state = .reconnecting
                attempt += 1
                try? await Task.sleep(for: .seconds(min(8, 1 << min(attempt - 1, 3))))
            }
        }
    }

    private func connect(epoch: Int) async throws {
        lastServerError = nil
        guard let http else { throw HostError.unexpected("Desktop transport is not configured") }
        let metadata = try await http.desktopInfo()
        guard generation == epoch, !Task.isCancelled else { throw CancellationError() }
        // A JPEG fallback is sticky for this socket's lifetime: reconnect must
        // not re-request h264 just because the old cached info still lists it.
        // Only a fresh info that newly advertises h264 clears the flag.
        let advertisedBefore = info?.supportedCodecs.contains(DesktopCodec.h264.rawValue) ?? false
        let advertisesNow = metadata.supportedCodecs.contains(DesktopCodec.h264.rawValue)
        info = metadata
        if h264Disabled && advertisesNow && !advertisedBefore {
            h264Disabled = false
        }
        displays = metadata.displays
        controlEnabled = metadata.controlEnabled
        if !metadata.supported { throw HostError.ipc("", "SCREEN_UNSUPPORTED") }
        if !metadata.viewEnabled { throw HostError.ipc("", "DESKTOP_DISABLED") }
        if !metadata.permissions.screenRecording { throw HostError.ipc("", "SCREEN_PERMISSION_REQUIRED") }
        if !displays.contains(where: { $0.id == selectedDisplay }) {
            selectedDisplay = displays.first(where: \.isPrimary)?.id ?? displays.first?.id
        }
        guard let display = selectedDisplay else { throw HostError.ipc("", "DISPLAY_NOT_FOUND") }
        var components = URLComponents(url: http.origin, resolvingAgainstBaseURL: false)
        components?.scheme = http.origin.scheme == "https" ? "wss" : "ws"
        components?.path = "/remote/desktop/stream"
        components?.query = nil
        components?.fragment = nil
        guard let url = components?.url else { throw HostError.unexpected("Invalid desktop WebSocket URL") }
        var request = URLRequest(url: url, timeoutInterval: HostTunnelSession.handshakeSeconds)
        request.assumesHTTP3Capable = false
        request.setValue(http.credentials.originString, forHTTPHeaderField: "Origin")
        let gate = HostWebSocketOpenGate { [weak self] in
            Task { @MainActor in
                guard let self, self.generation == epoch else { return }
                self.publishRevocation()
            }
        }
        let session = HostTunnelSession.make(delegate: gate)
        let socket = session.webSocketTask(with: request)
        socket.maximumMessageSize = 64 * 1024 * 1024
        self.gate = gate
        self.urlSession = session
        self.socket = socket
        socket.resume()
        try await gate.waitForOpen()
        guard generation == epoch else { throw CancellationError() }
        let id = UUID().uuidString
        try await write(["id": id, "type": "authenticate", "payload": ["token": http.credentials.bearer ?? ""]], to: socket)
        let response = try await Self.authenticationResponse(from: socket)
        guard generation == epoch else { throw CancellationError() }
        guard case .string(let text) = response,
              let data = text.data(using: .utf8),
              let object = try? WireJSON.object(from: data) else {
            throw HostError.network(String(localized: "Desktop connection interrupted."))
        }
        if let code = object["code"] as? String {
            try throwFailure(code: code, capability: object["capability"] as? String)
        }
        guard object["id"] as? String == id, object["success"] as? Bool == true else {
            throw HostError.network(String(localized: "Desktop connection interrupted."))
        }
        let activeDisplay = selectedDisplay ?? display
        configureFrames(display: activeDisplay, newStream: true)
        await frameConfiguration?.value
        guard generation == epoch else { throw CancellationError() }
        authenticated = true
        // Startup joins the same ordered queue as subsequent display switches and inputs.
        // h264Disabled stays sticky across reconnects (see connect()).
        startStream(display: activeDisplay)
        startHeartbeat(socket: socket, epoch: epoch)
        issue = nil
    }

    /// Sends `start` plus the first keyframe request. H.264 fields are only
    /// present when the host advertises the codec and this device can decode it;
    /// otherwise the payload is byte-for-byte the legacy JPEG request.
    private func startStream(display: String) {
        let codecs = info?.supportedCodecs ?? ["jpeg"]
        let h264 = !h264Disabled && codecs.contains(DesktopCodec.h264.rawValue)
            && DesktopH264Decoder.isHardwareDecodeSupported
        activeCodec = h264 ? .h264 : .jpeg
        enqueue(DesktopStartRequest.message(display: display, codecs: codecs, h264Available: h264))
        enqueue(["type": "keyframe"])
    }

    /// Decode failed three times in a row: silently resume the same display with
    /// the legacy JPEG stream instead of surfacing an error.
    private func restartWithJPEG() {
        guard authenticated, state != .revoked, let display = selectedDisplay else { return }
        h264Disabled = true
        startStream(display: display)
    }

    private func startHeartbeat(socket: URLSessionWebSocketTask, epoch: Int) {
        heartbeatTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    try await Task.sleep(for: .seconds(20))
                    guard let self, self.generation == epoch else { return }
                    try await withThrowingTaskGroup(of: Void.self) { group in
                        group.addTask {
                            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                                socket.sendPing { error in
                                    if let error { continuation.resume(throwing: error) }
                                    else { continuation.resume() }
                                }
                            }
                        }
                        group.addTask {
                            try await Task.sleep(for: .seconds(10))
                            socket.cancel(with: .goingAway, reason: nil)
                            throw URLError(.timedOut)
                        }
                        defer { group.cancelAll() }
                        try await group.next()
                    }
                } catch {
                    if !Task.isCancelled { socket.cancel(with: .goingAway, reason: nil) }
                    return
                }
            }
        }
    }

    private static func authenticationResponse(from socket: URLSessionWebSocketTask) async throws -> URLSessionWebSocketTask.Message {
        try await withThrowingTaskGroup(of: URLSessionWebSocketTask.Message.self) { group in
            group.addTask { try await socket.receive() }
            group.addTask {
                try await Task.sleep(for: .seconds(10))
                socket.cancel(with: .goingAway, reason: nil)
                throw HostError.network(String(localized: "Desktop connection interrupted."))
            }
            defer { group.cancelAll() }
            return try await group.next()!
        }
    }

    private func receive(_ message: URLSessionWebSocketTask.Message, epoch: Int) async throws {
        await frameConfiguration?.value
        guard generation == epoch, authenticated else { return }
        switch message {
        case .data(let data):
            frameWorker.submit(data, generation: frameGeneration)
        case .string(let text):
            guard let data = text.data(using: .utf8), let object = try? WireJSON.object(from: data) else { return }
            switch object["type"] as? String {
            case "meta":
                if let list = object["displays"], let bytes = try? JSONSerialization.data(withJSONObject: list),
                   let parsed = try? JSONDecoder().decode([DesktopDisplay].self, from: bytes) { displays = parsed }
                // Old hosts omit codec; their stream is always JPEG.
                let metaCodec = DesktopCodec(wire: object["codec"] as? String)
                if metaCodec != activeCodec {
                    activeCodec = metaCodec
                    if metaCodec == .jpeg { h264Disabled = true }
                }
                if let display = object["display"] as? String {
                    await frameWorker.acknowledge(display, generation: frameGeneration)
                }
            case "control_state":
                controlEnabled = object["enabled"] as? Bool ?? false
                if !controlEnabled { setControlActive(false) }
            case "error":
                let code = object["code"] as? String ?? "CAPTURE_FAILED"
                let capability = object["capability"] as? String
                lastServerError = (code, capability)
                if code == "DEVICE_REVOKED" { publishRevocation(); return }
                if let failure = HostError.accessFailure(code: code, capability: capability), let http {
                    failure.publishAccessFailure(origin: http.origin)
                }
                issue = DesktopIssue(code: code, capability: capability)
                setControlActive(false)
                if !["CONTROL_DISABLED", "INPUT_PERMISSION_REQUIRED"].contains(code), capability != "screen.control" {
                    state = .idle
                }
            default: break
            }
        @unknown default: break
        }
    }

    private func throwFailure(code: String, capability: String?) throws {
        if let failure = HostError.accessFailure(code: code, capability: capability) {
            if let http { failure.publishAccessFailure(origin: http.origin) }
            throw failure
        }
        throw HostError.ipc("", code)
    }

    private func publishRevocation() {
        let origin = http?.origin
        revoke()
        if let origin { HostError.deviceRevoked.publishAccessFailure(origin: origin) }
    }

    private func enqueue(_ object: [String: Any]) {
        guard authenticated, let socket else { return }
        // Serialize writes so pointer -> button and modifier down -> key -> up retain their order.
        guard let data = try? WireJSON.data(from: object), let text = String(data: data, encoding: .utf8) else { return }
        let previous = sendTask
        let epoch = generation
        sendTask = Task { [weak self] in
            await previous?.value
            guard !Task.isCancelled, let self, self.generation == epoch else { return }
            do { try await socket.send(.string(text)) }
            catch { socket.cancel(with: .goingAway, reason: nil) }
        }
    }

    private func write(_ object: [String: Any], to socket: URLSessionWebSocketTask) async throws {
        let data = try WireJSON.data(from: object)
        guard let text = String(data: data, encoding: .utf8) else { return }
        try await socket.send(.string(text))
    }
}
