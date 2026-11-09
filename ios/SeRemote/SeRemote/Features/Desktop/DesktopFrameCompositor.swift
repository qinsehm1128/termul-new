import CoreGraphics
import Foundation
import ImageIO

/// Each display owns its canvas and sequence; selecting another display never destroys it.
/// No canvas is published until every tile has decoded and the replacement image exists.
@MainActor
final class DesktopFrameCompositor {
    private struct Frame {
        let image: CGImage
        let sequence: UInt64?
        let synchronized: Bool
    }
    private var frames: [String: Frame] = [:]

    func image(for display: String) -> CGImage? { frames[display]?.image }

    func reset(display: String) { frames.removeValue(forKey: display) }
    func reset() { frames.removeAll() }

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
        return true
    }
}
