import SwiftUI

struct TerminalWorkspaceView: View {
    @Bindable var session: WorkspaceSession
    @State private var focusToken: UInt64 = 0
    @State private var textScale = TerminalTextScale.current
    @State private var scaleHud: String?

    var body: some View {
        VStack(spacing: 0) {
            if let error = session.terminals.errorMessage {
                terminalErrorBanner(error)
            }
            if let id = session.terminals.activeId {
                ZStack {
                    TerminalScreen(
                        terminalId: id,
                        buffered: session.terminals.pendingOutput[id] ?? Data(),
                        hostCols: activeTerminal?.cols ?? 80,
                        lockToHostCols: session.terminals.displayMode == .desktop,
                        textScale: textScale,
                        onSend: { text in
                            Task { await session.terminals.write(text) }
                        },
                        onResize: { cols, rows in
                            Task { await session.terminals.resize(cols: cols, rows: rows) }
                        },
                        onReady: { feed in
                            session.terminals.onFeed = { incomingId, data in
                                if incomingId == id {
                                    feed(data)
                                }
                            }
                        },
                        onTextScaleChange: { scale, settled in
                            textScale = scale
                            showScaleHud(scale)
                            if settled {
                                TerminalTextScale.current = scale
                            }
                        },
                        focusToken: focusToken,
                        blurToken: session.terminalBlurToken
                    )
                    .id(id)
                    .onAppear {
                        session.terminals.pendingOutput[id] = nil
                    }
                    if let scaleHud {
                        Text(scaleHud)
                            .font(.subheadline.weight(.semibold).monospaced())
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(.ultraThinMaterial, in: Capsule())
                            .allowsHitTesting(false)
                    }
                }
            } else {
                ContentUnavailableView(
                    "No live terminal",
                    systemImage: "apple.terminal",
                    description: Text("Open a terminal on the desktop, or start a new one here.")
                )
                if session.workspace == .conversation || session.workspace == .project {
                    Button {
                        Task { await session.spawnTerminal() }
                    } label: {
                        Text("New terminal")
                            .frame(minHeight: 44)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(SeTheme.accent)
                    .padding(.bottom, 24)
                }
            }
            TerminalInputDock(
                isEnabled: session.terminals.activeId != nil,
                isKeyboardVisible: session.terminalKeyboardVisible,
                displayMode: session.terminals.displayMode,
                textScale: textScale,
                onSend: { text in
                    Task { await session.terminals.write(text) }
                },
                onFocusTerminal: {
                    focusToken &+= 1
                },
                onDismissKeyboard: {
                    session.dismissTerminalKeyboard()
                },
                onToggleDisplayMode: {
                    Task {
                        let next: TerminalDisplayMode =
                            session.terminals.displayMode == .phone ? .desktop : .phone
                        await session.terminals.setDisplayMode(next)
                    }
                },
                onNudgeTextScale: { direction in
                    let next = TerminalTextScale.nudge(textScale, by: direction)
                    textScale = next
                    TerminalTextScale.current = next
                    showScaleHud(next)
                    HostLog.session.info("Terminal text scale \(next, privacy: .public)")
                }
            )
            .offset(y: session.terminalKeyboardVisible ? -session.terminalKeyboardHeight : 0)
            .zIndex(1)
        }
        .onAppear {
            // The wide layout mounts the terminal without a tab switch, so
            // geometry activation has to happen here.
            if session.isWideLayout {
                session.terminals.geometryActive = true
            }
        }
        .onReceive(ShortcutCenter.shortcuts) { shortcut in
            switch shortcut {
            case .textScaleUp:
                nudgeTextScale(1)
            case .textScaleDown:
                nudgeTextScale(-1)
            case .textScaleReset:
                applyTextScale(TerminalTextScale.defaultValue)
            case .focusTerminal:
                focusToken &+= 1
            default:
                break
            }
        }
    }

    private var activeTerminal: LiveTerminal? {
        session.terminals.terminals.first(where: { $0.id == session.terminals.activeId })
    }

    /// Terminal failures used to surface only in a distant workspace alert;
    /// keep them inline where the terminal lives, dismissible once read.
    private func terminalErrorBanner(_ error: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
            Text(error)
                .font(.footnote)
                .lineLimit(2)
            Spacer(minLength: 8)
            Button {
                session.terminals.errorMessage = nil
            } label: {
                Image(systemName: "xmark")
                    .font(.footnote.weight(.semibold))
                    .frame(minWidth: 32, minHeight: 32)
            }
            .accessibilityLabel(Text("Dismiss"))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(SeTheme.surface)
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
        .accessibilityIdentifier("terminal-error")
    }

    private func showScaleHud(_ scale: CGFloat) {
        scaleHud = "\(Int((scale * 100).rounded()))%"
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(900))
            if scaleHud == "\(Int((scale * 100).rounded()))%" {
                scaleHud = nil
            }
        }
    }

    private func nudgeTextScale(_ direction: Int) {
        applyTextScale(TerminalTextScale.nudge(textScale, by: direction))
    }

    private func applyTextScale(_ scale: CGFloat) {
        textScale = scale
        TerminalTextScale.current = scale
        showScaleHud(scale)
        HostLog.session.info("Terminal text scale \(scale, privacy: .public)")
    }
}
