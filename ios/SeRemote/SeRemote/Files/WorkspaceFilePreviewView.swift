import SwiftUI
import UIKit

@MainActor
struct WorkspaceFilePreviewView: View {
    let name: String
    let content: FileContent
    let isLoading: Bool
    let refresh: () async -> Void
    @State private var copied = false

    private var isTextEncoding: Bool {
        guard let encoding = content.encoding else { return true }
        return ["utf-8", "utf8"].contains(encoding.lowercased())
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                VStack(alignment: .leading, spacing: SeSpace.xs) {
                    Text(name)
                        .font(SeFont.rowTitle)
                        .foregroundStyle(SeTheme.accent)
                        .textSelection(.enabled)
                        .accessibilityAddTraits(.isHeader)
                    Label("Read-only preview", systemImage: "lock")
                        .font(SeFont.caption)
                        .foregroundStyle(SeTheme.secondaryInk)
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
                        .tint(SeTheme.accent)
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
            .padding(SeSpace.screenInset)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .refreshable { await refresh() }
        .background(SeTheme.canvas)
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
        .buttonStyle(SeSecondaryButtonStyle())
        .accessibilityLabel(copied ? Text("File text copied") : Text("Copy file text"))
        .accessibilityIdentifier("files-copy")

        ShareLink(item: content.content) {
            Label("Share", systemImage: "square.and.arrow.up")
        }
        .buttonStyle(SeSecondaryButtonStyle())
        .accessibilityLabel(Text("Share file text"))
        .accessibilityIdentifier("files-share")
    }
}
