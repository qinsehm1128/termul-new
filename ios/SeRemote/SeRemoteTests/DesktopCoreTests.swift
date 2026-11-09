import CoreGraphics
import Foundation
import ImageIO
import Testing
@testable import SeRemote

@Suite("Remote desktop pure core")
@MainActor
struct DesktopCoreTests {
    private func jpeg(width: Int = 128, height: Int = 128, red: CGFloat = 1,
                      green: CGFloat = 0, blue: CGFloat = 0) throws -> Data {
        let context = try #require(CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                            bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.setFillColor(red: red, green: green, blue: blue, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let image = try #require(context.makeImage())
        let data = NSMutableData()
        let destination = try #require(CGImageDestinationCreateWithData(data, "public.jpeg" as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 1] as CFDictionary)
        #expect(CGImageDestinationFinalize(destination))
        return data as Data
    }

    private func wire(type: String = "key", display: String = "display-1", seq: UInt64 = 1,
                      width: Int = 128, height: Int = 128,
                      rectangles: [[String: Any]]? = nil, payload: Data) throws -> Data {
        let header: [String: Any] = ["type": type, "display": display, "seq": seq, "w": width, "h": height,
                                    "tiles": rectangles ?? [["x": 0, "y": 0, "w": width, "h": height, "len": payload.count]]]
        let json = try JSONSerialization.data(withJSONObject: header, options: [.sortedKeys])
        let length = UInt32(json.count)
        return Data([UInt8((length >> 24) & 255), UInt8((length >> 16) & 255),
                     UInt8((length >> 8) & 255), UInt8(length & 255)]) + json + payload
    }

    private func key(display: String = "display-1", seq: UInt64 = 1,
                     width: Int = 128, height: Int = 128, green: CGFloat = 0) throws -> DesktopPacket {
        try DesktopPacket.decode(wire(display: display, seq: seq, width: width, height: height,
                                      payload: jpeg(width: width, height: height, red: 1 - green, green: green)))
    }

    private func pixel(_ image: CGImage, x: Int, y: Int) throws -> [UInt8] {
        let data = try #require(image.dataProvider?.data)
        let bytes = try #require(CFDataGetBytePtr(data))
        let offset = y * image.bytesPerRow + x * 4
        return Array(UnsafeBufferPointer(start: bytes + offset, count: 4))
    }

    @Test func metadataUsesExactHostFieldNames() throws {
        let json = """
        {"supported":true,"viewEnabled":true,"controlEnabled":false,
        "permissions":{"screenRecording":true,"accessibility":false},
        "displays":[{"id":"native:93","name":"Retina","pixel_width":3024,"pixel_height":1964,
        "scale":2,"origin_x":-1512,"origin_y":10,"is_primary":false}]}
        """
        let info = try JSONDecoder().decode(DesktopInfo.self, from: Data(json.utf8))
        #expect(info.supported && info.viewEnabled && !info.controlEnabled)
        #expect(info.permissions.screenRecording && !info.permissions.accessibility)
        let display = try #require(info.displays.first)
        #expect(display.id == "native:93" && display.pixelWidth == 3024 && display.pixelHeight == 1964)
        #expect(display.scale == 2 && display.originX == -1512 && display.originY == 10 && !display.isPrimary)
    }

    @Test func unsupportedHostCanHaveNoDisplays() throws {
        let info = try JSONDecoder().decode(DesktopInfo.self, from: Data("""
        {"supported":false,"viewEnabled":true,"controlEnabled":false,
        "permissions":{"screenRecording":false,"accessibility":false},"displays":[]}
        """.utf8))
        #expect(!info.supported && info.displays.isEmpty)
    }

    @Test func decodesKeyAndNonzeroBasedDataSlice() throws {
        let data = try wire(payload: jpeg())
        let padded = Data([0]) + data
        let packet = try DesktopPacket.decode(padded.dropFirst())
        #expect(packet.type == .key && packet.seq == 1 && packet.display == "display-1")
        #expect(packet.tiles.count == 1 && packet.tiles[0].width == 128)
    }

    @Test func rejectsShortOversizedOrNonJSONHeaders() throws {
        for data in [Data(), Data([0, 0, 0]), Data([0, 0, 0, 0]), Data([0, 4, 0, 1]),
                     Data([0, 0, 0, 2, 123]), Data([0, 0, 0, 1, 255])] {
            #expect(throws: DesktopProtocolError.self) { try DesktopPacket.decode(data) }
        }
        #expect(throws: DesktopProtocolError.self) {
            try DesktopPacket.decode(Data(count: DesktopPacket.maxPacketBytes + 1))
        }
    }

    @Test func rejectsTruncatedAndTrailingPayloads() throws {
        let data = try wire(payload: jpeg())
        #expect(throws: DesktopProtocolError.self) { try DesktopPacket.decode(data.dropLast()) }
        #expect(throws: DesktopProtocolError.self) { try DesktopPacket.decode(data + Data([0])) }
    }

    @Test func rejectsCanvasDimensionsAndUnboundedDisplayIds() throws {
        let payload = try jpeg()
        for (width, height) in [(0, 128), (-1, 128), (16_385, 1), (8192, 8192), (Int.max, Int.max)] {
            #expect(throws: DesktopProtocolError.self) {
                try DesktopPacket.decode(wire(width: width, height: height, payload: payload))
            }
        }
        for display in ["", "   ", String(repeating: "x", count: 257)] {
            #expect(throws: DesktopProtocolError.self) { try DesktopPacket.decode(wire(display: display, payload: payload)) }
        }
    }

    @Test func rejectsPartialKeyEmptyTilesAndInvalidRectangles() throws {
        let payload = try jpeg()
        let rect = ["x": 0, "y": 0, "w": 128, "h": 128, "len": payload.count]
        for rectangles in [[], [rect, rect], [["x": 1, "y": 0, "w": 127, "h": 128, "len": payload.count]],
                           [["x": 0, "y": 0, "w": 128, "h": 128, "len": Int.max]],
                           [["x": Int.max, "y": 0, "w": 128, "h": 128, "len": payload.count]],
                           [["x": 0, "y": -1, "w": 128, "h": 128, "len": payload.count]]] {
            #expect(throws: DesktopProtocolError.self) { try DesktopPacket.decode(wire(rectangles: rectangles, payload: payload)) }
        }
    }

    @Test func rejectsTooManyOrNoncanonicalDeltaTiles() throws {
        let payload = try jpeg()
        let rect = ["x": 0, "y": 0, "w": 128, "h": 128, "len": payload.count]
        #expect(throws: DesktopProtocolError.self) {
            try DesktopPacket.decode(wire(type: "tiles", rectangles: Array(repeating: rect, count: 4097), payload: payload))
        }
        for rects in [[rect, rect], [["x": 1, "y": 0, "w": 128, "h": 128, "len": payload.count]],
                      [["x": 0, "y": 0, "w": 127, "h": 128, "len": payload.count]]] {
            #expect(throws: DesktopProtocolError.self) {
                try DesktopPacket.decode(wire(type: "tiles", width: 256, rectangles: rects, payload: payload + payload))
            }
        }
    }

    @Test func acceptsCroppedRightAndBottomEdgeTile() throws {
        let payload = try jpeg(width: 2, height: 3)
        let packet = try DesktopPacket.decode(wire(type: "tiles", width: 130, height: 131,
            rectangles: [["x": 128, "y": 128, "w": 2, "h": 3, "len": payload.count]], payload: payload))
        #expect(packet.tiles[0].x == 128 && packet.tiles[0].height == 3)
    }

    @Test func rejectsNonJPEGOrDimensionMismatchBeforeComposition() throws {
        for payload in [Data([1, 2, 3, 4]), try jpeg(width: 64, height: 64)] {
            #expect(throws: DesktopProtocolError.invalidJPEG) { try DesktopPacket.decode(wire(payload: payload)) }
        }
    }

    @Test func keyframeAndDeltaComposeInTopLeftCoordinates() throws {
        let compositor = DesktopFrameCompositor()
        #expect(try compositor.apply(key(width: 256, height: 256)))
        let green = try jpeg(red: 0, green: 1)
        let delta = try DesktopPacket.decode(wire(type: "tiles", seq: 2, width: 256, height: 256,
            rectangles: [["x": 0, "y": 0, "w": 128, "h": 128, "len": green.count]], payload: green))
        #expect(try compositor.apply(delta))
        let image = try #require(compositor.image(for: "display-1"))
        let upperLeft = try pixel(image, x: 20, y: 20)
        let lowerLeft = try pixel(image, x: 20, y: 200)
        #expect(upperLeft[1] > 230 && upperLeft[0] < 20)
        #expect(lowerLeft[0] > 230 && lowerLeft[1] < 20)
    }

    @Test func stalePacketsDropAndDisplaysRetainIndependentFrames() throws {
        let compositor = DesktopFrameCompositor()
        #expect(try compositor.apply(key(seq: 9)))
        let original = try #require(compositor.image(for: "display-1"))
        #expect(try !compositor.apply(key(seq: 8, green: 1)))
        #expect(try !compositor.apply(key(seq: 9, green: 1)))
        #expect(try compositor.apply(key(display: "display-2", seq: 1, green: 1)))
        #expect(compositor.image(for: "display-1") === original)
        #expect(compositor.image(for: "display-2") != nil)
        compositor.reset(display: "display-2")
        #expect(compositor.image(for: "display-2") == nil && compositor.image(for: "display-1") === original)
    }

    @Test func keyframeResetsCanvasSizeAndNewStreamRetainsLastImage() throws {
        let compositor = DesktopFrameCompositor()
        try compositor.apply(key(seq: 10))
        let old = try #require(compositor.image(for: "display-1"))
        compositor.beginStream()
        #expect(compositor.image(for: "display-1") === old)
        try compositor.apply(key(seq: 0, width: 64, height: 32, green: 1))
        #expect(compositor.image(for: "display-1")?.width == 64)
        #expect(compositor.image(for: "display-1")?.height == 32)
        compositor.reset()
        #expect(compositor.image(for: "display-1") == nil)
    }

    @Test func deltaRequiresSynchronizedMatchingKeyframe() throws {
        let compositor = DesktopFrameCompositor()
        let payload = try jpeg()
        let delta = try DesktopPacket.decode(wire(type: "tiles", seq: 2, payload: payload))
        #expect(throws: DesktopProtocolError.missingKeyframe) { try compositor.apply(delta) }
        try compositor.apply(key(width: 64, height: 64))
        #expect(throws: DesktopProtocolError.missingKeyframe) { try compositor.apply(delta) }
        compositor.beginStream()
        #expect(throws: DesktopProtocolError.missingKeyframe) { try compositor.apply(delta) }
    }

    @Test func oneFailedTileLeavesEntireCanvasAndSequenceUnchanged() throws {
        let compositor = DesktopFrameCompositor()
        try compositor.apply(key(width: 256))
        let original = try #require(compositor.image(for: "display-1"))
        let green = try jpeg(red: 0, green: 1)
        // Construct a damaged already-framed packet to exercise the compositor's
        // independent full-decode guard after the first valid tile has decoded.
        let broken = DesktopPacket(type: .tiles, display: "display-1", seq: 2, width: 256, height: 128,
            tiles: [.init(x: 0, y: 0, width: 128, height: 128, jpeg: green),
                    .init(x: 128, y: 0, width: 128, height: 128, jpeg: Data([0xff, 0xd8]))])
        #expect(throws: DesktopProtocolError.invalidJPEG) { try compositor.apply(broken) }
        #expect(compositor.image(for: "display-1") === original)
        #expect(try compositor.apply(key(seq: 2, width: 256, green: 1)))
    }

    @Test func aspectFitLetterboxAndRetinaNegativeOriginAreNormalizedLocally() {
        let viewport = DesktopViewport(pixelSize: CGSize(width: 2000, height: 1000), viewportSize: CGSize(width: 400, height: 400))
        #expect(viewport.imageRect == CGRect(x: 0, y: 100, width: 400, height: 200))
        #expect(viewport.normalizedPoint(CGPoint(x: 200, y: 200)) == CGPoint(x: 0.5, y: 0.5))
        #expect(viewport.normalizedPoint(CGPoint(x: 400, y: 300)) == CGPoint(x: 1, y: 1))
        #expect(viewport.normalizedPoint(CGPoint(x: 200, y: 50)) == nil)
    }

    @Test func zoomPanAndDragCoordinatesAreClamped() {
        let viewport = DesktopViewport(pixelSize: CGSize(width: 2000, height: 1000),
                                       viewportSize: CGSize(width: 400, height: 400), zoom: 2,
                                       pan: CGPoint(x: 1000, y: -1000))
        #expect(viewport.imageRect == CGRect(x: 0, y: 0, width: 800, height: 400))
        #expect(viewport.normalizedPoint(CGPoint(x: 400, y: 200)) == CGPoint(x: 0.5, y: 0.5))
        #expect(viewport.clampedNormalizedPoint(CGPoint(x: -999, y: 999)) == CGPoint(x: 0, y: 1))
        #expect(DesktopViewport(pixelSize: .zero, viewportSize: .zero).normalizedPoint(.zero) == nil)
        #expect(viewport.normalizedPoint(CGPoint(x: CGFloat.nan, y: 0)) == nil)
        #expect(DesktopViewport(pixelSize: CGSize(width: 1, height: 1), viewportSize: CGSize(width: 100, height: 100), zoom: 999).zoom == 8)
    }

    @Test func directTapAndTrackpadTapUseDifferentPointerPositions() {
        let viewport = DesktopViewport(pixelSize: CGSize(width: 100, height: 100), viewportSize: CGSize(width: 100, height: 100))
        var direct = DesktopGestureMapper(display: "A", mode: .direct)
        var trackpad = DesktopGestureMapper(display: "A", mode: .trackpad)
        let tap = DesktopGesture.tap(location: CGPoint(x: 10, y: 20), count: 2, fingers: 1)
        #expect(direct.messages(for: tap, in: viewport) == [.pointer(display: "A", u: 0.1, v: 0.2), .click(display: "A", button: "left", count: 2)])
        #expect(trackpad.messages(for: tap, in: viewport) == [.pointer(display: "A", u: 0.5, v: 0.5), .click(display: "A", button: "left", count: 2)])
        #expect(trackpad.messages(for: .move(location: .zero, delta: CGSize(width: 100, height: -100)), in: viewport) == [.pointer(display: "A", u: 1, v: 0)])
    }

    @Test func rightClickDragScrollAndLocalOnlyGestures() {
        let viewport = DesktopViewport(pixelSize: CGSize(width: 100, height: 100), viewportSize: CGSize(width: 100, height: 100))
        var mapper = DesktopGestureMapper(display: "A", mode: .direct)
        let point = CGPoint(x: 20, y: 30)
        #expect(mapper.messages(for: .tap(location: point, count: 1, fingers: 2), in: viewport).last == .click(display: "A", button: "right", count: 1))
        #expect(mapper.messages(for: .longPress(location: point), in: viewport) == [.pointer(display: "A", u: 0.2, v: 0.3), .click(display: "A", button: "right", count: 1)])
        #expect(mapper.messages(for: .dragBegan(location: point), in: viewport).last == .button(button: "left", down: true))
        #expect(mapper.messages(for: .dragChanged(location: CGPoint(x: 999, y: -999), delta: .zero), in: viewport) == [.pointer(display: "A", u: 1, v: 0)])
        #expect(mapper.messages(for: .dragEnded, in: viewport) == [.button(button: "left", down: false)])
        #expect(mapper.cancelDrag().isEmpty)
        #expect(mapper.messages(for: .scroll(delta: CGSize(width: 2, height: -3)), in: viewport) == [.scroll(display: "A", dx: 2, dy: -3)])
        #expect(mapper.messages(for: .localOnly, in: viewport).isEmpty)
        #expect(mapper.messages(for: .tap(location: point, count: 1, fingers: 3), in: viewport).isEmpty)
    }

    @Test func inputJSONMatchesWireAndSerializesUnicodeTextAndModifiers() throws {
        let messages: [DesktopInputMessage] = [.pointer(display: "native", u: 0.25, v: 0.75),
            .button(button: "left", down: true), .click(display: "native", button: "right", count: 2),
            .scroll(display: "native", dx: -2, dy: 3),
            .key(code: "KeyA", down: true, modifiers: .init(shift: true, control: false, alt: true, meta: true)), .text("你好🙂")]
        for message in messages { #expect(JSONSerialization.isValidJSONObject(message.json)) }
        #expect(messages[0].json["display"] as? String == "native")
        #expect(messages[1].json["display"] == nil)
        #expect(messages[2].json["count"] as? Int == 2)
        #expect((messages[4].json["modifiers"] as? [String: Bool])?["meta"] == true)
        #expect(messages[5].json["text"] as? String == "你好🙂")
        #expect(DesktopModifiers() == .init(shift: false, control: false, alt: false, meta: false))
    }

    @Test func pointerCoalescerEmitsLatestValueAtThirtyHzAndNeverReordersClick() {
        var coalescer = DesktopPointerCoalescer()
        let first = DesktopInputMessage.pointer(display: "A", u: 0, v: 0)
        let second = DesktopInputMessage.pointer(display: "A", u: 0.5, v: 0.5)
        let third = DesktopInputMessage.pointer(display: "A", u: 1, v: 1)
        #expect(coalescer.offer(first, at: 0) == first)
        #expect(coalescer.offer(second, at: 0.01) == nil)
        #expect(coalescer.offer(third, at: 0.02) == nil)
        #expect(coalescer.flush(at: 0.03) == nil)
        #expect(coalescer.flush(at: 1.0 / 30) == third)
        #expect(coalescer.offer(second, at: 0.04) == nil)
        let click = DesktopInputMessage.click(display: "A", button: "left", count: 1)
        #expect(coalescer.submit(click, at: 0.041) == [second, click])
        #expect(coalescer.flush(at: 1) == nil)
        coalescer.reset()
        #expect(coalescer.offer(first, at: 0) == first)
    }

    @Test func deltaSequenceGapRequiresKeyframeWithoutReplacingLastImage() throws {
        let compositor = DesktopFrameCompositor()
        try compositor.apply(key(seq: 10))
        let original = try #require(compositor.image(for: "display-1"))
        let payload = try jpeg(red: 0, green: 1)
        let gap = try DesktopPacket.decode(wire(type: "tiles", seq: 12, payload: payload))
        #expect(throws: DesktopProtocolError.missingKeyframe) { try compositor.apply(gap) }
        #expect(compositor.image(for: "display-1") === original)
        let lateAncestor = try DesktopPacket.decode(wire(type: "tiles", seq: 11, payload: payload))
        #expect(throws: DesktopProtocolError.missingKeyframe) { try compositor.apply(lateAncestor) }
        #expect(try !compositor.apply(key(seq: 9)))
        #expect(try compositor.apply(key(seq: 15, green: 1)))
        let next = try DesktopPacket.decode(wire(type: "tiles", seq: 16, payload: payload))
        #expect(try compositor.apply(next))
    }

    @Test func clockFreeCoalescerKeepsOnlyLatestPointerAndTakeClearsIt() {
        var coalescer = DesktopPointerCoalescer()
        let first = DesktopInputMessage.pointer(display: "A", u: 0, v: 0)
        let last = DesktopInputMessage.pointer(display: "A", u: 1, v: 1)
        coalescer.offer(first)
        coalescer.offer(last)
        #expect(coalescer.take() == last)
        #expect(coalescer.take() == nil)
        coalescer.offer(first)
        coalescer.reset()
        #expect(coalescer.take() == nil)
    }

    @Test func scrollJSONRoundsClampsAndUsesIntegerTokens() throws {
        let rounded = DesktopInputMessage.scroll(display: "A", dx: 1.6, dy: -2.5).json
        #expect(rounded["dx"] as? Int == 2 && rounded["dy"] as? Int == -3)
        let bounded = DesktopInputMessage.scroll(display: "A", dx: .greatestFiniteMagnitude, dy: -.greatestFiniteMagnitude).json
        #expect(bounded["dx"] as? Int == Int(Int32.max) && bounded["dy"] as? Int == Int(Int32.min))
        let invalid = DesktopInputMessage.scroll(display: "A", dx: .nan, dy: .infinity).json
        #expect(invalid["dx"] as? Int == 0 && invalid["dy"] as? Int == 0)
        let json = try JSONSerialization.data(withJSONObject: rounded, options: [.sortedKeys])
        let text = try #require(String(data: json, encoding: .utf8))
        #expect(text.contains("\"dx\":2") && text.contains("\"dy\":-3"))
    }

    @Test func stableErrorsRemainDistinctAndNeverEchoUnknownServerStrings() {
        let mapping: [(String, DesktopIssue)] = [("DESKTOP_DISABLED", .disabled), ("CONTROL_DISABLED", .controlDisabled),
            ("SCREEN_UNSUPPORTED", .unsupported), ("SCREEN_PERMISSION_REQUIRED", .screenPermission),
            ("INPUT_PERMISSION_REQUIRED", .inputPermission), ("DISPLAY_NOT_FOUND", .displayNotFound),
            ("CAPTURE_FAILED", .captureFailed), ("INPUT_FAILED", .inputFailed), ("INVALID_REQUEST", .invalidRequest)]
        for (code, expected) in mapping {
            #expect(DesktopIssue(code: code) == expected)
            #expect(!expected.message.isEmpty)
        }
        #expect(DesktopIssue(code: "FORBIDDEN", capability: "screen.view") == .viewForbidden)
        #expect(DesktopIssue(code: "FORBIDDEN", capability: "screen.control") == .controlForbidden)
        #expect(DesktopIssue(code: "Bearer secret") == .unknown)
        #expect(!DesktopIssue(code: "Bearer secret").message.contains("secret"))
    }
}
