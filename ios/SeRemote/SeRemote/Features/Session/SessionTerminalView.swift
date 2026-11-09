import SwiftUI

// MARK: - Model

enum TerminalLineKind: String, Sendable {
    case plain
    case dim
    case command
    case success
    case warning
    case danger
}

struct SessionTerminalLine: Identifiable, Equatable, Sendable {
    let id: String
    let text: String
    var kind: TerminalLineKind = .plain
}

struct SessionTerminalTab: Identifiable, Equatable, Sendable {
    let id: String
    let index: Int
    let title: String
}

// MARK: - Screen

/// Terminal mode of the session workbench. The canvas keeps its own dark
/// surface in both themes; only the chrome follows day/night.
struct SessionTerminalView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var title: String
    var host: String
    var state: String
    var columns: String
    var tabs: [SessionTerminalTab]
    var activeTabID: String
    var lines: [SessionTerminalLine]

    var onBack: () -> Void = {}
    var onSelectMode: (WorkspaceTab) -> Void = { _ in }
    var onSelectTab: (SessionTerminalTab) -> Void = { _ in }
    var onAddTab: () -> Void = {}
    var onFit: () -> Void = {}
    var onAdjustFont: (Int) -> Void = { _ in }
    var onSend: (String) -> Void = { _ in }
    var onKey: (String) -> Void = { _ in }

    @State private var draft: String = ""

    var body: some View {
        VStack(spacing: 0) {
            header
            modeSwitch
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, SeSpace.md)
            metaRow
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, 10)
            tabStrip
            terminal
            keyRow
        }
        .background(palette.canvas.ignoresSafeArea())
        .safeAreaInset(edge: .bottom, spacing: 0) {
            dock
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
                Text(verbatim: "\(host) · \(state)")
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
                let isOn = tab == .terminal
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

    private var metaRow: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(palette.live)
                .frame(width: 6, height: 6)
            Text("PTY · Ready")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkSecondary)
            Spacer(minLength: 8)
            Text(verbatim: columns)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
            smallButton("FIT", action: onFit)
            smallButton("A+", action: { onAdjustFont(1) })
            smallButton("A−", action: { onAdjustFont(-1) })
        }
    }

    private var tabStrip: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 0) {
                ForEach(tabs) { tab in
                    let isOn = tab.id == activeTabID
                    Button {
                        onSelectTab(tab)
                    } label: {
                        HStack(spacing: 6) {
                            Text(verbatim: "\(tab.index)")
                                .font(SeNocturneType.monoSmall)
                                .opacity(0.6)
                            Text(verbatim: tab.title)
                                .font(SeNocturneType.font(SeNocturneType.labelSize))
                                .lineLimit(1)
                        }
                        .foregroundStyle(isOn ? palette.ink : palette.inkTertiary)
                        .padding(.horizontal, 11)
                        .frame(height: 34)
                        .overlay(alignment: .bottom) {
                            if isOn {
                                Rectangle()
                                    .fill(palette.accent)
                                    .frame(height: 2)
                                    .padding(.horizontal, 8)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }

                Button(action: onAddTab) {
                    Image(systemName: "plus")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(palette.inkTertiary)
                        .frame(width: 28, height: 28)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 7))
                        .overlay {
                            RoundedRectangle(cornerRadius: 7)
                                .strokeBorder(palette.stroke, lineWidth: 1)
                        }
                }
                .buttonStyle(.plain)
                .padding(.leading, 4)
            }
            .padding(.horizontal, 12)
        }
        .scrollIndicators(.hidden)
        .padding(.top, 10)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
        }
    }

    private var terminal: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(lines) { line in
                    Text(verbatim: line.text.isEmpty ? " " : line.text)
                        .font(SeNocturneType.font(SeNocturneType.monoSize))
                        .foregroundStyle(lineColor(line.kind))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                caret
            }
            .padding(.horizontal, 13)
            .padding(.vertical, 13)
        }
        .scrollIndicators(.hidden)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.terminalCanvas)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
        }
    }

    private var caret: some View {
        Rectangle()
            .fill(palette.terminalInk.opacity(0.7))
            .frame(width: 6, height: 12)
            .opacity(0.85)
            .accessibilityHidden(true)
    }

    private var keyRow: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(["Esc", "Tab", "Ctrl", "⌥", "↑", "↓", "←", "→", "⌘K"], id: \.self) { key in
                    Button {
                        onKey(key)
                    } label: {
                        Text(verbatim: key)
                            .font(SeNocturneType.monoSmall)
                            .foregroundStyle(palette.inkSecondary)
                            .padding(.horizontal, 9)
                            .frame(minWidth: 30, minHeight: 30)
                            .background(palette.card, in: RoundedRectangle(cornerRadius: 7))
                            .overlay {
                                RoundedRectangle(cornerRadius: 7)
                                    .strokeBorder(palette.stroke, lineWidth: 1)
                            }
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
        }
        .scrollIndicators(.hidden)
    }

    private var dock: some View {
        HStack(spacing: 8) {
            Text(verbatim: "$")
                .font(SeNocturneType.mono)
                .foregroundStyle(palette.live)
            TextField("Type a command…", text: $draft)
                .font(SeNocturneType.mono)
                .foregroundStyle(palette.ink)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.send)
                .onSubmit(sendDraft)
            Button(action: sendDraft) {
                Image(systemName: "arrow.up")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(palette.surface)
                    .frame(width: 30, height: 30)
                    .background(palette.ink, in: RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "Send"))
        }
        .padding(.horizontal, 11)
        .frame(height: 42)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.strip))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.strip)
                .strokeBorder(palette.strokeStrong, lineWidth: 1)
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

    private func smallButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(verbatim: title)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkSecondary)
                .padding(.horizontal, 6)
                .frame(height: 20)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 5))
                .overlay {
                    RoundedRectangle(cornerRadius: 5)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
    }

    private func sendDraft() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        onSend(text)
        draft = ""
    }

    private func lineColor(_ kind: TerminalLineKind) -> Color {
        switch kind {
        case .plain: palette.terminalInk
        case .dim: palette.terminalInk.opacity(0.55)
        case .command: palette.accent
        case .success: palette.live
        case .warning: palette.warning
        case .danger: palette.danger
        }
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

#Preview("Terminal · Night") {
    SessionTerminalView(
        title: "zsh — Termul",
        host: "MacBook Pro",
        state: "termul · main",
        columns: "80 × 24",
        tabs: [
            SessionTerminalTab(id: "1", index: 1, title: "zsh"),
            SessionTerminalTab(id: "2", index: 2, title: "dev"),
            SessionTerminalTab(id: "3", index: 3, title: "test")
        ],
        activeTabID: "1",
        lines: [
            SessionTerminalLine(id: "l0", text: "Last login: Sat Nov  9 09:38:02 on ttys004", kind: .dim),
            SessionTerminalLine(id: "l1", text: "➜ ~/termul on main via swift 6.1", kind: .success),
            SessionTerminalLine(id: "l2", text: "$ swift run SeRemote --remote", kind: .command),
            SessionTerminalLine(id: "l3", text: "[3/3] Linking SeRemote", kind: .dim),
            SessionTerminalLine(id: "l4", text: "✓ 284 ms · 42 files", kind: .success),
            SessionTerminalLine(id: "l5", text: "[PTY] session retained across reconnect", kind: .dim),
            SessionTerminalLine(id: "l6", text: "[PTY] reconnect backoff 1s → 2s → 4s", kind: .warning),
            SessionTerminalLine(id: "l7", text: "$ swift test --filter Reconnect", kind: .command),
            SessionTerminalLine(id: "l8", text: "✔ test_resume_keeps_session (0.014s)", kind: .success),
            SessionTerminalLine(id: "l9", text: "✘ test_backoff_caps_at_8s (0.009s)", kind: .danger),
            SessionTerminalLine(id: "l10", text: "Executed 2 tests, with 1 failure", kind: .dim)
        ]
    )
    .seThemeMode(.night)
}

#Preview("Terminal · Day") {
    SessionTerminalView(
        title: "zsh — Termul",
        host: "MacBook Pro",
        state: "termul · main",
        columns: "80 × 24",
        tabs: [SessionTerminalTab(id: "1", index: 1, title: "zsh")],
        activeTabID: "1",
        lines: [
            SessionTerminalLine(id: "l0", text: "$ npm run dev:web", kind: .command),
            SessionTerminalLine(id: "l1", text: "VITE v6.2.0 ready in 284 ms", kind: .success)
        ]
    )
    .seThemeMode(.day)
}
