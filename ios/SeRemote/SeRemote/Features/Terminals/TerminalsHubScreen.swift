import SwiftUI

/// Identifies the full-screen attach console for one project terminal.
nonisolated struct TerminalsHubConsoleRequest: Identifiable, Hashable, Sendable {
    let terminalId: String
    let title: String

    var id: String { terminalId }
}

/// The 终端 hub: every live project terminal grouped by project, then the
/// 快捷终端 group. Tapping a terminal attaches its existing host PTY; the hub
/// never spawns (closing the console only detaches) and never fabricates a
/// terminal the host did not report.
struct TerminalsHubScreen: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase

    let session: WorkspaceSession

    @State private var store = TerminalsHubStore()
    @State private var console: TerminalsHubConsoleRequest?
    @State private var quickConsole: QuickTerminalConsoleRequest?
    @State private var refreshing = false
    @State private var pendingRefresh = false

    private static let refreshInterval: Duration = .seconds(15)

    var body: some View {
        NocturneToolScreen(
            title: String(localized: "Terminal"),
            subtitle: subtitle,
            onRefresh: { await refresh() }
        ) {
            content
                .task {
                    store.attach(socket: session.terminalSocket) {
                        try await session.terminals.ensureConnected()
                    }
                    await refreshAndPoll()
                }
                .onChange(of: scenePhase) { _, phase in
                    guard phase == .active else { return }
                    Task { await refresh() }
                }
        }
        .fullScreenCover(item: $console, onDismiss: {
            Task { await refresh() }
        }) { request in
            QuickTerminalConsoleView(
                session: session,
                terminalId: request.terminalId,
                claim: nil,
                title: request.title,
                onClose: { console = nil }
            )
        }
        .fullScreenCover(item: $quickConsole, onDismiss: {
            Task { await refresh() }
        }) { request in
            QuickTerminalConsoleView(
                session: session,
                opened: request.opened,
                title: request.title,
                onClose: { quickConsole = nil }
            )
        }
    }

    // MARK: - Data

    private var hostName: String {
        session.origin.host() ?? String(localized: "Host")
    }

    private var subtitle: String {
        switch store.state {
        case .loaded, .empty:
            return "\(hostName) · \(String(localized: "\(store.summaries.count) terminals"))"
        default:
            return hostName
        }
    }

    private var terminalGroups: [TerminalsHubGroup] {
        let agentIds = Set(session.liveAgents.sessions.map(\.terminalId))
        let terminals = store.summaries.map {
            TerminalsHubPresentation.terminal(from: $0, runningAgentTerminalIds: agentIds)
        }
        let projects = session.projects.projects.map {
            TerminalsHubProject(id: $0.id, name: $0.name)
        }
        let excluded = TerminalsHubPresentation.quickTerminalIds(session.quickTerminals.records)
        return TerminalsHubPresentation.groups(
            terminals: terminals,
            projects: projects,
            excluding: excluded
        )
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if let error = store.errorMessage {
            NocturneNotice(message: error, isError: true)
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
        }
        if store.usedProjectFallback {
            NocturneNotice(
                message: String(localized: "This host lists terminals one project at a time."),
                isError: false
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 14)
        }

        if store.state == .loading && store.summaries.isEmpty {
            HStack(spacing: 8) {
                ProgressView()
                Text("Loading terminals from the host…")
                    .font(SeNocturneType.font(11, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
            }
            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 14)
        } else if store.state == .empty {
            NocturneEmptyState(
                systemImage: "terminal",
                title: String(localized: "No running terminals"),
                message: String(localized: "There are no running terminals on the host.")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 14)
        } else if store.errorMessage != nil && store.summaries.isEmpty {
            // The notice above carries the error; do not claim the host is empty.
            EmptyView()
        } else {
            ForEach(terminalGroups) { group in
                groupHeader(group)
                VStack(spacing: NocturneToolMetrics.cardGap) {
                    ForEach(group.terminals) { terminal in
                        terminalRow(terminal)
                    }
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
            }
        }

        TerminalsHubQuickSection(session: session, console: $quickConsole)
    }

    // MARK: - Groups

    private func groupHeader(_ group: TerminalsHubGroup) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(verbatim: group.name)
                .font(SeNocturneType.font(12, weight: .semibold))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            Text(verbatim: "\(group.count)")
                .font(SeNocturneType.font(9, weight: .medium, mono: true))
                .foregroundStyle(palette.inkSecondary)
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(palette.cardAlt, in: Capsule())
                .overlay {
                    Capsule().strokeBorder(palette.stroke, lineWidth: 1)
                }
                .accessibilityLabel(Text("\(group.count) terminals"))
            Spacer(minLength: 8)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 20)
        .padding(.bottom, 8)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("\(group.name), \(group.count) terminals"))
    }

    // MARK: - Rows

    private func terminalRow(_ terminal: TerminalsHubTerminal) -> some View {
        Button {
            console = TerminalsHubConsoleRequest(terminalId: terminal.id, title: terminal.title)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "terminal")
                    .font(.system(size: 14, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: 22, height: 22)

                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: terminal.title)
                        .font(SeNocturneType.font(12, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    if let detail = rowDetail(terminal) {
                        Text(verbatim: detail)
                            .font(SeNocturneType.font(9, weight: .regular, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                if terminal.hasRunningAgent {
                    agentBadge
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(palette.inkTertiary)
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
        .buttonStyle(NocturnePressStyle(reduceMotion: reduceMotion))
        .accessibilityLabel(Text(terminal.title))
        .accessibilityValue(Text(terminal.hasRunningAgent ? String(localized: "Agent running") : ""))
        .accessibilityHint(Text("Opens the running shell. Detaching leaves it running on the host."))
    }

    private func rowDetail(_ terminal: TerminalsHubTerminal) -> String? {
        var parts: [String] = []
        if let branch = terminal.branch { parts.append(branch) }
        if let cwd = terminal.cwd?.trimmingCharacters(in: .whitespacesAndNewlines), !cwd.isEmpty {
            parts.append(cwd)
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private var agentBadge: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(palette.live)
                .frame(width: 5, height: 5)
            Text("Agent")
                .font(SeNocturneType.font(9, weight: .medium))
                .foregroundStyle(palette.live)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(palette.liveSubtle, in: Capsule())
        .accessibilityLabel(Text("Agent running"))
    }

    // MARK: - Refresh

    /// Refresh on appear, keeps refreshing every 15 s while visible (the
    /// `.task` cancels on disappear), and refreshes again on foreground.
    private func refreshAndPoll() async {
        await refresh()
        while !Task.isCancelled {
            try? await Task.sleep(for: Self.refreshInterval)
            guard !Task.isCancelled else { return }
            await refresh()
        }
    }

    /// Single-flight, coalescing refresh. `projects` and `liveAgents` are
    /// polled by `WorkspaceSession` every 15 s, so the hub reads their
    /// published state and only refreshes what it owns: the live terminal
    /// summaries and the quick-terminal records. An overlapping request sets
    /// `pendingRefresh` and is folded into one follow-up pass instead of
    /// being dropped.
    private func refresh() async {
        guard !refreshing else {
            pendingRefresh = true
            return
        }
        refreshing = true
        repeat {
            pendingRefresh = false
            await session.quickTerminals.refresh()
            await store.refresh(projects: session.projects.projects)
        } while pendingRefresh
        refreshing = false
    }
}
