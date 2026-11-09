import SwiftUI

/// The Nocturne chat surface is the live timeline, not a second conversation model.
struct SessionChatView: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        ChatView(session: session, embedded: true)
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

struct SessionModeSwitch: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 4) {
            ForEach(WorkspaceTab.allCases) { tab in
                Button {
                    // This method owns the geometry lease ordering; never write workspaceTab.
                    session.setWorkspaceTab(tab)
                } label: {
                    Text(tab.title)
                        .font(.subheadline.weight(session.workspaceTab == tab ? .semibold : .regular))
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .foregroundStyle(session.workspaceTab == tab ? palette.canvas : palette.inkSecondary)
                        .background {
                            if session.workspaceTab == tab {
                                RoundedRectangle(cornerRadius: SeNocturneRadius.control).fill(palette.ink)
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!NocturneSessionPresentation.modeAvailable(tab, workspace: session.workspace))
                .accessibilityAddTraits(session.workspaceTab == tab ? .isSelected : [])
            }
        }
        .padding(4)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.strip).stroke(palette.strokeStrong, lineWidth: 1)
        }
        .animation(SeNocturneMotion.resolved(.smooth(duration: 0.18), reduceMotion: reduceMotion), value: session.workspaceTab)
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
            .background(primary ? palette.ink : palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.control).stroke(palette.strokeStrong, lineWidth: 1)
            }
            .opacity(!isEnabled ? 0.45 : (configuration.isPressed ? 0.7 : 1))
    }
}

struct SessionStatusRibbon: View {
    let status: NocturneSessionPresentation.Status
    var detail: String?
    @Environment(\.sePalette) private var palette

    private var tint: Color {
        switch status.tone {
        case .live: palette.live
        case .attention: palette.warning
        case .quiet: palette.inkSecondary
        case .failed: palette.danger
        }
    }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                stateLabel
                Spacer(minLength: 4)
                if let detail { detailLabel(detail) }
            }
            VStack(alignment: .leading, spacing: 4) {
                stateLabel
                if let detail { detailLabel(detail) }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control).stroke(palette.strokeStrong, lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
    }

    private var stateLabel: some View {
        Label {
            Text(status.title)
        } icon: {
            Circle().fill(tint).frame(width: 6, height: 6)
        }
        .font(.caption.monospaced())
        .foregroundStyle(tint)
    }

    private func detailLabel(_ text: String) -> some View {
        Text(text)
            .font(.caption.monospaced())
            .foregroundStyle(palette.inkSecondary)
            .lineLimit(1)
    }
}
