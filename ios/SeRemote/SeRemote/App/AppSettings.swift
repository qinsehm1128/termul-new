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
    /// Follow the phone's Light/Dark setting. The shipped default.
    case system
    case light
    case dark

    /// The appearance a fresh install starts with. Storing no preference means
    /// "follow the phone", so the app must not pin a color scheme by default.
    static let `default`: AppAppearance = .system

    var id: String { rawValue }

    /// `nil` means no override: SwiftUI then follows the device's
    /// `UIUserInterfaceStyle` and updates live when the phone appearance changes.
    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }

    /// The Nocturne mode this appearance resolves to, or `nil` when the device
    /// decides. The Day/Night labels stay for explicit Light/Dark choices.
    var themeMode: SeThemeMode? {
        switch self {
        case .system: nil
        case .light: .day
        case .dark: .night
        }
    }

    var title: String {
        switch self {
        case .system: String(localized: "Follow system")
        case .light: String(localized: "Day")
        case .dark: String(localized: "Night")
        }
    }
}

@MainActor
@Observable
final class AppSettings {
    private static let languageKey = "se.app.language"
    private static let appearanceKey = "se.app.appearance"
    private static let notificationsKey = "se.app.notifications"
    private static let hapticsKey = "se.app.haptics"

    /// Pre-rename keys. Read-only: the first launch after the rename falls back
    /// to them so a chosen language and appearance survive. Never written again
    /// and never removed — the old values stay on disk for a downgrade.
    private static let legacyLanguageKey = "termul.app.language"
    private static let legacyAppearanceKey = "termul.app.appearance"

    private let defaults: UserDefaults

    var language: AppLanguage {
        didSet { defaults.set(language.rawValue, forKey: Self.languageKey) }
    }

    var appearance: AppAppearance {
        didSet { defaults.set(appearance.rawValue, forKey: Self.appearanceKey) }
    }

    /// Local-notification opt-in. Off by default; the settings toggle asks
    /// the system for authorization the first time it flips on.
    var notificationsEnabled: Bool {
        didSet { defaults.set(notificationsEnabled, forKey: Self.notificationsKey) }
    }

    /// Haptic feedback opt-out. On by default; injected as `seHapticsEnabled`.
    var hapticsEnabled: Bool {
        didSet { defaults.set(hapticsEnabled, forKey: Self.hapticsKey) }
    }

    /// `defaults` is injectable so tests can resolve the defaults without
    /// touching the real app domain.
    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        language = AppLanguage(rawValue: Self.stored(defaults, Self.languageKey, legacy: Self.legacyLanguageKey) ?? "") ?? .system
        // No stored value means the user never chose: follow the phone.
        // An explicit Day/Night choice from an older build still wins.
        appearance = AppAppearance(rawValue: Self.stored(defaults, Self.appearanceKey, legacy: Self.legacyAppearanceKey) ?? "") ?? .default
        notificationsEnabled = defaults.bool(forKey: Self.notificationsKey)
        hapticsEnabled = defaults.object(forKey: Self.hapticsKey) as? Bool ?? true
    }

    /// The current key if it has ever been written, otherwise the pre-rename one.
    private static func stored(_ defaults: UserDefaults, _ key: String, legacy legacyKey: String) -> String? {
        defaults.string(forKey: key) ?? defaults.string(forKey: legacyKey)
    }

    var locale: Locale {
        language.locale ?? .autoupdatingCurrent
    }
}
