import Foundation
import Observation
import UIKit

private struct SavedLinkRecord: Codable {
    var id: UUID
    var title: String
    var origin: String
    var createdAt: Date
    var deviceId: String? = nil
    var deviceName: String? = nil
    var scopes: [String]? = nil
    var isRevoked: Bool? = nil
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
    var isPairing = false
    private var pairingGeneration = UUID()
    private var pairingTargetID: UUID?
    /// A connected screen asked for a fresh pairing (its saved origin or its
    /// credential can no longer open the host). Transient: the home screen
    /// consumes it by presenting the pairing sheet.
    var pairingRequested = false

    init() {
        savedLinks = Self.load()
    }

    func connect(to raw: String) {
        do {
            let scanned = try RemoteLink.parse(raw)
            // Explicit scans may rotate credentials. Leave the old workspace
            // intentionally (detach only, never kill its host PTYs) before opening
            // the new grant; ordinary permission/revocation updates stay mounted.
            if let current = activeLink {
                if current.refersToSameHost(as: scanned) { pairingTargetID = current.id }
                disconnect()
            }
            Task {
                do { _ = try await connectScanned(scanned) }
                catch { errorMessage = error.localizedDescription }
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// The code is consumed before any record or session sees it.
    @discardableResult
    func connectScanned(_ link: RemoteLink, surface: WorkspaceSurface = .chat) async throws -> RemoteLink {
        guard !isPairing else { throw PairingExchangeError.failed }
        isPairing = true
        let generation = UUID()
        pairingGeneration = generation
        defer { if pairingGeneration == generation { isPairing = false } }
        var name = UIDevice.current.name
        while name.utf8.count > 128 { name.removeLast() }
        if name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { name = "iOS" }
        let targetID = pairingTargetID
        var ready = try await PairingClient().exchange(link: link, name: name)
        try Task.checkCancellation()
        guard pairingGeneration == generation else { throw CancellationError() }
        if let target = savedLinks.first(where: { $0.id == targetID }) {
            ready = target.adoptingRescan(ready)
        }
        pairingTargetID = nil
        return connect(link: ready, surface: surface)
    }

    func cancelPairing() {
        pairingGeneration = UUID()
        pairingTargetID = nil
        isPairing = false
    }

    /// Erase disk credentials but keep the mounted session until the user rescans.
    func revoke(origin: String) {
        for index in savedLinks.indices where savedLinks[index].originURL.absoluteString == origin {
            var link = savedLinks[index]
            link.bearer = nil
            link.accessURL = link.originURL
            link.pairCode = nil
            link.isRevoked = true
            savedLinks[index] = link
            KeychainStore.deleteBearer(account: link.id.uuidString)
        }
        persist()
    }

    /// Returns the saved record the session dials. A rescan of a host that moved
    /// to a new origin folds into the existing record, so this is not always the
    /// link that was passed in.
    @discardableResult
    func connect(link: RemoteLink, surface: WorkspaceSurface = .chat) -> RemoteLink {
        let stored = remember(link)
        guard !stored.isRevoked else {
            requestRePairing(hostID: stored.id)
            return stored
        }
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
    func requestRePairing(hostID: UUID? = nil) {
        pairingTargetID = hostID ?? activeLink?.id
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
        let stored = saved.first { $0.id == link.id || $0.refersToSameHost(as: link) }?.adoptingRescan(link) ?? link
        let remaining = saved.filter { candidate in
            candidate.id != stored.id
                && !candidate.refersToSameHost(as: stored)
                && candidate.accessURL.absoluteString != stored.accessURL.absoluteString
        }
        return (stored, [stored] + remaining)
    }

    private func persist() {
        let records = savedLinks.map { link in
            if !link.isRevoked, let token = link.pairingToken, !token.isEmpty {
                KeychainStore.saveBearer(token, account: link.id.uuidString)
            } else {
                KeychainStore.deleteBearer(account: link.id.uuidString)
            }
            return SavedLinkRecord(
                id: link.id,
                title: link.title,
                origin: link.savedOrigin,
                createdAt: link.createdAt,
                deviceId: link.deviceId, deviceName: link.deviceName,
                scopes: link.scopes, isRevoked: link.isRevoked
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
                let bearer = KeychainStore.loadBearer(account: record.id.uuidString)
                var link = RemoteLink(
                    id: record.id, title: record.title, accessURL: origin,
                    bearer: bearer, createdAt: record.createdAt
                )
                link.deviceId = record.deviceId
                link.deviceName = record.deviceName
                link.scopes = record.scopes
                link.isRevoked = record.isRevoked == true || bearer?.isEmpty != false
                return link
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
                createdAt: $0.createdAt,
                deviceId: $0.deviceId, deviceName: $0.deviceName,
                scopes: $0.scopes, isRevoked: $0.isRevoked
            )
        }
    }
}
