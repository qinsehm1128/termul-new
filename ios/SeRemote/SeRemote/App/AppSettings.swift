import Foundation
import Observation
import SwiftUI

enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english
    case simplifiedChinese

    var id: String { rawValue }

    var locale: Locale? {
        switch self {
        case .system: nil
        case .english: Locale(identifier: "en")
        case .simplifiedChinese: Locale(identifier: "zh-Hans")
        }
    }
}

enum AppAppearance: String, CaseIterable, Identifiable {
    case system
    case dark
    case light

    var id: String { rawValue }

    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .dark: .dark
        case .light: .light
        }
    }
}

@MainActor
@Observable
final class AppSettings {
    private static let languageKey = "se.app.language"
    private static let appearanceKey = "se.app.appearance"
    private static let notificationsKey = "se.app.notifications"

    /// Pre-rename keys. Read-only: the first launch after the rename falls back
    /// to them so a chosen language and appearance survive. Never written again
    /// and never removed — the old values stay on disk for a downgrade.
    private static let legacyLanguageKey = "termul.app.language"
    private static let legacyAppearanceKey = "termul.app.appearance"

    var language: AppLanguage {
        didSet { UserDefaults.standard.set(language.rawValue, forKey: Self.languageKey) }
    }

    var appearance: AppAppearance {
        didSet { UserDefaults.standard.set(appearance.rawValue, forKey: Self.appearanceKey) }
    }

    /// Local-notification opt-in. Off by default; the settings toggle asks
    /// the system for authorization the first time it flips on.
    var notificationsEnabled: Bool {
        didSet { UserDefaults.standard.set(notificationsEnabled, forKey: Self.notificationsKey) }
    }

    init() {
        language = AppLanguage(rawValue: Self.stored(Self.languageKey, legacy: Self.legacyLanguageKey) ?? "") ?? .system
        appearance = AppAppearance(rawValue: Self.stored(Self.appearanceKey, legacy: Self.legacyAppearanceKey) ?? "") ?? .dark
        notificationsEnabled = UserDefaults.standard.bool(forKey: Self.notificationsKey)
    }

    /// The current key if it has ever been written, otherwise the pre-rename one.
    private static func stored(_ key: String, legacy legacyKey: String) -> String? {
        UserDefaults.standard.string(forKey: key) ?? UserDefaults.standard.string(forKey: legacyKey)
    }

    var locale: Locale {
        language.locale ?? .autoupdatingCurrent
    }
}
