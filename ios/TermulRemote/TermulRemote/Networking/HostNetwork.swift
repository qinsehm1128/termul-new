import Foundation
import Network

nonisolated enum HostNetwork {
    /// After an iPhone reboot, Wi-Fi can be associated while DNS/Cloudflare
    /// still fail. Wait for a usable path before the first health probe.
    nonisolated static func waitUntilReady(timeoutSeconds: Double = 12) async {
        let gate = PathGate()
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { path in
            if path.status == .satisfied {
                gate.open()
            }
        }
        // Same bundle-derived stem as the log subsystem: this label shows up in
        // crash reports and the debugger, so it should name the app that shipped.
        monitor.start(queue: DispatchQueue(label: "\(HostLog.subsystem).path"))
        defer { monitor.cancel() }
        await gate.wait(timeoutSeconds: timeoutSeconds)
    }
}

nonisolated private final class PathGate: @unchecked Sendable {
    private let lock = NSLock()
    private var opened = false
    private var continuation: CheckedContinuation<Void, Never>?

    func open() {
        lock.lock()
        opened = true
        let waiting = continuation
        continuation = nil
        lock.unlock()
        waiting?.resume()
    }

    func wait(timeoutSeconds: Double) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.lock()
            if opened {
                lock.unlock()
                continuation.resume()
                return
            }
            self.continuation = continuation
            lock.unlock()
            DispatchQueue.global().asyncAfter(deadline: .now() + timeoutSeconds) { [weak self] in
                self?.open()
            }
        }
    }
}

enum HostURLSession {
    static let shared: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        // PathGate already waited for a usable route. Waiting again lets a
        // cold-launch health probe sit on the spinner until the process dies.
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = 20
        configuration.timeoutIntervalForResource = 45
        configuration.httpShouldUsePipelining = false
        return URLSession(configuration: configuration)
    }()
}
