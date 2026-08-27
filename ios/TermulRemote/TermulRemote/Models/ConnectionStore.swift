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
    private static let storageKey = "termul.remote.savedLinks"

    var savedLinks: [RemoteLink] = []
    var activeLink: RemoteLink?
    var surface: WorkspaceSurface = .chat
    var errorMessage: String?
    var isScanning = false

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

    func connect(link: RemoteLink, surface: WorkspaceSurface = .chat) {
        remember(link)
        errorMessage = nil
        self.surface = surface
        activeLink = link
    }

    func disconnect() {
        activeLink = nil
        surface = .chat
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
        _ = AppLaunchURL.take()
        HostLog.session.info("Opening pairing link")
        connect(to: url.absoluteString)
    }

    func consumePendingLaunchURL() {
        guard let url = AppLaunchURL.take() else { return }
        openIncomingURL(url)
    }

    func dismissError() {
        errorMessage = nil
    }

    private func remember(_ link: RemoteLink) {
        savedLinks.removeAll {
            $0.id == link.id
                || $0.originHost == link.originHost
                || $0.accessURL.absoluteString == link.accessURL.absoluteString
        }
        savedLinks.insert(link, at: 0)
        persist()
    }

    private func persist() {
        let records = savedLinks.map { link in
            if let token = link.pairingToken, !token.isEmpty {
                KeychainStore.saveBearer(token, account: link.id.uuidString)
            }
            return SavedLinkRecord(
                id: link.id,
                title: link.title,
                origin: link.originURL.absoluteString,
                createdAt: link.createdAt
            )
        }
        guard let data = try? JSONEncoder().encode(records) else { return }
        UserDefaults.standard.set(data, forKey: Self.storageKey)
    }

    private static func load() -> [RemoteLink] {
        guard let data = UserDefaults.standard.data(forKey: storageKey) else { return [] }
        if let records = try? JSONDecoder().decode([SavedLinkRecord].self, from: data) {
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
                origin: $0.originURL.absoluteString,
                createdAt: $0.createdAt
            )
        }
    }
}
