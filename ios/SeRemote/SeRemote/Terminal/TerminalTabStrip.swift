import SwiftUI

struct TerminalTabStrip: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: SeSpace.xs) {
                ForEach(session.terminals.terminals) { item in
                    let isSelected = session.terminals.activeId == item.id
                    Button {
                        Task { await session.revealTerminal(item.id) }
                    } label: {
                        HStack(spacing: SeSpace.xs) {
                            Image(systemName: isSelected ? "terminal.fill" : "terminal")
                                .accessibilityHidden(true)
                            VStack(alignment: .leading, spacing: SeSpace.xxs) {
                                Text(item.title)
                                    .font(SeFont.label)
                                    .lineLimit(1)
                                if let folder = HostTimestamp.folderName(from: item.cwd) {
                                    Text(folder)
                                        .font(SeFont.caption)
                                        .foregroundStyle(SeTheme.secondaryInk)
                                        .lineLimit(1)
                                } else if let branch = item.gitBranch, !branch.isEmpty {
                                    Text(branch)
                                        .font(SeFont.caption)
                                        .foregroundStyle(SeTheme.secondaryInk)
                                        .lineLimit(1)
                                }
                            }
                        }
                        .foregroundStyle(isSelected ? SeTheme.accent : SeTheme.secondaryInk)
                        .padding(.horizontal, SeSpace.md)
                        .padding(.vertical, SeSpace.xs)
                        .frame(minHeight: SeSpace.hitTarget)
                        .background(isSelected ? SeTheme.accentSubtle : SeTheme.elevated, in: Capsule())
                        .overlay {
                            Capsule().strokeBorder(isSelected ? SeTheme.accent : SeTheme.stroke, lineWidth: 1)
                        }
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
                        .foregroundStyle(SeTheme.accent)
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        .background(SeTheme.elevated, in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("New Terminal"))
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.vertical, SeSpace.xs)
        }
        .background(SeTheme.surface)
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
        .seHaptic(trigger: session.terminals.activeId) { old, new in
            old != nil && new != nil ? .selection : nil
        }
    }
}
