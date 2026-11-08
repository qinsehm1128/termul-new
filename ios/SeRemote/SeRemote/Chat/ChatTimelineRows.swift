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
                .foregroundStyle(activity.attentionRequired ? SeTheme.warning : SeTheme.secondaryInk)
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
                    .foregroundStyle(SeTheme.secondaryInk)
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
        .background(SeTheme.codeCanvas, in: RoundedRectangle(cornerRadius: SeRadius.tool))
        .overlay {
            RoundedRectangle(cornerRadius: SeRadius.tool)
                .stroke(tool.status == "failed" ? SeTheme.danger : SeTheme.stroke, lineWidth: 1)
        }
    }

    private var outcome: some View {
        SeStatusBadge(status: status)
            .accessibilityLabel(WorkspaceChatPresentation.outcome(tool.status))
    }

    private var metadata: some View {
        Text("Target unavailable · Duration unavailable")
            .font(.caption)
            .foregroundStyle(SeTheme.tertiaryInk)
    }
}

struct ThoughtBlock: View {
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
                .foregroundStyle(SeTheme.secondaryInk)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(Text(expanded ? "Expanded" : "Collapsed"))
            if expanded {
                Text(text)
                    .font(.footnote)
                    .foregroundStyle(SeTheme.secondaryInk)
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
                tint: SeTheme.danger,
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
        .background(message.role == .user ? SeTheme.accentSubtle : SeTheme.canvas)
        .foregroundStyle(SeTheme.ink)
        .clipShape(RoundedRectangle(cornerRadius: SeRadius.content, style: .continuous))
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
    let card: PermissionCard
    var isSubmitting = false
    var onChoose: (String?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            HStack(alignment: .top) {
                Label("Permission required", systemImage: "lock.shield")
                    .font(.headline)
                Spacer(minLength: SeSpace.xs)
                if isSubmitting {
                    ProgressView().accessibilityLabel(Text("Submitting"))
                } else {
                    Text("Pending").font(.caption).foregroundStyle(SeTheme.warning)
                }
            }
            Text(card.title)
                .font(.subheadline)
                .foregroundStyle(SeTheme.secondaryInk)
                .textSelection(.enabled)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: SeSpace.xs) { choices }
                VStack(spacing: SeSpace.xs) { choices }
            }
            .disabled(isSubmitting)
            if isSubmitting {
                Text("Waiting for host…")
                    .font(.caption)
                    .foregroundStyle(SeTheme.secondaryInk)
            }
        }
        .padding(SeSpace.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SeTheme.warningSubtle, in: RoundedRectangle(cornerRadius: SeRadius.content))
        .overlay {
            RoundedRectangle(cornerRadius: SeRadius.content).stroke(SeTheme.warning.opacity(0.5), lineWidth: 1)
        }
    }

    @ViewBuilder
    private var choices: some View {
        Button("Deny") { onChoose(nil) }
            .frame(maxWidth: .infinity, minHeight: 44)
            .buttonStyle(SeSecondaryButtonStyle())
        ForEach(card.options) { option in
            Button(option.name) { onChoose(option.id) }
                .frame(maxWidth: .infinity, minHeight: 44)
                .buttonStyle(SePrimaryButtonStyle())
        }
    }
}

struct QuestionCardView: View {
    let card: QuestionCard
    var onChoose: (String) -> Void
    @State private var submitted = false

    var body: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            Label(card.question, systemImage: "questionmark.bubble")
                .font(.headline)
            ForEach(card.options) { option in
                Button(option.label) {
                    guard !submitted else { return }
                    submitted = true
                    onChoose(option.id)
                }
                .buttonStyle(SeSecondaryButtonStyle())
                .disabled(submitted)
            }
        }
        .padding(SeSpace.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SeTheme.surface, in: RoundedRectangle(cornerRadius: SeRadius.content))
    }
}
