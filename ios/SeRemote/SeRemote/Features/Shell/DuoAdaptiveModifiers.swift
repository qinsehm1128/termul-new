import SwiftUI

/// Adaptive modifiers aligned with Apple's iPhone Duo & Liquid Glass guidance.
///
/// Built against Xcode 27.0+ while preserving an iOS 26.0 deployment target:
/// native `TabView` + `.toolbar` unlocks the system trailing vertical bar
/// automatically when linked with the iOS 27.1+ SDK.
enum DuoToolbarPriority: Sendable {
    case high
    case standard
    case low
}

extension View {
    /// Prefers the floating tab bar placement when tab bar and sidebar are
    /// mutually exclusive (iPhone Duo outer screen & inner landscape), while
    /// keeping `.sidebarAdaptable` sidebar toggling intact on iPadOS.
    @ViewBuilder
    func duoPreferredTabBarPlacement() -> some View {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            self.defaultTabBarPlacement(.tabBar)
        } else {
            self.defaultAdaptableTabBarPlacement(.tabBar)
        }
        #else
        self.defaultAdaptableTabBarPlacement(.tabBar)
        #endif
    }

    /// Extends canvas backgrounds cleanly beneath floating Liquid Glass sidebars
    /// and trailing vertical bars.
    @ViewBuilder
    func duoBackgroundExtension() -> some View {
        self.backgroundExtensionEffect()
    }
}

extension ToolbarContent {
    /// Controls overflow retention order when iPhone Duo's trailing vertical
    /// bar has limited vertical room (e.g. outer landscape or active Live Activity).
    @ToolbarContentBuilder
    func duoVisibilityPriority(_ priority: DuoToolbarPriority) -> some ToolbarContent {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            switch priority {
            case .high:
                self.visibilityPriority(.high)
            case .standard:
                self.visibilityPriority(.automatic)
            case .low:
                self.visibilityPriority(.low)
            }
        } else {
            self
        }
        #else
        self
        #endif
    }
}

enum DuoToolbarPlacement {
    /// Prominent trailing action placement: pins directly below navigation in
    /// iPhone Duo's vertical bar on iOS 27+, falling back to `.topBarTrailing`.
    static var pinnedTrailing: ToolbarItemPlacement {
        #if compiler(>=6.4)
        if #available(iOS 27.0, *) {
            return .topBarPinnedTrailing
        }
        #endif
        return .topBarTrailing
    }
}
