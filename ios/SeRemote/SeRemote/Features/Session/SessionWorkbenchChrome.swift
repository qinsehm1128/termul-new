import SwiftUI

/// R2 workbench chrome primitives.
///
/// These are private to the session workbench rewrite (chat / terminal /
/// files). They mirror the prototype's small repeated pieces — status dots,
/// pills, card spines, group labels and fit chips — without changing the
/// shared design-system file owned by r1.
enum R2Tone: Equatable {
    case live
    case attention
    case quiet
    case failed

    func color(_ palette: SePalette) -> Color {
        switch self {
        case .live: palette.live
        case .attention: palette.warning
        case .quiet: palette.inkTertiary
        case .failed: palette.danger
        }
    }

    func surface(_ palette: SePalette) -> Color {
        switch self {
        case .live: palette.liveSubtle
        case .attention: palette.warningSubtle
        case .quiet: palette.cardAlt
        case .failed: palette.dangerSubtle
        }
    }
}

struct R2Dot: View {
    var tone: R2Tone = .live
    var size: CGFloat = 6
    @Environment(\.sePalette) private var palette

    var body: some View {
        Circle()
            .fill(tone.color(palette))
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

struct R2Pill: View {
    let text: String
    var tone: R2Tone = .quiet
    @Environment(\.sePalette) private var palette

    var body: some View {
        Text(text)
            .font(SeNocturneType.monoSmall)
            .foregroundStyle(tone.color(palette))
            .padding(.horizontal, 7)
            .frame(height: 20)
            .background(tone.surface(palette), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .accessibilityLabel(Text(text))
    }
}

/// The 3 × 26 card spine used by session and file rows.
struct R2Spine: View {
    var tone: R2Tone = .quiet
    @Environment(\.sePalette) private var palette

    var body: some View {
        RoundedRectangle(cornerRadius: 3, style: .continuous)
            .fill(tone == .quiet ? palette.strokeStrong : tone.color(palette))
            .frame(width: 3, height: 26)
            .accessibilityHidden(true)
    }
}

struct R2GroupLabel: View {
    let title: String
    var detail: String?
    @Environment(\.sePalette) private var palette

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(SeNocturneType.mono)
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 8)
            if let detail {
                Text(detail)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
            }
        }
        .padding(.top, 20)
        .padding(.bottom, 8)
        .accessibilityElement(children: .combine)
    }
}

/// Prototype `.fitchip`: 20pt tall, 5pt radius, mono 8pt label.
struct R2FitChip: View {
    let label: String
    var isActive = false
    var action: () -> Void
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(isActive ? palette.accent : palette.inkSecondary)
                .padding(.horizontal, 6)
                .frame(minHeight: 28)
                .background(isActive ? palette.accentSubtle : palette.card, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .stroke(palette.stroke, lineWidth: 1)
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(minHeight: SeSpace.hitTarget)
        .contentShape(Rectangle())
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: isActive)
    }
}

/// Prototype `.key`: 30pt tall accessory key. The frame below 44pt is the
/// visual capsule; the enclosing button keeps a 44pt hit target.
struct R2KeyChip: View {
    let label: String
    var isActive = false
    var repeatable = false
    var accessibilityLabel: String?
    var action: () -> Void
    var repeatAction: (() -> Void)?

    @Environment(\.sePalette) private var palette
    @State private var pressed = false
    @State private var repeatTask: Task<Void, Never>?

    var body: some View {
        Text(label)
            .font(.system(size: 11, weight: .medium, design: .monospaced))
            .foregroundStyle(pressed || isActive ? palette.canvas : palette.inkSecondary)
            .padding(.horizontal, 9)
            .frame(minWidth: 30, minHeight: 30)
            .background(pressed || isActive ? palette.ink : palette.card, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .stroke(isActive ? palette.accent : palette.stroke, lineWidth: 1)
            }
            .frame(minHeight: SeSpace.hitTarget)
            .contentShape(Rectangle())
            .onTapGesture { action() }
            .onLongPressGesture(minimumDuration: 0.01, pressing: { down in
                pressed = down
                if down, repeatable {
                    startRepeat()
                } else if !down {
                    stopRepeat()
                }
            }, perform: {})
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in
                        guard repeatable, !pressed else { return }
                        pressed = true
                        startRepeat()
                    }
                    .onEnded { _ in
                        pressed = false
                        stopRepeat()
                    }
            )
            .accessibilityLabel(Text(accessibilityLabel ?? label))
            .accessibilityAddTraits(.isButton)
            .accessibilityAddTraits(isActive ? .isSelected : [])
            .onDisappear { stopRepeat() }
    }

    private func startRepeat() {
        guard repeatable, let repeatAction else { return }
        stopRepeat()
        repeatAction()
        repeatTask = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(380))
            while !Task.isCancelled {
                repeatAction()
                try? await Task.sleep(for: .milliseconds(70))
            }
        }
    }

    private func stopRepeat() {
        repeatTask?.cancel()
        repeatTask = nil
    }
}
