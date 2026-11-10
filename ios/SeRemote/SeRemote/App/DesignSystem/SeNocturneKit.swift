import SwiftUI

// MARK: - Surfaces

extension SeThemeMode {
    var isNight: Bool { self == .night }

    /// Bottom sheets sit one step above the page in both modes.
    var sheetSurface: Color {
        isNight ? Color(hex: 0x10141C) : Color(hex: 0xF6F8FB)
    }

    /// The translucent bottom bar / rail backdrop.
    var barSurface: Color {
        isNight ? Color(hex: 0x0A0C11).opacity(0.90) : Color(hex: 0xF6F8FC).opacity(0.96)
    }

    var railSurface: Color {
        isNight ? Color(hex: 0x090B10).opacity(0.98) : Color(hex: 0xF5F8FC).opacity(0.97)
    }

    /// Filled inset controls (icon buttons, segmented tracks, chips).
    var insetFill: Color {
        isNight ? Color.white.opacity(0.045) : Color.white
    }

    var insetStroke: Color {
        isNight ? Color.white.opacity(0.10) : Color.black.opacity(0.12)
    }

    var hairline: Color {
        isNight ? Color.white.opacity(0.08) : Color.black.opacity(0.10)
    }

    var strongHairline: Color {
        isNight ? Color.white.opacity(0.16) : Color.black.opacity(0.20)
    }
}

// MARK: - Typography helpers

struct SeDisplayText: View {
    let text: String
    @Environment(\.sePalette) private var palette

    var body: some View {
        Text(text)
            .font(SeNocturneType.font(33, weight: .semibold))
            .tracking(-1.45)
            .foregroundStyle(palette.ink)
            .accessibilityAddTraits(.isHeader)
    }
}

struct SeEyebrow: View {
    let text: String
    var color: Color? = nil

    @Environment(\.sePalette) private var palette

    var body: some View {
        Text(text)
            .font(SeNocturneType.font(10, mono: true))
            .foregroundStyle(color ?? palette.inkTertiary)
            .lineLimit(1)
    }
}

struct SeGroupLabel: View {
    let title: String
    var count: String?
    /// Standing explanation for the group, kept out of the layout as a tooltip.
    var info: String? = nil
    /// Accessible name for the info control; defaults to the group title.
    var infoAccessibilityLabel: String? = nil

    @Environment(\.sePalette) private var palette

    var body: some View {
        HStack(spacing: 6) {
            Text(title)
            if let info {
                SeInfoTip(text: info, accessibilityLabel: infoAccessibilityLabel ?? title, size: 22)
            }
            Spacer(minLength: 0)
            if let count {
                Text(count)
            }
        }
        .font(SeNocturneType.font(10, mono: true))
        .foregroundStyle(palette.inkTertiary)
        .padding(.horizontal, 18)
        .padding(.top, 20)
        .padding(.bottom, 8)
    }
}

// MARK: - Buttons

struct SePressable: ButtonStyle {
    var scale: CGFloat = 0.99
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? scale : 1)
            .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: configuration.isPressed)
    }
}

struct SeIconButton: View {
    let icon: SeIcon
    var accessibilityLabel: String
    var size: CGFloat = 34
    var action: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        Button(action: action) {
            SeIconView(icon: icon, size: size * 0.47, stroke: 1.6, color: palette.inkSecondary)
                .frame(width: size, height: size)
                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 9, style: .continuous)
                        .strokeBorder(mode.insetStroke, lineWidth: 1)
                }
                .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
                .frame(minWidth: 44, minHeight: 44)
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(accessibilityLabel)
    }
}

/// Bone-white in the night, ink in the day: the one primary action.
struct SeSolidButton: View {
    let title: String
    var icon: SeIcon?
    var action: () -> Void

    @Environment(\.sePalette) private var palette

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let icon { SeIconView(icon: icon, size: 16, stroke: 1.7, color: palette.canvas) }
                Text(title)
                    .font(SeNocturneType.font(13, weight: .semibold))
            }
            .foregroundStyle(palette.canvas)
            .frame(maxWidth: .infinity, minHeight: 44)
            .padding(.horizontal, 12)
            .background(palette.ink, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
        }
        .buttonStyle(SePressable())
    }
}

struct SeOutlineButton: View {
    let title: String
    var icon: SeIcon?
    var square = false
    var action: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let icon { SeIconView(icon: icon, size: 16, stroke: 1.6, color: palette.ink) }
                if !square {
                    Text(title).font(SeNocturneType.font(13, weight: .medium))
                }
            }
            .foregroundStyle(palette.ink)
            .frame(maxWidth: square ? nil : .infinity, minHeight: 44)
            .frame(width: square ? 44 : nil)
            .padding(.horizontal, square ? 0 : 12)
            .background(Color.clear, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(mode.strongHairline, lineWidth: 1)
            }
        }
        .buttonStyle(SePressable())
    }
}

// MARK: - Segmented control

struct SeSegment: Identifiable, Sendable {
    let id: String
    let title: String
    var count: Int?

    init(_ id: String, _ title: String, count: Int? = nil) {
        self.id = id
        self.title = title
        self.count = count
    }
}

struct SeSegmented: View {
    let segments: [SeSegment]
    @Binding var selection: String
    var wide = false
    var identifier: String? = nil

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Namespace private var namespace

    var body: some View {
        HStack(spacing: 2) {
            ForEach(segments) { segment in
                Button {
                    guard selection != segment.id else { return }
                    withAnimation(SeNocturneMotion.resolved(SeNocturneMotion.panel, reduceMotion: reduceMotion)) {
                        selection = segment.id
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text(segment.title)
                        if let count = segment.count {
                            Text("\(count)").opacity(0.62)
                        }
                    }
                    .font(SeNocturneType.font(wide ? 12 : 11.5, weight: selection == segment.id ? .semibold : .regular))
                    .foregroundStyle(selection == segment.id ? palette.canvas : palette.inkSecondary)
                    .frame(maxWidth: .infinity, minHeight: wide ? 32 : 27)
                    .background {
                        if selection == segment.id {
                            Capsule(style: .continuous)
                                .fill(palette.ink)
                                .matchedGeometryEffect(id: "seg", in: namespace)
                        }
                    }
                    .contentShape(Capsule(style: .continuous))
                }
                .buttonStyle(SePressable())
                .accessibilityAddTraits(selection == segment.id ? .isSelected : [])
            }
        }
        .padding(3)
        .background(mode.insetFill, in: Capsule(style: .continuous))
        .overlay {
            Capsule(style: .continuous).strokeBorder(mode.hairline, lineWidth: 1)
        }
        .accessibilityIdentifier(identifier ?? "")
    }
}

// MARK: - Chips

struct SeChipItem: Identifiable, Sendable {
    let id: String
    let title: String
    var dot: SeNeedDot.Kind? = nil
    var count: Int? = nil
}

struct SeChips: View {
    let items: [SeChipItem]
    @Binding var selection: String?

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 7) {
                ForEach(items) { item in
                    let on = selection == item.id
                    Button {
                        selection = item.id
                    } label: {
                        HStack(spacing: 6) {
                            if let dot = item.dot { SeNeedDot(kind: dot, size: 5) }
                            Text(item.title)
                            if let count = item.count {
                                Text("\(count)").foregroundStyle(palette.inkTertiary)
                            }
                        }
                        .font(SeNocturneType.font(11.5))
                        .foregroundStyle(on ? palette.ink : palette.inkSecondary)
                        .padding(.horizontal, 9)
                        .frame(height: 29)
                        .background(on ? mode.insetFill : Color.clear, in: Capsule(style: .continuous))
                        .overlay {
                            Capsule(style: .continuous)
                                .strokeBorder(on ? mode.insetStroke : mode.hairline, lineWidth: 1)
                        }
                        .frame(height: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(SePressable())
                }
            }
            .padding(.horizontal, 18)
        }
    }
}

// MARK: - Top chrome

struct SeNavBar: View {
    let title: String
    var subtitle: String? = nil
    var live = false
    var onBack: (() -> Void)? = nil
    var onTrailing: (() -> Void)? = nil

    @Environment(\.sePalette) private var palette

    var body: some View {
        HStack(spacing: 10) {
            if let onBack {
                SeIconButton(icon: .back, accessibilityLabel: String(localized: "Back"), size: 34, action: onBack)
            }
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    if live { SeNeedDot(kind: .live) }
                    Text(title)
                        .font(SeNocturneType.font(19, weight: .semibold))
                        .tracking(-0.5)
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                }
                if let subtitle {
                    Text(subtitle)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let onTrailing {
                SeIconButton(icon: .gear, accessibilityLabel: String(localized: "More"), size: 34, action: onTrailing)
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }
}

/// The live screen's host switch plus theme and sidebar controls.
struct SeHostSwitch: View {
    let title: String
    var meta: String? = nil
    var onHost: () -> Void
    var onTheme: () -> Void
    var themeIcon: SeIcon
    var onRail: () -> Void
    var onCommand: (() -> Void)? = nil
    var showsRail = true

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        HStack(spacing: 8) {
            Button(action: onHost) {
                HStack(spacing: 6) {
                    Text(title)
                        .font(SeNocturneType.font(11))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    SeIconView(icon: .chevron, size: 10, stroke: 1.7, color: palette.inkSecondary)
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(mode.insetStroke, lineWidth: 1)
                }
            }
            .buttonStyle(SePressable())

            if let meta {
                Text(meta)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            if let onCommand {
                Button(action: onCommand) {
                    Text("⌘")
                        .font(SeNocturneType.font(13, mono: true))
                        .foregroundStyle(palette.accent)
                        .frame(width: 34, height: 34)
                        .background(mode.insetFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .strokeBorder(mode.insetStroke, lineWidth: 1)
                        }
                        .contentShape(Rectangle())
                        .frame(width: 44, height: 44)
                }
                .buttonStyle(SePressable())
                .accessibilityLabel(String(localized: "Command palette"))
            }
            SeIconButton(icon: themeIcon, accessibilityLabel: String(localized: "Toggle theme"), size: 34, action: onTheme)
            if showsRail {
                SeIconButton(icon: .sidebar, accessibilityLabel: String(localized: "All destinations"), size: 34, action: onRail)
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 2)
    }
}

// MARK: - Needs strip

struct SeNeedStrip: View {
    let permissions: Int
    let failures: Int
    var onTap: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    private var isVisible: Bool { permissions + failures > 0 }

    var body: some View {
        if isVisible {
            Button(action: onTap) {
                HStack(spacing: 10) {
                    HStack(spacing: 7) {
                        if permissions > 0 {
                            SeNeedDot(kind: .wait)
                            Text("\(permissions) permissions").fontWeight(.semibold)
                        }
                        if permissions > 0 && failures > 0 {
                            Rectangle()
                                .fill(mode.strongHairline)
                                .frame(width: 1, height: 12)
                                .padding(.horizontal, 3)
                        }
                        if failures > 0 {
                            SeNeedDot(kind: .fail)
                            Text("\(failures) failures").fontWeight(.semibold)
                        }
                    }
                    .font(SeNocturneType.font(11.5))
                    .foregroundStyle(palette.ink)

                    Spacer(minLength: 0)
                    Text("Needs you →")
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.warning)
                }
                .padding(.horizontal, 13)
                .frame(height: 46)
                .background(palette.warningSubtle, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(palette.warning.opacity(0.26), lineWidth: 1)
                }
                .padding(.horizontal, 18)
                .padding(.top, 14)
            }
            .buttonStyle(SePressable())
            .accessibilityLabel(String(localized: "Needs you"))
        }
    }
}

// MARK: - Cards and rows

/// The live/workbench session row: spine, title, mono meta, time.
struct SeSessionRow: View {
    enum Status: Sendable { case wait, live, idle }
    let title: String
    let meta: String
    var time: String? = nil
    var status: Status = .idle
    var alert = false
    var trailing: String? = nil
    var onTap: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    private var spineColor: Color {
        switch status {
        case .wait: palette.warning
        case .live: palette.live
        case .idle: palette.inkTertiary.opacity(0.35)
        }
    }

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 11) {
                RoundedRectangle(cornerRadius: 2, style: .continuous)
                    .fill(spineColor)
                    .frame(width: 3, height: 26)
                VStack(alignment: .leading, spacing: 4) {
                    Text(title)
                        .font(SeNocturneType.font(12.5, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(meta)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                if let time {
                    Text(time)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
                if let trailing {
                    Text(trailing)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 52)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(alert ? palette.warning.opacity(0.30) : mode.hairline, lineWidth: 1)
            }
            .contentShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        }
        .buttonStyle(SePressable())
    }
}

/// Hairline-separated host row used on the pre-connection home.
struct SeHostRowView: View {
    let index: String
    let name: String
    var meta: String
    var latency: String?
    var online = false
    var onTap: () -> Void
    var onMore: (() -> Void)?

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 12) {
                Text(index)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(width: 20, alignment: .leading)
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 7) {
                        Text(name)
                            .font(SeNocturneType.font(13.5, weight: .semibold))
                            .foregroundStyle(palette.ink)
                            .lineLimit(1)
                        SeNeedDot(kind: online ? .live : .off, size: 5)
                    }
                    Text(meta)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                if let latency {
                    Text(latency)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
                if let onMore {
                    Button(action: onMore) {
                        SeIconView(icon: .more, size: 16, stroke: 1.6, color: palette.inkTertiary)
                            .frame(width: 44, height: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(String(localized: "Host actions"))
                }
            }
            .padding(.horizontal, 2)
            .padding(.vertical, 16)
            .overlay(alignment: .bottom) {
                Rectangle().fill(mode.hairline).frame(height: 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
    }
}

struct SeResumeRow: View {
    let title: String
    let meta: String
    let actionTitle: String
    var onTap: () -> Void

    @Environment(\.sePalette) private var palette

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 10) {
                SeIconView(icon: .up, size: 16, stroke: 1.7, color: palette.onAccent)
                    .frame(width: 30, height: 30)
                    .background(palette.accent, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(SeNocturneType.font(12.5, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(meta)
                        .font(SeNocturneType.font(8.5, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                Text(actionTitle)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.accent)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .background(palette.accentSubtle, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .strokeBorder(palette.accent.opacity(0.20), lineWidth: 1)
            }
        }
        .buttonStyle(SePressable())
    }
}

/// The 2-column tool tile used by the workspace and host screens.
struct SeToolTile: View {
    let icon: SeIcon
    let label: String
    let title: String
    let meta: String
    var onTap: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        Button(action: onTap) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    SeIconView(icon: icon, size: 13, stroke: 1.6, color: palette.inkTertiary)
                    Text(label)
                        .font(SeNocturneType.font(8.5, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                    Spacer(minLength: 0)
                }
                Text(title)
                    .font(SeNocturneType.font(12, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                    .padding(.top, 16)
                Text(meta)
                    .font(SeNocturneType.font(8.5, mono: true))
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
                    .padding(.top, 3)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(mode.hairline, lineWidth: 1)
            }
        }
        .buttonStyle(SePressable())
    }
}

// MARK: - Grouped settings card

struct SeGroupCard<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        VStack(spacing: 0) { content }
            .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(mode.hairline, lineWidth: 1)
            }
    }
}

struct SeRowItem<Trailing: View>: View {
    var icon: SeIcon?
    let title: String
    var detail: String?
    var value: String?
    var destructive = false
    var action: (() -> Void)?
    @ViewBuilder var trailing: Trailing

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    init(icon: SeIcon? = nil,
         title: String,
         detail: String? = nil,
         value: String? = nil,
         destructive: Bool = false,
         action: (() -> Void)? = nil,
         @ViewBuilder trailing: () -> Trailing) {
        self.icon = icon
        self.title = title
        self.detail = detail
        self.value = value
        self.destructive = destructive
        self.action = action
        self.trailing = trailing()
    }

    var body: some View {
        let content = HStack(spacing: 10) {
            if let icon {
                SeIconView(icon: icon, size: 17, stroke: 1.6, color: palette.inkSecondary)
                    .frame(width: 22)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(SeNocturneType.font(12.5, weight: .medium))
                    .foregroundStyle(destructive ? palette.danger : palette.ink)
                if let detail {
                    Text(detail)
                        .font(SeNocturneType.font(9, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            if let value {
                Text(value)
                    .font(SeNocturneType.font(9, mono: true))
                    .foregroundStyle(palette.inkTertiary)
            }
            trailing
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 48)
        .contentShape(Rectangle())

        if let action {
            Button(action: action) { content }
                .buttonStyle(SePressable())
                .overlay(alignment: .bottom) {
                    Rectangle().fill(mode.hairline).frame(height: 1).padding(.leading, icon == nil ? 12 : 44)
                }
        } else {
            content
                .overlay(alignment: .bottom) {
                    Rectangle().fill(mode.hairline).frame(height: 1).padding(.leading, icon == nil ? 12 : 44)
                }
        }
    }
}

struct SeToggle: View {
    @Binding var isOn: Bool
    var label: String

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        Button {
            isOn.toggle()
        } label: {
            Capsule(style: .continuous)
                .fill(isOn ? (mode.isNight ? palette.accent : palette.accent) : (mode.isNight ? Color.white.opacity(0.09) : Color.black.opacity(0.08)))
                .frame(width: 40, height: 24)
                .overlay(alignment: isOn ? .trailing : .leading) {
                    Circle()
                        .fill(isOn ? palette.onAccent : palette.inkTertiary)
                        .frame(width: 18, height: 18)
                        .padding(3)
                }
                .overlay {
                    Capsule(style: .continuous).strokeBorder(mode.insetStroke, lineWidth: 1)
                }
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

struct SeThemePick: View {
    @Binding var mode: SeThemeMode

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var current

    var body: some View {
        HStack(spacing: 4) {
            ForEach(SeThemeMode.allCases) { item in
                Button {
                    mode = item
                } label: {
                    Text(item == .day ? "Day" : "Night")
                        .font(SeNocturneType.font(10))
                        .foregroundStyle(mode == item ? palette.canvas : palette.inkTertiary)
                        .padding(.horizontal, 8)
                        .frame(height: 24)
                        .background(mode == item ? palette.ink : Color.clear,
                                    in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                        .frame(height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(SePressable())
            }
        }
        .padding(2)
        .overlay {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(current.insetStroke, lineWidth: 1)
        }
    }
}

// MARK: - Bottom bar

struct SeBottomBar: View {
    let selection: SidebarDestination
    var onTerminals: () -> Void
    var onConversations: () -> Void
    var onTools: () -> Void

    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    var body: some View {
        HStack(spacing: 6) {
            tab(icon: .terminal, title: String(localized: "Terminals"), on: selection == .terminals, action: onTerminals)
            tab(icon: .chat, title: String(localized: "Conversations"), on: selection == .conversations, action: onConversations)
            tab(icon: .folder, title: String(localized: "Tools"), on: selection == .tools, action: onTools)
        }
        .padding(.horizontal, 14)
        .padding(.top, 8)
        .padding(.bottom, 22)
        .frame(height: 78)
        .background(.ultraThinMaterial)
        .background(mode.barSurface)
        .overlay(alignment: .top) {
            Rectangle().fill(mode.hairline).frame(height: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(String(localized: "Main navigation"))
    }

    private func tab(icon: SeIcon, title: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                SeIconView(icon: icon, size: 18, stroke: 1.6, color: on ? palette.ink : palette.inkTertiary)
                Text(title)
                    .font(SeNocturneType.font(9.5))
                    .foregroundStyle(on ? palette.ink : palette.inkTertiary)
            }
            .frame(maxWidth: .infinity, minHeight: 44)
            .background(on ? mode.insetFill : Color.clear,
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
        .buttonStyle(SePressable())
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

// MARK: - Disclosure

/// Keeps an explanation out of the layout: a compact `info.circle` control that
/// shows the text in a popover on touch, and as a pointer tooltip via `help`.
struct SeInfoTip: View {
    let text: String
    var accessibilityLabel: String = String(localized: "Details")
    var size: CGFloat = 28

    @Environment(\.sePalette) private var palette
    @State private var isPresented = false

    var body: some View {
        Button {
            isPresented = true
        } label: {
            Image(systemName: "info.circle")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(palette.inkTertiary)
                .frame(width: size, height: size)
                .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .help(text)
        .accessibilityLabel(accessibilityLabel)
        .accessibilityHint(text)
        .popover(isPresented: $isPresented, arrowEdge: .bottom) {
            Text(text)
                .font(SeNocturneType.font(12))
                .foregroundStyle(palette.ink)
                .fixedSize(horizontal: false, vertical: true)
                .padding(14)
                .frame(maxWidth: 300, alignment: .leading)
                .presentationCompactAdaptation(.popover)
                .presentationBackground(palette.card)
        }
    }
}

// MARK: - Sheet chrome

struct SeSheetHeader: View {
    let title: String
    var subtitle: String? = nil
    var onClose: () -> Void

    @Environment(\.sePalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Capsule()
                .fill(palette.inkTertiary.opacity(0.35))
                .frame(width: 36, height: 4)
                .frame(maxWidth: .infinity)
                .padding(.bottom, 14)
            HStack(alignment: .center, spacing: 8) {
                Text(title)
                    .font(SeNocturneType.font(19, weight: .semibold))
                    .tracking(-0.55)
                    .foregroundStyle(palette.ink)
                Spacer(minLength: 0)
                SeIconButton(icon: .close, accessibilityLabel: String(localized: "Close"), size: 34, action: onClose)
            }
            if let subtitle {
                Text(subtitle)
                    .font(SeNocturneType.font(10.5))
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.top, 2)
            }
        }
    }
}

/// A link-styled row used for "add host" under the host list.
struct SeLinkRow: View {
    let title: String
    var icon: SeIcon = .link
    var onTap: () -> Void

    @Environment(\.sePalette) private var palette

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 7) {
                SeIconView(icon: icon, size: 14, stroke: 1.6, color: palette.accent)
                Text(title)
                    .font(SeNocturneType.font(11.5))
                    .foregroundStyle(palette.accent)
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
    }
}
