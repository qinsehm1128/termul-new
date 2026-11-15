import SwiftUI

/// Host calls the hub, the switcher and the rail share, so a shell opens the
/// same way from every entry point.
extension WorkspaceSession {
    /// A shell a quick terminal owns attaches through the quick-terminal route,
    /// which hands back the claim for exactly that shell; any other shell is a
    /// project/conversation PTY and attaches as a workspace terminal.
    func openRequest(for shell: TerminalHubIndex.Shell) async throws -> TerminalOpenRequest {
        guard let quickID = shell.quickID else {
            return TerminalHubIndex.request(for: shell)
        }
        HostLog.session.info("Opening quick terminal shell from hub")
        let opened = try await quickTerminals.open(id: quickID, cols: 80, rows: 24, terminalId: shell.id)
        return TerminalOpenRequest(
            attach: .standalone(terminalID: opened.terminalId, claim: QuickTerminalPresentation.attachClaim(opened)),
            title: shell.quickTitle ?? shell.title,
            cwd: opened.record.cwd,
            projectID: shell.projectID,
            projectName: shell.projectName
        )
    }

    /// Opens (and, when it has none, starts) a quick terminal's shell.
    func openRequest(forQuick quick: TerminalHubIndex.Quick) async throws -> TerminalOpenRequest {
        HostLog.session.info("Opening quick terminal from hub")
        let opened = try await quickTerminals.open(id: quick.id, cols: 80, rows: 24)
        return TerminalOpenRequest(
            attach: .standalone(terminalID: opened.terminalId, claim: QuickTerminalPresentation.attachClaim(opened)),
            title: quick.title,
            cwd: opened.record.cwd,
            projectID: quick.projectID
        )
    }
}

/// One-step new terminal: search or pick a project and the shell starts.
///
/// "Save as quick" creates a quick terminal at the project root and starts
/// its first shell in the same tap. The host only starts project-less shells
/// through a quick terminal, so the workspace row always saves one.
struct NewTerminalSheet: View {
    @Bindable var session: WorkspaceSession
    var allowsCreate: Bool
    var preselectedProjectID: String?
    var onOpen: (TerminalOpenRequest) -> Void
    var onClose: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    @State private var query = ""
    @State private var saveAsQuick = false
    @State private var startingID: String?
    @State private var errorMessage: String?
    @FocusState private var searchFocused: Bool

    private static let workspaceRowID = "__workspace__"

    private var recentIDs: [String] { TerminalRecentProjects.load(origin: session.origin) }

    private var runningByProject: [String: Int] {
        Dictionary(session.projects.projects.map { ($0.id, $0.liveTerminalCount) }, uniquingKeysWith: { first, _ in first })
    }

    private var sections: [NewTerminalCatalog.Section] {
        NewTerminalCatalog.sections(
            projects: session.projects.projects,
            groups: session.projects.groups,
            recentProjectIDs: recentIDs,
            runningByProject: runningByProject,
            query: query
        )
    }

    private var showsWorkspace: Bool {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return needle.isEmpty || TerminalHubIndex.workspaceName.lowercased().contains(needle)
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            searchField
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            if let errorMessage {
                NocturneNotice(message: errorMessage, isError: true, onDismiss: { self.errorMessage = nil })
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
            List {
                if showsWorkspace {
                    workspaceRow
                }
                ForEach(sections) { section in
                    Section {
                        ForEach(section.items) { item in
                            projectRow(item)
                        }
                    } header: {
                        if !section.title.isEmpty {
                            Text(verbatim: section.title)
                                .font(SeNocturneType.font(13, weight: .semibold))
                                .foregroundStyle(palette.inkSecondary)
                                .textCase(nil)
                        }
                    }
                }
                if sections.isEmpty, !showsWorkspace {
                    Text(verbatim: hubText("No matching projects"))
                        .font(SeNocturneType.font(15))
                        .foregroundStyle(palette.inkSecondary)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 24)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .scrollDismissesKeyboard(.interactively)
            quickToggle
        }
        .background(mode.sheetSurface.ignoresSafeArea())
        .disabled(startingID != nil)
        .task {
            if let preselectedProjectID,
               let project = session.projects.projects.first(where: { $0.id == preselectedProjectID }) {
                await start(project: project)
            }
        }
    }

    private var topBar: some View {
        HStack {
            Text(verbatim: hubText("New terminal"))
                .font(SeNocturneType.font(17, weight: .semibold))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 0)
            Button(action: onClose) {
                SeIconView(icon: .close, size: 14, stroke: 1.7, color: palette.inkSecondary)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(SePressable())
            .accessibilityLabel(Text(hubText("Close")))
        }
        .padding(.leading, 16)
        .padding(.trailing, 4)
        .padding(.top, 8)
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            SeIconView(icon: .search, size: 16, stroke: 1.6, color: palette.inkTertiary)
            TextField(text: $query) {
                Text(verbatim: hubText("Search projects"))
            }
            .font(SeNocturneType.font(15))
            .foregroundStyle(palette.ink)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .focused($searchFocused)
        }
        .padding(.horizontal, 12)
        .frame(height: 44)
        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(searchFocused ? palette.accent.opacity(0.6) : mode.insetStroke, lineWidth: 1)
        }
    }

    private var workspaceRow: some View {
        Button {
            Task { await startWorkspace() }
        } label: {
            rowLabel(
                icon: .hosts,
                title: TerminalHubIndex.workspaceName,
                meta: hubText("No project"),
                running: 0,
                busy: startingID == Self.workspaceRowID
            )
        }
        .buttonStyle(SePressable())
        .listRowBackground(Color.clear)
        .disabled(!allowsCreate)
    }

    private func projectRow(_ item: NewTerminalCatalog.Item) -> some View {
        Button {
            guard let project = session.projects.projects.first(where: { $0.id == item.id }) else { return }
            Task { await start(project: project) }
        } label: {
            rowLabel(
                icon: .folder,
                title: item.name,
                meta: [query.isEmpty ? nil : item.groupName, TerminalHubIndex.displayPath(item.path)]
                    .compactMap { $0 }.joined(separator: "  "),
                running: item.runningCount,
                busy: startingID == item.id
            )
        }
        .buttonStyle(SePressable())
        .listRowBackground(Color.clear)
        .disabled(!allowsCreate)
    }

    private func rowLabel(icon: SeIcon, title: String, meta: String, running: Int, busy: Bool) -> some View {
        HStack(spacing: 12) {
            SeIconView(icon: icon, size: 16, stroke: 1.6, color: palette.inkSecondary)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: title)
                    .font(SeNocturneType.font(15, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                if !meta.isEmpty {
                    Text(verbatim: meta)
                        .font(SeNocturneType.font(12, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
            Spacer(minLength: 8)
            if busy {
                ProgressView().controlSize(.small)
            } else {
                if running > 0 {
                    Text(verbatim: hubText("\(running) running"))
                        .font(SeNocturneType.font(12, mono: true))
                        .foregroundStyle(palette.live)
                }
                SeIconView(icon: .chevron, size: 12, stroke: 1.6, color: palette.inkTertiary)
            }
        }
        .frame(minHeight: 52)
        .contentShape(Rectangle())
    }

    private var quickToggle: some View {
        Toggle(isOn: $saveAsQuick) {
            HStack(spacing: 8) {
                SeIconView(icon: .bolt, size: 14, stroke: 1.6, color: palette.inkSecondary)
                Text(verbatim: hubText("Save as quick terminal"))
                    .font(SeNocturneType.font(15))
                    .foregroundStyle(palette.ink)
            }
        }
        .tint(palette.accent)
        .padding(.horizontal, 16)
        .frame(minHeight: 56)
        .background(mode.barSurface)
        .overlay(alignment: .top) {
            Rectangle().fill(mode.hairline).frame(height: 1)
        }
    }

    // MARK: - Start

    private func start(project: HostProject) async {
        guard startingID == nil else { return }
        startingID = project.id
        errorMessage = nil
        defer { startingID = nil }
        TerminalRecentProjects.record(project.id, origin: session.origin)
        do {
            let request: TerminalOpenRequest
            if saveAsQuick {
                guard let root = project.path?.trimmingCharacters(in: .whitespacesAndNewlines), !root.isEmpty else {
                    errorMessage = hubText("This project has no host folder, so it cannot be saved as a quick terminal.")
                    return
                }
                HostLog.session.info("Creating quick terminal at project root from new-terminal sheet")
                let record = try await session.quickTerminals.create(
                    target: .projectRoot(projectId: project.id, projectRoot: root),
                    title: project.name
                )
                let opened = try await session.quickTerminals.open(id: record.id, cols: 80, rows: 24)
                request = TerminalOpenRequest(
                    attach: .standalone(terminalID: opened.terminalId, claim: QuickTerminalPresentation.attachClaim(opened)),
                    title: QuickTerminalPresentation.title(opened.record),
                    cwd: opened.record.cwd,
                    projectID: project.id,
                    projectName: project.name
                )
            } else {
                HostLog.session.info("Starting project terminal from new-terminal sheet")
                await session.selectProject(project)
                await session.spawnTerminal()
                guard let terminalID = session.terminals.activeId else {
                    errorMessage = session.terminals.errorMessage ?? hubText("The host did not start a shell.")
                    HostLog.session.error("New terminal sheet: host did not start a shell")
                    return
                }
                let terminal = session.terminals.terminals.first(where: { $0.id == terminalID })
                request = TerminalOpenRequest(
                    attach: .workspace(terminalID: terminalID),
                    title: terminal?.title ?? hubText("Terminal"),
                    cwd: terminal?.cwd ?? project.path,
                    projectID: project.id,
                    projectName: project.name
                )
            }
            onOpen(request)
        } catch {
            errorMessage = error.localizedDescription
            HostLog.session.error("New terminal sheet start failed")
        }
    }

    private func startWorkspace() async {
        guard startingID == nil else { return }
        startingID = Self.workspaceRowID
        errorMessage = nil
        defer { startingID = nil }
        do {
            HostLog.session.info("Starting workspace quick terminal from new-terminal sheet")
            let record = try await session.quickTerminals.create(
                target: .workspace,
                title: saveAsQuick ? nil : TerminalHubIndex.workspaceName
            )
            let opened = try await session.quickTerminals.open(id: record.id, cols: 80, rows: 24)
            onOpen(TerminalOpenRequest(
                attach: .standalone(terminalID: opened.terminalId, claim: QuickTerminalPresentation.attachClaim(opened)),
                title: QuickTerminalPresentation.title(opened.record),
                cwd: opened.record.cwd
            ))
        } catch {
            errorMessage = error.localizedDescription
            HostLog.session.error("New terminal sheet workspace start failed")
        }
    }
}
