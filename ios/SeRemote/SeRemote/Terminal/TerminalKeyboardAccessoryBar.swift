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
}
