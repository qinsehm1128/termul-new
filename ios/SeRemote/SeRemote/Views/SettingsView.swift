import SwiftUI

@MainActor
struct SettingsView: View {
    @Bindable var settings: AppSettings
    var store: ConnectionStore? = nil
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        SettingsScreen(settings: settings, store: store, onBack: { dismiss() })
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
    }
}
