import SwiftUI

/// T1 — the terminal hub's data plumbing.
///
/// Builds the merged `TerminalHubIndex` from the live list, quick terminals,
/// live agents and the host's project groups, owns search/filter state and
/// the quick-terminal mutations, and renders `TerminalHubView`. Opening a
/// shell is forwarded: the shell scopes the session and opens the workbench,
/// this view never attaches a PTY itself.
struct TerminalsHubScreen: View {
    @Environment(\.sePalette) private var palette

    @Bindable var session: WorkspaceSession
    @Bindable var store: TerminalsHubStore
    let hostName: String
    let hostMeta: String
    let themeIcon: SeIcon
    let lastTerminalID: String?
    let allowsCreate: Bool
    var isRefreshing: Bool
    var notice: String?
    @Binding var query: TerminalHubIndex.Query
    var onRefresh: () async -> Void
    var onSelectHost: () -> Void
    var onToggleTheme: () -> Void
    var onOpenDrawer: () -> Void
    var onOpenCommand: () -> Void
    var onOpen: (TerminalOpenRequest) -> Void
    /// Forwards a notice dismissal to the shell so cross-tab notices (deep
    /// links, ⌘K misses) clear at their source, not only locally.
    var onDismissNotice: (() -> Void)? = nil

    @State private var newTerminal: NewTerminalRoute?
    @State private var renaming: QuickTerminalRecordWire?
    @State private var renameTitle = ""
    @State private var pendingDestruction: Destruction?
    @State private var busy = false
    @State private var localNotice: String?
    @State private var localError: String?

    struct NewTerminalRoute: Identifiable {
        let projectID: String?
        var id: String { projectID ?? "__any__" }
    }

    private enum Destruction: Identifiable {
        case closeQuick(QuickTerminalRecordWire)
        case deleteQuick(QuickTerminalRecordWire)
        case closeShell(QuickTerminalRecordWire, TerminalHubIndex.Shell)

        var id: String {
            switch self {
            case .closeQuick(let record): "close-\(record.id)"
            case .deleteQuick(let record): "delete-\(record.id)"
            case .closeShell(_, let shell): "shell-\(shell.id)"
            }
        }
    }

    private var index: TerminalHubIndex {
        TerminalHubIndex.make(
            summaries: store.summaries,
            projects: session.projects.projects,
            projectGroups: session.projects.groups,
            quickRecords: session.quickTerminals.records,
            agentTerminalIDs: Set(session.liveAgents.sessions.map(\.terminalId)),
            lastTerminalID: lastTerminalID,
            allowsCreate: allowsCreate,
            query: query
        )
    }

    private var displayedNotice: String? { localNotice ?? notice }
    private var displayedError: String? { localError ?? store.errorMessage }

    /// True until the first host answer lands: `idle`/`loading` with an empty
    /// list means "unknown", and must not render a "no running terminals" claim.
    static func isLoading(state: TerminalsHubStore.State, summaryCount: Int) -> Bool {
        (state == .idle || state == .loading) && summaryCount == 0
    }

    private var isLoading: Bool {
        TerminalsHubScreen.isLoading(state: store.state, summaryCount: store.summaries.count)
    }

    var body: some View {
        let index = index
        TerminalHubView(
            index: index,
            query: $query,
            selectedTerminalID: nil,
            isLoading: isLoading,
            notice: displayedNotice,
            errorMessage: displayedError,
            onDismissNotice: {
                localNotice = nil
                onDismissNotice?()
            },
            onDismissError: { localError = nil },
            actions: actions
        ) {
            header(index)
        }
        .sheet(item: $newTerminal) { route in
            NewTerminalSheet(
                session: session,
                allowsCreate: allowsCreate,
                preselectedProjectID: route.projectID,
                onOpen: { request in
                    newTerminal = nil
                    onOpen(request)
                    Task { await onRefresh() }
                },
                onClose: { newTerminal = nil }
            )
            .presentationDetents(route.projectID == nil ? [.large, .medium] : [.medium])
            .presentationDragIndicator(.visible)
        }
        .alert(hubText("Rename quick terminal"), isPresented: renameBinding) {
            TextField(hubText("Title"), text: $renameTitle)
            Button(hubText("Cancel"), role: .cancel) { renaming = nil }
            Button(hubText("Rename")) { rename() }
        }
        .confirmationDialog(
            pendingDestruction.map(destructionTitle) ?? "",
            isPresented: destructionBinding,
            titleVisibility: .visible,
            presenting: pendingDestruction
        ) { item in
            Button(destructionAction(item), role: .destructive) { Task { await destroy(item) } }
            Button(hubText("Cancel"), role: .cancel) { pendingDestruction = nil }
        } message: { item in
            Text(verbatim: destructionMessage(item))
        }
    }

    private func header(_ index: TerminalHubIndex) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            SeHostSwitch(
                title: hostName,
                meta: hostMeta,
                onHost: onSelectHost,
                onTheme: onToggleTheme,
                themeIcon: themeIcon,
                onRail: onOpenDrawer,
                onCommand: onOpenCommand
            )
            .padding(.horizontal, -18)
            .padding(.top, 8)

            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(verbatim: hubText("Terminals"))
                    .font(SeNocturneType.font(28, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .accessibilityAddTraits(.isHeader)
                Spacer(minLength: 0)
                if isRefreshing {
                    ProgressView().controlSize(.small)
                }
                if index.totalRunning > 0 {
                    HStack(spacing: 6) {
                        Circle().fill(palette.live).frame(width: 8, height: 8)
                        Text(verbatim: "\(index.totalRunning)")
                            .font(SeNocturneType.font(13, mono: true))
                            .foregroundStyle(palette.inkSecondary)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(Text(hubText("\(index.totalRunning) running")))
                }
            }
        }
    }

    // MARK: - Actions

    private var actions: TerminalHubActions {
        TerminalHubActions(
            openShell: { shell in open { try await session.openRequest(for: shell) } },
            openQuick: { quick in open { try await session.openRequest(forQuick: quick) } },
            newTerminal: { newTerminal = NewTerminalRoute(projectID: nil) },
            newTerminalInProject: { id in newTerminal = NewTerminalRoute(projectID: id) },
            renameQuick: beginRename,
            closeQuick: { id in
                guard let record = record(id) else { return }
                pendingDestruction = .closeQuick(record)
            },
            deleteQuick: { id in
                guard let record = record(id) else { return }
                pendingDestruction = .deleteQuick(record)
            },
            closeShell: { shell in
                guard let quickID = shell.quickID, let record = record(quickID) else { return }
                pendingDestruction = .closeShell(record, shell)
            },
            refresh: onRefresh
        )
    }

    private func open(_ make: @escaping () async throws -> TerminalOpenRequest) {
        guard !busy else { return }
        busy = true
        localError = nil
        // `busy` stays set until the host answers, so a double tap cannot open
        // two shells.
        Task {
            defer { busy = false }
            do {
                onOpen(try await make())
            } catch {
                localError = error.localizedDescription
                HostLog.session.error("Terminal hub open failed")
            }
        }
    }

    private func record(_ id: String) -> QuickTerminalRecordWire? {
        session.quickTerminals.records.first(where: { $0.id == id })
    }

    private func beginRename(_ id: String) {
        guard let record = record(id) else { return }
        renameTitle = record.title ?? ""
        renaming = record
    }

    private func rename() {
        guard let record = renaming else { return }
        renaming = nil
        Task {
            busy = true
            localError = nil
            defer { busy = false }
            do {
                let title = renameTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                try await session.quickTerminals.rename(id: record.id, title: title.isEmpty ? nil : title)
                await onRefresh()
            } catch {
                localError = error.localizedDescription
            }
        }
    }

    private func destroy(_ item: Destruction) async {
        pendingDestruction = nil
        busy = true
        localError = nil
        defer { busy = false }
        do {
            switch item {
            case .closeQuick(let record):
                try await session.quickTerminals.close(id: record.id)
            case .deleteQuick(let record):
                try await session.quickTerminals.delete(id: record.id)
            case .closeShell(let record, let shell):
                try await session.quickTerminals.closeTerminal(id: record.id, terminalId: shell.id)
            }
            await onRefresh()
        } catch {
            localError = error.localizedDescription
            HostLog.session.error("Terminal hub destructive action failed")
        }
    }

    private var renameBinding: Binding<Bool> {
        Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
    }

    private var destructionBinding: Binding<Bool> {
        Binding(get: { pendingDestruction != nil }, set: { if !$0 { pendingDestruction = nil } })
    }

    private func destructionTitle(_ item: Destruction) -> String {
        switch item {
        case .closeQuick: hubText("Close its running shells?")
        case .deleteQuick: hubText("Delete this quick terminal?")
        case .closeShell: hubText("Close this shell?")
        }
    }

    private func destructionAction(_ item: Destruction) -> String {
        switch item {
        case .closeQuick: hubText("Close shells")
        case .deleteQuick: hubText("Delete")
        case .closeShell: hubText("Close shell")
        }
    }

    private func destructionMessage(_ item: Destruction) -> String {
        switch item {
        case .closeQuick(let record):
            hubText("End all running shells for \(QuickTerminalPresentation.title(record))? Unsaved work may be lost. The shortcut and folder are kept.")
        case .deleteQuick(let record):
            hubText("Delete \(QuickTerminalPresentation.title(record)) and end all its running shells? The host folder is kept.")
        case .closeShell(_, let shell):
            hubText("End \(shell.title) on the host? Unsaved work may be lost.")
        }
    }
}
