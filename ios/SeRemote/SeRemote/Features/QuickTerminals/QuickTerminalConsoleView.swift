import SwiftUI

/// Full-screen live view of one host PTY.
///
/// The console attaches through `TerminalStore.attachViewer`, so it shares the
/// session's single `TerminalSocket` and the workbench's SwiftTerm canvas. It
/// never spawns: the host PTY already exists, and detaching leaves it running.
///
/// Quick terminals pass the claim `open` returned; project terminals pass no
/// claim and attach through the claim-free `watch` op instead.
struct QuickTerminalConsoleView: View {
    @Environment(\.sePalette) private var palette

    let session: WorkspaceSession
    let terminalId: String
    let claim: String?
    let title: String
    let unattachableNotice: String?
    let onClose: () -> Void

    init(
        session: WorkspaceSession,
        opened: QuickTerminalOpenedWire,
        title: String,
        onClose: @escaping () -> Void
    ) {
        self.init(
            session: session,
            terminalId: opened.terminalId,
            claim: QuickTerminalPresentation.attachClaim(opened),
            title: title,
            unattachableNotice: QuickTerminalPresentation.consoleUnattachableNotice(opened),
            onClose: onClose
        )
    }

    init(
        session: WorkspaceSession,
        terminalId: String,
        claim: String?,
        title: String,
        unattachableNotice: String? = nil,
        onClose: @escaping () -> Void
    ) {
        self.session = session
        self.terminalId = terminalId
        self.claim = claim
        self.title = title
        self.unattachableNotice = unattachableNotice
        self.onClose = onClose
    }

    @State private var viewer: TerminalPTYViewer?
    @State private var attachError: String?
    @State private var attempting = false
    @State private var textScale = TerminalTextScale.current
    @State private var controlModifier = TerminalControlModifier()
    @State private var focusToken: UInt64 = 0

    var body: some View {
        VStack(spacing: 0) {
            header
            if let viewer {
                meta(viewer)
                if let error = viewer.errorMessage {
                    SeErrorBanner(
                        message: error,
                        retryTitle: String(localized: "Dismiss"),
                        retry: { viewer.errorMessage = nil }
                    )
                    .padding(.horizontal, 14)
                    .padding(.bottom, 6)
                }
                canvas(viewer)
                TerminalInputDock(
                    isEnabled: viewer.isAttached && !viewer.hasExited,
                    isKeyboardVisible: session.terminalKeyboardVisible,
                    controlModifier: $controlModifier,
                    onSend: { text in viewer.send(controlModifier.apply(to: text)) },
                    onFocusTerminal: { focusToken &+= 1 },
                    onDismissKeyboard: { session.dismissTerminalKeyboard() }
                )
                .padding(.bottom, 8)
            } else {
                placeholder
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(palette.canvas.ignoresSafeArea())
        .task { await start() }
        .onDisappear { release() }
        .onChange(of: terminalId) { _, _ in
            controlModifier = TerminalControlModifier()
        }
    }

    // MARK: - Chrome

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: title)
                    .font(SeNocturneType.font(12, weight: .medium, mono: true))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: statusLine)
                    .font(SeNocturneType.font(9, weight: .regular, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 6)
            Button(action: onClose) {
                HStack(spacing: 5) {
                    Image(systemName: "rectangle.portrait.and.arrow.right")
                        .font(.system(size: 12, weight: .semibold))
                    Text("Detach")
                        .font(SeNocturneType.font(10, weight: .medium))
                }
                .foregroundStyle(palette.ink)
                .padding(.horizontal, 10)
                .frame(minHeight: 30)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .strokeBorder(palette.strokeStrong, lineWidth: 1)
                }
            }
            .buttonStyle(.plain)
            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            .accessibilityLabel(Text("Detach terminal"))
            .accessibilityHint(Text("Leaves the shell running on the host"))
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 6)
        .background(palette.surface)
    }

    private var statusLine: String {
        guard let viewer else {
            return attachError == nil
                ? String(localized: "Attaching to the host shell…")
                : String(localized: "Not attached")
        }
        if viewer.hasExited { return String(localized: "Shell exited") }
        return viewer.isAttached
            ? String(localized: "Attached")
            : String(localized: "Attaching to the host shell…")
    }

    private func meta(_ viewer: TerminalPTYViewer) -> some View {
        HStack(spacing: 8) {
            R2Dot(tone: viewer.hasExited ? .quiet : (viewer.isAttached ? .live : .attention))
            Text(verbatim: viewer.hasExited
                 ? String(localized: "Exited")
                 : (viewer.isAttached ? String(localized: "Attached") : String(localized: "Attaching")))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 4)
            if let dimensions = SessionTerminalPresentation.dimensions(cols: viewer.cols, rows: viewer.rows) {
                Text(verbatim: dimensions)
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
        }
        .font(.system(size: 9, design: .monospaced))
        .padding(.horizontal, 16)
        .padding(.bottom, 4)
    }

    // MARK: - Canvas

    private func canvas(_ viewer: TerminalPTYViewer) -> some View {
        ZStack {
            TerminalScreen(
                terminalId: viewer.terminalId,
                buffered: Data(),
                hostCols: viewer.cols,
                lockToHostCols: viewer.locksToHostCols,
                textScale: textScale,
                onSend: { text in viewer.send(controlModifier.apply(to: text)) },
                onResize: { cols, rows in viewer.resize(cols: cols, rows: rows) },
                onReady: { feed in viewer.install(feed) },
                onTextScaleChange: { scale, settled in
                    textScale = scale
                    if settled { TerminalTextScale.current = scale }
                },
                focusToken: focusToken
            )
            .id(viewer.terminalId)
            .transaction { $0.animation = nil }
            if viewer.hasExited {
                Text("Shell exited")
                    .font(SeNocturneType.font(10, weight: .medium, mono: true))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(.ultraThinMaterial, in: Capsule())
                    .allowsHitTesting(false)
            }
        }
        .padding(13)
        .background(palette.terminalCanvas)
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .stroke(palette.strokeStrong, lineWidth: 1)
        }
        .padding(.horizontal, 14)
        .frame(maxHeight: .infinity)
    }

    // MARK: - Attach state

    @ViewBuilder
    private var placeholder: some View {
        VStack(spacing: 14) {
            if let attachError {
                SeErrorBanner(
                    message: attachError,
                    retryTitle: String(localized: "Try again"),
                    retry: { retry() }
                )
                if let notice = unattachableNotice {
                    Text(verbatim: notice)
                        .font(SeNocturneType.font(10, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                        .multilineTextAlignment(.center)
                }
                Button(action: onClose) {
                    Text("Back to quick terminals")
                        .font(SeNocturneType.font(10, weight: .medium))
                        .foregroundStyle(palette.ink)
                        .padding(.horizontal, 12)
                        .frame(minHeight: NocturneToolMetrics.minimumTarget)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .strokeBorder(palette.strokeStrong, lineWidth: 1)
                        }
                }
                .buttonStyle(.plain)
            } else {
                ProgressView()
                Text("Attaching to the host shell…")
                    .font(SeNocturneType.font(10, weight: .regular, mono: true))
                    .foregroundStyle(palette.inkSecondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }

    private func start() async {
        guard viewer == nil, !attempting else { return }
        attempting = true
        defer { attempting = false }
        attachError = nil
        do {
            let created = try await session.terminals.attachViewer(
                terminalId: terminalId,
                claim: claim,
                title: title,
                cols: 80,
                rows: 24
            )
            guard !Task.isCancelled else {
                await session.terminals.releaseViewer(created)
                return
            }
            viewer = created
            focusToken &+= 1
        } catch {
            guard !Task.isCancelled else { return }
            attachError = Self.failureMessage(unattachableNotice: unattachableNotice, error: error)
        }
    }

    private func retry() {
        attachError = nil
        Task { await start() }
    }

    /// A refused claim is named only when the host actually answered
    /// UNAUTHORIZED; a transport failure keeps its own message.
    private static func failureMessage(unattachableNotice: String?, error: Error) -> String {
        if case HostError.ipc(_, let code) = error, code == "UNAUTHORIZED",
           let unattachableNotice {
            return unattachableNotice
        }
        return error.localizedDescription
    }

    /// Detach only: the host shell is never killed.
    private func release() {
        guard let viewer else { return }
        self.viewer = nil
        Task { await session.terminals.releaseViewer(viewer) }
    }
}

/// Identifiable payload so the quick-terminal list can present the console.
nonisolated struct QuickTerminalConsoleRequest: Identifiable, Hashable, Sendable {
    let opened: QuickTerminalOpenedWire
    let title: String

    var id: String { opened.terminalId }
}
