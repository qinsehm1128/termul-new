import Foundation

struct TerminalAccessoryKey: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let bytes: String
    let accessibilityLabel: String
    var repeatable: Bool = false
}

enum TerminalAccessoryCatalog {
    static let keys: [TerminalAccessoryKey] = [
        .init(id: "escape", label: String(localized: "Esc"), bytes: "\u{1b}", accessibilityLabel: String(localized: "Escape")),
        .init(id: "tab", label: String(localized: "Tab"), bytes: "\t", accessibilityLabel: String(localized: "Tab")),
        .init(id: "enter", label: String(localized: "Enter"), bytes: "\r", accessibilityLabel: String(localized: "Enter")),
        .init(id: "backspace", label: String(localized: "⌫"), bytes: "\u{7f}", accessibilityLabel: String(localized: "Backspace"), repeatable: true),
        .init(id: "arrowUp", label: String(localized: "↑"), bytes: "\u{1b}[A", accessibilityLabel: String(localized: "Arrow up"), repeatable: true),
        .init(id: "arrowDown", label: String(localized: "↓"), bytes: "\u{1b}[B", accessibilityLabel: String(localized: "Arrow down"), repeatable: true),
        .init(id: "arrowLeft", label: String(localized: "←"), bytes: "\u{1b}[D", accessibilityLabel: String(localized: "Arrow left"), repeatable: true),
        .init(id: "arrowRight", label: String(localized: "→"), bytes: "\u{1b}[C", accessibilityLabel: String(localized: "Arrow right"), repeatable: true),
        .init(id: "pageUp", label: String(localized: "PgUp"), bytes: "\u{1b}[5~", accessibilityLabel: String(localized: "Page up"), repeatable: true),
        .init(id: "pageDown", label: String(localized: "PgDn"), bytes: "\u{1b}[6~", accessibilityLabel: String(localized: "Page down"), repeatable: true),
        .init(id: "ctrlC", label: String(localized: "Ctrl+C"), bytes: "\u{3}", accessibilityLabel: String(localized: "Interrupt")),
        .init(id: "ctrlD", label: String(localized: "Ctrl+D"), bytes: "\u{4}", accessibilityLabel: String(localized: "Send EOF")),
        .init(id: "ctrlZ", label: String(localized: "Ctrl+Z"), bytes: "\u{1a}", accessibilityLabel: String(localized: "Suspend")),
        .init(id: "ctrlL", label: String(localized: "Ctrl+L"), bytes: "\u{0c}", accessibilityLabel: String(localized: "Clear screen")),
        .init(id: "ctrlR", label: String(localized: "Ctrl+R"), bytes: "\u{12}", accessibilityLabel: String(localized: "Reverse search")),
        .init(id: "ctrlA", label: String(localized: "Ctrl+A"), bytes: "\u{1}", accessibilityLabel: String(localized: "Start of line")),
        .init(id: "ctrlE", label: String(localized: "Ctrl+E"), bytes: "\u{5}", accessibilityLabel: String(localized: "End of line")),
        .init(id: "ctrlW", label: String(localized: "Ctrl+W"), bytes: "\u{17}", accessibilityLabel: String(localized: "Delete word")),
        .init(id: "ctrlU", label: String(localized: "Ctrl+U"), bytes: "\u{15}", accessibilityLabel: String(localized: "Clear line"))
    ]
}

/// A local keyboard modifier only. It never owns or reconnects the PTY.
struct TerminalControlModifier: Equatable, Sendable {
    private(set) var isLatched = false
    private(set) var isHeld = false
    private var usedDuringPress = false

    var isActive: Bool { isLatched || isHeld }

    mutating func beginPress() {
        guard !isHeld else { return }
        isHeld = true
        usedDuringPress = false
    }

    /// A short tap latches the next control-compatible key. A hold releases
    /// without latching, including when another finger has already sent input.
    mutating func endPress(isTap: Bool) {
        isHeld = false
        if isTap && !usedDuringPress { isLatched.toggle() }
        usedDuringPress = false
    }

    mutating func toggleLatch() {
        isLatched.toggle()
    }

    mutating func releaseHold() {
        isHeld = false
        usedDuringPress = false
    }

    mutating func apply(to input: String) -> String {
        guard isActive, let control = Self.controlSequence(for: input) else { return input }
        if isHeld { usedDuringPress = true }
        isLatched = false
        return control
    }

    /// Do not rewrite paste, escape sequences, existing control bytes, or IME
    /// text. Ctrl is only applied to a single compatible ASCII key.
    static func controlSequence(for input: String) -> String? {
        guard input.unicodeScalars.count == 1, let scalar = input.unicodeScalars.first else { return nil }
        let value = scalar.value
        let code: UInt32
        switch value {
        case 65 ... 90, 97 ... 122: code = value & 0x1f
        case 64, 91 ... 95: code = value & 0x1f
        case 32: code = 0
        case 63: code = 127
        default: return nil
        }
        guard let result = UnicodeScalar(code) else { return nil }
        return String(result)
    }
}
