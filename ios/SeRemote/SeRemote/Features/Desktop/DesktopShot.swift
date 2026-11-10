import CoreGraphics
import Foundation
import ImageIO
import UIKit
import UniformTypeIdentifiers

/// One captured region ready to hand to a chat or the host staging route.
///
/// `pixelSize` is the size of the cropped image in host pixels, not points.
nonisolated struct ShotAttachment: Equatable, Sendable {
    var data: Data
    var mimeType: String
    var pixelSize: CGSize
    var capturedAt: Date

    init(data: Data, mimeType: String, pixelSize: CGSize, capturedAt: Date = Date()) {
        self.data = data
        self.mimeType = mimeType
        self.pixelSize = pixelSize
        self.capturedAt = capturedAt
    }
}

nonisolated enum DesktopShotError: Error, Equatable {
    case cropFailed
    case encodeFailed
    case tooLarge
}

/// Selection geometry shared by the viewfinder and its tests.
///
/// The selection is stored in **image pixels**. Converting from the view goes
/// through `DesktopLocalViewport` so the sheet and the canvas agree on fit,
/// zoom and pan.
nonisolated enum DesktopShotGeometry {
    static let minimumSide: CGFloat = 64
    static let defaultFraction: CGFloat = 0.6

    /// The 60% centered starting selection.
    static func defaultSelection(in pixelSize: CGSize) -> CGRect {
        guard pixelSize.width > 0, pixelSize.height > 0 else { return .zero }
        let width = max(min(pixelSize.width * defaultFraction, pixelSize.width), minSide(for: pixelSize))
        let height = max(min(pixelSize.height * defaultFraction, pixelSize.height), minSide(for: pixelSize))
        return CGRect(
            x: (pixelSize.width - width) / 2,
            y: (pixelSize.height - height) / 2,
            width: width,
            height: height
        )
    }

    static func minSide(for pixelSize: CGSize) -> CGFloat {
        min(minimumSide, min(pixelSize.width, pixelSize.height))
    }

    /// Keeps a selection inside the frame and above the minimum size.
    static func clamped(_ rect: CGRect, to pixelSize: CGSize) -> CGRect {
        guard pixelSize.width > 0, pixelSize.height > 0 else { return .zero }
        let minWidth = minSide(for: pixelSize)
        let minHeight = minSide(for: pixelSize)
        let width = min(max(rect.width.isFinite ? rect.width : minWidth, minWidth), pixelSize.width)
        let height = min(max(rect.height.isFinite ? rect.height : minHeight, minHeight), pixelSize.height)
        let x = min(max(rect.origin.x.isFinite ? rect.origin.x : 0, 0), pixelSize.width - width)
        let y = min(max(rect.origin.y.isFinite ? rect.origin.y : 0, 0), pixelSize.height - height)
        return CGRect(x: x, y: y, width: width, height: height)
    }

    /// Snaps a selection to whole frame pixels after clamping.
    static func pixelSnapped(_ rect: CGRect) -> CGRect {
        let x = rect.origin.x.rounded(.toNearestOrAwayFromZero)
        let y = rect.origin.y.rounded(.toNearestOrAwayFromZero)
        var width = rect.size.width.rounded(.toNearestOrAwayFromZero)
        var height = rect.size.height.rounded(.toNearestOrAwayFromZero)
        if width < 1 { width = 1 }
        if height < 1 { height = 1 }
        return CGRect(x: x, y: y, width: width, height: height)
    }

    static func prepared(_ rect: CGRect, in pixelSize: CGSize) -> CGRect {
        pixelSnapped(clamped(pixelSnapped(rect), to: pixelSize))
    }

    /// Fit-zoom-pan frame used by the canvas. Kept here so the viewfinder
    /// mapping is a pure function and testable off the main actor.
    static func imageFrame(
        in bounds: CGRect,
        imageSize: CGSize,
        zoom: CGFloat = 1,
        pan: CGSize = .zero
    ) -> CGRect {
        guard bounds.width > 0, bounds.height > 0,
              imageSize.width > 0, imageSize.height > 0 else { return .zero }
        let scale = min(bounds.width / imageSize.width, bounds.height / imageSize.height) * max(zoom, 1)
        let width = imageSize.width * scale
        let height = imageSize.height * scale
        return CGRect(
            x: bounds.midX - width / 2 + pan.width,
            y: bounds.midY - height / 2 + pan.height,
            width: width,
            height: height
        )
    }

    /// View point → image pixel point. Returns nil outside the rendered frame.
    static func imagePoint(
        fromView point: CGPoint,
        in bounds: CGRect,
        imageSize: CGSize,
        zoom: CGFloat = 1,
        pan: CGSize = .zero
    ) -> CGPoint? {
        let frame = imageFrame(in: bounds, imageSize: imageSize, zoom: zoom, pan: pan)
        guard frame.width > 0, frame.height > 0,
              point.x.isFinite, point.y.isFinite,
              frame.insetBy(dx: -0.5, dy: -0.5).contains(point) else { return nil }
        return CGPoint(
            x: (point.x - frame.minX) / frame.width * imageSize.width,
            y: (point.y - frame.minY) / frame.height * imageSize.height
        )
    }

    /// Image pixel rect → view rect.
    static func viewRect(
        fromImage rect: CGRect,
        in bounds: CGRect,
        imageSize: CGSize,
        zoom: CGFloat = 1,
        pan: CGSize = .zero
    ) -> CGRect {
        let frame = imageFrame(in: bounds, imageSize: imageSize, zoom: zoom, pan: pan)
        guard frame.width > 0, frame.height > 0,
              imageSize.width > 0, imageSize.height > 0 else { return .zero }
        let scaleX = frame.width / imageSize.width
        let scaleY = frame.height / imageSize.height
        return CGRect(
            x: frame.minX + rect.minX * scaleX,
            y: frame.minY + rect.minY * scaleY,
            width: rect.width * scaleX,
            height: rect.height * scaleY
        )
    }

    /// View-space rect → image-pixel rect, used by the drag gestures.
    static func imageRect(
        fromView rect: CGRect,
        in bounds: CGRect,
        imageSize: CGSize,
        zoom: CGFloat = 1,
        pan: CGSize = .zero
    ) -> CGRect? {
        let frame = imageFrame(in: bounds, imageSize: imageSize, zoom: zoom, pan: pan)
        guard frame.width > 0, frame.height > 0,
              imageSize.width > 0, imageSize.height > 0 else { return nil }
        return CGRect(
            x: (rect.minX - frame.minX) / frame.width * imageSize.width,
            y: (rect.minY - frame.minY) / frame.height * imageSize.height,
            width: rect.width / frame.width * imageSize.width,
            height: rect.height / frame.height * imageSize.height
        )
    }
}

nonisolated enum DesktopShotCropper {
    /// Crop in host pixels. `rect` is clamped and snapped by the caller.
    static func crop(_ image: CGImage, to rect: CGRect) -> CGImage? {
        let bounds = CGRect(x: 0, y: 0, width: image.width, height: image.height)
        let snapped = DesktopShotGeometry.pixelSnapped(rect)
        let clamped = snapped.intersection(bounds)
        guard clamped.width >= 1, clamped.height >= 1 else { return nil }
        return image.cropping(to: clamped)
    }
}

/// Produces a chat-safe image: JPEG, aspect preserved, at or below 144 KiB
/// (base64 then stays well under the host's 256 KiB conversation record cap).
nonisolated enum DesktopShotEncoder {
    static let maxInlineBytes = 144 * 1024

    static let chatScales: [CGFloat] = [1, 0.85, 0.7, 0.55, 0.45, 0.35, 0.27, 0.2, 0.14, 0.1, 0.07]
    static let chatQualities: [CGFloat] = [0.82, 0.68, 0.54, 0.42, 0.32, 0.24, 0.18, 0.12]

    static func encodeForChat(_ image: CGImage, maxBytes: Int = maxInlineBytes) -> ShotAttachment? {
        guard image.width > 0, image.height > 0, maxBytes > 0 else { return nil }
        for scale in chatScales {
            let size = scaledSize(of: image, by: scale)
            guard let scaled = resize(image, to: size) else { continue }
            let ui = UIImage(cgImage: scaled)
            for quality in chatQualities {
                guard let data = ui.jpegData(compressionQuality: quality), data.count <= maxBytes else { continue }
                return ShotAttachment(
                    data: data,
                    mimeType: "image/jpeg",
                    pixelSize: size
                )
            }
        }
        return nil
    }

    static func encodePNG(_ image: CGImage) -> ShotAttachment? {
        let ui = UIImage(cgImage: image)
        guard let data = ui.pngData() else { return nil }
        return ShotAttachment(
            data: data,
            mimeType: "image/png",
            pixelSize: CGSize(width: image.width, height: image.height)
        )
    }

    static func scaledSize(of image: CGImage, by scale: CGFloat) -> CGSize {
        let width = max(1, (CGFloat(image.width) * scale).rounded())
        let height = max(1, (CGFloat(image.height) * scale).rounded())
        return CGSize(width: width, height: height)
    }

    /// Aspect-preserving resize. The caller supplies the already-scaled size.
    static func resize(_ image: CGImage, to size: CGSize) -> CGImage? {
        let width = Int(size.width.rounded())
        let height = Int(size.height.rounded())
        guard width > 0, height > 0 else { return nil }
        guard let colorSpace = image.colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return context.makeImage()
    }
}
