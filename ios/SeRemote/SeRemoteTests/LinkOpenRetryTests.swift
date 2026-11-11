import Foundation
import Testing
@testable import SeRemote

/// A recorder for the injected link-open operation. The retry helper is the
/// unit that decides whether the UI ever sees `.failed`; these tests pin both
/// sides of that boundary without a live host.
@MainActor
private final class LinkAttempts {
    private(set) var count = 0
    private var failuresRemaining: Int
    private let failure: any Error

    init(failures: Int, failure: any Error = URLError(.cannotConnectToHost)) {
        self.failuresRemaining = failures
        self.failure = failure
    }

    func run() throws {
        count += 1
        if failuresRemaining > 0 {
            failuresRemaining -= 1
            throw failure
        }
    }
}

@Suite("Opening the host link survives one transient failure")
struct LinkOpenRetryTests {
    @Test("One stale-path failure followed by success never surfaces a failure")
    func oneFailureThenSuccess() async throws {
        let attempts = LinkAttempts(failures: 1, failure: URLError(.cannotConnectToHost))
        try await LinkOpenRetry.run { _ in
            try attempts.run()
        }
        // The first attempt is the -1004 the device log shows; the automatic
        // retry settles it, so the caller never enters the disconnected state.
        #expect(attempts.count == 2)
    }

    @Test("Three consecutive failures are reported after exactly three attempts")
    func threeFailuresSurface() async {
        let attempts = LinkAttempts(failures: 3, failure: URLError(.cannotConnectToHost))
        await #expect(throws: (any Error).self) {
            try await LinkOpenRetry.run { _ in
                try attempts.run()
            }
        }
        #expect(attempts.count == LinkOpenRetry.maxAttempts)
    }

    @Test("An access failure is not retried")
    func accessFailureIsImmediate() async {
        let attempts = LinkAttempts(failures: 1, failure: HostError.deviceRevoked)
        await #expect(throws: HostError.self) {
            try await LinkOpenRetry.run { _ in
                try attempts.run()
            }
        }
        #expect(attempts.count == 1)
    }

    @Test("The whole retry budget stays inside a few seconds")
    func budgetIsShort() {
        #expect(LinkOpenRetry.maxAttempts == 3)
        let total = (0 ..< LinkOpenRetry.maxAttempts - 1)
            .reduce(Duration.zero) { $0 + LinkOpenRetry.backoff(after: $1) }
        #expect(total == .milliseconds(1000))
        #expect(total < .seconds(3))
    }
}
