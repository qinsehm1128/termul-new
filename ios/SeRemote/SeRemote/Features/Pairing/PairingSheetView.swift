import SwiftUI

/// The phone consumes the desktop's QR code or complete access URL. Parsing is
/// not a handshake: success feedback belongs to the connected workspace.
@MainActor
struct PairingSheetView: View {
    let onConnect: (RemoteLink) -> Void

    private enum Mode: String, CaseIterable, Identifiable {
        case scan, paste
        var id: String { rawValue }
        var title: String {
            self == .scan ? String(localized: "Scan QR code") : String(localized: "Paste link")
        }
    }

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.dismiss) private var dismiss

    @State private var method: Mode = .scan
    @State private var draft = ShellPairingDraft()
    @State private var showScanner = false
    @State private var scannedValue: String?
    @State private var failureCount = 0
    @FocusState private var urlFocused: Bool

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeSheetHeader(title: String(localized: "Pair a new host"), onClose: { dismiss() })

                HStack(alignment: .center, spacing: 6) {
                    SeSegmented(
                        segments: Mode.allCases.map { SeSegment($0.rawValue, $0.title) },
                        selection: Binding(get: { method.rawValue }, set: { method = Mode(rawValue: $0) ?? .scan }),
                        identifier: "pairing.method"
                    )
                    SeInfoTip(text: String(localized: "Open SeRemote on the Mac, then Remote Access, and scan the code. You confirm the connection on the host. Credentials stay in this phone's keychain; the first connection is read-only and writes are approved one by one."))
                }
                .padding(.top, 14)

                if method == .scan { scanContent } else { pasteContent }

                if let error = draft.errorMessage {
                    SeErrorBanner(message: error)
                        .padding(.top, 12)
                        .accessibilityIdentifier("pairing.error")
                }

                SeSolidButton(title: method == .scan ? String(localized: "Start scanning") : String(localized: "Connect"), icon: method == .scan ? .scan : .arrowRight) {
                    method == .scan ? (showScanner = true) : submit()
                }
                .disabled(method == .paste && !draft.canSubmit)
                .accessibilityIdentifier(method == .scan ? "pairing.scan" : "pairing.connect")
                .padding(.top, 18)
            }
            .padding(.horizontal, 18)
            .padding(.top, 10)
            .padding(.bottom, 26)
        }
        .background(mode.sheetSurface)
        .foregroundStyle(palette.ink)
        .scrollDismissesKeyboard(.interactively)
        .presentationCornerRadius(SeNocturneRadius.sheet)
        .onChange(of: method) { _, newMode in urlFocused = newMode == .paste }
        .seHaptic(.error, trigger: failureCount)
        .fullScreenCover(isPresented: $showScanner, onDismiss: acceptScan) {
            ScannerView { raw in
                scannedValue = raw
                showScanner = false
            }
        }
    }

    private var scanContent: some View {
        QRBox()
            .frame(width: 176, height: 176)
            .frame(maxWidth: .infinity)
            .padding(.top, 20)
            .accessibilityHidden(true)
    }

    private var pasteContent: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Desktop access link")
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
            HStack(spacing: 8) {
                SeIconView(icon: .link, size: 15, stroke: 1.6, color: palette.inkTertiary)
                SecureField("https://host.example/#access_token=…", text: $draft.url)
                    .font(SeNocturneType.font(12, mono: true))
                    .foregroundStyle(palette.ink)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .keyboardType(.URL)
                    .privacySensitive()
                    .focused($urlFocused)
                    .submitLabel(.go)
                    .onSubmit { submit() }
                    .accessibilityIdentifier("pairing.url")
                    .accessibilityHint(String(localized: "Use the complete http or https link, including its access key."))
            }
            .padding(.horizontal, 12)
            .frame(height: 46)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(mode.insetStroke, lineWidth: 1)
            }
        }
        .padding(.top, 14)
    }

    private func acceptScan() {
        guard let raw = scannedValue else { return }
        scannedValue = nil
        draft.url = raw
        submit()
        if draft.errorMessage != nil { method = .paste }
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

/// A static, decorative QR frame in the prototype's bone-white box.
private struct QRBox: View {
    private let cells: [Bool] = (0..<144).map { index in
        // Deterministic pattern; no data is encoded here.
        let row = index / 12
        let column = index % 12
        let finder = (row < 3 && column < 3) || (row < 3 && column > 8) || (row > 8 && column < 3)
        return finder || (row * 7 + column * 11 + row * column) % 3 == 0
    }

    var body: some View {
        GeometryReader { geometry in
            let side = min(geometry.size.width, geometry.size.height)
            let cell = (side - 24) / 12
            ZStack {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color(hex: 0xF2F0E8))
                VStack(spacing: 0) {
                    ForEach(0..<12, id: \.self) { row in
                        HStack(spacing: 0) {
                            ForEach(0..<12, id: \.self) { column in
                                Rectangle()
                                    .fill(cells[row * 12 + column] ? Color(hex: 0x0B0D12) : Color.clear)
                                    .frame(width: cell, height: cell)
                            }
                        }
                    }
                }
                .padding(12)
            }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color(hex: 0xF2F0E8), lineWidth: 1)
        }
        .accessibilityLabel(String(localized: "QR code"))
    }
}
