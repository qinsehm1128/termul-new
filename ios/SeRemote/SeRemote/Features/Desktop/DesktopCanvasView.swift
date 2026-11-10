import SwiftUI
import UIKit

struct DesktopCanvasView: UIViewRepresentable {
    let image: CGImage?
    let displayID: String?
    let mode: DesktopInteractionMode
    let acceptsInput: Bool
    var allowsNavigation = true
    var zoom: CGFloat?
    let onInput: (DesktopInputMessage) -> Void
    let onKeyboard: () -> Void

    func makeUIView(context: Context) -> DesktopCanvasUIView { DesktopCanvasUIView() }

    func updateUIView(_ view: DesktopCanvasUIView, context: Context) {
        view.onInput = onInput
        view.onKeyboard = onKeyboard
        view.configure(
            image: image,
            displayID: displayID,
            mode: mode,
            acceptsInput: acceptsInput,
            allowsNavigation: allowsNavigation,
            zoom: zoom
        )
    }

    static func dismantleUIView(_ view: DesktopCanvasUIView, coordinator: ()) { view.cancelInteraction() }
}

/// The compositor's CGImage goes straight into CALayer.contents. No UIImage
/// serialization, JPEG re-encoding, or full-frame SwiftUI image work per update.
final class DesktopCanvasUIView: UIView, UIGestureRecognizerDelegate {
    private let frameLayer = CALayer()
    private let cursorLayer = CAShapeLayer()
    private var viewports: [String: DesktopLocalViewport] = [:]
    private var pointers: [String: CGPoint] = [:]
    private var displayID: String?
    private var imageSize = CGSize.zero
    private var mode: DesktopInteractionMode = .trackpad
    private var acceptsInput = false
    private var allowsNavigation = true
    private var appliedZoom: CGFloat?
    private var dragging = false
    private var pinching = false
    var onInput: ((DesktopInputMessage) -> Void)?
    var onKeyboard: (() -> Void)?

    private var viewport: DesktopLocalViewport {
        get { displayID.flatMap { viewports[$0] } ?? DesktopLocalViewport() }
        set { if let displayID { viewports[displayID] = newValue } }
    }
    private var pointer: CGPoint {
        get { displayID.flatMap { pointers[$0] } ?? CGPoint(x: 0.5, y: 0.5) }
        set { if let displayID { pointers[displayID] = newValue } }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        clipsToBounds = true
        layer.addSublayer(frameLayer)
        frameLayer.contentsGravity = .resize
        frameLayer.magnificationFilter = .linear
        frameLayer.minificationFilter = .linear
        cursorLayer.path = UIBezierPath(ovalIn: CGRect(x: -7, y: -7, width: 14, height: 14)).cgPath
        cursorLayer.fillColor = UIColor.white.withAlphaComponent(0.25).cgColor
        cursorLayer.strokeColor = UIColor.white.cgColor
        cursorLayer.lineWidth = 2
        cursorLayer.shadowColor = UIColor.black.cgColor
        cursorLayer.shadowOpacity = 0.8
        cursorLayer.shadowRadius = 2
        layer.addSublayer(cursorLayer)
        isAccessibilityElement = true
        accessibilityLabel = String(localized: "Remote desktop canvas")
        accessibilityHint = String(localized: "Use the toolbar to enable control or show gesture help.")

        let single = UITapGestureRecognizer(target: self, action: #selector(tap(_:)))
        let double = UITapGestureRecognizer(target: self, action: #selector(doubleTap(_:)))
        double.numberOfTapsRequired = 2
        single.require(toFail: double)
        let right = UITapGestureRecognizer(target: self, action: #selector(rightTap(_:)))
        right.numberOfTouchesRequired = 2
        let keyboard = UITapGestureRecognizer(target: self, action: #selector(keyboardTap))
        keyboard.numberOfTouchesRequired = 3
        right.require(toFail: keyboard)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(pan(_:)))
        pan.maximumNumberOfTouches = 1
        let scroll = UIPanGestureRecognizer(target: self, action: #selector(scroll(_:)))
        scroll.minimumNumberOfTouches = 2
        scroll.maximumNumberOfTouches = 2
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(pinch(_:)))
        let long = UILongPressGestureRecognizer(target: self, action: #selector(longPress(_:)))
        long.minimumPressDuration = 0.5
        let drag = UILongPressGestureRecognizer(target: self, action: #selector(drag(_:)))
        drag.numberOfTapsRequired = 1 // tap, then hold the second contact and move
        drag.minimumPressDuration = 0.15
        pan.require(toFail: drag)
        long.require(toFail: drag)
        for recognizer in [single, double, right, keyboard, pan, scroll, pinch, long, drag] {
            recognizer.delegate = self
            addGestureRecognizer(recognizer)
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func configure(
        image: CGImage?,
        displayID: String?,
        mode: DesktopInteractionMode,
        acceptsInput: Bool,
        allowsNavigation: Bool = true,
        zoom: CGFloat? = nil
    ) {
        if self.displayID != displayID || self.mode != mode || (self.acceptsInput && !acceptsInput) {
            cancelInteraction()
        }
        self.displayID = displayID
        self.mode = mode
        self.acceptsInput = acceptsInput
        if self.allowsNavigation && !allowsNavigation {
            var reset = viewport
            reset.pan = .zero
            reset.zoom = 1
            viewport = reset
            appliedZoom = nil
        }
        self.allowsNavigation = allowsNavigation
        if let zoom, appliedZoom != zoom, displayID != nil {
            var state = viewport
            state.zoom = zoom
            state.constrain(in: bounds, imageSize: image.map { CGSize(width: $0.width, height: $0.height) } ?? imageSize)
            viewport = state
            appliedZoom = zoom
        }
        // The socket retains the last frame on reconnect and supplies only the
        // selected display's preview (or nil) when switching displays.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        frameLayer.contents = image
        CATransaction.commit()
        imageSize = image.map { CGSize(width: $0.width, height: $0.height) } ?? .zero
        accessibilityValue = acceptsInput ? String(localized: "Controlling") : String(localized: "View only")
        accessibilityCustomActions = acceptsInput ? [
            UIAccessibilityCustomAction(name: String(localized: "Left click"), target: self, selector: #selector(accessibleClick)),
            UIAccessibilityCustomAction(name: String(localized: "Right click"), target: self, selector: #selector(accessibleRightClick)),
            UIAccessibilityCustomAction(name: String(localized: "Show keyboard"), target: self, selector: #selector(accessibleKeyboard))
        ] : []
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        var state = viewport
        state.constrain(in: bounds, imageSize: imageSize)
        viewport = state
        renderTransform()
    }

    private func renderTransform() {
        let rect = viewport.imageRect(in: bounds, imageSize: imageSize)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        frameLayer.frame = rect
        cursorLayer.position = CGPoint(x: rect.minX + pointer.x * rect.width, y: rect.minY + pointer.y * rect.height)
        cursorLayer.isHidden = !acceptsInput || imageSize == .zero
        CATransaction.commit()
    }

    func cancelInteraction() {
        if dragging { onInput?(.button(button: "left", down: false)) }
        dragging = false
        pinching = false
    }

    private func move(to point: CGPoint, clamped: Bool = false) -> Bool {
        guard acceptsInput, let displayID else { return false }
        let mapped = clamped
            ? viewport.clampedNormalized(point, in: bounds, imageSize: imageSize)
            : viewport.normalized(point, in: bounds, imageSize: imageSize)
        guard let normalized = mapped else { return false }
        pointer = normalized
        onInput?(.pointer(display: displayID, u: Double(normalized.x), v: Double(normalized.y)))
        renderTransform()
        return true
    }

    private func sendPointer() {
        guard acceptsInput, let displayID, imageSize != .zero else { return }
        onInput?(.pointer(display: displayID, u: Double(pointer.x), v: Double(pointer.y)))
        renderTransform()
    }

    private func click(at point: CGPoint?, button: String, count: Int) {
        guard acceptsInput, let displayID, imageSize != .zero else { return }
        if mode == .direct, let point, !move(to: point) { return }
        sendPointer()
        onInput?(.click(display: displayID, button: button, count: count))
    }

    @objc private func tap(_ gesture: UITapGestureRecognizer) { click(at: gesture.location(in: self), button: "left", count: 1) }
    @objc private func doubleTap(_ gesture: UITapGestureRecognizer) { click(at: gesture.location(in: self), button: "left", count: 2) }
    @objc private func rightTap(_ gesture: UITapGestureRecognizer) { click(at: gesture.location(in: self), button: "right", count: 1) }
    @objc private func keyboardTap() { if acceptsInput { onKeyboard?() } }
    @objc private func accessibleClick() -> Bool { click(at: nil, button: "left", count: 1); return acceptsInput }
    @objc private func accessibleRightClick() -> Bool { click(at: nil, button: "right", count: 1); return acceptsInput }
    @objc private func accessibleKeyboard() -> Bool { keyboardTap(); return acceptsInput }

    @objc private func longPress(_ gesture: UILongPressGestureRecognizer) {
        if gesture.state == .began { click(at: gesture.location(in: self), button: "right", count: 1) }
    }

    @objc private func pan(_ gesture: UIPanGestureRecognizer) {
        let delta = gesture.translation(in: self)
        gesture.setTranslation(.zero, in: self)
        guard gesture.state == .changed, !pinching else { return }
        if acceptsInput, mode == .trackpad {
            let rect = viewport.imageRect(in: bounds, imageSize: imageSize)
            pointer = DesktopInteractionPolicy.relativePointer(pointer, delta: CGSize(width: delta.x, height: delta.y), renderedSize: rect.size)
            sendPointer()
        } else {
            localPan(delta)
        }
    }

    private func localPan(_ delta: CGPoint) {
        guard allowsNavigation else { return }
        var state = viewport
        state.pan.width += delta.x
        state.pan.height += delta.y
        state.constrain(in: bounds, imageSize: imageSize)
        viewport = state
        renderTransform()
    }

    @objc private func scroll(_ gesture: UIPanGestureRecognizer) {
        let delta = gesture.translation(in: self)
        gesture.setTranslation(.zero, in: self)
        guard gesture.state == .changed, !pinching else { return }
        if acceptsInput, let displayID {
            onInput?(.scroll(display: displayID, dx: Double(-delta.x), dy: Double(-delta.y)))
        } else {
            localPan(delta)
        }
    }

    @objc private func pinch(_ gesture: UIPinchGestureRecognizer) {
        guard allowsNavigation else { pinching = false; return }
        pinching = gesture.state == .began || gesture.state == .changed
        if pinching {
            var state = viewport
            state.magnify(by: gesture.scale, around: gesture.location(in: self), in: bounds, imageSize: imageSize)
            viewport = state
            gesture.scale = 1
            renderTransform()
        }
    }

    @objc private func drag(_ gesture: UILongPressGestureRecognizer) {
        guard acceptsInput, imageSize != .zero else { cancelInteraction(); return }
        switch gesture.state {
        case .began:
            guard mode == .trackpad || move(to: gesture.location(in: self)) else { return }
            sendPointer()
            onInput?(.button(button: "left", down: true))
            dragging = true
        case .changed:
            guard dragging else { return }
            if mode == .direct { _ = move(to: gesture.location(in: self), clamped: true) }
            else {
                let point = gesture.location(in: self)
                let rect = viewport.imageRect(in: bounds, imageSize: imageSize)
                pointer = DesktopInteractionPolicy.relativePointer(pointer,
                    delta: CGSize(width: point.x - lastDragPoint.x, height: point.y - lastDragPoint.y), renderedSize: rect.size)
                sendPointer()
            }
        case .ended:
            if dragging, mode == .direct { _ = move(to: gesture.location(in: self), clamped: true) }
            cancelInteraction()
        case .cancelled, .failed: cancelInteraction()
        default: break
        }
        lastDragPoint = gesture.location(in: self)
    }
    private var lastDragPoint = CGPoint.zero

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer) -> Bool {
        // Pinch is exclusively local: never turn its translation into host scroll.
        gestureRecognizer is UIPinchGestureRecognizer || otherGestureRecognizer is UIPinchGestureRecognizer
    }
}
