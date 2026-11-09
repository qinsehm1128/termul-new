import SwiftUI
import UIKit

/// Prototype terminal accessory row (`.keyrow`) and command dock (`.cmdline`).
///
/// The Ctrl key keeps the existing latch/hold contract: a tap latches exactly
/// one compatible key, a hold applies while pressed. Repeatable keys (arrows,
/// backspace) repeat while held. Live keyboard focus and buffered commands are
/// both preserved so no input route from the old dock is lost.
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
    @GestureState private var controlTouchDown = false
    @FocusState private var commandFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            keyRow
            commandLine
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 10)
        .background(palette.surface)
        .opacity(isEnabled ? 1 : 0.35)
        .allowsHitTesting(isEnabled)
        .disabled(!isEnabled)
        .onChange(of: isKeyboardVisible) { _, visible in
            if !visible {
                commandFocused = false
            }
        }
        .onChange(of: controlTouchDown) { _, isPressed in
            if !isPressed, controlPressStarted != nil {
                controlModifier.releaseHold()
                controlPressStarted = nil
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
                        R2KeyChip(
                            label: String(localized: "Paste"),
                            accessibilityLabel: String(localized: "Paste from clipboard")
                        ) {
                            if let text = UIPasteboard.general.string, !text.isEmpty { send(text) }
                        }
                    }
                }
                .padding(.vertical, 2)
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
                .foregroundStyle(isKeyboardVisible ? palette.ink : palette.inkSecondary)
                .frame(width: 30, height: 30)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .stroke(isKeyboardVisible ? palette.accent : palette.stroke, lineWidth: 1)
                }
                .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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
            R2KeyChip(
                label: key.label,
                repeatable: true,
                accessibilityLabel: key.accessibilityLabel,
                action: { send(key.bytes) },
                repeatAction: { send(key.bytes) }
            )
        } else {
            R2KeyChip(
                label: key.label,
                accessibilityLabel: key.accessibilityLabel
            ) {
                send(key.bytes)
            }
        }
    }

    private var controlKey: some View {
        R2KeyChip(
            label: controlModifier.isHeld ? String(localized: "Ctrl · Held")
                : controlModifier.isLatched ? String(localized: "Ctrl · Latched") : String(localized: "Ctrl"),
            isActive: controlModifier.isActive,
            accessibilityLabel: String(localized: "Control key"),
            action: { controlModifier.toggleLatch() }
        )
        .accessibilityValue(Text(controlModifier.isHeld
            ? String(localized: "Held")
            : controlModifier.isLatched ? String(localized: "Latched") : String(localized: "Off")))
        .accessibilityHint(Text("Tap to latch the next key, or hold while typing a key."))
        .accessibilityIdentifier("terminal-control-key")
        .simultaneousGesture(
            DragGesture(minimumDistance: 0)
                .updating($controlTouchDown) { _, pressed, _ in pressed = true }
                .onChanged { _ in
                    guard controlPressStarted == nil else { return }
                    controlPressStarted = Date()
                    controlModifier.beginPress()
                }
                .onEnded { _ in
                    let isTap = controlPressStarted.map { Date().timeIntervalSince($0) < 0.35 } ?? false
                    controlModifier.endPress(isTap: isTap)
                    controlPressStarted = nil
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
