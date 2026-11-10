import SwiftUI

struct RootView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sePalette) private var palette
    @State private var deniedCapability: String?

    private var isRevoked: Bool {
        guard let active = store.activeLink else { return false }
        return store.savedLinks.first(where: { $0.id == active.id })?.isRevoked == true
    }

    var body: some View {
        // The old hard cut protected WorkspaceView's socket lifetime: a
        // transition can report disappearance before an insertion completes.
        // WorkspaceView now checks the authoritative active link before
        // stopping, and each link has a distinct, stable session identity.
        ZStack {
            if let link = store.activeLink {
                WorkspaceView(store: store, settings: settings, link: link)
                    // Keep the terminal subtree mounted for this saved host.
                    .id(link.id)
                    .environment(\.remoteDeviceLink, link)
                    .allowsHitTesting(!isRevoked)
                    .accessibilityHidden(isRevoked)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            } else {
                HomeView(store: store, settings: settings)
                    .transition(SeTransition.entrance(reduceMotion: reduceMotion))
            }
        }
        .overlay {
            if isRevoked {
                VStack(spacing: 18) {
                    Image(systemName: "lock.slash")
                        .font(.title)
                        .accessibilityHidden(true)
                    Text("This device has been removed by the host")
                        .font(SeNocturneType.sheetTitle)
                        .multilineTextAlignment(.center)
                    SeSolidButton(title: String(localized: "Scan QR code again"), icon: .plus) {
                        deniedCapability = nil
                        store.requestRePairing()
                    }
                    .accessibilityIdentifier("device.revoked.rescan")
                }
                .padding(28)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .foregroundStyle(palette.ink)
                .background(palette.canvas.ignoresSafeArea())
                .accessibilityElement(children: .contain)
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if let capability = deniedCapability, !isRevoked {
                HStack(spacing: 10) {
                    SeIconView(icon: .info, size: 15, stroke: 1.6, color: palette.ink)
                    Text(String(localized: "Permission required: \(DeviceScope.label(for: capability))"))
                        .font(SeNocturneType.font(12))
                    Spacer(minLength: 0)
                    Button { deniedCapability = nil } label: {
                        SeIconView(icon: .close, size: 14, stroke: 1.7, color: palette.inkSecondary)
                            .frame(width: 44, height: 44)
                            .contentShape(Rectangle())
                    }
                    .accessibilityLabel(String(localized: "Dismiss"))
                }
                .padding(.leading, 18)
                .foregroundStyle(palette.ink)
                .background(palette.card)
                .accessibilityIdentifier("device.permission.notice")
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .hostDeviceRevoked)) { note in
            guard let origin = note.object as? String else { return }
            store.revoke(origin: origin)
        }
        .onReceive(NotificationCenter.default.publisher(for: .hostCapabilityDenied)) { note in
            guard let capability = note.object as? String,
                  note.userInfo?["origin"] as? String == store.activeLink?.originURL.absoluteString else { return }
            deniedCapability = capability
        }
        .onChange(of: store.activeLink?.id) { _, _ in deniedCapability = nil }
        .background(SeTheme.canvas.ignoresSafeArea())
        .seAnimation(.context, value: store.activeLink?.sessionIdentity)
        // Below SeRemoteApp's preferredColorScheme, so the persisted
        // appearance (or the device setting for .system) decides day/night.
        .seResolvedThemeMode()
    }
}
