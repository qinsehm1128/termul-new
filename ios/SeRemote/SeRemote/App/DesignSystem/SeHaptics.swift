import SwiftUI

/// Haptics map (design-spec.md section 8). Every event fires once, at the
/// moment its outcome is known: approval feedback waits for the host ack, and
/// Run now reports the result rather than the request.
enum SeHaptic: Equatable, Sendable {
    /// A custom tab, mode or terminal tab selection actually changed.
    case selection
    /// A message was accepted by the local send queue.
    case send
    /// The host acknowledged an approval, a completed pairing, or a run succeeded.
    case success
    /// The host acknowledged a denial.
    case warning
    /// A clear failure: cannot connect, or an action failed.
    case error

    var feedback: SensoryFeedback {
        switch self {
        case .selection: .selection
        case .send: .impact(weight: .light, intensity: 0.45)
        case .success: .success
        case .warning: .warning
        case .error: .error
        }
    }
}

extension EnvironmentValues {
    /// User preference from Settings. Views never vibrate when this is false.
    @Entry var seHapticsEnabled: Bool = true
}

extension View {
    /// Plays `haptic` whenever `trigger` changes, if haptics are enabled.
    func seHaptic(_ haptic: SeHaptic, trigger: some Equatable) -> some View {
        modifier(SeHapticModifier(haptic: haptic, trigger: trigger))
    }

    /// Plays the haptic chosen by `event` whenever `trigger` changes. Return
    /// nil to stay silent for that change.
    func seHaptic<Trigger: Equatable>(
        trigger: Trigger,
        _ event: @escaping (_ old: Trigger, _ new: Trigger) -> SeHaptic?
    ) -> some View {
        modifier(SeDynamicHapticModifier(trigger: trigger, event: event))
    }
}

private struct SeHapticModifier<Trigger: Equatable>: ViewModifier {
    let haptic: SeHaptic
    let trigger: Trigger
    @Environment(\.seHapticsEnabled) private var enabled

    func body(content: Content) -> some View {
        content.sensoryFeedback(haptic.feedback, trigger: trigger) { _, _ in enabled }
    }
}

private struct SeDynamicHapticModifier<Trigger: Equatable>: ViewModifier {
    let trigger: Trigger
    let event: (Trigger, Trigger) -> SeHaptic?
    @Environment(\.seHapticsEnabled) private var enabled

    func body(content: Content) -> some View {
        content.sensoryFeedback(trigger: trigger) { old, new in
            guard enabled else { return nil }
            return event(old, new)?.feedback
        }
    }
}
