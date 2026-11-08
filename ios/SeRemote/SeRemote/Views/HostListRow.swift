import SwiftUI

struct HostListRow: View {
    let title: String
    var preview: String?
    var previewMono = false
    var meta: String?
    var status: HostRowStatus?
    var lifecycle: ShellHostLifecycle?
    var time: String?
    var glyph: String = "bubble.left.and.bubble.right"
    var showsChevron = false

    @ScaledMetric(relativeTo: .footnote) private var glyphSize: CGFloat = 22

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: glyph)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(SeTheme.muted)
                .frame(width: glyphSize, height: glyphSize)
                .padding(.top, 2)

            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)

                if let preview, !preview.isEmpty {
                    Text(preview)
                        .font(previewMono ? .subheadline.monospaced() : .subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }

                if let meta, !meta.isEmpty {
                    Text(meta)
                        .font(.caption)
                        .foregroundStyle(SeTheme.muted)
                        .lineLimit(1)
                }

                if let lifecycle {
                    HStack(spacing: 4) {
                        if lifecycle.kind == .unknown {
                            Image(systemName: "questionmark.circle")
                                .foregroundStyle(SeTheme.secondaryInk)
                        } else {
                            SeStatusBadge(status: lifecycle.kind == .deleted ? .offline : .idle, compact: true)
                                .accessibilityHidden(true)
                        }
                        Text(lifecycle.label)
                            .foregroundStyle(SeTheme.secondaryInk)
                    }
                    .font(SeFont.label)
                    .accessibilityElement(children: .combine)
                } else if let status {
                    SeStatusBadge(status: status.seStatus)
                }
            }

            Spacer(minLength: 8)

            if let time {
                Text(time)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .padding(.top, 2)
            } else if showsChevron {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .padding(.top, 4)
            }
        }
        .padding(.vertical, 10)
        .frame(minHeight: 56, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

struct HostStatusChip: View {
    let status: HostRowStatus

    var body: some View {
        SeStatusBadge(status: status.seStatus)
    }
}
