import CoreGraphics
import Foundation
import ImageIO
import UIKit
import UniformTypeIdentifiers

/// Makes a picked photo acceptable to `POST /remote/attachments/stage`: PNG or
/// JPEG, at most 8 MiB, longest edge at most 4096 px.
///
/// Photos arrive as HEIC, many-megapixel PNGs or already-small JPEGs. Data that
/// is already a host-safe PNG/JPEG is passed through byte-for-byte; everything
/// else is decoded, downscaled and re-encoded as JPEG. Non-image data returns
/// `nil`, so the sheet can say so instead of staging an opaque failure.
nonisolated enum AttachmentImagePolicy {
    static let maxDimension: CGFloat = 4096
    static let maxBytes = 8 * 1024 * 1024

    /// Quality ladder for one size. Noise-heavy photos do not fit 8 MiB at the
    /// top of the ladder, so the size is stepped down instead of dropping the
    /// quality to a single smeared attempt.
    private static let jpegQualities: [CGFloat] = [0.9, 0.78, 0.64, 0.5, 0.38]
    /// Each retry shrinks the longest edge; the loop stops at 6% of the source.
    private static let shrinkStep: CGFloat = 0.72
    private static let minimumScale: CGFloat = 0.06

    /// Host-accepted container types, so a small screenshot is never re-encoded.
    private enum SourceKind {
        case png
        case jpeg
    }

    /// Preflight on the caller's thread. Long decodes belong on
    /// `preparedDetached`.
    static func prepared(data: Data, name: String) -> ShotAttachment? {
        prepared(data: data, name: name, maxBytes: maxBytes)
    }

    /// Same policy with an explicit byte budget, so the ladder is testable
    /// without a multi-megabyte fixture.
    static func prepared(data: Data, name: String, maxBytes byteBudget: Int) -> ShotAttachment? {
        guard byteBudget > 0,
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(source) > 0 else { return nil }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let sourceWidth = (properties?[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue ?? 0
        let sourceHeight = (properties?[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue ?? 0
        guard sourceWidth > 0, sourceHeight > 0 else { return nil }

        if let passthrough = passthrough(
            source: source,
            data: data,
            sourceWidth: sourceWidth,
            sourceHeight: sourceHeight,
            byteBudget: byteBudget
        ) {
            return passthrough
        }

        var scale = min(1, maxDimension / CGFloat(max(sourceWidth, sourceHeight)))
        while scale >= minimumScale {
            let size = scaledSize(width: sourceWidth, height: sourceHeight, by: scale)
            if let image = resizedImage(from: source, to: size),
               let shot = encodeJPEG(image, budget: byteBudget) {
                return shot
            }
            scale *= shrinkStep
        }
        return nil
    }

    /// Runs the decode and encode off the main actor; the sheet only awaits.
    static func preparedDetached(data: Data, name: String) async -> ShotAttachment? {
        await Task.detached(priority: .userInitiated) {
            prepared(data: data, name: name)
        }.value
    }

    static func scaledSize(width: Int, height: Int, by scale: CGFloat) -> CGSize {
        let longest = CGFloat(max(width, height))
        guard longest > 0, scale > 0 else { return .zero }
        let fitted = min(scale, maxDimension / longest)
        let scaledWidth = min(maxDimension, max(1, (CGFloat(width) * fitted).rounded()))
        let scaledHeight = min(maxDimension, max(1, (CGFloat(height) * fitted).rounded()))
        return CGSize(width: scaledWidth, height: scaledHeight)
    }

    private static func passthrough(
        source: CGImageSource,
        data: Data,
        sourceWidth: Int,
        sourceHeight: Int,
        byteBudget: Int
    ) -> ShotAttachment? {
        guard data.count <= byteBudget,
              max(sourceWidth, sourceHeight) <= Int(maxDimension),
              let kind = kind(of: source) else { return nil }
        return ShotAttachment(
            data: data,
            mimeType: kind == .png ? "image/png" : "image/jpeg",
            pixelSize: CGSize(width: sourceWidth, height: sourceHeight)
        )
    }

    private static func kind(of source: CGImageSource) -> SourceKind? {
        guard let identifier = CGImageSourceGetType(source) as String?,
              let type = UTType(identifier) else { return nil }
        if type.conforms(to: .png) { return .png }
        if type.conforms(to: .jpeg) { return .jpeg }
        return nil
    }

    private static func encodeJPEG(_ image: CGImage, budget: Int) -> ShotAttachment? {
        let ui = UIImage(cgImage: image)
        let size = CGSize(width: image.width, height: image.height)
        for quality in jpegQualities {
            guard let data = ui.jpegData(compressionQuality: quality), data.count <= budget else { continue }
            return ShotAttachment(data: data, mimeType: "image/jpeg", pixelSize: size)
        }
        return nil
    }

    /// `CGImageSourceCreateThumbnailAtIndex` decodes straight to the target
    /// size, which keeps a many-megapixel photo from ever becoming a full-size
    /// bitmap in memory. The fallback resizes a full decode for formats ImageIO
    /// will not thumbnail.
    private static func resizedImage(from source: CGImageSource, to size: CGSize) -> CGImage? {
        let maxPixelSize = Int(max(size.width, size.height))
        guard maxPixelSize > 0 else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        if let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) {
            return thumbnail.width <= Int(maxDimension) && thumbnail.height <= Int(maxDimension) ? thumbnail : nil
        }
        guard let full = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
        if max(full.width, full.height) <= maxPixelSize { return full }
        return DesktopShotEncoder.resize(full, to: size)
    }
}
