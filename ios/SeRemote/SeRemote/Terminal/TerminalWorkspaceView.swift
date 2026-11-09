import SwiftUI

struct TerminalWorkspaceView: View {
    @Environment(\.sePalette) private var palette
    @Bindable var session: WorkspaceSession
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
                terminalStatusBar
                hostScreenObservation
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
            } else {
                SeEmptyState(
                    title: String(localized: "No live terminal"),
                    systemImage: "apple.terminal",
                    message: String(localized: "Open a terminal on the desktop, or start a new one here."),
                    actionTitle: canSpawnTerminal ? String(localized: "New Terminal") : nil,
                    action: canSpawnTerminal ? { Task { await session.spawnTerminal() } } : nil
                )
            }
            TerminalInputDock(
                isEnabled: session.terminals.activeId != nil,
                isKeyboardVisible: session.terminalKeyboardVisible,
                displayMode: session.terminals.displayMode,
                textScale: textScale,
                controlModifier: $controlModifier,
                onSend: sendTerminalInput,
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
            .zIndex(1)
        }
        .background(SeTheme.codeCanvas)
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
                focusToken &+= 1
            default:
                break
            }
        }
    }

    private var terminalStatusBar: some View {
        HStack(spacing: SeSpace.xs) {
            Circle()
                .fill(session.terminals.isConnecting ? palette.warning : (session.terminalSocket.isConnected ? palette.live : palette.inkSecondary))
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
            Text(session.terminals.isConnecting ? String(localized: "Connecting…") : "PTY")
                .font(.caption.monospaced())
                .foregroundStyle(palette.inkSecondary)
            Spacer(minLength: SeSpace.xs)
            if let dimensions = SessionTerminalPresentation.dimensions(cols: activeTerminal?.cols, rows: activeTerminal?.rows) {
                Text(dimensions)
                    .font(.caption.monospaced())
                    .foregroundStyle(palette.inkSecondary)
                    .lineLimit(1)
            }
            Button {
                Task { await session.terminals.setDisplayMode(.phone) }
            } label: {
                Text("FIT").font(.caption.monospaced())
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            }
            .accessibilityLabel(Text("Phone Fit"))
            .accessibilityValue(Text(session.terminals.displayMode == .phone ? String(localized: "Active") : String(localized: "Idle")))
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
            } label: {
                Text("A+").font(.caption.monospaced())
                    .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            }
            .accessibilityLabel(Text("Text Size"))
        }
        .buttonStyle(.plain)
        .foregroundStyle(palette.ink)
        .padding(.horizontal, SeSpace.md)
        .background(palette.canvas)
        .overlay(alignment: .bottom) {
            Rectangle().fill(palette.strokeStrong).frame(height: 1)
        }
        .seHaptic(trigger: session.terminals.displayMode) { _, _ in .selection }
        .seHaptic(trigger: textScale) { _, _ in .selection }
    }

    private var activeTerminal: LiveTerminal? {
        session.terminals.terminals.first(where: { $0.id == session.terminals.activeId })
    }

    /// A host snapshot, not agent activity, the engine preference, or the
    /// phone renderer's health. Only the refresh button issues screen_status.
    private var hostScreenObservation: some View {
        let observation = activeTerminal?.screenObservation ?? .legacyHost
        return HStack(alignment: .top, spacing: 12) {
            DisclosureGroup {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Read-only · last host report; not live monitoring")
                    Text("Applied \(observation.appliedSeq) / captured \(observation.capturedSeq)")
                        .monospacedDigit()
                    if let buildId = observation.observerBuildId {
                        Text("Observer build: \(buildId)")
                            .textSelection(.enabled)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Host screen observation")
                        .font(.caption.weight(.semibold))
                    Text(observationSummary(observation))
                        .font(.caption)
                        .foregroundStyle(observation.isReady ? Color.green : Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Button {
                Task { await session.terminals.refreshScreenObservation() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .frame(minWidth: 44, minHeight: 44)
            }
            .disabled(session.terminals.isRefreshingScreenObservation)
            .accessibilityLabel(Text("Refresh host screen observation"))
            .accessibilityHint(Text("Read-only status request for the current terminal"))
            .accessibilityIdentifier("refresh-host-screen-observation")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .background(SeTheme.surface)
        .accessibilityIdentifier("host-screen-observation")
    }

    private func observationSummary(_ observation: HostScreenObservation) -> String {
        let summary = "\(observation.terminalScreenEngine.rawValue) · \(observation.state.rawValue)"
        if let reason = observation.reason {
            return "\(summary) · \(reason.rawValue)"
        }
        return summary
    }

    private var canSpawnTerminal: Bool {
        session.workspace == .conversation || session.workspace == .project
    }

    private func sendTerminalInput(_ text: String) {
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
        .padding(SeSpace.md)
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
