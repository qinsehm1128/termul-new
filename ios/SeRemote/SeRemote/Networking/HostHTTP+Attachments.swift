import Foundation

/// `data` of the `POST /remote/attachments/stage` IPC envelope.
nonisolated struct StagedAttachment: Decodable, Equatable, Sendable {
    let path: String
    let bytes: Int
    let width: Int
    let height: Int
    let mime: String
}

/// Client-facing staging failures. The sheet maps these to one sentence with a
/// next action; raw host text never reaches the user.
nonisolated enum AttachmentStageError: Error, Equatable {
    /// The paired device lacks `terminal.input`.
    case scopeMissing
    /// Only PNG and JPEG are accepted.
    case unsupportedMediaType
    /// Declared Content-Type did not match the image bytes.
    case contentTypeMismatch
    /// Body exceeded the host's 8 MiB cap.
    case payloadTooLarge
    /// The bytes were not a readable PNG/JPEG.
    case invalidImage
    /// A dimension exceeded the host's 4096 px cap.
    case dimensionsTooLarge
    case emptyBody
    /// Anything else, with the host's stable code retained for diagnostics.
    case failed(String)

    init(code: String?, statusCode: Int) {
        if statusCode == 403 || statusCode == 401 ||
            code == "FORBIDDEN" || code == "SCOPE_REQUIRED" || code == "UNAUTHORIZED" {
            self = .scopeMissing
            return
        }
        switch code {
        case "UNSUPPORTED_MEDIA_TYPE": self = .unsupportedMediaType
        case "CONTENT_TYPE_MISMATCH": self = .contentTypeMismatch
        case "PAYLOAD_TOO_LARGE": self = .payloadTooLarge
        case "INVALID_IMAGE": self = .invalidImage
        case "IMAGE_DIMENSIONS_TOO_LARGE": self = .dimensionsTooLarge
        case "EMPTY_BODY": self = .emptyBody
        case let code?: self = .failed(code)
        case nil: self = .failed("HTTP_\(statusCode)")
        }
    }

    var message: String {
        switch self {
        case .scopeMissing:
            String(localized: "This device needs terminal input permission to stage screenshots.")
        case .unsupportedMediaType, .contentTypeMismatch:
            String(localized: "Only PNG and JPEG screenshots can be sent.")
        case .payloadTooLarge:
            String(localized: "The screenshot is too large to send.")
        case .invalidImage:
            String(localized: "The screenshot could not be read.")
        case .dimensionsTooLarge:
            String(localized: "The screenshot is larger than 4096 px.")
        case .emptyBody:
            String(localized: "The screenshot was empty.")
        case .failed:
            String(localized: "The host could not stage the screenshot.")
        }
    }
}

/// Builds the raw-body stage request so the wire shape is unit-testable
/// without a live host.
nonisolated struct AttachmentStageRequest {
    static let path = "/remote/attachments/stage"

    let data: Data
    let mimeType: String

    @MainActor
    func make(origin: URL, credentials: HostCredentials) -> URLRequest {
        var base = origin.absoluteString
        while base.hasSuffix("/") {
            base.removeLast()
        }
        let url = URL(string: base + Self.path) ?? origin.appending(path: Self.path)
        var request = URLRequest(url: url, timeoutInterval: 30)
        request.httpMethod = "POST"
        request.assumesHTTP3Capable = false
        request.setValue(mimeType, forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue(String(data.count), forHTTPHeaderField: "Content-Length")
        credentials.apply(to: &request)
        request.httpBody = data
        return request
    }
}

extension HostHTTP {
    func stageAttachment(_ attachment: ShotAttachment) async throws -> StagedAttachment {
        try await stageAttachment(data: attachment.data, mimeType: attachment.mimeType)
    }

    func stageAttachment(data: Data, mimeType: String) async throws -> StagedAttachment {
        let request = AttachmentStageRequest(data: data, mimeType: mimeType)
            .make(origin: origin, credentials: credentials)
        let (body, response) = try await HostURLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw AttachmentStageError.failed("NO_HTTP_RESPONSE")
        }
        let envelope = try? JSONDecoder().decode(IpcResult<StagedAttachment>.self, from: body)
        if let envelope, envelope.success, let staged = envelope.data {
            return staged
        }
        let code = envelope?.code ?? Self.stageErrorCode(body)
        throw AttachmentStageError(code: code, statusCode: http.statusCode)
    }

    private static func stageErrorCode(_ data: Data) -> String? {
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        return object?["code"] as? String
    }
}
