import SwiftUI
import VisionKit

struct ScannerView: View {
    var onScan: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isSupported, DataScannerViewController.isAvailable {
                    DataScannerRepresentable(onScan: onScan)
                } else {
                    ContentUnavailableView(
                        String(localized: "Camera unavailable"),
                        systemImage: "qrcode.viewfinder",
                        description: Text(String(localized: "Use a physical iPhone, or paste the link instead."))
                    )
                }
            }
            .navigationTitle(String(localized: "Scan QR"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "Close"), systemImage: "xmark") {
                        dismiss()
                    }
                }
            }
        }
    }
}

/// Starts the camera only after the scanner is in a window. Starting in
/// `makeUIViewController` races the keyboard InputUI scene and can abort
/// the app with `No scene exists for identity: com.apple.InputUI.keyboard`.
private struct DataScannerRepresentable: UIViewControllerRepresentable {
    var onScan: (String) -> Void

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
        context.coordinator.startIfReady(scanner)
    }

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        coordinator.handled = true
        if controller.isScanning {
            controller.stopScanning()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onScan: onScan)
    }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        var onScan: (String) -> Void
        weak var controller: DataScannerViewController?
        var handled = false

        init(onScan: @escaping (String) -> Void) {
            self.onScan = onScan
        }

        func startIfReady(_ scanner: DataScannerViewController) {
            guard scanner.view.window != nil, !scanner.isScanning, !handled else { return }
            do {
                try scanner.startScanning()
            } catch {
                HostLog.session.error("QR scanner failed to start")
            }
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
