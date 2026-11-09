import SwiftUI

struct GitDiffView: View {
    @Environment(\.sePalette) private var p
    let session: WorkspaceSession
    let selection: GitDiffSelection
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeNocturneType.bodySize

    private var store: GitStore { session.git }
    private var selectionIsCurrent: Bool {
        session.workspaceRoot == selection.cwd && store.cwd == selection.cwd
            && store.diffPath == selection.path && store.diffStaged == selection.staged
    }

    var body: some View {
        GeometryReader { geometry in
            let width = max(0, geometry.size.width - 2 * 20)
            let split = GitDiffPresentation.usesSplit(detailWidth: width,
                                                       minimumWidth: SeLayout.splitDiffMinimumWidth,
                                                       largeText: dynamicTypeSize.isAccessibilitySize)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(selection.path).font(SeNocturneType.font(codeSize, mono: true)).textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                        Label(selection.staged ? "Staged · read only" : "Unstaged · read only", systemImage: "lock")
                            .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                    }
                    if session.workspaceRoot != selection.cwd {
                        ToolNocturneEmptyState(title: "Workspace changed", systemImage: "folder",
                                     message: "Go back to inspect the selected workspace.")
                    } else if !selectionIsCurrent {
                        progress
                    } else {
                        switch store.diffState {
                        case .idle, .loading: progress
                        case .failed(let message):
                            ToolNocturneErrorBanner(message: message) { Task { await load() } }
                        case .empty:
                            ToolNocturneEmptyState(title: "No textual diff", systemImage: "doc.text.magnifyingglass",
                                         message: "The file may be untracked or unchanged. The host returned no diff.")
                        case .loaded:
                            diffSummary(split: split)
                            diffFiles(width: width, split: split)
                        }
                    }
                }
                .padding(20)
            }
            .refreshable { await load() }
        }
        .background(p.canvas)
        .tint(p.accent)
        .toolNocturneFont(.body)
        .foregroundStyle(p.ink)
        .navigationTitle("Diff")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { Task { await load() } } label: {
                    Label("Refresh diff", systemImage: "arrow.clockwise")
                        .frame(minWidth: 44, minHeight: 44)
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
        HStack(spacing: 12) {
            if reduceMotion { Image(systemName: "hourglass") } else { ProgressView() }
            Text("Loading diff…").toolNocturneFont(.support)
        }
        .frame(minHeight: 44)
        .accessibilityElement(children: .combine)
    }

    private func diffSummary(split: Bool) -> some View {
        let count = GitDiffPresentation.counts(store.diff)
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) { summaryLabels(count: count, split: split) }
            VStack(alignment: .leading, spacing: 8) { summaryLabels(count: count, split: split) }
        }
    }

    @ViewBuilder private func summaryLabels(count: (added: Int, removed: Int), split: Bool) -> some View {
        Label("\(count.added) added", systemImage: "plus").foregroundStyle(p.live)
        Label("\(count.removed) removed", systemImage: "minus").foregroundStyle(p.danger)
        Text(split ? "Split diff" : "Unified diff").foregroundStyle(p.inkSecondary)
    }

    private func diffFiles(width: CGFloat, split: Bool) -> some View {
        let lineGutterWidth = gutterWidth
        return ForEach(Array(store.diff.files.enumerated()), id: \.offset) { index, file in
            VStack(alignment: .leading, spacing: 12) {
                ToolNocturneSectionHeader(title: file.path.isEmpty ? selection.path : file.path,
                                detail: "\(index + 1) of \(store.diff.files.count)")
                if file.isBinary {
                    Label("Binary file · no source preview", systemImage: "doc.zipper")
                        .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                }
                // Metadata remains visible for renames, mode changes, binary files,
                // and combined merge diffs which cannot have honest line gutters.
                if !file.headers.isEmpty {
                    ScrollView(.horizontal) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(file.headers.enumerated()), id: \.offset) { _, line in
                                Text(line.content).font(SeNocturneType.font(codeSize, mono: true))
                                    .foregroundStyle(p.inkSecondary)
                                    .fixedSize(horizontal: true, vertical: false)
                            }
                        }
                        .padding(8)
                        .textSelection(.enabled)
                    }
                    .background(p.codeCanvas, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.stroke) }
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
            .font(SeNocturneType.font(codeSize, mono: true))
            .textSelection(.enabled)
        }
        .background(p.codeCanvas, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.stroke) }
    }

    private func splitHunk(_ hunk: UnifiedDiffHunk, width: CGFloat, gutterWidth: CGFloat) -> some View {
        let rows = GitDiffPresentation.splitRows(hunk.lines)
        let paneWidth = max(0, (width - 1) / 2)
        return VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal) { hunkHeader(hunk.header.content) }
            HStack(spacing: 0) {
                splitPane(rows, old: true, width: paneWidth, gutterWidth: gutterWidth)
                Rectangle().fill(p.stroke).frame(width: 1)
                splitPane(rows, old: false, width: paneWidth, gutterWidth: gutterWidth)
            }
        }
        .background(p.codeCanvas, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.stroke) }
    }

    private func splitPane(_ rows: [GitDiffPresentation.SplitRow], old: Bool, width: CGFloat, gutterWidth: CGFloat) -> some View {
        ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                Text(old ? "Before" : "After")
                    .foregroundStyle(p.inkSecondary)
                    .padding(.horizontal, 8)
                    .frame(height: rowHeight)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    let line = old ? row.old : row.new
                    HStack(spacing: 0) {
                        gutter(old ? line?.oldLineNumber : line?.newLineNumber, width: gutterWidth)
                        if let line { source(line) } else { Text(" ").frame(height: rowHeight) }
                    }
                    .frame(minWidth: width, minHeight: rowHeight, alignment: .leading)
                    .background(line.map { background($0.kind) } ?? p.cardAlt)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(line.map { accessibilityLine($0) } ?? "No corresponding line")
                }
            }
            .font(SeNocturneType.font(codeSize, mono: true))
            .textSelection(.enabled)
        }
        .frame(width: width)
    }

    private var rowHeight: CGFloat { ceil(codeSize * 1.65) }
    private var gutterWidth: CGFloat {
        let largest = store.diff.files.flatMap(\.hunks).flatMap(\.lines)
            .map { max($0.oldLineNumber ?? 0, $0.newLineNumber ?? 0) }.max() ?? 1
        return CGFloat(max(3, String(largest).count)) * codeSize * 0.65 + 12
    }

    private func gutter(_ number: Int?, width: CGFloat) -> some View {
        Text(GitDiffPresentation.gutter(number))
            .foregroundStyle(p.inkTertiary)
            .frame(width: width, height: rowHeight, alignment: .trailing)
            .padding(.trailing, 8)
            .accessibilityHidden(true)
    }

    private func source(_ line: UnifiedDiffLine) -> some View {
        Text(GitDiffPresentation.prefix(for: line) + " " + line.content
             + (line.noNewlineAtEnd ? "  [no newline at end of file]" : ""))
            .fixedSize(horizontal: true, vertical: false)
            .foregroundStyle(line.kind == .addition ? p.live : line.kind == .removal ? p.danger : p.ink)
            .frame(height: rowHeight, alignment: .leading)
            .padding(.trailing, 8)
    }

    private func hunkHeader(_ title: String) -> some View {
        Text(title).font(SeNocturneType.font(codeSize, mono: true))
            .foregroundStyle(p.inkSecondary)
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, 8)
            .frame(minHeight: rowHeight)
            .background(p.cardAlt)
    }

    private func background(_ kind: UnifiedDiffLine.Kind) -> Color {
        switch kind {
        case .addition: p.live.opacity(0.12)
        case .removal: p.danger.opacity(0.12)
        case .header: p.cardAlt
        case .context: p.codeCanvas
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
