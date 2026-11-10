import Foundation
import Testing
@testable import SeRemote

@Suite("Host deep links")
struct HostDeepLinkTests {
    @Test("A pairing link is handed back untouched")
    func pairing() throws {
        let url = try #require(URL(string: "se://open?url=https%3A%2F%2Fhost.example%2F%23pair%3Dabc"))
        #expect(HostDeepLink.parse(url) == .pairing(url))
        // The pre-existing form keeps the embedded URL; parsing still belongs
        // to RemoteLink, never to the terminal route.
        #expect((try? RemoteLink.parse(url.absoluteString)) != nil)
    }

    @Test("A terminal link carries the terminal and optional host")
    func terminal() throws {
        let url = try #require(URL(string: "se://terminal?host=host-1&terminal=pty-9"))
        #expect(HostDeepLink.parse(url) == .terminal(TerminalRoute(hostID: "host-1", terminalID: "pty-9")))

        let noHost = try #require(URL(string: "se://terminal?terminal=pty-9"))
        #expect(HostDeepLink.parse(noHost) == .terminal(TerminalRoute(hostID: nil, terminalID: "pty-9")))
    }

    @Test("Unknown and malformed links never become a destination")
    func unknown() throws {
        for raw in [
            "se://terminal",
            "se://terminal?terminal=",
            "se://other?terminal=pty-9",
            "https://host.example/terminal?terminal=pty-9",
            "not a url",
        ] {
            let url = try #require(URL(string: raw))
            #expect(HostDeepLink.parse(url) == .unknown, "\(raw) must not route")
        }
    }

    @Test("Whitespace around a query value is not an id")
    func trimsValues() throws {
        let url = try #require(URL(string: "se://terminal?terminal=%20%20"))
        #expect(HostDeepLink.parse(url) == .unknown)
    }
}
