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
}

enum HostTunnelSession {
    nonisolated static let handshakeSeconds: TimeInterval = 60

    static func make(delegate: URLSessionDelegate) -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        // Health already proved the path. Waiting again lets a Cloudflare
        // WebSocket sit idle until URLSession reports -1001.
        configuration.waitsForConnectivity = false
        configuration.httpShouldUsePipelining = false
        configuration.timeoutIntervalForRequest = handshakeSeconds
        configuration.timeoutIntervalForResource = handshakeSeconds + 30
        return URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
    }
}

final class HostWebSocketOpenGate: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var didOpen = false
    private var opened: CheckedContinuation<Void, Error>?

    func waitForOpen(seconds: Double = 60) async throws {
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask {
                try await withCheckedThrowingContinuation { continuation in
                    self.lock.lock()
                    if self.didOpen {
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
        let continuation = opened
        opened = nil
        lock.unlock()
        continuation?.resume(throwing: error)
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
        failOpen(HostError.network(String(localized: "Disconnected from the host.")))
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        HostLog.session.error("WebSocket task failed: \(error.localizedDescription, privacy: .public)")
        failOpen(HostError.network(error.localizedDescription))
    }
}
