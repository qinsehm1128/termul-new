import SwiftTerm
import SwiftUI
import UIKit

struct TerminalScreen: UIViewRepresentable {
    @Environment(\.remoteDeviceLink) private var deviceLink
    private var allowsInput: Bool { deviceLink?.allows("terminal.input") ?? true }

    var terminalId: String
    var hostCols: Int
    var lockToHostCols: Bool
    /// The grid the host replay about to be fed was authored at. `nil` when the
    /// canvas is not carrying a replay (a resume) or the host did not state it.
    var replayGrid: TerminalReplayGrid? = nil
    var textScale: CGFloat
    var onSend: (String) -> Void
    var onResize: (Int, Int) -> Void
    var onReady: (@escaping (Data) -> Void) -> Void
    var onTextScaleChange: (CGFloat, Bool) -> Void
    var focusToken: UInt64 = 0
    var blurToken: UInt64 = 0
    /// The workbench's keyboard accessory row. `nil` keeps the historical
    /// behavior (no accessory); the hosting controller is retained by the
    /// coordinator so the row cannot be deallocated while the keyboard is up.
    var inputAccessory: AnyView? = nil

    func makeCoordinator() -> Coordinator {
        Coordinator(onSend: onSend, onResize: onResize, onTextScaleChange: onTextScaleChange)
    }

    func makeUIView(context: Context) -> PhoneTerminalView {
        let view = PhoneTerminalView(frame: .zero)
        view.allowsInput = allowsInput
        context.coordinator.allowsInput = allowsInput
        view.terminalDelegate = context.coordinator
        view.inputAccessoryView = nil
        context.coordinator.installAccessory(inputAccessory, on: view)
        view.hostCols = hostCols
        view.lockToHostCols = lockToHostCols
        view.textScale = textScale
        view.onTextScaleChange = { scale, settled in
            context.coordinator.onTextScaleChange(scale, settled)
        }
        context.coordinator.attach(view)
        context.coordinator.replayGrid = replayGrid
        // Buffered output is not replayed here: `onReady` installs the feed and
        // drains the backlog in the same synchronous step, so no byte can fall
        // between the snapshot this struct captured and the installed feed.
        // The coordinator holds those bytes until the canvas has fitted to its
        // own grid: this view was created at a zero size, so SwiftTerm still
        // owns its 80x25 startup grid, and a takeover replay painted there
        // outlives the shrink inside the alternate buffer.
        onReady { data in
            context.coordinator.receive(data, into: view)
        }
        return view
    }

    func updateUIView(_ uiView: PhoneTerminalView, context: Context) {
        context.coordinator.replayGrid = replayGrid
        // A canvas mounted without a size report still must not hold the replay
        // forever; a laid-out view is enough to interpret it at its own width.
        context.coordinator.flushFeedIfFitted(uiView)
        uiView.allowsInput = allowsInput
        context.coordinator.allowsInput = allowsInput
        context.coordinator.onSend = onSend
        context.coordinator.onResize = onResize
        context.coordinator.onTextScaleChange = onTextScaleChange
        uiView.terminalDelegate = context.coordinator
        // hostCols only drives the desktop column fit; assigning it in phone
        // mode would invalidate the fit on every host reply for no gain.
        uiView.lockToHostCols = lockToHostCols
        if lockToHostCols {
            uiView.hostCols = max(hostCols, 20)
        }
        uiView.textScale = textScale
        uiView.onTextScaleChange = { scale, settled in
            context.coordinator.onTextScaleChange(scale, settled)
        }
        // The workbench supplies an accessory row; every other surface keeps
        // the historical nil so no keyboard strip appears unexpectedly.
        context.coordinator.installAccessory(inputAccessory, on: uiView)
        if inputAccessory == nil, uiView.inputAccessoryView != nil {
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
        var allowsInput = true
        var lastBlurToken: UInt64 = 0
        private var lastCols = 0
        private var lastRows = 0
        /// Grid the host replay was authored at, supplied by the surface.
        var replayGrid: TerminalReplayGrid?
        /// Bytes that arrived before this canvas owned a grid. The store hands
        /// the whole buffered replay over at mount, when SwiftTerm is still on
        /// its 80x25 startup grid; painting it there authors the screen at a
        /// grid the canvas is about to leave.
        private var pendingFeed = Data()
        /// True once the replay was applied at the canvas's own grid. From then
        /// on host bytes are live and go straight to the emulator.
        private(set) var didApplyReplay = false
        /// A canvas that never reports a size must not buffer without bound.
        private static let maxPendingFeed = 4 * 1024 * 1024
        /// Strong ownership: UIKit holds the accessory view weakly enough that
        /// a dropped hosting controller would blank the strip mid-typing.
        private var accessoryHost: UIHostingController<AnyView>?

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
            // Seed sentinels, not the view's current grid: a freshly created
            // view still reports SwiftTerm's default 80x24 before its first
            // layout fit. Seeding the real values would swallow the first
            // sizeChanged when the fitted grid happens to equal 80x24 — and
            // that report is the only thing that arms the takeover push.
            lastCols = 0
            lastRows = 0
        }

        /// Host output for this canvas. Everything before the canvas owns a
        /// grid is held back: the replay's cursor moves and wraps are relative
        /// to the grid it was authored at, and the phone's fitted grid is only
        /// known after the first layout pass.
        func receive(_ data: Data, into view: PhoneTerminalView) {
            if didApplyReplay {
                view.feed(byteArray: [UInt8](data)[...])
                return
            }
            pendingFeed.append(data)
            if pendingFeed.count > Self.maxPendingFeed {
                applyPendingFeed(into: view)
            }
        }

        /// Apply the held replay once the canvas has a real, laid-out grid.
        /// SwiftTerm resizes the emulator before it reports `sizeChanged`, so
        /// at this point the bytes are interpreted at the width the user sees.
        func flushFeedIfFitted(_ view: PhoneTerminalView) {
            guard !didApplyReplay,
                  view.bounds.width > 8,
                  view.bounds.height > 8
            else { return }
            applyPendingFeed(into: view)
        }

        private func applyPendingFeed(into view: PhoneTerminalView) {
            didApplyReplay = true
            guard !pendingFeed.isEmpty else { return }
            let data = pendingFeed
            pendingFeed = Data()
            view.feed(byteArray: [UInt8](data)[...])
            dropDesktopFrameIfNeeded(in: view)
        }

        /// A replay authored at another grid cannot rebuild a full-screen
        /// program. SwiftTerm's alternate buffer is neither reflowed nor trimmed
        /// when the canvas shrinks from the desktop grid to the phone grid, so
        /// the desktop-laid-out frame stays in the buffer and the program's
        /// next (incremental) repaint only overwrites part of it: broken lines
        /// and a mix of old and new content. Drop that frame instead; the fit
        /// below resizes the PTY, and the program repaints on SIGWINCH.
        ///
        /// The normal buffer keeps its replay: it is reflowed to the new width,
        /// so shell scrollback survives a takeover.
        private func dropDesktopFrameIfNeeded(in view: PhoneTerminalView) {
            guard let replayGrid else { return }
            let terminal = view.getTerminal()
            guard terminal.isCurrentBufferAlternate else { return }
            guard terminal.cols != replayGrid.cols || terminal.rows != replayGrid.rows else { return }
            // Through the view, so the cleared screen is queued for display
            // instead of waiting for the next host byte.
            view.feed(text: Self.takeoverScreenHygiene)
            HostLog.session.info("Dropped a takeover frame authored at another grid")
        }

        /// Origin mode off, scroll region reset, SGR reset, clear, home. The
        /// program's repaint after the takeover resize owns the screen again.
        private static let takeoverScreenHygiene = "\u{1b}[?6l\u{1b}[r\u{1b}[0m\u{1b}[2J\u{1b}[H"

        /// Mount or refresh the terminal's keyboard accessory row. A hosting
        /// controller is created once and retained; updates only swap its
        /// `rootView`, so the strip survives key taps and focus changes.
        func installAccessory(_ content: AnyView?, on view: PhoneTerminalView) {
            guard let content else {
                if accessoryHost != nil {
                    accessoryHost = nil
                    view.inputAccessoryView = nil
                }
                return
            }
            if let host = accessoryHost {
                host.rootView = content
                return
            }
            let host = UIHostingController(rootView: content)
            host.view.backgroundColor = .clear
            host.view.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: 50)
            host.view.autoresizingMask = [.flexibleWidth]
            accessoryHost = host
            view.inputAccessoryView = host.view
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            guard newCols != lastCols || newRows != lastRows else { return }
            lastCols = newCols
            lastRows = newRows
            if let view = source as? PhoneTerminalView {
                // SwiftTerm already resized the emulator, so the held replay can
                // now be interpreted at the grid the user actually sees.
                flushFeedIfFitted(view)
            }
            onResize(newCols, newRows)
        }

        func setTerminalTitle(source: TerminalView, title: String) {}

        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            guard allowsInput else { return }
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
    var allowsInput = true {
        didSet {
            if !allowsInput, isFirstResponder { _ = resignFirstResponder() }
        }
    }

    override var canBecomeFirstResponder: Bool { allowsInput && super.canBecomeFirstResponder }

    override func paste(_ sender: Any?) {
        guard allowsInput else { return }
        super.paste(sender)
    }

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
    /// The fit the current bounds resolved to. Phone mode uses its font size
    /// (the grid then follows from the cell metrics); desktop mode also forces
    /// its column count onto the terminal.
    private var pendingFit: TerminalGridFit.Fit?
    private var pinchStart: CGFloat = TerminalTextScale.defaultValue
    private var didInstallPinch = false
    private var didInstallDismissDrag = false
    private var dismissDragFired = false

    /// Downward travel that reads as "swipe the keyboard away".
    private static let dismissDragDistance: CGFloat = 24

    override func didMoveToWindow() {
        super.didMoveToWindow()
        installPinchIfNeeded()
        installDismissDragIfNeeded()
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

    /// Cell metrics of the live face, measured once per layout rather than
    /// hard-coding SF Mono: the fallback face is whatever the system picks.
    private func measuredMetrics() -> (advance: CGFloat, lineHeight: CGFloat) {
        let sample = UIFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        let advance = max("W".size(withAttributes: [.font: sample]).width / 12, 0.1)
        // Round the row box up: a row taller than SwiftTerm's own cell height
        // would clip the bottom line and its cursor.
        let lineHeight = max(sample.lineHeight.rounded(.up) / 12, 0.5)
        return (advance, lineHeight)
    }

    private func applyColumnFit() {
        let size = bounds.size
        guard size.width > 8 else { return }
        let metrics = measuredMetrics()
        let fit: TerminalGridFit.Fit
        let mode: String
        if lockToHostCols {
            mode = "desktop"
            fit = TerminalGridFit.desktopFit(
                size: size,
                hostCols: hostCols,
                textScale: textScale,
                advanceRatio: metrics.advance,
                lineHeightRatio: metrics.lineHeight
            )
        } else {
            mode = "phone"
            fit = TerminalGridFit.phoneFit(
                size: size,
                textScale: textScale,
                advanceRatio: metrics.advance,
                lineHeightRatio: metrics.lineHeight
            )
        }
        // Height belongs in the key: the keyboard showing/hiding changes only
        // the rows, and the phone grid must be recomputed for it.
        let key = "\(mode):\(Int(size.width.rounded()))x\(Int(size.height.rounded())):\(fit.cols)x\(fit.rows):\(Int((textScale * 100).rounded()))"
        guard key != lastFitKey else { return }
        lastFitKey = key
        pendingFit = fit
        if abs(font.pointSize - fit.fontSize) >= 0.25 {
            font = UIFont.monospacedSystemFont(ofSize: fit.fontSize, weight: .regular)
        }
    }

    /// Hold the host column count in desktop mode.
    ///
    /// Phone mode must not call `resize` here: `TerminalView.resize` runs
    /// `softReset`, which clears cursor-keypad and origin modes — a running TUI
    /// would break on every keyboard toggle. SwiftTerm's own layout fit already
    /// lays out the exact grid `TerminalGridFit` sized the font for, and reports
    /// it through `sizeChanged`, which is what reaches the host.
    private func lockGridIfNeeded() {
        guard lockToHostCols, bounds.height > 8, let fit = pendingFit else { return }
        let cols = max(fit.cols, 1)
        let rows = max(fit.rows, 1)
        let terminal = getTerminal()
        if terminal.cols != cols || terminal.rows != rows {
            resize(cols: cols, rows: rows)
        }
    }

    // MARK: - Keyboard dismissal

    /// A downward drag over the canvas drops the keyboard, matching the
    /// `scrollDismissesKeyboard` gesture the rest of the app uses. Recognition
    /// runs alongside SwiftTerm's scroll/selection pans and the dock's field.
    private func installDismissDragIfNeeded() {
        guard !didInstallDismissDrag else { return }
        didInstallDismissDrag = true
        let drag = UIPanGestureRecognizer(target: self, action: #selector(handleDismissDrag))
        drag.cancelsTouchesInView = false
        drag.delegate = self
        addGestureRecognizer(drag)
    }

    @objc
    private func handleDismissDrag(_ gesture: UIPanGestureRecognizer) {
        switch gesture.state {
        case .began:
            dismissDragFired = false
        case .changed:
            guard !dismissDragFired else { return }
            let translation = gesture.translation(in: self)
            // Vertical-only: a horizontal pan belongs to SwiftTerm selection or
            // to the accessory strip, never to keyboard dismissal.
            guard translation.y >= Self.dismissDragDistance,
                  abs(translation.y) > 2 * abs(translation.x)
            else { return }
            dismissDragFired = true
            dismissKeyboard()
        case .ended, .cancelled, .failed:
            dismissDragFired = false
        default:
            break
        }
    }

    private func dismissKeyboard() {
        // Either the canvas or the dock's command field can own the keyboard;
        // resign the whole chain so both paths converge on one hidden state.
        if isFirstResponder {
            _ = resignFirstResponder()
        }
        KeyboardGuard.resign()
    }
}

extension PhoneTerminalView: UIGestureRecognizerDelegate {
    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        // SwiftTerm scrolls scrollback with its own pans; dismissing on a
        // downward drag must not steal those touches.
        true
    }
}
