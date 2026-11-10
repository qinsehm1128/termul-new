import SwiftUI
import UIKit

/// Where a captured region is delivered.
nonisolated enum DesktopShotTarget: Equatable, Sendable {
    /// ACP conversation chat; the image becomes an inline content block.
    case chat(conversationID: String?)
    /// A raw PTY; the staged path is typed into the terminal.
    case terminal(id: String)
    /// A terminal-agent chat; the staged path lands in the composer draft.
    case agentChat(id: String)

    var isChat: Bool {
        if case .chat = self { return true }
        return false
    }
}

/// Resolves the send target from the workspace surface. The sheet entries pass
/// explicit targets, but the mapping is kept pure and testable.
nonisolated enum DesktopShotTargetResolver {
    static func resolve(
        activeTerminalID: String?,
        activeConversationID: String?,
        agentChatTerminalID: String?,
        terminalTabActive: Bool = false
    ) -> DesktopShotTarget? {
        if let agentChatTerminalID {
            return .agentChat(id: agentChatTerminalID)
        }
        // The visible workbench decides: on the terminal tab the shot goes to
        // the PTY even when the session also has a conversation.
        if terminalTabActive, let activeTerminalID {
            return .terminal(id: activeTerminalID)
        }
        if let activeConversationID {
            return .chat(conversationID: activeConversationID)
        }
        if let activeTerminalID {
            return .terminal(id: activeTerminalID)
        }
        return nil
    }
}

nonisolated enum DesktopShotSendError: Error, Equatable {
    case chat(ChatSendFailure)
    case noTerminal

    /// Localized sentence for the notice pill. Host text never reaches the UI.
    var message: String {
        switch self {
        case let .chat(failure): failure.message
        case .noTerminal: String(localized: "The screenshot could not be sent.")
        }
    }
}

/// Delivers captures through the routes each session type already supports.
@MainActor
struct DesktopShotSender {
    let http: HostHTTP
    var chat: ChatStore? = nil
    var terminals: TerminalStore? = nil
    var conversation: HostConversation? = nil
    /// Injected by `TerminalAgentChatContainer` to place the staged path into
    /// the agent-chat composer draft instead of the PTY.
    var draftAgentChat: ((String, String) -> Void)?

    func send(_ shot: ShotAttachment, to target: DesktopShotTarget) async throws {
        switch target {
        case .chat:
            guard let chat else { throw DesktopShotSendError.chat(.notConnected) }
            if let failure = await chat.sendImage(
                shot,
                caption: String(localized: "Screenshot from desktop"),
                in: conversation
            ) {
                throw DesktopShotSendError.chat(failure)
            }
        case let .terminal(id):
            guard let terminals else { throw DesktopShotSendError.noTerminal }
            let staged = try await http.stageAttachment(shot)
            await terminals.open(id)
            await terminals.write(DesktopShellPath.stagedArgument(staged.path))
        case let .agentChat(id):
            let staged = try await http.stageAttachment(shot)
            draftAgentChat?(id, DesktopShellPath.stagedArgument(staged.path))
        }
    }
}

/// Sheet chrome per §6.2/§6.3: a 44pt bar, the existing desktop canvas, and a
/// viewfinder mode that dims the frame and sends the selected region.
struct DesktopQuickSheet: View {
    let hostName: String
    let sender: DesktopShotSender
    let target: DesktopShotTarget
    var startsInViewfinder: Bool = false

    @Environment(\.dismiss) private var dismiss
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// The sheet streams on its own socket: closing it must never stop the
    /// Screen page's live desktop, which keeps `WorkspaceSession.desktopSocket`.
    @State private var socket = DesktopSocket()
    @State private var viewfinder: Bool
    @State private var selection = CGRect.zero
    @State private var fit = true
    @State private var sending = false
    @State private var notice: Notice?
    @State private var opened = false

    init(
        hostName: String,
        sender: DesktopShotSender,
        target: DesktopShotTarget,
        startsInViewfinder: Bool = false
    ) {
        self.hostName = hostName
        self.sender = sender
        self.target = target
        self.startsInViewfinder = startsInViewfinder
        _viewfinder = State(initialValue: startsInViewfinder)
    }

    private struct Notice: Identifiable {
        let id = UUID()
        let message: String
        let isError: Bool
    }

    private var imageSize: CGSize {
        guard let image = socket.image else { return .zero }
        return CGSize(width: image.width, height: image.height)
    }

    private var pixelLabel: String {
        guard imageSize.width > 0 else { return "—" }
        return "\(Int(imageSize.width)) × \(Int(imageSize.height))"
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            GeometryReader { proxy in
                let bounds = CGRect(origin: .zero, size: proxy.size)
                ZStack(alignment: .top) {
                    Color.black
                    DesktopCanvasView(
                        image: socket.image,
                        displayID: socket.selectedDisplay,
                        mode: .trackpad,
                        acceptsInput: !viewfinder,
                        allowsNavigation: !viewfinder,
                        zoom: zoom(in: bounds),
                        onInput: { socket.send($0) },
                        onKeyboard: {}
                    )
                    if viewfinder, imageSize.width > 0 {
                        DesktopViewfinderOverlay(
                            selection: $selection,
                            imageSize: imageSize,
                            bounds: bounds,
                            zoom: zoom(in: bounds)
                        )
                        .transition(.opacity)
                    }
                    if socket.image == nil {
                        VStack(spacing: 10) {
                            ProgressView().tint(palette.inkSecondary)
                            Text("Waiting for the desktop…")
                                .font(SeNocturneType.font(11, mono: true))
                                .foregroundStyle(palette.inkTertiary)
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    if let notice {
                        noticePill(notice)
                            .padding(.top, 12)
                            .transition(.move(edge: .top).combined(with: .opacity))
                    }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 2, style: .continuous))
            footer
        }
        .background(palette.surface)
        .presentationDetents([.large])
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(SeNocturneRadius.sheet)
        .interactiveDismissDisabled(sending)
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: viewfinder)
        .task {
            socket.configure(http: sender.http)
            await socket.open()
            opened = true
            seedSelectionIfNeeded()
        }
        .onChange(of: imageSize) { _, _ in seedSelectionIfNeeded() }
        .onDisappear {
            if opened { socket.close() }
        }
    }

    // MARK: - Bars

    private var topBar: some View {
        Group {
            if viewfinder {
                viewfinderBar
            } else {
                operateBar
            }
        }
        .frame(height: 44)
        .padding(.horizontal, 12)
        .background(palette.card)
        .overlay(alignment: .bottom) { Rectangle().fill(palette.stroke).frame(height: 1) }
    }

    private var operateBar: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(socket.image == nil ? palette.inkTertiary : palette.live)
                .frame(width: 6, height: 6)
            Text(hostName)
                .font(SeNocturneType.font(12, mono: true))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            Text(pixelLabel)
                .font(SeNocturneType.font(9, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 6)
            Button {
                setViewfinder(true)
            } label: {
                Text("Viewfinder")
                    .font(SeNocturneType.font(11, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .padding(.horizontal, 8)
                    .frame(minHeight: 30)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            .accessibilityLabel(Text("Viewfinder"))
            closeButton
        }
    }

    private var viewfinderBar: some View {
        HStack(spacing: 9) {
            Text("Viewfinder")
                .font(SeNocturneType.font(11, weight: .semibold))
                .foregroundStyle(palette.ink)
            Text("\(Int(selection.width)) × \(Int(selection.height))")
                .font(SeNocturneType.font(10, mono: true))
                .foregroundStyle(palette.inkTertiary)
                .contentTransition(.numericText())
            Spacer(minLength: 4)
            Button("Cancel") {
                setViewfinder(false)
            }
            .buttonStyle(.plain)
            .font(SeNocturneType.font(12, weight: .medium))
            .foregroundStyle(palette.inkSecondary)
            .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
            SeSolidButton(
                title: sending ? String(localized: "Sending…") : String(localized: "Send to agent"),
                icon: .send
            ) {
                Task { await captureAndSend() }
            }
            .frame(width: 148)
            .disabled(sending || imageSize.width == 0)
            .opacity(sending || imageSize.width == 0 ? 0.5 : 1)
            .accessibilityLabel(Text("Send to agent"))
        }
    }

    private var closeButton: some View {
        Button {
            dismiss()
        } label: {
            SeIconView(icon: .close, size: 13, stroke: 1.6, color: palette.inkSecondary)
                .frame(width: 30, height: 30)
                .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
        .accessibilityLabel(Text("Close"))
    }

    private var footer: some View {
        HStack(spacing: 8) {
            R2FitChip(label: String(localized: "FIT"), isActive: fit) { fit = true }
            R2FitChip(label: "1:1", isActive: !fit) { fit = false }
            Spacer()
            Text(viewfinder
                 ? "\(Int(selection.width)) × \(Int(selection.height)) px"
                 : pixelLabel)
                .font(SeNocturneType.font(9, mono: true))
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(palette.card)
        .overlay(alignment: .top) { Rectangle().fill(palette.stroke).frame(height: 1) }
    }

    private func noticePill(_ notice: Notice) -> some View {
        HStack(spacing: 8) {
            SeIconView(
                icon: notice.isError ? .info : .check,
                size: 13,
                stroke: 1.8,
                color: notice.isError ? palette.warning : palette.live
            )
            Text(notice.message)
                .font(SeNocturneType.font(11.5, weight: .medium))
                .foregroundStyle(palette.ink)
                .multilineTextAlignment(.leading)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .stroke(palette.stroke, lineWidth: 1)
        }
        .padding(.horizontal, 16)
        .accessibilityElement(children: .combine)
    }

    // MARK: - Actions

    private func setViewfinder(_ active: Bool) {
        withAnimation(reduceMotion ? nil : SeNocturneMotion.quick) {
            viewfinder = active
            if active { seedSelectionIfNeeded() }
        }
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    private func seedSelectionIfNeeded() {
        guard selection.isEmpty || selection.width < 1 || selection.height < 1 else { return }
        guard imageSize.width > 0, imageSize.height > 0 else { return }
        selection = DesktopShotGeometry.defaultSelection(in: imageSize)
    }

    private func zoom(in bounds: CGRect) -> CGFloat {
        guard !fit, imageSize.width > 0, imageSize.height > 0,
              bounds.width > 0, bounds.height > 0 else { return 1 }
        let fitScale = min(bounds.width / imageSize.width, bounds.height / imageSize.height)
        guard fitScale > 0 else { return 1 }
        return min(max(1 / fitScale, 1), 8)
    }

    private func captureAndSend() async {
        guard !sending else { return }
        sending = true
        defer { sending = false }
        guard let capture = await captureSelection() else {
            show(Notice(message: String(localized: "The desktop frame is not ready yet."), isError: true))
            return
        }
        guard let cropped = DesktopShotCropper.crop(capture.image, to: capture.rect) else {
            show(Notice(message: String(localized: "The screenshot could not be prepared."), isError: true))
            return
        }
        let shot: ShotAttachment?
        if target.isChat {
            let inline = await DesktopShotEncoder.encodeForChatDetached(cropped)
            shot = inline ?? DesktopShotEncoder.encodePNG(cropped)
        } else {
            shot = DesktopShotEncoder.encodePNG(cropped)
        }
        guard let shot else {
            show(Notice(message: String(localized: "The screenshot could not be prepared."), isError: true))
            return
        }
        do {
            try await sender.send(shot, to: target)
            show(Notice(message: String(localized: "Screenshot sent to the agent."), isError: false))
            try? await Task.sleep(for: .milliseconds(1200))
            dismiss()
        } catch let error as DesktopShotSendError {
            show(Notice(message: error.message, isError: true))
        } catch let error as AttachmentStageError {
            show(Notice(message: error.message, isError: true))
        } catch {
            show(Notice(message: String(localized: "The screenshot could not be sent."), isError: true))
        }
    }

    /// The region the user selected, applied to a frame no older than about 2s.
    /// A stale stream frame is replaced by a fresh screenshot, and the selection
    /// is scaled onto that image's pixel size so the crop still matches it.
    private func captureSelection() async -> (image: CGImage, rect: CGRect)? {
        let displayed = imageSize
        if DesktopFrameFreshness.isFresh(updatedAt: socket.imageUpdatedAt), let frame = socket.image {
            return (frame, DesktopShotGeometry.prepared(selection, in: displayed))
        }
        guard let display = socket.selectedDisplay,
              let data = try? await sender.http.desktopScreenshot(display: display, format: "png", quality: 100),
              let image = UIImage(data: data)?.cgImage else { return nil }
        let size = CGSize(width: image.width, height: image.height)
        let rect = DesktopShotGeometry.scaledSelection(selection, from: displayed, to: size)
        return (image, DesktopShotGeometry.prepared(rect, in: size))
    }

    private func show(_ notice: Notice) {
        withAnimation(reduceMotion ? nil : SeNocturneMotion.quick) { self.notice = notice }
    }
}

/// Dimmed frame with the drag/resize selection. Coordinates are image pixels;
/// the view rect is derived through the same viewport the canvas uses.
private struct DesktopViewfinderOverlay: View {
    @Binding var selection: CGRect
    let imageSize: CGSize
    let bounds: CGRect
    let zoom: CGFloat

    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var origin: CGRect?
    @State private var activeHandle: Handle?

    private enum Handle: CaseIterable {
        case topLeft, top, topRight, left, right, bottomLeft, bottom, bottomRight

        var affectsLeft: Bool { self == .topLeft || self == .left || self == .bottomLeft }
        var affectsRight: Bool { self == .topRight || self == .right || self == .bottomRight }
        var affectsTop: Bool { self == .topLeft || self == .top || self == .topRight }
        var affectsBottom: Bool { self == .bottomLeft || self == .bottom || self == .bottomRight }
    }

    private var frameRect: CGRect {
        DesktopShotGeometry.viewRect(fromImage: selection, in: bounds, imageSize: imageSize, zoom: zoom)
    }

    var body: some View {
        ZStack {
            // Swallows drags that miss a handle, so the canvas can never pan the
            // frame while the selection is being edited.
            Rectangle()
                .fill(Color.clear)
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0))
                .accessibilityHidden(true)

            Path { path in
                path.addRect(bounds)
                path.addRect(frameRect)
            }
            .fill(Color.black.opacity(0.45), style: FillStyle(eoFill: true))
            .allowsHitTesting(false)

            Path { path in
                path.addRect(frameRect)
            }
            .stroke(palette.accent, lineWidth: 1.5)
            .shadow(color: palette.accentSubtle, radius: 3)
            .allowsHitTesting(false)

            Rectangle()
                .fill(Color.clear)
                .frame(width: max(frameRect.width, 1), height: max(frameRect.height, 1))
                .position(x: frameRect.midX, y: frameRect.midY)
                .contentShape(Rectangle())
                .gesture(moveGesture)

            ForEach(Array(Handle.allCases.enumerated()), id: \.offset) { _, handle in
                handleView(handle)
            }
        }
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: frameRect)
    }

    private func handleView(_ handle: Handle) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: 4, style: .continuous)
                .fill(palette.accent)
                .frame(width: 14, height: 14)
                .overlay {
                    RoundedRectangle(cornerRadius: 4, style: .continuous)
                        .stroke(palette.canvas, lineWidth: 1)
                }
        }
        .frame(width: 44, height: 44)
        .contentShape(Rectangle())
        .position(handlePoint(handle))
        .gesture(resizeGesture(handle))
        .accessibilityHidden(true)
    }

    private func handlePoint(_ handle: Handle) -> CGPoint {
        switch handle {
        case .topLeft: CGPoint(x: frameRect.minX, y: frameRect.minY)
        case .top: CGPoint(x: frameRect.midX, y: frameRect.minY)
        case .topRight: CGPoint(x: frameRect.maxX, y: frameRect.minY)
        case .left: CGPoint(x: frameRect.minX, y: frameRect.midY)
        case .right: CGPoint(x: frameRect.maxX, y: frameRect.midY)
        case .bottomLeft: CGPoint(x: frameRect.minX, y: frameRect.maxY)
        case .bottom: CGPoint(x: frameRect.midX, y: frameRect.maxY)
        case .bottomRight: CGPoint(x: frameRect.maxX, y: frameRect.maxY)
        }
    }

    private var moveGesture: some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { value in
                if origin == nil {
                    origin = selection
                    activeHandle = nil
                }
                guard let origin else { return }
                let delta = imageDelta(value.translation)
                selection = DesktopShotGeometry.prepared(
                    origin.offsetBy(dx: delta.width, dy: delta.height),
                    in: imageSize
                )
            }
            .onEnded { _ in
                origin = nil
                activeHandle = nil
            }
    }

    private func resizeGesture(_ handle: Handle) -> some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { value in
                if origin == nil {
                    origin = selection
                    activeHandle = handle
                }
                guard let origin else { return }
                selection = resized(origin, handle: handle, by: imageDelta(value.translation))
            }
            .onEnded { _ in
                origin = nil
                activeHandle = nil
            }
    }

    private func imageDelta(_ translation: CGSize) -> CGSize {
        let frame = DesktopShotGeometry.imageFrame(in: bounds, imageSize: imageSize, zoom: zoom)
        guard frame.width > 0, frame.height > 0,
              imageSize.width > 0, imageSize.height > 0 else { return .zero }
        return CGSize(
            width: translation.width / frame.width * imageSize.width,
            height: translation.height / frame.height * imageSize.height
        )
    }

    private func resized(_ start: CGRect, handle: Handle, by delta: CGSize) -> CGRect {
        var minX = start.minX
        var maxX = start.maxX
        var minY = start.minY
        var maxY = start.maxY

        if handle.affectsLeft { minX += delta.width }
        if handle.affectsRight { maxX += delta.width }
        if handle.affectsTop { minY += delta.height }
        if handle.affectsBottom { maxY += delta.height }

        let minSide = DesktopShotGeometry.minSide(for: imageSize)
        if maxX - minX < minSide {
            if handle.affectsLeft {
                minX = maxX - minSide
            } else {
                maxX = minX + minSide
            }
        }
        if maxY - minY < minSide {
            if handle.affectsTop {
                minY = maxY - minSide
            } else {
                maxY = minY + minSide
            }
        }
        return DesktopShotGeometry.prepared(
            CGRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY),
            in: imageSize
        )
    }
}
