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
    private static let languageKey = "termul.app.language"
    private static let appearanceKey = "termul.app.appearance"

    var language: AppLanguage {
        didSet { UserDefaults.standard.set(language.rawValue, forKey: Self.languageKey) }
    }

    var appearance: AppAppearance {
        didSet { UserDefaults.standard.set(appearance.rawValue, forKey: Self.appearanceKey) }
    }

    init() {
        language = AppLanguage(rawValue: UserDefaults.standard.string(forKey: Self.languageKey) ?? "") ?? .system
        appearance = AppAppearance(rawValue: UserDefaults.standard.string(forKey: Self.appearanceKey) ?? "") ?? .dark
    }

    var locale: Locale {
        language.locale ?? .autoupdatingCurrent
    }
}
