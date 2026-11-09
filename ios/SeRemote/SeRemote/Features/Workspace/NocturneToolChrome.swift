import SwiftUI

// MARK: - Metrics
//
// The prototype is authored in CSS px and every value here is converted 1:1 to
// points. Horizontal gutter is 18, card radius 16, list rhythm 8.

enum NocturneToolMetrics {
    static let gutter: CGFloat = 18
    static let cardGap: CGFloat = 8
    static let cardRadius: CGFloat = 16
    static let controlRadius: CGFloat = 9
    static let minimumTarget: CGFloat = 44
}

// MARK: - Screen scaffold

/// One scrolling screen in the prototype's visual language: a custom left
/// aligned nav bar (back / title / mono subtitle) followed by screen content.
struct NocturneToolScreen<Content: View>: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss

    let title: String
    let subtitle: String
    var trailing: AnyView?
    var onRefresh: (() async -> Void)?
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                NocturneNavBar(title: title, subtitle: subtitle, onBack: { dismiss() }, trailing: trailing)
                content()
            }
            .padding(.bottom, 30)
        }
        .scrollIndicators(.hidden)
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .nocturneRefreshable(onRefresh)
        .toolbar(.hidden, for: .navigationBar)
        .navigationBarBackButtonHidden(true)
        .navigationTitle("")
    }
}

extension View {
    @ViewBuilder
    func nocturneRefreshable(_ action: (() async -> Void)?) -> some View {
        if let action {
            refreshable { await action() }
        } else {
            self
        }
    }
}

// MARK: - Navigation bar

struct NocturneNavBar: View {
    @Environment(\.sePalette) private var palette

    let title: String
    let subtitle: String
    let onBack: () -> Void
    var trailing: AnyView?

    var body: some View {
        HStack(spacing: 10) {
            Button(action: onBack) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 32, height: 32)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
                    .overlay {
                        RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                    .frame(width: NocturneToolMetrics.minimumTarget, height: NocturneToolMetrics.minimumTarget)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "Back"))

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(SeNocturneType.font(14, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 8)

            if let trailing {
                trailing
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 8)
    }
}

/// Square icon control matching the prototype's .iconbtn (34pt visual, 44pt target).
struct NocturneIconButton: View {
    @Environment(\.sePalette) private var palette

    let systemImage: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 15, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
                .overlay {
                    RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
                .frame(width: NocturneToolMetrics.minimumTarget, height: NocturneToolMetrics.minimumTarget)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Section label

struct NocturneGroupLabel: View {
    @Environment(\.sePalette) private var palette

    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(SeNocturneType.font(10, weight: .regular, mono: true))
                .foregroundStyle(palette.inkTertiary)
            Spacer(minLength: 8)
            Text(detail)
                .font(SeNocturneType.font(9, weight: .regular, mono: true))
                .foregroundStyle(palette.inkTertiary)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 20)
        .padding(.bottom, 8)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Segmented control

struct NocturneSegment<Value: Hashable>: Identifiable {
    let value: Value
    let title: String
    var count: Int?
    var id: Value { value }
}

struct NocturneSegmented<Value: Hashable>: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let segments: [NocturneSegment<Value>]
    @Binding var selection: Value

    var body: some View {
        HStack(spacing: 3) {
            ForEach(segments) { segment in
                let isOn = selection == segment.value
                Button {
                    withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion)) {
                        selection = segment.value
                    }
                } label: {
                    HStack(spacing: 5) {
                        Text(segment.title)
                            .font(SeNocturneType.font(12, weight: isOn ? .medium : .regular))
                        if let count = segment.count {
                            Text(verbatim: "\(count)")
                                .font(SeNocturneType.font(9, weight: .regular, mono: true))
                                .opacity(0.6)
                        }
                    }
                    .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                    .frame(maxWidth: .infinity)
                    .frame(height: 32)
                    .background(
                        isOn ? palette.ink : .clear,
                        in: RoundedRectangle(cornerRadius: 8)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(isOn ? [.isSelected] : [])
            }
        }
        .padding(3)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 16)
    }
}

// MARK: - Chips

struct NocturneChip: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let title: String
    let count: Int?
    let isOn: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Text(title)
                    .font(SeNocturneType.font(11.5, weight: .medium))
                    .lineLimit(1)
                if let count {
                    Text(verbatim: "\(count)")
                        .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
            }
            .foregroundStyle(isOn ? palette.ink : palette.inkSecondary)
            .padding(.horizontal, 9)
            .frame(height: 29)
            .background(
                isOn ? palette.card : .clear,
                in: Capsule()
            )
            .overlay {
                Capsule().strokeBorder(isOn ? palette.stroke : .clear, lineWidth: 1)
            }
            .frame(minHeight: NocturneToolMetrics.minimumTarget)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? [.isSelected] : [])
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: isOn)
    }
}

struct NocturneChipRail<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 7) {
                content()
            }
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 16)
            .padding(.bottom, 4)
        }
        .scrollIndicators(.hidden)
    }
}

// MARK: - Cards and rows

struct NocturneCard<Content: View>: View {
    @Environment(\.sePalette) private var palette

    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(13)
            .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.cardRadius))
            .overlay {
                RoundedRectangle(cornerRadius: NocturneToolMetrics.cardRadius)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
    }
}

enum NocturneSpine: Equatable {
    case none
    case live
    case wait
}

/// The prototype's .scard: 3pt spine, title + mono-ish subtitle, trailing slot.
struct NocturneSessionCard: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let title: String
    let subtitle: String
    var spine: NocturneSpine = .none
    var alert = false
    var trailingText: String?
    var trailingTint: Color?
    var trailingSystemImage: String?
    var action: (() -> Void)?

    var body: some View {
        if let action {
            Button(action: action) { row }
                .buttonStyle(NocturnePressStyle(reduceMotion: reduceMotion))
        } else {
            row
        }
    }

    private var row: some View {
        HStack(spacing: 11) {
            RoundedRectangle(cornerRadius: 3)
                .fill(spineColor)
                .frame(width: 3, height: 26)

            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(SeNocturneType.font(12.5, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(SeNocturneType.font(10, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            if let trailingText {
                Text(trailingText)
                    .font(SeNocturneType.font(9, weight: .regular, mono: true))
                    .foregroundStyle(trailingTint ?? palette.inkTertiary)
            }
            if let trailingSystemImage {
                Image(systemName: trailingSystemImage)
                    .font(.system(size: 12, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .strokeBorder(alert ? palette.warning.opacity(0.34) : palette.stroke, lineWidth: 1)
        }
        .contentShape(Rectangle())
    }

    private var spineColor: Color {
        switch spine {
        case .none: palette.strokeStrong
        case .live: palette.live
        case .wait: palette.warning
        }
    }
}

// MARK: - Pills and badges

enum NocturnePillTone {
    case live, wait, off, blue
}

struct NocturnePill: View {
    @Environment(\.sePalette) private var palette

    let text: String
    let tone: NocturnePillTone

    var body: some View {
        Text(text)
            .font(SeNocturneType.font(9, weight: .regular, mono: true))
            .lineLimit(1)
            .padding(.horizontal, 7)
            .frame(height: 20)
            .foregroundStyle(foreground)
            .background(background, in: RoundedRectangle(cornerRadius: 6))
    }

    private var foreground: Color {
        switch tone {
        case .live: palette.live
        case .wait: palette.warning
        case .off: palette.inkTertiary
        case .blue: palette.accent
        }
    }

    private var background: Color {
        switch tone {
        case .live: palette.live.opacity(0.14)
        case .wait: palette.warning.opacity(0.14)
        case .off: palette.cardAlt
        case .blue: palette.accent.opacity(0.14)
        }
    }
}

// MARK: - Terminal surface

struct NocturneTerminalBlock<Content: View>: View {
    @Environment(\.sePalette) private var palette

    var maxHeight: CGFloat?
    var fontSize: CGFloat = 9.5
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView(.vertical) {
            VStack(alignment: .leading, spacing: 3) {
                content()
            }
            .font(.system(size: fontSize, design: .monospaced))
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: maxHeight)
        .scrollIndicators(.hidden)
        .padding(.horizontal, 13)
        .padding(.vertical, 14)
        .background(palette.terminalCanvas, in: RoundedRectangle(cornerRadius: 14))
        .overlay {
            RoundedRectangle(cornerRadius: 14)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }
}

// MARK: - Buttons

/// The prototype's .runbtn: 28pt visual height, 44pt hit target.
struct NocturneRunButton: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let title: String
    var systemImage: String?
    var role: ButtonRole?
    var action: () -> Void

    var body: some View {
        Button(role: role, action: action) {
            HStack(spacing: 5) {
                if let systemImage {
                    Image(systemName: systemImage).font(.system(size: 10, weight: .semibold))
                }
                Text(title)
                    .font(SeNocturneType.font(10, weight: .medium))
            }
            .foregroundStyle(role == .destructive ? palette.danger : palette.inkSecondary)
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background(palette.cardAlt.opacity(0.001), in: RoundedRectangle(cornerRadius: 7))
            .overlay {
                RoundedRectangle(cornerRadius: 7)
                    .strokeBorder(palette.strokeStrong, lineWidth: 1)
            }
            .frame(minHeight: NocturneToolMetrics.minimumTarget)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .animation(SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion), value: title)
    }
}

/// Press feedback is a 0.99 scale, and it disappears under Reduce Motion.
struct NocturnePressStyle: ButtonStyle {
    let reduceMotion: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.99 : 1)
            .opacity(configuration.isPressed ? 0.94 : 1)
            .animation(
                SeNocturneMotion.resolved(SeNocturneMotion.quick, reduceMotion: reduceMotion),
                value: configuration.isPressed
            )
    }
}

// MARK: - Empty and error states

struct NocturneEmptyState: View {
    @Environment(\.sePalette) private var palette

    let systemImage: String
    let title: String
    var message: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: systemImage)
                    .font(.system(size: 15, weight: .regular))
                    .foregroundStyle(palette.inkTertiary)
                Text(title)
                    .font(SeNocturneType.font(13, weight: .semibold))
                    .foregroundStyle(palette.ink)
            }
            if let message {
                Text(message)
                    .font(SeNocturneType.font(11, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.cardRadius))
        .overlay {
            RoundedRectangle(cornerRadius: NocturneToolMetrics.cardRadius)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }
}

struct NocturneNotice: View {
    @Environment(\.sePalette) private var palette

    let message: String
    var isError = false

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: isError ? "exclamationmark.triangle" : "info.circle")
                .font(.system(size: 13, weight: .regular))
                .foregroundStyle(isError ? palette.danger : palette.inkTertiary)
                .padding(.top, 1)
            Text(message)
                .font(SeNocturneType.font(10.5, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 14))
        .overlay {
            RoundedRectangle(cornerRadius: 14)
                .strokeBorder(isError ? palette.danger.opacity(0.3) : palette.stroke, lineWidth: 1)
        }
    }
}

// MARK: - Helpers

extension View {
    func nocturneGutter() -> some View {
        padding(.horizontal, NocturneToolMetrics.gutter)
    }
}

extension String {
    /// Host timestamps are opaque strings. Only render one when it is already
    /// human readable; never invent a local time or a relative age.
    var nocturneNonEmpty: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
