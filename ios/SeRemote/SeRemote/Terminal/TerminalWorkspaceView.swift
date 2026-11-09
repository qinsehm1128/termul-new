import SwiftUI

/// Nocturne terminal surface (prototype `data-screen="terminal"`).
///
/// Chrome order matches the prototype: mono PTY meta line, the near-black term
/// canvas, then accessory keys + command dock. The SwiftTerm host subtree is
/// identity-stable: it is created once per terminal id and never recreated by
/// mode switches, rotation, size-class changes or text-scale changes.
struct TerminalWorkspaceView: View {
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette
    @Bindable var session: WorkspaceSession
    private var allowsInput: Bool { (deviceLink?.allows("terminal.input") ?? true) }
    @State private var focusToken: UInt64 = 0
    @State private var textScale = TerminalTextScale.current
    @State private var scaleHud: String?
    @State private var controlModifier = TerminalControlModifier()

    var body: some View {
        VStack(spacing: 0) {
            if let error = session.terminals.errorMessage {
                terminalErrorBanner(error)
            }
            if let id = session.terminals.activeId {
                terminalMeta
                terminalCanvas(id: id)
                if allowsInput {
                    TerminalInputDock(
                        isEnabled: true,
                        isKeyboardVisible: session.terminalKeyboardVisible,
                        controlModifier: $controlModifier,
                        onSend: sendTerminalInput,
                        onFocusTerminal: {
                            focusToken &+= 1
                        },
                        onDismissKeyboard: {
                            session.dismissTerminalKeyboard()
                        }
                    )
                    .padding(.bottom, 8)
                }
            } else {
                SeEmptyState(
                    title: String(localized: "No live terminal"),
                    systemImage: "apple.terminal",
                    message: String(localized: "Open a terminal on the desktop, or start a new one here."),
                    actionTitle: canSpawnTerminal ? String(localized: "New Terminal") : nil,
                    action: canSpawnTerminal ? { Task { await session.spawnTerminal() } } : nil
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.surface)
        .onChange(of: session.terminals.activeId) { _, _ in
            controlModifier = TerminalControlModifier()
        }
        .onAppear {
            session.activateTerminalGeometry()
        }
        .onDisappear {
            // The feed closure points at this mount's SwiftTerm view; clear it
            // so output buffers (deliverOutput's pending path) instead of
            // feeding a dead view between remounts (size-class swap).
            session.terminals.onFeed = nil
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
                if allowsInput { focusToken &+= 1 }
            default:
                break
            }
        }
    }

    // MARK: - Meta (prototype `.termmeta`)

    private var terminalMeta: some View {
        HStack(spacing: 8) {
            R2Dot(tone: session.terminals.isConnecting ? .attention : (session.terminalSocket.isConnected ? .live : .quiet))
            Text(terminalMetaLabel)
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 4)
            if let dimensions = SessionTerminalPresentation.dimensions(cols: activeTerminal?.cols, rows: activeTerminal?.rows) {
                Text(dimensions)
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            R2FitChip(label: "FIT", isActive: session.terminals.displayMode == .phone) {
                let next: TerminalDisplayMode = session.terminals.displayMode == .phone ? .desktop : .phone
                Task { await session.terminals.setDisplayMode(next) }
            }
            .accessibilityLabel(Text(session.terminals.displayMode == .phone
                ? "Switch to desktop fit" : "Fit the phone"))
            .accessibilityValue(Text(session.terminals.displayMode == .phone ? String(localized: "Active") : String(localized: "Idle")))
            textScaleMenu
        }
        .font(.system(size: 9, design: .monospaced))
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .frame(height: 44)
        .seHaptic(trigger: session.terminals.displayMode) { _, _ in .selection }
        .seHaptic(trigger: textScale) { _, _ in .selection }
    }

    private var terminalMetaLabel: String {
        NocturneR2Presentation.terminalMetaLabel(
            isConnecting: session.terminals.isConnecting,
            isConnected: session.terminalSocket.isConnected
        )
    }

    private var textScaleMenu: some View {
        Menu {
            Button { nudgeTextScale(-1) } label: {
                Label(String(localized: "Smaller Text"), systemImage: "textformat.size.smaller")
            }
            Button { nudgeTextScale(1) } label: {
                Label(String(localized: "Larger Text"), systemImage: "textformat.size.larger")
            }
            Button { applyTextScale(TerminalTextScale.defaultValue) } label: {
                Label(String(localized: "Reset Text Size"), systemImage: "arrow.counterclockwise")
            }
            Divider()
            Button {
                Task { await session.terminals.refreshScreenObservation() }
            } label: {
                Label(String(localized: "Refresh host screen observation"), systemImage: "arrow.clockwise")
            }
            .disabled(session.terminals.isRefreshingScreenObservation)
            .accessibilityHint(Text("Read-only status request for the current terminal"))
            .accessibilityIdentifier("refresh-host-screen-observation")
        } label: {
            Text("A+")
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(palette.inkSecondary)
                .padding(.horizontal, 6)
                .frame(minHeight: 28)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .stroke(palette.stroke, lineWidth: 1)
                }
                .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                .contentShape(Rectangle())
        }
        .accessibilityLabel(Text("Text Size"))
    }

    // MARK: - Term canvas (prototype `.term`)

    private func terminalCanvas(id: String) -> some View {
        ZStack {
            TerminalScreen(
                terminalId: id,
                buffered: session.terminals.pendingOutput[id] ?? Data(),
                hostCols: activeTerminal?.cols ?? 80,
                lockToHostCols: session.terminals.displayMode == .desktop,
                textScale: textScale,
                onSend: sendTerminalInput,
                onResize: { cols, rows in
                    session.terminals.resize(cols: cols, rows: rows)
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
            .transaction { $0.animation = nil }
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
        .background(palette.terminalCanvas)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var activeTerminal: LiveTerminal? {
        session.terminals.terminals.first(where: { $0.id == session.terminals.activeId })
    }

    private var canSpawnTerminal: Bool {
        (deviceLink?.allows("terminal.create") ?? true) && (session.workspace == .conversation || session.workspace == .project)
    }

    private func sendTerminalInput(_ text: String) {
        guard allowsInput else { return }
        let input = controlModifier.apply(to: text)
        Task { await session.terminals.write(input) }
    }

    /// Keep failures inline where the terminal lives, dismissible once read.
    private func terminalErrorBanner(_ error: String) -> some View {
        SeErrorBanner(
            message: error,
            retryTitle: String(localized: "Dismiss"),
            retry: { session.terminals.errorMessage = nil }
        )
        .padding(.horizontal, 18)
        .padding(.top, 8)
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
