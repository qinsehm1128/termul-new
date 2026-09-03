import SwiftTerm
import SwiftUI
import UIKit

struct TerminalScreen: UIViewRepresentable {
    var terminalId: String
    var buffered: Data
    var hostCols: Int
    var lockToHostCols: Bool
    var textScale: CGFloat
    var onSend: (String) -> Void
    var onResize: (Int, Int) -> Void
    var onReady: (@escaping (Data) -> Void) -> Void
    var onTextScaleChange: (CGFloat, Bool) -> Void
    var focusToken: UInt64 = 0
    var blurToken: UInt64 = 0

    func makeCoordinator() -> Coordinator {
        Coordinator(onSend: onSend, onResize: onResize, onTextScaleChange: onTextScaleChange)
    }

    func makeUIView(context: Context) -> PhoneTerminalView {
        let view = PhoneTerminalView(frame: .zero)
        view.terminalDelegate = context.coordinator
        view.inputAccessoryView = nil
        view.hostCols = hostCols
        view.lockToHostCols = lockToHostCols
        view.textScale = textScale
        view.onTextScaleChange = { scale, settled in
            context.coordinator.onTextScaleChange(scale, settled)
        }
        context.coordinator.attach(view)
        if !buffered.isEmpty {
            view.feed(byteArray: [UInt8](buffered)[...])
        }
        onReady { data in
            view.feed(byteArray: [UInt8](data)[...])
        }
        return view
    }

    func updateUIView(_ uiView: PhoneTerminalView, context: Context) {
        context.coordinator.onSend = onSend
        context.coordinator.onResize = onResize
        context.coordinator.onTextScaleChange = onTextScaleChange
        uiView.terminalDelegate = context.coordinator
        uiView.hostCols = max(hostCols, 20)
        uiView.lockToHostCols = lockToHostCols
        uiView.textScale = textScale
        uiView.onTextScaleChange = { scale, settled in
            context.coordinator.onTextScaleChange(scale, settled)
        }
        if uiView.inputAccessoryView != nil {
            uiView.inputAccessoryView = nil
        }
        if blurToken != context.coordinator.lastBlurToken {
            context.coordinator.lastBlurToken = blurToken
            _ = uiView.resignFirstResponder()
        }
        if focusToken != context.coordinator.lastFocusToken {
            context.coordinator.lastFocusToken = focusToken
            _ = uiView.becomeFirstResponder()
        }
    }

    @MainActor
    final class Coordinator: NSObject, TerminalViewDelegate {
        var onSend: (String) -> Void
        var onResize: (Int, Int) -> Void
        var onTextScaleChange: (CGFloat, Bool) -> Void
        var lastFocusToken: UInt64 = 0
        var lastBlurToken: UInt64 = 0
        private var lastCols = 0
        private var lastRows = 0

        init(
            onSend: @escaping (String) -> Void,
            onResize: @escaping (Int, Int) -> Void,
            onTextScaleChange: @escaping (CGFloat, Bool) -> Void
        ) {
            self.onSend = onSend
            self.onResize = onResize
            self.onTextScaleChange = onTextScaleChange
        }

        func attach(_ view: TerminalView) {
            let cols = view.getTerminal().cols
            let rows = view.getTerminal().rows
            lastCols = cols
            lastRows = rows
            onResize(cols, rows)
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            guard newCols != lastCols || newRows != lastRows else { return }
            lastCols = newCols
            lastRows = newRows
            onResize(newCols, newRows)
        }

        func setTerminalTitle(source: TerminalView, title: String) {}

        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            onSend(String(decoding: data, as: UTF8.self))
        }

        func scrolled(source: TerminalView, position: Double) {}

        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
            guard let url = URL(string: link) else { return }
            UIApplication.shared.open(url)
        }

        func bell(source: TerminalView) {}

        func clipboardCopy(source: TerminalView, content: Data) {
            UIPasteboard.general.string = String(decoding: content, as: UTF8.self)
        }

        func clipboardRead(source: TerminalView) -> Data? { nil }

        func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}

        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    }
}

/// Phone mode sizes the live grid to the viewport. Desktop mode fits host columns.
final class PhoneTerminalView: TerminalView {
    var hostCols: Int = 80 {
        didSet {
            if hostCols != oldValue {
                invalidateFit()
            }
        }
    }

    var lockToHostCols = true {
        didSet {
            if lockToHostCols != oldValue {
                invalidateFit()
            }
        }
    }

    var textScale: CGFloat = TerminalTextScale.defaultValue {
        didSet {
            if abs(textScale - oldValue) >= 0.01 {
                invalidateFit()
            }
        }
    }

    var onTextScaleChange: ((CGFloat, Bool) -> Void)?

    private var lastFitKey = ""
    private var pinchStart: CGFloat = TerminalTextScale.defaultValue
    private var didInstallPinch = false

    override func didMoveToWindow() {
        super.didMoveToWindow()
        installPinchIfNeeded()
    }

    override func layoutSubviews() {
        applyColumnFit()
        super.layoutSubviews()
        lockGridIfNeeded()
    }

    private func installPinchIfNeeded() {
        guard !didInstallPinch else { return }
        didInstallPinch = true
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch))
        pinch.cancelsTouchesInView = false
        addGestureRecognizer(pinch)
    }

    @objc
    private func handlePinch(_ gesture: UIPinchGestureRecognizer) {
        switch gesture.state {
        case .began:
            pinchStart = textScale
        case .changed:
            let live = TerminalTextScale.clamp(pinchStart * gesture.scale)
            textScale = live
            onTextScaleChange?(live, false)
        case .ended, .cancelled:
            let snapped = TerminalTextScale.snap(pinchStart * gesture.scale)
            textScale = snapped
            TerminalTextScale.current = snapped
            onTextScaleChange?(snapped, true)
            HostLog.session.info("Terminal text scale \(snapped, privacy: .public)")
        default:
            break
        }
    }

    private func invalidateFit() {
        lastFitKey = ""
        setNeedsLayout()
    }

    private func applyColumnFit() {
        let width = bounds.width
        guard width > 8 else { return }
        if lockToHostCols {
            let cols = max(hostCols, 20)
            let key = "desktop:\(Int(width.rounded())):\(cols):\(Int((textScale * 100).rounded()))"
            guard key != lastFitKey else { return }
            lastFitKey = key
            let sample = UIFont.monospacedSystemFont(ofSize: 12, weight: .regular)
            let sampleWidth = max("W".size(withAttributes: [.font: sample]).width, 1)
            let fitted = 12 * (width / (sampleWidth * CGFloat(cols))) * textScale
            let size = min(22, max(6, (fitted * 2).rounded() / 2))
            if abs(font.pointSize - size) >= 0.25 {
                font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
            }
            return
        }
        let key = "phone:\(Int(width.rounded())):\(Int((textScale * 100).rounded()))"
        guard key != lastFitKey else { return }
        lastFitKey = key
        let size = min(20, max(10, (13 * textScale * 2).rounded() / 2))
        if abs(font.pointSize - size) >= 0.25 {
            font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        }
    }

    private func lockGridIfNeeded() {
        guard lockToHostCols, bounds.height > 8 else { return }
        let cols = max(hostCols, 20)
        let cellHeight = max(font.lineHeight, 1)
        let rows = max(4, Int(bounds.height / cellHeight))
        let terminal = getTerminal()
        if terminal.cols != cols || terminal.rows != rows {
            resize(cols: cols, rows: rows)
        }
    }
}
