import Foundation
import Testing
@testable import SeRemote

/// The plugins browser decides what it is willing to load and whether the hub
/// may offer the entry. Both questions are pure and exercised here without a
/// web view.
struct PluginBrowserPolicyTests {
    private let origin = URL(string: "http://10.0.0.5:8090")!

    // MARK: - Bootstrap URL

    @Test func tokenEncodingCannotBreakTheFragment() throws {
        let token = "a&=#+% b"
        let url = try #require(PluginBrowserPolicy.bootstrapURL(
            origin: URL(string: "http://10.0.0.5:8090/old/path?x=1#frag")!,
            token: token
        ))
        let expectedToken = ["a", "%26", "%3D", "%23", "%2B", "%25", "%20", "b"].joined()
        #expect(url.absoluteString == "http://10.0.0.5:8090/#access_token=" + expectedToken + "&route=/plugins")

        // Parse the raw fragment the way the renderer's URLSearchParams does:
        // exactly two pairs, and the token decodes back to its original bytes.
        let fragment = try #require(url.absoluteString.split(separator: "#", maxSplits: 1).last.map(String.init))
        let pairs = fragment.split(separator: "&").map { $0.split(separator: "=", maxSplits: 1) }
        #expect(pairs.count == 2)
        #expect(pairs.map { String($0[0]) } == ["access_token", "route"])
        #expect(String(pairs[0][1]).removingPercentEncoding == token)
        #expect(String(pairs[1][1]) == "/plugins")
    }

    @Test func portIsKeptAndOriginPathIsDropped() throws {
        let url = try #require(PluginBrowserPolicy.bootstrapURL(
            origin: URL(string: "http://10.0.0.5:8090/some/path?query=1#fragment")!,
            token: "token"
        ))
        #expect(url.absoluteString.hasPrefix("http://10.0.0.5:8090/#access_token="))
        #expect(!url.absoluteString.contains("/some/path"))
        #expect(!url.absoluteString.contains("query=1"))
        #expect(url.fragment != nil)
    }

    @Test func httpsOriginKeepsItsScheme() throws {
        let url = try #require(PluginBrowserPolicy.bootstrapURL(
            origin: URL(string: "https://desk.example.com")!,
            token: "token"
        ))
        #expect(url.absoluteString.hasPrefix("https://desk.example.com/#access_token="))
    }

    @Test func missingTokenHasNoBootstrap() {
        #expect(PluginBrowserPolicy.bootstrapURL(origin: origin, token: nil) == nil)
        #expect(PluginBrowserPolicy.bootstrapURL(origin: origin, token: "") == nil)
    }

    @Test func nonWebOriginsHaveNoBootstrap() {
        #expect(PluginBrowserPolicy.bootstrapURL(origin: URL(string: "ftp://host/x")!, token: "token") == nil)
        #expect(PluginBrowserPolicy.bootstrapURL(origin: URL(string: "file:///tmp/x")!, token: "token") == nil)
    }

    // MARK: - decide()

    @Test func mainFrameStaysOnThePairedOrigin() {
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "http://10.0.0.5:8090/plugins"), isMainFrame: true, origin: origin) == .allow)
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "about:blank"), isMainFrame: true, origin: origin) == .allow)
    }

    @Test func defaultPortsNormalize() {
        let https = URL(string: "https://host")!
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "https://host:443/x"), isMainFrame: true, origin: https) == .allow)
        let http = URL(string: "http://host")!
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "http://host:80/x"), isMainFrame: true, origin: http) == .allow)
    }

    @Test func mismatchedOriginsLeaveTheApp() throws {
        let otherPort = try #require(URL(string: "http://10.0.0.5:8091/x"))
        #expect(PluginBrowserPolicy.decide(url: otherPort, isMainFrame: true, origin: origin)
            == .openExternally(otherPort))
        let otherHost = try #require(URL(string: "https://example.com/x"))
        #expect(PluginBrowserPolicy.decide(url: otherHost, isMainFrame: true, origin: origin)
            == .openExternally(otherHost))
    }

    @Test func nonWebMainFrameSchemesAreCancelled() throws {
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "javascript:alert(1)"), isMainFrame: true, origin: origin) == .cancel)
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "file:///etc/passwd"), isMainFrame: true, origin: origin) == .cancel)
        #expect(PluginBrowserPolicy.decide(url: nil, isMainFrame: true, origin: origin) == .cancel)
    }

    @Test func subframesAllowHostGovernedContent() throws {
        let allowed = [
            "http://10.0.0.5:8090/plugin",
            "https://cdn.example.com/x",
            "about:blank",
            "data:text/html,x",
            "blob:http://10.0.0.5:8090/abc",
        ]
        for raw in allowed {
            let url = try #require(URL(string: raw))
            #expect(PluginBrowserPolicy.decide(url: url, isMainFrame: false, origin: origin) == .allow)
        }
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "javascript:void(0)"), isMainFrame: false, origin: origin) == .cancel)
        #expect(PluginBrowserPolicy.decide(
            url: URL(string: "custom://plugin"), isMainFrame: false, origin: origin) == .cancel)
    }

    // MARK: - Entry gating

    @Test func pluginsEntryDoesNotFollowTheTerminalScope() {
        // Host-scoped: no terminal scope is involved at all.
        #expect(PluginBrowserPolicy.entryEnabled(isRevoked: false, scopes: ["view"]))
        // Legacy bearers without a scope list keep the old allow policy.
        #expect(PluginBrowserPolicy.entryEnabled(isRevoked: false, scopes: nil))
    }

    @Test func pluginsEntryRequiresALiveViewGrant() {
        let missingView = makeLink(scopes: ["terminal.input"])
        #expect(PluginBrowserPolicy.entryEnabled(isRevoked: false, scopes: ["terminal.input"]) == false)
        #expect(PluginBrowserPolicy.entryEnabled(isRevoked: true, scopes: ["view"]) == false)
        #expect(missingView.isRevoked == false)
    }

    @MainActor
    @Test func pluginsDestinationUsesTheLinkGrant() {
        #expect(ToolDestination.plugins.browserEntryEnabled(for: nil))
        #expect(ToolDestination.plugins.browserEntryEnabled(for: makeLink(scopes: ["view"])))
        #expect(!ToolDestination.plugins.browserEntryEnabled(for: makeLink(scopes: ["terminal.input"])))
        #expect(!ToolDestination.plugins.browserEntryEnabled(for: makeLink(isRevoked: true, scopes: ["view"])))
        // Other tools keep their existing gate (terminal scope lives elsewhere).
        #expect(ToolDestination.git.browserEntryEnabled(for: makeLink(scopes: ["terminal.input"])))
    }

    private func makeLink(isRevoked: Bool = false, scopes: [String]?) -> RemoteLink {
        var link = RemoteLink(accessURL: origin, bearer: "token")
        link.scopes = scopes
        link.isRevoked = isRevoked
        return link
    }
}
