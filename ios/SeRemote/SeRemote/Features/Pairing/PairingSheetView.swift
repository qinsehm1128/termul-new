import SwiftUI

/// The phone consumes the desktop's QR code or complete access URL. Parsing is
/// not a handshake: success feedback belongs to the connected workspace.
@MainActor
struct PairingSheetView: View {
    let onConnect: (RemoteLink) -> Void

    private enum Mode: String, CaseIterable, Identifiable {
        case scan, paste
        var id: String { rawValue }
    }

    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var mode: Mode = .scan
    @State private var draft = ShellPairingDraft()
    @State private var showScanner = false
    @State private var scannedValue: String?
    @State private var failureCount = 0
    @FocusState private var urlFocused: Bool
    @ScaledMetric(relativeTo: .subheadline) private var secondarySize = SeNocturneType.secondarySize
    @ScaledMetric(relativeTo: .title3) private var titleSize = SeNocturneType.sheetTitleSize

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.lg) {
                header
                Text("Open SeRemote on your desktop, then scan its QR code or copy its access URL.")
                    .font(SeNocturneType.font(secondarySize))
                    .foregroundStyle(palette.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)

                Picker("Pairing method", selection: $mode) {
                    Text("Scan").tag(Mode.scan)
                    Text("Paste URL").tag(Mode.paste)
                }
                .pickerStyle(.segmented)
                .frame(minHeight: SeSpace.hitTarget)
                .accessibilityIdentifier("pairing.method")

                VStack(alignment: .leading, spacing: SeSpace.md) {
                    if mode == .scan { scanContent } else { pasteContent }
                }
                .padding(SeSpace.lg)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }

                if let error = draft.errorMessage {
                    SeErrorBanner(message: error)
                        .accessibilityIdentifier("pairing.error")
                }
                Label {
                    Text("Connect only to a desktop you trust. The access secret stays on this device and is sent only to that host.")
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "lock")
                }
                .font(SeNocturneType.font(secondarySize))
                .foregroundStyle(palette.inkSecondary)
            }
            .padding(SeSpace.lg)
            .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(palette.surface.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .tint(palette.accent)
        .scrollDismissesKeyboard(.interactively)
        .presentationCornerRadius(SeNocturneRadius.sheet)
        .onChange(of: mode) { _, newMode in urlFocused = newMode == .paste }
        .seHaptic(.error, trigger: failureCount)
        .fullScreenCover(isPresented: $showScanner, onDismiss: acceptScan) {
            ScannerView { raw in
                scannedValue = raw
                showScanner = false
            }
        }
    }

    private var header: some View {
        HStack(spacing: SeSpace.md) {
            Text("Pair a host")
                .font(SeNocturneType.font(titleSize, weight: .semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer()
            Button { dismiss() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Cancel")
        }
    }

    private var scanContent: some View {
        VStack(alignment: .leading, spacing: SeSpace.md) {
            Image(systemName: "qrcode.viewfinder")
                .font(.system(size: 52, weight: .light))
                .foregroundStyle(palette.inkSecondary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, SeSpace.lg)
                .accessibilityHidden(true)
            Text("Scan the desktop QR code")
                .font(SeNocturneType.font(titleSize, weight: .semibold))
            Text("Camera access is requested only when you open the scanner. You can always paste the URL instead.")
                .font(SeNocturneType.font(secondarySize))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
            Button { showScanner = true } label: {
                Label("Open scanner", systemImage: "qrcode.viewfinder")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(SePrimaryButtonStyle())
            .accessibilityIdentifier("pairing.scan")
        }
    }

    private var pasteContent: some View {
        VStack(alignment: .leading, spacing: SeSpace.md) {
            Text("Desktop access URL")
                .font(SeNocturneType.font(secondarySize))
            SecureField("https://host.example/#access_token=…", text: $draft.url)
                .font(SeNocturneType.font(secondarySize))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .privacySensitive()
                .focused($urlFocused)
                .submitLabel(.go)
                .onSubmit { submit() }
                .padding(SeSpace.sm)
                .frame(minHeight: SeSpace.hitTarget, alignment: .leading)
                .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
                .accessibilityLabel("Desktop access URL")
                .accessibilityIdentifier("pairing.url")
            Text("Use the complete http or https link, including its access secret.")
                .font(SeNocturneType.font(secondarySize))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
            Button(action: submit) {
                Label("Connect", systemImage: "arrow.right")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(SePrimaryButtonStyle())
            .disabled(!draft.canSubmit)
            .accessibilityIdentifier("pairing.connect")
        }
    }

    private func acceptScan() {
        guard let raw = scannedValue else { return }
        scannedValue = nil
        draft.url = raw
        // Dismiss camera before parsing or connecting to avoid presentation races.
        submit()
        if draft.errorMessage != nil { mode = .paste }
    }

    private func submit() {
        guard let link = draft.parse() else {
            failureCount += 1
            return
        }
        urlFocused = false
        onConnect(link)
    }
}
