import SwiftUI

/// Theme mode for the Nocturne prototype
/// (`.qin-code/design/seremote-ios27-v2/index.html`).
///
/// The prototype switches day/night inside the app, so these colors are fixed
/// values per mode rather than `Color(lightHex:darkHex:)` dynamic providers.
/// Screens read the mode from the environment and pick `mode.palette`.
enum SeThemeMode: String, CaseIterable, Identifiable, Sendable {
    case day
    case night

    var id: String { rawValue }

    var title: String {
        switch self {
        case .day: String(localized: "Day")
        case .night: String(localized: "Night")
        }
    }

    var colorScheme: ColorScheme {
        switch self {
        case .day: .light
        case .night: .dark
        }
    }

    var palette: SePalette {
        switch self {
        case .day: .day
        case .night: .night
        }
    }
}

/// Semantic colors of the Nocturne system.
///
/// Chrome stays neutral in both modes; color is reserved for state and the one
/// primary action. The terminal gets a readable pair per mode: a dark canvas
/// in Night and a light canvas in Day, matching the surface it sits inside.
struct SePalette: Sendable {
    let canvas: Color
    let surface: Color
    let card: Color
    let cardAlt: Color
    let ink: Color
    let inkSecondary: Color
    let inkTertiary: Color
    let stroke: Color
    let strokeStrong: Color
    let accent: Color
    let onAccent: Color
    let accentSubtle: Color
    let live: Color
    let liveSubtle: Color
    let warning: Color
    let warningSubtle: Color
    let danger: Color
    let dangerSubtle: Color
    let codeCanvas: Color
    let codeInk: Color
    let terminalCanvas: Color
    let terminalInk: Color

    static let night = SePalette(
        canvas: Color(hex: 0x0B0D12),
        surface: Color(hex: 0x0B0D12),
        card: Color(hex: 0x12161F),
        cardAlt: Color(hex: 0x181D28),
        ink: Color(hex: 0xF2F0E8),
        inkSecondary: Color(hex: 0xA9AEBB),
        inkTertiary: Color(hex: 0x6F7585),
        stroke: Color.white.opacity(0.085),
        strokeStrong: Color.white.opacity(0.16),
        accent: Color(hex: 0x98A6FF),
        onAccent: Color(hex: 0x0B0D12),
        accentSubtle: Color(hex: 0x8B9BFF).opacity(0.14),
        live: Color(hex: 0x4FD8AE),
        liveSubtle: Color(hex: 0x4FD8AE).opacity(0.13),
        warning: Color(hex: 0xF2B36B),
        warningSubtle: Color(hex: 0xF2B36B).opacity(0.12),
        danger: Color(hex: 0xFF8F84),
        dangerSubtle: Color(hex: 0xFF8F84).opacity(0.12),
        codeCanvas: Color(hex: 0x05070A),
        codeInk: Color(hex: 0xE9E7DC),
        terminalCanvas: Color(hex: 0x05070A),
        terminalInk: Color(hex: 0xE9E7DC)
    )

    static let day = SePalette(
        canvas: Color(hex: 0xF1F4F8),
        surface: Color(hex: 0xF1F4F8),
        card: Color(hex: 0xFFFFFF),
        cardAlt: Color(hex: 0xE7ECF3),
        ink: Color(hex: 0x10151C),
        inkSecondary: Color(hex: 0x4B5666),
        inkTertiary: Color(hex: 0x7A8697),
        stroke: Color.black.opacity(0.10),
        strokeStrong: Color.black.opacity(0.20),
        accent: Color(hex: 0x4353EE),
        onAccent: Color(hex: 0xFFFFFF),
        accentSubtle: Color(hex: 0x4353EE).opacity(0.10),
        live: Color(hex: 0x0C7A5A),
        liveSubtle: Color(hex: 0x0C7A5A).opacity(0.10),
        warning: Color(hex: 0x9A5B08),
        warningSubtle: Color(hex: 0x9A5B08).opacity(0.10),
        danger: Color(hex: 0xC0392B),
        dangerSubtle: Color(hex: 0xC0392B).opacity(0.10),
        codeCanvas: Color(hex: 0xF1F4F8),
        codeInk: Color(hex: 0x10151C),
        // The terminal is a surface inside the screen, so it needs a light
        // variant too: readable ink on a near-white canvas in Day, the
        // original dark canvas in Night.
        terminalCanvas: Color(hex: 0xF7F8FB),
        terminalInk: Color(hex: 0x1B2028)
    )

    static func palette(for mode: SeThemeMode) -> SePalette {
        mode.palette
    }
}

// MARK: - Resolution and persistence

extension SeThemeMode {
    /// The mode a subtree renders in. The effective color scheme already folds
    /// in the persisted `AppAppearance` (`preferredColorScheme`) or, for
    /// `.system`, the device setting, so it is the single source of truth.
    static func resolved(colorScheme: ColorScheme) -> SeThemeMode {
        colorScheme == .dark ? .night : .day
    }

    /// The persisted appearance that pins this mode. The day/night toggle
    /// writes an explicit choice; picking "system" stays in Settings.
    var appearance: AppAppearance {
        switch self {
        case .day: .light
        case .night: .dark
        }
    }

    var toggled: SeThemeMode {
        switch self {
        case .day: .night
        case .night: .day
        }
    }
}

/// Injects the resolved Nocturne mode for everything below the app root.
private struct SeThemeModeResolver: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        content.seThemeMode(.resolved(colorScheme: colorScheme))
    }
}

extension View {
    /// Resolves `seThemeMode` from the effective color scheme. Apply below
    /// `preferredColorScheme` so the persisted appearance is honored.
    func seResolvedThemeMode() -> some View {
        modifier(SeThemeModeResolver())
    }
}

// MARK: - Environment

private struct SeThemeModeKey: EnvironmentKey {
    static let defaultValue: SeThemeMode = .night
}

extension EnvironmentValues {
    var seThemeMode: SeThemeMode {
        get { self[SeThemeModeKey.self] }
        set { self[SeThemeModeKey.self] = newValue }
    }

    var sePalette: SePalette { seThemeMode.palette }
}

extension View {
    /// Applies the Nocturne theme to this subtree.
    func seThemeMode(_ mode: SeThemeMode) -> some View {
        environment(\.seThemeMode, mode)
    }
}

// MARK: - Type

/// Base sizes from the prototype. Views that need Dynamic Type should wrap
/// these in `@ScaledMetric(relativeTo:)` and call `font(_:weight:mono:)`.
///
/// Every size the prototype uses in-app is represented here (`index.html`);
/// the 62pt marketing masthead is page chrome outside the app and is not part
/// of the scale. Sizes are placeholders for Dynamic Type, never fixed layout.
enum SeNocturneType {
    // Display and titles
    static let screenTitleSize: CGFloat = 33
    /// Mobile section display (prototype `h2` at 24px).
    static let compactDisplaySize: CGFloat = 24
    /// iPad portrait shell title (prototype `.pad.pt .padtitle` at 30px).
    static let padDisplaySize: CGFloat = 30
    static let statSize: CGFloat = 26
    static let sheetTitleSize: CGFloat = 19
    static let headlineSize: CGFloat = 17
    static let sectionTitleSize: CGFloat = 15
    /// Navbar title / rail host name (`13.5–14px` in the prototype).
    static let navbarTitleSize: CGFloat = 14
    static let cardTitleSize: CGFloat = 13.5
    static let buttonTitleSize: CGFloat = 13

    // Body
    static let rowTitleSize: CGFloat = 12.5
    static let bodySize: CGFloat = 12.5
    static let bodyCompactSize: CGFloat = 12
    static let secondarySize: CGFloat = 11.5
    static let tertiarySize: CGFloat = 11
    static let labelSize: CGFloat = 10.5

    // Mono labels and micro type
    static let eyebrowSize: CGFloat = 10
    static let monoSize: CGFloat = 10
    static let monoLabelSize: CGFloat = 9.5
    static let monoSmallSize: CGFloat = 9
    static let monoEyebrowSize: CGFloat = 8.5
    static let microSize: CGFloat = 8
    /// Collapsed-rail counters (prototype `7px`).
    static let badgeSize: CGFloat = 7
    /// Console / terminal meta in mono (`12px`).
    static let monoBodySize: CGFloat = 12

    /// The in-app scale, smallest to largest, asserted by `DesignSystemTests`.
    static let scale: [CGFloat] = [
        7, 8, 8.5, 9, 9.5, 10, 10.5, 11, 11.5, 12, 12.5,
        13, 13.5, 14, 15, 17, 19, 24, 26, 30, 33,
    ]

    static func font(
        _ size: CGFloat,
        weight: Font.Weight = .regular,
        mono: Bool = false
    ) -> Font {
        .system(size: size, weight: weight, design: mono ? .monospaced : .default)
    }

    static let screenTitle = font(screenTitleSize, weight: .semibold)
    static let compactDisplay = font(compactDisplaySize, weight: .semibold)
    static let padDisplay = font(padDisplaySize, weight: .semibold)
    static let sheetTitle = font(sheetTitleSize, weight: .semibold)
    static let headline = font(headlineSize, weight: .semibold)
    static let sectionTitle = font(sectionTitleSize, weight: .semibold)
    static let navbarTitle = font(navbarTitleSize, weight: .semibold)
    static let cardTitle = font(cardTitleSize, weight: .semibold)
    static let buttonTitle = font(buttonTitleSize, weight: .semibold)
    static let rowTitle = font(rowTitleSize, weight: .semibold)
    static let body = font(bodySize)
    static let bodyCompact = font(bodyCompactSize)
    static let secondary = font(secondarySize)
    static let tertiary = font(tertiarySize)
    static let label = font(labelSize, weight: .medium)
    static let eyebrow = font(eyebrowSize, mono: true)
    static let mono = font(monoSize, mono: true)
    static let monoLabel = font(monoLabelSize, mono: true)
    static let monoSmall = font(monoSmallSize, mono: true)
    static let monoEyebrow = font(monoEyebrowSize, mono: true)
    static let monoBody = font(monoBodySize, mono: true)
    static let micro = font(microSize)
    static let badge = font(badgeSize, mono: true)
    static let stat = font(statSize, weight: .semibold)
}

// MARK: - Shape and motion

enum SeNocturneRadius {
    /// Segmented controls, small buttons, chips.
    static let control: CGFloat = 9
    /// Cards, list rows, tool tiles.
    static let card: CGFloat = 16
    /// Bottom sheets.
    static let sheet: CGFloat = 24
    /// Full-width strips such as the needs bar.
    static let strip: CGFloat = 12
    /// Pills and counters.
    static let pill: CGFloat = 999
}

enum SeNocturneMotion {
    /// Color and press feedback.
    static let quick: Animation = .smooth(duration: 0.18)
    /// Segmented indicators, sidebar width, mode changes.
    static let panel: Animation = .smooth(duration: 0.30)
    /// Bottom sheets and drawers.
    static let sheet: Animation = .spring(response: 0.42, dampingFraction: 0.92)

    static func resolved(_ animation: Animation, reduceMotion: Bool) -> Animation? {
        reduceMotion ? nil : animation
    }
}
