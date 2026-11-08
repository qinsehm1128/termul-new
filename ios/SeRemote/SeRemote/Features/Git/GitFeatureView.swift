import SwiftUI

/// Read-only repository inspector, presented inside the workspace's navigation.
struct GitFeatureView: View {
    let session: WorkspaceSession
    @State private var section: Section = .changes
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeFont.codeBaseSize

    private enum Section: String, CaseIterable, Identifiable {
        case changes = "Changes", history = "History", references = "Refs"
        var id: Self { self }
    }

    private var store: GitStore { session.git }
    private var loading: Bool { store.state == .loading }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: SeSpace.section) {
                if let root = session.workspaceRoot {
                    repositoryHeader(root: root)
                    sectionPicker
                    switch store.state {
                    case .idle, .loading:
                        loadingView
                    case .failed(let message):
                        SeErrorBanner(message: message) { Task { await refresh() } }
                    case .loaded, .empty:
                        switch section {
                        case .changes: changes
                        case .history: history
                        case .references: references
                        }
                    }
                } else {
                    SeEmptyState(title: "No workspace selected", systemImage: "folder",
                                 message: "Open a conversation or project to inspect its repository.")
                }
            }
            .padding(SeSpace.screenInset)
        }
        .background(SeTheme.canvas)
        .foregroundStyle(SeTheme.ink)
        .navigationTitle("Git")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    Task { await refresh() }
                } label: {
                    Label("Refresh Git", systemImage: "arrow.clockwise")
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                }
                .disabled(loading || session.workspaceRoot == nil)
            }
        }
        .refreshable { await refresh() }
        .task(id: session.workspaceRoot) { await refresh() }
        .navigationDestination(for: GitDiffSelection.self) { selection in
            GitDiffView(session: session, selection: selection)
        }
    }

    private func refresh() async {
        guard let root = session.workspaceRoot else { return }
        await store.refresh(cwd: root)
    }

    private func repositoryHeader(root: String) -> some View {
        SeCard {
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                Label(store.context?.branch ?? (store.context?.hasHead == false ? "No commits yet" : "Repository"),
                      systemImage: "arrow.triangle.branch")
                    .font(SeFont.rowTitle)
                Text(root)
                    .font(SeFont.code(size: codeSize))
                    .foregroundStyle(SeTheme.secondaryInk)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if let context = store.context {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: SeSpace.md) { repositoryCounts(context) }
                        VStack(alignment: .leading, spacing: SeSpace.xs) { repositoryCounts(context) }
                    }
                    if !context.hasUpstream {
                        Text("No upstream configured").font(SeFont.caption).foregroundStyle(SeTheme.secondaryInk)
                    }
                }
                Label("Read only", systemImage: "lock")
                    .font(SeFont.caption)
                    .foregroundStyle(SeTheme.secondaryInk)
            }
        }
    }

    @ViewBuilder private func repositoryCounts(_ context: GitCommitContextWire) -> some View {
        Label("\(context.ahead) ahead", systemImage: "arrow.up")
        Label("\(context.behind) behind", systemImage: "arrow.down")
        Label("\(context.stagedCount) staged", systemImage: "tray.full")
    }

    private var sectionPicker: some View {
        // Scrolling text tabs keep a 44pt hit target even at accessibility sizes.
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: SeSpace.xs) {
                ForEach(Section.allCases) { item in
                    Button {
                        section = item
                    } label: {
                        Text(item.rawValue)
                            .font(SeFont.label)
                            .padding(.horizontal, SeSpace.md)
                            .frame(minHeight: SeSpace.hitTarget)
                            .background(section == item ? SeTheme.accentSubtle : SeTheme.surface,
                                        in: Capsule())
                            .overlay { Capsule().strokeBorder(section == item ? SeTheme.accent : SeTheme.stroke) }
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(section == item ? .isSelected : [])
                }
            }
        }
        .accessibilityLabel("Git sections")
    }

    private var loadingView: some View {
        HStack(spacing: SeSpace.sm) {
            if reduceMotion { Image(systemName: "hourglass") } else { ProgressView() }
            Text("Refreshing repository…").font(SeFont.support)
        }
        .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private var changes: some View {
        VStack(alignment: .leading, spacing: SeSpace.section) {
            if store.status.isEmpty {
                SeEmptyState(title: "Working tree clean", systemImage: "checkmark.circle",
                             message: "There are no staged or unstaged changes.")
            } else {
                changeGroup(staged: true)
                changeGroup(staged: false)
            }
        }
    }

    private func changeGroup(staged: Bool) -> some View {
        let files = store.status.filter { $0.staged == staged }
        return VStack(alignment: .leading, spacing: SeSpace.xs) {
            SeSectionHeader(title: staged ? "Staged" : "Unstaged", detail: "\(files.count) files")
            if files.isEmpty {
                Text(staged ? "No staged changes" : "No unstaged changes")
                    .font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                    .padding(.vertical, SeSpace.sm)
            }
            ForEach(Array(files.enumerated()), id: \.offset) { _, file in
                NavigationLink(value: GitDiffSelection(cwd: session.workspaceRoot ?? "", path: file.path, staged: staged)) {
                    changeRow(file)
                }
                .buttonStyle(SePressableRowStyle())
                Divider().overlay(SeTheme.stroke)
            }
        }
    }

    private func changeRow(_ file: GitStatusDetailWire) -> some View {
        let presentation = GitChangePresentation.status(file.status, staged: file.staged)
        return HStack(spacing: SeSpace.sm) {
            Image(systemName: presentation.symbol).foregroundStyle(SeTheme.secondaryInk)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: SeSpace.xxs) {
                Text(file.path).font(SeFont.code(size: codeSize))
                    .fixedSize(horizontal: false, vertical: true)
                Text(presentation.title).font(SeFont.caption).foregroundStyle(SeTheme.secondaryInk)
            }
            Spacer(minLength: SeSpace.xs)
            Image(systemName: "chevron.right").font(SeFont.caption).foregroundStyle(SeTheme.tertiaryInk)
        }
        .frame(minHeight: SeSpace.hitTarget)
        .padding(.vertical, SeSpace.xs)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint("View read-only diff")
    }

    private var history: some View {
        VStack(alignment: .leading, spacing: SeSpace.md) {
            SeSectionHeader(title: "Recent history", detail: "\(store.commits.count) commits")
            if store.commits.isEmpty {
                SeEmptyState(title: "No commits yet", systemImage: "clock", message: "History will appear after the first commit.")
            }
            ForEach(Array(store.commits.enumerated()), id: \.offset) { _, commit in
                VStack(alignment: .leading, spacing: SeSpace.xs) {
                    Text(commit.subject).font(SeFont.rowTitle).textSelection(.enabled)
                    Text(commit.shortHash).font(SeFont.code(size: codeSize)).foregroundStyle(SeTheme.accent)
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: SeSpace.xs) { commitAuthorAndDate(commit) }
                        VStack(alignment: .leading, spacing: SeSpace.xxs) { commitAuthorAndDate(commit) }
                    }
                    if !commit.refs.isEmpty {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: SeSpace.xs) {
                                ForEach(Array(commit.refs.enumerated()), id: \.offset) { _, ref in
                                    refChip(ref, symbol: "tag")
                                }
                            }
                        }
                    }
                }
                .padding(.vertical, SeSpace.xs)
                Divider().overlay(SeTheme.stroke)
            }
        }
    }

    @ViewBuilder private func commitAuthorAndDate(_ commit: GitCommitWire) -> some View {
        Text(commit.author).font(SeFont.support)
        if let date = GitDiffPresentation.date(commit.date) {
            Text(date, style: .relative).font(SeFont.timing).foregroundStyle(SeTheme.secondaryInk)
        } else {
            Text(commit.date).font(SeFont.timing).foregroundStyle(SeTheme.secondaryInk)
        }
    }

    private var references: some View {
        VStack(alignment: .leading, spacing: SeSpace.section) {
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                SeSectionHeader(title: "Branches", detail: "\(store.branches.count)")
                if store.branches.isEmpty {
                    Text("No branches").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                }
                ForEach(Array(store.branches.enumerated()), id: \.offset) { _, branch in
                    VStack(alignment: .leading, spacing: SeSpace.xs) {
                        Label(branch.name, systemImage: branch.isRemote ? "network" : "arrow.triangle.branch")
                            .font(SeFont.code(size: codeSize))
                        if branch.isCurrent { refChip("Current branch", symbol: "checkmark") }
                        if branch.isCheckedOutElsewhere { refChip("Checked out in another worktree", symbol: "folder") }
                        if let target = branch.symbolicTarget {
                            Label(target, systemImage: "arrow.right").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                        }
                    }
                    .padding(.vertical, SeSpace.xs)
                    Divider().overlay(SeTheme.stroke)
                }
            }
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                SeSectionHeader(title: "Stashes", detail: "\(store.stashes.count)")
                if store.stashes.isEmpty {
                    Text("No stashes").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                }
                ForEach(Array(store.stashes.enumerated()), id: \.offset) { _, stash in
                    VStack(alignment: .leading, spacing: SeSpace.xxs) {
                        Label(stash.name, systemImage: "archivebox").font(SeFont.code(size: codeSize))
                        Text(stash.message).font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                    }
                    .padding(.vertical, SeSpace.xs)
                    Divider().overlay(SeTheme.stroke)
                }
            }
        }
        .textSelection(.enabled)
    }

    private func refChip(_ title: String, symbol: String) -> some View {
        Label(title, systemImage: symbol)
            .font(SeFont.caption)
            .padding(.horizontal, SeSpace.xs).padding(.vertical, SeSpace.xxs)
            .background(SeTheme.elevated, in: RoundedRectangle(cornerRadius: SeRadius.tag))
    }
}
