import SwiftUI

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
                        .navigationTitle(session.files.previewName ?? session.files.crumbs.last?.name ?? String(localized: "Files"))
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

    private var browser: some View {
        GeometryReader { geometry in
            let split = SeLayout.columns(
                width: geometry.size.width,
                isRegular: sizeClass == .regular,
                largeText: dynamicTypeSize.isAccessibilitySize
            ) != .single
            VStack(spacing: 0) {
                crumbs
                if let message = session.files.errorMessage {
                    SeErrorBanner(message: message) {
                        Task { await refresh() }
                    }
                    .disabled(busy)
                    .padding(SeSpace.sm)
                    .accessibilityIdentifier("files-error")
                }
                if split {
                    HStack(spacing: 0) {
                        fileList
                            .frame(width: min(SeLayout.sidebarWidth.upperBound, geometry.size.width * 0.4))
                        Rectangle().fill(palette.strokeStrong).frame(width: 1)
                        preview
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                } else if session.files.preview != nil {
                    preview
                } else {
                    fileList
                }
                desktopOnlyFooter
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.canvas)
        }
    }

    private var crumbs: some View {
        HStack(spacing: SeSpace.xs) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: SeSpace.xxs) {
                    ForEach(Array(session.files.crumbs.enumerated()), id: \.element.id) { index, crumb in
                        if index > 0 {
                            Text("/")
                                .font(.caption.monospaced())
                                .foregroundStyle(palette.inkTertiary)
                                .accessibilityHidden(true)
                        }
                        Button {
                            Task { await popTo(crumb) }
                        } label: {
                            HStack(spacing: SeSpace.xs) {
                                Text(crumb.name).lineLimit(1)
                            }
                            .font(.caption.monospaced())
                            .foregroundStyle(palette.inkSecondary)
                            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                        }
                        .buttonStyle(SePressableRowStyle())
                        .disabled(busy || (index == session.files.crumbs.count - 1 && session.files.preview == nil && session.files.errorMessage == nil))
                        .accessibilityHint(Text("Open this folder"))
                    }
                }
            }
            Button {
                Task { await refresh() }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .labelStyle(.iconOnly)
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            }
            .disabled(busy || session.files.crumbs.isEmpty)
            .accessibilityIdentifier("files-refresh")
        }
        .padding(.horizontal, SeSpace.screenInset)
        .overlay(alignment: .bottom) { Rectangle().fill(palette.strokeStrong).frame(height: 1) }
        .accessibilityIdentifier("files-crumbs")
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
    private var preview: some View {
        if let content = session.files.preview {
            WorkspaceFilePreviewView(
                name: session.files.previewName ?? String(localized: "File preview"),
                content: content,
                isLoading: busy,
                refresh: { await refresh() }
            )
        } else {
            SeEmptyState(
                title: String(localized: "Select a file"),
                systemImage: "doc.text",
                message: String(localized: "Choose a file to view its read-only contents.")
            )
        }
    }

    /// Paired phones have no fs mutation routes. Keep this boundary visible in previews too.
    private var desktopOnlyFooter: some View {
        Label("File editing is desktop-only. Changes need the host's local web client.", systemImage: "lock.shield")
            .font(SeFont.caption)
            .foregroundStyle(palette.inkSecondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, SeSpace.screenInset)
            .padding(.vertical, SeSpace.sm)
            .background(palette.card)
            .overlay(alignment: .top) { Rectangle().fill(palette.strokeStrong).frame(height: 1) }
            .accessibilityIdentifier("files-desktop-only")
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
            // FileStore only records a preview path after a successful read. Retain
            // the attempted entry so an initial read failure retries that exact file.
            if let retryFile, session.files.preview == nil {
                await session.files.open(retryFile)
            } else {
                await session.files.refresh()
            }
        }
    }
}
