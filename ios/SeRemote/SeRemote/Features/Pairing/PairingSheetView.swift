import SwiftUI

/// The phone consumes the desktop's QR code or complete access URL. Parsing is
/// not a handshake: success feedback belongs to the connected workspace.
@MainActor
struct PairingSheetView: View {
    let onConnect: (RemoteLink) -> Void

    private enum Mode: String, CaseIterable, Identifiable {
        case scan, paste
        var id: String { rawValue }
        var title: String { self == .scan ? "扫码" : "粘贴链接" }
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
                SeSheetHeader(title: "配对新主机", onClose: { dismiss() })

                Text("在 Mac 上打开 SeRemote → 远程访问，扫描这个码。连接要由你在主机上确认。")
                    .font(SeNocturneType.font(11))
                    .foregroundStyle(palette.inkTertiary)
                    .lineSpacing(3)
                    .padding(.top, 4)

                SeSegmented(
                    segments: Mode.allCases.map { SeSegment($0.rawValue, $0.title) },
                    selection: Binding(get: { method.rawValue }, set: { method = Mode(rawValue: $0) ?? .scan }),
                    identifier: "pairing.method"
                )
                .padding(.top, 14)

                if method == .scan { scanContent } else { pasteContent }

                if let error = draft.errorMessage {
                    SeErrorBanner(message: error)
                        .padding(.top, 12)
                        .accessibilityIdentifier("pairing.error")
                }

                steps
                    .padding(.top, 18)

                HStack(spacing: 8) {
                    SeSolidButton(title: method == .scan ? "开始扫码" : "连接", icon: method == .scan ? .scan : .arrowRight) {
                        method == .scan ? (showScanner = true) : submit()
                    }
                    .disabled(method == .paste && !draft.canSubmit)
                    .accessibilityIdentifier(method == .scan ? "pairing.scan" : "pairing.connect")

                    SeOutlineButton(title: method == .scan ? "粘贴链接" : "扫码", icon: method == .scan ? .link : .scan, square: true) {
                        method = method == .scan ? .paste : .scan
                    }
                }
                .padding(.top, 18)

                Text("连接只写入这台手机的钥匙串。首次连接默认只读，写操作逐条许可。")
                    .font(SeNocturneType.font(9.5))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 12)
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
        VStack(spacing: 12) {
            QRBox()
                .frame(width: 176, height: 176)
                .padding(.top, 16)
            Text("把主机的二维码放进框内")
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
        }
        .frame(maxWidth: .infinity)
    }

    private var pasteContent: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("桌面访问链接")
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
            }
            .padding(.horizontal, 12)
            .frame(height: 46)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(mode.insetStroke, lineWidth: 1)
            }
            Text("使用完整的 http 或 https 链接，包含它的访问密钥。")
                .font(SeNocturneType.font(9.5))
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.top, 14)
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 10) {
            step(1, "主机上出现这次请求，确认设备名与地点。")
            step(2, "凭据写入钥匙串，只对这台主机生效。")
            step(3, "首次连接默认只读，写操作逐条许可。")
        }
    }

    private func step(_ number: Int, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(number)")
                .font(SeNocturneType.font(9, mono: true))
                .foregroundStyle(palette.inkTertiary)
                .frame(width: 18, height: 18)
                .overlay {
                    Circle().strokeBorder(mode.strongHairline, lineWidth: 1)
                }
            Text(text)
                .font(SeNocturneType.font(10.5))
                .foregroundStyle(palette.inkSecondary)
                .lineSpacing(2)
            Spacer(minLength: 0)
        }
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
        .accessibilityLabel("二维码")
    }
}
