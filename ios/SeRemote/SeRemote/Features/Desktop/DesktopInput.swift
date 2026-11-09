import CoreGraphics
import Foundation

nonisolated struct DesktopModifiers: Equatable, Sendable {
    var shift = false
    var control = false
    var alt = false
    var meta = false

    var json: [String: Any] {
        ["shift": shift, "control": control, "alt": alt, "meta": meta]
    }
}

nonisolated enum DesktopInputMessage: Equatable, Sendable {
    case pointer(display: String, u: Double, v: Double)
    case button(button: String, down: Bool)
    case click(display: String, button: String, count: Int)
    case scroll(display: String, dx: Double, dy: Double)
    case key(code: String, down: Bool, modifiers: DesktopModifiers)
    case text(String)

    var json: [String: Any] {
        switch self {
        case let .pointer(display, u, v):
            ["type": "pointer", "display": display, "u": u, "v": v]
        case let .button(button, down):
            ["type": "button", "button": button, "down": down]
        case let .click(display, button, count):
            ["type": "click", "display": display, "button": button, "count": count]
        case let .scroll(display, dx, dy):
            ["type": "scroll", "display": display, "dx": Self.scrollInteger(dx), "dy": Self.scrollInteger(dy)]
        case let .key(code, down, modifiers):
            ["type": "key", "code": code, "down": down, "modifiers": modifiers.json]
        case let .text(text):
            ["type": "text", "text": text]
        }
    }

    private static func scrollInteger(_ value: Double) -> Int {
        guard value.isFinite else { return 0 }
        return Int(min(Double(Int32.max), max(Double(Int32.min), value.rounded())))
    }
}

/// All units are local points except pixelSize; no Retina scale or host origin is
/// applied here. The host converts the normalized result using its native display.
nonisolated struct DesktopViewport: Equatable, Sendable {
    let pixelSize: CGSize
    let viewportSize: CGSize
    let zoom: CGFloat
    let pan: CGPoint

    init(pixelSize: CGSize, viewportSize: CGSize, zoom: CGFloat = 1, pan: CGPoint = .zero) {
        self.pixelSize = pixelSize
        self.viewportSize = viewportSize
        self.zoom = zoom.isFinite ? min(8, max(1, zoom)) : 1
        self.pan = CGPoint(x: pan.x.isFinite ? pan.x : 0, y: pan.y.isFinite ? pan.y : 0)
    }

    var imageRect: CGRect {
        guard pixelSize.width.isFinite, pixelSize.height.isFinite,
              viewportSize.width.isFinite, viewportSize.height.isFinite,
              pixelSize.width > 0, pixelSize.height > 0,
              viewportSize.width > 0, viewportSize.height > 0 else { return .zero }
        let fit = min(viewportSize.width / pixelSize.width, viewportSize.height / pixelSize.height)
        let width = pixelSize.width * fit * zoom
        let height = pixelSize.height * fit * zoom
        guard width.isFinite, height.isFinite, width > 0, height > 0 else { return .zero }
        let limitX = max(0, (width - viewportSize.width) / 2)
        let limitY = max(0, (height - viewportSize.height) / 2)
        let x = min(limitX, max(-limitX, pan.x))
        let y = min(limitY, max(-limitY, pan.y))
        return CGRect(x: (viewportSize.width - width) / 2 + x,
                      y: (viewportSize.height - height) / 2 + y, width: width, height: height)
    }

    func normalizedPoint(_ point: CGPoint) -> CGPoint? {
        let rect = imageRect
        guard rect.width > 0, rect.height > 0, point.x.isFinite, point.y.isFinite,
              point.x >= 0, point.y >= 0,
              point.x <= viewportSize.width, point.y <= viewportSize.height,
              point.x >= rect.minX, point.x <= rect.maxX,
              point.y >= rect.minY, point.y <= rect.maxY else { return nil }
        return clampedNormalizedPoint(point)
    }

    /// A drag which began on the canvas may leave it; keep its pointer on an edge.
    func clampedNormalizedPoint(_ point: CGPoint) -> CGPoint? {
        let rect = imageRect
        guard rect.width > 0, rect.height > 0, point.x.isFinite, point.y.isFinite else { return nil }
        return CGPoint(x: min(1, max(0, (point.x - rect.minX) / rect.width)),
                       y: min(1, max(0, (point.y - rect.minY) / rect.height)))
    }
}

nonisolated enum DesktopInputMode: Sendable { case trackpad, direct }

/// Local pinch/zoom and three-finger keyboard gestures deliberately emit no host input.
nonisolated enum DesktopGesture: Sendable {
    case tap(location: CGPoint, count: Int, fingers: Int)
    case longPress(location: CGPoint)
    case move(location: CGPoint, delta: CGSize)
    case dragBegan(location: CGPoint)
    case dragChanged(location: CGPoint, delta: CGSize)
    case dragEnded
    case scroll(delta: CGSize)
    case localOnly
}

nonisolated struct DesktopGestureMapper: Sendable {
    var display: String
    var mode: DesktopInputMode
    private(set) var pointer: CGPoint
    private(set) var dragging = false

    init(display: String, mode: DesktopInputMode, pointer: CGPoint = CGPoint(x: 0.5, y: 0.5)) {
        self.display = display
        self.mode = mode
        self.pointer = CGPoint(x: pointer.x.isFinite ? min(1, max(0, pointer.x)) : 0.5,
                               y: pointer.y.isFinite ? min(1, max(0, pointer.y)) : 0.5)
    }

    mutating func messages(for gesture: DesktopGesture, in viewport: DesktopViewport) -> [DesktopInputMessage] {
        switch gesture {
        case let .tap(location, count, fingers):
            guard (1...3).contains(count), (1...2).contains(fingers),
                  move(to: location, delta: .zero, in: viewport) else { return [] }
            return [pointerMessage, .click(display: display, button: fingers == 2 ? "right" : "left", count: count)]
        case let .longPress(location):
            guard move(to: location, delta: .zero, in: viewport) else { return [] }
            return [pointerMessage, .click(display: display, button: "right", count: 1)]
        case let .move(location, delta):
            guard move(to: location, delta: delta, in: viewport) else { return [] }
            return [pointerMessage]
        case let .dragBegan(location):
            guard !dragging, move(to: location, delta: .zero, in: viewport) else { return [] }
            dragging = true
            return [pointerMessage, .button(button: "left", down: true)]
        case let .dragChanged(location, delta):
            guard dragging, move(to: location, delta: delta, in: viewport, clamped: true) else { return [] }
            return [pointerMessage]
        case .dragEnded:
            return cancelDrag()
        case let .scroll(delta):
            guard delta.width.isFinite, delta.height.isFinite else { return [] }
            return [.scroll(display: display, dx: Double(delta.width), dy: Double(delta.height))]
        case .localOnly: return []
        }
    }

    /// Call before switching displays/modes or suspending so a held mouse cannot stick.
    mutating func cancelDrag() -> [DesktopInputMessage] {
        guard dragging else { return [] }
        dragging = false
        return [.button(button: "left", down: false)]
    }

    private var pointerMessage: DesktopInputMessage {
        .pointer(display: display, u: Double(pointer.x), v: Double(pointer.y))
    }

    private mutating func move(to location: CGPoint, delta: CGSize, in viewport: DesktopViewport,
                               clamped: Bool = false) -> Bool {
        switch mode {
        case .direct:
            guard let point = clamped ? viewport.clampedNormalizedPoint(location) : viewport.normalizedPoint(location) else {
                return false
            }
            pointer = point
        case .trackpad:
            let rect = viewport.imageRect
            guard rect.width > 0, rect.height > 0, delta.width.isFinite, delta.height.isFinite else { return false }
            pointer.x = min(1, max(0, pointer.x + delta.width / rect.width))
            pointer.y = min(1, max(0, pointer.y + delta.height / rect.height))
        }
        return true
    }
}

/// Deterministic latest-value coalescing. Pass monotonic clock seconds and schedule
/// flush at the next interval; no timer, Task, actor or socket is owned here.
nonisolated struct DesktopPointerCoalescer: Sendable {
    let interval: TimeInterval
    private var lastSent: TimeInterval?
    private var pending: DesktopInputMessage?

    init(interval: TimeInterval = 1.0 / 30) {
        self.interval = interval.isFinite && interval > 0 ? interval : 1.0 / 30
    }

    /// Clock-free storage for a caller which already owns its 33ms scheduler.
    /// Use this pair OR the timestamped API, rather than mixing timing policies.
    mutating func offer(_ message: DesktopInputMessage) {
        guard case .pointer = message else { return }
        pending = message
    }

    mutating func take() -> DesktopInputMessage? {
        defer { pending = nil }
        return pending
    }

    mutating func offer(_ message: DesktopInputMessage, at time: TimeInterval) -> DesktopInputMessage? {
        guard case .pointer = message else { return message }
        guard time.isFinite else { return nil }
        pending = message
        return flush(at: time)
    }

    mutating func flush(at time: TimeInterval, force: Bool = false) -> DesktopInputMessage? {
        guard time.isFinite, let pending else { return nil }
        guard force || lastSent == nil || time - (lastSent ?? time) >= interval else { return nil }
        self.pending = nil
        lastSent = time
        return pending
    }

    /// For mixed messages, flush the latest pointer BEFORE a click/key/button.
    /// This prevents a coalesced move from arriving after its click or mouse-up.
    mutating func submit(_ message: DesktopInputMessage, at time: TimeInterval) -> [DesktopInputMessage] {
        if case .pointer = message { return offer(message, at: time).map { [$0] } ?? [] }
        return (flush(at: time, force: true).map { [$0] } ?? []) + [message]
    }

    mutating func reset() { lastSent = nil; pending = nil }
}
