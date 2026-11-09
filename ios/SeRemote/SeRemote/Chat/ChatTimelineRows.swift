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

struct ActivityDisclosure: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let activity: TurnActivity
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: SeSpace.xs) {
            Button { expanded.toggle() } label: {
                HStack(spacing: SeSpace.xs) {
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .seAnimation(.chevron, value: expanded)
                        .transaction { if reduceMotion { $0.animation = nil } }
                    Text(activity.summary)
                        .font(.subheadline.weight(.medium))
                    Spacer(minLength: 0)
                }
                .foregroundStyle(activity.attentionRequired ? palette.warning : palette.inkSecondary)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(activity.summary))
            .accessibilityHint(Text(expanded ? "Collapse" : "Expand"))
            .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))

            if expanded, !activity.thoughts.isEmpty {
                ThoughtBlock(messages: activity.thoughts)
                    .transition(.opacity)
            }
            // Every execution keeps its command/outcome summary visible.
            // Folding only compacts successful titles; errors never truncate.
            ForEach(activity.tools) { tool in
                WorkspaceToolSummary(tool: tool, expanded: expanded)
                    .transition(.opacity)
            }
        }
        .padding(.vertical, SeSpace.xxs)
        .seAnimation(.disclosure, value: expanded)
    }
}

struct WorkspaceToolSummary: View {
    @Environment(\.sePalette) private var palette
    let tool: ToolCard
    var expanded = false

    private var status: SeStatus {
        switch tool.status {
        case "failed": .failed
        case "completed", "success": .completed
        case "pending", "in_progress": .working
        default: .idle
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: SeSpace.xs) {
            HStack(alignment: .top, spacing: SeSpace.xs) {
                Image(systemName: "terminal")
                    .foregroundStyle(palette.inkSecondary)
                Text(WorkspaceChatPresentation.toolCommand(tool.title))
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(expanded || tool.needsAttention ? nil : 2)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            ViewThatFits(in: .horizontal) {
                HStack { outcome; metadata }
                VStack(alignment: .leading, spacing: SeSpace.xxs) { outcome; metadata }
            }
        }
        .padding(SeSpace.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.codeCanvas, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                .stroke(tool.status == "failed" ? palette.danger : palette.strokeStrong, lineWidth: 1)
        }
    }

    private var outcome: some View {
        SeStatusBadge(status: status)
            .accessibilityLabel(WorkspaceChatPresentation.outcome(tool.status))
    }

    private var metadata: some View {
        Text("Target unavailable · Duration unavailable")
            .font(.caption)
            .foregroundStyle(palette.inkTertiary)
    }
}

struct ThoughtBlock: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let messages: [ChatMessage]
    @State private var expanded = false

    private var text: String { messages.map(\.text).joined() }

    var body: some View {
        VStack(alignment: .leading, spacing: SeSpace.xs) {
            Button { expanded.toggle() } label: {
                HStack(spacing: SeSpace.xs) {
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .seAnimation(.chevron, value: expanded)
                        .transaction { if reduceMotion { $0.animation = nil } }
                    Text("Reasoning")
                    Spacer()
                }
                .font(.footnote.weight(.medium))
                .foregroundStyle(palette.inkSecondary)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))
            if expanded {
                Text(text)
                    .font(.footnote)
                    .foregroundStyle(palette.inkSecondary)
                    .textSelection(.enabled)
                    .padding(.leading, SeSpace.md)
                    .transaction { $0.animation = nil }
                    .transition(.opacity)
            }
        }
        .seAnimation(.disclosure, value: expanded)
    }
}

struct MessageBubble: View {
    @Environment(\.sePalette) private var palette
    let message: ChatMessage
    var onEdit: (() -> Void)?
    var onRetry: (() -> Void)?

    var body: some View {
        HStack {
            if message.role == .user { Spacer(minLength: 48) }
            VStack(alignment: message.role == .user ? .trailing : .leading, spacing: 4) {
                // Equatable content: streaming token updates re-render only
                // the changed bubble, so unchanged rows skip the markdown
                // reparse instead of every visible bubble reparsing per token.
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
            if message.role != .user { Spacer(minLength: 0) }
        }
        .contextMenu { messageActions }
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

    private var attributed: AttributedString {
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .full
        options.failurePolicy = .returnPartiallyParsedIfPossible
        return (try? AttributedString(markdown: message.text, options: options))
            ?? AttributedString(message.text)
    }
}

/// Equatable bubble payload — equality over the render-affecting fields so
/// `.equatable()` can skip unchanged rows (actions stay on the wrapper).
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
        VStack(alignment: message.role == .user ? .trailing : .leading, spacing: 6) {
            Text(attributed)
                .font(.body)
                .textSelection(.enabled)
            if message.streaming {
                ProgressView()
                    .controlSize(.mini)
            }
        }
        .padding(message.role == .user ? SeSpace.sm : SeSpace.xxs)
        .frame(maxWidth: message.role == .user ? nil : .infinity, alignment: .leading)
        .background(message.role == .user ? palette.ink : palette.canvas)
        .foregroundStyle(message.role == .user ? palette.canvas : palette.ink)
        .clipShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
        .transaction { $0.animation = nil }
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
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            HStack(alignment: .top) {
                Label("Permission required", systemImage: "lock.shield")
                    .font(.headline)
                    .foregroundStyle(palette.warning)
                Spacer(minLength: SeSpace.xs)
                if isSubmitting {
                    ProgressView().accessibilityLabel(Text("Submitting"))
                } else {
                    Text("Pending").font(.caption).foregroundStyle(palette.warning)
                }
            }
            Text(card.title)
                .font(.subheadline)
                .foregroundStyle(palette.inkSecondary)
                .textSelection(.enabled)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: SeSpace.xs) { choices }
                VStack(spacing: SeSpace.xs) { choices }
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
                    .font(.caption)
                    .foregroundStyle(palette.inkSecondary)
            }
        }
        .padding(SeSpace.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card).stroke(palette.warning.opacity(0.5), lineWidth: 1)
        }
    }

    @ViewBuilder
    private var choices: some View {
        Button("Deny") { choose(nil) }
            .frame(maxWidth: .infinity, minHeight: 44)
            .buttonStyle(NocturneSessionButtonStyle())
        ForEach(card.options) { option in
            Button(option.name) { choose(option.id) }
                .frame(maxWidth: .infinity, minHeight: 44)
                .buttonStyle(NocturneSessionButtonStyle(primary: true))
        }
    }
}

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
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            Label(card.question, systemImage: "questionmark.bubble")
                .font(.headline)
            ForEach(card.options) { option in
                Button(option.label) {
                    lastChoice = option.id
                    onChoose(option.id)
                }
                .buttonStyle(NocturneSessionButtonStyle())
                .disabled(isSubmitting)
            }
            if isSubmitting {
                ProgressView("Waiting for host…")
                    .font(SeFont.caption)
            }
            if case .failed(let message) = submission {
                SeErrorBanner(message: message, retry: retry)
                if lastChoice == nil {
                    Text("Choose an answer to retry.")
                        .font(SeFont.caption)
                        .foregroundStyle(palette.inkSecondary)
                }
            }
        }
        .padding(SeSpace.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
    }
}
