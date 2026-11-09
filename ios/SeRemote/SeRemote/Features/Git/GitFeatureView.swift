import SwiftUI

/// Read-only Git: branch, status and diff for the workspace root the host
/// actually reported. It never commits, stages or rolls back.
struct GitFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession

    @State private var tab: GitTab = .status
    @State private var busy = false

    private var git: GitStore { session.git }

    var body: some View {
        NocturneToolScreen(
            title: String(localized: "Changes"),
            subtitle: navSubtitle,
            onRefresh: refresh
        ) {
            content
                .task { await refreshIfNeeded() }
        }
    }

    // MARK: Scope

    private var root: String? { session.workspaceRoot }

    /// The store owns one snapshot. Branch and counts are shown only when that
    /// snapshot was loaded for this workspace root, never for another project.
    private var scoped: Bool {
        guard let root, let cwd = git.cwd else { return false }
        return samePath(cwd, root)
    }

    private var usable: Bool {
        root != nil && (git.cwd == nil || scoped)
    }

    private var repoName: String {
        guard let root, !root.isEmpty else { return String(localized: "Changes") }
        return (root as NSString).lastPathComponent
    }

    private var hostName: String {
        session.origin.host() ?? String(localized: "Host")
    }

    private var navSubtitle: String {
        var parts = [hostName]
        if !repoName.isEmpty, repoName != String(localized: "Changes") { parts.append(repoName) }
        if scoped, let branch = git.context?.branch, !branch.isEmpty { parts.append(branch) }
        return parts.joined(separator: " · ")
    }

    private var headBranch: String? {
        guard scoped, let branch = git.context?.branch, !branch.isEmpty else { return nil }
        return branch
    }

    private var aheadBehind: String? {
        guard scoped, let context = git.context, context.hasUpstream else { return nil }
        var parts: [String] = []
        if context.ahead > 0 { parts.append(String(localized: "\(context.ahead) ahead")) }
        if context.behind > 0 { parts.append(String(localized: "\(context.behind) behind")) }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        hostHead

        if root == nil {
            NocturneEmptyState(
                systemImage: "folder.badge.questionmark",
                title: String(localized: "No repository scope"),
                message: String(localized: "Open a project or conversation first.")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
        } else if !usable {
            NocturneNotice(
                message: String(localized: "Git is showing a different project. Open this workspace again to reload its status."),
                isError: false
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
        } else {
            NocturneSegmented(
                segments: [
                    NocturneSegment(value: GitTab.status, title: String(localized: "Status"), count: scoped ? git.status.count : nil),
                    NocturneSegment(value: GitTab.diff, title: String(localized: "Diff"), count: scoped && git.diffPath != nil ? git.diff.files.count : nil)
                ],
                selection: $tab
            )

            switch tab {
            case .status: statusContent
            case .diff: diffContent
            }
        }
    }

    private var hostHead: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(verbatim: repoName)
                .font(SeNocturneType.font(23, weight: .semibold))
                .tracking(-0.9)
                .foregroundStyle(palette.ink)
                .lineLimit(1)
                .padding(.top, 8)
            if let headBranch {
                HStack(spacing: 8) {
                    Circle()
                        .fill(palette.live)
                        .frame(width: 6, height: 6)
                    Text(verbatim: [headBranch, aheadBehind].compactMap { $0 }.joined(separator: " · "))
                        .font(SeNocturneType.font(11, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                .padding(.top, 8)
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 12)
    }

    // MARK: Status

    @ViewBuilder
    private var statusContent: some View {
        if case let .failed(message) = git.state {
            NocturneNotice(message: message, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 16)
        } else if git.state == .loading && git.status.isEmpty {
            loadingRow
        } else if git.status.isEmpty {
            NocturneEmptyState(
                systemImage: "checkmark.circle",
                title: String(localized: "Clean working tree")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 16)
        } else {
            NocturneGroupLabel(
                title: String(localized: "Uncommitted"),
                detail: String(format: "%02d", git.status.count)
            )
            VStack(spacing: NocturneToolMetrics.cardGap) {
                ForEach(git.status, id: \.self) { change in
                    changeRow(change)
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
        }

        NocturneGroupLabel(title: String(localized: "Diff preview"), detail: String(localized: "Read only"))
        Group {
            if let path = git.diffPath {
                GitDiffView(diff: git.diff, rawText: git.rawDiff, path: path)
            } else {
                NocturneNotice(
                    message: String(localized: "Select a changed file to see its diff."),
                    isError: false
                )
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 18)
    }

    private func changeRow(_ change: GitStatusDetailWire) -> some View {
        let presentation = GitChangePresentation.status(change.status, staged: change.staged)
        let marker = marker(presentation)
        return NocturneSessionCard(
            title: change.path,
            subtitle: [presentation.title,
                       change.staged ? String(localized: "staged") : String(localized: "unstaged")]
                .joined(separator: " · "),
            spine: spine(presentation),
            alert: presentation.title == "Conflict",
            trailingText: marker.symbol,
            trailingTint: marker.color,
            action: {
                tab = .diff
                Task { await git.loadDiff(path: change.path, staged: change.staged) }
            }
        )
    }

    private func spine(_ presentation: GitChangePresentation) -> NocturneSpine {
        switch presentation.title {
        case "Added": .live
        case "Modified", "Conflict": .wait
        default: .none
        }
    }

    private func marker(_ presentation: GitChangePresentation) -> (symbol: String, color: Color) {
        switch presentation.title {
        case "Added": ("A", palette.live)
        case "Modified": ("M", palette.warning)
        case "Deleted": ("D", palette.danger)
        case "Renamed": ("R", palette.accent)
        case "Copied": ("C", palette.accent)
        case "Untracked": ("?", palette.inkTertiary)
        case "Conflict": ("!", palette.danger)
        default: ("·", palette.inkTertiary)
        }
    }

    // MARK: Diff

    @ViewBuilder
    private var diffContent: some View {
        if case let .failed(message) = git.diffState {
            NocturneNotice(message: message, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 16)
        } else if git.diffPath == nil {
            NocturneEmptyState(
                systemImage: "doc.text.magnifyingglass",
                title: String(localized: "No diff loaded")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 16)
        } else {
            if git.diffState == .loading {
                loadingRow
            }
            GitDiffView(diff: git.diff, rawText: git.rawDiff, path: git.diffPath ?? "")
                .padding(.horizontal, 14)
                .padding(.top, 16)
        }
    }

    private var loadingRow: some View {
        HStack(spacing: 8) {
            ProgressView()
            Text("Loading from the host…")
                .font(SeNocturneType.font(11, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 16)
        .frame(minHeight: NocturneToolMetrics.minimumTarget)
    }

    // MARK: Actions

    private func refreshIfNeeded() async {
        guard let root else { return }
        if git.cwd == nil || !samePath(git.cwd ?? "", root) {
            await git.refresh(cwd: root)
        }
    }

    private func refresh() async {
        guard let root, !busy else { return }
        busy = true
        defer { busy = false }
        await git.refresh(cwd: root)
    }

    private func samePath(_ lhs: String, _ rhs: String) -> Bool {
        URL(fileURLWithPath: lhs).standardizedFileURL.path == URL(fileURLWithPath: rhs).standardizedFileURL.path
    }
}

enum GitTab: Hashable {
    case status
    case diff
}
