import Foundation

/// What the read-only preview surface can render for a file.
///
/// Dispatch is extension-first because the host chooses the transport by
/// extension too: `/fs/read` for UTF-8 text and `/fs/image` for its raster
/// allowlist. SVGs stay text (the host deliberately refuses them as images);
/// HEIC/HEIF are reported as images the host cannot preview.
nonisolated enum FilePreviewKind: Equatable, Sendable {
    case markdown
    case html
    case image(ImageFormat)
    case text
    /// A recognisable image whose format the host does not serve.
    case unsupportedImage
    /// Binary content, or text in an encoding the host never returns.
    case unsupported

    nonisolated struct ImageFormat: Equatable, Sendable {
        let mime: String
    }

    nonisolated var isImage: Bool {
        if case .image = self { return true }
        return false
    }

    /// Markdown and HTML offer the prototype's 源码 / 渲染 switch.
    nonisolated var supportsRenderedMode: Bool {
        self == .markdown || self == .html
    }

    /// - Parameters:
    ///   - name: file name only; directories never reach the preview.
    ///   - encoding: the host-declared encoding when it has already read the
    ///     file, otherwise `nil` (before a read, or for the image transport).
    nonisolated static func resolve(name: String, encoding: String?) -> FilePreviewKind {
        let fileExtension = (name as NSString).pathExtension.lowercased()
        switch fileExtension {
        case "md", "markdown", "mdx":
            return isSupportedText(encoding) ? .markdown : .unsupported
        case "html", "htm":
            return isSupportedText(encoding) ? .html : .unsupported
        case "png", "apng":
            return .image(ImageFormat(mime: "image/png"))
        case "jpg", "jpeg", "jfif":
            return .image(ImageFormat(mime: "image/jpeg"))
        case "gif":
            return .image(ImageFormat(mime: "image/gif"))
        case "webp":
            return .image(ImageFormat(mime: "image/webp"))
        case "bmp":
            return .image(ImageFormat(mime: "image/bmp"))
        case "ico":
            return .image(ImageFormat(mime: "image/x-icon"))
        case "avif":
            return .image(ImageFormat(mime: "image/avif"))
        case "heic", "heif":
            // `/fs/image` only serves raster formats it can signature-check.
            return .unsupportedImage
        case "svg":
            // UTF-8 XML: readable as source, never executed as markup.
            return textKind(encoding: encoding) ?? .unsupported
        default:
            return textKind(encoding: encoding) ?? .unsupported
        }
    }

    private nonisolated static func isSupportedText(_ encoding: String?) -> Bool {
        FilePanelPresentation.supportsTextPreview(encoding: encoding)
    }

    private nonisolated static func textKind(encoding: String?) -> FilePreviewKind? {
        isSupportedText(encoding) ? .text : nil
    }
}

/// Calm, code-derived preview failures. Raw host messages never reach the card.
nonisolated enum FilePreviewFailure: Equatable, Sendable {
    case tooLarge
    case binary
    case unsupportedImage
    case unavailable

    /// `nil` means the error carried no preview-relevant meaning and the
    /// caller should keep its existing error surface.
    nonisolated static func resolve(_ error: Error) -> FilePreviewFailure? {
        guard let hostError = error as? HostError else { return nil }
        switch hostError {
        case .ipc(_, let code):
            switch code {
            case "FILE_TOO_LARGE": return .tooLarge
            case "BINARY_FILE": return .binary
            case "UNSUPPORTED_IMAGE_FORMAT", "INVALID_IMAGE": return .unsupportedImage
            default: return .unavailable
            }
        case .network, .unexpected, .rePairingRequired, .deviceRevoked, .forbidden:
            return .unavailable
        }
    }
}
