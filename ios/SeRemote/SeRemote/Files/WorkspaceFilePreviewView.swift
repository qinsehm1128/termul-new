import SwiftUI
import UIKit

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
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                VStack(alignment: .leading, spacing: SeSpace.xs) {
                    Text(name)
                        .font(SeFont.rowTitle)
                        .foregroundStyle(palette.ink)
                        .textSelection(.enabled)
                        .accessibilityAddTraits(.isHeader)
                    Label("Read-only preview", systemImage: "lock")
                        .font(SeFont.caption)
                        .foregroundStyle(palette.inkSecondary)
                    if isTextEncoding {
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: SeSpace.sm) { actions }
                            VStack(alignment: .leading, spacing: SeSpace.xs) { actions }
                        }
                    }
                }
                if isLoading {
                    ProgressView("Refreshing preview…")
                        .font(SeFont.support)
                        .tint(palette.accent)
                }
                if !isTextEncoding {
                    SeEmptyState(
                        title: String(localized: "Unsupported preview"),
                        systemImage: "doc.questionmark",
                        message: String(localized: "This format is not supported for text preview. Open it in the host's local web client.")
                    )
                } else if content.content.isEmpty {
                    SeEmptyState(
                        title: String(localized: "Empty file"),
                        systemImage: "doc",
                        message: String(localized: "This file has no text to preview.")
                    )
                } else {
                    SeCodeBlock(text: content.content)
                        .accessibilityIdentifier("files-preview-text")
                }
            }
            .padding(SeSpace.sm)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
            }
            .padding(SeSpace.screenInset)
        }
        .refreshable { await refresh() }
        .background(palette.canvas)
        .seHaptic(trigger: copied) { _, new in new ? .success : nil }
        .onChange(of: content.content) { _, _ in copied = false }
        .onChange(of: name) { _, _ in copied = false }
        .accessibilityIdentifier("files-preview")
    }

    @ViewBuilder
    private var actions: some View {
        Button {
            UIPasteboard.general.string = content.content
            copied = true
        } label: {
            Label(copied ? String(localized: "Copied") : String(localized: "Copy"), systemImage: copied ? "checkmark" : "doc.on.doc")
        }
        .buttonStyle(NocturneSessionButtonStyle())
        .accessibilityLabel(copied ? Text("File text copied") : Text("Copy file text"))
        .accessibilityIdentifier("files-copy")

        ShareLink(item: content.content) {
            Label("Share", systemImage: "square.and.arrow.up")
        }
        .buttonStyle(NocturneSessionButtonStyle())
        .accessibilityLabel(Text("Share file text"))
        .accessibilityIdentifier("files-share")
    }
}
