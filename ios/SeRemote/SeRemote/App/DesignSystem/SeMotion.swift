import SwiftUI

/// Motion tokens (design-spec.md section 7). `control` and `disclosure` are
/// converted from the desktop renderer's chat springs; tune on device.
enum SeMotion {
    case control
    case disclosure
    case context
    case refreshReturn
    case press
    case chevron
    case fade

    /// Full-motion animation.
    var animation: Animation {
        switch self {
        case .control: .spring(response: 0.246, dampingFraction: 0.735)
        case .disclosure: .spring(response: 0.29, dampingFraction: 0.977)
        case .context: .spring(response: 0.34, dampingFraction: 0.88)
        case .refreshReturn: .spring(response: 0.40, dampingFraction: 0.88)
        case .press: .easeOut(duration: 0.14)
        case .chevron: .easeInOut(duration: 0.15)
        case .fade: .easeOut(duration: 0.16)
        }
    }

    /// Reduce Motion fallback: no travel, scale, overshoot or rotation, only a
    /// short opacity change.
    static let reducedAnimation: Animation = .easeOut(duration: 0.1)

    func animation(reduceMotion: Bool) -> Animation {
        reduceMotion ? Self.reducedAnimation : animation
    }

    /// Vertical travel for entrances such as a sent message or a permission card.
    static let entranceTravel: CGFloat = 8
}

extension View {
    /// Animates `value` changes with a motion token, honoring Reduce Motion.
    func seAnimation(_ motion: SeMotion, value: some Equatable) -> some View {
        modifier(SeAnimationModifier(motion: motion, value: value))
    }

    /// Entrance used for a newly sent message or an arriving permission card:
    /// opacity plus a small upward travel, or opacity only under Reduce Motion.
    func seEntrance() -> some View {
        modifier(SeEntranceTransitionModifier())
    }
}

private struct SeAnimationModifier<Value: Equatable>: ViewModifier {
    let motion: SeMotion
    let value: Value
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.animation(motion.animation(reduceMotion: reduceMotion), value: value)
    }
}

private struct SeEntranceTransitionModifier: ViewModifier {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.transition(SeTransition.entrance(reduceMotion: reduceMotion))
    }
}

enum SeTransition {
    static func entrance(reduceMotion: Bool) -> AnyTransition {
        reduceMotion
            ? .opacity
            : .opacity.combined(with: .offset(y: SeMotion.entranceTravel))
    }

    /// Content swap for tabs: short fade with at most 8pt sideways travel.
    static func tabContent(reduceMotion: Bool, forward: Bool) -> AnyTransition {
        guard !reduceMotion else { return .opacity }
        let travel = forward ? SeMotion.entranceTravel : -SeMotion.entranceTravel
        return .asymmetric(
            insertion: .opacity.combined(with: .offset(x: travel)),
            removal: .opacity
        )
    }
}

/// `withAnimation` that honors Reduce Motion. Views read the environment
/// value and pass it in, because `withAnimation` cannot read it itself.
@MainActor
func seWithAnimation<Result>(
    _ motion: SeMotion,
    reduceMotion: Bool,
    _ body: () throws -> Result
) rethrows -> Result {
    try withAnimation(motion.animation(reduceMotion: reduceMotion), body)
}
