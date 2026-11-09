import SwiftUI

/// Prototype `.tabstrip`: index + title tabs with a 2pt accent underline and a
/// compact add button. Terminal close has no host route on the phone, so the
/// prototype's `x` is intentionally omitted rather than faked.
struct TerminalTabStrip: View {
    @Environment(\.sePalette) private var palette
    @Bindable var session: WorkspaceSession

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array(session.terminals.terminals.enumerated()), id: \.element.id) { index, item in
                    let isSelected = session.terminals.activeId == item.id
                    Button {
                        Task { await session.revealTerminal(item.id) }
                    } label: {
                        HStack(spacing: 6) {
                            Text("\(index + 1)")
                                .font(.system(size: 9, design: .monospaced))
                                .foregroundStyle(palette.inkTertiary.opacity(0.6))
                            Text(item.title)
                                .font(.system(size: 12))
                                .lineLimit(1)
                        }
                        .foregroundStyle(isSelected ? palette.ink : palette.inkTertiary)
                        .padding(.horizontal, 11)
                        .frame(height: 34)
                        .overlay(alignment: .bottom) {
                            if isSelected {
                                Capsule()
                                    .fill(palette.accent)
                                    .frame(height: 2)
                                    .padding(.horizontal, 8)
                            }
                        }
                        .frame(minHeight: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                        // Selection chrome only; no animation reaches the PTY.
                        .seAnimation(.control, value: isSelected)
                    }
                    .buttonStyle(.plain)
                    .accessibilityValue(Text(isSelected ? String(localized: "Active") : String(localized: "Idle")))
                    .accessibilityAddTraits(isSelected ? .isSelected : [])
                    .contextMenu {
                        Button {
                            UIPasteboard.general.string = item.cwd
                        } label: {
                            Label(String(localized: "Copy Path"), systemImage: "doc.on.doc")
                        }
                    }
                }

                Button {
                    Task { await session.spawnTerminal() }
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.inkTertiary)
                        .frame(width: 28, height: 28)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .stroke(palette.stroke, lineWidth: 1)
                        }
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("New Terminal"))
                .padding(.leading, 2)
            }
            .padding(.horizontal, 12)
            .padding(.top, 12)
        }
        .background(palette.surface)
        .overlay(alignment: .bottom) {
            Rectangle().fill(palette.stroke).frame(height: 1)
        }
        .seHaptic(trigger: session.terminals.activeId) { old, new in
            old != nil && new != nil ? .selection : nil
        }
    }
}
