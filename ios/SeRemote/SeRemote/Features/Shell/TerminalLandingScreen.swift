import SwiftUI

/// T1 — the terminal list.
///
/// The directory the app lands on after connecting: a "continue" hero for the
/// last terminal, live terminals grouped by project, the quick-terminal
/// shortcuts, and one new-terminal entry. Stateless by design: every action is
/// forwarded so the shell owns attachment and the host keeps the PTY.
struct TerminalLandingScreen: View {
    let hostName: String
    let hostMeta: String
    let model: TerminalLandingModel
    var isRefreshing = false
    var notice: String?
    var errorMessage: String?
    var themeIcon: SeIcon
    var onRefresh: () -> Void
    var onSelectHost: () -> Void
    var onToggleTheme: () -> Void
    var onOpenDrawer: () -> Void
    var onOpenCommand: () -> Void
    var onOpen: (TerminalLandingModel.Row) -> Void
    var onOpenQuick: (TerminalLandingModel.QuickRow) -> Void
    var onRenameQuick: (TerminalLandingModel.QuickRow) -> Void
    var onCloseQuick: (TerminalLandingModel.QuickRow) -> Void
    var onDeleteQuick: (TerminalLandingModel.QuickRow) -> Void
    var onNewTerminal: () -> Void
    var onNewQuickTerminal: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                SeHostSwitch(
                    title: hostName,
                    meta: hostMeta,
                    onHost: onSelectHost,
                    onTheme: onToggleTheme,
                    themeIcon: themeIcon,
                    onRail: onOpenDrawer,
                    onCommand: onOpenCommand
                )
                .padding(.top, 8)

                header
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 14)

                if let notice {
                    NocturneNotice(message: notice, isError: false)
                        .padding(.horizontal, NocturneToolMetrics.gutter)
                        .padding(.top, 12)
                }
                if let errorMessage {
                    NocturneNotice(message: errorMessage, isError: true)
                        .padding(.horizontal, NocturneToolMetrics.gutter)
                        .padding(.top, 12)
                }

                if let hero = model.hero {
                    heroCard(hero)
                        .padding(.horizontal, NocturneToolMetrics.gutter)
                        .padding(.top, 16)
                }

                if model.isEmpty {
                    emptyState
                        .padding(.horizontal, NocturneToolMetrics.gutter)
                        .padding(.top, 16)
                } else {
                    ForEach(model.groups) { group in
                        groupHeader(group)
                        VStack(spacing: NocturneToolMetrics.cardGap) {
                            ForEach(group.rows) { row in
                                terminalRow(row)
                            }
                        }
                        .padding(.horizontal, NocturneToolMetrics.gutter)
                    }
                }

                quickSection

                newTerminalButton
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 22)

                Color.clear.frame(height: 28)
            }
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: model.hero?.id)
    }

    // MARK: - Header

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 5) {
                SeDisplayText(text: String(localized: "Terminals"))
                Text(verbatim: subtitle)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Button(action: onRefresh) {
                if isRefreshing {
                    ProgressView()
                        .controlSize(.small)
                        .frame(width: 30, height: 30)
                        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                } else {
                    SeIconView(icon: .refresh, size: 13, stroke: 1.6, color: palette.inkSecondary)
                        .frame(width: 30, height: 30)
                        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .strokeBorder(mode.insetStroke, lineWidth: 1)
                        }
                        .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
            }
            .buttonStyle(SePressable())
            .accessibilityLabel(String(localized: "Refresh"))
            .disabled(isRefreshing)
        }
    }

    private var subtitle: String {
        let count = model.terminalCount
        guard count > 0 else { return hostMeta }
        let terminals = count == 1
            ? String(localized: "1 terminal")
            : String(localized: "\(count) terminals")
        return "\(hostMeta) · \(terminals)"
    }

    // MARK: - Hero

    private func heroCard(_ row: TerminalLandingModel.Row) -> some View {
        Button {
            onOpen(row)
        } label: {
            VStack(alignment: .leading, spacing: 8) {
                SeEyebrow(text: String(localized: "Continue"))
                HStack(spacing: 8) {
                    Circle()
                        .fill(palette.live)
                        .frame(width: 6, height: 6)
                    Text(verbatim: row.title)
                        .font(SeNocturneType.font(15, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    SeIconView(icon: .arrowRight, size: 13, stroke: 1.7, color: palette.inkSecondary)
                }
                Text(verbatim: meta(row))
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous)
                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
            }
            .contentShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(Text(row.title))
        .accessibilityHint(Text("Opens the running shell. Detaching leaves it running on the host."))
    }

    // MARK: - Groups

    private func groupHeader(_ group: TerminalLandingModel.Group) -> some View {
        SeGroupLabel(
            title: group.name,
            count: String(format: "%02d", group.count)
        )
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("\(group.name), \(group.count) terminals"))
    }

    private func terminalRow(_ row: TerminalLandingModel.Row) -> some View {
        Button {
            onOpen(row)
        } label: {
            HStack(spacing: 12) {
                SeIconView(icon: .terminal, size: 14, stroke: 1.6, color: palette.inkTertiary)
                    .frame(width: 22, height: 22)

                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: row.title)
                        .font(SeNocturneType.font(12, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(verbatim: meta(row))
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                if row.hasAgent {
                    agentBadge
                }
                SeIconView(icon: .arrowRight, size: 11, stroke: 1.7, color: palette.inkTertiary)
            }
            .padding(12)
            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(Text(row.title))
        .accessibilityValue(Text(row.hasAgent ? String(localized: "Agent running") : ""))
        .accessibilityHint(Text("Opens the running shell. Detaching leaves it running on the host."))
    }

    private func meta(_ row: TerminalLandingModel.Row) -> String {
        var parts: [String] = []
        if let branch = row.branch, !branch.isEmpty { parts.append(branch) }
        if let cwd = row.cwd, !cwd.isEmpty { parts.append(cwd) }
        return parts.isEmpty ? row.projectName ?? "" : parts.joined(separator: " · ")
    }

    private var agentBadge: some View {
        HStack(spacing: 4) {
            Circle().fill(palette.live).frame(width: 5, height: 5)
            Text("Agent")
                .font(SeNocturneType.font(9, weight: .medium))
                .foregroundStyle(palette.live)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(palette.liveSubtle, in: Capsule())
        .accessibilityLabel(Text("Agent running"))
    }

    // MARK: - Quick terminals

    @ViewBuilder
    private var quickSection: some View {
        SeGroupLabel(
            title: String(localized: "Quick terminals"),
            count: String(format: "%02d", model.quick.count)
        )
        VStack(spacing: NocturneToolMetrics.cardGap) {
            ForEach(model.quick) { row in
                quickRow(row)
            }
            Button(action: onNewQuickTerminal) {
                HStack(spacing: 7) {
                    SeIconView(icon: .plus, size: 11, stroke: 1.8, color: palette.inkTertiary)
                    Text("New quick terminal")
                        .font(SeNocturneType.font(10.5, weight: .medium))
                }
                .foregroundStyle(palette.inkTertiary)
                .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
                .background(.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(palette.stroke, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(SePressable())
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, model.quick.isEmpty ? 18 : 0)
    }

    private func quickRow(_ row: TerminalLandingModel.QuickRow) -> some View {
        HStack(spacing: 10) {
            SeIconView(icon: .bolt, size: 14, stroke: 1.6, color: palette.inkTertiary)
                .frame(width: 22, height: 22)

            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: row.title)
                    .font(SeNocturneType.font(11, weight: .regular, mono: true))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: quickMeta(row))
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Button {
                onOpenQuick(row)
            } label: {
                Text("Open")
                    .font(SeNocturneType.font(10, weight: .medium))
                    .foregroundStyle(row.canOpen ? palette.accent : palette.inkTertiary)
                    .padding(.horizontal, 10)
                    .frame(minHeight: SeSpace.hitTarget)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
                    .contentShape(Rectangle())
            }
            .buttonStyle(SePressable())
            .accessibilityLabel(String(localized: "Open \(row.title)"))
            .disabled(!row.canOpen)

            Menu {
                Button(String(localized: "Rename"), systemImage: "pencil") { onRenameQuick(row) }
                Button(String(localized: "Close shells"), systemImage: "xmark.circle") { onCloseQuick(row) }
                Button(String(localized: "Delete"), systemImage: "trash", role: .destructive) { onDeleteQuick(row) }
            } label: {
                SeIconView(icon: .more, size: 14, stroke: 1.7, color: palette.inkSecondary)
                    .frame(width: 38, height: 28)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(String(localized: "More actions"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func quickMeta(_ row: TerminalLandingModel.QuickRow) -> String {
        var parts = [row.targetLabel]
        if !row.cwd.isEmpty { parts.append(row.cwd) }
        parts.append(row.liveCount == 1
            ? String(localized: "1 live")
            : String(localized: "\(row.liveCount) live"))
        return parts.joined(separator: " · ")
    }

    // MARK: - Empty and new

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            SeIconView(icon: .terminal, size: 20, stroke: 1.6, color: palette.inkTertiary)
            Text("No running terminals")
                .font(SeNocturneType.font(12, weight: .semibold))
                .foregroundStyle(palette.ink)
            Text("Start one here, or open a shell on the host.")
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous)
                .strokeBorder(mode.hairline, lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
    }

    private var newTerminalButton: some View {
        Button(action: onNewTerminal) {
            HStack(spacing: 7) {
                SeIconView(icon: .plus, size: 12, stroke: 1.8, color: palette.inkTertiary)
                Text("New Terminal")
                    .font(SeNocturneType.font(10.5, weight: .medium))
            }
            .foregroundStyle(palette.inkTertiary)
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(palette.stroke, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .disabled(!model.canCreate)
        .opacity(model.canCreate ? 1 : 0.45)
        .accessibilityLabel(String(localized: "New Terminal"))
        .accessibilityHint(model.canCreate
            ? Text("Pick a project to start a shell in.")
            : Text("Add a project on the host first."))
    }
}
