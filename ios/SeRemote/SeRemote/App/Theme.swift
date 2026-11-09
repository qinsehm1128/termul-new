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
    static let canvas = Color(lightHex: 0xF1F4F8, darkHex: 0x0B0D12)
    static let surface = Color(lightHex: 0xFFFFFF, darkHex: 0x12161F)
    static let elevated = Color(lightHex: 0xE7ECF3, darkHex: 0x181D28)
    static let ink = Color(lightHex: 0x10151C, darkHex: 0xF2F0E8)
    static let secondaryInk = Color(lightHex: 0x4B5666, darkHex: 0xA9AEBB)
    static let tertiaryInk = Color(lightHex: 0x5F6B7C, darkHex: 0x8F95A4)
    static let stroke = Color(lightHex: 0xD3DAE4, darkHex: 0x262B36)
    static let accent = Color(lightHex: 0x4353EE, darkHex: 0x98A6FF)
    static let onAccent = Color(lightHex: 0xFFFFFF, darkHex: 0x0B0D12)
    static let accentSubtle = Color(lightHex: 0xE3E6FD, darkHex: 0x1D2240)
    static let live = Color(lightHex: 0x0C7A5A, darkHex: 0x4FD8AE)
    static let liveSubtle = Color(lightHex: 0xDCEFE8, darkHex: 0x13302A)
    static let warning = Color(lightHex: 0x9A5B08, darkHex: 0xF2B36B)
    static let warningSubtle = Color(lightHex: 0xF5ECDF, darkHex: 0x2E2619)
    static let success = Color(lightHex: 0x0C7A5A, darkHex: 0x4FD8AE)
    static let danger = Color(lightHex: 0xC0392B, darkHex: 0xFF8F84)
    static let dangerSubtle = Color(lightHex: 0xF8E4E2, darkHex: 0x33201F)
    static let codeCanvas = Color(lightHex: 0xF1F4F8, darkHex: 0x05070A)
    static let diffAdded = Color(lightHex: 0xDCEFE8, darkHex: 0x13302A)
    static let diffAddedInk = Color(lightHex: 0x0A5A43, darkHex: 0x8FE6C8)
    static let diffRemoved = Color(lightHex: 0xF8E4E2, darkHex: 0x33201F)
    static let diffRemovedInk = Color(lightHex: 0x9A2E22, darkHex: 0xFFB4AB)

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

enum WorkspaceTab: String, CaseIterable, Identifiable {
    case chat
    case terminal
    case files

    var id: String { rawValue }

    var title: String {
        switch self {
        case .chat: String(localized: "Chat")
        case .terminal: String(localized: "Terminal")
        case .files: String(localized: "Files")
        }
    }

    var systemImage: String {
        switch self {
        case .chat: "bubble.left.and.bubble.right"
        case .terminal: "apple.terminal"
        case .files: "folder"
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
