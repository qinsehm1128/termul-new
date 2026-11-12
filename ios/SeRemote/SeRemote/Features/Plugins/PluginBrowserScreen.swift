import SwiftUI
import UIKit

/// The desktop's remote-access web app, opened on its plugins page.
///
/// Chrome is intentionally minimal: close, title, reload, and a thin load
/// indicator. Failures show a retry that reloads the bootstrap URL because the
/// renderer strips the credential from the page URL once it has consumed it.
struct PluginBrowserScreen: View {
    let origin: URL
    let token: String?
    /// The presenting context's authenticated client, used only for a
    /// revocation preflight.
    var http: HostHTTP? = nil

    @Environment(\.dismiss) private var dismiss
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var theme

    @State private var attempt = 0
    @State private var progress: Double = 0
    @State private var isLoading = false
    @State private var loadFailed = false

    private var bootstrap: URL? {
        PluginBrowserPolicy.bootstrapURL(origin: origin, token: token)
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            progressBar
            ZStack {
                Rectangle().fill(palette.canvas)
                if let bootstrap, !loadFailed {
                    PluginWebView(
                        bootstrapURL: bootstrap,
                        origin: origin,
                        backgroundColor: UIColor(palette.canvas),
                        progress: $progress,
                        isLoading: $isLoading,
                        loadFailed: $loadFailed
                    )
                    .id(attempt)
                } else {
                    errorState
                }
            }
        }
        .background(palette.canvas.ignoresSafeArea())
        .onAppear {
            if bootstrap != nil { isLoading = true; progress = 0 }
        }
        .task {
            HostLog.ui.info("plugins browser open")
            await runPreflight()
        }
    }

    // MARK: - Chrome

    private var topBar: some View {
        HStack(spacing: 8) {
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .frame(width: 32, height: 32)
                    .background(theme.insetFill, in: Circle())
                    .overlay { Circle().strokeBorder(theme.insetStroke, lineWidth: 1) }
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Close"))

            Spacer(minLength: 0)
            Text("Plugins")
                .font(SeNocturneType.font(15, weight: .semibold))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            Spacer(minLength: 0)

            Button(action: reload) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .frame(width: 32, height: 32)
                    .background(theme.insetFill, in: RoundedRectangle(cornerRadius: 9))
                    .overlay { RoundedRectangle(cornerRadius: 9).strokeBorder(theme.insetStroke, lineWidth: 1) }
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(bootstrap == nil)
            .opacity(bootstrap == nil ? 0.4 : 1)
            .accessibilityLabel(Text("Reload"))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 44)
        .background(theme.barSurface)
    }

    @ViewBuilder
    private var progressBar: some View {
        if isLoading && !loadFailed {
            GeometryReader { geometry in
                Rectangle()
                    .fill(palette.accent)
                    .frame(width: max(2, geometry.size.width * min(max(progress, 0), 1)))
                    .frame(maxHeight: .infinity, alignment: .top)
            }
            .frame(height: 2)
            .animation(.linear(duration: 0.15), value: progress)
            .accessibilityHidden(true)
        }
    }

    private var errorState: some View {
        VStack(spacing: 10) {
            SeIconView(icon: .info, size: 22, stroke: 1.6, color: palette.inkTertiary)
            Text(bootstrap == nil ? String(localized: "Pairing needed") : String(localized: "Plugins unavailable"))
                .font(SeNocturneType.headline)
                .foregroundStyle(palette.ink)
                .multilineTextAlignment(.center)
            if bootstrap != nil {
                Button("Retry", action: reload)
                    .buttonStyle(.bordered)
                    .tint(palette.accent)
                    .frame(minWidth: 44, minHeight: SeSpace.hitTarget)
            }
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.canvas)
    }

    // MARK: - Actions

    private func reload() {
        guard bootstrap != nil else { return }
        progress = 0
        isLoading = true
        loadFailed = false
        attempt += 1
    }

    /// A revoked token has to reach the app's existing re-pairing flow, which
    /// `HostHTTP` notifies on a 401/403. Anything else is diagnostic only; the
    /// web view still carries the session.
    private func runPreflight() async {
        guard let http else { return }
        do {
            let _: IpcResult<PluginPreflightProbe> = try await http.getEnvelope(PluginBrowserPolicy.pluginsRoute)
        } catch {
            let failure = error as NSError
            HostLog.ui.debug("plugins preflight: \(failure.domain, privacy: .public) \(failure.code)")
        }
    }
}

/// The preflight only needs the transport to report 401/403; the payload is
/// irrelevant, so an empty object satisfies the envelope's decoded `data`.
private struct PluginPreflightProbe: Decodable {}
