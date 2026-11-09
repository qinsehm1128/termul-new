import SwiftUI
import UIKit

/// Prototype "Recently read" preview card: an eyebrow + read-only pill, the
/// selected file's real text, and compact copy/share actions. No fabricated
/// language badge, size or age beyond what the host returned.
@MainActor
struct WorkspaceFilePreviewView: View {
    @Environment(\.sePalette) private var palette
    let name: String
    let content: FileContent
    let isLoading: Bool
    let refresh: () async -> Void
    @State private var copied = false

    private var isTextEncoding: Bool {
        SessionFilesPresentation.supportsTextPreview(encoding: content.encoding)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
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

                if isLoading {
                    ProgressView("Refreshing preview…")
                        .font(.system(size: 11))
                        .tint(palette.accent)
                }

                if !isTextEncoding {
                    Text("This format is not supported for text preview. Open it in the host's local web client.")
                        .font(.system(size: 12))
                        .foregroundStyle(palette.inkSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else if content.content.isEmpty {
                    Text("This file has no text to preview.")
                        .font(.system(size: 12))
                        .foregroundStyle(palette.inkSecondary)
                } else {
                    Text(content.content)
                        .font(.system(size: 11, design: .monospaced))
                        .lineSpacing(4)
                        .foregroundStyle(palette.inkSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("files-preview-text")
                }

                if isTextEncoding {
                    HStack(spacing: 8) {
                        Button {
                            UIPasteboard.general.string = content.content
                            copied = true
                        } label: {
                            Label(copied ? String(localized: "Copied") : String(localized: "Copy"),
                                  systemImage: copied ? "checkmark" : "doc.on.doc")
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
                        .buttonStyle(.plain)
                        .accessibilityLabel(copied ? Text("File text copied") : Text("Copy file text"))
                        .accessibilityIdentifier("files-copy")

                        ShareLink(item: content.content) {
                            Label("Share", systemImage: "square.and.arrow.up")
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
                        .accessibilityLabel(Text("Share file text"))
                        .accessibilityIdentifier("files-share")
                        Spacer(minLength: 0)
                    }
                }
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
        .onChange(of: content.content) { _, _ in copied = false }
        .onChange(of: name) { _, _ in copied = false }
        .accessibilityIdentifier("files-preview")
    }
}
