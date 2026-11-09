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
        link.deviceId = "d1"
        link.deviceName = "Phone"
        link.scopes = ["view"]
        let restored = try JSONDecoder().decode(RemoteLink.self, from: JSONEncoder().encode(link))
        #expect(restored.deviceId == link.deviceId)
        #expect(restored.deviceName == "Phone")
        #expect(restored.scopes == ["view"])
        #expect(restored.pairCode == nil)
    }
}
