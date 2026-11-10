import SwiftUI

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

    static func subtitle(host: String, agent: String?, model: String?) -> String {
        [host, agent, model]
            .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
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
