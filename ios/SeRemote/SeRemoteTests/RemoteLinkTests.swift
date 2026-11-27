import Foundation
import Testing
@testable import SeRemote

@Suite("RemoteLink pairing URL parsing")
struct RemoteLinkTests {
    @Test("An https access URL keeps its fragment bearer")
    func httpsFragmentBearer() throws {
        let link = try RemoteLink.parse("https://abc.trycloudflare.com/#access_token=s3cret")
        #expect(link.pairingToken == "s3cret")
        #expect(link.originHost == "abc.trycloudflare.com")
        #expect(link.originURL.absoluteString == "https://abc.trycloudflare.com")
    }

    @Test("A LAN http URL with a query bearer is accepted")
    func lanHTTPQueryBearer() throws {
        let link = try RemoteLink.parse("  http://192.168.1.20:4317/?access_token=tok\n")
        #expect(link.pairingToken == "tok")
        #expect(RemoteLink.accessToken(in: link.accessURL) == "tok")
    }

    @Test("A URL without a bearer is rejected as missing token")
    func missingToken() {
        #expect(throws: RemoteLinkError.self) {
            try RemoteLink.parse("https://example.com/")
        }
    }

    @Test("Non-http schemes are rejected", arguments: ["ftp://host/#access_token=a", "termul://open?url=x"])
    func rejectsOtherSchemes(raw: String) {
        #expect(throws: RemoteLinkError.self) {
            try RemoteLink.parse(raw)
        }
    }

    @Test("Garbage input is an invalid URL")
    func garbage() {
        #expect(throws: RemoteLinkError.self) {
            try RemoteLink.parse("not a link")
        }
    }

    @Test("se:// deep link unwraps the embedded access URL")
    func deepLink() throws {
        let inner = "https://host.example/#access_token=deep"
        let encoded = inner.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? inner
        let link = try RemoteLink.parse("se://open?url=\(encoded)")
        #expect(link.pairingToken == "deep")
        #expect(link.originHost == "host.example")
    }

    @Test("se:// deep link accepts access_token as a query item")
    func deepLinkQueryToken() throws {
        let link = try RemoteLink.parse("se://open?url=https%3A%2F%2Fhost.example%2F&access_token=qtok")
        #expect(link.pairingToken == "qtok")
    }

    @Test("Fragment tokens are percent-decoded")
    func percentDecodedFragment() {
        #expect(RemoteLink.accessToken(inFragment: "x=1&access_token=a%2Bb") == "a+b")
        #expect(RemoteLink.accessToken(inFragment: "") == nil)
        #expect(RemoteLink.accessToken(inFragment: nil) == nil)
    }

    @Test("Codable round trip preserves the bearer")
    func codableRoundTrip() throws {
        let link = try RemoteLink.parse("https://h.example/#access_token=rt")
        let data = try JSONEncoder().encode(link)
        let decoded = try JSONDecoder().decode(RemoteLink.self, from: data)
        #expect(decoded == link)
        #expect(decoded.pairingToken == "rt")
    }

    @Test("Private network hosts are recognized", arguments: [
        ("localhost", true), ("mac.local", true), ("10.0.0.2", true), ("172.16.4.1", true),
        ("172.32.0.1", false), ("192.168.0.9", true), ("169.254.1.1", true), ("8.8.8.8", false),
        ("example.com", false),
    ])
    func privateHosts(host: String, expected: Bool) {
        #expect(RemoteLink.isPrivateNetworkHost(host) == expected)
    }

    @Test("The Tailscale CGNAT range counts as a private network host", arguments: [
        // RFC 6598 100.64.0.0/10: inclusive bounds, and the two just outside.
        ("100.64.0.4", true), ("100.127.255.254", true),
        ("100.63.255.255", false), ("100.128.0.1", false),
        ("100.64.0.4.5", false), ("not-an-ip", false),
    ])
    func cgnatPrivateHosts(host: String, expected: Bool) {
        #expect(RemoteLink.isPrivateNetworkHost(host) == expected)
    }

    @Test("A tailnet host is a CGNAT literal only", arguments: [
        ("100.64.0.4", true), ("100.100.100.100", true),
        ("100.128.0.1", false), ("192.168.1.20", false), ("8.8.8.8", false),
        // MagicDNS names resolve through the tailnet but carry no such guarantee.
        ("mac.tail1234.ts.net", false), ("mac.vpn.internal", false), ("", false),
    ])
    func tailnetHosts(host: String, expected: Bool) {
        #expect(RemoteLink.isTailnetHost(host) == expected)
        // Every tailnet host is private, so the cleartext policy agrees with the
        // tailnet-specific messaging.
        if expected {
            #expect(RemoteLink.isPrivateNetworkHost(host))
        }
    }
}
