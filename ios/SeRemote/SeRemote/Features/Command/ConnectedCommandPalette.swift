import SwiftUI

/// Live-data adapter for the item-based palette. Selection alone never creates,
/// opens, attaches, or closes a quick terminal.
struct ConnectedCommandPalette: View {
    let session: WorkspaceSession
    let onOpenSession: () -> Void

    @State private var query = ""
    @State private var presentedRoute: PresentedRoute?
    @State private var openingResult = false
    @State private var fileError: String?
    @State private var openTask: Task<Void, Never>?
    @Environment(\.sePalette) private var palette

    init(session: WorkspaceSession, onOpenSession: @escaping () -> Void) {
        self.session = session
        self.onOpenSession = onOpenSession
    }

    private enum PresentedRoute: Identifiable {
        case tool(ToolDestination)
        case quickTerminal(String)
        var id: String {
            switch self {
            case let .tool(tool): "tool:\(tool.rawValue)"
            case let .quickTerminal(id): "quick-terminal:\(id)"
            }
        }
    }

    private var entries: [CommandPaletteEntry] {
        CommandPaletteMapping.entries(
            conversations: session.conversations.conversations,
            projects: session.projects.projects,
            quickTerminals: session.quickTerminals.records,
            files: session.search.results, query: query)
    }

    var body: some View {
        CommandPaletteView(items: entries.map(\.item), query: $query) { item in
            guard let entry = entries.first(where: { $0.id == item.id }) else { return }
            select(entry.target)
        }
        .disabled(openingResult)
        .overlay(alignment: .bottom) {
            VStack(spacing: SeSpace.xs) {
                if openingResult { ProgressView("Opening file…") }
                if session.search.isLoading { ProgressView("Searching files…") }
                if let fileError { Text(fileError).foregroundStyle(palette.inkSecondary) }
                if let error = session.search.errorMessage { Text(error).foregroundStyle(palette.inkSecondary) }
            }
            .font(SeNocturneType.font(SeNocturneType.secondarySize))
            .padding(SeSpace.md)
            .background(palette.canvas)
            .accessibilityElement(children: .combine)
        }
        .task(id: session.workspaceRoot) {
            let root = session.workspaceRoot ?? ""
            session.search.configure(scopeRoot: root, rootPath: root)
            session.search.setQuery(query)
            await session.quickTerminals.refresh()
        }
        .onChange(of: query) { _, value in session.search.setQuery(value) }
        .onDisappear {
            session.search.cancelSearch()
            openTask?.cancel()
            openTask = nil
        }
        .sheet(item: $presentedRoute) { route in
            NavigationStack {
                Group {
                    switch route {
                    case let .tool(tool):
                        WorkspaceToolDestinationView(session: session, destination: tool, onOpenFile: {
                            presentedRoute = nil
                            onOpenSession()
                        })
                    case .quickTerminal:
                        WorkspaceToolDestinationView(session: session, destination: .quickTerminals)
                    }
                }
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { presentedRoute = nil }
                            .frame(minHeight: SeSpace.hitTarget)
                    }
                }
            }
            .tint(palette.ink)
        }
    }

    private func select(_ target: CommandPaletteTarget) {
        switch target {
        case let .conversation(id):
            guard let conversation = session.conversations.conversations.first(where: { $0.id == id }) else { return }
            openingResult = true
            openTask = Task {
                defer { openingResult = false }
                await session.selectConversation(conversation)
                guard !Task.isCancelled else { return }
                onOpenSession()
            }
        case let .project(id):
            guard let project = session.projects.projects.first(where: { $0.id == id }) else { return }
            openingResult = true
            openTask = Task {
                defer { openingResult = false }
                await session.selectProject(project)
                guard !Task.isCancelled else { return }
                onOpenSession()
            }
        case let .file(path): openFile(path)
        case let .quickTerminal(id): presentedRoute = .quickTerminal(id)
        case let .tool(tool): presentedRoute = .tool(tool)
        }
    }

    /// Same preview/read-before-navigation contract as SearchFeatureView.
    private func openFile(_ path: String) {
        guard !openingResult else { return }
        openingResult = true
        fileError = nil
        let root = session.workspaceRoot
        openTask = Task { @MainActor in
            defer { openingResult = false }
            let opened = await session.files.openReturningSuccess(DirectoryEntry(
                name: URL(fileURLWithPath: path).lastPathComponent, path: path, type: "file"))
            guard !Task.isCancelled, session.workspaceRoot == root else { return }
            guard opened else {
                fileError = session.files.errorMessage ?? String(localized: "The host could not open this file preview.")
                return
            }
            session.setWorkspaceTab(.files)
            onOpenSession()
        }
    }
}
