import SwiftUI

/// Compatibility entry point shared by the home and connected host shells.
@MainActor
struct ShellPairingView: View {
    let onConnect: (RemoteLink) -> Void

    var body: some View {
        PairingSheetView(onConnect: onConnect)
    }
}
