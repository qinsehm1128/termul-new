import SwiftUI

/// T1 — the terminal directory's data plumbing.
///
/// Owns the live-terminal list (15 s poll, foreground refresh) and the quick
/// terminal mutations, then renders `TerminalLandingScreen` with a pure model.
/// Opening a row is forwarded: the shell scopes the session and opens the
/// workbench, this view never attaches a PTY itself.
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
    var onRefresh: () -> Void
    var onSelectHost: () -> Void
    var onToggleTheme: () -> Void
    var onOpenDrawer: () -> Void
    var onOpenCommand: () -> Void
    var onOpen: (TerminalOpenRequest) -> Void

    @State private var showsProjectPicker = false
    @State private var spawning = false
    @State private var showsCreateQuick = false
    @State private var quickTitle = ""
    @State private var renaming: QuickTerminalRecordWire?
    @State private var renameTitle = ""
    @State private var pendingDestruction: Destruction?
    @State private var busy = false
    @State private var localNotice: String?
    @State private var localError: String?

    private struct Destruction: Identifiable {
        let record: QuickTerminalRecordWire
        let deleting: Bool
        var id: String { "\(record.id)-\(deleting)" }
    }

    private var model: TerminalLandingModel {
        let agentIDs = Set(session.liveAgents.sessions.map(\.terminalId))
        return TerminalLandingModel.make(
            summaries: store.summaries,
            projects: session.projects.projects,
            quickRecords: session.quickTerminals.records,
            runningAgentTerminalIDs: agentIDs,
            lastTerminalID: lastTerminalID,
            allowsCreate: allowsCreate
        )
    }

    private var creatableProjects: [HostProject] {
        session.projects.projects.filter { !$0.isArchived }
    }

    private var displayedNotice: String? { localNotice ?? notice }
    private var displayedError: String? { localError ?? store.errorMessage }

    var body: some View {
        TerminalLandingScreen(
            hostName: hostName,
            hostMeta: hostMeta,
            model: model,
            isRefreshing: isRefreshing,
            notice: displayedNotice,
            errorMessage: displayedError,
            themeIcon: themeIcon,
            onRefresh: onRefresh,
            onSelectHost: onSelectHost,
            onToggleTheme: onToggleTheme,
            onOpenDrawer: onOpenDrawer,
            onOpenCommand: onOpenCommand,
            onOpen: { row in onOpen(TerminalLandingModel.request(for: row)) },
            onOpenQuick: openQuick,
            onRenameQuick: beginRename,
            onCloseQuick: { row in
                guard let record = record(for: row) else { return }
                pendingDestruction = Destruction(record: record, deleting: false)
            },
            onDeleteQuick: { row in
                guard let record = record(for: row) else { return }
                pendingDestruction = Destruction(record: record, deleting: true)
            },
            onNewTerminal: { showsProjectPicker = true },
            onNewQuickTerminal: {
                quickTitle = ""
                localError = nil
                showsCreateQuick = true
            }
        )
        .sheet(isPresented: $showsProjectPicker) { projectPicker }
        .sheet(isPresented: $showsCreateQuick) { createQuickSheet }
        .alert(String(localized: "Rename quick terminal"), isPresented: renameBinding) {
            TextField(String(localized: "Title"), text: $renameTitle)
            Button(String(localized: "Cancel"), role: .cancel) { renaming = nil }
            Button(String(localized: "Rename")) { rename() }
        } message: {
            Text("Leave the title empty to clear it and use the fallback title.")
        }
        .confirmationDialog(
            pendingDestruction.map(destructionTitle) ?? "",
            isPresented: destructionBinding,
            titleVisibility: .visible,
            presenting: pendingDestruction
        ) { item in
            Button(
                item.deleting ? String(localized: "Delete") : String(localized: "Close shells"),
                role: .destructive
            ) { Task { await destroy(item) } }
            Button(String(localized: "Cancel"), role: .cancel) { pendingDestruction = nil }
        } message: { item in
            Text(item.deleting
                 ? String(localized: "Delete \(QuickTerminalPresentation.title(item.record)) and end all its running shells? The host folder is kept.")
                 : String(localized: "End all running shells for \(QuickTerminalPresentation.title(item.record))? Unsaved work may be lost. The shortcut and folder are kept."))
        }
    }

    // MARK: - Project picker

    private var projectPicker: some View {
        NavigationStack {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: NocturneToolMetrics.cardGap) {
                    if creatableProjects.isEmpty {
                        Text("This host has no projects yet.")
                            .font(SeNocturneType.font(11))
                            .foregroundStyle(palette.inkSecondary)
                            .padding(.vertical, 18)
                    }
                    ForEach(creatableProjects) { project in
                        Button {
                            Task { await start(in: project) }
                        } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(project.name)
                                    .font(SeNocturneType.font(12, weight: .semibold))
                                    .foregroundStyle(palette.ink)
                                Text(project.path ?? String(localized: "Project"))
                                    .font(SeNocturneType.font(9, mono: true))
                                    .foregroundStyle(palette.inkTertiary)
                                    .lineLimit(1)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12)
                            .frame(minHeight: NocturneToolMetrics.minimumTarget)
                            .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .overlay {
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .strokeBorder(palette.stroke, lineWidth: 1)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(SePressable())
                        .disabled(spawning)
                    }
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.vertical, 18)
            }
            .background(palette.canvas)
            .navigationTitle(String(localized: "New Terminal"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { showsProjectPicker = false }
                        .frame(minHeight: SeSpace.hitTarget)
                }
            }
            .overlay {
                if spawning {
                    ProgressView(String(localized: "Starting shell…"))
                        .padding(18)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func start(in project: HostProject) async {
        guard allowsCreate, !spawning else { return }
        spawning = true
        localError = nil
        defer { spawning = false }
        await session.selectProject(project)
        await session.spawnTerminal()
        showsProjectPicker = false
        guard let terminalID = session.terminals.activeId else {
            localError = session.terminals.errorMessage ?? String(localized: "The host did not start a shell.")
            return
        }
        let terminal = session.terminals.terminals.first(where: { $0.id == terminalID })
        onOpen(TerminalOpenRequest(
            attach: .workspace(terminalID: terminalID),
            title: terminal?.title ?? String(localized: "Terminal"),
            cwd: terminal?.cwd ?? project.path,
            projectID: terminal?.projectId ?? project.id,
            projectName: project.name
        ))
    }

    // MARK: - Quick terminals

    private var createQuickSheet: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                Text("New quick terminal")
                    .font(SeNocturneType.font(19, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Text("The host opens a shell in its workspace folder. Creating the shortcut does not start one.")
                    .font(SeNocturneType.font(11))
                    .foregroundStyle(palette.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                TextField(String(localized: "Title"), text: $quickTitle)
                    .font(SeNocturneType.font(12))
                    .padding(12)
                    .frame(minHeight: NocturneToolMetrics.minimumTarget)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                if let localError {
                    NocturneNotice(message: localError, isError: true)
                }
                Spacer(minLength: 0)
                HStack(spacing: 8) {
                    Button(String(localized: "Cancel")) { showsCreateQuick = false }
                        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
                        .foregroundStyle(palette.inkSecondary)
                    Button(String(localized: "Create")) { Task { await createQuick() } }
                        .font(SeNocturneType.font(12, weight: .semibold))
                        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
                        .foregroundStyle(palette.onAccent)
                        .background(palette.ink, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
                        .disabled(busy)
                }
            }
            .padding(NocturneToolMetrics.gutter)
            .background(palette.canvas)
            .toolbar(.hidden, for: .navigationBar)
        }
        .presentationDetents([.height(300), .medium])
        .presentationDragIndicator(.hidden)
    }

    private func createQuick() async {
        guard allowsCreate, !busy else { return }
        busy = true
        localError = nil
        defer { busy = false }
        do {
            let title = quickTitle.trimmingCharacters(in: .whitespacesAndNewlines)
            _ = try await session.quickTerminals.create(
                target: .workspace,
                title: title.isEmpty ? nil : title
            )
            showsCreateQuick = false
            localNotice = String(localized: "Quick terminal created. Open it to start a shell on the host.")
        } catch {
            localError = error.localizedDescription
        }
    }

    private func openQuick(_ row: TerminalLandingModel.QuickRow) {
        guard row.canOpen, !busy else { return }
        busy = true
        localError = nil
        defer { busy = false }
        Task {
            do {
                let opened = try await session.quickTerminals.open(id: row.id, cols: 80, rows: 24)
                let claim = QuickTerminalPresentation.attachClaim(opened)
                onOpen(TerminalOpenRequest(
                    attach: .standalone(terminalID: opened.terminalId, claim: claim),
                    title: row.title,
                    cwd: opened.record.cwd
                ))
            } catch {
                localError = error.localizedDescription
            }
        }
    }

    private func record(for row: TerminalLandingModel.QuickRow) -> QuickTerminalRecordWire? {
        session.quickTerminals.records.first(where: { $0.id == row.id })
    }

    private func beginRename(_ row: TerminalLandingModel.QuickRow) {
        guard let record = record(for: row) else { return }
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
                localNotice = String(localized: "Quick terminal renamed.")
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
            if item.deleting {
                try await session.quickTerminals.delete(id: item.record.id)
                localNotice = String(localized: "Quick terminal deleted. Its host folder was kept.")
            } else {
                try await session.quickTerminals.close(id: item.record.id)
                localNotice = String(localized: "All terminals for this shortcut were closed.")
            }
        } catch {
            localError = error.localizedDescription
        }
    }

    private var renameBinding: Binding<Bool> {
        Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
    }

    private var destructionBinding: Binding<Bool> {
        Binding(get: { pendingDestruction != nil }, set: { if !$0 { pendingDestruction = nil } })
    }

    private func destructionTitle(_ item: Destruction) -> String {
        item.deleting
            ? String(localized: "Delete this quick terminal?")
            : String(localized: "Close its running shells?")
    }
}
