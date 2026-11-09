import SwiftUI

struct TerminalTabStrip: View {
    @Environment(\.sePalette) private var palette
    @Bindable var session: WorkspaceSession

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: SeSpace.xs) {
                ForEach(Array(session.terminals.terminals.enumerated()), id: \.element.id) { index, item in
                    let isSelected = session.terminals.activeId == item.id
                    Button {
                        Task { await session.revealTerminal(item.id) }
                    } label: {
                        HStack(spacing: SeSpace.xs) {
                            Text("\(index + 1)")
                                .font(.caption.monospaced())
                                .foregroundStyle(palette.inkTertiary)
                            Text(item.title)
                                .font(SeFont.label)
                                .lineLimit(1)
                        }
                        .foregroundStyle(isSelected ? palette.ink : palette.inkSecondary)
                        .padding(.horizontal, SeSpace.sm)
                        .frame(minHeight: SeSpace.hitTarget)
                        .overlay(alignment: .bottom) {
                            if isSelected {
                                Capsule().fill(palette.accent).frame(height: 2)
                            }
                        }
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
                        .foregroundStyle(palette.ink)
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        .overlay {
                            RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(palette.strokeStrong, lineWidth: 1)
                        }
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("New Terminal"))
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.vertical, SeSpace.xs)
        }
        .background(palette.canvas)
        .overlay(alignment: .bottom) {
            Rectangle().fill(palette.strokeStrong).frame(height: 1)
        }
        .seHaptic(trigger: session.terminals.activeId) { old, new in
            old != nil && new != nil ? .selection : nil
        }
    }
}
