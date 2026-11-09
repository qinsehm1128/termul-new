import SwiftUI

// MARK: - Model

struct SessionChatMessage: Identifiable, Equatable, Sendable {
    enum Role: String, Sendable {
        case user
        case agent
    }

    let id: String
    let role: Role
    let text: String
    var files: [String] = []
    var reasoning: [String] = []
    var timestamp: String = ""
}

struct SessionChatPermission: Equatable, Sendable {
    let title: String
    let path: String
    let diff: [String]
    let scope: String
}

// MARK: - Screen

/// The session workbench in chat mode. Terminal and files are the same
/// session, not separate destinations: the mode switch stays in the header.
struct SessionChatView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var title: String
    var host: String
    var agent: String
    var model: String
    var elapsed: String
    var readableFiles: Int
    var messages: [SessionChatMessage]
    var permission: SessionChatPermission?

    var onBack: () -> Void = {}
    var onSelectMode: (WorkspaceTab) -> Void = { _ in }
    var onAllow: () -> Void = {}
    var onDeny: () -> Void = {}
    var onSend: (String) -> Void = { _ in }
    var onQuickAction: (String) -> Void = { _ in }

    @State private var draft: String = ""
    @State private var openReasoning: Set<String> = []

    var body: some View {
        VStack(spacing: 0) {
            header
            modeSwitch
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, SeSpace.md)
            statusRibbon
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, 10)
            timeline
        }
        .background(palette.canvas.ignoresSafeArea())
        .safeAreaInset(edge: .bottom, spacing: 0) {
            composer
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            iconButton(systemImage: "chevron.left", label: String(localized: "Back"), action: onBack)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: title)
                    .font(SeNocturneType.font(SeNocturneType.secondarySize + 1.5, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: "\(host) · \(agent) · \(model)")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            iconButton(systemImage: "ellipsis", label: String(localized: "More"), action: {})
        }
        .padding(.horizontal, SeSpace.lg)
        .padding(.top, SeSpace.xs)
    }

    private var modeSwitch: some View {
        HStack(spacing: 3) {
            ForEach(WorkspaceTab.allCases) { tab in
                let isOn = tab == .chat
                Button {
                    guard !isOn else { return }
                    onSelectMode(tab)
                } label: {
                    Text(tab.title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: isOn ? .medium : .regular))
                        .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                        .frame(maxWidth: .infinity)
                        .frame(height: 30)
                        .background(
                            isOn ? palette.ink : .clear,
                            in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var statusRibbon: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(palette.live)
                .frame(width: 6, height: 6)
            Text("Live")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.live)
            Text(verbatim: elapsed)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 8)
            Text(readableFiles == 1 ? "1 file readable" : "\(readableFiles) files readable")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var timeline: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(messages) { message in
                    messageView(message)
                }
                if let permission {
                    permissionCard(permission)
                }
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.top, SeSpace.md)
            .padding(.bottom, SeSpace.lg)
        }
        .scrollIndicators(.hidden)
    }

    @ViewBuilder
    private func messageView(_ message: SessionChatMessage) -> some View {
        switch message.role {
        case .user:
            HStack {
                Spacer(minLength: 34)
                Text(verbatim: message.text)
                    .font(SeNocturneType.font(SeNocturneType.bodySize))
                    .foregroundStyle(palette.surface)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 11)
                    .background(palette.ink, in: RoundedRectangle(cornerRadius: 14))
            }
        case .agent:
            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 7) {
                    Text("Se")
                        .font(SeNocturneType.font(9, weight: .semibold, mono: true))
                        .foregroundStyle(palette.onAccent)
                        .frame(width: 20, height: 20)
                        .background(palette.accent, in: RoundedRectangle(cornerRadius: 6))
                    Text(verbatim: agent)
                        .font(SeNocturneType.font(SeNocturneType.labelSize, weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Text(verbatim: message.timestamp)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                }
                Text(verbatim: message.text)
                    .font(SeNocturneType.font(SeNocturneType.bodySize))
                    .foregroundStyle(palette.ink)
                    .fixedSize(horizontal: false, vertical: true)

                if !message.files.isEmpty {
                    HStack(spacing: 5) {
                        ForEach(message.files, id: \.self) { file in
                            fileChip(file)
                        }
                    }
                }

                if !message.reasoning.isEmpty {
                    reasoningDisclosure(message)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func fileChip(_ file: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: "doc")
                .font(.system(size: 9))
                .foregroundStyle(palette.inkTertiary)
            Text(verbatim: file)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkSecondary)
        }
        .padding(.horizontal, 7)
        .frame(height: 22)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private func reasoningDisclosure(_ message: SessionChatMessage) -> some View {
        let isOpen = openReasoning.contains(message.id)
        return VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion)) {
                    if isOpen { openReasoning.remove(message.id) } else { openReasoning.insert(message.id) }
                }
            } label: {
                HStack(spacing: 5) {
                    Text("Reasoning")
                        .font(SeNocturneType.monoSmall)
                    Text(verbatim: "\(message.reasoning.count) steps")
                        .font(SeNocturneType.monoSmall)
                        .opacity(0.7)
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                }
                .foregroundStyle(palette.inkTertiary)
            }
            .buttonStyle(.plain)

            if isOpen {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(message.reasoning.enumerated()), id: \.offset) { index, step in
                        Text(verbatim: "\(index + 1) — \(step)")
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(palette.inkTertiary)
                    }
                }
                .padding(.leading, 10)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(palette.strokeStrong)
                        .frame(width: 1)
                }
                .transition(.opacity)
            }
        }
        .padding(.leading, 10)
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(palette.stroke)
                .frame(width: 1)
        }
    }

    private func permissionCard(_ permission: SessionChatPermission) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(permission.title)
                    .font(SeNocturneType.font(SeNocturneType.labelSize, weight: .semibold))
                    .foregroundStyle(palette.warning)
                Spacer()
                Text(permission.scope)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.warning.opacity(0.8))
            }
            .padding(.horizontal, 11)
            .frame(height: 34)
            .background(palette.warningSubtle)

            VStack(alignment: .leading, spacing: 9) {
                Text(verbatim: permission.path)
                    .font(SeNocturneType.font(SeNocturneType.labelSize, weight: .medium))
                    .foregroundStyle(palette.ink)
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(permission.diff.enumerated()), id: \.offset) { _, line in
                        Text(verbatim: line)
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(diffColor(line))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(9)
                .background(palette.codeCanvas, in: RoundedRectangle(cornerRadius: 7))
                .overlay {
                    RoundedRectangle(cornerRadius: 7)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }

                HStack(spacing: 7) {
                    actionButton("Deny", isPrimary: false, action: onDeny)
                    actionButton("Allow once", isPrimary: true, action: onAllow)
                }
                Button(action: onAllow) {
                    Text("Always allow this file")
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                }
                .buttonStyle(.plain)
            }
            .padding(11)
        }
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.strip)
                .strokeBorder(palette.warning.opacity(0.34), lineWidth: 1)
        }
        .clipShape(RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
    }

    private func actionButton(_ title: LocalizedStringKey, isPrimary: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .semibold))
                .foregroundStyle(isPrimary ? palette.surface : palette.inkSecondary)
                .frame(maxWidth: .infinity)
                .frame(height: 36)
                .background(
                    isPrimary ? palette.ink : Color.clear,
                    in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                )
                .overlay {
                    if !isPrimary {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.strokeStrong, lineWidth: 1)
                    }
                }
        }
        .buttonStyle(.plain)
    }

    private var composer: some View {
        VStack(spacing: 8) {
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(["接着修", "跑测试", "看 diff", "换个思路"], id: \.self) { action in
                        Button {
                            onQuickAction(action)
                        } label: {
                            Text(verbatim: action)
                                .font(SeNocturneType.font(SeNocturneType.labelSize))
                                .foregroundStyle(palette.inkSecondary)
                                .padding(.horizontal, 9)
                                .frame(height: 26)
                                .background(palette.card, in: RoundedRectangle(cornerRadius: 7))
                                .overlay {
                                    RoundedRectangle(cornerRadius: 7)
                                        .strokeBorder(palette.stroke, lineWidth: 1)
                                }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .scrollIndicators(.hidden)

            HStack(alignment: .bottom, spacing: 8) {
                TextField("Continue this session…", text: $draft, axis: .vertical)
                    .font(SeNocturneType.font(SeNocturneType.bodySize))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1 ... 4)
                    .textInputAutocapitalization(.never)
                    .submitLabel(.send)
                    .onSubmit(sendDraft)

                HStack(spacing: 5) {
                    Circle()
                        .fill(palette.live)
                        .frame(width: 4, height: 4)
                    Text(verbatim: model)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkSecondary)
                }
                .padding(.horizontal, 7)
                .frame(height: 24)
                .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 6))
                .overlay {
                    RoundedRectangle(cornerRadius: 6)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }

                Button(action: sendDraft) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.surface)
                        .frame(width: 32, height: 32)
                        .background(palette.ink, in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "Send"))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 15))
            .overlay {
                RoundedRectangle(cornerRadius: 15)
                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
            }

            Text("Text only. Running commands still needs your permission.")
                .font(.system(size: 9))
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(palette.surface)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
        }
    }

    private func sendDraft() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        onSend(text)
        draft = ""
    }

    private func diffColor(_ line: String) -> Color {
        if line.hasPrefix("+") { return palette.live }
        if line.hasPrefix("-") { return palette.danger }
        return palette.codeInk
    }

    private func iconButton(systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Previews

#Preview("Night") {
    SessionChatView(
        title: "优化远程连接恢复",
        host: "MacBook Pro",
        agent: "Claude Code",
        model: "Sonnet 4.5",
        elapsed: "00:42",
        readableFiles: 3,
        messages: [
            SessionChatMessage(
                id: "u1",
                role: .user,
                text: "接着断开连接恢复逻辑，先把根因写清楚。"
            ),
            SessionChatMessage(
                id: "a1",
                role: .agent,
                text: "恢复连接应保留当前会话，而不是重建。原因在 TerminalSocket：重连时先把 socket 置空，sessionId 却没有绑定到新的 transport。",
                files: ["TerminalSocket.swift", "ReconnectPolicy.swift"],
                reasoning: ["reconnect 前 socket 被置空", "新的 transport 没有 resume(id:)"],
                timestamp: "ACP · 2 分钟前"
            )
        ],
        permission: SessionChatPermission(
            title: "需要你的许可",
            path: "修改 src/net/TerminalSocket.swift",
            diff: [
                "- session = createSession()",
                "+ guard let id = sessionID else { return nil }",
                "+ return resumeSession(id: id)"
            ],
            scope: "写 · 1 个文件"
        )
    )
    .seThemeMode(.night)
}

#Preview("Day") {
    SessionChatView(
        title: "优化远程连接恢复",
        host: "MacBook Pro",
        agent: "Claude Code",
        model: "Sonnet 4.5",
        elapsed: "00:42",
        readableFiles: 3,
        messages: [
            SessionChatMessage(id: "u1", role: .user, text: "接着断开连接恢复逻辑，先把根因写清楚。"),
            SessionChatMessage(
                id: "a1",
                role: .agent,
                text: "恢复连接应保留当前会话，而不是重建。",
                files: ["TerminalSocket.swift"],
                reasoning: ["先补失败用例"],
                timestamp: "ACP · 2 分钟前"
            )
        ],
        permission: nil
    )
    .seThemeMode(.day)
}
