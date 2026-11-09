import SwiftUI

struct WorkspaceMessageEntrance: ViewModifier {
    let enabled: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if enabled {
            content.seEntrance()
        } else {
            content.transition(.identity)
        }
    }
}

/// Prototype agent row: a 20pt mark, the agent name, and an ACP mono label.
struct MessageBubble: View {
    @Environment(\.sePalette) private var palette
    let message: ChatMessage
    var agentName: String?
    var onEdit: (() -> Void)?
    var onRetry: (() -> Void)?

    var body: some View {
        Group {
            if message.role == .user {
                userRow
            } else {
                agentRow
            }
        }
        .contextMenu { messageActions }
    }

    // MARK: User — right-aligned bone pill (prototype `.msg-user .bubble`).

    private var userRow: some View {
        HStack {
            Spacer(minLength: 48)
            VStack(alignment: .trailing, spacing: 4) {
                MessageBubbleContent(message: message)
                    .equatable()
                if let receipt {
                    if message.delivery == .failed, let onRetry {
                        Button(action: onRetry) {
                            receiptView(receipt)
                        }
                        .buttonStyle(.plain)
                        .frame(minHeight: 44)
                        .accessibilityHint(Text("Tap to resend"))
                    } else {
                        receiptView(receipt)
                    }
                }
            }
        }
        .padding(.top, 14)
    }

    // MARK: Agent — open row with mark/name header (prototype `.agenthead`).

    private var agentRow: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 7) {
                Text(agentMark)
                    .font(.system(size: 9, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white)
                    .frame(width: 20, height: 20)
                    .background(palette.accent, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .accessibilityHidden(true)
                Text(displayAgentName)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                Text("ACP")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(palette.inkTertiary)
                Spacer(minLength: 0)
            }
            MessageBubbleContent(message: message)
                .equatable()
            if message.streaming {
                ProgressView()
                    .controlSize(.mini)
                    .tint(palette.accent)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 14)
    }

    private var displayAgentName: String {
        let trimmed = agentName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? String(localized: "Agent") : trimmed
    }

    /// Prototype uses a two-letter mark; derive it from the real agent name.
    private var agentMark: String {
        NocturneR2Presentation.agentMark(displayAgentName)
    }

    @ViewBuilder
    private var messageActions: some View {
        Button {
            UIPasteboard.general.string = message.text
        } label: {
            Label(String(localized: "Copy"), systemImage: "doc.on.doc")
        }
        if message.role == .user {
            if let onEdit {
                Button(action: onEdit) {
                    Label(String(localized: "Edit & Resend"), systemImage: "square.and.pencil")
                }
            }
            if message.delivery == .failed, let onRetry {
                Button(action: onRetry) {
                    Label(String(localized: "Resend"), systemImage: "arrow.up.circle")
                }
            }
        } else if let onRetry {
            Button(action: onRetry) {
                Label(String(localized: "Regenerate"), systemImage: "arrow.clockwise")
            }
        }
        ShareLink(item: message.text) {
            Label(String(localized: "Share"), systemImage: "square.and.arrow.up")
        }
    }

    private func receiptView(_ receipt: Receipt) -> some View {
        HStack(spacing: 4) {
            if receipt.showsProgress {
                ProgressView()
                    .controlSize(.mini)
            } else {
                Image(systemName: receipt.symbol)
            }
            Text(receipt.label)
        }
        .font(.caption2)
        .foregroundStyle(receipt.tint)
        .padding(.horizontal, 4)
        .accessibilityLabel(Text(receipt.label))
    }

    private var receipt: Receipt? {
        guard message.role == .user, let delivery = message.delivery else { return nil }
        switch delivery {
        case .sending:
            return Receipt(
                symbol: "arrow.up.circle",
                label: String(localized: "Sending…"),
                tint: .secondary,
                showsProgress: true
            )
        case .accepted:
            return Receipt(
                symbol: "checkmark",
                label: String(localized: "Host received"),
                tint: .secondary,
                showsProgress: false
            )
        case .failed:
            return Receipt(
                symbol: "exclamationmark.circle",
                label: String(localized: "Not received"),
                tint: palette.danger,
                showsProgress: false
            )
        }
    }
}

/// Equatable bubble payload — equality over the render-affecting fields so
/// `.equatable()` can skip unchanged rows (actions stay on the wrapper).
///
/// User text sits in a right-aligned `--ink` pill. Agent text is open, 13pt,
/// 1.66 line height, exactly like the prototype's `.agentbody`.
struct MessageBubbleContent: View, Equatable {
    @Environment(\.sePalette) private var palette
    let message: ChatMessage

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.message.id == rhs.message.id
            && lhs.message.role == rhs.message.role
            && lhs.message.text == rhs.message.text
            && lhs.message.streaming == rhs.message.streaming
            && lhs.message.delivery == rhs.message.delivery
    }

    var body: some View {
        if message.role == .user {
            Text(attributed)
                .font(.system(size: 13))
                .lineSpacing(3)
                .foregroundStyle(palette.surface)
                .textSelection(.enabled)
                .padding(.horizontal, 13)
                .padding(.vertical, 11)
                .background(palette.ink, in: UnevenRoundedRectangle(
                    topLeadingRadius: 13,
                    bottomLeadingRadius: 13,
                    bottomTrailingRadius: 3,
                    topTrailingRadius: 13,
                    style: .continuous
                ))
                .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            Text(attributed)
                .font(.system(size: 13))
                .lineSpacing(4)
                .foregroundStyle(palette.ink)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var attributed: AttributedString {
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .full
        options.failurePolicy = .returnPartiallyParsedIfPossible
        return (try? AttributedString(markdown: message.text, options: options))
            ?? AttributedString(message.text)
    }
}

struct Receipt {
    var symbol: String
    var label: String
    var tint: Color
    var showsProgress: Bool
}

/// Prototype activity: inline tool chips (`.readchips`) plus the collapsible
/// reasoning block (`.reason`). Tool target and duration are not supplied by
/// the host, so only the real title and outcome are shown.
struct ActivityDisclosure: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let activity: TurnActivity
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            if !activity.tools.isEmpty {
                toolChips
            }
            if !activity.thoughts.isEmpty {
                reasonBlock
            }
        }
        .padding(.top, 14)
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: expanded)
    }

    private var toolChips: some View {
        // A wrapping chip grid; the prototype wraps `.rc` chips with a 5pt gap.
        FlowLayout(spacing: 5) {
            ForEach(activity.tools) { tool in
                toolChip(tool)
            }
        }
    }

    private func toolChip(_ tool: ToolCard) -> some View {
        let tone = toolTone(tool)
        return HStack(spacing: 5) {
            Circle()
                .fill(tone.color(palette))
                .frame(width: 5, height: 5)
            Text(WorkspaceChatPresentation.toolCommand(tool.title))
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(1)
            Text(WorkspaceChatPresentation.outcome(tool.status))
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(tone.color(palette))
        }
        .padding(.horizontal, 7)
        .frame(height: 22)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .stroke(tool.needsAttention && tool.status == "failed" ? palette.danger : palette.stroke, lineWidth: 1)
        }
        .contextMenu {
            Button {
                UIPasteboard.general.string = tool.title
            } label: {
                Label(String(localized: "Copy"), systemImage: "doc.on.doc")
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("\(WorkspaceChatPresentation.toolCommand(tool.title)), \(WorkspaceChatPresentation.outcome(tool.status))"))
    }

    private func toolTone(_ tool: ToolCard) -> R2Tone {
        NocturneR2Presentation.toolTone(tool.status)
    }

    private var reasonBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion)) {
                    expanded.toggle()
                }
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    Text(reasonLabel)
                        .font(.system(size: 10, design: .monospaced))
                    Spacer(minLength: 0)
                }
                .foregroundStyle(palette.inkTertiary)
                .frame(minHeight: 32)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(reasonLabel))
            .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))
            .accessibilityHint(Text(expanded ? "Collapse" : "Expand"))

            if expanded {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(activity.thoughts.enumerated()), id: \.element.id) { index, thought in
                        Text("\(index + 1) — \(thought.text)")
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(palette.inkTertiary)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .transition(.opacity)
            }
        }
        .padding(.leading, 10)
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(palette.strokeStrong)
                .frame(width: 1)
        }
    }

    private var reasonLabel: String {
        NocturneR2Presentation.reasonLabel(thoughtCount: activity.thoughtCount, active: activity.active)
    }
}

/// Prototype `.perm`: amber header strip, body, deny + allow buttons, and the
/// "always allow" affordance. The card leaves the stack only after the host
/// acknowledges the reply (owned by ChatStore + ChatView).
struct PermissionCardView: View {
    @Environment(\.sePalette) private var palette
    let card: PermissionCard
    let submission: CardSubmission?
    var onChoose: (String?) -> Void
    @State private var lastChoice: String?
    @State private var hasChosen = false

    private var isSubmitting: Bool { submission == .submitting }

    private var retry: (() -> Void)? {
        guard hasChosen else { return nil }
        return { onChoose(lastChoice) }
    }

    private func choose(_ option: String?) {
        lastChoice = option
        hasChosen = true
        onChoose(option)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 7) {
                Text(String(localized: "Permission required"))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.warning)
                Spacer(minLength: 4)
                if isSubmitting {
                    ProgressView()
                        .controlSize(.mini)
                        .tint(palette.warning)
                } else {
                    Text(optionSummary)
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(palette.warning.opacity(0.75))
                }
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.warningSubtle)

            VStack(alignment: .leading, spacing: 8) {
                Text(card.title)
                    .font(.system(size: 12))
                    .foregroundStyle(palette.inkSecondary)
                    .lineSpacing(3)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)

                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 7) { actions }
                    VStack(spacing: 7) { actions }
                }
                .disabled(isSubmitting)

                if case .failed(let message) = submission {
                    SeErrorBanner(message: message, retry: retry)
                    if !hasChosen {
                        Text("Choose an option to retry.")
                            .font(SeFont.caption)
                            .foregroundStyle(palette.inkSecondary)
                    }
                }
                if isSubmitting {
                    Text("Waiting for host…")
                        .font(.system(size: 10))
                        .foregroundStyle(palette.inkTertiary)
                }
            }
            .padding(11)
        }
        .background(palette.card)
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(palette.warning.opacity(0.42), lineWidth: 1)
        }
        .seEntrance()
    }

    private var optionSummary: String {
        NocturneR2Presentation.optionSummary(card.options.count)
    }

    @ViewBuilder
    private var actions: some View {
        Button(String(localized: "Deny")) { choose(nil) }
            .frame(maxWidth: .infinity, minHeight: 40)
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(palette.inkSecondary)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .stroke(palette.strokeStrong, lineWidth: 1)
            }
            .buttonStyle(.plain)
        ForEach(card.options) { option in
            Button(option.name) { choose(option.id) }
                .frame(maxWidth: .infinity, minHeight: 40)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(palette.canvas)
                .background(palette.ink, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .buttonStyle(.plain)
        }
    }
}

/// Prototype permission-card frame reused for agent questions. The question
/// title and host-provided choices are the only content.
struct QuestionCardView: View {
    @Environment(\.sePalette) private var palette
    let card: QuestionCard
    let submission: CardSubmission?
    var onChoose: (String) -> Void
    @State private var lastChoice: String?

    private var isSubmitting: Bool { submission == .submitting }

    private var retry: (() -> Void)? {
        guard let lastChoice else { return nil }
        return { onChoose(lastChoice) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 7) {
                Text(String(localized: "Question"))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.accent)
                Spacer(minLength: 4)
                if isSubmitting {
                    ProgressView()
                        .controlSize(.mini)
                        .tint(palette.accent)
                }
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.accentSubtle)

            VStack(alignment: .leading, spacing: 8) {
                Text(card.question)
                    .font(.system(size: 12))
                    .foregroundStyle(palette.ink)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 7) { choices }
                    VStack(spacing: 7) { choices }
                }
                .disabled(isSubmitting)
                if isSubmitting {
                    Text("Waiting for host…")
                        .font(.system(size: 10))
                        .foregroundStyle(palette.inkTertiary)
                }
                if case .failed(let message) = submission {
                    SeErrorBanner(message: message, retry: retry)
                }
            }
            .padding(11)
        }
        .background(palette.card)
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(palette.accent.opacity(0.42), lineWidth: 1)
        }
        .seEntrance()
    }

    @ViewBuilder
    private var choices: some View {
        ForEach(card.options) { option in
            Button(option.label) {
                lastChoice = option.id
                onChoose(option.id)
            }
            .frame(maxWidth: .infinity, minHeight: 40)
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(palette.canvas)
            .background(palette.ink, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .buttonStyle(.plain)
        }
    }
}

/// Minimal wrapping layout for the prototype's `.readchips` row.
struct FlowLayout: Layout {
    var spacing: CGFloat = 5

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var rowWidth: CGFloat = 0
        var rowHeight: CGFloat = 0
        var totalHeight: CGFloat = 0
        var maxWidth: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if rowWidth > 0, rowWidth + spacing + size.width > width {
                totalHeight += rowHeight + spacing
                maxWidth = max(maxWidth, rowWidth)
                rowWidth = size.width
                rowHeight = size.height
            } else {
                rowWidth += (rowWidth > 0 ? spacing : 0) + size.width
                rowHeight = max(rowHeight, size.height)
            }
        }
        totalHeight += rowHeight
        maxWidth = max(maxWidth, rowWidth)
        return CGSize(width: proposal.width ?? maxWidth, height: totalHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}
