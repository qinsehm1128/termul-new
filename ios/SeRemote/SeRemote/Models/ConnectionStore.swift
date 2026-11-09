import Foundation
import Observation

private struct SavedLinkRecord: Codable {
    var id: UUID
    var title: String
    var origin: String
    var createdAt: Date
}

@MainActor
@Observable
final class ConnectionStore {
    private static let storageKey = "se.remote.savedLinks"

    /// Pre-rename key. Read-only: this list is hand-curated by the user, so a
    /// silent loss on the first launch after the rename is the worst outcome
    /// this migration can produce. `load()` copies it forward; nothing ever
    /// writes or removes it.
    private static let legacyStorageKey = "termul.remote.savedLinks"

    var savedLinks: [RemoteLink] = []
    var activeLink: RemoteLink?
    var surface: WorkspaceSurface = .chat
    var errorMessage: String?
    var isScanning = false
    /// A connected screen asked for a fresh pairing (its saved origin or its
    /// credential can no longer open the host). Transient: the home screen
    /// consumes it by presenting the pairing sheet.
    var pairingRequested = false

    init() {
        savedLinks = Self.load()
    }

    func connect(to raw: String) {
        do {
            let link = try RemoteLink.parse(raw)
            if let current = activeLink,
               current.originHost == link.originHost,
               current.pairingToken == link.pairingToken {
                remember(current)
                errorMessage = nil
                return
            }
            remember(link)
            errorMessage = nil
            surface = .chat
            activeLink = link
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Returns the saved record the session dials. A rescan of a host that moved
    /// to a new origin folds into the existing record, so this is not always the
    /// link that was passed in.
    @discardableResult
    func connect(link: RemoteLink, surface: WorkspaceSurface = .chat) -> RemoteLink {
        let stored = remember(link)
        errorMessage = nil
        self.surface = surface
        activeLink = stored
        return stored
    }

    func disconnect() {
        activeLink = nil
        surface = .chat
    }

    /// Leave a host that cannot be opened and hand the user the pairing sheet.
    ///
    /// Used instead of a bare retry when the saved origin no longer reaches the
    /// host or the host refuses this device's credential: only a fresh scan can
    /// supply the other origin or the new credential.
    func requestRePairing() {
        disconnect()
        errorMessage = nil
        pairingRequested = true
        HostLog.session.info("Requested a fresh pairing")
    }

    func forget(_ link: RemoteLink) {
        savedLinks.removeAll { $0.id == link.id }
        KeychainStore.deleteBearer(account: link.id.uuidString)
        if activeLink?.id == link.id {
            disconnect()
        }
        persist()
        HostLog.session.info("Forgot a saved desk")
    }

    func rename(_ link: RemoteLink, title: String) {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        guard let index = savedLinks.firstIndex(where: { $0.id == link.id }) else { return }
        savedLinks[index].title = trimmed
        if activeLink?.id == link.id {
            activeLink?.title = trimmed
        }
        persist()
        HostLog.session.info("Renamed a saved desk")
    }

    func openIncomingURL(_ url: URL) {
        HostLog.session.info("Opening pairing link")
        connect(to: url.absoluteString)
    }

    func dismissError() {
        errorMessage = nil
    }

    @discardableResult
    private func remember(_ link: RemoteLink) -> RemoteLink {
        let folded = Self.fold(rescan: link, into: savedLinks)
        savedLinks = folded.saved
        // A mounted session must dial the address that was just scanned: the
        // active link follows the record this scan was folded into, so the dead
        // origin cannot survive a rescan.
        if activeLink.map({ $0.id == link.id || $0.refersToSameHost(as: link) }) == true {
            activeLink = folded.stored
        }
        persist()
        return folded.stored
    }

    /// Fold a freshly scanned link into the saved list.
    ///
    /// A rescan of a host whose address moved (`192.168.100.71` → `192.168.6.105`)
    /// arrives as a different origin carrying the same credential. Inserting it
    /// beside the old row would keep the dead origin in the list and split the
    /// host's activity history across two records, so the scan updates the record
    /// it belongs to and every stale duplicate is dropped.
    ///
    /// Pure so the dedupe rules are testable without UserDefaults or the keychain.
    static func fold(
        rescan link: RemoteLink,
        into saved: [RemoteLink]
    ) -> (stored: RemoteLink, saved: [RemoteLink]) {
        let stored = saved.first { $0.refersToSameHost(as: link) }?.adoptingRescan(link) ?? link
        let remaining = saved.filter { candidate in
            candidate.id != stored.id
                && !candidate.refersToSameHost(as: stored)
                && candidate.accessURL.absoluteString != stored.accessURL.absoluteString
        }
        return (stored, [stored] + remaining)
    }

    private func persist() {
        let records = savedLinks.map { link in
            if let token = link.pairingToken, !token.isEmpty {
                KeychainStore.saveBearer(token, account: link.id.uuidString)
            }
            return SavedLinkRecord(
                id: link.id,
                title: link.title,
                origin: link.savedOrigin,
                createdAt: link.createdAt
            )
        }
        guard let data = try? JSONEncoder().encode(records) else { return }
        UserDefaults.standard.set(data, forKey: Self.storageKey)
    }

    private static func load() -> [RemoteLink] {
        let current = UserDefaults.standard.data(forKey: storageKey)
        guard let data = current ?? UserDefaults.standard.data(forKey: legacyStorageKey) else { return [] }
        if let records = try? JSONDecoder().decode([SavedLinkRecord].self, from: data) {
            if current == nil {
                // Byte-for-byte copy forward. The pre-rename key keeps its copy.
                UserDefaults.standard.set(data, forKey: storageKey)
            }
            return records.compactMap { record in
                guard let origin = URL(string: record.origin) else { return nil }
                guard let bearer = KeychainStore.loadBearer(account: record.id.uuidString), !bearer.isEmpty else {
                    HostLog.session.error("Saved desk is missing its pairing secret; skipping")
                    return nil
                }
                return RemoteLink(
                    id: record.id,
                    title: record.title,
                    accessURL: origin,
                    bearer: bearer,
                    createdAt: record.createdAt
                )
            }
        }
        guard let legacy = try? JSONDecoder().decode([RemoteLink].self, from: data) else { return [] }
        for link in legacy {
            if let token = link.pairingToken, !token.isEmpty {
                KeychainStore.saveBearer(token, account: link.id.uuidString)
            }
        }
        let migrated = ConnectionStorePlaceholder.records(from: legacy)
        if let encoded = try? JSONEncoder().encode(migrated) {
            UserDefaults.standard.set(encoded, forKey: storageKey)
        }
        return legacy
    }
}

/// Tiny helper so `load()` can build metadata records without a live store.
private enum ConnectionStorePlaceholder {
    static func records(from links: [RemoteLink]) -> [SavedLinkRecord] {
        links.map {
            SavedLinkRecord(
                id: $0.id,
                title: $0.title,
                origin: $0.savedOrigin,
                createdAt: $0.createdAt
            )
        }
    }
}
