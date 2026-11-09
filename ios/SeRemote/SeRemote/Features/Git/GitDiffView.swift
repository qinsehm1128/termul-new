import SwiftUI

/// Read-only unified diff rendering in the prototype's terminal surface.
/// It never offers stage, commit or rollback controls.
struct GitDiffView: View {
    @Environment(\.sePalette) private var palette

    let diff: UnifiedDiff
    let rawText: String
    let path: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if diff.files.isEmpty {
                if let rawText = rawText.nocturneNonEmpty {
                    rawSurface(rawText)
                } else {
                    NocturneNotice(
                        message: String(localized: "The host returned an empty diff for this file."),
                        isError: false
                    )
                }
            } else {
                ForEach(Array(diff.files.enumerated()), id: \.offset) { _, file in
                    fileBlock(file)
                }
            }
        }
    }

    private func fileBlock(_ file: UnifiedDiffFile) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(verbatim: file.path.isEmpty ? path : file.path)
                    .font(SeNocturneType.font(10.5, weight: .semibold, mono: true))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if file.isNew {
                    NocturnePill(text: String(localized: "New"), tone: .live)
                } else if file.isDeleted {
                    NocturnePill(text: String(localized: "Deleted"), tone: .wait)
                } else if file.isBinary {
                    NocturnePill(text: String(localized: "Binary"), tone: .off)
                }
            }

            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(file.headers.enumerated()), id: \.offset) { _, header in
                        lineRow(header)
                    }
                    ForEach(Array(file.hunks.enumerated()), id: \.offset) { _, hunk in
                        lineRow(hunk.header)
                        ForEach(Array(hunk.lines.enumerated()), id: \.offset) { _, line in
                            lineRow(line)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 13)
                .padding(.vertical, 14)
            }
            .scrollIndicators(.hidden)
            .background(palette.terminalCanvas, in: RoundedRectangle(cornerRadius: 14))
            .overlay {
                RoundedRectangle(cornerRadius: 14)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
        }
    }

    private func rawSurface(_ text: String) -> some View {
        ScrollView([.horizontal, .vertical]) {
            Text(verbatim: text)
                .font(.system(size: 9.5, design: .monospaced))
                .foregroundStyle(palette.terminalInk)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 13)
                .padding(.vertical, 14)
        }
        .scrollIndicators(.hidden)
        .frame(maxHeight: 190)
        .background(palette.terminalCanvas, in: RoundedRectangle(cornerRadius: 14))
        .overlay {
            RoundedRectangle(cornerRadius: 14)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func lineRow(_ line: UnifiedDiffLine) -> some View {
        HStack(spacing: 4) {
            if line.kind != .header {
                Text(verbatim: GitDiffPresentation.prefix(for: line))
                    .foregroundStyle(prefixColor(line.kind))
            }
            Text(verbatim: line.content)
                .foregroundStyle(contentColor(line.kind))
                .lineLimit(1)
        }
        .font(.system(size: 9.5, design: .monospaced))
    }

    private func prefixColor(_ kind: UnifiedDiffLine.Kind) -> Color {
        switch kind {
        case .addition: palette.live
        case .removal: palette.danger
        case .context: palette.inkTertiary
        case .header: palette.inkTertiary
        }
    }

    private func contentColor(_ kind: UnifiedDiffLine.Kind) -> Color {
        switch kind {
        case .addition: palette.live
        case .removal: palette.danger
        case .context: palette.terminalInk
        case .header: palette.inkTertiary
        }
    }
}
