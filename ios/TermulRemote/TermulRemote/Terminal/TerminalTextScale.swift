import Foundation

enum TerminalTextScale {
    static let presets: [CGFloat] = [0.5, 0.75, 1, 1.25, 1.5, 2]
    static let defaultValue: CGFloat = 1.25
    private static let storageKey = "termul.companion.terminalTextScale"

    static var current: CGFloat {
        get {
            if let stored = UserDefaults.standard.object(forKey: storageKey) as? Double {
                return snap(CGFloat(stored))
            }
            return defaultValue
        }
        set {
            UserDefaults.standard.set(Double(snap(newValue)), forKey: storageKey)
        }
    }

    static func clamp(_ value: CGFloat) -> CGFloat {
        guard value.isFinite else { return defaultValue }
        return min(presets.last ?? 2, max(presets.first ?? 0.5, value))
    }

    static func snap(_ value: CGFloat) -> CGFloat {
        let clamped = clamp(value)
        return presets.min(by: { abs($0 - clamped) < abs($1 - clamped) }) ?? defaultValue
    }

    static func nudge(_ value: CGFloat, by direction: Int) -> CGFloat {
        let snapped = snap(value)
        guard let index = presets.firstIndex(of: snapped) else { return snapped }
        let next = index + direction
        if next < 0 { return presets[0] }
        if next >= presets.count { return presets[presets.count - 1] }
        return presets[next]
    }
}
