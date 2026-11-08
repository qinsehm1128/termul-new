import Foundation
import Testing

@testable import SeRemote

@Suite("Shell pairing")
@MainActor
struct ShellPairingTests {
    @Test("Blank input cannot be submitted", arguments: ["", " ", "\n\t"])
    func emptyInput(raw: String) {
        let draft = ShellPairingDraft()
        draft.url = raw
        #expect(!draft.canSubmit)
        #expect(draft.parse() == nil)
        #expect(draft.errorMessage != nil)
    }

    @Test(
        "Invalid URLs retain input and expose an inline error",
        arguments: [
            "not a link", "https://host.example/", "ftp://host.example/#access_token=do-not-leak-pairing-token",
        ])
    func invalidInput(raw: String) {
        let draft = ShellPairingDraft()
        draft.url = raw
        #expect(draft.parse() == nil)
        #expect(draft.url == raw)
        #expect(draft.errorMessage?.isEmpty == false)
        #expect(draft.errorMessage?.contains("do-not-leak-pairing-token") == false)
    }

    @Test(
        "Scan and paste use RemoteLink.parse without rewriting credentials",
        arguments: [
            "https://host.example/#access_token=a%2Bb",
            " http://192.168.1.20:4317/?access_token=lan\n",
        ])
    func validInput(raw: String) throws {
        let expected = try RemoteLink.parse(raw)
        let draft = ShellPairingDraft()
        draft.url = raw
        let actual = try #require(draft.parse())
        #expect(actual.originURL == expected.originURL)
        #expect(actual.pairingToken == expected.pairingToken)
        #expect(draft.errorMessage == nil)
    }

    @Test("Editing clears a stale error; correcting can be retried")
    func correction() throws {
        let draft = ShellPairingDraft()
        draft.url = "https://host.example/"
        #expect(draft.parse() == nil)
        #expect(draft.errorMessage != nil)
        draft.url = "https://host.example/#access_token=corrected"
        #expect(draft.errorMessage == nil)
        #expect(try #require(draft.parse()).pairingToken == "corrected")
    }

    @Test("Last opened persists separately from saved/connected state and stores no credentials")
    func recentActivity() throws {
        let suite = "ShellPairingTests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let activity = ShellPairingActivity(defaults: defaults)
        let link = try RemoteLink.parse("https://host.example/#access_token=never-store-this")
        #expect(activity.date(for: link) == nil)
        let time = Date(timeIntervalSince1970: 1_700_000_000)
        activity.recordOpen(link, at: time)
        #expect(activity.date(for: link) == time)
        let reloaded = ShellPairingActivity(defaults: defaults)
        #expect(reloaded.date(for: link) == time)
        let stored = try #require(defaults.dictionary(forKey: ShellPairingActivity.storageKey))
        #expect(Set(stored.keys) == [link.id.uuidString])
        #expect(!String(describing: stored).contains("never-store-this"))
        #expect(!String(describing: stored).contains("host.example"))
        reloaded.forget(link)
        #expect(ShellPairingActivity(defaults: defaults).date(for: link) == nil)
    }

    @Test("Only actual connection events record last connected; both histories persist and forget")
    func confirmedConnectionActivity() throws {
        let suite = "ShellPairingTests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let home = ShellPairingActivity(defaults: defaults)
        let workspace = ShellPairingActivity(defaults: defaults)
        let link = try RemoteLink.parse("https://host.example/#access_token=connection-secret")
        let opened = Date(timeIntervalSince1970: 1_700_000_000)
        let connected = opened.addingTimeInterval(10)
        #expect(home.connectedDate(for: link) == nil)
        home.recordOpen(link, at: opened)
        #expect(home.date(for: link) == opened)
        #expect(home.connectedDate(for: link) == nil)

        workspace.recordConnection(link, at: connected)
        #expect(workspace.connectedDate(for: link) == connected)
        #expect(workspace.date(for: link) == opened)
        home.reload()
        #expect(home.connectedDate(for: link) == connected)
        // A later unsuccessful Open cannot turn into a new connection timestamp
        // or overwrite the event recorded by a separate workspace instance.
        let reopened = connected.addingTimeInterval(20)
        home.recordOpen(link, at: reopened)
        let reloaded = ShellPairingActivity(defaults: defaults)
        #expect(reloaded.date(for: link) == reopened)
        #expect(reloaded.connectedDate(for: link) == connected)
        let stored = try #require(defaults.dictionary(forKey: ShellPairingActivity.connectedStorageKey))
        #expect(Set(stored.keys) == [link.id.uuidString])
        #expect(!String(describing: stored).contains("connection-secret"))
        #expect(!String(describing: stored).contains("host.example"))
        home.forget(link)
        let forgotten = ShellPairingActivity(defaults: defaults)
        #expect(forgotten.date(for: link) == nil)
        #expect(forgotten.connectedDate(for: link) == nil)
    }

    @Test("Renaming keeps the same recent activity identity")
    func renameKeepsActivity() throws {
        let suite = "ShellPairingTests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let activity = ShellPairingActivity(defaults: defaults)
        var link = try RemoteLink.parse("https://host.example/#access_token=token")
        let time = Date(timeIntervalSince1970: 1_700_000_001)
        activity.recordOpen(link, at: time)
        activity.recordConnection(link, at: time)
        link.title = "Renamed desktop"
        #expect(activity.date(for: link) == time)
        #expect(activity.connectedDate(for: link) == time)
    }

    @Test("About version and build are resolved from the bundle")
    func bundleMetadata() {
        #expect(
            ShellPairingAppInfo.version()
                == (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"))
        #expect(
            ShellPairingAppInfo.build()
                == (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"))
    }
}
