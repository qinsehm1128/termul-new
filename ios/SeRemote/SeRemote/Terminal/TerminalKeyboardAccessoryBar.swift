import SwiftUI
import UIKit

/// The terminal keyboard's accessory row.
///
/// This row lives at the bottom of the system keyboard (`inputAccessoryView`),
/// not in the terminal layout, so typing never steals rows from the PTY. There
/// is deliberately no text field: the canvas is the only input route.
///
/// `+` sits at the leading edge (it replaces the floating cluster while the
/// keyboard is up), the keys type straight into the PTY, and `Hide` is pinned
/// at the trailing edge so the way out never scrolls.
struct TerminalKeyboardAccessoryBar: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    @Binding var controlModifier: TerminalControlModifier
    var isEnabled = true
    var onSend: (String) -> Void
    var onPageScroll: ((Bool) -> Void)? = nil
    var onAttach: () -> Void
    var onHide: () -> Void

    @State private var controlPressStarted: Date?
    @State private var controlPressFinished = false

    private static let controlTapDuration: TimeInterval = 0.35

    var body: some View {
        HStack(spacing: 6) {
            TerminalKeyButton(
                label: "+",
                accessibilityLabel: String(localized: "Attach")
            ) {
                onAttach()
            }

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(TerminalAccessoryCatalog.keys.filter { key in
                        ["escape", "tab"].contains(key.id)
                    }) { key in
                        TerminalKeyButton(
                            label: key.label,
                            accessibilityLabel: key.accessibilityLabel
                        ) {
                            send(key.bytes)
                        }
                    }
                    controlKey
                    ForEach(TerminalAccessoryCatalog.keys.filter { key in
                        ["arrowUp", "arrowDown", "arrowLeft", "arrowRight"].contains(key.id)
                    }) { key in
                        TerminalKeyButton(
                            label: key.label,
                            repeatable: true,
                            accessibilityLabel: key.accessibilityLabel,
                            action: { send(key.bytes) },
                            repeatAction: { send(key.bytes) }
                        )
                    }
                    ForEach(TerminalAccessoryCatalog.keys.filter { key in
                        ["pageUp", "pageDown"].contains(key.id)
                    }) { key in
                        let isUp = key.id == "pageUp"
                        TerminalKeyButton(
                            label: key.label,
                            repeatable: true,
                            accessibilityLabel: key.accessibilityLabel,
                            action: { triggerPageScroll(up: isUp, fallbackBytes: key.bytes) },
                            repeatAction: { triggerPageScroll(up: isUp, fallbackBytes: key.bytes) }
                        )
                    }
                    TerminalKeyButton(
                        label: "⌘V",
                        accessibilityLabel: String(localized: "Paste")
                    ) {
                        guard let text = UIPasteboard.general.string, !text.isEmpty else { return }
                        send(text)
                    }
                }
                .padding(.vertical, 2)
            }

            TerminalKeyButton(
                label: String(localized: "Hide"),
                accessibilityLabel: String(localized: "Hide keyboard")
            ) {
                onHide()
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .frame(height: 50)
        .background(palette.card)
        .overlay(alignment: .top) {
            Rectangle().fill(mode.hairline).frame(height: 1)
        }
        .opacity(isEnabled ? 1 : 0.4)
        .allowsHitTesting(isEnabled)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Terminal keys"))
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
            accessibilityIdentifier: "terminal-accessory-control-key",
            action: {
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
                    controlModifier.releaseHold()
                }
            }
        )
    }

    private func send(_ bytes: String) {
        guard isEnabled else { return }
        onSend(controlModifier.apply(to: bytes))
    }

    private func triggerPageScroll(up: Bool, fallbackBytes: String) {
        guard isEnabled else { return }
        if let onPageScroll {
            onPageScroll(up)
        } else {
            send(fallbackBytes)
        }
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
