import SwiftUI
import UIKit

/// Prototype "Recently read" preview card: file identity, a read-only pill,
/// the source/rendered switch for Markdown and HTML, the real content, and
/// compact copy/share actions. Failures stay calm and code-derived.
@MainActor
struct WorkspaceFilePreviewView: View {
    @Environment(\.sePalette) private var palette
    let name: String
    let content: FileContent?
    let kind: FilePreviewKind?
    let imageData: Data?
    let failure: FilePreviewFailure?
    let isLoading: Bool
    let refresh: () async -> Void

    @State private var mode: PreviewMode = .rendered
    @State private var copied = false

    private var text: String? { content?.content }
    private var hasText: Bool {
        guard let text, !text.isEmpty else { return false }
        return true
    }
    private var showsModeToggle: Bool {
        failure == nil && hasText && kind?.supportsRenderedMode == true
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 11) {
                header
                if showsModeToggle {
                    PreviewModeToggle(mode: $mode)
                }
                if isLoading {
                    ProgressView("Refreshing preview…")
                        .font(.system(size: 11))
                        .tint(palette.accent)
                }
                contentView
                actions
            }
            .padding(13)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(palette.stroke, lineWidth: 1)
            }
            .padding(.horizontal, 18)
            .padding(.bottom, 12)
        }
        .refreshable { await refresh() }
        .background(palette.surface)
        .seHaptic(trigger: copied) { _, new in new ? .success : nil }
        .onChange(of: content?.content) { _, _ in copied = false }
        .onChange(of: name) { _, _ in
            copied = false
            mode = .rendered
        }
        .accessibilityIdentifier("files-preview")
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text(name)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .textSelection(.enabled)
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 4)
            R2Pill(text: String(localized: "Read only"), tone: .quiet)
        }
    }

    @ViewBuilder
    private var contentView: some View {
        if let failure {
            Text(message(for: failure))
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("files-preview-state")
        } else if let kind {
            switch kind {
            case .markdown:
                markdownContent
            case .html:
                htmlContent
            case .image:
                imageContent
            case .text:
                textContent
            case .unsupportedImage:
                Text("This image format can't be previewed.")
                    .font(.system(size: 12))
                    .foregroundStyle(palette.inkSecondary)
                    .accessibilityIdentifier("files-preview-state")
            case .unsupported:
                Text("This file can't be previewed.")
                    .font(.system(size: 12))
                    .foregroundStyle(palette.inkSecondary)
                    .accessibilityIdentifier("files-preview-state")
            }
        } else if hasText, let text {
            sourceText(text)
        } else if isLoading {
            EmptyView()
        } else {
            Text("This file has no text to preview.")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
        }
    }

    @ViewBuilder
    private var markdownContent: some View {
        if let text, !text.isEmpty {
            if mode == .rendered {
                MarkdownPreviewView(markdown: text)
                    .accessibilityIdentifier("files-preview-markdown")
            } else {
                sourceText(text)
            }
        } else {
            Text("This file has no text to preview.")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
        }
    }

    @ViewBuilder
    private var htmlContent: some View {
        if let text, !text.isEmpty {
            if mode == .rendered {
                SandboxedHTMLView(
                    html: text,
                    textColor: UIColor(palette.ink),
                    backgroundColor: .clear
                )
                .frame(height: 300)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(palette.stroke, lineWidth: 1)
                }
                .accessibilityIdentifier("files-preview-html")
            } else {
                sourceText(text)
            }
        } else {
            Text("This file has no text to preview.")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
        }
    }

    @ViewBuilder
    private var imageContent: some View {
        if let imageData, let image = UIImage(data: imageData) {
            ZoomableImagePreview(image: image, data: imageData, name: name)
        } else if !isLoading {
            Text("This file can't be previewed.")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
                .accessibilityIdentifier("files-preview-state")
        }
    }

    @ViewBuilder
    private var textContent: some View {
        if let text, !text.isEmpty {
            sourceText(text)
        } else {
            Text("This file has no text to preview.")
                .font(.system(size: 12))
                .foregroundStyle(palette.inkSecondary)
        }
    }

    private func sourceText(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11, design: .monospaced))
            .lineSpacing(4)
            .foregroundStyle(palette.inkSecondary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier("files-preview-text")
    }

    @ViewBuilder
    private var actions: some View {
        if let text, !text.isEmpty {
            HStack(spacing: 8) {
                Button {
                    UIPasteboard.general.string = text
                    copied = true
                } label: {
                    actionLabel(
                        copied ? String(localized: "Copied") : String(localized: "Copy"),
                        systemImage: copied ? "checkmark" : "doc.on.doc"
                    )
                }
                .buttonStyle(.plain)
                .accessibilityLabel(copied ? Text("File text copied") : Text("Copy file text"))
                .accessibilityIdentifier("files-copy")

                ShareLink(item: text) {
                    actionLabel("Share", systemImage: "square.and.arrow.up")
                }
                .accessibilityLabel(Text("Share file text"))
                .accessibilityIdentifier("files-share")
                Spacer(minLength: 0)
            }
        }
    }

    private func actionLabel(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(palette.inkSecondary)
            .padding(.horizontal, 10)
            .frame(minHeight: 32)
            .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(palette.stroke, lineWidth: 1)
            }
            .frame(minHeight: SeSpace.hitTarget)
            .contentShape(Rectangle())
    }

    private func message(for failure: FilePreviewFailure) -> String {
        switch failure {
        case .tooLarge: String(localized: "This file is too large to preview.")
        case .binary: String(localized: "Binary files can't be previewed.")
        case .unsupportedImage: String(localized: "This image format can't be previewed.")
        case .unavailable: String(localized: "This file can't be previewed.")
        }
    }
}

private enum PreviewMode: String, CaseIterable {
    case rendered
    case source

    var title: String {
        switch self {
        case .rendered: String(localized: "Rendered")
        case .source: String(localized: "Source")
        }
    }
}

/// Prototype-sized 源码 / 渲染 switch; defaults to rendered.
private struct PreviewModeToggle: View {
    @Environment(\.sePalette) private var palette
    @Binding var mode: PreviewMode

    var body: some View {
        HStack(spacing: 2) {
            ForEach(PreviewMode.allCases, id: \.self) { option in
                Button {
                    mode = option
                } label: {
                    Text(option.title)
                        .font(.system(size: 10.5, weight: .medium))
                        .foregroundStyle(mode == option ? palette.onAccent : palette.inkTertiary)
                        .padding(.horizontal, 10)
                        .frame(minHeight: 26)
                        .background(
                            mode == option ? palette.ink : Color.clear,
                            in: Capsule(style: .continuous)
                        )
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(mode == option ? .isSelected : [])
            }
        }
        .padding(2)
        .background(palette.cardAlt, in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous).stroke(palette.stroke, lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Preview mode"))
    }
}
