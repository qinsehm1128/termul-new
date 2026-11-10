import SwiftUI

/// Breadcrumb + full-height listing shared by the iPhone file panel and the
/// iPad file inspector. Browsing, refresh and the store stay owned by
/// `FileStore`; taps on files are reported through `onOpenFile` so the caller
/// can present the full-screen preview cover.
@MainActor
struct TerminalFileBrowser: View {
    @Environment(\.sePalette) private var palette

    let session: WorkspaceSession
    let rootPath: String
    let onOpenFile: (String) -> Void
    /// The panel shows a close button next to its breadcrumb; the inspector
    /// column has none.
    var onClose: (() -> Void)?

    @State private var query = ""
    @State private var selectedPath: String?
    @State private var listScrollPath: String?
    @State private var isRequesting = false

    private var busy: Bool { isRequesting || session.files.isLoading }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let message = session.files.errorMessage {
                SeErrorBanner(message: message) {
                    Task { await refresh() }
                }
                .disabled(busy)
                .padding(.horizontal, 18)
                .padding(.top, 8)
                .accessibilityIdentifier("files-panel-error")
            }
            WorkspaceFileListView(
                entries: session.files.entries,
                query: $query,
                scrollPath: $listScrollPath,
                selectedPath: selectedPath,
                isLoading: busy,
                hasError: session.files.errorMessage != nil,
                open: { entry in Task { await open(entry) } },
                refresh: { await refresh() }
            )
            .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task(id: rootPath) { await loadRoot() }
    }

    // MARK: - Breadcrumb

    private var header: some View {
        HStack(spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 5) {
                    if session.files.crumbs.isEmpty {
                        Text(FilePanelPresentation.title(rootPath: rootPath))
                            .foregroundStyle(palette.inkTertiary)
                    }
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
                                .foregroundStyle(isCurrent(index) ? palette.ink : palette.inkTertiary)
                                .frame(minHeight: SeSpace.hitTarget)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(busy || isCurrent(index))
                        .accessibilityHint(Text("Open this folder"))
                    }
                }
            }
            .font(.system(size: 10, design: .monospaced))
            .accessibilityIdentifier("files-panel-crumbs")

            if let onClose {
                SeIconButton(
                    icon: .close,
                    accessibilityLabel: String(localized: "Close"),
                    size: 34,
                    action: onClose
                )
                .accessibilityIdentifier("files-panel-close")
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 6)
    }

    private func isCurrent(_ index: Int) -> Bool {
        index == session.files.crumbs.count - 1
    }

    // MARK: - Loading and navigation

    private func loadRoot() async {
        guard FilePanelPresentation.shouldReloadRoot(
            currentRoot: session.files.currentRootPath,
            requestedRoot: rootPath
        ) else { return }
        query = ""
        selectedPath = nil
        listScrollPath = nil
        isRequesting = true
        defer { isRequesting = false }
        await session.files.openRootIfNeeded(rootPath)
    }

    private func open(_ entry: DirectoryEntry) async {
        guard !busy else { return }
        if entry.isDirectory {
            query = ""
            selectedPath = nil
            listScrollPath = nil
            await perform { await session.files.openReturningSuccess(entry) }
        } else {
            selectedPath = entry.path
            await perform {
                // The store publishes the preview identity before the request,
                // so the cover can render loading and failure states.
                _ = await session.files.openReturningSuccess(entry)
                onOpenFile(entry.path)
            }
        }
    }

    private func popTo(_ crumb: DirectoryEntry) async {
        guard crumb.path != session.files.crumbs.last?.path else { return }
        query = ""
        selectedPath = nil
        listScrollPath = nil
        await perform { await session.files.popTo(crumb) }
    }

    private func refresh() async {
        await perform { await session.files.refresh() }
    }

    private func perform(_ action: @MainActor () async -> Void) async {
        guard !busy else { return }
        isRequesting = true
        session.files.errorMessage = nil
        defer { isRequesting = false }
        await action()
    }
}

/// Terminal file panel sheet: breadcrumb rooted at the terminal cwd and a
/// listing that fills the sheet. Tapping a directory navigates; tapping a file
/// reports `onOpenFile` so the caller can present the full-screen preview.
@MainActor
struct TerminalFilePanel: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    let session: WorkspaceSession
    let rootPath: String
    let terminalTitle: String
    let onOpenFile: (String) -> Void
    let onClose: () -> Void

    /// `agentAvailable` is part of the shared workbench initializer contract.
    /// The panel has no Agent action of its own; the preview cover owns it.
    init(
        session: WorkspaceSession,
        rootPath: String,
        terminalTitle: String,
        agentAvailable: Bool,
        onOpenFile: @escaping (String) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.session = session
        self.rootPath = rootPath
        self.terminalTitle = terminalTitle
        self.onOpenFile = onOpenFile
        self.onClose = onClose
    }

    var body: some View {
        VStack(spacing: 0) {
            Capsule()
                .fill(palette.inkTertiary.opacity(0.35))
                .frame(width: 36, height: 4)
                .frame(maxWidth: .infinity)
                .padding(.top, 10)
                .padding(.bottom, 6)
                .accessibilityHidden(true)
            TerminalFileBrowser(
                session: session,
                rootPath: rootPath,
                onOpenFile: onOpenFile,
                onClose: onClose
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(mode.sheetSurface)
        .foregroundStyle(palette.ink)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Files in \(terminalTitle)"))
        .accessibilityIdentifier("files-panel")
    }
}
