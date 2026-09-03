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
        .init(id: "escape", label: "Esc", bytes: "\u{1b}", accessibilityLabel: "Escape"),
        .init(id: "tab", label: "Tab", bytes: "\t", accessibilityLabel: "Tab"),
        .init(id: "enter", label: "Enter", bytes: "\r", accessibilityLabel: "Enter"),
        .init(id: "backspace", label: "⌫", bytes: "\u{7f}", accessibilityLabel: "Backspace", repeatable: true),
        .init(id: "arrowUp", label: "↑", bytes: "\u{1b}[A", accessibilityLabel: "Arrow up", repeatable: true),
        .init(id: "arrowDown", label: "↓", bytes: "\u{1b}[B", accessibilityLabel: "Arrow down", repeatable: true),
        .init(id: "arrowLeft", label: "←", bytes: "\u{1b}[D", accessibilityLabel: "Arrow left", repeatable: true),
        .init(id: "arrowRight", label: "→", bytes: "\u{1b}[C", accessibilityLabel: "Arrow right", repeatable: true),
        .init(id: "ctrlC", label: "Ctrl+C", bytes: "\u{3}", accessibilityLabel: "Interrupt"),
        .init(id: "ctrlD", label: "Ctrl+D", bytes: "\u{4}", accessibilityLabel: "Send EOF"),
        .init(id: "ctrlZ", label: "Ctrl+Z", bytes: "\u{1a}", accessibilityLabel: "Suspend"),
        .init(id: "ctrlL", label: "Ctrl+L", bytes: "\u{0c}", accessibilityLabel: "Clear screen"),
        .init(id: "ctrlR", label: "Ctrl+R", bytes: "\u{12}", accessibilityLabel: "Reverse search"),
        .init(id: "ctrlA", label: "Ctrl+A", bytes: "\u{1}", accessibilityLabel: "Start of line"),
        .init(id: "ctrlE", label: "Ctrl+E", bytes: "\u{5}", accessibilityLabel: "End of line"),
        .init(id: "ctrlW", label: "Ctrl+W", bytes: "\u{17}", accessibilityLabel: "Delete word"),
        .init(id: "ctrlU", label: "Ctrl+U", bytes: "\u{15}", accessibilityLabel: "Clear line")
    ]
}
