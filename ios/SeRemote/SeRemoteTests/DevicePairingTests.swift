import Foundation
import Testing
@testable import SeRemote

@Suite("Device pairing and scope contract")
struct DevicePairingTests {
    @Test("Pair-only links are accepted; codes remain fragment-only")
    func pairLink() throws {
        let link = try RemoteLink.parse("https://host.example/#pair=a%2Bb")
        #expect(link.pairCode == "a+b")
        #expect(link.pairingToken == nil)
        #expect(link.originURL.absoluteString == "https://host.example")
    }

    @Test("Pair and legacy bearer may coexist")
    func alongsideLegacy() throws {
        let link = try RemoteLink.parse("http://192.168.0.2:4321/#access_token=legacy&pair=once")
        #expect(link.pairCode == "once")
        #expect(link.pairingToken == "legacy")
    }

    @Test("Deep links carry a pair fragment outside or inside the embedded URL")
    func pairDeepLink() throws {
        let outer = try RemoteLink.parse("se://open?url=https%3A%2F%2Fhost.example%2F#pair=outer")
        #expect(outer.pairCode == "outer")
        let inner = try RemoteLink.parse("se://open?url=https%3A%2F%2Fhost.example%2F%23pair%3Dinner")
        #expect(inner.pairCode == "inner")
    }

    @Test("Empty pair code does not authorize a connection")
    func emptyPair() {
        #expect(throws: RemoteLinkError.self) { try RemoteLink.parse("https://host.example/#pair=") }
    }

    @Test("Pair response decodes the camelCase contract")
    func decodeExchange() throws {
        let data = Data(#"{"deviceId":"d1","deviceToken":"dev_tok","scopes":["view","terminal.input"]}"#.utf8)
        let grant = try JSONDecoder().decode(PairExchange.self, from: data)
        #expect(grant.deviceId == "d1")
        #expect(grant.deviceToken == "dev_tok")
        #expect(grant.scopes == ["view", "terminal.input"])
    }

    @Test("Rescanning keeps the saved identity and device token, never the legacy host token")
    func rescanDeviceMerge() throws {
        var saved = try RemoteLink.parse("https://host.example/#access_token=device-secret")
        saved.title = "My Mac"
        saved.deviceId = "d1"
        saved.deviceName = "Phone"
        saved.scopes = ["view"]
        let scan = try RemoteLink.parse("https://host.example/#access_token=legacy&pair=new-code")
        let merged = saved.adoptingRescan(scan)
        #expect(merged.id == saved.id)
        #expect(merged.title == "My Mac")
        #expect(merged.pairingToken == "device-secret")
        #expect(merged.deviceId == "d1")
        #expect(merged.scopes == ["view"])
        #expect(merged.pairCode == "new-code")
    }

    @Test("A successful new exchange rotates only the credential, retaining the saved host")
    func exchangedMerge() throws {
        var saved = try RemoteLink.parse("https://host.example/#access_token=old-device")
        saved.deviceId = "old"
        saved.isRevoked = true
        var fresh = RemoteLink(accessURL: saved.originURL, bearer: "new-device")
        fresh.deviceId = "new"
        fresh.deviceName = "Phone"
        fresh.scopes = ["view", "terminal.create"]
        let merged = saved.adoptingRescan(fresh)
        #expect(merged.id == saved.id)
        #expect(merged.pairingToken == "new-device")
        #expect(!merged.isRevoked)
        #expect(merged.scopes == fresh.scopes)
    }

    @Test("Capabilities map through one localized permission vocabulary")
    func permissionLabels() {
        #expect(DeviceScope.label(for: "terminal.input") == String(localized: "Terminal input"))
        #expect(DeviceScope.label(for: "terminal.create") == String(localized: "Create terminals"))
        #expect(DeviceScope.label(for: "agent.run") == String(localized: "Run agents"))
        #expect(DeviceScope.label(for: "files.write") == String(localized: "Write files"))
        #expect(DeviceScope.label(for: "git.write") == String(localized: "Git write"))
        #expect(DeviceScope.label(for: "screen.view") == String(localized: "View screen"))
        #expect(DeviceScope.label(for: "screen.control") == String(localized: "Control screen"))
        #expect(DeviceScope.label(for: "future.internal.scope") == String(localized: "Additional permission"))
    }

    @Test("Scopes distinguish legacy, denied, and revoked hosts")
    func scopePolicy() {
        var link = RemoteLink(accessURL: URL(string: "https://host.example")!, bearer: "t")
        #expect(link.allows("terminal.input"))
        link.scopes = []
        #expect(!link.allows("terminal.input"))
        link.scopes = ["view", "terminal.input"]
        #expect(link.allows("terminal.input"))
        #expect(!link.allows("agent.run"))
        link.isRevoked = true
        #expect(!link.allows("terminal.input"))
    }

    @Test("Stable host identity merges a moved origin and rotated device token")
    func movedHostIdentityMerge() {
        var saved = RemoteLink(accessURL: URL(string: "https://old.example")!, bearer: "old-device")
        saved.hostId = "01234567-89AB-4CDE-8123-456789ABCDEF"
        saved.deviceId = "old-device-id"
        saved.title = "My Mac"
        var fresh = RemoteLink(accessURL: URL(string: "https://new.example")!, bearer: "new-device")
        fresh.hostId = "01234567-89ab-4cde-8123-456789abcdef"
        fresh.deviceId = "new-device-id"
        fresh.scopes = ["view"]
        let folded = ConnectionStore.fold(rescan: fresh, into: [saved])
        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == saved.id)
        #expect(folded.stored.title == "My Mac")
        #expect(folded.stored.createdAt == saved.createdAt)
        #expect(folded.stored.originURL == fresh.originURL)
        #expect(folded.stored.pairingToken == "new-device")
        #expect(folded.stored.deviceId == "new-device-id")
        #expect(folded.stored.stableHostID == fresh.stableHostID)
    }

    @Test("Different host identities remain separate even at the same recycled address")
    func differentHostIdentityCreatesRecord() {
        var saved = RemoteLink(accessURL: URL(string: "https://host.example")!, bearer: "old-device")
        saved.hostId = "host-one"
        var fresh = RemoteLink(accessURL: saved.originURL, bearer: "new-device")
        fresh.hostId = "host-two"
        fresh.deviceId = "new-device-id"
        let folded = ConnectionStore.fold(rescan: fresh, into: [saved])
        #expect(folded.saved.count == 2)
        #expect(folded.stored.id == fresh.id)
        #expect(folded.saved.contains { $0.id == saved.id })
        #expect(!saved.refersToSameHost(as: fresh))
    }

    @Test("Missing hostId retains same-origin and legacy same-bearer fallbacks")
    func missingHostIdentityFallsBack() {
        var saved = RemoteLink(accessURL: URL(string: "https://old.example")!, bearer: "legacy")
        saved.hostId = "known-host"
        let movedLegacy = RemoteLink(accessURL: URL(string: "https://new.example")!, bearer: "legacy")
        let moved = ConnectionStore.fold(rescan: movedLegacy, into: [saved])
        #expect(moved.saved.count == 1)
        #expect(moved.stored.id == saved.id)
        #expect(moved.stored.hostId == saved.hostId)
        var rotated = RemoteLink(accessURL: saved.originURL, bearer: "rotated-device")
        rotated.deviceId = "new-device-id"
        let sameOrigin = ConnectionStore.fold(rescan: rotated, into: [saved])
        #expect(sameOrigin.saved.count == 1)
        #expect(sameOrigin.stored.id == saved.id)
        #expect(sameOrigin.stored.pairingToken == "rotated-device")
    }

    @Test("Known host identity outranks another legacy record at its new address")
    func identityOutranksOriginFallback() {
        let legacy = RemoteLink(accessURL: URL(string: "https://new.example")!, bearer: "legacy")
        var saved = RemoteLink(accessURL: URL(string: "https://old.example")!, bearer: "old-device")
        saved.hostId = "known-host"
        var fresh = RemoteLink(accessURL: legacy.originURL, bearer: "new-device")
        fresh.hostId = "known-host"
        fresh.deviceId = "new-device-id"
        let folded = ConnectionStore.fold(rescan: fresh, into: [legacy, saved])
        #expect(folded.saved.count == 1)
        #expect(folded.stored.id == saved.id)
    }

    @Test("Public info identity is optional and uses the raw hostId contract")
    func decodePublicHostInfo() {
        let data = Data(#"{"hostId":"01234567-89AB-4CDE-8123-456789ABCDEF"}"#.utf8)
        #expect(HostIdentityClient.decode(data) == "01234567-89ab-4cde-8123-456789abcdef")
        #expect(HostIdentityClient.decode(Data(#"{"hostId":""}"#.utf8)) == nil)
        #expect(HostIdentityClient.decode(Data(#"{}"#.utf8)) == nil)
        #expect(HostIdentityClient.decode(Data("OK".utf8)) == nil)
    }

    @Test("Pair responses optionally carry stable hostId")
    func decodeOptionalHostIdentity() throws {
        let data = Data(#"{"hostId":"host-one","deviceId":"d1","deviceToken":"tok","scopes":["view"]}"#.utf8)
        #expect(try JSONDecoder().decode(PairExchange.self, from: data).hostId == "host-one")
        let legacy = Data(#"{"deviceId":"d1","deviceToken":"tok","scopes":["view"]}"#.utf8)
        #expect(try JSONDecoder().decode(PairExchange.self, from: legacy).hostId == nil)
    }

    @Test("One-time pair fragments are never encoded")
    func codeIsEphemeral() throws {
        let link = try RemoteLink.parse("https://host.example/#pair=secret-once")
        let data = try JSONEncoder().encode(link)
        #expect(!String(decoding: data, as: UTF8.self).contains("secret-once"))
        let restored = try JSONDecoder().decode(RemoteLink.self, from: data)
        #expect(restored.pairCode == nil)
    }

    @Test("Device grants survive Codable without an ephemeral pair code")
    func metadataRoundTrip() throws {
        var link = RemoteLink(accessURL: URL(string: "https://host.example")!, bearer: "t")
        link.hostId = "stable-host"
        link.deviceId = "d1"
        link.deviceName = "Phone"
        link.scopes = ["view"]
        let restored = try JSONDecoder().decode(RemoteLink.self, from: JSONEncoder().encode(link))
        #expect(restored.hostId == "stable-host")
        #expect(restored.deviceId == link.deviceId)
        #expect(restored.deviceName == "Phone")
        #expect(restored.scopes == ["view"])
        #expect(restored.pairCode == nil)
    }
}
