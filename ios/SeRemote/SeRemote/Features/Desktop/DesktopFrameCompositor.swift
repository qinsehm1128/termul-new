import CoreGraphics
import Foundation
import ImageIO

/// Mutable canvases are confined to DesktopFrameWorker (tests may own a private instance).
/// No canvas is published until every tile has decoded and the replacement image exists.
nonisolated final class DesktopFrameCompositor {
    private struct Frame {
        let image: CGImage
        let sequence: UInt64?
        let synchronized: Bool
    }
    private var frames: [String: Frame] = [:]
    private var recency: [String] = []
    private var selectedDisplay: String?
    let imageByteBudget: Int

    init(imageByteBudget: Int = 96 * 1024 * 1024) {
        self.imageByteBudget = max(0, imageByteBudget)
    }

    var retainedImageBytes: Int {
        frames.values.reduce(0) { $0 + $1.image.bytesPerRow * $1.image.height }
    }

    func image(for display: String) -> CGImage? { frames[display]?.image }

    func selectDisplay(_ display: String) {
        selectedDisplay = display
        touch(display)
        evictImages()
    }

    private func touch(_ display: String) {
        recency.removeAll { $0 == display }
        recency.append(display)
    }

    private func evictImages() {
        // The current canvas is never evicted. The production budget exceeds the
        // protocol's 64 MiB single-canvas limit. Keep at most one other preview.
        let countLimit = selectedDisplay.flatMap { frames[$0] } == nil ? 1 : 2
        while frames.count > countLimit || retainedImageBytes > imageByteBudget {
            guard let victim = recency.first(where: { $0 != selectedDisplay && frames[$0] != nil }) else { break }
            frames.removeValue(forKey: victim)
            recency.removeAll { $0 == victim }
        }
    }

    func reset(display: String) {
        frames.removeValue(forKey: display)
        recency.removeAll { $0 == display }
    }
    func reset() { frames.removeAll(); recency.removeAll() }

    /// New engine sessions restart sequence numbers. Keep the last visible images,
    /// but require a keyframe before applying any delta from the new connection.
    func beginStream() {
        frames = frames.mapValues { Frame(image: $0.image, sequence: nil, synchronized: false) }
    }

    @discardableResult
    func apply(_ packet: DesktopPacket) throws -> Bool {
        let previous = frames[packet.display]
        if let sequence = previous?.sequence, packet.seq <= sequence { return false }
        guard packet.width > 0, packet.height > 0,
              packet.width <= DesktopPacket.maxDimension,
              packet.height <= DesktopPacket.maxDimension,
              packet.width * packet.height <= DesktopPacket.maxPixels else {
            throw DesktopProtocolError.invalidPacket
        }
        if packet.type == .tiles {
            guard let previous, let sequence = previous.sequence, previous.synchronized,
                  sequence < UInt64.max, packet.seq == sequence + 1,
                  previous.image.width == packet.width, previous.image.height == packet.height else {
                if let previous {
                    frames[packet.display] = Frame(image: previous.image, sequence: previous.sequence, synchronized: false)
                }
                throw DesktopProtocolError.missingKeyframe
            }
        }

        let options = [kCGImageSourceShouldCacheImmediately: true] as CFDictionary
        let decoded = try packet.tiles.map { tile -> (DesktopPacket.Tile, CGImage) in
            guard let source = CGImageSourceCreateWithData(tile.jpeg as CFData, nil),
                  CGImageSourceGetStatus(source) == .statusComplete,
                  let image = CGImageSourceCreateImageAtIndex(source, 0, options),
                  CGImageSourceGetStatusAtIndex(source, 0) == .statusComplete,
                  image.width == tile.width, image.height == tile.height else {
                throw DesktopProtocolError.invalidJPEG
            }
            return (tile, image)
        }
        guard let context = CGContext(
            data: nil, width: packet.width, height: packet.height,
            bitsPerComponent: 8, bytesPerRow: packet.width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        ) else { throw DesktopProtocolError.allocationFailed }
        context.interpolationQuality = .none
        context.setBlendMode(.copy)
        let canvas = CGRect(x: 0, y: 0, width: packet.width, height: packet.height)
        if packet.type == .tiles, let previous {
            context.draw(previous.image, in: canvas)
        }
        for (tile, image) in decoded {
            // The wire's origin is top-left; CoreGraphics drawing uses bottom-left.
            let rect = CGRect(x: tile.x, y: packet.height - tile.y - tile.height,
                              width: tile.width, height: tile.height)
            context.draw(image, in: rect)
        }
        guard let image = context.makeImage() else { throw DesktopProtocolError.allocationFailed }
        frames[packet.display] = Frame(image: image, sequence: packet.seq, synchronized: true)
        if selectedDisplay == nil { selectedDisplay = packet.display }
        touch(packet.display)
        evictImages()
        return true
    }
}
