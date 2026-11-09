import Foundation
import Testing
@testable import SeRemote

@Suite("Recovering a saved host that no longer opens")
struct PairingRecoveryTests {
    @Test("A host that refuses the credential asks for a new scan")
    func rejectedCredential() {
        #expect(PairingRecovery.needsRePairing(
            HostError.rePairingRequired("This host no longer accepts this device's pairing.")
        ))
    }

    @Test(
        "An address the saved origin cannot reach asks for a new scan",
        arguments: [
            URLError.Code.cannotConnectToHost,
            .cannotFindHost,
            .dnsLookupFailed,
            .networkConnectionLost
        ]
    )
    func staleOrigin(code: URLError.Code) {
        #expect(PairingRecovery.needsRePairing(URLError(code)))
    }

    @Test(
        "Transient failures stay retryable",
        arguments: [
            URLError.Code.timedOut,
            .notConnectedToInternet,
            .secureConnectionFailed
        ]
    )
    func transient(code: URLError.Code) {
        #expect(!PairingRecovery.needsRePairing(URLError(code)))
    }

    @Test("Host-side failures that carry a message are not pairing problems")
    func hostFailures() {
        #expect(!PairingRecovery.needsRePairing(HostError.network("boom")))
        #expect(!PairingRecovery.needsRePairing(HostError.ipc("boom", "unsupported")))
        #expect(!PairingRecovery.needsRePairing(HostError.unexpected("boom")))
        #expect(!PairingRecovery.needsRePairing(CancellationError()))
    }

    @Test("A saved host keeps its port and its bearer across a reload")
    func savedRecordKeepsOriginAndBearer() throws {
        let paired = try RemoteLink.parse("http://192.168.100.71:52953/#access_token=host-bearer")
        // `ConnectionStore.persist` writes `savedOrigin` and the keychain keeps
        // the bearer; `load` rebuilds the link from exactly those two values.
        let saved = paired.savedOrigin
        #expect(saved == "http://192.168.100.71:52953")

        let reloaded = RemoteLink(
            id: paired.id,
            title: paired.title,
            accessURL: try #require(URL(string: saved)),
            bearer: paired.pairingToken
        )
        #expect(reloaded.originURL == paired.originURL)
        #expect(reloaded.pairingToken == "host-bearer")
        // The reloaded link must dial the same scheme/host/port with the same
        // bearer; that pair is what the host admits.
        let dialing = HostCredentials(accessURL: reloaded.accessURL, bearer: reloaded.pairingToken)
        #expect(dialing.originString == "http://192.168.100.71:52953")
        #expect(dialing.bearer == "host-bearer")
    }

    @Test("A saved tunnel origin keeps its host so a rotated tunnel is detectable")
    func savedTunnelOrigin() throws {
        let paired = try RemoteLink.parse("https://shipped-cause-cab-italia.trycloudflare.com/#access_token=t")
        #expect(paired.savedOrigin == "https://shipped-cause-cab-italia.trycloudflare.com")
        let other = try RemoteLink.parse("https://next-tunnel.trycloudflare.com/#access_token=t")
        #expect(paired.originURL != other.originURL)
    }
}
