import SwiftUI

// MARK: - Model

struct WorkspaceTool: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let detail: String
    let note: String
    let systemImage: String
}

struct WorkspaceProject: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let branch: String
    let detail: String
    let hasChanges: Bool
}

enum WorkspaceSection: String, CaseIterable, Identifiable {
    case tools
    case projects

    var id: String { rawValue }

    var title: LocalizedStringKey {
        switch self {
        case .tools: "Tools"
        case .projects: "Projects"
        }
    }
}

// MARK: - Screen

/// The second destination: everything that is not a live session. Tools are
/// entry points, not separate top-level tabs.
struct WorkspaceHubView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var host: String
    var latency: String
    var project: String
    var branch: String
    var tools: [WorkspaceTool]
    var projects: [WorkspaceProject]

    var onOpenTool: (WorkspaceTool) -> Void = { _ in }
    var onOpenProject: (WorkspaceProject) -> Void = { _ in }
    var onSelectHost: () -> Void = {}
    var onOpenLive: () -> Void = {}
    var onOpenCommand: () -> Void = {}
    var onOpenDrawer: () -> Void = {}
    var onToggleTheme: () -> Void = {}

    @State private var section: WorkspaceSection = .tools

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.top, SeSpace.xs)
                title
                    .padding(.top, SeSpace.lg)
                sectionSwitch
                    .padding(.top, SeSpace.md)
                switch section {
                case .tools:
                    toolGrid
                        .padding(.top, SeSpace.lg)
                case .projects:
                    projectList
                        .padding(.top, SeSpace.md)
                }
            }
            .padding(.horizontal, SeSpace.lg)
            .padding(.bottom, SeSpace.xxl)
        }
        .scrollIndicators(.hidden)
        .background(palette.canvas.ignoresSafeArea())
        .safeAreaInset(edge: .bottom, spacing: 0) {
            bottomBar
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button(action: onSelectHost) {
                HStack(spacing: 6) {
                    Text(verbatim: host)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(palette.inkTertiary)
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
            }
            .buttonStyle(.plain)

            Text(verbatim: latency)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)

            Spacer(minLength: 8)

            iconButton(
                systemImage: mode == .night ? "sun.max" : "moon",
                label: String(localized: "Switch theme"),
                action: onToggleTheme
            )
            iconButton(
                systemImage: "sidebar.left",
                label: String(localized: "All destinations"),
                action: onOpenDrawer
            )
        }
    }

    private var title: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Workspace")
                .font(SeNocturneType.screenTitle)
                .foregroundStyle(palette.ink)
            Text(verbatim: "\(project) · \(branch)")
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkTertiary)
        }
    }

    private var sectionSwitch: some View {
        HStack(spacing: 3) {
            ForEach(WorkspaceSection.allCases) { item in
                let isOn = section == item
                Button {
                    withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion)) {
                        section = item
                    }
                } label: {
                    Text(item.title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: isOn ? .medium : .regular))
                        .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                        .frame(maxWidth: .infinity)
                        .frame(height: 30)
                        .background(
                            isOn ? palette.ink : .clear,
                            in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var toolGrid: some View {
        LazyVGrid(
            columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)],
            spacing: 8
        ) {
            ForEach(tools) { tool in
                Button {
                    onOpenTool(tool)
                } label: {
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(spacing: 6) {
                            Image(systemName: tool.systemImage)
                                .font(.system(size: 12, weight: .regular))
                                .foregroundStyle(palette.inkTertiary)
                            Text(verbatim: tool.title)
                                .font(SeNocturneType.monoSmall)
                                .foregroundStyle(palette.inkTertiary)
                            Spacer(minLength: 0)
                        }
                        Text(verbatim: tool.detail)
                            .font(SeNocturneType.font(SeNocturneType.sectionTitleSize, weight: .semibold))
                            .foregroundStyle(palette.ink)
                            .lineLimit(1)
                            .padding(.top, 18)
                        Text(verbatim: tool.note)
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                            .padding(.top, 3)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 14))
                    .overlay {
                        RoundedRectangle(cornerRadius: 14)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var projectList: some View {
        VStack(spacing: 6) {
            ForEach(projects) { project in
                Button {
                    onOpenProject(project)
                } label: {
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 2)
                            .fill(project.hasChanges ? palette.live : palette.inkTertiary)
                            .frame(width: 3, height: 24)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: project.name)
                                .font(SeNocturneType.rowTitle)
                                .foregroundStyle(palette.ink)
                            Text(verbatim: "\(project.branch) · \(project.detail)")
                                .font(SeNocturneType.monoSmall)
                                .foregroundStyle(palette.inkTertiary)
                        }
                        Spacer(minLength: 8)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(palette.inkTertiary)
                    }
                    .padding(12)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var bottomBar: some View {
        HStack(spacing: 6) {
            tabItem(title: "Live", systemImage: "dot.radiowaves.left.and.right", isOn: false, action: onOpenLive)
            tabItem(title: "Workspace", systemImage: "folder", isOn: true, action: {})
            Button(action: onOpenCommand) {
                Image(systemName: "command")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(palette.accent)
                    .frame(width: 52, height: 44)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, SeSpace.md)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(palette.surface)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
        }
    }

    private func tabItem(
        title: LocalizedStringKey,
        systemImage: String,
        isOn: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: systemImage)
                    .font(.system(size: 17, weight: .regular))
                Text(title)
                    .font(SeNocturneType.font(SeNocturneType.monoSmallSize))
            }
            .foregroundStyle(isOn ? palette.ink : palette.inkTertiary)
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(
                isOn ? palette.card.opacity(0.6) : .clear,
                in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
            )
        }
        .buttonStyle(.plain)
    }

    private func iconButton(systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Previews

#Preview("Workspace · Night") {
    WorkspaceHubView(
        host: "MacBook Pro",
        latency: "12 ms · 2 connections",
        project: "Termul",
        branch: "main · 2 changes",
        tools: [
            WorkspaceTool(id: "files", title: "Files", detail: "482", note: "Read only", systemImage: "folder"),
            WorkspaceTool(id: "git", title: "Changes", detail: "2 files", note: "+12 −4", systemImage: "arrow.triangle.branch"),
            WorkspaceTool(id: "tasks", title: "Schedules", detail: "4 tasks", note: "1 running", systemImage: "clock"),
            WorkspaceTool(id: "quick", title: "Quick", detail: "4 commands", note: "Latest 2m", systemImage: "apple.terminal"),
            WorkspaceTool(id: "search", title: "Search", detail: "Sessions & files", note: "Or press ⌘", systemImage: "magnifyingglass"),
            WorkspaceTool(id: "pair", title: "Pair", detail: "New host", note: "QR or link", systemImage: "plus")
        ],
        projects: [
            WorkspaceProject(id: "p1", name: "Termul", branch: "main", detail: "2 changes · just now", hasChanges: true),
            WorkspaceProject(id: "p2", name: "se-server", branch: "dev", detail: "clean · yesterday", hasChanges: false)
        ]
    )
    .seThemeMode(.night)
}

#Preview("Workspace · Day") {
    WorkspaceHubView(
        host: "MacBook Pro",
        latency: "12 ms",
        project: "Termul",
        branch: "main",
        tools: [
            WorkspaceTool(id: "files", title: "Files", detail: "482", note: "Read only", systemImage: "folder")
        ],
        projects: []
    )
    .seThemeMode(.day)
}
