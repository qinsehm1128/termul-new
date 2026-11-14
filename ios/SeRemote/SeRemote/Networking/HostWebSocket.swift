import Foundation
import os

enum HostLog {
    /// `os_log` subsystem, and the stem of every queue label this app names.
    ///
    /// Apple's convention is that the subsystem *is* the bundle identifier, and
    /// Console.app's subsystem filter is what makes that convention load-bearing:
    /// someone reading device logs types the bundle id. So it is read from the
    /// bundle rather than written down a second time. The two spellings then
    /// cannot disagree, and the pending bundle-identifier rename carries this
    /// along without anyone having to remember that it also lives here — which
    /// is exactly how the pre-rename value survived in this line to begin with.
    ///
    /// `bundleIdentifier` is nil only outside an app bundle, where the process
    /// name is the more useful label anyway.
    nonisolated static let subsystem = Bundle.main.bundleIdentifier
        ?? ProcessInfo.processInfo.processName

    nonisolated static let session = Logger(subsystem: subsystem, category: "session")
    nonisolated static let ui = Logger(subsystem: subsystem, category: "ui")
}

enum HostTunnelSession {
    nonisolated static let handshakeSeconds: TimeInterval = 60
    /// Above the host's 8 MiB outbound frame budget. URLSession's default is
    /// 1 MiB, and an oversized frame fails the socket, not just the message.
    nonisolated static let maximumMessageBytes = 16 * 1024 * 1024

    static func make(delegate: URLSessionDelegate) -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        // Health already proved the path. Waiting again lets a Cloudflare
        // WebSocket sit idle until URLSession reports -1001.
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = handshakeSeconds
        configuration.timeoutIntervalForResource = handshakeSeconds + 30
        return URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
    }
}

/// What a protocol-level WebSocket ping says about a socket the app still
/// believes is connected.
///
/// A socket that outlived its network path usually reports an immediate error
/// (`-1004` / "on unconnected nw_connection"). A timeout is not proof of death:
/// cellular commonly needs a few seconds to wake, so it stays `.unknown` and
/// the caller keeps the mounted session instead of replaying a terminal.
nonisolated enum TransportLiveness: Equatable, Sendable {
    case alive
    case dead
    case unknown

    var isDead: Bool { self == .dead }
}

nonisolated enum WebSocketLiveness {
    static func probe(
        _ task: URLSessionWebSocketTask,
        timeout: Duration = .seconds(4)
    ) async -> TransportLiveness {
        await withCheckedContinuation { continuation in
            let gate = PingGate(continuation)
            task.sendPing { error in
                gate.finish(error == nil ? .alive : .dead)
            }
            Task {
                try? await Task.sleep(for: timeout)
                gate.finish(.unknown)
            }
        }
    }
}

nonisolated private final class PingGate: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<TransportLiveness, Never>?

    init(_ continuation: CheckedContinuation<TransportLiveness, Never>) {
        self.continuation = continuation
    }

    func finish(_ result: TransportLiveness) {
        lock.lock()
        let waiting = continuation
        continuation = nil
        lock.unlock()
        waiting?.resume(returning: result)
    }
}

final class HostWebSocketOpenGate: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var didOpen = false
    private var openFailure: Error?
    private var revoked = false
    private var opened: CheckedContinuation<Void, Error>?
    private let onRevoked: @Sendable () -> Void

    init(onRevoked: @escaping @Sendable () -> Void = {}) {
        self.onRevoked = onRevoked
        super.init()
    }

    /// Used by every socket, including closures received after authentication.
    /// A policy close is terminal, never a transient tunnel failure.
    @discardableResult
    func handleClose(_ code: URLSessionWebSocketTask.CloseCode) -> HostError {
        guard code == .policyViolation else {
            let error = HostError.network(String(localized: "Disconnected from the host."))
            failOpen(error)
            return error
        }
        lock.lock()
        let shouldPublish = !revoked
        revoked = true
        lock.unlock()
        let error = HostError.deviceRevoked
        failOpen(error)
        if shouldPublish { onRevoked() }
        return error
    }

    func waitForOpen(seconds: Double = 60) async throws {
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask {
                try await withCheckedThrowingContinuation { continuation in
                    self.lock.lock()
                    if let failure = self.openFailure {
                        self.lock.unlock()
                        continuation.resume(throwing: failure)
                    } else if self.didOpen {
                        self.lock.unlock()
                        continuation.resume()
                    } else {
                        self.opened = continuation
                        self.lock.unlock()
                    }
                }
            }
            group.addTask {
                try await Task.sleep(for: .seconds(seconds))
                let error = HostError.network(String(localized: "The tunnel timed out. Check that remote access is still on."))
                self.failOpen(error)
                throw error
            }
            try await group.next()!
            group.cancelAll()
        }
    }

    func failOpen(_ error: Error) {
        lock.lock()
        let failure: Error = revoked ? HostError.deviceRevoked : error
        if openFailure == nil || revoked { openFailure = failure }
        let continuation = opened
        opened = nil
        lock.unlock()
        continuation?.resume(throwing: failure)
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocolName: String?
    ) {
        lock.lock()
        didOpen = true
        let continuation = opened
        opened = nil
        lock.unlock()
        continuation?.resume()
        HostLog.session.info("WebSocket handshake opened")
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        HostLog.session.info("WebSocket closed code=\(closeCode.rawValue, privacy: .public)")
        handleClose(closeCode)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let socket = task as? URLSessionWebSocketTask, socket.closeCode == .policyViolation {
            handleClose(.policyViolation)
            return
        }
        guard let error else { return }
        HostLog.session.error("WebSocket task failed: \(error.localizedDescription, privacy: .public)")
        failOpen(HostError.network(error.localizedDescription))
    }
}
