import SwiftUI

/// Quick terminals: create, open, rename, close and delete real host shortcuts.
/// The store never attaches a host PTY here; it returns the claim the terminal
/// flow can use, and this screen says so instead of pretending it is attached.
struct QuickTerminalsFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession

    @State private var filter: QuickFilter = .all
    @State private var showingCreate = false
    @State private var renaming: QuickTerminalRecordWire?
    @State private var pending: QuickDestruction?
    @State private var draftTitle = ""
    @State private var renameTitle = ""
    @State private var useProject = false
    @State private var busy = false
    @State private var errorMessage: String?
    @State private var notice: String?
    @State private var openedTerminalId: String?

    private var store: QuickTerminalStore { session.quickTerminals }

    var body: some View {
        NocturneToolScreen(
            title: String(localized: "Quick terminals"),
            subtitle: navSubtitle,
            onRefresh: refresh
        ) {
            content
                .task { await refreshIfNeeded() }
        }
        .sheet(isPresented: $showingCreate) { createSheet }
        .alert(String(localized: "Rename quick terminal"), isPresented: renameBinding) {
            TextField(String(localized: "Title"), text: $renameTitle)
            Button(String(localized: "Cancel"), role: .cancel) { renaming = nil }
            Button(String(localized: "Rename")) { rename() }
        } message: {
            Text("Leave the title empty to clear it and use the fallback title.")
        }
        .confirmationDialog(
            pending.map(destructionTitle) ?? "",
            isPresented: pendingBinding,
            titleVisibility: .visible,
            presenting: pending
        ) { item in
            Button(item.deleting ? String(localized: "Delete") : String(localized: "Close shells"),
                   role: .destructive) { Task { await destroy(item) } }
            Button(String(localized: "Cancel"), role: .cancel) { pending = nil }
        } message: { item in
            Text(item.deleting
                 ? String(localized: "Delete \(QuickTerminalPresentation.title(item.record)) and end all its running shells? The host folder is kept.")
                 : String(localized: "End all running shells for \(QuickTerminalPresentation.title(item.record))? Unsaved work may be lost. The shortcut and folder are kept."))
        }
        .seHaptic(.selection, trigger: filter)
    }

    // MARK: Data

    private var hostName: String {
        session.origin.host() ?? String(localized: "Host")
    }

    private var loaded: Bool {
        store.state == .loaded || store.state == .empty
    }

    private var navSubtitle: String {
        guard loaded else { return hostName }
        return "\(hostName) · \(String(localized: "\(store.records.count) commands"))"
    }

    private var creationTargets: [QuickTerminalTargetWire] {
        QuickTerminalPresentation.creationTargets(
            isProjectSession: session.workspace == .project,
            projectId: session.projects.active?.id,
            projectRoot: session.projects.active?.path
        )
    }

    private var targetOptions: [(target: QuickTerminalTargetWire, label: String)] {
        creationTargets.map { target in
            (target, target == .workspace ? String(localized: "Host workspace") : String(localized: "Active project root"))
        }
    }

    private var visibleRecords: [QuickTerminalRecordWire] {
        switch filter {
        case .all: store.records
        case .workspace: store.records.filter { if case .workspace = $0.target { return true }; return false }
        case .project: store.records.filter { if case .projectRoot = $0.target { return true }; return false }
        }
    }

    private var hasProjectTargets: Bool {
        store.records.contains { if case .projectRoot = $0.target { return true }; return false }
    }

    private var storeError: String? {
        if case let .failed(message) = store.state { return message }
        return nil
    }

    // MARK: Body

    @ViewBuilder
    private var content: some View {
        NocturneChipRail {
            NocturneChip(title: String(localized: "All"), count: loaded ? store.records.count : nil,
                         isOn: filter == .all) { filter = .all }
            NocturneChip(title: String(localized: "Workspace"),
                         count: loaded ? store.records.filter { if case .workspace = $0.target { return true }; return false }.count : nil,
                         isOn: filter == .workspace) { filter = .workspace }
            if hasProjectTargets {
                NocturneChip(title: String(localized: "Project root"),
                             count: store.records.filter { if case .projectRoot = $0.target { return true }; return false }.count,
                             isOn: filter == .project) { filter = .project }
            }
        }

        if let errorMessage {
            NocturneNotice(message: errorMessage, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
        } else if let storeError {
            NocturneNotice(message: storeError, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
        } else if let notice {
            NocturneNotice(message: notice, isError: false)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
        }

        NocturneGroupLabel(
            title: String(localized: "Pinned terminals"),
            detail: loaded ? String(format: "%02d", visibleRecords.count) : ""
        )

        VStack(spacing: NocturneToolMetrics.cardGap) {
            if store.state == .loading && store.records.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading from the host…")
                        .font(SeNocturneType.font(11, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                }
                .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            } else if visibleRecords.isEmpty {
                NocturneEmptyState(
                    systemImage: "terminal",
                    title: String(localized: "No quick terminals"),
                    message: String(localized: "Create a shortcut to a host folder. Creating it does not start a shell.")
                )
            } else {
                ForEach(visibleRecords) { record in
                    recordRow(record)
                }
            }

            createRow
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)

        NocturneGroupLabel(title: String(localized: "Note"), detail: "")
        NocturneNotice(
            message: String(localized: "Quick terminals save a command target, not output. Output stays in the host terminal."),
            isError: false
        )
        .padding(.horizontal, NocturneToolMetrics.gutter)
    }

    // MARK: Rows

    private func recordRow(_ record: QuickTerminalRecordWire) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "terminal")
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkTertiary)
                .frame(width: 22, height: 22)

            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: QuickTerminalPresentation.title(record))
                    .font(SeNocturneType.font(10.5, weight: .regular, mono: true))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: meta(record))
                    .font(SeNocturneType.font(9, weight: .regular, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            NocturneRunButton(title: String(localized: "Open"), systemImage: "arrow.up.forward.app") {
                Task { await open(record) }
            }
            .disabled(busy)

            Menu {
                Button(String(localized: "Rename"), systemImage: "pencil") { beginRename(record) }
                Button(String(localized: "Close shells"), systemImage: "xmark.circle") {
                    pending = QuickDestruction(record: record, deleting: false)
                }
                Button(String(localized: "Delete"), systemImage: "trash", role: .destructive) {
                    pending = QuickDestruction(record: record, deleting: true)
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 38, height: 28)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 7))
                    .overlay {
                        RoundedRectangle(cornerRadius: 7)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
                    .frame(width: NocturneToolMetrics.minimumTarget, height: NocturneToolMetrics.minimumTarget)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(String(localized: "More actions"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var createRow: some View {
        Button {
            draftTitle = ""
            useProject = false
            showingCreate = true
        } label: {
            HStack(spacing: 7) {
                Image(systemName: "plus")
                    .font(.system(size: 12, weight: .semibold))
                Text("New quick terminal")
                    .font(SeNocturneType.font(10.5, weight: .medium))
            }
            .foregroundStyle(palette.inkTertiary)
            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
            .background(.clear, in: RoundedRectangle(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(palette.stroke, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func meta(_ record: QuickTerminalRecordWire) -> String {
        var parts = [QuickTerminalPresentation.targetLabel(record.target)]
        if let detail = QuickTerminalPresentation.targetDetail(record.target) { parts.append(detail) }
        if let cwd = record.cwd.nocturneNonEmpty { parts.append(cwd) }
        let live = QuickTerminalPresentation.liveTerminalIds(record).count
        parts.append(String(localized: "\(live) live"))
        return parts.joined(separator: " · ")
    }

    // MARK: Create sheet

    private var createSheet: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Capsule()
                        .fill(palette.strokeStrong)
                        .frame(width: 38, height: 5)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 6)

                    Text("New quick terminal")
                        .font(SeNocturneType.font(19, weight: .semibold))
                        .foregroundStyle(palette.ink)

                    Text("The host creates the folder on demand. Creating a shortcut does not start a shell.")
                        .font(SeNocturneType.font(11, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                        .fixedSize(horizontal: false, vertical: true)

                    NocturneGroupLabel(title: String(localized: "Title"), detail: String(localized: "Optional"))
                        .padding(.horizontal, -NocturneToolMetrics.gutter)
                    TextField(String(localized: "Title"), text: $draftTitle)
                        .font(SeNocturneType.font(12, weight: .regular))
                        .padding(12)
                        .frame(minHeight: NocturneToolMetrics.minimumTarget)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 12))
                        .overlay {
                            RoundedRectangle(cornerRadius: 12)
                                .strokeBorder(palette.stroke, lineWidth: 1)
                        }

                    NocturneGroupLabel(title: String(localized: "Target"), detail: "")
                        .padding(.horizontal, -NocturneToolMetrics.gutter)
                    ForEach(Array(targetOptions.enumerated()), id: \.offset) { _, option in
                        targetOption(option.target, label: option.label)
                    }

                    if let errorMessage {
                        NocturneNotice(message: errorMessage, isError: true)
                    }

                    HStack(spacing: 8) {
                        Button(String(localized: "Cancel")) { showingCreate = false }
                            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
                            .foregroundStyle(palette.inkSecondary)
                            .overlay {
                                RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius)
                                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
                            }
                        Button(String(localized: "Create")) { Task { await create() } }
                            .font(SeNocturneType.font(12, weight: .semibold))
                            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget)
                            .foregroundStyle(palette.onAccent)
                            .background(palette.ink, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
                    }
                    .buttonStyle(.plain)
                    .disabled(busy)
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.bottom, 24)
            }
            .scrollIndicators(.hidden)
            .background(palette.canvas.ignoresSafeArea())
            .toolbar(.hidden, for: .navigationBar)
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.hidden)
    }

    private func targetOption(_ target: QuickTerminalTargetWire, label: String) -> some View {
        let isSelected = target == .workspace ? !useProject : useProject
        return Button {
            useProject = target != .workspace
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: label)
                        .font(SeNocturneType.font(12, weight: .medium))
                        .foregroundStyle(palette.ink)
                    if case .projectRoot(_, let root) = target, let root = root.nocturneNonEmpty {
                        Text(verbatim: root)
                            .font(SeNocturneType.font(9, weight: .regular, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                Image(systemName: isSelected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundStyle(isSelected ? palette.accent : palette.inkTertiary)
            }
            .padding(12)
            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: Actions

    private func create() async {
        guard !busy else { return }
        busy = true
        errorMessage = nil
        notice = nil
        defer { busy = false }
        // Re-evaluate at submit time; stale project state cannot leak in.
        let targets = creationTargets
        guard !useProject || targets.count > 1 else {
            errorMessage = String(localized: "The active project root is no longer available. Choose Host workspace.")
            return
        }
        do {
            let title = draftTitle.trimmingCharacters(in: .whitespacesAndNewlines)
            try await store.create(target: useProject ? targets[1] : targets[0], title: title.isEmpty ? nil : title)
            notice = String(localized: "Quick terminal created. Open it to start a shell on the host.")
            showingCreate = false
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func open(_ record: QuickTerminalRecordWire) async {
        guard !busy else { return }
        busy = true
        errorMessage = nil
        notice = nil
        defer { busy = false }
        do {
            let result = try await store.open(id: record.id, cols: 80, rows: 24)
            openedTerminalId = result.terminalId
            notice = result.spawned
                ? String(localized: "A terminal was started on the host; it is not attached here.")
                : String(localized: "An existing host terminal was found; it is not attached here.")
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func beginRename(_ record: QuickTerminalRecordWire) {
        renameTitle = record.title ?? ""
        renaming = record
    }

    private func rename() {
        guard let record = renaming else { return }
        renaming = nil
        Task {
            busy = true
            errorMessage = nil
            notice = nil
            defer { busy = false }
            do {
                let title = renameTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                try await store.rename(id: record.id, title: title.isEmpty ? nil : title)
                notice = String(localized: "Quick terminal renamed.")
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func destroy(_ item: QuickDestruction) async {
        pending = nil
        busy = true
        errorMessage = nil
        notice = nil
        defer { busy = false }
        do {
            if item.deleting {
                try await store.delete(id: item.record.id)
                notice = String(localized: "Quick terminal deleted. Its host folder was kept.")
            } else {
                try await store.close(id: item.record.id)
                notice = String(localized: "All terminals for this shortcut were closed.")
            }
            if let openedTerminalId, QuickTerminalPresentation.liveTerminalIds(item.record).contains(openedTerminalId) {
                self.openedTerminalId = nil
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func refreshIfNeeded() async {
        if store.state == .idle { await store.refresh() }
    }

    private func refresh() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        await store.refresh()
        if case .failed = store.state {
            errorMessage = nil
        }
    }

    private var renameBinding: Binding<Bool> {
        // The alert buttons clear this state explicitly. Clearing it from the
        // setter can race ahead of the Rename action and lose the record.
        Binding(get: { renaming != nil }, set: { _ in })
    }

    private var pendingBinding: Binding<Bool> {
        Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })
    }

    private func destructionTitle(_ item: QuickDestruction) -> String {
        item.deleting
            ? String(localized: "Delete this quick terminal?")
            : String(localized: "Close its running shells?")
    }
}

enum QuickFilter: Hashable {
    case all, workspace, project
}

struct QuickDestruction: Identifiable {
    let record: QuickTerminalRecordWire
    let deleting: Bool
    var id: String { "\(record.id)-\(deleting)" }
}
