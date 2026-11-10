import SwiftUI

/// The iPad inspector's two segments: the terminal's file browser and its
/// agent conversation.
nonisolated enum TerminalInspectorSegment: String, CaseIterable, Hashable, Identifiable, Sendable {
    case files
    case agent

    var id: String { rawValue }

    var title: String {
        switch self {
        case .files: String(localized: "Files")
        case .agent: String(localized: "Agent")
        }
    }
}

/// iPad inspector "Files" segment: breadcrumb plus listing rooted at the
/// terminal cwd. There is no inline preview; a file tap reports `onOpenFile` so
/// the caller can present the same full-screen cover as the iPhone panel.
@MainActor
struct TerminalFileInspector: View {
    let session: WorkspaceSession
    let rootPath: String
    let terminalTitle: String
    let onOpenFile: (String) -> Void

    var body: some View {
        TerminalFileBrowser(
            session: session,
            rootPath: rootPath,
            onOpenFile: onOpenFile
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Files in \(terminalTitle)"))
        .accessibilityIdentifier("files-inspector")
    }
}

/// Right-hand iPad inspector: a Files / Agent segment switch over the terminal
/// file browser and the caller-supplied agent surface.
@MainActor
struct TerminalInspector: View {
    @Environment(\.sePalette) private var palette

    let session: WorkspaceSession
    let rootPath: String
    let terminalTitle: String
    let agentAvailable: Bool
    @Binding var segment: TerminalInspectorSegment
    let onOpenFile: (String) -> Void
    let agentContent: AnyView

    init(
        session: WorkspaceSession,
        rootPath: String,
        terminalTitle: String,
        agentAvailable: Bool,
        segment: Binding<TerminalInspectorSegment>,
        onOpenFile: @escaping (String) -> Void,
        agentContent: AnyView
    ) {
        self.session = session
        self.rootPath = rootPath
        self.terminalTitle = terminalTitle
        self.agentAvailable = agentAvailable
        self._segment = segment
        self.onOpenFile = onOpenFile
        self.agentContent = agentContent
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            segmentRow
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(palette.surface)
        .task {
            if !agentAvailable, segment == .agent {
                segment = .files
            }
        }
        .onChange(of: agentAvailable) { _, available in
            if !available, segment == .agent {
                segment = .files
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("terminal-inspector")
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(String(localized: "Files"))
                .font(SeNocturneType.font(15, weight: .semibold))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 8)
            Text(terminalTitle)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(palette.inkTertiary)
                .lineLimit(1)
        }
        .padding(.horizontal, 18)
        .padding(.top, 14)
    }

    private var segmentRow: some View {
        HStack(spacing: 6) {
            TerminalInspectorSegmentControl(segment: $segment, agentAvailable: agentAvailable)
            if !agentAvailable {
                SeInfoTip(
                    text: String(localized: "No agent is attached to this terminal."),
                    accessibilityLabel: String(localized: "Agent unavailable"),
                    size: 30
                )
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 4)
    }

    @ViewBuilder
    private var content: some View {
        switch segment {
        case .files:
            TerminalFileInspector(
                session: session,
                rootPath: rootPath,
                terminalTitle: terminalTitle,
                onOpenFile: onOpenFile
            )
        case .agent:
            agentContent
        }
    }
}

/// Prototype capsule segment switch (Files / Agent). The Agent segment is
/// disabled when the terminal has no agent surface.
private struct TerminalInspectorSegmentControl: View {
    @Binding var segment: TerminalInspectorSegment
    let agentAvailable: Bool

    @Environment(\.sePalette) private var palette

    var body: some View {
        HStack(spacing: 2) {
            ForEach(TerminalInspectorSegment.allCases) { option in
                let enabled = option != .agent || agentAvailable
                Button {
                    segment = option
                } label: {
                    Text(option.title)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(segment == option ? palette.canvas : palette.inkTertiary)
                        .frame(maxWidth: .infinity, minHeight: 30)
                        .background(
                            segment == option ? palette.ink : Color.clear,
                            in: Capsule(style: .continuous)
                        )
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .disabled(!enabled)
                .opacity(enabled ? 1 : 0.5)
                .accessibilityAddTraits(segment == option ? .isSelected : [])
            }
        }
        .padding(2)
        .background(palette.cardAlt, in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous).stroke(palette.stroke, lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Inspector section"))
    }
}
