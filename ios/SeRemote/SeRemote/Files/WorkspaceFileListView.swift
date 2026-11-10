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

/// Prototype file list: `.scard` rows with a 3pt spine, name, real size/date
/// metadata and a chevron. Folder summaries come from the host listing.
@MainActor
struct WorkspaceFileListView: View {
    @Environment(\.sePalette) private var palette
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
            ScrollView {
                LazyVStack(spacing: 8) {
                    if isLoading {
                        ProgressView()
                            .tint(palette.accent)
                            .padding(.vertical, SeSpace.md)
                    }
                    if entries.isEmpty && !isLoading {
                        SeEmptyState(
                            title: hasError ? String(localized: "Folder unavailable") : String(localized: "No files"),
                            systemImage: hasError ? "folder.badge.questionmark" : "folder",
                            message: hasError
                                ? String(localized: "Use Refresh above to load this folder again.")
                                : nil
                        )
                        .frame(maxWidth: .infinity, minHeight: 160)
                    } else if filteredEntries.isEmpty && !isLoading {
                        SeEmptyState(
                            title: String(localized: "No matching names"),
                            systemImage: "line.3.horizontal.decrease",
                            actionTitle: String(localized: "Clear filter"),
                            action: { query = "" }
                        )
                        .frame(maxWidth: .infinity, minHeight: 160)
                    } else {
                        ForEach(filteredEntries) { entry in
                            row(entry)
                                .id(entry.path)
                        }
                    }
                }
                .padding(.horizontal, 18)
                .padding(.bottom, 16)
            }
            .scrollPosition(id: $scrollPath)
            .refreshable { await refresh() }
            .scrollDismissesKeyboard(.interactively)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityIdentifier("files-list")
    }

    private var filterField: some View {
        HStack(spacing: 8) {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(palette.inkTertiary)
                .accessibilityHidden(true)
            TextField("Filter names in this folder", text: $query)
                .font(.system(size: 12))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.done)
                .frame(minHeight: SeSpace.hitTarget)
                .accessibilityIdentifier("files-name-filter")
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 13))
                        .foregroundStyle(palette.inkTertiary)
                        .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("Clear name filter"))
            }
        }
        .padding(.leading, 10)
        .frame(minHeight: 36)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .stroke(palette.stroke, lineWidth: 1)
        }
    }

    private func row(_ entry: DirectoryEntry) -> some View {
        let selected = selectedPath == entry.path
        return Button {
            open(entry)
        } label: {
            HStack(spacing: 11) {
                R2Spine()
                VStack(alignment: .leading, spacing: 3) {
                    Text(entry.name)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        if entry.isDirectory {
                            Text("Folder")
                        } else if let size = FilePanelPresentation.byteCount(entry.size) {
                            Text(size)
                        }
                        if let modified = FilePanelPresentation.modificationDate(entry.modifiedAt) {
                            Text("Modified")
                            Text(modified, style: .relative)
                        }
                    }
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.inkTertiary)
                    .accessibilityHidden(true)
            }
            .padding(12)
            .frame(minHeight: SeSpace.hitTarget)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .stroke(selected ? palette.accent.opacity(0.6) : palette.stroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(isLoading)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityHint(entry.isDirectory ? Text("Open folder") : Text("Open read-only preview"))
    }
}
