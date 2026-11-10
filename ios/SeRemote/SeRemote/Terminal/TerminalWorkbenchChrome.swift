import SwiftUI

/// The terminal workbench chrome.
///
/// The terminal fills the surface; every affordance floats above it in the same
/// `ZStack`. The overlay panel, the scrim and the pill never participate in the
/// canvas layout, so presenting the panel cannot reflow the PTY (I2).
struct TerminalWorkbenchChrome<Canvas: View>: View {
    @Bindable var model: TerminalWorkbenchModel
    var keyboardVisible = false
    var canvas: Canvas
    var onBack: () -> Void
    var onFocusKeyboard: () -> Void
    var onToggleFit: () -> Void
    var onNudgeTextScale: (Int) -> Void
    var onSelectTab: (String) -> Void
    var onSpawnTerminal: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Fixed top gutter: the first terminal row clears the floating pill.
    static var topInset: CGFloat { 44 }

    private var presentation: TerminalPanelPresentation { model.panelPresentation }

    var body: some View {
        ZStack(alignment: .top) {
            VStack(spacing: 0) {
                Color.clear
                    .frame(height: Self.topInset)
                    .allowsHitTesting(false)
                canvas
            }

            if model.panelExpanded {
                scrim
                panel
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .zIndex(2)
            } else {
                topRow
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .transition(.opacity)
                    .zIndex(1)
            }

            if let error = model.displayError {
                Text(error)
                    .font(SeNocturneType.font(10.5, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(.ultraThinMaterial, in: Capsule())
                    .overlay { Capsule().strokeBorder(palette.danger.opacity(0.5), lineWidth: 1) }
                    .padding(.top, Self.topInset + 8)
                    .allowsHitTesting(false)
                    .zIndex(3)
            }
        }
        .overlay(alignment: .bottomTrailing) {
            if !keyboardVisible, !model.panelExpanded {
                cluster
                    .padding(.trailing, 14)
                    .padding(.bottom, 14)
                    .transition(.opacity.combined(with: .scale(scale: 0.9)))
            }
        }
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion),
                   value: model.panelExpanded)
    }

    // MARK: - Collapsed chrome

    private var topRow: some View {
        HStack(alignment: .center, spacing: 8) {
            titlePill
            Spacer(minLength: 8)
            if model.agentReady {
                agentChip
            }
        }
    }

    private var titlePill: some View {
        HStack(spacing: 0) {
            Button(action: onBack) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .frame(width: 34, height: 40)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Back"))

            Rectangle()
                .fill(mode.insetStroke)
                .frame(width: 1, height: 18)
                .padding(.horizontal, 2)

            Text(verbatim: presentation.title)
                .font(SeNocturneType.font(12, weight: .medium, mono: true))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .frame(maxWidth: 190, alignment: .leading)
                .padding(.leading, 8)

            Text(verbatim: presentation.counter)
                .font(SeNocturneType.font(9, mono: true))
                .foregroundStyle(palette.inkSecondary)
                .padding(.horizontal, 5)
                .frame(height: 18)
                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                .padding(.leading, 6)

            Button {
                model.panelExpanded.toggle()
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 32, height: 40)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Terminal menu"))
            .accessibilityAddTraits(model.panelExpanded ? .isSelected : [])
        }
        .frame(height: 40)
        .background(.ultraThinMaterial, in: Capsule(style: .continuous))
        .background(palette.card.opacity(0.55), in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous).strokeBorder(mode.insetStroke, lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
    }

    private var agentChip: some View {
        Button {
            model.present(.agent)
        } label: {
            HStack(spacing: 6) {
                Circle()
                    .fill(palette.live)
                    .frame(width: 6, height: 6)
                Text(verbatim: model.agentName ?? String(localized: "Agent"))
                    .font(SeNocturneType.font(11, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                if model.agentPermissionCount > 0 {
                    Text(verbatim: "\(model.agentPermissionCount)")
                        .font(SeNocturneType.font(9, weight: .semibold, mono: true))
                        .foregroundStyle(palette.onAccent)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(palette.warning, in: Circle())
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(.ultraThinMaterial, in: Capsule(style: .continuous))
            .background(palette.card.opacity(0.55), in: Capsule(style: .continuous))
            .overlay {
                Capsule(style: .continuous).strokeBorder(mode.insetStroke, lineWidth: 1)
            }
            .contentShape(Capsule(style: .continuous))
            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text("Agent conversation"))
        .accessibilityValue(Text(model.agentPermissionCount > 0
            ? "\(model.agentPermissionCount) pending"
            : String(localized: "Idle")))
    }

    // MARK: - Overlay panel

    private var scrim: some View {
        Color.black.opacity(mode.isNight ? 0.45 : 0.18)
            .ignoresSafeArea()
            .contentShape(Rectangle())
            .onTapGesture { model.panelExpanded = false }
            .transition(.opacity)
            .accessibilityLabel(Text("Collapse panel"))
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { model.panelExpanded = false }
    }

    private var panel: some View {
        VStack(alignment: .leading, spacing: 0) {
            panelHeader
            if presentation.showsTabs {
                tabRow
            }
            ptyRow
            if let agent = presentation.agent {
                agentRow(agent)
                    .padding(.top, 8)
            }
            actionRow
                .padding(.top, 8)
        }
        .padding(12)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .background(palette.card.opacity(0.86), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .strokeBorder(mode.insetStroke, lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Terminal panel"))
    }

    private var panelHeader: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(verbatim: presentation.title)
                .font(SeNocturneType.font(17, weight: .semibold))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            if let subtitle = presentation.subtitle, !subtitle.isEmpty {
                Text(verbatim: subtitle)
                    .font(SeNocturneType.font(10, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 6)
            Button {
                model.panelExpanded = false
            } label: {
                Image(systemName: "chevron.up")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 30, height: 30)
                    .background(mode.insetFill, in: Circle())
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            .accessibilityLabel(Text("Collapse panel"))
        }
        .padding(.bottom, 10)
    }

    private var tabRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(presentation.tabs) { tab in
                    let isSelected = tab.id == presentation.activeTabID
                    Button {
                        model.panelExpanded = false
                        onSelectTab(tab.id)
                    } label: {
                        HStack(spacing: 5) {
                            Text(verbatim: "\(tab.index + 1)")
                                .font(SeNocturneType.font(9, mono: true))
                                .foregroundStyle(palette.inkTertiary.opacity(0.7))
                            Text(verbatim: tab.title)
                                .font(SeNocturneType.font(12))
                                .lineLimit(1)
                        }
                        .foregroundStyle(isSelected ? palette.ink : palette.inkTertiary)
                        .padding(.horizontal, 10)
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
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(isSelected ? .isSelected : [])
                }

                Button(action: onSpawnTerminal) {
                    HStack(spacing: 4) {
                        Image(systemName: "plus")
                            .font(.system(size: 10, weight: .semibold))
                        Text("New")
                            .font(SeNocturneType.font(11, weight: .medium))
                    }
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.horizontal, 10)
                    .frame(height: 30)
                    .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .frame(minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.leading, 6)
                .accessibilityLabel(Text("New terminal"))
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(mode.hairline).frame(height: 1) }
    }

    private var ptyRow: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(presentation.pty.isReady ? palette.live : palette.warning)
                .frame(width: 6, height: 6)
            Text(verbatim: presentation.pty.status)
                .font(SeNocturneType.font(11, weight: .medium))
                .foregroundStyle(palette.inkSecondary)
            Text(verbatim: presentation.pty.dimensions)
                .font(SeNocturneType.font(10, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 6)
            if presentation.pty.showsFit {
                panelChip("FIT", isActive: presentation.pty.fitActive, action: onToggleFit)
                    .accessibilityLabel(Text("Fit the terminal to the phone"))
                    .accessibilityValue(Text(presentation.pty.fitActive
                        ? String(localized: "Active")
                        : String(localized: "Idle")))
            }
            Menu {
                Button { onNudgeTextScale(1) } label: {
                    Label(String(localized: "Larger Text"), systemImage: "textformat.size.larger")
                }
                Button { onNudgeTextScale(-1) } label: {
                    Label(String(localized: "Smaller Text"), systemImage: "textformat.size.smaller")
                }
                Button { onNudgeTextScale(0) } label: {
                    Label(String(localized: "Reset Text Size"), systemImage: "arrow.counterclockwise")
                }
            } label: {
                Text(verbatim: presentation.pty.textScaleLabel)
                    .font(SeNocturneType.font(10, weight: .medium, mono: true))
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.horizontal, 8)
                    .frame(height: 28)
                    .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(Text("Text size"))
        }
        .padding(.top, 10)
    }

    private func panelChip(_ label: String, isActive: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(verbatim: label)
                .font(SeNocturneType.font(10, weight: .medium, mono: true))
                .foregroundStyle(isActive ? palette.canvas : palette.inkSecondary)
                .padding(.horizontal, 8)
                .frame(height: 28)
                .background(isActive ? palette.ink : mode.insetFill,
                            in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func agentRow(_ agent: TerminalPanelPresentation.AgentRow) -> some View {
        Button {
            model.present(.agent)
        } label: {
            HStack(spacing: 8) {
                Circle().fill(agent.permissionCount > 0 ? palette.warning : palette.live)
                    .frame(width: 7, height: 7)
                Text(verbatim: agent.name)
                    .font(SeNocturneType.font(12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Text(verbatim: agent.status)
                    .font(SeNocturneType.font(10, mono: true))
                    .foregroundStyle(agent.permissionCount > 0 ? palette.warning : palette.inkTertiary)
                Spacer(minLength: 4)
                Text("Chat")
                    .font(SeNocturneType.font(11, weight: .medium))
                    .foregroundStyle(palette.inkSecondary)
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(palette.inkTertiary)
            }
            .padding(.horizontal, 10)
            .frame(height: 44)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text("Agent conversation"))
        .accessibilityValue(Text(agent.permissionCount > 0
            ? "\(agent.permissionCount) pending"
            : agent.status))
    }

    private var actionRow: some View {
        HStack(spacing: 8) {
            actionTile(title: String(localized: "Files"), icon: .folder, label: "Files") {
                model.present(.files)
            }
            actionTile(title: String(localized: "Desktop"), icon: .desktop, label: "Desktop") {
                model.present(.desktop)
            }
            actionTile(title: String(localized: "More"), icon: .more, label: "More") {
                model.present(.more)
            }
        }
    }

    private func actionTile(
        title: String,
        icon: SeIcon,
        label: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                SeIconView(icon: icon, size: 17, stroke: 1.6, color: palette.inkSecondary)
                Text(verbatim: title)
                    .font(SeNocturneType.font(11, weight: .medium))
                    .foregroundStyle(palette.inkSecondary)
            }
            .frame(maxWidth: .infinity, minHeight: 58)
            .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(mode.insetStroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(label))
    }

    // MARK: - Floating cluster

    private var cluster: some View {
        HStack(spacing: 0) {
            Button(action: { model.present(.attach) }) {
                Image(systemName: "plus")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .frame(width: 46, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Attach"))

            Rectangle()
                .fill(mode.insetStroke)
                .frame(width: 1, height: 18)

            Button(action: onFocusKeyboard) {
                Image(systemName: "keyboard")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .frame(width: 46, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("Keyboard"))
        }
        .background(.ultraThinMaterial, in: Capsule(style: .continuous))
        .background(palette.card.opacity(0.55), in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous).strokeBorder(mode.insetStroke, lineWidth: 1)
        }
    }
}
