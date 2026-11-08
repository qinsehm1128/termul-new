import SwiftUI

/// Filters only names from the supplied listing; it never searches paths or file contents.
/// Keeping the store's order also preserves its directories-first ordering.
enum WorkspaceFilesFilter {
    nonisolated static func entries(_ entries: [DirectoryEntry], matching query: String) -> [DirectoryEntry] {
        let term = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !term.isEmpty else { return entries }
        return entries.filter { $0.name.localizedStandardContains(term) }
    }
}

@MainActor
struct WorkspaceFileListView: View {
    let entries: [DirectoryEntry]
    @Binding var query: String
    @Binding var scrollPath: String?
    let selectedPath: String?
    let isLoading: Bool
    let hasError: Bool
    let open: (DirectoryEntry) -> Void
    let refresh: () async -> Void

    private var filteredEntries: [DirectoryEntry] {
        WorkspaceFilesFilter.entries(entries, matching: query)
    }

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: SeSpace.xs) {
                SeSectionHeader(title: String(localized: "Files"), detail: String(localized: "Read only"))
                HStack(spacing: SeSpace.xs) {
                    Image(systemName: "line.3.horizontal.decrease")
                        .foregroundStyle(SeTheme.secondaryInk)
                        .accessibilityHidden(true)
                    TextField("Filter names in this folder", text: $query)
                        .font(SeFont.support)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .frame(minHeight: SeSpace.hitTarget)
                        .accessibilityIdentifier("files-name-filter")
                    if !query.isEmpty {
                        Button {
                            query = ""
                        } label: {
                            Label("Clear name filter", systemImage: "xmark.circle.fill")
                                .labelStyle(.iconOnly)
                                .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        }
                    }
                }
                .padding(.horizontal, SeSpace.sm)
                .frame(minHeight: SeSpace.hitTarget)
                .background(SeTheme.surface, in: RoundedRectangle(cornerRadius: SeRadius.tool))
                Text("Names in this folder only. File contents are not searched.")
                    .font(SeFont.caption)
                    .foregroundStyle(SeTheme.secondaryInk)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(SeSpace.screenInset)

            if isLoading {
                ProgressView("Loading files…")
                    .font(SeFont.support)
                    .tint(SeTheme.accent)
                    .padding(SeSpace.sm)
            }

            ScrollView {
                if entries.isEmpty && !isLoading {
                    SeEmptyState(
                        title: hasError ? String(localized: "Folder unavailable") : String(localized: "No files"),
                        systemImage: hasError ? "folder.badge.questionmark" : "folder",
                        message: hasError
                            ? String(localized: "Use Retry above to load this folder again.")
                            : String(localized: "This folder is empty.")
                    )
                } else if filteredEntries.isEmpty && !isLoading {
                    SeEmptyState(
                        title: String(localized: "No matching names"),
                        systemImage: "line.3.horizontal.decrease",
                        message: String(localized: "Try another name or clear the filter to see this folder's files."),
                        actionTitle: String(localized: "Clear filter"),
                        action: { query = "" }
                    )
                } else {
                    LazyVStack(spacing: SeSpace.xxs) {
                        ForEach(filteredEntries) { entry in
                            row(entry)
                                .id(entry.path)
                        }
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, SeSpace.screenInset)
                    .padding(.bottom, SeSpace.sm)
                }
            }
            .scrollPosition(id: $scrollPath)
            .refreshable { await refresh() }
            .scrollDismissesKeyboard(.interactively)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityIdentifier("files-list")
    }

    private func row(_ entry: DirectoryEntry) -> some View {
        let selected = selectedPath == entry.path
        return Button {
            open(entry)
        } label: {
            HStack(spacing: SeSpace.sm) {
                Image(systemName: entry.isDirectory ? "folder" : "doc.text")
                    .frame(width: 20)
                    .accessibilityHidden(true)
                Text(entry.name)
                    .font(SeFont.support)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .multilineTextAlignment(.leading)
                if entry.isDirectory {
                    Image(systemName: "chevron.right")
                        .font(SeFont.caption)
                        .foregroundStyle(SeTheme.tertiaryInk)
                        .accessibilityHidden(true)
                } else if let size = entry.size {
                    Text(ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file))
                        .font(SeFont.caption)
                        .foregroundStyle(SeTheme.secondaryInk)
                }
            }
            .foregroundStyle(selected ? SeTheme.accent : (entry.isDirectory ? SeTheme.secondaryInk : SeTheme.ink))
            .padding(.horizontal, SeSpace.sm)
            .padding(.vertical, SeSpace.xs)
            .frame(minHeight: SeSpace.hitTarget)
            .background(selected ? SeTheme.accentSubtle : .clear, in: RoundedRectangle(cornerRadius: SeRadius.tool))
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressableRowStyle())
        .disabled(isLoading)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityHint(entry.isDirectory ? Text("Open folder") : Text("Open read-only preview"))
    }
}
