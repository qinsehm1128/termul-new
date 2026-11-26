import SwiftUI
import UIKit

/// Nocturne palette (`.qin-code/design/seremote-ios27-v2`). Neutral chrome in
/// both modes; color is reserved for state and the one primary action. The
/// values mirror `SePalette.day` / `SePalette.night` so screens that still read
/// `SeTheme` follow the same day/night switch as the Nocturne screens.
///
/// The original names (`muted`, `lamp`, `wordmark`, `display`, `radius`) stay
/// as aliases so existing views keep compiling while they migrate.
enum SeTheme {
    static let canvas = Color(lightHex: 0xFAF7F5, darkHex: 0x181523)
    static let surface = Color(lightHex: 0xFFFFFF, darkHex: 0x201C2E)
    static let elevated = Color(lightHex: 0xF4EEF7, darkHex: 0x29243B)
    static let ink = Color(lightHex: 0x251E38, darkHex: 0xECE8FA)
    static let secondaryInk = Color(lightHex: 0x62587A, darkHex: 0xB0A9C9)
    static let tertiaryInk = Color(lightHex: 0x8A7FA3, darkHex: 0x7E7599)
    static let stroke = Color(lightHex: 0xE3DBE9, darkHex: 0x352F4B)
    static let accent = Color(lightHex: 0x7D54B5, darkHex: 0xC4A7E7)
    static let onAccent = Color(lightHex: 0xFFFFFF, darkHex: 0x181523)
    static let accentSubtle = Color(lightHex: 0xEFE6FA, darkHex: 0x2B2142)
    static let live = Color(lightHex: 0x2D8377, darkHex: 0x8BC9B8)
    static let liveSubtle = Color(lightHex: 0xDFF2EE, darkHex: 0x1D2E2F)
    static let warning = Color(lightHex: 0xB87924, darkHex: 0xF6C177)
    static let warningSubtle = Color(lightHex: 0xF9EEDB, darkHex: 0x342A22)
    static let success = Color(lightHex: 0x2D8377, darkHex: 0x8BC9B8)
    static let danger = Color(lightHex: 0xB84266, darkHex: 0xEB6F92)
    static let dangerSubtle = Color(lightHex: 0xF9E4EC, darkHex: 0x36202F)
    static let codeCanvas = Color(lightHex: 0xF4EEF7, darkHex: 0x12101A)
    static let diffAdded = Color(lightHex: 0xDFF2EE, darkHex: 0x1D2E2F)
    static let diffAddedInk = Color(lightHex: 0x1E655B, darkHex: 0xA4DACB)
    static let diffRemoved = Color(lightHex: 0xF9E4EC, darkHex: 0x36202F)
    static let diffRemovedInk = Color(lightHex: 0x9A2E52, darkHex: 0xF28BA8)

    // Legacy aliases.
    static let muted = secondaryInk
    static let lamp = live
    static let wordmark: Font = SeFont.identity
    static let display: Font = .system(.title2, design: .serif)
    static let radius: CGFloat = SeRadius.content
}

enum HostRowStatus: Equatable {
    case need
    case working
    case idle

    var title: String {
        switch self {
        case .need: String(localized: "Need you")
        case .working: String(localized: "Working")
        case .idle: String(localized: "Idle")
        }
    }
}

extension Color {
    init(light: Color, dark: Color) {
        // UIKit resolves dynamic providers on arbitrary threads (SwiftUI's
        // AsyncRenderer among them). Convert here, on the constructing thread,
        // and let the provider capture plain UIColors only — converting a
        // SwiftUI Color inside the provider trips Swift 6's executor
        // isolation assert when resolution lands off-main
        // (dispatch_assert_queue_fail crash).
        let lightUI = UIColor(light)
        let darkUI = UIColor(dark)
        self.init(uiColor: UIColor { @Sendable traits in
            traits.userInterfaceStyle == .dark ? darkUI : lightUI
        })
    }

    init(lightHex: UInt32, darkHex: UInt32) {
        self.init(light: Color(hex: lightHex), dark: Color(hex: darkHex))
    }

    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}
