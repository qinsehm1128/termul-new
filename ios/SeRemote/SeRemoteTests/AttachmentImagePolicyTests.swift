import CoreGraphics
import Foundation
import Testing
import UIKit
@testable import SeRemote

@Suite("Attachment image policy")
struct AttachmentImagePolicyTests {
    @Test func pngWithinLimitsIsPassedThrough() throws {
        let data = try #require(Self.png(width: 320, height: 200))
        let shot = try #require(AttachmentImagePolicy.prepared(data: data, name: "shot.png"))
        #expect(shot.data == data)
        #expect(shot.mimeType == "image/png")
        #expect(shot.pixelSize == CGSize(width: 320, height: 200))
    }

    @Test func oversizedLongEdgeIsDownscaledTo4096() throws {
        let data = try #require(Self.png(width: 6000, height: 3000))
        let shot = try #require(AttachmentImagePolicy.prepared(data: data, name: "wide.png"))
        #expect(shot.mimeType == "image/jpeg")
        #expect(shot.pixelSize.width <= AttachmentImagePolicy.maxDimension)
        #expect(shot.pixelSize.width == 4096)
        #expect(shot.pixelSize.height == 2048)
        let aspect = shot.pixelSize.width / shot.pixelSize.height
        #expect(abs(aspect - 2) < 0.01)
        #expect(shot.data.count <= AttachmentImagePolicy.maxBytes)
    }

    @Test func anImageOverEightMebibytesIsReencodedUnderTheBudget() throws {
        #expect(AttachmentImagePolicy.maxBytes == 8 * 1024 * 1024)
        let data = try #require(Self.png(width: 2048, height: 2048, noise: true))
        #expect(data.count > AttachmentImagePolicy.maxBytes)
        let shot = try #require(AttachmentImagePolicy.prepared(data: data, name: "noise.png"))
        #expect(shot.data.count <= AttachmentImagePolicy.maxBytes)
        #expect(shot.mimeType == "image/jpeg")
        #expect(shot.pixelSize.width <= AttachmentImagePolicy.maxDimension)
        #expect(shot.pixelSize.height <= AttachmentImagePolicy.maxDimension)
    }

    @Test func theBudgetLadderHonoursASmallerBudget() throws {
        let data = try #require(Self.png(width: 1400, height: 1000, noise: true))
        let budget = 120 * 1024
        let shot = try #require(AttachmentImagePolicy.prepared(data: data, name: "noise.png", maxBytes: budget))
        #expect(shot.data.count <= budget)
        #expect(shot.mimeType == "image/jpeg")
    }

    @Test func nonImagesReturnNil() {
        #expect(AttachmentImagePolicy.prepared(data: Data("not an image".utf8), name: "notes.txt") == nil)
        #expect(AttachmentImagePolicy.prepared(data: Data(), name: "empty.png") == nil)
        #expect(AttachmentImagePolicy.prepared(
            data: Data("%PDF-1.7".utf8),
            name: "report.pdf"
        ) == nil)
    }

    @Test func detachedPreparationKeepsTheSamePolicy() async throws {
        let data = try #require(Self.png(width: 320, height: 200))
        let shot = await AttachmentImagePolicy.preparedDetached(data: data, name: "shot.png")
        #expect(shot?.mimeType == "image/png")
        #expect(shot?.pixelSize == CGSize(width: 320, height: 200))
    }

    // MARK: - Fixtures

    /// `noise` makes PNG incompressible, so the file is larger than the 8 MiB
    /// budget. The smooth ramp compresses to almost nothing, which keeps the
    /// dimension test about pixels, not bytes.
    private static func png(width: Int, height: Int, noise: Bool = false) -> Data? {
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ), let raw = context.data else { return nil }
        let pixels = raw.assumingMemoryBound(to: UInt8.self)
        if noise {
            var state: UInt32 = 0x9E37_79B9
            for index in 0 ..< (width * height * 4) {
                state ^= state << 13
                state ^= state >> 17
                state ^= state << 5
                pixels[index] = (index % 4 == 3) ? 255 : UInt8(truncatingIfNeeded: state)
            }
        } else {
            for y in 0 ..< height {
                for x in 0 ..< width {
                    let offset = (y * width + x) * 4
                    pixels[offset] = UInt8(truncatingIfNeeded: x / 8)
                    pixels[offset + 1] = UInt8(truncatingIfNeeded: y / 8)
                    pixels[offset + 2] = 96
                    pixels[offset + 3] = 255
                }
            }
        }
        guard let image = context.makeImage() else { return nil }
        return UIImage(cgImage: image).pngData()
    }
}
