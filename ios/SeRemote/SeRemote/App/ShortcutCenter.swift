import Combine
import Foundation

/// Hardware-keyboard menu commands. The app is single-window, so a static bus
/// is the simplest routing surface; the visible screen decides what each
/// shortcut means for the current layout width.
@MainActor
enum ShortcutCenter {
    enum Shortcut: String {
        case focusChat
        case focusTerminal
        case toggleFiles
        case newTerminal
        case nextTerminal
        case previousTerminal
        case textScaleUp
        case textScaleDown
        case textScaleReset
    }

    private static let subject = PassthroughSubject<Shortcut, Never>()

    static var shortcuts: AnyPublisher<Shortcut, Never> {
        subject.eraseToAnyPublisher()
    }

    static func send(_ shortcut: Shortcut) {
        HostLog.ui.info("Shortcut \(shortcut.rawValue, privacy: .public)")
        subject.send(shortcut)
    }
}
