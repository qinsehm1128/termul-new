import SwiftUI

/// Nocturne files browse + read-only preview (prototype `data-screen="files"`).
///
/// Compact width shows the prototype's breadcrumb, group label, `.scard` list
/// and a "Recently read" preview card pinned below. Regular width keeps the
/// list/preview split. Browsing, refresh, filter and search-opened previews all
/// remain owned by FileStore.
@MainActor
struct FileBrowserView: View {
    @Environment(\.sePalette) private var palette
    @Bindable var session: WorkspaceSession
    var embedded = false
    @Environment(\.dismiss) private var dismiss
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @State private var nameFilter = ""
    @State private var selectedPath: String?
    @State private var listScrollPath: String?
    @State private var retryFile: DirectoryEntry?
    @State private var isRequesting = false

    private var busy: Bool { isRequesting || session.files.isLoading }

    var body: some View {
        Group {
            if embedded {
                browser
            } else {
                NavigationStack {
                    browser
                        .navigationTitle(title)
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Close") { dismiss() }
                            }
                        }
                }
            }
        }
        .seHaptic(trigger: selectedPath) { old, new in
            old != new && new != nil ? .selection : nil
        }
        .task {
            if selectedPath == nil, let previewName = session.files.previewName {
                selectedPath = session.files.entries.first { $0.name == previewName }?.path
            }
            guard session.files.crumbs.isEmpty else { return }
            if let path = session.projects.active?.path ?? session.conversations.active?.workspaceCwd {
                await perform { await session.files.openRoot(path) }
            }
        }
    }

    private var title: String {
        session.files.previewName
            ?? session.files.crumbs.last?.name
            ?? String(localized: "Files")
    }

    private var browser: some View {
        GeometryReader { geometry in
            let split = SeLayout.columns(
                width: geometry.size.width,
                isRegular: sizeClass == .regular,
                largeText: dynamicTypeSize.isAccessibilitySize
            ) != .single
            VStack(spacing: 0) {
                breadcrumb
                if let message = session.files.errorMessage {
                    SeErrorBanner(message: message) {
                        Task { await refresh() }
                    }
                    .disabled(busy)
                    .padding(.horizontal, 18)
                    .padding(.top, 8)
                    .accessibilityIdentifier("files-error")
                }
                if split {
                    HStack(spacing: 0) {
                        VStack(spacing: 0) {
                            R2GroupLabel(title: folderLabel, detail: String(localized: "RO"))
                                .padding(.horizontal, 18)
                            fileList
                        }
                        .frame(width: min(320, geometry.size.width * 0.4))
                        Rectangle().fill(palette.stroke).frame(width: 1)
                        previewPane
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                } else {
                    R2GroupLabel(title: folderLabel, detail: String(localized: "RO"))
                        .padding(.horizontal, 18)
                    fileList
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.surface)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if !split, session.files.hasPreview {
                    previewCard
                }
            }
        }
    }

    // MARK: - Breadcrumb (prototype mono path)

    private var breadcrumb: some View {
        HStack(spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 5) {
                    ForEach(Array(session.files.crumbs.enumerated()), id: \.element.id) { index, crumb in
                        if index > 0 {
                            Text("/")
                                .foregroundStyle(palette.inkTertiary)
                                .accessibilityHidden(true)
                        }
                        Button {
                            Task { await popTo(crumb) }
                        } label: {
                            Text(crumb.name)
                                .lineLimit(1)
                                .foregroundStyle(index == session.files.crumbs.count - 1
                                    ? palette.ink : palette.inkTertiary)
                                .frame(minHeight: SeSpace.hitTarget)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(busy || (index == session.files.crumbs.count - 1 && !session.files.hasPreview && session.files.errorMessage == nil))
                        .accessibilityHint(Text("Open this folder"))
                    }
                }
            }
            Button {
                Task { await refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(busy || session.files.crumbs.isEmpty)
            .accessibilityLabel(Text("Refresh"))
            .accessibilityIdentifier("files-refresh")
        }
        .font(.system(size: 10, design: .monospaced))
        .padding(.horizontal, 18)
        .padding(.top, 12)
        .accessibilityIdentifier("files-crumbs")
    }

    private var folderLabel: String {
        NocturneR2Presentation.folderSummary(
            name: session.files.crumbs.last?.name,
            count: session.files.entries.count
        )
    }

    private var fileList: some View {
        WorkspaceFileListView(
            entries: session.files.errorMessage != nil && retryFile == nil ? [] : session.files.entries,
            query: $nameFilter,
            scrollPath: $listScrollPath,
            selectedPath: selectedPath,
            isLoading: busy,
            hasError: session.files.errorMessage != nil,
            open: { entry in Task { await open(entry) } },
            refresh: { await refresh() }
        )
    }

    @ViewBuilder
    private var previewPane: some View {
        if session.files.hasPreview {
            VStack(spacing: 0) {
                R2GroupLabel(title: String(localized: "Recently read"), detail: String(localized: "RO"))
                    .padding(.horizontal, 18)
                WorkspaceFilePreviewView(
                    name: session.files.previewName ?? String(localized: "File preview"),
                    content: session.files.preview,
                    kind: session.files.previewKind,
                    imageData: session.files.previewImage,
                    failure: session.files.previewFailure,
                    isLoading: busy,
                    refresh: { await refresh() }
                )
            }
            .background(palette.surface)
        } else {
            SeEmptyState(
                title: String(localized: "Select a file"),
                systemImage: "doc.text"
            )
        }
    }

    /// Compact width: the prototype pins the recently read card below the list.
    private var previewCard: some View {
        VStack(spacing: 0) {
            R2GroupLabel(title: String(localized: "Recently read"), detail: String(localized: "RO"))
                .padding(.horizontal, 18)
            if session.files.hasPreview {
                WorkspaceFilePreviewView(
                    name: session.files.previewName ?? String(localized: "File preview"),
                    content: session.files.preview,
                    kind: session.files.previewKind,
                    imageData: session.files.previewImage,
                    failure: session.files.previewFailure,
                    isLoading: busy,
                    refresh: { await refresh() }
                )
                .frame(maxHeight: 260)
            }
        }
        .background(palette.surface)
        .overlay(alignment: .top) {
            Rectangle().fill(palette.stroke).frame(height: 1)
        }
    }

    private func perform(_ action: @MainActor () async -> Void) async {
        guard !busy else { return }
        isRequesting = true
        session.files.errorMessage = nil
        defer { isRequesting = false }
        await action()
    }

    private func open(_ entry: DirectoryEntry) async {
        await perform {
            retryFile = entry.isDirectory ? nil : entry
            if entry.isDirectory {
                nameFilter = ""
                selectedPath = nil
                listScrollPath = nil
            } else {
                selectedPath = entry.path
            }
            await session.files.open(entry)
        }
    }

    private func popTo(_ crumb: DirectoryEntry) async {
        await perform {
            retryFile = nil
            // Returning from a preview to its folder preserves filter and row selection.
            if crumb.path != session.files.crumbs.last?.path {
                nameFilter = ""
                selectedPath = nil
                listScrollPath = nil
            }
            await session.files.popTo(crumb)
        }
    }

    private func refresh() async {
        await perform {
            // FileStore publishes the preview path before the request. Retain the
            // attempted entry so an initial failure retries that exact file.
            if let retryFile, !session.files.hasPreview {
                await session.files.open(retryFile)
            } else {
                await session.files.refresh()
            }
        }
    }
}
