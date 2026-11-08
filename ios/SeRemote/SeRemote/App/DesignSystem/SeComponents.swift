import SwiftUI

/// Semantic status shared by hosts, sessions, tasks and terminals. Every badge
/// pairs text with a symbol, so state never depends on hue alone.
enum SeStatus: Equatable, Sendable {
    case online
    case connecting
    case reconnecting
    case offline
    case working
    case needsApproval
    case completed
    case failed
    case idle
    case paused

    var title: String {
        switch self {
        case .online: String(localized: "Online")
        case .connecting: String(localized: "Connecting")
        case .reconnecting: String(localized: "Reconnecting")
        case .offline: String(localized: "Offline")
        case .working: String(localized: "Working")
        case .needsApproval: String(localized: "Needs approval")
        case .completed: String(localized: "Completed")
        case .failed: String(localized: "Failed")
        case .idle: String(localized: "Idle")
        case .paused: String(localized: "Paused")
        }
    }

    var symbol: String {
        switch self {
        case .online: "circle.fill"
        case .connecting, .reconnecting: "arrow.triangle.2.circlepath"
        case .offline: "circle.slash"
        case .working: "circle.dotted.circle"
        case .needsApproval: "hand.raised.fill"
        case .completed: "checkmark.circle.fill"
        case .failed: "exclamationmark.triangle.fill"
        case .idle: "circle"
        case .paused: "pause.circle"
        }
    }

    var foreground: Color {
        switch self {
        case .online, .working: SeTheme.live
        case .connecting, .reconnecting, .needsApproval: SeTheme.warning
        case .completed: SeTheme.success
        case .failed: SeTheme.danger
        case .offline, .idle, .paused: SeTheme.secondaryInk
        }
    }

    var background: Color {
        switch self {
        case .online, .working: SeTheme.liveSubtle
        case .connecting, .reconnecting, .needsApproval: SeTheme.warningSubtle
        case .completed: SeTheme.accentSubtle
        case .failed: SeTheme.dangerSubtle
        case .offline, .idle, .paused: SeTheme.elevated
        }
    }
}

extension HostRowStatus {
    var seStatus: SeStatus {
        switch self {
        case .need: .needsApproval
        case .working: .working
        case .idle: .idle
        }
    }
}

struct SeStatusBadge: View {
    let status: SeStatus
    var compact = false

    var body: some View {
        Label {
            if !compact { Text(status.title) }
        } icon: {
            Image(systemName: status.symbol)
                .imageScale(.small)
        }
        .font(SeFont.label)
        .foregroundStyle(status.foreground)
        .padding(.horizontal, compact ? 6 : 8)
        .padding(.vertical, 3)
        .background(status.background, in: Capsule())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(status.title)
    }
}

/// Opaque content container. Glass is reserved for navigation chrome.
struct SeCard<Content: View>: View {
    var padding: CGFloat = SeSpace.md
    var fill: Color = SeTheme.surface
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(fill, in: RoundedRectangle(cornerRadius: SeRadius.content, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: SeRadius.content, style: .continuous)
                    .strokeBorder(SeTheme.stroke, lineWidth: 1)
            }
    }
}

struct SeSectionHeader: View {
    let title: String
    var detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(SeFont.label)
                .foregroundStyle(SeTheme.secondaryInk)
                .textCase(.uppercase)
            Spacer(minLength: SeSpace.xs)
            if let detail {
                Text(detail)
                    .font(SeFont.caption)
                    .foregroundStyle(SeTheme.tertiaryInk)
                    .monospacedDigit()
            }
        }
        .accessibilityAddTraits(.isHeader)
    }
}

/// Empty and unavailable states. Serif title per the identity rule.
struct SeEmptyState: View {
    let title: String
    let systemImage: String
    var message: String?
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        ContentUnavailableView {
            Label {
                Text(title).font(.system(.title3, design: .serif).weight(.semibold))
            } icon: {
                Image(systemName: systemImage).foregroundStyle(SeTheme.secondaryInk)
            }
        } description: {
            if let message { Text(message).foregroundStyle(SeTheme.secondaryInk) }
        } actions: {
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .buttonStyle(SePrimaryButtonStyle())
            }
        }
    }
}

/// Inline, persistent error. Failures never rely on a transient toast alone.
struct SeErrorBanner: View {
    let message: String
    var retryTitle: String = String(localized: "Retry")
    var retry: (() -> Void)?

    var body: some View {
        HStack(alignment: .top, spacing: SeSpace.sm) {
            Image(systemName: SeStatus.failed.symbol)
                .foregroundStyle(SeTheme.danger)
            Text(message)
                .font(SeFont.support)
                .foregroundStyle(SeTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
            if let retry {
                Button(retryTitle, action: retry)
                    .font(SeFont.label)
                    .frame(minHeight: SeSpace.hitTarget)
            }
        }
        .padding(SeSpace.sm)
        .background(SeTheme.dangerSubtle, in: RoundedRectangle(cornerRadius: SeRadius.tool, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

/// Solid accent action, for example Send, Approve or Connect.
struct SePrimaryButtonStyle: ButtonStyle {
    var role: ButtonRole?
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        let destructive = role == .destructive
        configuration.label
            .font(.body.weight(.semibold))
            .foregroundStyle(destructive ? Color.white : SeTheme.onAccent)
            .padding(.horizontal, SeSpace.md)
            .frame(minHeight: SeSpace.hitTarget)
            .background(destructive ? SeTheme.danger : SeTheme.accent, in: Capsule())
            .opacity(isEnabled ? (configuration.isPressed ? 0.82 : 1) : 0.45)
            .animation(SeMotion.press.animation(reduceMotion: reduceMotion), value: configuration.isPressed)
    }
}

/// Quiet secondary action on a tinted fill.
struct SeSecondaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.medium))
            .foregroundStyle(SeTheme.ink)
            .padding(.horizontal, SeSpace.md)
            .frame(minHeight: SeSpace.hitTarget)
            .background(SeTheme.elevated, in: Capsule())
            .opacity(isEnabled ? (configuration.isPressed ? 0.75 : 1) : 0.45)
            .animation(SeMotion.press.animation(reduceMotion: reduceMotion), value: configuration.isPressed)
    }
}

/// Row press feedback: a short background change, never a whole-card scale.
struct SePressableRowStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .contentShape(Rectangle())
            .background(configuration.isPressed ? SeTheme.elevated : Color.clear)
            .animation(SeMotion.press.animation(reduceMotion: reduceMotion), value: configuration.isPressed)
    }
}

/// Agent, mode and model chips, plus filters.
struct SeChip: View {
    let title: String
    var systemImage: String?
    var isSelected = false
    var isEnabled = true

    var body: some View {
        HStack(spacing: SeSpace.xxs) {
            if let systemImage { Image(systemName: systemImage).imageScale(.small) }
            Text(title).lineLimit(1)
        }
        .font(SeFont.label)
        .foregroundStyle(isSelected ? SeTheme.accent : SeTheme.ink)
        .padding(.horizontal, SeSpace.sm)
        .frame(minHeight: 32)
        .background(isSelected ? SeTheme.accentSubtle : SeTheme.elevated, in: Capsule())
        .overlay {
            Capsule().strokeBorder(isSelected ? SeTheme.accent.opacity(0.5) : Color.clear, lineWidth: 1)
        }
        .opacity(isEnabled ? 1 : 0.5)
        .contentShape(Capsule())
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}

/// Monospaced code surface for diffs, commands and screen text. Opaque, never material.
struct SeCodeBlock: View {
    let text: String
    @ScaledMetric(relativeTo: .subheadline) private var codeSize = SeFont.codeBaseSize

    var body: some View {
        ScrollView(.horizontal) {
            Text(text)
                .font(SeFont.code(size: codeSize))
                .foregroundStyle(SeTheme.ink)
                .textSelection(.enabled)
                .padding(SeSpace.sm)
                .fixedSize(horizontal: true, vertical: false)
        }
        .background(SeTheme.codeCanvas, in: RoundedRectangle(cornerRadius: SeRadius.tool, style: .continuous))
    }
}
