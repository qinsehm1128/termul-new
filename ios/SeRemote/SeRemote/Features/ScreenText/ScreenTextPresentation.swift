import Foundation

enum ScreenTextPresentation {
    static func semanticStatus(_ state: HostScreenObservation.State) -> SeStatus {
        switch state {
        case .ready: .completed
        case .starting, .catchingUp: .working
        case .degraded: .needsApproval
        case .disabled, .unsupported: .offline
        }
    }

    static func explanation(_ state: HostScreenObservation.State) -> String {
        switch state {
        case .ready: String(localized: "The host confirmed a consistent terminal screen.")
        case .starting: String(localized: "The host is starting screen observation. Refresh in a moment.")
        case .catchingUp: String(localized: "The host is catching up with terminal output. Refresh when the screen settles.")
        case .degraded: String(localized: "The host's screen observation is incomplete or out of sync. Text is hidden until a consistent snapshot is available.")
        case .disabled: String(localized: "Screen observation is disabled on this host. The terminal itself is unaffected.")
        case .unsupported: String(localized: "This host or terminal does not support consistent screen text. The terminal itself is unaffected.")
        }
    }
}
