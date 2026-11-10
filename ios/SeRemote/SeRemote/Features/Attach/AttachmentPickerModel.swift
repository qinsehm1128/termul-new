import Foundation
import Observation

extension DesktopShotTarget {
    var isAgentChat: Bool {
        if case .agentChat = self { return true }
        return false
    }
}

/// State and delivery for the attach sheet: where the image goes, whether it is
/// uploading, and the one sentence to show when it fails.
///
/// The sheet owns no transport: it injects `deliver`, which in production is
/// `DesktopShotSender.send(_:to:)`. Tests replace it with a closure, so the
/// target rule, the success path and the failure path are all asserted without
/// a host.
@MainActor
@Observable
final class AttachmentPickerModel {
    /// Agent when the terminal's agent is ready, otherwise the terminal itself.
    var target: DesktopShotTarget

    let terminalID: String
    let terminalTitle: String
    let agentReady: Bool

    private(set) var isUploading = false
    /// Success sentence. The sheet closes right after it is set; the caller
    /// shows the same text in the terminal.
    private(set) var notice: String?
    private(set) var errorMessage: String?
    /// The sheet was dismissed by a successful upload, not by the user.
    private(set) var isClosed = false

    private let deliver: (ShotAttachment, DesktopShotTarget) async throws -> Void
    private let close: () -> Void

    init(
        terminalID: String,
        terminalTitle: String,
        agentReady: Bool,
        deliver: @escaping (ShotAttachment, DesktopShotTarget) async throws -> Void,
        close: @escaping () -> Void = {}
    ) {
        self.terminalID = terminalID
        self.terminalTitle = terminalTitle
        self.agentReady = agentReady
        self.deliver = deliver
        self.close = close
        self.target = DesktopShotTargetResolver.attachment(terminalID: terminalID, agentReady: agentReady)
    }

    /// Production wiring: staging plus the two destinations the workbench
    /// already supports (PTY write, agent composer draft).
    convenience init(
        http: HostHTTP,
        terminalID: String,
        terminalTitle: String,
        agentReady: Bool,
        onWriteTerminal: @escaping (String) async -> Void,
        onDraftAgent: @escaping (String, String) -> Void,
        onClose: @escaping () -> Void
    ) {
        let sender = DesktopShotSender(
            http: http,
            writeTerminal: onWriteTerminal,
            draftAgentChat: onDraftAgent
        )
        self.init(
            terminalID: terminalID,
            terminalTitle: terminalTitle,
            agentReady: agentReady,
            deliver: { shot, target in try await sender.send(shot, to: target) },
            close: onClose
        )
    }

    var targetIsAgent: Bool { target.isAgentChat }

    /// The title shown after “Send to”.
    var targetName: String {
        target.isAgentChat ? String(localized: "Agent") : terminalTitle
    }

    /// The trailing chip: which destination the row currently means.
    var targetBadge: String {
        target.isAgentChat ? String(localized: "Agent") : String(localized: "Current terminal")
    }

    func sendToTerminal() {
        target = .terminal(id: terminalID)
    }

    func sendToAgent() {
        guard agentReady else { return }
        target = .agentChat(id: terminalID)
    }

    /// Preflights the bytes, stages them, then reports success or the one
    /// sentence to show. A failure leaves the sheet open with `errorMessage`.
    func send(data: Data, name: String) async {
        guard !isUploading else { return }
        errorMessage = nil
        notice = nil
        isUploading = true
        defer { isUploading = false }

        guard let shot = await AttachmentImagePolicy.preparedDetached(data: data, name: name) else {
            errorMessage = String(localized: "That file is not an image.")
            return
        }

        let destination = target
        do {
            try await deliver(shot, destination)
        } catch let error as AttachmentStageError {
            errorMessage = error.message
            return
        } catch let error as DesktopShotSendError {
            errorMessage = error.message
            return
        } catch {
            errorMessage = String(localized: "The image could not be sent.")
            return
        }

        notice = destination.isAgentChat
            ? String(localized: "Uploaded \(name) · drafted to the agent")
            : String(localized: "Uploaded \(name) · path written to terminal")
        isClosed = true
        close()
    }

    /// A picker that produced no readable bytes (a cancelled iCloud download,
    /// an unreadable file URL) reports through the same error area.
    func fail(_ message: String) {
        errorMessage = message
    }
}
