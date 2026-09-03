import SwiftUI

struct HostListRow: View {
    let title: String
    var preview: String?
    var previewMono = false
    var meta: String?
    var status: HostRowStatus?
    var time: String?
    var glyph: String = "bubble.left.and.bubble.right"
    var showsChevron = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: glyph)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(SeTheme.muted)
                .frame(width: 22, height: 22)
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

                if let status {
                    HostStatusChip(status: status)
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
        HStack(spacing: 6) {
            if status == .working {
                Circle()
                    .fill(SeTheme.lamp)
                    .frame(width: 6, height: 6)
            }
            Text(status.title)
                .font(.caption.weight(.medium))
        }
        .foregroundStyle(foreground)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(background)
        .clipShape(Capsule())
    }

    private var foreground: Color {
        switch status {
        case .need: SeTheme.accent
        case .working: SeTheme.lamp
        case .idle: SeTheme.muted
        }
    }

    private var background: Color {
        switch status {
        case .need: SeTheme.accent.opacity(0.14)
        case .working: SeTheme.lamp.opacity(0.12)
        case .idle: SeTheme.stroke.opacity(0.45)
        }
    }
}
