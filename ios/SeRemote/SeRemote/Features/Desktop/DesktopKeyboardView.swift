import SwiftUI
import UIKit

/// A real UITextView supplies the system text/IME pipeline; only committed text
/// is sent. Its accessory remains available with software and hardware keyboards.
struct DesktopKeyboardBridge: UIViewRepresentable {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var theme
    @Binding var visible: Bool
    let enabled: Bool
    let onInput: (DesktopInputMessage) -> Void

    func makeUIView(context: Context) -> DesktopKeyboardTextView {
        let view = DesktopKeyboardTextView()
        view.onVisibility = { value in if visible != value { visible = value } }
        return view
    }

    func updateUIView(_ view: DesktopKeyboardTextView, context: Context) {
        view.onInput = onInput
        view.onVisibility = { value in if visible != value { visible = value } }
        view.inputEnabled = enabled
        view.wantsKeyboard = visible
        view.applyTheme(background: UIColor(theme.barSurface), foreground: UIColor(palette.ink),
                        accent: UIColor(palette.accent), night: theme.isNight)
        if enabled && visible {
            if !view.isFirstResponder {
                DispatchQueue.main.async { [weak view] in
                    guard let view, view.inputEnabled, view.wantsKeyboard, view.window != nil else { return }
                    view.becomeFirstResponder()
                }
            }
        } else {
            view.releaseModifiers()
            if view.isFirstResponder { view.resignFirstResponder() }
        }
    }

    static func dismantleUIView(_ view: DesktopKeyboardTextView, coordinator: ()) {
        view.releaseModifiers()
        view.resignFirstResponder()
    }
}

final class DesktopKeyboardTextView: UITextView, UITextViewDelegate {
    var onInput: ((DesktopInputMessage) -> Void)?
    var onVisibility: ((Bool) -> Void)?
    var inputEnabled = false
    var wantsKeyboard = false
    private var sticky = DesktopModifiers()
    private var accessoryButtons: [UIButton] = []

    func applyTheme(background: UIColor, foreground: UIColor, accent: UIColor, night: Bool) {
        inputAccessoryView?.backgroundColor = background
        overrideUserInterfaceStyle = night ? .dark : .light
        keyboardAppearance = night ? .dark : .light
        for button in accessoryButtons {
            button.tintColor = accent
            button.configuration?.baseForegroundColor = foreground
            button.configuration?.baseBackgroundColor = accent.withAlphaComponent(0.16)
        }
    }
    private var modifierButtons: [String: UIButton] = [:]
    private let sentinel = "\u{200B}"

    init() {
        super.init(frame: .zero, textContainer: nil)
        delegate = self
        text = sentinel
        selectedRange = NSRange(location: 1, length: 0)
        autocorrectionType = .no
        autocapitalizationType = .none
        smartQuotesType = .no
        smartDashesType = .no
        spellCheckingType = .no
        backgroundColor = .clear
        textColor = .clear
        tintColor = .clear
        accessibilityLabel = String(localized: "Remote keyboard input")
        inputAccessoryView = makeAccessory()
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func textViewDidChange(_ textView: UITextView) {
        guard markedTextRange == nil else { return } // preserve CJK composition
        let committed = text.replacingOccurrences(of: sentinel, with: "")
        if inputEnabled, !committed.isEmpty { sendCommitted(committed) }
        text = sentinel
        selectedRange = NSRange(location: 1, length: 0)
    }

    override func deleteBackward() {
        if markedTextRange != nil { super.deleteBackward(); return }
        sendKey("Backspace")
    }

    func textViewDidEndEditing(_ textView: UITextView) {
        releaseModifiers()
        onVisibility?(false)
    }

    private func sendCommitted(_ text: String) {
        guard inputEnabled else { return }
        let hasModifiers = sticky.shift || sticky.control || sticky.meta || sticky.alt
        if let code = DesktopKeyboardPolicy.committedKeyCode(text, hasModifiers: hasModifiers) {
            sendKey(code)
        } else {
            onInput?(.text(text))
        }
    }

    private func sendKey(_ code: String, modifiers: DesktopModifiers? = nil) {
        guard inputEnabled else { return }
        let mods = modifiers ?? sticky
        onInput?(.key(code: code, down: true, modifiers: mods))
        onInput?(.key(code: code, down: false, modifiers: mods))
    }

    func releaseModifiers() {
        // Sticky keys are event flags, not held remote keydowns, so cannot leave
        // the host with a stuck modifier after reconnect/background/dismissal.
        sticky = DesktopModifiers()
        for button in modifierButtons.values {
            button.isSelected = false
            button.accessibilityValue = String(localized: "Off")
        }
    }

    private func makeAccessory() -> UIView {
        let container = UIInputView(frame: CGRect(x: 0, y: 0, width: 320, height: 100), inputViewStyle: .keyboard)
        let rows = UIStackView()
        rows.axis = .vertical
        rows.spacing = 2
        rows.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(rows)
        NSLayoutConstraint.activate([
            rows.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 4),
            rows.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -4),
            rows.topAnchor.constraint(equalTo: container.topAnchor, constant: 4),
            rows.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -4)
        ])
        let main = accessoryRow()
        rows.addArrangedSubview(main.scroll)
        for (title, key, label) in [
            ("⇧", "shift", String(localized: "Shift modifier")),
            ("Ctrl", "control", String(localized: "Control modifier")),
            ("⌥", "alt", String(localized: "Option modifier")),
            ("⌘", "meta", String(localized: "Command modifier"))
        ] {
            let button = accessoryButton(title: title, label: label)
            button.accessibilityValue = String(localized: "Off")
            button.addAction(UIAction { [weak self, weak button] _ in
                guard let self, self.inputEnabled, let button else { return }
                switch key {
                case "shift": self.sticky.shift.toggle()
                case "control": self.sticky.control.toggle()
                case "alt": self.sticky.alt.toggle()
                default: self.sticky.meta.toggle()
                }
                button.isSelected.toggle()
                button.accessibilityValue = button.isSelected ? String(localized: "On") : String(localized: "Off")
            }, for: .touchUpInside)
            modifierButtons[key] = button
            main.stack.addArrangedSubview(button)
        }
        for (title, code, label) in [
            ("Esc", "Escape", String(localized: "Escape key")),
            ("Tab", "Tab", String(localized: "Tab key")),
            ("↵", "Enter", String(localized: "Enter key")),
            ("↓", "hide", String(localized: "Hide keyboard"))
        ] {
            let button = accessoryButton(title: title, label: label)
            button.addAction(UIAction { [weak self] _ in
                if code == "hide" { self?.resignFirstResponder() }
                else { self?.sendKey(code) }
            }, for: .touchUpInside)
            main.stack.addArrangedSubview(button)
        }
        let extra = accessoryRow()
        rows.addArrangedSubview(extra.scroll)
        let arrows = [("←", "ArrowLeft", String(localized: "Left arrow key")),
                      ("↑", "ArrowUp", String(localized: "Up arrow key")),
                      ("↓", "ArrowDown", String(localized: "Down arrow key")),
                      ("→", "ArrowRight", String(localized: "Right arrow key"))]
        for (title, code, label) in arrows + (1...12).map({ ("F\($0)", "F\($0)", "F\($0)") }) {
            let button = accessoryButton(title: title, label: label)
            button.addAction(UIAction { [weak self] _ in self?.sendKey(code) }, for: .touchUpInside)
            extra.stack.addArrangedSubview(button)
        }
        return container
    }

    private func accessoryRow() -> (scroll: UIScrollView, stack: UIStackView) {
        let scroll = UIScrollView()
        scroll.showsHorizontalScrollIndicator = false
        let stack = UIStackView()
        stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            stack.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 44)
        ])
        return (scroll, stack)
    }

    private func accessoryButton(title: String, label: String) -> UIButton {
        let button = UIButton(type: .system)
        var config = UIButton.Configuration.tinted()
        config.title = title
        config.cornerStyle = .small
        config.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 10, bottom: 8, trailing: 10)
        button.configuration = config
        button.accessibilityLabel = label
        button.widthAnchor.constraint(greaterThanOrEqualToConstant: 44).isActive = true
        button.heightAnchor.constraint(equalToConstant: 44).isActive = true
        accessoryButtons.append(button)
        return button
    }

    override var keyCommands: [UIKeyCommand]? {
        guard inputEnabled else { return [] }
        var commands: [UIKeyCommand] = []
        let special = [UIKeyCommand.inputEscape, UIKeyCommand.inputUpArrow, UIKeyCommand.inputDownArrow,
                       UIKeyCommand.inputLeftArrow, UIKeyCommand.inputRightArrow, "\t", "\r"]
        // Capture all modifier combinations so iPadOS does not steal remote
        // shortcuts such as Command-Tab / Command-arrow before our responder.
        for bits in 0..<16 {
            let flags = Self.flags(bits)
            for input in special {
                let command = UIKeyCommand(input: input, modifierFlags: flags, action: #selector(hardwareCommand(_:)))
                command.wantsPriorityOverSystemBehavior = true
                commands.append(command)
            }
            if bits != 0 {
                for input in DesktopKeyboardPolicy.hardwareShortcutInputs {
                    let command = UIKeyCommand(input: String(input), modifierFlags: flags, action: #selector(hardwareCommand(_:)))
                    command.wantsPriorityOverSystemBehavior = true
                    commands.append(command)
                }
            }
        }
        return commands
    }

    private static func flags(_ bits: Int) -> UIKeyModifierFlags {
        var flags: UIKeyModifierFlags = []
        if bits & 1 != 0 { flags.insert(.shift) }
        if bits & 2 != 0 { flags.insert(.control) }
        if bits & 4 != 0 { flags.insert(.alternate) }
        if bits & 8 != 0 { flags.insert(.command) }
        return flags
    }

    @objc private func hardwareCommand(_ command: UIKeyCommand) {
        guard let input = command.input else { return }
        let codes = [UIKeyCommand.inputEscape: "Escape", UIKeyCommand.inputUpArrow: "ArrowUp",
                     UIKeyCommand.inputDownArrow: "ArrowDown", UIKeyCommand.inputLeftArrow: "ArrowLeft",
                     UIKeyCommand.inputRightArrow: "ArrowRight", "\t": "Tab", "\r": "Enter"]
        guard let code = codes[input] ?? DesktopKeyboardPolicy.hostKeyCode(input) else { return }
        var mods = sticky
        mods.shift = mods.shift || command.modifierFlags.contains(.shift)
        mods.control = mods.control || command.modifierFlags.contains(.control)
        mods.alt = mods.alt || command.modifierFlags.contains(.alternate)
        mods.meta = mods.meta || command.modifierFlags.contains(.command)
        sendKey(code, modifiers: mods)
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let remaining = presses.filter { press in
            guard inputEnabled, let key = press.key,
                  (58...69).contains(key.keyCode.rawValue) else { return true }
            var mods = sticky
            mods.shift = mods.shift || key.modifierFlags.contains(.shift)
            mods.control = mods.control || key.modifierFlags.contains(.control)
            mods.alt = mods.alt || key.modifierFlags.contains(.alternate)
            mods.meta = mods.meta || key.modifierFlags.contains(.command)
            sendKey("F\(key.keyCode.rawValue - 57)", modifiers: mods)
            return false
        }
        if !remaining.isEmpty { super.pressesBegan(Set(remaining), with: event) }
    }

}
