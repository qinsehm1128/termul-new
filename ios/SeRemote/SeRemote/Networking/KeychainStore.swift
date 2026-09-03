import Foundation
import Security

enum KeychainStore {
    private static let service = "com.se-manager.remote.pairing"

    /// Pre-rename service. Read-only: `loadBearer` falls back to it so a device
    /// paired before the rename does not have to re-scan its QR. Nothing writes
    /// it and nothing removes it.
    private static let legacyService = "com.termul.remote.pairing"

    static func saveBearer(_ bearer: String, account: String) {
        guard !account.isEmpty, !bearer.isEmpty else { return }
        let payload = Data(bearer.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData as String] = payload
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        if status != errSecSuccess {
            HostLog.session.error("Pairing secret persist failed status=\(status, privacy: .public)")
        }
    }

    static func loadBearer(account: String) -> String? {
        guard !account.isEmpty else { return nil }
        return bearer(account: account, in: service) ?? bearer(account: account, in: legacyService)
    }

    private static func bearer(account: String, in service: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data else { return nil }
        let bearer = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
        return bearer?.isEmpty == false ? bearer : nil
    }

    /// Removes the secret this build wrote.
    ///
    /// It deliberately does not touch `legacyService`: the rename copies, it
    /// never deletes (FORBID-05). A device paired before the rename therefore
    /// keeps an orphaned pre-rename item after "forget", unreachable because
    /// `forget` also drops the account id from the saved list.
    static func deleteBearer(account: String) {
        guard !account.isEmpty else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        SecItemDelete(query as CFDictionary)
    }
}
