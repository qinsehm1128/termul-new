import Foundation
import Testing
@testable import SeRemote

/// A saved host keeps the origin it was paired with, and that origin dies with
/// the desktop's address: a DHCP lease, a different Wi-Fi or a restarted tunnel
/// moves it. Rescanning the same host's QR must update the record that is
/// already saved, not add a second one that leaves the dead origin on the home
/// screen.
@Suite("Rescanning a saved host updates its record")
struct SavedHostMergeTests {
    private static func lan(_ host: String, port: Int = 53231, token: String = "mac-bearer") throws -> RemoteLink {
        try RemoteLink.parse("http://\(host):\(port)/#access_token=\(token)")
    }

    @Test("A new LAN address with the same credential updates the saved record")
    func rescanAtNewAddressUpdatesRecord() throws {
        let saved = try Self.lan("192.168.100.71")
        let rescanned = try Self.lan("192.168.6.105")

        let folded = ConnectionStore.fold(rescan: rescanned, into: [saved])

        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == saved.id)
        #expect(folded.stored.createdAt == saved.createdAt)
        #expect(folded.stored.savedOrigin == "http://192.168.6.105:53231")
        #expect(folded.stored.pairingToken == "mac-bearer")
    }

    @Test("The dead origin does not survive the rescan")
    func deadOriginIsDropped() throws {
        let saved = try Self.lan("192.168.100.71")
        let rescanned = try Self.lan("192.168.6.105")

        let folded = ConnectionStore.fold(rescan: rescanned, into: [saved])

        #expect(!folded.saved.contains { $0.savedOrigin == "http://192.168.100.71:53231" })
        #expect(folded.saved.allSatisfy { $0.savedOrigin == "http://192.168.6.105:53231" })
    }

    @Test("The record that survives dials the new origin")
    func storedRecordDialsNewOrigin() throws {
        let folded = ConnectionStore.fold(
            rescan: try Self.lan("192.168.6.105"),
            into: [try Self.lan("192.168.100.71")]
        )
        // What the workspace is handed: the origin and bearer the host admits,
        // not the address the record was created with.
        let dialing = HostCredentials(
            accessURL: folded.stored.accessURL,
            bearer: folded.stored.pairingToken
        )
        #expect(dialing.originString == "http://192.168.6.105:53231")
        #expect(dialing.bearer == "mac-bearer")
    }

    @Test("A name the user gave the host outlives the address change")
    func customTitleIsKept() throws {
        var saved = try Self.lan("192.168.100.71")
        saved.title = "Studio Mac"

        let folded = ConnectionStore.fold(rescan: try Self.lan("192.168.6.105"), into: [saved])

        #expect(folded.stored.title == "Studio Mac")
        #expect(folded.stored.savedOrigin == "http://192.168.6.105:53231")
    }

    @Test("An unnamed record follows the address it now dials")
    func defaultTitleFollowsAddress() throws {
        let folded = ConnectionStore.fold(
            rescan: try Self.lan("192.168.6.105"),
            into: [try Self.lan("192.168.100.71")]
        )
        #expect(folded.stored.title == "192.168.6.105")
    }

    @Test("A different host stays a separate record")
    func otherHostIsAdded() throws {
        let saved = try Self.lan("192.168.100.71")
        let other = try Self.lan("192.168.6.105", token: "second-mac-bearer")

        let folded = ConnectionStore.fold(rescan: other, into: [saved])

        #expect(folded.saved.count == 2)
        #expect(folded.stored.id == other.id)
        #expect(folded.saved.contains { $0.id == saved.id && $0.savedOrigin == saved.savedOrigin })
    }

    @Test("A token-less record still matches the host it was saved from")
    func tokenlessRecordMatchesByHost() throws {
        let legacy = RemoteLink(accessURL: try #require(URL(string: "http://192.168.1.5:4317")))
        #expect(legacy.hostIdentity == nil)

        let folded = ConnectionStore.fold(
            rescan: try Self.lan("192.168.1.5", port: 4317, token: "issued-later"),
            into: [legacy]
        )

        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == legacy.id)
        #expect(folded.stored.pairingToken == "issued-later")
    }

    @Test("Stale duplicates of the same host collapse into one record")
    func duplicatesCollapse() throws {
        let first = try Self.lan("192.168.100.71")
        let staleDuplicate = try Self.lan("192.168.100.99")

        let folded = ConnectionStore.fold(rescan: try Self.lan("192.168.6.105"), into: [first, staleDuplicate])

        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == first.id)
    }

    @Test("Rescanning the very address already saved is a no-op")
    func rescanOfSameAddressKeepsRecord() throws {
        let saved = try Self.lan("192.168.6.105")

        let folded = ConnectionStore.fold(rescan: try Self.lan("192.168.6.105", token: "mac-bearer"), into: [saved])

        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == saved.id)
        #expect(folded.stored.savedOrigin == saved.savedOrigin)
    }

    @Test("Identity across origins is the credential, not the address")
    func hostIdentityUsesCredential() throws {
        let saved = try Self.lan("192.168.100.71")
        let rescanned = try Self.lan("192.168.6.105")
        let other = try Self.lan("192.168.6.105", token: "second-mac-bearer")

        #expect(saved.refersToSameHost(as: rescanned))
        #expect(rescanned.hostIdentity == saved.hostIdentity)
        #expect(other.hostIdentity != saved.hostIdentity)
        // Same address, different credential: still the machine at that address.
        #expect(rescanned.refersToSameHost(as: other))
    }
}
