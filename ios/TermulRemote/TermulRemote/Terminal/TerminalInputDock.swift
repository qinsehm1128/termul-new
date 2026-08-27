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
    var onSend: (String) -> Void
    var onFocusTerminal: () -> Void
    var onDismissKeyboard: () -> Void
    var onToggleDisplayMode: () -> Void = {}
    var onNudgeTextScale: (Int) -> Void

    @State private var mode: TerminalInputMode = .live
    @State private var command = ""
    @State private var repeatingId: String?
    @State private var repeatTask: Task<Void, Never>?
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
        .background(TermulTheme.surface)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(TermulTheme.stroke)
                .frame(height: 1)
        }
        .onChange(of: isKeyboardVisible) { _, visible in
            if !visible {
                commandFocused = false
            }
        }
        .onDisappear {
            stopRepeat()
        }
    }

    private var accessoryBar: some View {
        HStack(spacing: 6) {
            if isKeyboardVisible {
                accessoryChip(
                    label: nil,
                    systemImage: "keyboard.chevron.compact.down",
                    accessibility: String(localized: "Hide keyboard")
                ) {
                    dismissKeyboard()
                }
            }
            accessoryChip(
                label: displayMode == .phone
                    ? String(localized: "Phone")
                    : String(localized: "Desktop"),
                isActive: displayMode == .phone,
                accessibility: displayMode == .phone
                    ? String(localized: "Switch to desktop size")
                    : String(localized: "Fit the phone")
            ) {
                onToggleDisplayMode()
            }
            accessoryChip(
                label: "A-",
                accessibility: String(localized: "Smaller terminal text")
            ) {
                onNudgeTextScale(-1)
            }
            accessoryChip(
                label: "A+",
                accessibility: String(localized: "Larger terminal text")
            ) {
                onNudgeTextScale(1)
            }
            .accessibilityValue(Text("\(Int((textScale * 100).rounded())) percent"))
            accessoryChip(
                label: mode == .live ? "LIVE" : "BUF",
                isActive: mode == .live,
                accessibility: mode == .live ? "Switch to command field" : "Switch to live keyboard"
            ) {
                mode = mode == .live ? .buffered : .live
                if mode == .live {
                    if isKeyboardVisible {
                        onFocusTerminal()
                    }
                } else {
                    commandFocused = true
                }
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    if UIPasteboard.general.hasStrings {
                        accessoryChip(label: "Paste", accessibility: "Paste from clipboard") {
                            if let text = UIPasteboard.general.string, !text.isEmpty {
                                send(text)
                            }
                        }
                    }
                    ForEach(TerminalAccessoryCatalog.keys) { key in
                        if key.repeatable {
                            accessoryChip(label: key.label, accessibility: key.accessibilityLabel, action: nil)
                                .gesture(repeatGesture(for: key))
                        } else {
                            accessoryChip(label: key.label, accessibility: key.accessibilityLabel) {
                                send(key.bytes)
                            }
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .opacity(isEnabled ? 1 : 0.35)
        .allowsHitTesting(isEnabled)
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
                    .foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Live input")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.primary)
                    Text(isKeyboardVisible ? "Tap to hide keyboard" : "Tap to show keyboard")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 46, alignment: .center)
            .background(TermulTheme.canvas)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .accessibilityLabel(
            Text(isKeyboardVisible ? "Hide keyboard" : "Show keyboard for live terminal input")
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
                .padding(.horizontal, 12)
                .frame(minHeight: 36)
                .background(TermulTheme.canvas)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            Button {
                submitCommand()
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.title2)
                    .frame(width: 44, height: 44)
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
        .foregroundStyle(isActive ? Color.white : Color.primary)
        .padding(.horizontal, 10)
        .frame(minWidth: 44, minHeight: 36)
        .background(isActive ? TermulTheme.accent : TermulTheme.canvas)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(TermulTheme.stroke, lineWidth: 1)
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
        send(key.bytes, haptic: true)
        repeatTask = Task {
            try? await Task.sleep(for: .milliseconds(380))
            while !Task.isCancelled, repeatingId == key.id {
                send(key.bytes, haptic: false)
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

    private func send(_ bytes: String, haptic: Bool = true) {
        guard isEnabled else { return }
        if haptic {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
        }
        onSend(bytes)
    }

    private func dismissKeyboard() {
        commandFocused = false
        onDismissKeyboard()
    }
}
