import Foundation
import CoreGraphics

/// Local-only viewport. The host's pixel size / Retina scale / desktop origin do
/// not enter normalized input coordinates: the selected display ID is the space.
struct DesktopLocalViewport: Equatable {
    var zoom: CGFloat = 1
    var pan: CGSize = .zero

    func imageRect(in bounds: CGRect, imageSize: CGSize) -> CGRect {
        guard bounds.width > 0, bounds.height > 0,
              imageSize.width > 0, imageSize.height > 0 else { return .zero }
        let fit = min(bounds.width / imageSize.width, bounds.height / imageSize.height)
        let size = CGSize(width: imageSize.width * fit * zoom, height: imageSize.height * fit * zoom)
        return CGRect(x: bounds.midX - size.width / 2 + pan.width,
                      y: bounds.midY - size.height / 2 + pan.height,
                      width: size.width, height: size.height)
    }

    /// Reject letterbox taps instead of moving the remote pointer to an edge.
    func normalized(_ point: CGPoint, in bounds: CGRect, imageSize: CGSize) -> CGPoint? {
        let rect = imageRect(in: bounds, imageSize: imageSize)
        guard rect.width > 0, rect.height > 0,
              point.x.isFinite, point.y.isFinite,
              point.x >= rect.minX, point.x <= rect.maxX,
              point.y >= rect.minY, point.y <= rect.maxY else { return nil }
        return CGPoint(x: (point.x - rect.minX) / rect.width,
                       y: (point.y - rect.minY) / rect.height)
    }

    /// Active drags keep tracking at the nearest edge after leaving the image.
    /// This is deliberately separate from normalized(), which rejects taps.
    func clampedNormalized(_ point: CGPoint, in bounds: CGRect, imageSize: CGSize) -> CGPoint? {
        let rect = imageRect(in: bounds, imageSize: imageSize)
        guard rect.width > 0, rect.height > 0,
              point.x.isFinite, point.y.isFinite else { return nil }
        return CGPoint(x: min(1, max(0, (point.x - rect.minX) / rect.width)),
                       y: min(1, max(0, (point.y - rect.minY) / rect.height)))
    }

    mutating func constrain(in bounds: CGRect, imageSize: CGSize) {
        zoom = min(5, max(1, zoom.isFinite ? zoom : 1))
        let rect = imageRect(in: bounds, imageSize: imageSize)
        let x = max(0, (rect.width - bounds.width) / 2)
        let y = max(0, (rect.height - bounds.height) / 2)
        pan.width = min(x, max(-x, pan.width.isFinite ? pan.width : 0))
        pan.height = min(y, max(-y, pan.height.isFinite ? pan.height : 0))
    }

    mutating func magnify(by factor: CGFloat, around anchor: CGPoint, in bounds: CGRect, imageSize: CGSize) {
        guard factor.isFinite, factor > 0 else { return }
        let before = imageRect(in: bounds, imageSize: imageSize)
        let oldZoom = zoom
        zoom = min(5, max(1, zoom * factor))
        let ratio = zoom / oldZoom
        pan.width = anchor.x - bounds.midX - (anchor.x - before.midX) * ratio
        pan.height = anchor.y - bounds.midY - (anchor.y - before.midY) * ratio
        constrain(in: bounds, imageSize: imageSize)
    }
}

enum DesktopKeyboardPolicy {
    static let hardwareShortcutInputs = "abcdefghijklmnopqrstuvwxyz0123456789-=[]\\;'`,./!@#$%^&*()_+{}|:\"<>?~ "

    static func committedKeyCode(_ text: String, hasModifiers: Bool) -> String? {
        // Keep multi-scalar/multi-character IME commits as text, even with a
        // sticky modifier; a single key must preserve Shift/Control/etc flags.
        hasModifiers ? hostKeyCode(text) : nil
    }

    static func hostKeyCode(_ input: String) -> String? {
        if let special = [" ": "Space", "\n": "Enter", "\r": "Enter", "\t": "Tab"][input] {
            return special
        }
        // Host parse_key accepts a single Unicode scalar, not DOM KeyA / Digit1.
        guard input.unicodeScalars.count == 1,
              let scalar = input.unicodeScalars.first,
              !CharacterSet.controlCharacters.contains(scalar) else { return nil }
        let lowered = input.lowercased()
        return lowered.unicodeScalars.count == 1 ? lowered : input
    }
}

enum DesktopInteractionMode: String, CaseIterable, Identifiable {
    case trackpad, direct
    var id: String { rawValue }
    var title: String {
        switch self {
        case .trackpad: String(localized: "Trackpad")
        case .direct: String(localized: "Direct touch")
        }
    }
}

/// Shared gate for touch, accessibility, soft keyboard and hardware keyboard.
enum DesktopInteractionPolicy {
    static func canControl(deviceAllowed: Bool, hostAllowed: Bool, active: Bool, streaming: Bool) -> Bool {
        deviceAllowed && hostAllowed && active && streaming
    }

    static func relativePointer(_ pointer: CGPoint, delta: CGSize, renderedSize: CGSize) -> CGPoint {
        guard renderedSize.width > 0, renderedSize.height > 0 else { return pointer }
        return CGPoint(x: min(1, max(0, pointer.x + delta.width / renderedSize.width)),
                       y: min(1, max(0, pointer.y + delta.height / renderedSize.height)))
    }
}
