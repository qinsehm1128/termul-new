import SwiftUI
import UIKit

/// Host quick terminals have their own PTY scope, independent of this session.
struct QuickTerminalsFeatureView: View {
    @Environment(\.sePalette) private var p
    var session: WorkspaceSession
    @State private var busy = false
    @State private var errorMessage: String?
    @State private var notice: String?
    @State private var openedTerminalId: String?
    @State private var showingCreate = false
    @State private var draftTitle = ""
    @State private var useProject = false
    @State private var renaming: QuickTerminalRecordWire?
    @State private var renameTitle = ""
    @State private var confirming: PendingDestruction?
    @State private var successTick = 0
    @State private var failureTick = 0

    private struct PendingDestruction: Identifiable {
        let record: QuickTerminalRecordWire
        let deleting: Bool
        var id: String { record.id }
    }

    private var creationTargets: [QuickTerminalTargetWire] {
        QuickTerminalPresentation.creationTargets(
            isProjectSession: session.workspace == .project,
            projectId: session.projects.active?.id,
            projectRoot: session.projects.active?.path
        )
    }

    var body: some View {
        List {
            Section {
                Label(session.origin.host() ?? String(localized: "Connected host"), systemImage: "server.rack")
                    .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                Text("Quick terminals run on the host. They are separate from this workspace's terminal tabs.")
                    .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                Button("New quick terminal", systemImage: "plus") {
                    draftTitle = ""
                    useProject = false
                    showingCreate = true
                }
                .frame(minHeight: 44).disabled(busy)
            }
            if busy { ToolNocturneProgress(String(localized: "Waiting for host…")).accessibilityLabel("Waiting for host") }
            if let error = displayedError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(p.danger).toolNocturneFont(.support)
                }
            }
            if let notice {
                Section {
                    Label(notice, systemImage: "checkmark.circle").toolNocturneFont(.support)
                }
            }
            if let openedTerminalId {
                Section("Host terminal") {
                    Text("The host returned this terminal ID. It is not attached here: quick terminals use a separate host scope, which this workspace cannot safely retain. Use the host to interact with it, or close it below.")
                        .toolNocturneFont(.support)
                    Text(openedTerminalId).toolNocturneFont(.code)
                        .textSelection(.enabled)
                    Button("Copy terminal ID", systemImage: "doc.on.doc") {
                        UIPasteboard.general.string = openedTerminalId
                        self.notice = String(localized: "Terminal ID copied.")
                        successTick += 1
                    }
                    .frame(minHeight: 44)
                }
            }
            Section("Quick terminals") {
                if session.quickTerminals.records.isEmpty {
                    if case .loading = session.quickTerminals.state {
                        ToolNocturneProgress(String(localized: "Loading quick terminals…"))
                    } else {
                        ToolNocturneEmptyState(title: String(localized: "No quick terminals"), systemImage: "terminal",
                            message: String(localized: "Create a persistent host shortcut in a workspace folder or the active project root."))
                    }
                } else {
                    ForEach(session.quickTerminals.records) { record in
                        recordRow(record)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(p.canvas)
        .foregroundStyle(p.ink)
        .toolNocturneFont(.body)
        .tint(p.accent)
        .navigationTitle("Quick terminals")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Refresh", systemImage: "arrow.clockwise") { Task { await refresh() } }
                    .frame(minWidth: 44, minHeight: 44).disabled(busy)
            }
        }
        .refreshable { await refresh() }
        .task { if case .idle = session.quickTerminals.state { await refresh() } }
        .sheet(isPresented: $showingCreate) { creationSheet }
        .alert("Rename quick terminal", isPresented: Binding(
            get: { renaming != nil }, set: { if !$0 { renaming = nil } }
        )) {
            TextField("Title", text: $renameTitle)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Save") {
                guard let record = renaming else { return }
                let title = renameTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                renaming = nil
                Task { await perform {
                    try await session.quickTerminals.rename(id: record.id, title: title.isEmpty ? nil : title)
                    notice = String(localized: "Quick terminal renamed.")
                } }
            }
        } message: { Text("Save the new title on the host. An empty title restores the default name.") }
        .confirmationDialog("Confirm host action", isPresented: Binding(
            get: { confirming != nil }, set: { if !$0 { confirming = nil } }
        ), presenting: confirming) { pending in
            Button(pending.deleting ? "Delete quick terminal" : "Close all terminals", role: .destructive) {
                Task { await destroy(pending) }
            }
        } message: { pending in
            Text(pending.deleting
                ? "Delete \(QuickTerminalPresentation.title(pending.record)) and end all its running shells? The host folder is kept."
                : "End all running shells for \(QuickTerminalPresentation.title(pending.record))? Unsaved work may be lost. The shortcut and folder are kept.")
        }
        .seHaptic(.success, trigger: successTick)
        .seHaptic(.error, trigger: failureTick)
    }

    private var displayedError: String? {
        if let errorMessage { return errorMessage }
        if case .failed(let message) = session.quickTerminals.state { return message }
        return nil
    }

    private func recordRow(_ record: QuickTerminalRecordWire) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(QuickTerminalPresentation.title(record)).toolNocturneFont(.rowTitle)
            Label(QuickTerminalPresentation.targetLabel(record.target), systemImage: "folder")
                .toolNocturneFont(.label).foregroundStyle(p.inkSecondary)
            if let detail = QuickTerminalPresentation.targetDetail(record.target) {
                Text(detail).toolNocturneFont(.support).foregroundStyle(p.inkSecondary).textSelection(.enabled)
            }
            Text(record.cwd).toolNocturneFont(.code).textSelection(.enabled)
            let count = QuickTerminalPresentation.liveTerminalIds(record).count
            Label("\(count) live terminals", systemImage: "terminal")
                .toolNocturneFont(.label).foregroundStyle(p.inkSecondary)
            Button("Open on host", systemImage: "play") { Task { await open(record, createNew: false) } }
                .frame(minHeight: 44)
                .buttonStyle(ToolNocturneButtonStyle(primary: true))
            Button("Create another terminal", systemImage: "plus.rectangle.on.rectangle") {
                Task { await open(record, createNew: true) }
            }
            .frame(minHeight: 44)
            .buttonStyle(ToolNocturneButtonStyle())
            Menu {
                Button("Rename", systemImage: "pencil") {
                    renameTitle = record.title ?? ""
                    renaming = record
                }
                Button("Close all terminals", systemImage: "stop.circle", role: .destructive) {
                    confirming = PendingDestruction(record: record, deleting: false)
                }.disabled(count == 0)
                Button("Delete shortcut", systemImage: "trash", role: .destructive) {
                    confirming = PendingDestruction(record: record, deleting: true)
                }
            } label: {
                Label("Manage quick terminal", systemImage: "ellipsis.circle").frame(minHeight: 44)
            }
        }
        .padding(.vertical, 12)
        .buttonStyle(.borderless)
        .disabled(busy)
        .listRowBackground(p.card)
    }

    private var creationSheet: some View {
        NavigationStack {
            Form {
                Section("Shortcut") {
                    TextField("Title (optional)", text: $draftTitle).frame(minHeight: 44)
                    Picker("Target", selection: $useProject) {
                        Text("Host workspace").tag(false)
                        if creationTargets.count > 1 { Text("Active project root").tag(true) }
                    }.frame(minHeight: 44)
                    if useProject, let project = session.projects.active, session.workspace == .project {
                        Text(project.name).toolNocturneFont(.rowTitle)
                        if let path = project.path { Text(path).toolNocturneFont(.support).textSelection(.enabled) }
                    } else {
                        Text("The host creates a dedicated workspace folder. Creating a shortcut does not start a shell.")
                            .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                    }
                    Text("Remote worktree creation is not available.")
                        .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                }
                if let errorMessage {
                    Label(errorMessage, systemImage: "exclamationmark.triangle").foregroundStyle(p.danger)
                }
                if busy { ToolNocturneProgress(String(localized: "Creating on host…")) }
            }
            .scrollContentBackground(.hidden)
            .background(p.canvas)
            .foregroundStyle(p.ink)
            .toolNocturneFont(.body)
            .navigationTitle("New quick terminal")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { showingCreate = false }.disabled(busy)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") { Task { await create() } }.disabled(busy)
                }
            }
            .disabled(busy)
            .interactiveDismissDisabled(busy)
        }
        .tint(p.accent)
    }

    @MainActor private func refresh() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        errorMessage = nil
        await session.quickTerminals.refresh()
        if case .failed = session.quickTerminals.state { failureTick += 1 }
    }

    @MainActor private func perform(_ operation: () async throws -> Void) async {
        guard !busy else { return }
        busy = true
        errorMessage = nil
        notice = nil
        defer { busy = false }
        do {
            try await operation()
            successTick += 1
        } catch {
            errorMessage = error.localizedDescription
            failureTick += 1
        }
    }

    @MainActor private func create() async {
        await perform {
            // Re-evaluate at submit time; stale active-project state cannot leak into a conversation.
            let targets = creationTargets
            guard !useProject || targets.count > 1 else {
                throw HostError.unexpected("The active project root is no longer available. Choose Host workspace.")
            }
            let title = draftTitle.trimmingCharacters(in: .whitespacesAndNewlines)
            try await session.quickTerminals.create(target: useProject ? targets[1] : targets[0], title: title.isEmpty ? nil : title)
            notice = String(localized: "Quick terminal created. Open it to start a shell on the host.")
            showingCreate = false
        }
    }

    @MainActor private func open(_ record: QuickTerminalRecordWire, createNew: Bool) async {
        await perform {
            // Explicit user action only. No watch/claim/reveal: those cannot retain this foreign scope.
            let result: QuickTerminalOpenedWire
            if createNew {
                result = try await session.quickTerminals.createTerminal(id: record.id, cols: 80, rows: 24)
            } else {
                result = try await session.quickTerminals.open(id: record.id, cols: 80, rows: 24)
            }
            openedTerminalId = result.terminalId
            notice = result.spawned
                ? String(localized: "A terminal was started on the host; it is not attached here.")
                : String(localized: "An existing host terminal was found; it is not attached here.")
        }
    }

    @MainActor private func destroy(_ pending: PendingDestruction) async {
        await perform {
            if pending.deleting {
                try await session.quickTerminals.delete(id: pending.record.id)
                notice = String(localized: "Quick terminal deleted. Its host folder was kept.")
            } else {
                try await session.quickTerminals.close(id: pending.record.id)
                notice = String(localized: "All terminals for this shortcut were closed.")
            }
            if let openedTerminalId, QuickTerminalPresentation.liveTerminalIds(pending.record).contains(openedTerminalId) {
                self.openedTerminalId = nil
            }
        }
    }
}
