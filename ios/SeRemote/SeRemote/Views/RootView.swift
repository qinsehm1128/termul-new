import SwiftUI

struct RootView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings

    var body: some View {
        // No insert/remove transition. Animating this swap used to fire
        // WorkspaceView.onDisappear mid-connect and leave the spinner up.
        if let link = store.activeLink {
            WorkspaceView(store: store, link: link)
        } else {
            HomeView(store: store, settings: settings)
        }
    }
}
