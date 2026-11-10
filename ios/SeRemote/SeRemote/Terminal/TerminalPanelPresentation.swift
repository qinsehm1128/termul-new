import Foundation

/// What the more sheet's close action means for the surface it is closing.
///
/// Quick terminals have a host close route, so their shell really ends.
/// Workspace terminals have no kill route on the phone: closing only
/// disconnects this view and the shell keeps running on the host.
nonisolated enum TerminalClosePolicy: Equatable, Sendable {
    case closeShell
    case disconnect

    var actionTitle: String {
        switch self {
        case .closeShell: String(localized: "Close Terminal")
        case .disconnect: String(localized: "Disconnect")
        }
    }

    var confirmTitle: String {
        switch self {
        case .closeShell: String(localized: "Close this terminal?")
        case .disconnect: String(localized: "Disconnect from this terminal?")
        }
    }

    /// States the consequence, never asks the user to infer it.
    var consequence: String {
        switch self {
        case .closeShell:
            String(localized: "The shell on the host ends.")
        case .disconnect:
            String(localized: "The shell keeps running on the host.")
        }
    }

    var isDestructive: Bool { self == .closeShell }
}

/// Pure presentation for the collapsed pill's overlay panel.
///
/// Everything here is derived from facts passed in, so the panel can be
/// unit-tested without a mounted canvas and, crucially, without touching the
/// PTY. The panel is an overlay: producing this value never resizes anything.
nonisolated struct TerminalPanelPresentation: Equatable, Sendable {
    struct Tab: Identifiable, Equatable, Sendable {
        let id: String
        let title: String
        let index: Int
    }

    struct PTYRow: Equatable, Sendable {
        let status: String
        let isReady: Bool
        let dimensions: String
        /// FIT is a workspace concern; quick terminals keep their own lease.
        let showsFit: Bool
        /// Whether the phone fit currently owns the viewport lease.
        let fitActive: Bool
        let textScaleLabel: String
    }

    struct AgentRow: Equatable, Sendable {
        let name: String
        let status: String
        let permissionCount: Int
    }

    let title: String
    let subtitle: String?
    let counter: String
    let tabs: [Tab]
    let activeTabID: String?
    let showsTabs: Bool
    let pty: PTYRow
    let agent: AgentRow?
    let close: TerminalClosePolicy
    let isStandalone: Bool

    static func make(
        request: TerminalOpenRequest?,
        items: [TerminalSurfaceItem],
        activeID: String?,
        cols: Int?,
        rows: Int?,
        agentName: String?,
        agentReady: Bool,
        permissionCount: Int,
        subtitle: String? = nil,
        fitActive: Bool = true
    ) -> TerminalPanelPresentation {
        let isStandalone = request?.attach.isStandalone ?? false

        let activeIndex = items.firstIndex(where: { $0.id == activeID })
        let counter: String
        if let activeIndex {
            counter = "\(activeIndex + 1)/\(max(items.count, 1))"
        } else {
            counter = items.isEmpty ? "1/1" : "1/\(items.count)"
        }

        let title: String
        if let requestTitle = request?.title, !requestTitle.isEmpty {
            title = requestTitle
        } else if let activeIndex {
            title = items[activeIndex].title
        } else {
            title = String(localized: "Terminal")
        }

        let tabs = items.enumerated().map { index, item in
            Tab(id: item.id, title: item.title, index: index)
        }

        let dimensions: String
        if let cols, let rows, cols > 1, rows > 1 {
            dimensions = "\(cols) × \(rows)"
        } else {
            dimensions = "—"
        }

        let pty = PTYRow(
            status: dimensions == "—" ? String(localized: "Connecting…") : String(localized: "Ready"),
            isReady: dimensions != "—",
            dimensions: dimensions,
            showsFit: !isStandalone,
            fitActive: !isStandalone && fitActive,
            textScaleLabel: "A+"
        )

        let agent: AgentRow? = {
            guard agentReady, let agentName, !agentName.isEmpty else { return nil }
            return AgentRow(
                name: agentName,
                status: permissionCount > 0
                    ? String(localized: "Needs you")
                    : String(localized: "Working"),
                permissionCount: permissionCount
            )
        }()

        return TerminalPanelPresentation(
            title: title,
            subtitle: subtitle,
            counter: counter,
            tabs: tabs,
            activeTabID: activeID,
            showsTabs: !isStandalone,
            pty: pty,
            agent: agent,
            close: isStandalone ? .closeShell : .disconnect,
            isStandalone: isStandalone
        )
    }
}
