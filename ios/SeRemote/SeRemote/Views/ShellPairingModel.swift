import Foundation
import Observation

/// Parsing is deliberately separate from connecting: a valid URL is not a
/// successful host handshake, and must never produce success feedback.
@MainActor
@Observable
final class ShellPairingDraft {
    var url = "" {
        didSet { errorMessage = nil }
    }
    private(set) var errorMessage: String?

    var canSubmit: Bool {
        !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    func parse() -> RemoteLink? {
        do {
            let link = try RemoteLink.parse(url)
            errorMessage = nil
            return link
        } catch {
            errorMessage = error.localizedDescription
            return nil
        }
    }
}

/// Records explicit Open actions separately from confirmed host handshakes.
/// UUID keys contain no access URL or pairing secret, and historical connection
/// success never implies current host reachability.
@MainActor
@Observable
final class ShellPairingActivity {
    static let storageKey = "se.shell.lastOpenedHosts"
    static let connectedStorageKey = "se.shell.lastConnectedHosts"
    private let defaults: UserDefaults
    private(set) var lastOpened: [String: Double]
    private(set) var lastConnected: [String: Double]

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        lastOpened = defaults.dictionary(forKey: Self.storageKey) as? [String: Double] ?? [:]
        lastConnected = defaults.dictionary(forKey: Self.connectedStorageKey) as? [String: Double] ?? [:]
    }

    func reload() {
        lastOpened = defaults.dictionary(forKey: Self.storageKey) as? [String: Double] ?? [:]
        lastConnected = defaults.dictionary(forKey: Self.connectedStorageKey) as? [String: Double] ?? [:]
    }

    func date(for link: RemoteLink) -> Date? {
        lastOpened[link.id.uuidString].map(Date.init(timeIntervalSince1970:))
    }

    func connectedDate(for link: RemoteLink) -> Date? {
        lastConnected[link.id.uuidString].map(Date.init(timeIntervalSince1970:))
    }

    func recordOpen(_ link: RemoteLink, at date: Date = .now) {
        reload()
        lastOpened[link.id.uuidString] = date.timeIntervalSince1970
        persist()
    }

    /// Call only after the workspace reports a completed host handshake.
    func recordConnection(_ link: RemoteLink, at date: Date = .now) {
        reload()
        lastConnected[link.id.uuidString] = date.timeIntervalSince1970
        persist()
    }

    func forget(_ link: RemoteLink) {
        reload()
        lastOpened.removeValue(forKey: link.id.uuidString)
        lastConnected.removeValue(forKey: link.id.uuidString)
        persist()
    }

    private func persist() {
        defaults.set(lastOpened, forKey: Self.storageKey)
        defaults.set(lastConnected, forKey: Self.connectedStorageKey)
    }
}

/// Bundle metadata, never a hard-coded release version.
enum ShellPairingAppInfo {
    static func version(in bundle: Bundle = .main) -> String {
        bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
    }

    static func build(in bundle: Bundle = .main) -> String {
        bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
    }
}
