import SwiftUI
import UIKit

enum TerminalInputMode: String, CaseIterable, Identifiable {
    case live
    case buffered

    var id: String { rawValue }
}

struct TerminalInputDock: View {
    var isEnabled: Bool
    var isKeyboardVisible: Bool
    var displayMode: TerminalDisplayMode = .phone
    var textScale: CGFloat
    @Binding var controlModifier: TerminalControlModifier
    var onSend: (String) -> Void
    var onFocusTerminal: () -> Void
    var onDismissKeyboard: () -> Void
    var onToggleDisplayMode: () -> Void = {}
    var onNudgeTextScale: (Int) -> Void

    @State private var mode: TerminalInputMode = .live
    @State private var command = ""
    @State private var repeatingId: String?
    @State private var repeatTask: Task<Void, Never>?
    @State private var controlPressStarted: Date?
    @GestureState private var controlTouchDown = false
    @FocusState private var commandFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            accessoryBar
            if mode == .buffered {
                bufferedBar
            } else {
                liveBar
            }
        }
        .background(SeTheme.surface)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(SeTheme.stroke)
                .frame(height: 1)
        }
        .onChange(of: isKeyboardVisible) { _, visible in
            if !visible {
                commandFocused = false
            }
        }
        .onChange(of: controlTouchDown) { _, isPressed in
            if !isPressed, controlPressStarted != nil {
                controlModifier.releaseHold()
                controlPressStarted = nil
            }
        }
        .onDisappear {
            stopRepeat()
            controlModifier.releaseHold()
            controlPressStarted = nil
        }
    }

    private var accessoryBar: some View {
        VStack(spacing: SeSpace.xxs) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: SeSpace.xxs) {
                    accessoryChip(
                        label: displayMode == .phone
                            ? String(localized: "Phone")
                            : String(localized: "Desktop fit"),
                        isActive: true,
                        accessibility: displayMode == .phone
                            ? String(localized: "Switch to desktop fit")
                            : String(localized: "Fit the phone")
                    ) { onToggleDisplayMode() }
                    accessoryChip(label: "A−", accessibility: String(localized: "Smaller terminal text")) {
                        onNudgeTextScale(-1)
                    }
                    accessoryChip(label: "A+", accessibility: String(localized: "Larger terminal text")) {
                        onNudgeTextScale(1)
                    }
                    .accessibilityValue(Text("\(Int((textScale * 100).rounded())) percent"))
                    accessoryChip(
                        label: mode == .live ? String(localized: "Live") : String(localized: "Command"),
                        isActive: mode == .buffered,
                        accessibility: mode == .live
                            ? String(localized: "Switch to command field")
                            : String(localized: "Switch to live keyboard")
                    ) {
                        mode = mode == .live ? .buffered : .live
                        if mode == .live {
                            if isKeyboardVisible { onFocusTerminal() }
                        } else {
                            commandFocused = true
                        }
                    }
                    if isKeyboardVisible {
                        accessoryChip(
                            label: nil,
                            systemImage: "keyboard.chevron.compact.down",
                            accessibility: String(localized: "Hide keyboard")
                        ) { dismissKeyboard() }
                    }
                }
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: SeSpace.xxs) {
                    controlKey
                    ForEach(TerminalAccessoryCatalog.keys) { key in
                        if key.repeatable {
                            accessoryChip(label: key.label, accessibility: key.accessibilityLabel, action: nil)
                                .gesture(repeatGesture(for: key))
                                .accessibilityAction { send(key.bytes) }
                        } else {
                            accessoryChip(label: key.label, accessibility: key.accessibilityLabel) {
                                send(key.bytes)
                            }
                        }
                    }
                    if UIPasteboard.general.hasStrings {
                        accessoryChip(
                            label: String(localized: "Paste"),
                            accessibility: String(localized: "Paste from clipboard")
                        ) {
                            if let text = UIPasteboard.general.string, !text.isEmpty { send(text) }
                        }
                    }
                }
            }
        }
        .padding(.horizontal, SeSpace.sm)
        .padding(.vertical, SeSpace.xs)
        .opacity(isEnabled ? 1 : 0.35)
        .allowsHitTesting(isEnabled)
        .disabled(!isEnabled)
    }

    private var controlKey: some View {
        accessoryChip(
            label: controlModifier.isHeld ? String(localized: "Ctrl · Held")
                : controlModifier.isLatched ? String(localized: "Ctrl · Latched") : String(localized: "Ctrl"),
            isActive: controlModifier.isActive,
            accessibility: String(localized: "Control key")
        )
        .accessibilityValue(Text(controlModifier.isHeld
            ? String(localized: "Held")
            : controlModifier.isLatched ? String(localized: "Latched") : String(localized: "Off")))
        .accessibilityHint(Text("Tap to latch the next key, or hold while typing a key."))
        .accessibilityAddTraits(controlModifier.isActive ? .isSelected : [])
        .accessibilityAction { controlModifier.toggleLatch() }
        .gesture(
            DragGesture(minimumDistance: 0)
                .updating($controlTouchDown) { _, pressed, _ in pressed = true }
                .onChanged { _ in
                    guard controlPressStarted == nil else { return }
                    controlPressStarted = Date()
                    controlModifier.beginPress()
                }
                .onEnded { _ in
                    let isTap = controlPressStarted.map { Date().timeIntervalSince($0) < 0.35 } ?? false
                    controlModifier.endPress(isTap: isTap)
                    controlPressStarted = nil
                }
        )
        .accessibilityIdentifier("terminal-control-key")
    }

    private var liveBar: some View {
        Button {
            if isKeyboardVisible {
                dismissKeyboard()
            } else {
                onFocusTerminal()
            }
        } label: {
            HStack(spacing: 10) {
                Image(systemName: isKeyboardVisible ? "keyboard.chevron.compact.down" : "keyboard")
                    .font(.body)
                    .foregroundStyle(SeTheme.secondaryInk)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Live input")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(SeTheme.ink)
                    Text(isKeyboardVisible ? String(localized: "Tap to hide keyboard") : String(localized: "Tap to show keyboard"))
                        .font(.caption)
                        .foregroundStyle(SeTheme.secondaryInk)
                }
                Spacer()
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 46, alignment: .center)
            .background(SeTheme.elevated)
            .clipShape(RoundedRectangle(cornerRadius: SeRadius.tool, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .accessibilityLabel(
            Text(isKeyboardVisible ? String(localized: "Hide keyboard") : String(localized: "Show keyboard for live terminal input"))
        )
    }

    private var bufferedBar: some View {
        HStack(spacing: 8) {
            TextField(String(localized: "Type a command…"), text: $command)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.asciiCapable)
                .font(.body.monospaced())
                .submitLabel(.send)
                .focused($commandFocused)
                .onSubmit { submitCommand() }
                .onChange(of: command) { _, text in
                    // Ctrl in command mode sends just the compatible key, not a
                    // command plus Return. Leave multi-character paste untouched.
                    if controlModifier.isActive, TerminalControlModifier.controlSequence(for: text) != nil {
                        command = ""
                        send(text)
                    }
                }
                .padding(.horizontal, 12)
                .frame(minHeight: SeSpace.hitTarget)
                .background(SeTheme.elevated)
                .clipShape(RoundedRectangle(cornerRadius: SeRadius.tool, style: .continuous))
            Button {
                submitCommand()
            } label: {
                Image(systemName: "return")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(SeTheme.onAccent)
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .background(SeTheme.accent, in: RoundedRectangle(cornerRadius: SeRadius.tool))
            }
            .disabled(!isEnabled || command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityLabel(Text("Send command"))
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .toolbar {
            ToolbarItemGroup(placement: .keyboard) {
                Spacer()
                Button(String(localized: "Hide keyboard")) {
                    dismissKeyboard()
                }
            }
        }
    }

    @ViewBuilder
    private func accessoryChip(
        label: String?,
        systemImage: String? = nil,
        isActive: Bool = false,
        accessibility: String,
        action: (() -> Void)? = nil
    ) -> some View {
        let chip = Group {
            if let systemImage {
                Image(systemName: systemImage)
                    .font(.footnote.weight(.semibold))
            } else if let label {
                Text(label)
                    .font(.caption.weight(.semibold).monospaced())
            }
        }
        .foregroundStyle(isActive ? SeTheme.accent : SeTheme.ink)
        .padding(.horizontal, 10)
        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
        .background(isActive ? SeTheme.accentSubtle : SeTheme.elevated)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(SeTheme.stroke, lineWidth: 1)
        )
        .accessibilityLabel(Text(accessibility))
        .accessibilityAddTraits(.isButton)

        if let action {
            Button(action: action) { chip }
                .buttonStyle(.plain)
        } else {
            chip
        }
    }

    private func repeatGesture(for key: TerminalAccessoryKey) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { _ in
                guard key.repeatable, repeatingId != key.id else { return }
                startRepeat(key)
            }
            .onEnded { _ in
                if repeatingId == key.id {
                    stopRepeat()
                }
            }
    }

    private func startRepeat(_ key: TerminalAccessoryKey) {
        stopRepeat()
        repeatingId = key.id
        send(key.bytes)
        repeatTask = Task {
            try? await Task.sleep(for: .milliseconds(380))
            while !Task.isCancelled, repeatingId == key.id {
                send(key.bytes)
                try? await Task.sleep(for: .milliseconds(70))
            }
        }
    }

    private func stopRepeat() {
        repeatingId = nil
        repeatTask?.cancel()
        repeatTask = nil
    }

    private func submitCommand() {
        let text = command
        guard !text.isEmpty else { return }
        command = ""
        send(text.hasSuffix("\r") ? text : text + "\r")
    }

    private func send(_ bytes: String) {
        guard isEnabled else { return }
        onSend(bytes)
    }

    private func dismissKeyboard() {
        commandFocused = false
        onDismissKeyboard()
    }
}
