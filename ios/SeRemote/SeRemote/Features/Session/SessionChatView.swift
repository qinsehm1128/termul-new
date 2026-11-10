import SwiftUI

/// The Nocturne chat surface is the live timeline, not a second conversation model.
struct SessionChatView: View {
    private let session: WorkspaceSession?
    private let agentChat: AgentChatStore?

    init(session: WorkspaceSession) {
        self.session = session
        self.agentChat = nil
    }

    init(agentChat: AgentChatStore) {
        self.session = nil
        self.agentChat = agentChat
    }

    var body: some View {
        if let session {
            ChatView(session: session, embedded: true)
        } else if let agentChat {
            AgentChatView(store: agentChat)
        }
    }
}

/// Value-only presentation rules. No clocks, sample metrics or store mutations.
enum NocturneSessionPresentation {
    enum Tone: Equatable { case live, attention, quiet, failed }
    struct Status: Equatable {
        let title: String
        let tone: Tone
    }

    static func status(
        phase: SessionPhase,
        connected: Bool,
        permissions: Int,
        questions: Int,
        streaming: Bool
    ) -> Status {
        switch phase {
        case .idle:
            return Status(title: String(localized: "Disconnected"), tone: .quiet)
        case .connecting:
            return Status(title: String(localized: "Connecting…"), tone: .quiet)
        case .revoked:
            return Status(title: String(localized: "This device has been removed by the host"), tone: .failed)
        case .failed:
            return Status(title: String(localized: "Connection failed"), tone: .failed)
        case .connected:
            guard connected else {
                return Status(title: String(localized: "Reconnecting…"), tone: .quiet)
            }
            if permissions > 0 {
                return Status(title: String(localized: "Permission required"), tone: .attention)
            }
            if questions > 0 {
                return Status(title: String(localized: "Answer required"), tone: .attention)
            }
            return Status(title: streaming ? String(localized: "Working") : String(localized: "Connected"), tone: .live)
        }
    }

    static func acceptsShortcut(isActive: Bool, enabled: Bool) -> Bool {
        isActive && enabled
    }

    static func modeAvailable(_ tab: WorkspaceTab, workspace: WorkspaceKind) -> Bool {
        tab != .chat || workspace == .conversation
    }

    static func subtitle(host: String, agent: String?, model: String?) -> String {
        [host, agent, model]
            .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
    }
}

/// Prototype `.sessionmode` segmented mode bar. Visual height stays 30pt; the
/// button frame keeps a 44pt hit target.
struct SessionModeSwitch: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 0) {
            ForEach(WorkspaceTab.allCases) { tab in
                let selected = session.workspaceTab == tab
                Button {
                    // This method owns the geometry lease ordering; never write workspaceTab.
                    session.setWorkspaceTab(tab)
                } label: {
                    Text(tab.title)
                        .font(SeNocturneType.font(12, weight: selected ? .medium : .regular))
                        .foregroundStyle(selected ? palette.canvas : palette.inkSecondary)
                        .frame(maxWidth: .infinity)
                        .frame(height: 30)
                        .background(
                            selected ? palette.ink : .clear,
                            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
                        )
                        .frame(height: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!NocturneSessionPresentation.modeAvailable(tab, workspace: session.workspace))
                .opacity(NocturneSessionPresentation.modeAvailable(tab, workspace: session.workspace) ? 1 : 0.35)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .padding(3)
        .background(palette.cardAlt.opacity(0.65), in: RoundedRectangle(cornerRadius: 11, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .stroke(palette.strokeStrong, lineWidth: 1)
        }
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: session.workspaceTab)
    }
}

/// Completion feedback is owned by the ack handlers, never by button presses.
struct NocturneSessionButtonStyle: ButtonStyle {
    var primary = false
    @Environment(\.sePalette) private var palette
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.semibold))
            .frame(minHeight: 44)
            .padding(.horizontal, 12)
            .foregroundStyle(primary ? palette.canvas : palette.inkSecondary)
            .background(primary ? palette.ink : palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .stroke(palette.strokeStrong, lineWidth: 1)
            }
            .opacity(!isEnabled ? 0.45 : (configuration.isPressed ? 0.7 : 1))
    }
}

/// Prototype `.ribbon`: a mono status strip inside the chat surface.
struct SessionStatusRibbon: View {
    let status: NocturneSessionPresentation.Status
    var detail: String?
    @Environment(\.sePalette) private var palette

    private var tone: R2Tone {
        switch status.tone {
        case .live: .live
        case .attention: .attention
        case .quiet: .quiet
        case .failed: .failed
        }
    }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 7) {
                R2Dot(tone: tone)
                Text(status.title)
                    .foregroundStyle(tone.color(palette))
                if let detail {
                    Text(detail)
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: 7) {
                R2Dot(tone: tone)
                Text(status.title)
                    .foregroundStyle(tone.color(palette))
                Spacer(minLength: 0)
            }
        }
        .font(SeNocturneType.font(10, mono: true))
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .stroke(palette.stroke, lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text(status.title))
    }
}
