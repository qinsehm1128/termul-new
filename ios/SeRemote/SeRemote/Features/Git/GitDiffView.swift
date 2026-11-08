import SwiftUI

struct GitDiffView: View {
    let session: WorkspaceSession
    let selection: GitDiffSelection
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeFont.codeBaseSize

    private var store: GitStore { session.git }
    private var selectionIsCurrent: Bool {
        session.workspaceRoot == selection.cwd && store.cwd == selection.cwd
            && store.diffPath == selection.path && store.diffStaged == selection.staged
    }

    var body: some View {
        GeometryReader { geometry in
            let width = max(0, geometry.size.width - 2 * SeSpace.screenInset)
            let split = GitDiffPresentation.usesSplit(detailWidth: width,
                                                       minimumWidth: SeLayout.splitDiffMinimumWidth,
                                                       largeText: dynamicTypeSize.isAccessibilitySize)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: SeSpace.md) {
                    VStack(alignment: .leading, spacing: SeSpace.xs) {
                        Text(selection.path).font(SeFont.code(size: codeSize)).textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                        Label(selection.staged ? "Staged · read only" : "Unstaged · read only", systemImage: "lock")
                            .font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                    }
                    if session.workspaceRoot != selection.cwd {
                        SeEmptyState(title: "Workspace changed", systemImage: "folder",
                                     message: "Go back to inspect the selected workspace.")
                    } else if !selectionIsCurrent {
                        progress
                    } else {
                        switch store.diffState {
                        case .idle, .loading: progress
                        case .failed(let message):
                            SeErrorBanner(message: message) { Task { await load() } }
                        case .empty:
                            SeEmptyState(title: "No textual diff", systemImage: "doc.text.magnifyingglass",
                                         message: "The file may be untracked or unchanged. The host returned no diff.")
                        case .loaded:
                            diffSummary(split: split)
                            diffFiles(width: width, split: split)
                        }
                    }
                }
                .padding(SeSpace.screenInset)
            }
            .refreshable { await load() }
        }
        .background(SeTheme.canvas)
        .foregroundStyle(SeTheme.ink)
        .navigationTitle("Diff")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { Task { await load() } } label: {
                    Label("Refresh diff", systemImage: "arrow.clockwise")
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                }
                .disabled(store.diffState == .loading || session.workspaceRoot != selection.cwd)
            }
        }
        .task(id: selection) { await load() }
    }

    private func load() async {
        guard session.workspaceRoot == selection.cwd else { return }
        if store.cwd != selection.cwd { await store.refresh(cwd: selection.cwd) }
        guard session.workspaceRoot == selection.cwd else { return }
        await store.loadDiff(path: selection.path, staged: selection.staged)
    }

    private var progress: some View {
        HStack(spacing: SeSpace.sm) {
            if reduceMotion { Image(systemName: "hourglass") } else { ProgressView() }
            Text("Loading diff…").font(SeFont.support)
        }
        .frame(minHeight: SeSpace.hitTarget)
        .accessibilityElement(children: .combine)
    }

    private func diffSummary(split: Bool) -> some View {
        let count = GitDiffPresentation.counts(store.diff)
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: SeSpace.sm) { summaryLabels(count: count, split: split) }
            VStack(alignment: .leading, spacing: SeSpace.xs) { summaryLabels(count: count, split: split) }
        }
    }

    @ViewBuilder private func summaryLabels(count: (added: Int, removed: Int), split: Bool) -> some View {
        Label("\(count.added) added", systemImage: "plus").foregroundStyle(SeTheme.success)
        Label("\(count.removed) removed", systemImage: "minus").foregroundStyle(SeTheme.danger)
        Text(split ? "Split diff" : "Unified diff").foregroundStyle(SeTheme.secondaryInk)
    }

    private func diffFiles(width: CGFloat, split: Bool) -> some View {
        let lineGutterWidth = gutterWidth
        return ForEach(Array(store.diff.files.enumerated()), id: \.offset) { index, file in
            VStack(alignment: .leading, spacing: SeSpace.sm) {
                SeSectionHeader(title: file.path.isEmpty ? selection.path : file.path,
                                detail: "\(index + 1) of \(store.diff.files.count)")
                if file.isBinary {
                    Label("Binary file · no source preview", systemImage: "doc.zipper")
                        .font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                }
                // Metadata remains visible for renames, mode changes, binary files,
                // and combined merge diffs which cannot have honest line gutters.
                if !file.headers.isEmpty {
                    ScrollView(.horizontal) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(file.headers.enumerated()), id: \.offset) { _, line in
                                Text(line.content).font(SeFont.code(size: codeSize))
                                    .foregroundStyle(SeTheme.secondaryInk)
                                    .fixedSize(horizontal: true, vertical: false)
                            }
                        }
                        .padding(SeSpace.xs)
                        .textSelection(.enabled)
                    }
                    .background(SeTheme.surface)
                }
                ForEach(Array(file.hunks.enumerated()), id: \.offset) { _, hunk in
                    if split {
                        splitHunk(hunk, width: width, gutterWidth: lineGutterWidth)
                    } else {
                        unifiedHunk(hunk, width: width, gutterWidth: lineGutterWidth)
                    }
                }
            }
        }
    }

    private func unifiedHunk(_ hunk: UnifiedDiffHunk, width: CGFloat, gutterWidth: CGFloat) -> some View {
        ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                hunkHeader(hunk.header.content)
                ForEach(Array(hunk.lines.enumerated()), id: \.offset) { _, line in
                    HStack(spacing: 0) {
                        gutter(line.oldLineNumber, width: gutterWidth)
                        gutter(line.newLineNumber, width: gutterWidth)
                        source(line)
                    }
                    .frame(minWidth: width, alignment: .leading)
                    .background(background(line.kind))
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(accessibilityLine(line))
                }
            }
            .font(SeFont.code(size: codeSize))
            .textSelection(.enabled)
        }
        .background(SeTheme.surface)
    }

    private func splitHunk(_ hunk: UnifiedDiffHunk, width: CGFloat, gutterWidth: CGFloat) -> some View {
        let rows = GitDiffPresentation.splitRows(hunk.lines)
        let paneWidth = max(0, (width - 1) / 2)
        return VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal) { hunkHeader(hunk.header.content) }
            HStack(spacing: 0) {
                splitPane(rows, old: true, width: paneWidth, gutterWidth: gutterWidth)
                Rectangle().fill(SeTheme.stroke).frame(width: 1)
                splitPane(rows, old: false, width: paneWidth, gutterWidth: gutterWidth)
            }
        }
        .background(SeTheme.surface)
    }

    private func splitPane(_ rows: [GitDiffPresentation.SplitRow], old: Bool, width: CGFloat, gutterWidth: CGFloat) -> some View {
        ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                Text(old ? "Before" : "After")
                    .foregroundStyle(SeTheme.secondaryInk)
                    .padding(.horizontal, SeSpace.xs)
                    .frame(height: rowHeight)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    let line = old ? row.old : row.new
                    HStack(spacing: 0) {
                        gutter(old ? line?.oldLineNumber : line?.newLineNumber, width: gutterWidth)
                        if let line { source(line) } else { Text(" ").frame(height: rowHeight) }
                    }
                    .frame(minWidth: width, minHeight: rowHeight, alignment: .leading)
                    .background(line.map { background($0.kind) } ?? SeTheme.elevated)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(line.map { accessibilityLine($0) } ?? "No corresponding line")
                }
            }
            .font(SeFont.code(size: codeSize))
            .textSelection(.enabled)
        }
        .frame(width: width)
    }

    private var rowHeight: CGFloat { ceil(codeSize * 1.65) }
    private var gutterWidth: CGFloat {
        let largest = store.diff.files.flatMap(\.hunks).flatMap(\.lines)
            .map { max($0.oldLineNumber ?? 0, $0.newLineNumber ?? 0) }.max() ?? 1
        return CGFloat(max(3, String(largest).count)) * codeSize * 0.65 + SeSpace.sm
    }

    private func gutter(_ number: Int?, width: CGFloat) -> some View {
        Text(GitDiffPresentation.gutter(number))
            .foregroundStyle(SeTheme.tertiaryInk)
            .frame(width: width, height: rowHeight, alignment: .trailing)
            .padding(.trailing, SeSpace.xs)
            .accessibilityHidden(true)
    }

    private func source(_ line: UnifiedDiffLine) -> some View {
        Text(GitDiffPresentation.prefix(for: line) + " " + line.content
             + (line.noNewlineAtEnd ? "  [no newline at end of file]" : ""))
            .fixedSize(horizontal: true, vertical: false)
            .foregroundStyle(line.kind == .addition ? SeTheme.diffAddedInk : line.kind == .removal ? SeTheme.diffRemovedInk : SeTheme.ink)
            .frame(height: rowHeight, alignment: .leading)
            .padding(.trailing, SeSpace.xs)
    }

    private func hunkHeader(_ title: String) -> some View {
        Text(title).font(SeFont.code(size: codeSize))
            .foregroundStyle(SeTheme.secondaryInk)
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, SeSpace.xs)
            .frame(minHeight: rowHeight)
            .background(SeTheme.elevated)
    }

    private func background(_ kind: UnifiedDiffLine.Kind) -> Color {
        switch kind {
        case .addition: SeTheme.diffAdded
        case .removal: SeTheme.diffRemoved
        case .header: SeTheme.elevated
        case .context: SeTheme.codeCanvas
        }
    }

    private func accessibilityLine(_ line: UnifiedDiffLine) -> String {
        let change: String
        switch line.kind {
        case .addition: change = "Added"
        case .removal: change = "Removed"
        case .context: change = "Context"
        case .header: change = "Header"
        }
        let old = line.oldLineNumber.map { " before line \($0)" } ?? ""
        let new = line.newLineNumber.map { " after line \($0)" } ?? ""
        return "\(change)\(old)\(new), \(line.content)" + (line.noNewlineAtEnd ? ", no newline at end of file" : "")
    }
}
