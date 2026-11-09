import SwiftUI

/// Read-only repository inspector, presented inside the workspace's navigation.
struct GitFeatureView: View {
    @Environment(\.sePalette) private var p
    let session: WorkspaceSession
    @State private var section: Section = .changes
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeNocturneType.bodySize

    private enum Section: String, CaseIterable, Identifiable {
        case changes = "Changes", history = "History", references = "Refs"
        var id: Self { self }
    }

    private var store: GitStore { session.git }
    private var loading: Bool { store.state == .loading }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 24) {
                if let root = session.workspaceRoot {
                    repositoryHeader(root: root)
                    sectionPicker
                    switch store.state {
                    case .idle, .loading:
                        loadingView
                    case .failed(let message):
                        ToolNocturneErrorBanner(message: message) { Task { await refresh() } }
                    case .loaded, .empty:
                        switch section {
                        case .changes: changes
                        case .history: history
                        case .references: references
                        }
                    }
                } else {
                    ToolNocturneEmptyState(title: "No workspace selected", systemImage: "folder",
                                 message: "Open a conversation or project to inspect its repository.")
                }
            }
            .padding(20)
        }
        .background(p.canvas)
        .tint(p.accent)
        .toolNocturneFont(.body)
        .foregroundStyle(p.ink)
        .navigationTitle("Git")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    Task { await refresh() }
                } label: {
                    Label("Refresh Git", systemImage: "arrow.clockwise")
                        .frame(minWidth: 44, minHeight: 44)
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
        ToolNocturneCard {
            VStack(alignment: .leading, spacing: 12) {
                Label(store.context?.branch ?? (store.context?.hasHead == false ? "No commits yet" : "Repository"),
                      systemImage: "arrow.triangle.branch")
                    .toolNocturneFont(.rowTitle)
                Text(root)
                    .font(SeNocturneType.font(codeSize, mono: true))
                    .foregroundStyle(p.inkSecondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if let context = store.context {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 16) { repositoryCounts(context) }
                        VStack(alignment: .leading, spacing: 8) { repositoryCounts(context) }
                    }
                    if !context.hasUpstream {
                        Text("No upstream configured").toolNocturneFont(.caption).foregroundStyle(p.inkSecondary)
                    }
                }
                Label("Read only", systemImage: "lock")
                    .toolNocturneFont(.caption)
                    .foregroundStyle(p.inkSecondary)
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
            HStack(spacing: 8) {
                ForEach(Section.allCases) { item in
                    Button {
                        section = item
                    } label: {
                        Text(item.rawValue)
                            .toolNocturneFont(.label)
                            .padding(.horizontal, 16)
                            .frame(minWidth: 44, minHeight: 44)
                            .foregroundStyle(section == item ? p.canvas : p.inkSecondary)
                            .background(section == item ? p.ink : p.card, in: Capsule())
                            .overlay { Capsule().strokeBorder(section == item ? Color.clear : p.stroke) }
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(section == item ? .isSelected : [])
                }
            }
        }
        .accessibilityLabel("Git sections")
    }

    private var loadingView: some View {
        HStack(spacing: 12) {
            if reduceMotion { Image(systemName: "hourglass") } else { ProgressView() }
            Text("Refreshing repository…").toolNocturneFont(.support)
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private var changes: some View {
        VStack(alignment: .leading, spacing: 24) {
            if store.status.isEmpty {
                ToolNocturneEmptyState(title: "Working tree clean", systemImage: "checkmark.circle",
                             message: "There are no staged or unstaged changes.")
            } else {
                changeGroup(staged: true)
                changeGroup(staged: false)
            }
        }
    }

    private func changeGroup(staged: Bool) -> some View {
        let files = store.status.filter { $0.staged == staged }
        return VStack(alignment: .leading, spacing: 8) {
            ToolNocturneSectionHeader(title: staged ? "Staged" : "Unstaged", detail: "\(files.count) files")
            if files.isEmpty {
                Text(staged ? "No staged changes" : "No unstaged changes")
                    .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                    .padding(.vertical, 12)
            }
            ForEach(Array(files.enumerated()), id: \.offset) { _, file in
                NavigationLink(value: GitDiffSelection(cwd: session.workspaceRoot ?? "", path: file.path, staged: staged)) {
                    changeRow(file)
                }
                .buttonStyle(ToolNocturneRowStyle())
                Divider().overlay(p.stroke)
            }
        }
    }

    private func changeRow(_ file: GitStatusDetailWire) -> some View {
        let presentation = GitChangePresentation.status(file.status, staged: file.staged)
        return HStack(spacing: 12) {
            Image(systemName: presentation.symbol).foregroundStyle(p.inkSecondary)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(file.path).font(SeNocturneType.font(codeSize, mono: true))
                    .fixedSize(horizontal: false, vertical: true)
                Text(presentation.title).toolNocturneFont(.caption).foregroundStyle(p.inkSecondary)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right").toolNocturneFont(.caption).foregroundStyle(p.inkTertiary)
        }
        .frame(minHeight: 44)
        .padding(.vertical, 8)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint("View read-only diff")
    }

    private var history: some View {
        VStack(alignment: .leading, spacing: 16) {
            ToolNocturneSectionHeader(title: "Recent history", detail: "\(store.commits.count) commits")
            if store.commits.isEmpty {
                ToolNocturneEmptyState(title: "No commits yet", systemImage: "clock", message: "History will appear after the first commit.")
            }
            ForEach(Array(store.commits.enumerated()), id: \.offset) { _, commit in
                VStack(alignment: .leading, spacing: 8) {
                    Text(commit.subject).toolNocturneFont(.rowTitle).textSelection(.enabled)
                    Text(commit.shortHash).font(SeNocturneType.font(codeSize, mono: true)).foregroundStyle(p.accent)
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 8) { commitAuthorAndDate(commit) }
                        VStack(alignment: .leading, spacing: 4) { commitAuthorAndDate(commit) }
                    }
                    if !commit.refs.isEmpty {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(Array(commit.refs.enumerated()), id: \.offset) { _, ref in
                                    refChip(ref, symbol: "tag")
                                }
                            }
                        }
                    }
                }
                .padding(.vertical, 8)
                Divider().overlay(p.stroke)
            }
        }
    }

    @ViewBuilder private func commitAuthorAndDate(_ commit: GitCommitWire) -> some View {
        Text(commit.author).toolNocturneFont(.support)
        if let date = GitDiffPresentation.date(commit.date) {
            Text(date, style: .relative).toolNocturneFont(.timing).foregroundStyle(p.inkSecondary)
        } else {
            Text(commit.date).toolNocturneFont(.timing).foregroundStyle(p.inkSecondary)
        }
    }

    private var references: some View {
        VStack(alignment: .leading, spacing: 24) {
            VStack(alignment: .leading, spacing: 12) {
                ToolNocturneSectionHeader(title: "Branches", detail: "\(store.branches.count)")
                if store.branches.isEmpty {
                    Text("No branches").toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                }
                ForEach(Array(store.branches.enumerated()), id: \.offset) { _, branch in
                    VStack(alignment: .leading, spacing: 8) {
                        Label(branch.name, systemImage: branch.isRemote ? "network" : "arrow.triangle.branch")
                            .font(SeNocturneType.font(codeSize, mono: true))
                        if branch.isCurrent { refChip("Current branch", symbol: "checkmark") }
                        if branch.isCheckedOutElsewhere { refChip("Checked out in another worktree", symbol: "folder") }
                        if let target = branch.symbolicTarget {
                            Label(target, systemImage: "arrow.right").toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                        }
                    }
                    .padding(.vertical, 8)
                    Divider().overlay(p.stroke)
                }
            }
            VStack(alignment: .leading, spacing: 12) {
                ToolNocturneSectionHeader(title: "Stashes", detail: "\(store.stashes.count)")
                if store.stashes.isEmpty {
                    Text("No stashes").toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                }
                ForEach(Array(store.stashes.enumerated()), id: \.offset) { _, stash in
                    VStack(alignment: .leading, spacing: 4) {
                        Label(stash.name, systemImage: "archivebox").font(SeNocturneType.font(codeSize, mono: true))
                        Text(stash.message).toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                    }
                    .padding(.vertical, 8)
                    Divider().overlay(p.stroke)
                }
            }
        }
        .textSelection(.enabled)
    }

    private func refChip(_ title: String, symbol: String) -> some View {
        Label(title, systemImage: symbol)
            .toolNocturneFont(.caption)
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(p.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
    }
}

// Presentation primitives shared only by the workspace tool views. Kept here so
// the view-only migration does not change stores, formatters or shared controls.
enum ToolNocturneTextStyle {
    case body, support, caption, label, timing, rowTitle, sectionTitle, code

    var size: CGFloat {
        switch self {
        case .body: SeNocturneType.bodySize
        case .support: SeNocturneType.secondarySize
        case .caption: SeNocturneType.labelSize
        case .label: SeNocturneType.eyebrowSize
        case .timing: SeNocturneType.monoSize
        case .rowTitle: SeNocturneType.rowTitleSize
        case .sectionTitle: SeNocturneType.sectionTitleSize
        case .code: SeNocturneType.bodySize
        }
    }
    var weight: Font.Weight {
        switch self {
        case .rowTitle, .sectionTitle: .semibold
        case .label: .medium
        default: .regular
        }
    }
    var mono: Bool { self == .timing || self == .label || self == .code }
}

private struct ToolNocturneTypography: ViewModifier {
    let style: ToolNocturneTextStyle
    @ScaledMetric(relativeTo: .body) private var scale: CGFloat = 1

    func body(content: Content) -> some View {
        content.font(SeNocturneType.font(style.size * scale, weight: style.weight, mono: style.mono))
    }
}

extension View {
    func toolNocturneFont(_ style: ToolNocturneTextStyle) -> some View {
        modifier(ToolNocturneTypography(style: style))
    }
}

struct ToolNocturneCard<Content: View>: View {
    @Environment(\.sePalette) private var p
    @ViewBuilder var content: Content

    var body: some View {
        content
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
            .foregroundStyle(p.ink)
            .background(p.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.card).strokeBorder(p.stroke) }
    }
}

struct ToolNocturneSectionHeader: View {
    @Environment(\.sePalette) private var p
    let title: String
    var detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(title).toolNocturneFont(.label)
            Spacer(minLength: 8)
            if let detail { Text(detail).toolNocturneFont(.timing) }
        }
        .foregroundStyle(p.inkSecondary)
        .accessibilityAddTraits(.isHeader)
    }
}

struct ToolNocturneEmptyState: View {
    @Environment(\.sePalette) private var p
    let title: String
    let systemImage: String
    var message: String?
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        ContentUnavailableView {
            Label {
                Text(title).toolNocturneFont(.sectionTitle).foregroundStyle(p.ink)
            } icon: {
                Image(systemName: systemImage).foregroundStyle(p.inkTertiary)
            }
        } description: {
            if let message { Text(message).toolNocturneFont(.support).foregroundStyle(p.inkSecondary) }
        } actions: {
            if let actionTitle, let action {
                Button(actionTitle, action: action).buttonStyle(ToolNocturneButtonStyle(primary: true))
            }
        }
    }
}

struct ToolNocturneErrorBanner: View {
    @Environment(\.sePalette) private var p
    let message: String
    var retryTitle: String = String(localized: "Retry")
    var retry: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                Text(message).toolNocturneFont(.support).foregroundStyle(p.ink).textSelection(.enabled)
            } icon: {
                Image(systemName: "exclamationmark.triangle").foregroundStyle(p.danger)
            }
            if let retry {
                Button(retryTitle, action: retry).buttonStyle(ToolNocturneButtonStyle())
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(p.danger.opacity(0.08), in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.danger.opacity(0.3)) }
        .accessibilityElement(children: .contain)
    }
}

struct ToolNocturneRowStyle: ButtonStyle {
    @Environment(\.sePalette) private var p
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .padding(.horizontal, 16)
            .frame(minWidth: 44, minHeight: 44)
            .background(p.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.card).strokeBorder(p.stroke) }
            .contentShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .opacity(configuration.isPressed ? 0.75 : 1)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

struct ToolNocturneButtonStyle: ButtonStyle {
    var primary = false
    @Environment(\.sePalette) private var p
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .toolNocturneFont(.body)
            .fontWeight(.medium)
            .foregroundStyle(primary ? p.canvas : p.ink)
            .padding(.horizontal, 16)
            .frame(minWidth: 44, minHeight: 44)
            .background(primary ? p.ink : p.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(primary ? Color.clear : p.strokeStrong) }
            .contentShape(RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            .opacity(isEnabled ? (configuration.isPressed ? 0.75 : 1) : 0.45)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

struct ToolNocturneStatusBadge: View {
    @Environment(\.sePalette) private var p
    let status: SeStatus
    var title: String?

    private var color: Color {
        switch status {
        case .online, .working, .completed: p.live
        case .connecting, .reconnecting, .needsApproval: p.warning
        case .failed: p.danger
        case .offline, .idle, .paused: p.inkSecondary
        }
    }

    var body: some View {
        Label(title ?? status.title, systemImage: status.symbol)
            .toolNocturneFont(.label)
            .foregroundStyle(color)
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(color.opacity(0.10), in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            .accessibilityLabel(title ?? status.title)
    }
}

struct ToolNocturneCodeBlock: View {
    @Environment(\.sePalette) private var p
    let text: String
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeNocturneType.bodySize

    var body: some View {
        ScrollView(.horizontal) {
            Text(text)
                .font(SeNocturneType.font(codeSize, mono: true))
                .foregroundStyle(p.ink)
                .textSelection(.enabled)
                .padding(12)
                .fixedSize(horizontal: true, vertical: false)
        }
        .background(p.codeCanvas, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.stroke) }
    }
}

struct ToolNocturneProgress: View {
    @Environment(\.sePalette) private var p
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let title: String

    init(_ title: String) { self.title = title }

    var body: some View {
        Group {
            if reduceMotion {
                Label(title, systemImage: "hourglass")
            } else {
                ProgressView { Text(title) }
            }
        }
        .toolNocturneFont(.support)
        .foregroundStyle(p.inkSecondary)
        .accessibilityElement(children: .combine)
    }
}
