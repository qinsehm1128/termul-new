import Foundation
import Testing
@testable import SeRemote

@Suite("Shell connection lifetime")
struct ShellConnectionLifecycleTests {
    private static func link(_ origin: String, token: String = "tok") throws -> RemoteLink {
        try RemoteLink.parse("\(origin)/#access_token=\(token)")
    }

    @Test("An animated disappearance cannot stop the active connection")
    func transitionKeepsActiveSession() throws {
        let active = try Self.link("http://192.168.6.105:53231")
        #expect(!ShellConnectionLifecycle.shouldStop(
            disappearingSession: active.sessionIdentity,
            activeSession: active.sessionIdentity
        ))
    }

    @Test("Explicit disconnect tears down the departing session")
    func disconnectStopsSession() throws {
        let active = try Self.link("http://192.168.6.105:53231")
        #expect(ShellConnectionLifecycle.shouldStop(
            disappearingSession: active.sessionIdentity,
            activeSession: nil
        ))
    }

    @Test("Replacing a host tears down only the old session")
    func replacementStopsOldSession() throws {
        let old = try Self.link("http://192.168.100.71:53231")
        let replacement = try Self.link("http://192.168.6.105:53231", token: "other")
        #expect(ShellConnectionLifecycle.shouldStop(
            disappearingSession: old.sessionIdentity,
            activeSession: replacement.sessionIdentity
        ))
        #expect(!ShellConnectionLifecycle.shouldStop(
            disappearingSession: replacement.sessionIdentity,
            activeSession: replacement.sessionIdentity
        ))
    }

    @Test("A rescan that keeps the record but moves the origin replaces the session")
    func rescanAtNewOriginStopsOldSession() throws {
        // Same saved record (same id) — only the address changed, so the old
        // session still dials a dead host and must be released.
        let saved = try Self.link("http://192.168.100.71:53231")
        let rescanned = saved.adoptingRescan(try Self.link("http://192.168.6.105:53231"))
        #expect(rescanned.id == saved.id)
        #expect(rescanned.sessionIdentity != saved.sessionIdentity)
        #expect(ShellConnectionLifecycle.shouldStop(
            disappearingSession: saved.sessionIdentity,
            activeSession: rescanned.sessionIdentity
        ))
    }
}
