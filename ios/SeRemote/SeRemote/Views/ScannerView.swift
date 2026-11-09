import SwiftUI
import VisionKit

@MainActor
struct ScannerView: View {
    var onScan: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @Environment(\.sePalette) private var palette
    @State private var cameraError: String?

    var body: some View {
        ZStack {
            Color(hex: 0x05070A).ignoresSafeArea()

            Group {
                if let cameraError {
                    ContentUnavailableView(
                        "摄像头不可用",
                        systemImage: "camera.fill",
                        description: Text(cameraError)
                    )
                    .foregroundStyle(.white)
                } else if DataScannerViewController.isSupported, DataScannerViewController.isAvailable {
                    ZStack {
                        DataScannerRepresentable(onScan: onScan, onError: { cameraError = $0 })
                        ScannerFrameOverlay()
                    }
                } else {
                    ContentUnavailableView(
                        "摄像头不可用",
                        systemImage: "qrcode.viewfinder",
                        description: Text("关闭扫码后，可以粘贴桌面的访问链接。摄像头可能不可用，或已在 iOS 设置里关闭。")
                    )
                    .foregroundStyle(.white)
                }
            }

            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Button {
                        dismiss()
                    } label: {
                        SeIconView(icon: .close, size: 14, stroke: 1.7, color: Color(hex: 0xF2F0E8))
                            .frame(width: 34, height: 34)
                            .background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                            .contentShape(Rectangle())
                            .frame(width: 44, height: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("关闭扫码")
                    VStack(alignment: .leading, spacing: 2) {
                        Text("扫码配对")
                            .font(SeNocturneType.font(19, weight: .semibold))
                            .tracking(-0.5)
                            .foregroundStyle(Color(hex: 0xF2F0E8))
                        Text("SE REMOTE · PAIR")
                            .font(SeNocturneType.font(9, mono: true))
                            .foregroundStyle(Color(hex: 0xA9AEBB))
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 18)
                .padding(.top, 8)
                Spacer()
            }
        }
        .tint(palette.accent)
        .seHaptic(.error, trigger: cameraError)
    }
}

/// Camera viewfinder chrome: dimmed mask with a rounded-rect cutout, corner
/// brackets, and a sweeping scan line. Purely decorative — hit-testing is off
/// so taps still reach the scanner's own barcode-tap recognition.
@MainActor
struct ScannerFrameOverlay: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var sweep: CGFloat = 0

    var body: some View {
        GeometryReader { geo in
            let side = min(geo.size.width * 0.72, geo.size.height * 0.46)
            let rect = CGRect(
                x: (geo.size.width - side) / 2,
                y: geo.size.height * 0.5 - side / 2 - geo.size.height * 0.04,
                width: side,
                height: side
            )
            ZStack {
                maskWithCutout(size: geo.size, cutout: rect)
                CornerBrackets(rect: rect)
                    .stroke(.white.opacity(0.95), lineWidth: 4)
                if !reduceMotion {
                    scanLine(rect: rect)
                }
                Text(String(localized: "Align the QR code inside the frame"))
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.white)
                    .shadow(color: .black.opacity(0.6), radius: 3)
                    .position(x: rect.midX, y: rect.maxY + 34)
            }
        }
        .allowsHitTesting(false)
        .task(id: reduceMotion) {
            var reset = Transaction(animation: nil)
            reset.disablesAnimations = true
            withTransaction(reset) { sweep = 0 }
            guard !reduceMotion else { return }
            await Task.yield()
            guard !Task.isCancelled else { return }
            withAnimation(.linear(duration: 2.4).repeatForever(autoreverses: false)) {
                sweep = 1
            }
        }
    }

    /// Even-odd fill: full-screen dim with a transparent rounded-rect hole.
    private func maskWithCutout(size: CGSize, cutout: CGRect) -> some View {
        Path { path in
            path.addRect(CGRect(origin: .zero, size: size))
            path.addRoundedRect(
                in: cutout.insetBy(dx: 8, dy: 8),
                cornerSize: CGSize(width: 18, height: 18)
            )
        }
        .fill(Color.black.opacity(0.45), style: FillStyle(eoFill: true))
    }

    @ViewBuilder
    private func scanLine(rect: CGRect) -> some View {
        let barHeight: CGFloat = 3
        let travel = max(rect.height - barHeight - 12, 0)
        Capsule()
            .fill(
                LinearGradient(
                    colors: [.clear, .white.opacity(0.9), .clear],
                    startPoint: .leading,
                    endPoint: .trailing
                )
            )
            .frame(width: rect.width - 28, height: barHeight)
            .position(
                x: rect.midX,
                y: rect.minY + 6 + sweep * travel + barHeight / 2
            )
            .opacity(0.9)
    }
}

/// Four L-shaped corner brackets around the cutout.
private struct CornerBrackets: Shape {
    let rect: CGRect
    let length: CGFloat = 26

    nonisolated func path(in _: CGRect) -> Path {
        var path = Path()
        let corners: [(CGPoint, CGPoint, CGPoint)] = [
            // top-leading
            (CGPoint(x: rect.minX, y: rect.minY + length),
             CGPoint(x: rect.minX, y: rect.minY),
             CGPoint(x: rect.minX + length, y: rect.minY)),
            // top-trailing
            (CGPoint(x: rect.maxX - length, y: rect.minY),
             CGPoint(x: rect.maxX, y: rect.minY),
             CGPoint(x: rect.maxX, y: rect.minY + length)),
            // bottom-trailing
            (CGPoint(x: rect.maxX, y: rect.maxY - length),
             CGPoint(x: rect.maxX, y: rect.maxY),
             CGPoint(x: rect.maxX - length, y: rect.maxY)),
            // bottom-leading
            (CGPoint(x: rect.minX + length, y: rect.maxY),
             CGPoint(x: rect.minX, y: rect.maxY),
             CGPoint(x: rect.minX, y: rect.maxY - length)),
        ]
        for (a, corner, b) in corners {
            path.move(to: a)
            path.addLine(to: corner)
            path.addLine(to: b)
        }
        return path
    }
}

/// Starts the camera only after the scanner is in a window. Starting in
/// `makeUIViewController` races the keyboard InputUI scene and can abort
/// the app with `No scene exists for identity: com.apple.InputUI.keyboard`.
@MainActor
private struct DataScannerRepresentable: UIViewControllerRepresentable {
    var onScan: (String) -> Void
    var onError: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighFrameRateTrackingEnabled: false,
            isPinchToZoomEnabled: true,
            isGuidanceEnabled: true,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        context.coordinator.controller = scanner
        let coordinator = context.coordinator
        DispatchQueue.main.async {
            coordinator.startIfReady(scanner)
        }
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {
        context.coordinator.onScan = onScan
        context.coordinator.onError = onError
        context.coordinator.startIfReady(scanner)
    }

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        coordinator.handled = true
        if controller.isScanning {
            controller.stopScanning()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onScan: onScan, onError: onError)
    }

    @MainActor
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        var onScan: (String) -> Void
        var onError: (String) -> Void
        weak var controller: DataScannerViewController?
        var handled = false

        init(onScan: @escaping (String) -> Void, onError: @escaping (String) -> Void) {
            self.onScan = onScan
            self.onError = onError
        }

        func startIfReady(_ scanner: DataScannerViewController) {
            guard scanner.view.window != nil, !scanner.isScanning, !handled else { return }
            do {
                try scanner.startScanning()
            } catch {
                HostLog.session.error("QR scanner failed to start")
                reportUnavailable()
            }
        }

        private func reportUnavailable() {
            guard !handled else { return }
            handled = true
            DispatchQueue.main.async { [weak self] in
                self?.onError(String(localized: "Close the scanner and paste the desktop access URL instead. Camera access may be unavailable or disabled in iOS Settings."))
            }
        }

        func dataScanner(
            _: DataScannerViewController,
            becameUnavailableWithError _: DataScannerViewController.ScanningUnavailable
        ) {
            reportUnavailable()
        }

        private func accept(_ payload: String) {
            guard !handled, !payload.isEmpty else { return }
            handled = true
            if controller?.isScanning == true {
                controller?.stopScanning()
            }
            onScan(payload)
        }

        func dataScanner(
            _: DataScannerViewController,
            didTapOn item: RecognizedItem
        ) {
            if case let .barcode(barcode) = item, let payload = barcode.payloadStringValue {
                accept(payload)
            }
        }

        func dataScanner(
            _: DataScannerViewController,
            didAdd addedItems: [RecognizedItem],
            allItems _: [RecognizedItem]
        ) {
            guard let item = addedItems.first else { return }
            if case let .barcode(barcode) = item, let payload = barcode.payloadStringValue {
                accept(payload)
            }
        }
    }
}
