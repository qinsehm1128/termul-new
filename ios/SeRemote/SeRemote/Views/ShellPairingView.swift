import SwiftUI

@MainActor
struct ShellPairingView: View {
    let onConnect: (RemoteLink) -> Void

    private enum Mode: String, CaseIterable, Identifiable {
        case scan, paste
        var id: String { rawValue }
    }

    @Environment(\.dismiss) private var dismiss
    @State private var mode: Mode = .scan
    @State private var draft = ShellPairingDraft()
    @State private var showScanner = false
    @State private var scannedValue: String?
    @State private var failureCount = 0
    @FocusState private var urlFocused: Bool

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: SeSpace.xl) {
                    introduction
                    Picker("Pairing method", selection: $mode) {
                        Text("Scan").tag(Mode.scan)
                        Text("Paste URL").tag(Mode.paste)
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("pairing.method")

                    if mode == .scan {
                        scanContent
                    } else {
                        pasteContent
                    }

                    if let error = draft.errorMessage {
                        SeErrorBanner(message: error)
                            .accessibilityIdentifier("pairing.error")
                    }

                    Text(
                        "Connect only to a desktop you trust. The access secret stays on this device and is sent only to that host."
                    )
                    .font(SeFont.support)
                    .foregroundStyle(SeTheme.secondaryInk)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
                .padding(SeSpace.screenInset)
                .frame(maxWidth: .infinity)
            }
            .background(SeTheme.canvas)
            .foregroundStyle(SeTheme.ink)
            .navigationTitle("Pair a host")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: mode) { _, newMode in
                urlFocused = newMode == .paste
            }
            .seHaptic(.error, trigger: failureCount)
            .fullScreenCover(isPresented: $showScanner, onDismiss: acceptScan) {
                ScannerView { raw in
                    scannedValue = raw
                    showScanner = false
                }
            }
        }
        .tint(SeTheme.accent)
    }

    private var introduction: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            Text("CONNECT A HOST")
                .font(SeFont.label)
                .tracking(1.2)
                .foregroundStyle(SeTheme.secondaryInk)
            Text("Bring your desktop along.")
                .font(SeFont.screenTitle)
                .accessibilityAddTraits(.isHeader)
            Text("Open SeRemote on your desktop, then scan its QR code or copy its access URL.")
                .font(SeFont.body)
                .foregroundStyle(SeTheme.secondaryInk)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var scanContent: some View {
        VStack(alignment: .leading, spacing: SeSpace.md) {
            Label("Scan the desktop QR code", systemImage: "qrcode.viewfinder")
                .font(SeFont.sectionTitle)
            Text("Camera access is requested only when you open the scanner. You can always paste the URL instead.")
                .font(SeFont.support)
                .foregroundStyle(SeTheme.secondaryInk)
            Button {
                showScanner = true
            } label: {
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
                .font(SeFont.rowTitle)
            SecureField("https://host.example/#access_token=…", text: $draft.url)
                .font(SeFont.body)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .privacySensitive()
                .focused($urlFocused)
                .submitLabel(.go)
                .onSubmit { submit() }
                .padding(SeSpace.sm)
                .frame(minHeight: SeSpace.hitTarget, alignment: .leading)
                .background(SeTheme.elevated, in: RoundedRectangle(cornerRadius: SeRadius.tool))
                .overlay {
                    RoundedRectangle(cornerRadius: SeRadius.tool)
                        .strokeBorder(SeTheme.stroke, lineWidth: 1)
                }
                .accessibilityLabel("Desktop access URL")
                .accessibilityIdentifier("pairing.url")

            Text("Use the complete http or https link, including its access secret.")
                .font(SeFont.support)
                .foregroundStyle(SeTheme.secondaryInk)

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
        // Parse after camera dismissal, so errors stay in the pairing sheet and
        // a successful handoff never races two presentations.
        submit()
        if draft.errorMessage != nil { mode = .paste }
    }

    private func submit() {
        guard let link = draft.parse() else {
            failureCount += 1
            return
        }
        urlFocused = false
        // The workspace owns success feedback after an actual host connection.
        onConnect(link)
    }
}
