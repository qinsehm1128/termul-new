import SwiftUI

/// The 快捷终端 group inside the 终端 hub.
///
/// It renders the same host records as the dedicated quick-terminal screen and
/// offers the same create / rename / close / delete actions. Opening a record
/// still asks the host for its claim and attaches the existing PTY.
struct TerminalsHubQuickSection: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession
    @Binding var console: QuickTerminalConsoleRequest?

    @State private var showingCreate = false
    @State private var renaming: QuickTerminalRecordWire?
    @State private var pending: TerminalsHubQuickDestruction?
    @State private var draftTitle = ""
    @State private var renameTitle = ""
    @State private var useProject = false
    @State private var busy = false
    @State private var errorMessage: String?
    @State private var notice: String?

    private var store: QuickTerminalStore { session.quickTerminals }

    private var loaded: Bool {
        store.state == .loaded || store.state == .empty
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
            (target, target == .workspace
                ? String(localized: "Host workspace")
                : String(localized: "Active project root"))
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("Quick terminals")
                .font(SeNocturneType.font(12, weight: .semibold))
                .foregroundStyle(palette.ink)
            if loaded {
                Text(verbatim: "\(store.records.count)")
                    .font(SeNocturneType.font(9, weight: .medium, mono: true))
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(palette.cardAlt, in: Capsule())
                    .overlay {
                        Capsule().strokeBorder(palette.stroke, lineWidth: 1)
                    }
            }
            Spacer(minLength: 8)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 26)
        .padding(.bottom, 8)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("Quick terminals, \(store.records.count) shortcuts"))

        VStack(spacing: NocturneToolMetrics.cardGap) {
            if let errorMessage {
                NocturneNotice(message: errorMessage, isError: true)
            } else if let notice {
                NocturneNotice(message: notice, isError: false)
            }

            if store.state == .loading && store.records.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading from the host…")
                        .font(SeNocturneType.font(11, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                }
                .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            } else if store.records.isEmpty {
                NocturneEmptyState(
                    systemImage: "bolt.horizontal.circle",
                    title: String(localized: "No quick terminals"),
                    message: String(localized: "Create a shortcut to a host folder. Creating it does not start a shell.")
                )
            } else {
                ForEach(store.records) { record in
                    recordRow(record)
                }
            }

            createRow
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.bottom, 10)
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
            Button(
                item.deleting ? String(localized: "Delete") : String(localized: "Close shells"),
                role: .destructive
            ) { Task { await destroy(item) } }
            Button(String(localized: "Cancel"), role: .cancel) { pending = nil }
        } message: { item in
            Text(item.deleting
                 ? String(localized: "Delete \(QuickTerminalPresentation.title(item.record)) and end all its running shells? The host folder is kept.")
                 : String(localized: "End all running shells for \(QuickTerminalPresentation.title(item.record))? Unsaved work may be lost. The shortcut and folder are kept."))
        }
    }

    // MARK: - Rows

    private func recordRow(_ record: QuickTerminalRecordWire) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "bolt.horizontal.circle")
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkTertiary)
                .frame(width: 22, height: 22)

            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: QuickTerminalPresentation.title(record))
                    .font(SeNocturneType.font(11, weight: .regular, mono: true))
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
                    pending = TerminalsHubQuickDestruction(record: record, deleting: false)
                }
                Button(String(localized: "Delete"), systemImage: "trash", role: .destructive) {
                    pending = TerminalsHubQuickDestruction(record: record, deleting: true)
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
            errorMessage = nil
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
        .disabled(busy)
    }

    private func meta(_ record: QuickTerminalRecordWire) -> String {
        var parts = [QuickTerminalPresentation.targetLabel(record.target)]
        if let detail = QuickTerminalPresentation.targetDetail(record.target) { parts.append(detail) }
        let live = QuickTerminalPresentation.liveTerminalIds(record).count
        parts.append(String(localized: "\(live) live"))
        return parts.joined(separator: " · ")
    }

    // MARK: - Create sheet

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
                    if case .projectRoot(_, let root) = target, !root.isEmpty {
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

    // MARK: - Actions

    private func create() async {
        guard !busy else { return }
        busy = true
        errorMessage = nil
        notice = nil
        defer { busy = false }
        let targets = creationTargets
        guard !useProject || targets.count > 1 else {
            errorMessage = String(localized: "The active project root is no longer available. Choose Host workspace.")
            return
        }
        do {
            let title = draftTitle.trimmingCharacters(in: .whitespacesAndNewlines)
            try await store.create(
                target: useProject ? targets[1] : targets[0],
                title: title.isEmpty ? nil : title
            )
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
            // Never present the console while a hidden workbench terminal owns
            // the keyboard: dismiss first so the cover gets a clean responder.
            session.dismissTerminalKeyboard()
            console = QuickTerminalConsoleRequest(
                opened: result,
                title: QuickTerminalPresentation.title(record)
            )
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

    private func destroy(_ item: TerminalsHubQuickDestruction) async {
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
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private var renameBinding: Binding<Bool> {
        Binding(get: { renaming != nil }, set: { _ in })
    }

    private var pendingBinding: Binding<Bool> {
        Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })
    }

    private func destructionTitle(_ item: TerminalsHubQuickDestruction) -> String {
        item.deleting
            ? String(localized: "Delete this quick terminal?")
            : String(localized: "Close its running shells?")
    }
}

struct TerminalsHubQuickDestruction: Identifiable {
    let record: QuickTerminalRecordWire
    let deleting: Bool
    var id: String { "\(record.id)-\(deleting)" }
}
