import SwiftUI
import UIKit

/// Field Console palette (design-spec.md section 2). Warm graphite, one lichen
/// accent for actions, cyan `live` for connection and working state.
///
/// The original names (`muted`, `lamp`, `wordmark`, `display`, `radius`) stay
/// as aliases so existing views keep compiling while they migrate.
enum SeTheme {
    static let canvas = Color(lightHex: 0xF7F5EF, darkHex: 0x202320)
    static let surface = Color(lightHex: 0xFFFEFA, darkHex: 0x2A2E29)
    static let elevated = Color(lightHex: 0xECEEE5, darkHex: 0x343A31)
    static let ink = Color(lightHex: 0x20271F, darkHex: 0xF1EDDF)
    static let secondaryInk = Color(lightHex: 0x596451, darkHex: 0xB0B5A9)
    static let tertiaryInk = Color(lightHex: 0x66715F, darkHex: 0xAAB2A0)
    static let stroke = Color(lightHex: 0xD5D9CD, darkHex: 0x41473E)
    static let accent = Color(lightHex: 0x4E6431, darkHex: 0xBDCBA1)
    static let onAccent = Color(lightHex: 0xFFFFFF, darkHex: 0x20251B)
    static let accentSubtle = Color(lightHex: 0xE6EDD9, darkHex: 0x343E2A)
    static let live = Color(lightHex: 0x246578, darkHex: 0x91D4D5)
    static let liveSubtle = Color(lightHex: 0xE1EFF2, darkHex: 0x253F43)
    static let warning = Color(lightHex: 0x8B4B0B, darkHex: 0xE9BB75)
    static let warningSubtle = Color(lightHex: 0xFFF0D9, darkHex: 0x423523)
    static let success = Color(lightHex: 0x3D673B, darkHex: 0xB2D4A6)
    static let danger = Color(lightHex: 0xA52F36, darkHex: 0xEFA595)
    static let dangerSubtle = Color(lightHex: 0xFBE5E5, darkHex: 0x442D2D)
    static let codeCanvas = Color(lightHex: 0xF1F3EB, darkHex: 0x191E19)
    static let diffAdded = Color(lightHex: 0xDDEED8, darkHex: 0x253B29)
    static let diffAddedInk = Color(lightHex: 0x284F2E, darkHex: 0xB8DEBC)
    static let diffRemoved = Color(lightHex: 0xF9E2DF, darkHex: 0x412B29)
    static let diffRemovedInk = Color(lightHex: 0x8E302E, darkHex: 0xF0BAB4)

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
