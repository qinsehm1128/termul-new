import SwiftUI

struct SearchFeatureView: View {
    @Environment(\.sePalette) private var p
    var session: WorkspaceSession
    var onOpenFile: () -> Void
    @State private var query = ""
    @State private var openingFile = false
    @State private var fileError: String?
    /// The in-flight file open. Cancelled when the screen goes away so a slow
    /// read cannot navigate after the user has left.
    @State private var openTask: Task<Void, Never>?
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeNocturneType.bodySize

    var body: some View {
        List {
            Section {
                TextField("Search workspace contents", text: $query)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    .submitLabel(.search).frame(minHeight: 44)
                    .onSubmit { Task { await session.search.refresh() } }
                Label(session.workspaceRoot ?? String(localized: "No workspace selected"), systemImage: "folder")
                    .toolNocturneFont(.support).textSelection(.enabled)
                Text("Fixed-string, case-insensitive search. Up to 500 characters.")
                    .toolNocturneFont(.caption).foregroundStyle(p.inkSecondary)
            }
            if session.search.isLoading || openingFile {
                Section { ToolNocturneProgress(openingFile ? String(localized: "Opening preview…") : String(localized: "Searching…")) }
            }
            if let error = session.search.errorMessage {
                Section { ToolNocturneErrorBanner(message: error) { Task { await session.search.refresh() } } }
            }
            if let error = fileError {
                Section { ToolNocturneErrorBanner(message: error) }
            }
            if session.search.response?.truncated == true {
                Section {
                    Label("Results were truncated. Narrow your search to find more specific matches.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(p.warning).toolNocturneFont(.support)
                }
            }
            ForEach(SearchResultPresentation.groups(session.search.results)) { group in
                Section {
                    ForEach(group.matches, id: \.self) { match in
                        Button {
                            openFile(group.filePath)
                        } label: {
                            HStack(alignment: .top, spacing: 12) {
                                Text("\(match.lineNumber)").foregroundStyle(p.inkTertiary)
                                    .monospacedDigit()
                                Text(highlighted(match.lineText))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .font(SeNocturneType.font(codeSize, mono: true)).foregroundStyle(p.ink)
                            .frame(minHeight: 44, alignment: .leading)
                        }
                        .disabled(openingFile)
                        .listRowBackground(p.card)
                        .accessibilityLabel(Text("Line \(match.lineNumber): \(match.lineText)"))
                        .accessibilityHint("Open this file in Files")
                    }
                } header: { Text(group.filePath).textCase(nil) }
            }
            if session.search.results.isEmpty && !session.search.isLoading && session.search.errorMessage == nil {
                Section {
                    ToolNocturneEmptyState(title: query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?
                        String(localized: "Search this workspace") : String(localized: "No matches"),
                        systemImage: "magnifyingglass",
                        message: String(localized: "Search file contents without changing anything on the host."))
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(p.canvas)
        .foregroundStyle(p.ink)
        .toolNocturneFont(.body)
        .tint(p.accent)
        .navigationTitle("Search")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("Refresh", systemImage: "arrow.clockwise") { Task { await session.search.refresh() } }
                    .frame(minWidth: 44, minHeight: 44)
                    .disabled(session.search.isLoading || query.isEmpty)
            }
        }
        .task {
            let root = session.workspaceRoot ?? ""
            session.search.configure(scopeRoot: root, rootPath: root)
            query = session.search.query
            if !query.isEmpty { session.search.setQuery(query) }
        }
        .onChange(of: query) { _, value in session.search.setQuery(value) }
        .onDisappear {
            session.search.cancelSearch()
            openTask?.cancel()
            openTask = nil
        }
        .refreshable { await session.search.refresh() }
    }

    private func highlighted(_ line: String) -> AttributedString {
        var result = AttributedString(line)
        for range in SearchResultPresentation.highlightRanges(in: line, query: query) {
            if let lower = AttributedString.Index(range.lowerBound, within: result),
               let upper = AttributedString.Index(range.upperBound, within: result) {
                result[lower..<upper].backgroundColor = p.accent
                result[lower..<upper].foregroundColor = p.onAccent
            }
        }
        return result
    }

    private func openFile(_ path: String) {
        guard !openingFile else { return }
        openingFile = true
        fileError = nil
        // Capture the workspace this result belongs to. If it changes while
        // the read is in flight, the result is no longer this screen's to apply.
        let root = session.workspaceRoot
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
}
