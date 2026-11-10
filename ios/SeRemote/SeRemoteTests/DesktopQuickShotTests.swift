import CoreGraphics
import Foundation
import Testing
@testable import SeRemote

@Suite("Desktop quick shot geometry")
struct DesktopShotGeometryTests {
    private let bounds = CGRect(x: 0, y: 0, width: 400, height: 400)
    private let imageSize = CGSize(width: 1920, height: 1080)

    @Test func defaultSelectionIsSixtyPercentCentered() {
        let rect = DesktopShotGeometry.defaultSelection(in: imageSize)
        #expect(rect == CGRect(x: 384, y: 216, width: 1152, height: 648))
        #expect(rect.midX == imageSize.width / 2)
        #expect(rect.midY == imageSize.height / 2)
    }

    @Test func clampKeepsMinimumAndBounds() {
        let tiny = DesktopShotGeometry.clamped(CGRect(x: 10, y: 10, width: 4, height: 4), to: imageSize)
        #expect(tiny.width == 64)
        #expect(tiny.height == 64)

        let outside = DesktopShotGeometry.clamped(CGRect(x: 1900, y: -40, width: 200, height: 200), to: imageSize)
        #expect(outside.maxX <= imageSize.width)
        #expect(outside.minY >= 0)
        #expect(outside.width == 200)
        #expect(outside.height == 200)
    }

    @Test func clampHandlesFramesSmallerThanMinimum() {
        let rect = DesktopShotGeometry.clamped(CGRect(x: 5, y: 5, width: 100, height: 100), to: CGSize(width: 40, height: 30))
        #expect(rect == CGRect(x: 0, y: 0, width: 40, height: 30))
    }

    @Test func pixelSnapUsesWholePixels() {
        let snapped = DesktopShotGeometry.pixelSnapped(CGRect(x: 10.4, y: 20.6, width: 100.2, height: 50.5))
        #expect(snapped == CGRect(x: 10, y: 21, width: 100, height: 51))
        let prepared = DesktopShotGeometry.prepared(CGRect(x: 0.6, y: 0.6, width: 63.2, height: 63.2), in: imageSize)
        #expect(prepared == CGRect(x: 1, y: 1, width: 64, height: 64))
    }

    @Test func viewToImageMappingAtFit() {
        let point = DesktopShotGeometry.imagePoint(fromView: CGPoint(x: 200, y: 200), in: bounds, imageSize: imageSize)
        #expect(point == CGPoint(x: 960, y: 540))
        let topLeft = DesktopShotGeometry.imagePoint(fromView: CGPoint(x: 0, y: 87.5), in: bounds, imageSize: imageSize)
        #expect(topLeft == .zero)
        #expect(DesktopShotGeometry.imagePoint(fromView: CGPoint(x: 200, y: 10), in: bounds, imageSize: imageSize) == nil)
    }

    @Test func viewToImageMappingWithZoomAndPan() {
        let point = DesktopShotGeometry.imagePoint(
            fromView: .zero,
            in: bounds,
            imageSize: imageSize,
            zoom: 2,
            pan: CGSize(width: 50, height: -20)
        )
        #expect(point?.x == 360)
        #expect(point?.y == 108)

        let rect = CGRect(x: 0, y: 0, width: 100, height: 50)
        let view = DesktopShotGeometry.viewRect(
            fromImage: rect,
            in: bounds,
            imageSize: imageSize,
            zoom: 2,
            pan: CGSize(width: 50, height: -20)
        )
        #expect(view.minX == -150)
        #expect(view.minY == -45)
        #expect(abs(view.width - 100.0 / 1920.0 * 800.0) < 0.001)
        #expect(abs(view.height - 50.0 / 1080.0 * 450.0) < 0.001)

        let roundTrip = DesktopShotGeometry.imageRect(
            fromView: view,
            in: bounds,
            imageSize: imageSize,
            zoom: 2,
            pan: CGSize(width: 50, height: -20)
        )
        #expect(roundTrip?.origin.x == 0)
        #expect(roundTrip?.origin.y == 0)
    }

    @Test func cropUsesHostPixels() throws {
        let image = try #require(Self.makeImage(width: 200, height: 100))
        let cropped = try #require(DesktopShotCropper.crop(image, to: CGRect(x: 20, y: 10, width: 64, height: 64)))
        #expect(cropped.width == 64)
        #expect(cropped.height == 64)
        #expect(DesktopShotCropper.crop(image, to: CGRect(x: 500, y: 500, width: 20, height: 20)) == nil)
    }

    static func makeImage(width: Int, height: Int) -> CGImage? {
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
        for index in 0..<(width * height * 4) {
            pixels[index] = UInt8(truncatingIfNeeded: index &* 2_654_435_761)
        }
        return context.makeImage()
    }
}

@Suite("Desktop quick shot encoding")
struct DesktopShotEncodingTests {
    @Test func chatEncodingStaysUnderInlineLimitAndKeepsAspect() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 1200, height: 800))
        let shot = try #require(DesktopShotEncoder.encodeForChat(image))
        #expect(shot.data.count <= DesktopShotEncoder.maxInlineBytes)
        #expect(shot.mimeType == "image/jpeg")
        #expect(shot.pixelSize.width > 0)
        #expect(shot.pixelSize.height > 0)
        let aspect = shot.pixelSize.width / shot.pixelSize.height
        #expect(abs(aspect - 1.5) < 0.02)
        #expect(DesktopShotEncoder.resize(image, to: shot.pixelSize) != nil)
    }

    @Test func chatEncodingHonoursCustomBudget() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 1600, height: 1200))
        let shot = try #require(DesktopShotEncoder.encodeForChat(image, maxBytes: 8 * 1024))
        #expect(shot.data.count <= 8 * 1024)
    }

    @Test func pngEncodingKeepsFullPixelSize() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 320, height: 200))
        let shot = try #require(DesktopShotEncoder.encodePNG(image))
        #expect(shot.mimeType == "image/png")
        #expect(shot.pixelSize == CGSize(width: 320, height: 200))
    }
}

@Suite("Desktop shot target resolution")
struct DesktopShotTargetTests {
    @Test func agentChatWinsOverEverything() {
        let target = DesktopShotTargetResolver.resolve(
            activeTerminalID: "term-1",
            activeConversationID: "conv-1",
            agentChatTerminalID: "term-1"
        )
        #expect(target == .agentChat(id: "term-1"))
    }

    @Test func conversationBeatsTerminal() {
        let target = DesktopShotTargetResolver.resolve(
            activeTerminalID: "term-1",
            activeConversationID: "conv-1",
            agentChatTerminalID: nil
        )
        #expect(target == .chat(conversationID: "conv-1"))
    }

    @Test func terminalWorkbenchUsesTheConversationOnlyWhenShowingAndReady() {
        #expect(DesktopShotTargetResolver.terminalWorkbench(
            terminalID: "t", conversationShowing: true, conversationReady: true
        ) == .agentChat(id: "t"))
        #expect(DesktopShotTargetResolver.terminalWorkbench(
            terminalID: "t", conversationShowing: true, conversationReady: false
        ) == .terminal(id: "t"))
        #expect(DesktopShotTargetResolver.terminalWorkbench(
            terminalID: "t", conversationShowing: false, conversationReady: true
        ) == .terminal(id: "t"))
    }

    @Test func terminalTabSendsToTheTerminalEvenWithAConversation() {
        let target = DesktopShotTargetResolver.resolve(
            activeTerminalID: "term-1",
            activeConversationID: "conv-1",
            agentChatTerminalID: nil,
            terminalTabActive: true
        )
        #expect(target == .terminal(id: "term-1"))
        #expect(DesktopShotTargetResolver.resolve(
            activeTerminalID: nil,
            activeConversationID: "conv-1",
            agentChatTerminalID: nil,
            terminalTabActive: true
        ) == .chat(conversationID: "conv-1"))
    }

    @Test func terminalIsTheFallback() {
        let target = DesktopShotTargetResolver.resolve(
            activeTerminalID: "term-2",
            activeConversationID: nil,
            agentChatTerminalID: nil
        )
        #expect(target == .terminal(id: "term-2"))
        #expect(DesktopShotTargetResolver.resolve(
            activeTerminalID: nil,
            activeConversationID: nil,
            agentChatTerminalID: nil
        ) == nil)
    }

    @Test func onlyChatUsesInlineImageEncoding() {
        #expect(DesktopShotTarget.chat(conversationID: nil).isChat)
        #expect(!DesktopShotTarget.terminal(id: "t").isChat)
        #expect(!DesktopShotTarget.agentChat(id: "t").isChat)
    }
}

@Suite("Attachment staging transport")
struct AttachmentStageTests {
    private let origin = URL(string: "https://host.example:8443/base?token=link")!
    private let credentials = HostCredentials(accessURL: URL(string: "https://host.example:8443/base?token=link")!, bearer: "secret")

    @Test @MainActor func requestCarriesRawImageAndHeaders() throws {
        let data = Data([0x89, 0x50, 0x4E, 0x47])
        let request = AttachmentStageRequest(data: data, mimeType: "image/png")
            .make(origin: credentials.origin, credentials: credentials)
        #expect(request.httpMethod == "POST")
        #expect(request.url?.path == "/remote/attachments/stage")
        #expect(request.value(forHTTPHeaderField: "Content-Type") == "image/png")
        #expect(request.value(forHTTPHeaderField: "Accept") == "application/json")
        #expect(request.value(forHTTPHeaderField: "Content-Length") == String(data.count))
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer secret")
        #expect(request.value(forHTTPHeaderField: "Origin") == "https://host.example:8443")
        #expect(request.httpBody == data)
    }

    @Test func responseDecodesStagedAttachment() throws {
        let json = """
        {"success":true,"data":{"path":"/tmp/seremote/shot.png","bytes":1234,"width":800,"height":600,"mime":"image/png"}}
        """.data(using: .utf8)!
        let envelope = try JSONDecoder().decode(IpcResult<StagedAttachment>.self, from: json)
        #expect(envelope.success)
        #expect(envelope.data == StagedAttachment(
            path: "/tmp/seremote/shot.png",
            bytes: 1234,
            width: 800,
            height: 600,
            mime: "image/png"
        ))
    }

    @Test func hostCodesMapToClearClientFailures() {
        #expect(AttachmentStageError(code: "PAYLOAD_TOO_LARGE", statusCode: 413) == .payloadTooLarge)
        #expect(AttachmentStageError(code: "UNSUPPORTED_MEDIA_TYPE", statusCode: 415) == .unsupportedMediaType)
        #expect(AttachmentStageError(code: "INVALID_IMAGE", statusCode: 422) == .invalidImage)
        #expect(AttachmentStageError(code: "IMAGE_DIMENSIONS_TOO_LARGE", statusCode: 422) == .dimensionsTooLarge)
        #expect(AttachmentStageError(code: "FORBIDDEN", statusCode: 403) == .scopeMissing)
        #expect(AttachmentStageError(code: nil, statusCode: 403) == .scopeMissing)
        #expect(AttachmentStageError(code: "INTERNAL", statusCode: 500) == .failed("INTERNAL"))
        #expect(AttachmentStageError.scopeMissing.message
            == String(localized: "This device needs terminal input permission to send images."))
    }
}

@Suite("ACP image content blocks")
struct ChatPromptContentTests {
    @Test func imageComesBeforeCaptionAndBase64RoundTrips() throws {
        let bytes = Data([1, 2, 3, 4, 5])
        let image = ChatImageAttachment(ShotAttachment(
            data: bytes,
            mimeType: "image/jpeg",
            pixelSize: CGSize(width: 640, height: 480)
        ))
        let blocks = ChatPromptContent.blocks(text: "look here", image: image)
        #expect(blocks.count == 2)
        #expect(blocks[0]["type"] as? String == "image")
        #expect(blocks[0]["mimeType"] as? String == "image/jpeg")
        #expect(blocks[0]["data"] as? String == bytes.base64EncodedString())
        #expect(blocks[1]["type"] as? String == "text")
        #expect(blocks[1]["text"] as? String == "look here")
    }

    @Test func textOnlyAndUnsupportedMime() {
        #expect(ChatPromptContent.blocks(text: "hi", image: nil).count == 1)
        let weird = ChatImageAttachment(ShotAttachment(
            data: Data([0]),
            mimeType: "image/heic",
            pixelSize: CGSize(width: 10, height: 10)
        ))
        let blocks = ChatPromptContent.blocks(text: "hi", image: weird)
        #expect(blocks.count == 1)
        #expect(blocks[0]["type"] as? String == "text")
    }
}

@Suite("Desktop capture freshness")
struct DesktopFrameFreshnessTests {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    @Test func streamFrameIsFreshWithinTwoSeconds() {
        #expect(DesktopFrameFreshness.isFresh(updatedAt: now, now: now))
        #expect(DesktopFrameFreshness.isFresh(updatedAt: now.addingTimeInterval(-1.9), now: now))
        #expect(DesktopFrameFreshness.isFresh(updatedAt: now.addingTimeInterval(0.2), now: now))
    }

    @Test func olderFrameNeedsAFreshScreenshot() {
        #expect(!DesktopFrameFreshness.isFresh(updatedAt: now.addingTimeInterval(-2.5), now: now))
        #expect(!DesktopFrameFreshness.isFresh(updatedAt: .distantPast, now: now))
        #expect(!DesktopFrameFreshness.isFresh(updatedAt: nil, now: now))
    }

    @Test func selectionScalesOntoAReplacementImage() {
        let selection = CGRect(x: 100, y: 50, width: 400, height: 200)
        let scaled = DesktopShotGeometry.scaledSelection(
            selection,
            from: CGSize(width: 1920, height: 1080),
            to: CGSize(width: 960, height: 540)
        )
        #expect(scaled == CGRect(x: 50, y: 25, width: 200, height: 100))

        let upscaled = DesktopShotGeometry.scaledSelection(
            CGRect(x: 10, y: 10, width: 20, height: 20),
            from: CGSize(width: 100, height: 100),
            to: CGSize(width: 200, height: 400)
        )
        #expect(upscaled == CGRect(x: 20, y: 40, width: 40, height: 80))
    }

    @Test func selectionSurvivesAnUnknownDisplayedSize() {
        let selection = CGRect(x: 1, y: 2, width: 3, height: 4)
        #expect(DesktopShotGeometry.scaledSelection(selection, from: .zero, to: CGSize(width: 800, height: 600)) == selection)
        #expect(DesktopShotGeometry.scaledSelection(selection, from: CGSize(width: 800, height: 600), to: .zero) == selection)
    }
}

@Suite("Staged path shell escaping")
struct DesktopShellPathTests {
    @Test func spaceFreeHostPathIsTypedVerbatim() {
        #expect(DesktopShellPath.stagedArgument("/var/folders/6t/abc123/T/seremote-shot-1.png")
            == "/var/folders/6t/abc123/T/seremote-shot-1.png ")
    }

    @Test func spacesAndMetacharactersAreSingleQuoted() {
        #expect(DesktopShellPath.stagedArgument("/Users/qs/Library/Application Support/shot.png")
            == "'/Users/qs/Library/Application Support/shot.png' ")
        #expect(DesktopShellPath.stagedArgument("/tmp/a&b;c.png") == "'/tmp/a&b;c.png' ")
        #expect(DesktopShellPath.stagedArgument("/tmp/截图.png") == "'/tmp/截图.png' ")
    }

    @Test func embeddedSingleQuoteIsClosedAndReopened() {
        #expect(DesktopShellPath.escaped("/tmp/o'brien.png") == "'/tmp/o'\\''brien.png'")
        #expect(DesktopShellPath.stagedArgument("/tmp/o'brien.png").hasSuffix(" "))
    }

    @Test func emptyPathStillProducesTheTrailingSpace() {
        #expect(DesktopShellPath.stagedArgument("") == " ")
    }
}

@Suite("Desktop encoder seeding")
struct DesktopShotEncoderSeedTests {
    @Test func largeCropStartsAlreadyDownscaled() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 4096, height: 4096))
        let scales = DesktopShotEncoder.attemptScales(for: image)
        let first = try #require(scales.first)
        #expect(first < 0.1)
        #expect(scales == scales.sorted(by: >))
        #expect(!scales.contains(1))
    }

    @Test func smallCropStillStartsAtFullResolution() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 320, height: 200))
        #expect(DesktopShotEncoder.attemptScales(for: image) == DesktopShotEncoder.chatScales)
    }

    @Test func seededQualitySkipsAttemptsThatCannotFit() {
        let qualities = DesktopShotEncoder.attemptQualities(pixels: 122_880)
        #expect(qualities.first == 0.54)
        #expect(!qualities.contains(0.82))
        #expect(qualities == DesktopShotEncoder.chatQualities.filter { $0 <= 0.54 })
    }

    @Test func generousBudgetStartsAtBestQuality() {
        #expect(DesktopShotEncoder.attemptQualities(pixels: 1_000).first == 0.82)
        #expect(DesktopShotEncoder.attemptQualities(pixels: 80_000).first == 0.68)
        #expect(DesktopShotEncoder.attemptQualities(pixels: 120_000).first == 0.54)
    }

    @Test func noisyCropIsDownscaledBeforeTheBudgetLoop() throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 1200, height: 800))
        let shot = try #require(DesktopShotEncoder.encodeForChat(image))
        #expect(shot.data.count <= DesktopShotEncoder.maxInlineBytes)
        #expect(shot.pixelSize.width <= 500)
    }

    @Test func detachedEncodingKeepsTheBudget() async throws {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 1200, height: 800))
        let shot = try #require(await DesktopShotEncoder.encodeForChatDetached(image))
        #expect(shot.data.count <= DesktopShotEncoder.maxInlineBytes)
        #expect(shot.mimeType == "image/jpeg")
        let aspect = shot.pixelSize.width / shot.pixelSize.height
        #expect(abs(aspect - 1.5) < 0.02)
    }
}

@Suite("Chat send failure mapping")
struct ChatSendFailureTests {
    @Test func failuresUseExistingLocalizedSentences() {
        #expect(ChatSendFailure.host.message == ChatSendFailure.notConnected.message)
        #expect(ChatSendFailure.host.message == String(localized: "The screenshot could not be sent."))
        #expect(ChatSendFailure.noAgent.message != ChatSendFailure.host.message)
        #expect(DesktopShotSendError.chat(.host).message == ChatSendFailure.host.message)
        #expect(DesktopShotSendError.noTerminal.message == ChatSendFailure.host.message)
    }
}
