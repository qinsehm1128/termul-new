import SwiftUI
import UIKit

/// Keyboard visibility for terminal surfaces.
///
/// The workbench reads it from `WorkspaceSession`, because the chat composer
/// can own the keyboard in the wide layout. A full-screen attach console covers
/// that view, so it hears the keyboard itself — otherwise a console opened from
/// the chat tab sees `terminalKeyboardVisible == false`, the dock keeps showing
/// its "show keyboard" face and there is no way back out of the keyboard.
enum TerminalKeyboard {
    /// Overlap below which a keyboard frame is a floating or hardware
    /// accessory rather than a keyboard covering the terminal.
    static let minimumVisibleHeight: CGFloat = 40

    static func isVisible(_ height: CGFloat) -> Bool {
        height > minimumVisibleHeight
    }
}

private struct TerminalKeyboardHeightModifier: ViewModifier {
    @Binding var height: CGFloat

    func body(content: Content) -> some View {
        content
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { notification in
                height = KeyboardGuard.overlapHeight(from: notification)
            }
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
                height = 0
            }
    }
}

extension View {
    /// Tracks how much of the window the keyboard covers on show, hide and
    /// frame change (split and floating keyboards included).
    func terminalKeyboardHeight(_ height: Binding<CGFloat>) -> some View {
        modifier(TerminalKeyboardHeightModifier(height: height))
    }
}
