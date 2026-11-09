import SwiftUI

/// Fixed-string, case-insensitive workspace search. The host returns file
/// matches; this screen presents only that real result set. Opening a file uses
/// an owned, cancellable task and refuses paths outside the workspace root.
struct SearchFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession
    var onOpenFile: () -> Void

    @State private var query = ""
    @State private var openingFile = false
    @State private var fileError: String?
    /// The in-flight file open. Cancelled when the screen goes away so a slow
    /// read cannot navigate after the user has left.
    @State private var openTask: Task<Void, Never>?

    private var store: SearchStore { session.search }

    var body: some View {
        NocturneToolScreen(
            title: String(localized: "Search"),
            subtitle: navSubtitle,
            onRefresh: { await store.refresh() }
        ) {
            searchField
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)

            if openingFile {
                progress(String(localized: "Opening preview…"))
            } else if store.isLoading {
                progress(String(localized: "Searching…"))
            }

            if let error = store.errorMessage {
                NocturneNotice(message: error, isError: true)
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 14)
            }
            if let fileError {
                NocturneNotice(message: fileError, isError: true)
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 14)
            }
            if store.response?.truncated == true {
                NocturneNotice(
                    message: String(localized: "Results were truncated. Narrow the search to see every match."),
                    isError: false
                )
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
            }

            results
        }
        .task {
            let root = session.workspaceRoot ?? ""
            store.configure(scopeRoot: root, rootPath: root)
            query = store.query
            if !query.isEmpty { store.setQuery(query) }
        }
        .onChange(of: query) { _, value in store.setQuery(value) }
        .onDisappear {
            store.cancelSearch()
            openTask?.cancel()
            openTask = nil
        }
    }

    // MARK: Header

    private var navSubtitle: String {
        let host = session.origin.host() ?? String(localized: "Host")
        return "\(host) · \(String(localized: "Files"))"
    }

    private var searchField: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 13, weight: .regular))
                .foregroundStyle(palette.inkTertiary)
            TextField(String(localized: "Search workspace contents…"), text: $query)
                .font(SeNocturneType.font(12, weight: .regular))
                .foregroundStyle(palette.ink)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .onSubmit { Task { await store.refresh() } }
                .frame(minHeight: NocturneToolMetrics.minimumTarget)
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 14, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                        .frame(width: NocturneToolMetrics.minimumTarget, height: NocturneToolMetrics.minimumTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "Clear search"))
            }
        }
        .padding(.leading, 13)
        .padding(.trailing, 4)
        .frame(height: 40)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func progress(_ label: String) -> some View {
        HStack(spacing: 8) {
            ProgressView()
            Text(verbatim: label)
                .font(SeNocturneType.font(11, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 14)
        .frame(minHeight: NocturneToolMetrics.minimumTarget)
    }

    // MARK: Results

    @ViewBuilder
    private var results: some View {
        let groups = SearchResultPresentation.groups(store.results)

        if groups.isEmpty && !store.isLoading && store.errorMessage == nil {
            NocturneEmptyState(
                systemImage: "magnifyingglass",
                title: trimmedQuery.isEmpty ? String(localized: "Search this workspace") : String(localized: "No matches")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
        } else if !groups.isEmpty {
            NocturneGroupLabel(
                title: String(localized: "Files"),
                detail: store.response.map { String(format: "%02d", $0.results.count) } ?? ""
            )
            VStack(spacing: NocturneToolMetrics.cardGap) {
                ForEach(groups) { group in
                    fileCard(group)
                }
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)

            if let response = store.response, response.failedFiles > 0 || response.scannedFiles > 0 {
                Text(verbatim: scanDetail(response))
                    .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 10)
            }
        }
    }

    private func fileCard(_ group: SearchFileGroup) -> some View {
        NocturneCard {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(verbatim: URL(fileURLWithPath: group.filePath).lastPathComponent)
                            .font(SeNocturneType.font(12.5, weight: .semibold))
                            .foregroundStyle(palette.ink)
                            .lineLimit(1)
                        Text(verbatim: parentPath(group.filePath))
                            .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Text(verbatim: String(localized: "\(group.matches.count) hits"))
                        .font(SeNocturneType.font(9, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
                .padding(.bottom, 6)

                ForEach(Array(group.matches.enumerated()), id: \.offset) { index, match in
                    Button {
                        openFile(group.filePath)
                    } label: {
                        HStack(alignment: .top, spacing: 10) {
                            Text(verbatim: "\(match.lineNumber)")
                                .font(SeNocturneType.font(9, weight: .regular, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                                .frame(minWidth: 30, alignment: .trailing)
                            Text(highlighted(match.lineText))
                                .font(SeNocturneType.font(10.5, weight: .regular, mono: true))
                                .foregroundStyle(palette.terminalInk)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .lineLimit(2)
                        }
                        .frame(minHeight: NocturneToolMetrics.minimumTarget, alignment: .topLeading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(openingFile)
                    .accessibilityLabel(Text("Line \(match.lineNumber): \(match.lineText)"))
                    if index < group.matches.count - 1 {
                        Rectangle()
                            .fill(palette.stroke)
                            .frame(height: 1)
                    }
                }
            }
        }
    }

    // MARK: Helpers

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func parentPath(_ path: String) -> String {
        let parent = (path as NSString).deletingLastPathComponent
        return parent.isEmpty ? "/" : parent
    }

    private func scanDetail(_ response: FileSearchResponse) -> String {
        var parts = [String(localized: "\(response.scannedFiles) files scanned")]
        if response.failedFiles > 0 {
            parts.append(String(localized: "\(response.failedFiles) failed"))
        }
        return parts.joined(separator: " · ")
    }

    private func highlighted(_ line: String) -> AttributedString {
        var result = AttributedString(line)
        for range in SearchResultPresentation.highlightRanges(in: line, query: query) {
            if let lower = AttributedString.Index(range.lowerBound, within: result),
               let upper = AttributedString.Index(range.upperBound, within: result) {
                result[lower..<upper].backgroundColor = palette.accent
                result[lower..<upper].foregroundColor = palette.onAccent
            }
        }
        return result
    }

    // MARK: Open file

    private func openFile(_ path: String) {
        guard !openingFile else { return }
        fileError = nil

        // Capture the workspace this result belongs to. A result outside the
        // root, or a root that changes mid-read, is not this screen's to apply.
        guard let root = session.workspaceRoot?.nocturneNonEmpty else {
            fileError = String(localized: "No workspace is selected.")
            return
        }
        guard Self.isInsideRoot(path, root: root) else {
            fileError = String(localized: "This result is outside the workspace root.")
            return
        }

        openingFile = true
        openTask = Task { @MainActor in
            defer { openingFile = false }
            let opened = await session.files.openReturningSuccess(DirectoryEntry(
                name: URL(fileURLWithPath: path).lastPathComponent, path: path, type: "file"))
            guard !Task.isCancelled, session.workspaceRoot == root else { return }
            guard opened else {
                fileError = session.files.errorMessage ?? String(localized: "The host could not open this file preview.")
                return
            }
            session.setWorkspaceTab(.files)
            onOpenFile()
        }
    }

    /// Standardized prefix check: `/root/apple` is not inside `/root/app`.
    static func isInsideRoot(_ path: String, root: String) -> Bool {
        let rootURL = URL(fileURLWithPath: root).standardizedFileURL
        let fileURL = URL(fileURLWithPath: path).standardizedFileURL
        if fileURL.path == rootURL.path { return true }
        let prefix = rootURL.path.hasSuffix("/") ? rootURL.path : rootURL.path + "/"
        return fileURL.path.hasPrefix(prefix)
    }
}
