import SwiftUI
import UIKit

/// Prototype terminal accessory row (`.keyrow`) and command dock (`.cmdline`).
///
/// The Ctrl key keeps the existing latch/hold contract: a tap latches exactly
/// one compatible key, a hold applies while pressed. Repeatable keys (arrows,
/// backspace) repeat while held. Live keyboard focus and buffered commands are
/// both preserved so no input route from the old dock is lost.
///
/// Every accessory key is a real `Button`. Gestures are deliberately absent:
/// a `DragGesture(minimumDistance: 0)` on a child inside a horizontal
/// `ScrollView` wins the gesture arbitration, which both blocks the scroll and
/// swallows taps. A `ButtonStyle` gives the press feedback and the long-press
/// repeat without competing with the scroll view.
///
/// The system keyboard has one owner at a time: either the live terminal canvas
/// or the command field. The accessory row's keyboard control is the single
/// hide/show affordance, and it is pinned outside the scrolling key strip so
/// hiding never depends on what is on screen.
struct TerminalInputDock: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var isEnabled: Bool
    var isKeyboardVisible: Bool
    @Binding var controlModifier: TerminalControlModifier
    var onSend: (String) -> Void
    var onFocusTerminal: () -> Void
    var onDismissKeyboard: () -> Void

    @State private var command = ""
    @State private var controlPressStarted: Date?
    @State private var controlPressFinished = false
    @FocusState private var commandFocused: Bool

    /// How long a Ctrl press counts as a tap (latch) rather than a hold.
    private static let controlTapDuration: TimeInterval = 0.35

    var body: some View {
        VStack(spacing: 0) {
            keyRow
            commandLine
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 8)
        .background(palette.surface)
        .opacity(isEnabled ? 1 : 0.35)
        .allowsHitTesting(isEnabled)
        .disabled(!isEnabled)
        .onChange(of: isKeyboardVisible) { _, visible in
            if !visible {
                commandFocused = false
            }
        }
        .onDisappear {
            controlModifier.releaseHold()
            controlPressStarted = nil
        }
    }

    // MARK: - Accessory keys

    /// Keys scroll horizontally; the keyboard toggle stays pinned so the one
    /// escape from the keyboard never scrolls out of reach.
    private var keyRow: some View {
        HStack(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    // Prototype order: Esc, Tab, Ctrl, arrows, then the real host
                    // extras (Enter/Backspace/control letters). The prototype's
                    // Option/Meta chord keys are omitted because the phone input
                    // model has no modifier-chord route.
                    ForEach(headKeys) { key in
                        keyChip(key)
                    }
                    controlKey
                    ForEach(arrowKeys) { key in
                        keyChip(key)
                    }
                    ForEach(remainingKeys) { key in
                        keyChip(key)
                    }
                    if UIPasteboard.general.hasStrings {
                        TerminalKeyButton(
                            label: String(localized: "Paste"),
                            accessibilityLabel: String(localized: "Paste from clipboard")
                        ) {
                            if let text = UIPasteboard.general.string, !text.isEmpty { send(text) }
                        }
                    }
                }
                .padding(.vertical, 2)
                .padding(.trailing, 14)
            }
            // The strip fades into the pinned keyboard key instead of being
            // cut off mid-glyph. It never intercepts touches.
            .overlay(alignment: .trailing) {
                LinearGradient(
                    colors: [palette.surface.opacity(0), palette.surface],
                    startPoint: .leading,
                    endPoint: .trailing
                )
                .frame(width: 18)
                .allowsHitTesting(false)
            }
            keyboardToggle
        }
    }

    /// One input path for the system keyboard: hide it, or hand focus back to
    /// the live terminal. The command field is the buffered route and never
    /// competes for the same responder at the same time.
    private var keyboardToggle: some View {
        Button {
            if isKeyboardVisible {
                commandFocused = false
                onDismissKeyboard()
            } else {
                onFocusTerminal()
            }
        } label: {
            Image(systemName: isKeyboardVisible ? "keyboard.chevron.compact.down" : "keyboard")
                .font(.system(size: 14, weight: .medium))
                .frame(width: 30, height: 30)
        }
        .buttonStyle(TerminalKeyButtonStyle(
            isActive: isKeyboardVisible,
            horizontalPadding: 0
        ))
        .accessibilityLabel(Text(isKeyboardVisible
            ? "Hide keyboard"
            : "Show keyboard for live terminal input"))
        .accessibilityIdentifier("terminal-keyboard-toggle")
    }

    private var headKeys: [TerminalAccessoryKey] {
        ["escape", "tab"].compactMap(key(id:))
    }

    private var arrowKeys: [TerminalAccessoryKey] {
        ["arrowUp", "arrowDown", "arrowLeft", "arrowRight"].compactMap(key(id:))
    }

    private var remainingKeys: [TerminalAccessoryKey] {
        let used = Set(headKeys.map(\.id) + arrowKeys.map(\.id))
        return TerminalAccessoryCatalog.keys.filter { !used.contains($0.id) }
    }

    private func key(id: String) -> TerminalAccessoryKey? {
        TerminalAccessoryCatalog.keys.first { $0.id == id }
    }

    @ViewBuilder
    private func keyChip(_ key: TerminalAccessoryKey) -> some View {
        if key.repeatable {
            TerminalKeyButton(
                label: key.label,
                repeatable: true,
                accessibilityLabel: key.accessibilityLabel,
                action: { send(key.bytes) },
                repeatAction: { send(key.bytes) }
            )
        } else {
            TerminalKeyButton(
                label: key.label,
                accessibilityLabel: key.accessibilityLabel
            ) {
                send(key.bytes)
            }
        }
    }

    private var controlKey: some View {
        let label: String
        if controlModifier.isHeld {
            label = String(localized: "Ctrl · Held")
        } else if controlModifier.isLatched {
            label = String(localized: "Ctrl · Latched")
        } else {
            label = String(localized: "Ctrl")
        }
        return TerminalKeyButton(
            label: label,
            isActive: controlModifier.isActive,
            accessibilityLabel: String(localized: "Control key"),
            accessibilityValue: controlModifier.isHeld
                ? String(localized: "Held")
                : controlModifier.isLatched ? String(localized: "Latched") : String(localized: "Off"),
            accessibilityHint: String(localized: "Tap to latch the next key, or hold while typing a key."),
            accessibilityIdentifier: "terminal-control-key",
            action: {
                // Fires on a tap release and on VoiceOver activation. A hold
                // (or a press that was used by another key) must not latch.
                let duration = controlPressStarted.map { Date().timeIntervalSince($0) } ?? 0
                controlPressFinished = true
                controlModifier.endPress(isTap: duration < Self.controlTapDuration)
            },
            onPressChanged: { pressed in
                if pressed {
                    controlPressFinished = false
                    controlPressStarted = Date()
                    controlModifier.beginPress()
                } else if !controlPressFinished {
                    // A scroll or drag-out cancels the button without firing
                    // its action: release the hold, never latch.
                    controlModifier.releaseHold()
                }
            }
        )
    }

    // MARK: - Command dock (prototype `.cmdline`)

    private var commandLine: some View {
        HStack(spacing: 8) {
            Text("$")
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .foregroundStyle(palette.live)
                .accessibilityHidden(true)

            TextField("Type a command, or hold the microphone", text: $command)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.asciiCapable)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(palette.ink)
                .submitLabel(.send)
                .focused($commandFocused)
                .onSubmit { submitCommand() }
                .onChange(of: command) { _, text in
                    // Ctrl in command mode sends just the compatible key, not a
                    // command plus Return. Leave multi-character paste untouched.
                    if controlModifier.isActive, TerminalControlModifier.controlSequence(for: text) != nil {
                        command = ""
                        send(text)
                    }
                }
                .frame(minHeight: 32)

            Button {
                submitCommand()
            } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(palette.onAccent)
                    .frame(width: 30, height: 30)
                    .background(palette.accent, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .opacity(command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.45 : 1)
            .accessibilityLabel(Text("Send command"))
        }
        .padding(.leading, 11)
        .padding(.trailing, 6)
        .frame(height: 44)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(commandFocused ? palette.accent : palette.strokeStrong, lineWidth: 1)
        }
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: commandFocused)
        .padding(.top, 8)
    }

    private func submitCommand() {
        let text = command
        guard !text.isEmpty else { return }
        command = ""
        send(text.hasSuffix("\r") ? text : text + "\r")
    }

    private func send(_ bytes: String) {
        guard isEnabled else { return }
        onSend(bytes)
    }
}

// MARK: - Key button

/// One accessory key: a real `Button` with a capsule `ButtonStyle`.
///
/// The button is 44 pt tall and at least 40 pt wide, so the target survives
/// the 30 pt visual capsule. Repeatable keys start repeating after a short
/// hold; the tap action still sends exactly once when the button is released
/// before the repeat starts.
struct TerminalKeyButton: View {
    let label: String
    var isActive = false
    var repeatable = false
    var accessibilityLabel: String
    var accessibilityValue: String?
    var accessibilityHint: String?
    var accessibilityIdentifier: String?
    var action: () -> Void
    var repeatAction: (() -> Void)?
    var onPressChanged: ((Bool) -> Void)?

    @State private var repeatTask: Task<Void, Never>?
    @State private var didRepeat = false

    var body: some View {
        Button {
            // A hold that already repeated must not send an extra key on
            // release; a plain tap sends exactly once here.
            if !didRepeat { action() }
            didRepeat = false
        } label: {
            Text(label)
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .lineLimit(1)
        }
        .buttonStyle(TerminalKeyButtonStyle(
            isActive: isActive,
            onPressChanged: { pressed in
                if pressed {
                    didRepeat = false
                    if repeatable { startRepeat() }
                } else {
                    stopRepeat()
                }
                onPressChanged?(pressed)
            }
        ))
        .accessibilityLabel(Text(accessibilityLabel))
        .accessibilityValue(Text(accessibilityValue ?? ""))
        .accessibilityHint(Text(accessibilityHint ?? ""))
        .terminalAccessibilityIdentifier(accessibilityIdentifier)
        .accessibilityAddTraits(isActive ? .isSelected : [])
    }

    private func startRepeat() {
        guard repeatable, let repeatAction else { return }
        stopRepeat()
        repeatTask = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(380))
            while !Task.isCancelled {
                repeatAction()
                didRepeat = true
                try? await Task.sleep(for: .milliseconds(70))
            }
        }
    }

    private func stopRepeat() {
        repeatTask?.cancel()
        repeatTask = nil
    }
}

/// Capsule styling for accessory keys. The visual capsule stays ~30 pt tall
/// inside the 44 pt hit frame, so the row looks like the prototype while the
/// touch target meets the platform minimum. Reduce Motion removes the scale
/// press feedback but keeps the color change.
struct TerminalKeyButtonStyle: ButtonStyle {
    var isActive = false
    var horizontalPadding: CGFloat = 10
    var onPressChanged: ((Bool) -> Void)?

    func makeBody(configuration: Configuration) -> some View {
        TerminalKeySurface(
            configuration: configuration,
            isActive: isActive,
            horizontalPadding: horizontalPadding,
            onPressChanged: onPressChanged
        )
    }
}

private struct TerminalKeySurface: View {
    let configuration: ButtonStyleConfiguration
    let isActive: Bool
    let horizontalPadding: CGFloat
    let onPressChanged: ((Bool) -> Void)?

    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var isHighlighted: Bool { isActive || configuration.isPressed }

    var body: some View {
        configuration.label
            .foregroundStyle(isHighlighted ? palette.canvas : palette.inkSecondary)
            .padding(.horizontal, horizontalPadding)
            .frame(minWidth: 40, minHeight: 30)
            .background(
                isHighlighted ? palette.ink : palette.card,
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
            .overlay {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(isActive ? palette.accent : palette.stroke, lineWidth: 1)
            }
            .frame(minWidth: 40, minHeight: SeSpace.hitTarget)
            .contentShape(Rectangle())
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .animation(
                reduceMotion ? nil : .easeOut(duration: 0.12),
                value: configuration.isPressed
            )
            .onChange(of: configuration.isPressed) { _, pressed in
                onPressChanged?(pressed)
            }
    }
}

private extension View {
    @ViewBuilder
    func terminalAccessibilityIdentifier(_ identifier: String?) -> some View {
        if let identifier {
            accessibilityIdentifier(identifier)
        } else {
            self
        }
    }
}
