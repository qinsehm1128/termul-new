import Foundation
import Testing
import UIKit
@testable import SeRemote

@Suite("Attachment picker model")
@MainActor
struct AttachmentPickerModelTests {
    @Test func targetFollowsAgentReadiness() {
        let ready = Self.model(agentReady: true)
        #expect(ready.target == .agentChat(id: "term-1"))
        #expect(ready.targetIsAgent)
        #expect(ready.targetName == String(localized: "Agent"))
        #expect(ready.targetBadge == String(localized: "Agent"))

        let plain = Self.model(agentReady: false)
        #expect(plain.target == .terminal(id: "term-1"))
        #expect(!plain.targetIsAgent)
        #expect(plain.targetName == "zsh — Terminal")
        #expect(plain.targetBadge == String(localized: "Current terminal"))
    }

    @Test func theResolverIsTheSingleTargetRule() {
        #expect(DesktopShotTargetResolver.attachment(terminalID: "t", agentReady: true) == .agentChat(id: "t"))
        #expect(DesktopShotTargetResolver.attachment(terminalID: "t", agentReady: false) == .terminal(id: "t"))
    }

    @Test func switchingTargetNeverPicksAnUnreadyAgent() {
        let ready = Self.model(agentReady: true)
        ready.sendToTerminal()
        #expect(ready.target == .terminal(id: "term-1"))
        ready.sendToAgent()
        #expect(ready.target == .agentChat(id: "term-1"))

        let plain = Self.model(agentReady: false)
        plain.sendToAgent()
        #expect(plain.target == .terminal(id: "term-1"))
    }

    @Test func successToTerminalDeliversTheImageAndCloses() async throws {
        var deliveries: [(target: DesktopShotTarget, shot: ShotAttachment)] = []
        var didClose = false
        let model = Self.model(agentReady: false, deliver: { shot, target in
            deliveries.append((target, shot))
        }, close: {
            didClose = true
        })
        let data = try Self.pngData()

        await model.send(data: data, name: "shot.png")

        #expect(deliveries.count == 1)
        #expect(deliveries.first?.target == .terminal(id: "term-1"))
        #expect(deliveries.first?.shot.mimeType == "image/png")
        #expect(model.errorMessage == nil)
        let name = "shot.png"
        #expect(model.notice == String(localized: "Uploaded \(name) · path written to terminal"))
        #expect(model.isClosed)
        #expect(didClose)
        #expect(!model.isUploading)
    }

    @Test func successToAgentReportsTheDraft() async throws {
        var deliveredTarget: DesktopShotTarget?
        var didClose = false
        let model = Self.model(agentReady: true, deliver: { _, target in
            deliveredTarget = target
        }, close: {
            didClose = true
        })

        await model.send(data: try Self.pngData(), name: "shot.png")

        #expect(deliveredTarget == .agentChat(id: "term-1"))
        let name = "shot.png"
        #expect(model.notice == String(localized: "Uploaded \(name) · drafted to the agent"))
        #expect(model.errorMessage == nil)
        #expect(didClose)
    }

    @Test func failureKeepsTheSheetOpen() async throws {
        var attempts = 0
        var didClose = false
        let model = Self.model(agentReady: false, deliver: { _, _ in
            attempts += 1
            throw AttachmentStageError.payloadTooLarge
        }, close: {
            didClose = true
        })

        await model.send(data: try Self.pngData(), name: "shot.png")

        #expect(attempts == 1)
        #expect(!model.isClosed)
        #expect(!didClose)
        #expect(model.notice == nil)
        #expect(model.errorMessage == AttachmentStageError.payloadTooLarge.message)
        #expect(!model.isUploading)
    }

    @Test func aNonImageNeverReachesTheHost() async {
        var attempts = 0
        let model = Self.model(agentReady: false, deliver: { _, _ in
            attempts += 1
        })

        await model.send(data: Data("not an image".utf8), name: "notes.txt")

        #expect(attempts == 0)
        #expect(!model.isClosed)
        #expect(model.errorMessage == String(localized: "That file is not an image."))
    }

    @Test func anUnreadablePickCanReportThroughTheSameErrorArea() {
        let model = Self.model(agentReady: false)
        model.fail(String(localized: "That file could not be read."))
        #expect(model.errorMessage == String(localized: "That file could not be read."))
        #expect(!model.isClosed)
    }

    // MARK: - Fixtures

    private static func model(
        agentReady: Bool,
        deliver: @escaping (ShotAttachment, DesktopShotTarget) async throws -> Void = { _, _ in },
        close: @escaping () -> Void = {}
    ) -> AttachmentPickerModel {
        AttachmentPickerModel(
            terminalID: "term-1",
            terminalTitle: "zsh — Terminal",
            agentReady: agentReady,
            deliver: deliver,
            close: close
        )
    }

    private static func pngData() throws -> Data {
        let image = try #require(DesktopShotGeometryTests.makeImage(width: 64, height: 48))
        return try #require(UIImage(cgImage: image).pngData())
    }
}
