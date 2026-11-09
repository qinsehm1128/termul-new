import SwiftUI
import UIKit

struct DesktopFeatureView: View {
    let session: WorkspaceSession
    @Environment(\.remoteDeviceLink) private var deviceLink
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var theme
    @Environment(\.scenePhase) private var scenePhase
    @State private var interactionMode: DesktopInteractionMode = UIDevice.current.userInterfaceIdiom == .pad ? .direct : .trackpad
    @State private var keyboardVisible = false
    @State private var helpVisible = false
    @State private var screenshotBusy = false
    @State private var screenshotError: String?
    @State private var share: DesktopScreenshotShare?
    @State private var isVisible = false
    @State private var previousIdleTimerDisabled = false

    private var socket: DesktopSocket { session.desktopSocket }
    private var allowsView: Bool { deviceLink?.allows("screen.view") ?? true }
    private var allowsControl: Bool { deviceLink?.allows("screen.control") ?? true }
    private var streaming: Bool { socket.state == .streaming && scenePhase == .active }
    private var hostAllowsControl: Bool {
        socket.controlEnabled && (socket.info?.permissions.accessibility ?? false)
    }
    private var canEnableControl: Bool { allowsView && allowsControl && hostAllowsControl && streaming }
    private var acceptsInput: Bool {
        DesktopInteractionPolicy.canControl(deviceAllowed: allowsView && allowsControl,
            hostAllowed: hostAllowsControl, active: socket.controlActive, streaming: streaming)
    }

    var body: some View {
        VStack(spacing: 0) {
            statusBar
            controls
            ZStack {
                Rectangle().fill(theme.isNight ? Color.black : theme.insetFill)
                DesktopCanvasView(image: socket.image, displayID: socket.selectedDisplay,
                    mode: interactionMode, acceptsInput: acceptsInput, onInput: send,
                    onKeyboard: { keyboardVisible.toggle() })
                if socket.image == nil { emptyState }
                if socket.image != nil && socket.state == .reconnecting {
                    VStack {
                        Label("Reconnecting… Last frame", systemImage: "arrow.clockwise")
                            .font(SeNocturneType.font(12))
                            .padding(10)
                            .background(theme.sheetSurface.opacity(0.95), in: Capsule())
                            .padding(12)
                        Spacer()
                    }
                    .allowsHitTesting(false)
                }
            }
            .clipped()
            .overlay(alignment: .bottomLeading) {
                if socket.image != nil && !acceptsInput {
                    Text("View only")
                        .font(SeNocturneType.font(11, mono: true))
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(theme.sheetSurface.opacity(0.92), in: Capsule())
                        .padding(10)
                        .allowsHitTesting(false)
                }
            }
            .overlay(alignment: .topLeading) {
                DesktopKeyboardBridge(visible: $keyboardVisible, enabled: acceptsInput, onInput: send)
                    .frame(width: 1, height: 1)
                    .opacity(0.01)
                    .accessibilityHidden(!keyboardVisible)
            }
            guidance
        }
        .foregroundStyle(palette.ink)
        .tint(palette.accent)
        .background(theme.sheetSurface)
        .navigationTitle(Text("Desktop"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .toolbarBackground(theme.barSurface, for: .navigationBar)
        .task {
            guard allowsView else { return }
            await socket.open()
        }
        .onAppear {
            previousIdleTimerDisabled = UIApplication.shared.isIdleTimerDisabled
            isVisible = true
            updateIdleTimer()
        }
        .onDisappear {
            isVisible = false
            keyboardVisible = false
            socket.setControlActive(false)
            socket.close()
            updateIdleTimer()
            if share == nil { cleanupScreenshot() }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { relinquishControl() }
            updateIdleTimer()
        }
        .onChange(of: acceptsInput) { _, enabled in
            if !enabled { keyboardVisible = false }
        }
        .onChange(of: canEnableControl) { _, allowed in
            if !allowed { relinquishControl() }
        }
        .onChange(of: allowsView) { _, allowed in
            if !allowed { relinquishControl(); socket.close() }
            else { Task { await socket.open() } }
        }
        .sheet(isPresented: $helpVisible) { helpSheet }
        .sheet(item: $share, onDismiss: cleanupScreenshot) { item in
            DesktopShareSheet(url: item.url)
        }
    }

    private var statusBar: some View {
        HStack(spacing: 8) {
            Circle().fill(streaming ? palette.ink : palette.inkTertiary).frame(width: 6, height: 6)
                .accessibilityHidden(true)
            Text(statusTitle).font(SeNocturneType.font(11, mono: true))
                .accessibilityIdentifier("desktop.connectionStatus")
            Spacer(minLength: 8)
            if socket.displays.count >= 2 {
                Menu {
                    ForEach(socket.displays, id: \.id) { display in
                        Button {
                            relinquishControl()
                            socket.selectDisplay(display.id)
                        } label: {
                            if display.id == socket.selectedDisplay {
                                Label(display.name, systemImage: "checkmark")
                            } else { Text(display.name) }
                        }
                    }
                } label: {
                    Label(selectedDisplayName, systemImage: "display.2")
                        .font(SeNocturneType.font(12))
                        .lineLimit(1)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel(Text("Select display"))
                .accessibilityValue(selectedDisplayName)
                .accessibilityIdentifier("desktop.displayPicker")
            }
            iconButton("questionmark.circle", label: String(localized: "Desktop gesture help")) { helpVisible = true }
        }
        .padding(.horizontal, 14)
        .background(theme.barSurface)
        .accessibilityElement(children: .contain)
    }

    private var controls: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { controlToggle; Spacer(minLength: 0); modePicker; keyboardButton; screenshotButton }
            VStack(spacing: 4) {
                HStack { controlToggle; Spacer(); modePicker }
                HStack { Spacer(); keyboardButton; screenshotButton }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 6)
        .background(theme.barSurface)
    }

    private var controlToggle: some View {
        Toggle(isOn: Binding(get: { socket.controlActive }, set: { desired in
            keyboardVisible = false
            socket.setControlActive(desired && canEnableControl)
        })) {
            Text("Control").font(SeNocturneType.font(13))
        }
        .toggleStyle(.switch)
        .fixedSize()
        .frame(minHeight: 44)
        .disabled(!canEnableControl && !socket.controlActive)
        .accessibilityLabel(Text("Control remote desktop"))
        .accessibilityHint(Text("Requires host permission and this device’s screen control permission."))
        .help(guidanceMessage ?? String(localized: "Requires host permission and this device’s screen control permission."))
        .accessibilityIdentifier("desktop.controlToggle")
    }

    private var modePicker: some View {
        Menu {
            Picker("Interaction mode", selection: $interactionMode) {
                ForEach(DesktopInteractionMode.allCases) { mode in Text(mode.title).tag(mode) }
            }
        } label: {
            Label(interactionMode.title, systemImage: interactionMode == .trackpad ? "hand.draw" : "hand.tap")
                .font(SeNocturneType.font(12))
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .accessibilityLabel(Text("Interaction mode"))
        .accessibilityValue(interactionMode.title)
        .accessibilityIdentifier("desktop.modePicker")
        .onChange(of: interactionMode) { _, _ in keyboardVisible = false }
    }

    private var keyboardButton: some View {
        iconButton(keyboardVisible ? "keyboard.chevron.compact.down" : "keyboard",
                   label: keyboardVisible ? String(localized: "Hide keyboard") : String(localized: "Show keyboard")) {
            keyboardVisible.toggle()
        }
        .disabled(!acceptsInput)
        .accessibilityIdentifier("desktop.keyboard")
    }

    private var screenshotButton: some View {
        iconButton(screenshotBusy ? "hourglass" : "square.and.arrow.up", label: String(localized: "Share desktop screenshot")) {
            Task { await takeScreenshot() }
        }
        .disabled(screenshotBusy || !allowsView || socket.selectedDisplay == nil || socket.state == .revoked || socket.info?.supported == false || socket.info?.viewEnabled == false || socket.info?.permissions.screenRecording == false)
        .accessibilityIdentifier("desktop.screenshot")
    }

    private func iconButton(_ icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 17, weight: .medium))
                .frame(width: 44, height: 44)
                .background(theme.insetFill, in: RoundedRectangle(cornerRadius: 9))
                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(theme.insetStroke, lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(label)
    }

    @ViewBuilder private var guidance: some View {
        if let message = guidanceMessage {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "info.circle").accessibilityHidden(true)
                Text(message).font(SeNocturneType.font(12)).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                if allowsView && socket.state != .connecting && socket.state != .revoked &&
                    (socket.state != .streaming || socket.info?.permissions.accessibility == false) {
                    Button("Retry") { Task { socket.close(); await socket.open() } }
                        .frame(minWidth: 44, minHeight: 44)
                        .accessibilityLabel(Text("Reconnect remote desktop"))
                }
            }
            .foregroundStyle(palette.inkSecondary)
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(theme.insetFill)
            .accessibilityIdentifier("desktop.guidance")
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            if socket.state == .connecting || socket.state == .reconnecting { ProgressView() }
            else { Image(systemName: "display").font(.system(size: 34)).accessibilityHidden(true) }
            Text(statusTitle).font(SeNocturneType.font(18, weight: .semibold))
            Text("Your desktop appears here when the host starts sharing.")
                .font(SeNocturneType.font(13))
                .foregroundStyle(palette.inkSecondary)
                .multilineTextAlignment(.center)
        }
        .padding(28)
        .allowsHitTesting(false)
    }

    private var statusTitle: String {
        guard allowsView else { return String(localized: "Screen viewing not permitted") }
        switch socket.state {
        case .idle: return String(localized: "Desktop disconnected")
        case .connecting: return String(localized: "Connecting to desktop…")
        case .streaming: return acceptsInput ? String(localized: "Controlling") : String(localized: "Viewing desktop")
        case .reconnecting: return String(localized: "Reconnecting… Last frame")
        case .paused: return String(localized: "Desktop paused")
        case .revoked: return String(localized: "Desktop access revoked")
        }
    }

    private var selectedDisplayName: String {
        socket.displays.first { $0.id == socket.selectedDisplay }?.name ?? String(localized: "Select display")
    }

    private var guidanceMessage: String? {
        if !allowsView { return String(localized: "Ask the host owner to grant this device screen viewing permission.") }
        if socket.state == .revoked { return String(localized: "Access was revoked. Pair this device again from the host.") }
        if let screenshotError { return screenshotError }
        if let issue = socket.issue { return issue.message }
        if let info = socket.info {
            if !info.supported { return String(localized: "This host does not support remote desktop.") }
            if !info.viewEnabled { return String(localized: "Enable remote desktop viewing in the host’s settings.") }
            if !info.permissions.screenRecording { return String(localized: "On the Mac, allow Screen Recording in System Settings → Privacy & Security, then retry.") }
            if !allowsControl { return String(localized: "View only: ask the host owner to grant this device screen control permission.") }
            if !socket.controlEnabled { return String(localized: "View only: enable remote mouse and keyboard control on the host.") }
            if !info.permissions.accessibility { return String(localized: "View only: on the Mac, allow Accessibility in System Settings → Privacy & Security.") }
        }
        return nil
    }

    private var helpSheet: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Text("Desktop gestures").font(SeNocturneType.font(24, weight: .semibold)).accessibilityAddTraits(.isHeader)
                    Text("Trackpad: drag anywhere to move the cursor. Direct touch: tap the target on screen.")
                    Text("Tap: left click. Double tap: double click. Two-finger tap or long press: right click.")
                    Text("Tap, then hold and drag: drag with the left mouse button. Two-finger swipe: scroll. Three-finger tap: keyboard.")
                    Text("Pinch to zoom locally. In view-only mode, drag to pan the enlarged desktop. Each display remembers its zoom and position.")
                    Text("Control must be allowed on the host and for this device. The toggle only starts or stops this session’s control; it never changes host permissions.")
                    Text("Screenshots open the system share sheet. Save to Files or share from there; no photo library access is requested.")
                }
                .font(SeNocturneType.font(15))
                .foregroundStyle(palette.ink)
                .padding(24)
            }
            .background(theme.sheetSurface)
            .navigationTitle(Text("Desktop gesture help"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { helpVisible = false }.frame(minWidth: 44, minHeight: 44)
                }
            }
        }
    }

    private func send(_ message: DesktopInputMessage) {
        guard acceptsInput else { return }
        socket.send(message)
    }

    private func relinquishControl() {
        keyboardVisible = false
        socket.setControlActive(false)
    }

    private func updateIdleTimer() {
        UIApplication.shared.isIdleTimerDisabled = isVisible && scenePhase == .active ? true : previousIdleTimerDisabled
    }

    @State private var screenshotURL: URL?
    @MainActor private func takeScreenshot() async {
        guard allowsView, let display = socket.selectedDisplay, !screenshotBusy else { return }
        screenshotBusy = true
        screenshotError = nil
        defer { screenshotBusy = false }
        do {
            let data = try await session.http.desktopScreenshot(display: display, format: "png", quality: 80)
            guard isVisible, allowsView, scenePhase == .active, socket.state != .revoked else { return }
            cleanupScreenshot()
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("desktop-\(UUID().uuidString).png")
            try data.write(to: url, options: [.atomic, .completeFileProtection])
            screenshotURL = url
            share = DesktopScreenshotShare(url: url)
        } catch {
            screenshotError = String(localized: "Could not share the screenshot. Check host screen recording permission and connection, then try again.")
        }
    }

    private func cleanupScreenshot() {
        if let screenshotURL { try? FileManager.default.removeItem(at: screenshotURL) }
        screenshotURL = nil
    }
}

private struct DesktopScreenshotShare: Identifiable {
    let id = UUID()
    let url: URL
}

private struct DesktopShareSheet: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> UIActivityViewController {
        let controller = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        // Saving to Photos is deliberately not offered: this feature is share-
        // sheet-only and must not depend on a photo-library usage description.
        controller.excludedActivityTypes = [.saveToCameraRoll, .assignToContact]
        return controller
    }
    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}
