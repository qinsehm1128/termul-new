import SwiftUI

struct RootView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        // The old hard cut protected WorkspaceView's socket lifetime: a
        // transition can report disappearance before an insertion completes.
        // WorkspaceView now checks the authoritative active link before
        // stopping, and each link has a distinct, stable session identity.
        ZStack {
            if let link = store.activeLink {
                WorkspaceView(store: store, settings: settings, link: link)
                    // The origin is part of the key: a rescan that keeps the
                    // saved record but moves the address must open a session
                    // against the new origin, never keep dialing the dead one.
                    .id(link.sessionIdentity)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            } else {
                HomeView(store: store, settings: settings)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            }
        }
        .background(SeTheme.canvas.ignoresSafeArea())
        .seAnimation(.context, value: store.activeLink?.sessionIdentity)
        // Below SeRemoteApp's preferredColorScheme, so the persisted
        // appearance (or the device setting for .system) decides day/night.
        .seResolvedThemeMode()
    }
}
