import SwiftUI
import UIKit

/// Warm graphite companion chrome: one lichen accent, cyan lamp for live work.
enum TermulTheme {
    static let canvas = Color(
        light: Color(red: 0.967, green: 0.961, blue: 0.938),
        dark: Color(red: 0.054, green: 0.059, blue: 0.036)
    )
    static let surface = Color(
        light: Color(red: 0.997, green: 0.994, blue: 0.982),
        dark: Color(red: 0.091, green: 0.093, blue: 0.069)
    )
    static let ink = Color(
        light: Color(red: 0.077, green: 0.068, blue: 0.045),
        dark: Color(red: 0.897, green: 0.884, blue: 0.832)
    )
    static let muted = Color(
        light: Color(red: 0.379, green: 0.366, blue: 0.333),
        dark: Color(red: 0.584, green: 0.574, blue: 0.534)
    )
    static let stroke = Color(
        light: Color(red: 0.854, green: 0.844, blue: 0.816),
        dark: Color(red: 0.199, green: 0.202, blue: 0.171)
    )
    static let accent = Color(
        light: Color(red: 0.370, green: 0.443, blue: 0.235),
        dark: Color(red: 0.546, green: 0.616, blue: 0.430)
    )
    static let lamp = Color(
        light: Color(red: 0.203, green: 0.448, blue: 0.521),
        dark: Color(red: 0.486, green: 0.652, blue: 0.706)
    )
    static let wordmark: Font = .system(.largeTitle, design: .serif).bold()
    static let display: Font = .system(.title2, design: .serif)
    static let radius: CGFloat = 14
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
        self.init(uiColor: UIColor { traits in
            traits.userInterfaceStyle == .dark
                ? UIColor(dark)
                : UIColor(light)
        })
    }
}
