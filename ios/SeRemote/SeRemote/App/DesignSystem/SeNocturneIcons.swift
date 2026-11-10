import SwiftUI

/// The prototype's single linear icon set: 24 × 24, 1.6 pt stroke, round caps.
/// Drawn rather than borrowed so the weight matches in both modes.
nonisolated enum SeIcon: String, CaseIterable, Sendable {
    case hosts
    case terminal
    case branch
    case clock
    case folder
    case gear
    case search
    case sidebar
    case plus
    case chevron
    case chevronDown
    case back
    case up
    case close
    case file
    case check
    case play
    case refresh
    case lock
    case sun
    case moon
    case scan
    case link
    case arrowRight
    case more
    case chat

    // MARK: Phase 2 — desktop, capture and preview

    /// Monitor with an inset screen: the quick-desktop entry.
    case desktop
    /// Selection / crop frame used by the region-capture mode.
    case viewfinder
    /// Two stacked screens: switch display output.
    case display
    /// Fit the frame to width.
    case fit
    /// Diagonal arrows: grow to fill.
    case expand
    case send
    case image
    /// Document with text lines (rendered previews, not raw files).
    case doc
    /// Angle brackets and a slash: source / HTML preview.
    case code
    /// Framed M with a down arrow: Markdown preview.
    case markdown
    case keyboard
    case keyboardDown
    case pointer
    /// Cropped frame around a T: screen-text snapshot.
    case textview

    // MARK: Prototype glyphs the first pass missed

    /// Prototype `git` (same construction as `branch`).
    case git
    /// Open in another surface / pop out.
    case open
    case info
    case copy
    case bolt
    case server
    case detach
    case toggle
    case people
    case stop
    case filter
    case down
    case chevronUpDown
}

struct SeIconView: View {
    let icon: SeIcon
    var size: CGFloat = 18
    var stroke: CGFloat = 1.6
    var color: Color?

    @Environment(\.sePalette) private var palette

    var body: some View {
        SeIconShape(icon: icon)
            .stroke(
                color ?? palette.inkSecondary,
                style: StrokeStyle(lineWidth: stroke, lineCap: .round, lineJoin: .round)
            )
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

nonisolated struct SeIconShape: Shape {
    let icon: SeIcon

    nonisolated func path(in rect: CGRect) -> Path {
        let s = min(rect.width, rect.height) / 24
        var path = Path()
        func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
            CGPoint(x: rect.minX + x * s, y: rect.minY + y * s)
        }
        func move(_ x: CGFloat, _ y: CGFloat) { path.move(to: p(x, y)) }
        func line(_ x: CGFloat, _ y: CGFloat) { path.addLine(to: p(x, y)) }
        func circle(_ x: CGFloat, _ y: CGFloat, _ r: CGFloat) {
            path.addEllipse(in: CGRect(x: rect.minX + (x - r) * s, y: rect.minY + (y - r) * s,
                                       width: r * 2 * s, height: r * 2 * s))
        }
        /// A round-capped dot: the prototype writes these as `h.01`.
        func dot(_ x: CGFloat, _ y: CGFloat) {
            move(x, y); line(x + 0.01, y)
        }
        func rounded(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) {
            path.addRoundedRect(in: CGRect(x: rect.minX + x * s, y: rect.minY + y * s,
                                           width: w * s, height: h * s),
                                cornerSize: CGSize(width: r * s, height: r * s))
        }

        switch icon {
        case .hosts:
            move(12, 3); line(21, 7.5); line(21, 16.5); line(12, 21); line(3, 16.5); line(3, 7.5); path.closeSubpath()
            move(3, 7.5); line(12, 12); line(21, 7.5)
            move(12, 12); line(12, 21)
        case .terminal:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 16 * s),
                                cornerSize: CGSize(width: 2.5 * s, height: 2.5 * s))
            move(7, 10); line(10, 13); line(7, 16)
            move(12.5, 16.5); line(17, 16.5)
        case .branch, .git:
            circle(6, 5, 2)
            circle(6, 19, 2)
            circle(18, 8, 2)
            move(6, 7); line(6, 17)
            move(6, 11); line(6, 12)
            path.addQuadCurve(to: p(18, 10), control: p(11, 15))
        case .clock:
            circle(12, 12, 8.5)
            move(12, 7.2); line(12, 12); line(15.8, 14)
        case .folder:
            move(3, 8)
            line(3, 6.5)
            path.addQuadCurve(to: p(5, 4.5), control: p(3, 4.5))
            line(9.5, 4.5); line(11.5, 6.5); line(19, 6.5)
            path.addQuadCurve(to: p(21, 8.5), control: p(21, 6.5))
            line(21, 17.5)
            path.addQuadCurve(to: p(19, 19.5), control: p(21, 19.5))
            line(5, 19.5)
            path.addQuadCurve(to: p(3, 17.5), control: p(3, 19.5))
            path.closeSubpath()
        case .gear:
            circle(12, 12, 3.2)
            for i in 0..<8 {
                let a = Double(i) * .pi / 4
                let inner: CGFloat = 5.4
                let outer: CGFloat = 8.4
                move(12 + CGFloat(cos(a)) * inner, 12 + CGFloat(sin(a)) * inner)
                line(12 + CGFloat(cos(a)) * outer, 12 + CGFloat(sin(a)) * outer)
            }
        case .search:
            circle(11, 11, 6.5)
            move(15.8, 15.8); line(21, 21)
        case .sidebar:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 16 * s),
                                cornerSize: CGSize(width: 2 * s, height: 2 * s))
            move(9, 4); line(9, 20)
        case .plus:
            move(12, 5); line(12, 19)
            move(5, 12); line(19, 12)
        case .chevron:
            move(9, 6); line(15, 12); line(9, 18)
        case .chevronDown:
            move(6, 9); line(12, 15); line(18, 9)
        case .back:
            move(15, 5); line(8, 12); line(15, 19)
        case .up:
            move(12, 19); line(12, 5)
            move(6, 11); line(12, 5); line(18, 11)
        case .close:
            move(6, 6); line(18, 18)
            move(18, 6); line(6, 18)
        case .file:
            move(6, 3); line(14, 3); line(18, 7); line(18, 21); line(6, 21); path.closeSubpath()
            move(14, 3); line(14, 7); line(18, 7)
        case .check:
            move(5, 12); line(10, 17); line(19, 7)
        case .play:
            move(8, 5); line(19, 12); line(8, 19); path.closeSubpath()
        case .refresh:
            path.addArc(center: p(12, 12), radius: 8 * s,
                        startAngle: .degrees(-55), endAngle: .degrees(235), clockwise: false)
            move(13, 3.2); line(17.4, 6.2); line(13.2, 8.4)
        case .lock:
            path.addRoundedRect(in: CGRect(x: rect.minX + 5 * s, y: rect.minY + 10 * s,
                                           width: 14 * s, height: 10.5 * s),
                                cornerSize: CGSize(width: 2 * s, height: 2 * s))
            move(8, 10); line(8, 7.5)
            path.addArc(center: p(12, 7.5), radius: 4 * s,
                        startAngle: .degrees(180), endAngle: .degrees(0), clockwise: false)
            move(12, 14); line(12, 16.5)
        case .sun:
            circle(12, 12, 4)
            for i in 0..<8 {
                let a = Double(i) * .pi / 4
                move(12 + CGFloat(cos(a)) * 6.2, 12 + CGFloat(sin(a)) * 6.2)
                line(12 + CGFloat(cos(a)) * 8.6, 12 + CGFloat(sin(a)) * 8.6)
            }
        case .moon:
            move(20, 14.6)
            path.addCurve(to: p(9.4, 4), control1: p(20, 8), control2: p(15, 3.2))
            path.addCurve(to: p(20, 14.6), control1: p(9.2, 13.4), control2: p(13.8, 18.4))
            path.closeSubpath()
        case .scan:
            move(4, 9); line(4, 4); line(9, 4)
            move(15, 4); line(20, 4); line(20, 9)
            move(20, 15); line(20, 20); line(15, 20)
            move(9, 20); line(4, 20); line(4, 15)
        case .link:
            move(10, 14)
            path.addQuadCurve(to: p(14, 10), control: p(12, 12))
            move(9.5, 8.5)
            path.addQuadCurve(to: p(6.5, 8.5), control: p(8, 7))
            path.addQuadCurve(to: p(6.5, 15.5), control: p(4.5, 11.8))
            line(8.5, 17.5)
            path.addQuadCurve(to: p(11.5, 17.5), control: p(10, 19))
            move(14.5, 15.5)
            path.addQuadCurve(to: p(17.5, 15.5), control: p(16, 17))
            path.addQuadCurve(to: p(17.5, 8.5), control: p(19.5, 12.2))
            line(15.5, 6.5)
            path.addQuadCurve(to: p(12.5, 6.5), control: p(14, 5))
        case .arrowRight:
            move(4, 12); line(20, 12)
            move(14, 6); line(20, 12); line(14, 18)
        case .more:
            circle(5, 12, 1.2)
            circle(12, 12, 1.2)
            circle(19, 12, 1.2)
        case .chat:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 13.5 * s),
                                cornerSize: CGSize(width: 3 * s, height: 3 * s))
            move(8, 17.5); line(7, 21); line(12, 17.5)

        // MARK: Phase 2 — desktop, capture and preview

        case .desktop:
            // Monitor with an inset screen so it reads apart from `hosts`.
            rounded(3, 4, 18, 12.5, 2)
            rounded(6, 7, 12, 6.5, 1.2)
            move(8, 20.5); line(16, 20.5)
            move(12, 16.5); line(12, 20.5)

        case .viewfinder:
            // Selection frame: corner brackets around the crop box.
            move(8, 3.5); line(3.5, 3.5); line(3.5, 8)
            move(16, 3.5); line(20.5, 3.5); line(20.5, 8)
            move(20.5, 16); line(20.5, 20.5); line(16, 20.5)
            move(8, 20.5); line(3.5, 20.5); line(3.5, 16)
            rounded(8, 8, 8, 8, 1.5)

        case .display:
            // Two stacked screens: switch display output.
            rounded(3, 4.5, 13, 9, 1.6)
            rounded(8, 10.5, 13, 9, 1.6)

        case .fit:
            // Screen with a horizontal double arrow: fit to width.
            rounded(3, 6, 18, 12, 2.2)
            move(8, 12); line(16, 12)
            move(10.6, 9.4); line(8, 12); line(10.6, 14.6)
            move(13.4, 9.4); line(16, 12); line(13.4, 14.6)

        case .expand:
            // Diagonal arrows growing out of the corners.
            move(20, 4); line(13.2, 4)
            move(20, 4); line(20, 10.8)
            move(20, 4); line(12.6, 11.4)
            move(4, 20); line(10.8, 20)
            move(4, 20); line(4, 13.2)
            move(4, 20); line(11.4, 12.6)

        case .send:
            // Paper plane, prototype page turn folded in.
            move(20.5, 3.5); line(3.5, 10.2); line(10.3, 13.2)
            line(13.6, 20.2); path.closeSubpath()
            move(10.3, 13.2); line(20.5, 3.5)

        case .image:
            rounded(3, 4.5, 18, 15, 2.4)
            circle(8.5, 9.5, 1.5)
            move(3.6, 17.4); line(9.5, 11.5); line(14, 16)
            line(16.5, 13.5); line(20.4, 17.4)

        case .doc:
            rounded(5.5, 3.5, 13, 17, 2)
            move(8.6, 9); line(15.4, 9)
            move(8.6, 12.5); line(15.4, 12.5)
            move(8.6, 16); line(12.6, 16)

        case .code:
            move(9.2, 7.8); line(4.6, 12); line(9.2, 16.2)
            move(14.8, 7.8); line(19.4, 12); line(14.8, 16.2)
            move(13.4, 5.2); line(10.6, 18.8)

        case .markdown:
            rounded(3, 5, 18, 14, 2.4)
            move(6.8, 15.4); line(6.8, 9.6); line(9.2, 12.4)
            line(11.6, 9.6); line(11.6, 15.4)
            move(16.6, 9.6); line(16.6, 15.4)
            move(14.6, 13.4); line(16.6, 15.4); line(18.6, 13.4)

        case .keyboard:
            rounded(2.6, 6.4, 18.8, 11.2, 2.2)
            for x in [6, 9.4, 12.8, 16.2, 19] { dot(x, 10) }
            for x in [6, 9.4, 12.8, 16.2, 19] { dot(x, 13.4) }
            move(8, 16.4); line(16, 16.4)

        case .keyboardDown:
            rounded(2.6, 6.4, 18.8, 11.2, 2.2)
            move(8, 13.2); line(8, 15.4); line(16, 15.4); line(16, 13.2)
            move(12, 6.6); line(12, 11.8)
            move(9.8, 9.6); line(12, 11.8); line(14.2, 9.6)

        case .pointer:
            move(6, 3.5); line(6, 18.6); line(9.9, 14.7); line(12.6, 20.6)
            line(15.2, 19.4); line(12.4, 13.5); line(17.6, 13.5)
            path.closeSubpath()

        case .textview:
            // Crop marks around a T: read-only screen-text snapshot.
            move(4, 7.5); line(4, 5); line(6.5, 5)
            move(17.5, 5); line(20, 5); line(20, 7.5)
            move(20, 16.5); line(20, 19); line(17.5, 19)
            move(6.5, 19); line(4, 19); line(4, 16.5)
            move(8, 9.2); line(16, 9.2)
            move(12, 9.2); line(12, 16.2)
            move(9.6, 16.2); line(14.4, 16.2)

        // MARK: Prototype glyphs the first pass missed

        case .open:
            move(14, 4); line(20, 4); line(20, 10)
            move(20, 4); line(11.6, 12.4)
            move(18, 14); line(18, 18.5)
            path.addQuadCurve(to: p(16.5, 20), control: p(18, 20))
            line(5.5, 20)
            path.addQuadCurve(to: p(4, 18.5), control: p(4, 20))
            line(4, 7.5)
            path.addQuadCurve(to: p(5.5, 6), control: p(4, 6))
            line(10, 6)

        case .info:
            circle(12, 12, 8.4)
            move(12, 11.2); line(12, 16.2)
            dot(12, 8.1)

        case .copy:
            rounded(8.5, 8.5, 11.5, 11.5, 2)
            move(15.5, 8.5); line(15.5, 6)
            path.addQuadCurve(to: p(14, 4.5), control: p(15.5, 4.5))
            line(6, 4.5)
            path.addQuadCurve(to: p(4.5, 6), control: p(4.5, 4.5))
            line(4.5, 14)
            path.addQuadCurve(to: p(6, 15.5), control: p(4.5, 15.5))
            line(8.5, 15.5)

        case .bolt:
            move(13, 3); line(5.5, 13.4); line(10.7, 13.4)
            line(10.6, 21); line(18.5, 10.6); line(13.3, 10.6)
            path.closeSubpath()

        case .server:
            rounded(3.5, 3.6, 17, 7.2, 1.8)
            rounded(3.5, 13.2, 17, 7.2, 1.8)
            dot(7, 7.2)
            dot(7, 16.8)

        case .detach:
            rounded(4, 7, 16, 12, 2)
            move(9, 4.5); line(15, 4.5)
            move(12, 4.5); line(12, 7)
            move(17, 12); line(21, 12)
            move(19, 10); line(21, 12); line(19, 14)

        case .toggle:
            rounded(2.6, 7, 18.8, 10, 5)
            circle(16.4, 12, 3.2)

        case .people:
            circle(9, 8.4, 3.3)
            path.addArc(center: p(9, 19.2), radius: 5.6 * s, startAngle: .degrees(180),
                        endAngle: .degrees(0), clockwise: false)
            circle(17.2, 8.6, 2.6)
            move(14.6, 14.6)
            path.addQuadCurve(to: p(20.6, 18.6), control: p(19.2, 14.8))

        case .stop:
            rounded(6.5, 6.5, 11, 11, 1.8)

        case .filter:
            move(4, 7); line(20, 7)
            move(7, 12); line(17, 12)
            move(10, 17); line(14, 17)

        case .down:
            move(12, 5); line(12, 19)
            move(6, 13); line(12, 19); line(18, 13)

        case .chevronUpDown:
            move(8, 9.5); line(12, 5.5); line(16, 9.5)
            move(8, 14.5); line(12, 18.5); line(16, 14.5)
        }
        return path
    }
}

// MARK: - SF Symbol mapping

extension SeIcon {
    /// SF Symbol → Nocturne glyph. Lanes that still draw `Image(systemName:)`
    /// migrate by looking the symbol up here and rendering `SeIconView` instead:
    ///
    ///     SeIcon(systemName: "terminal").map { SeIconView(icon: $0, size: 16) }
    ///
    /// A `nil` answer means the glyph has no Nocturne equivalent yet: keep the
    /// SF Symbol until one is drawn rather than substituting a wrong metaphor.
    /// Selection controls (`circle`, `largecircle.fill.circle`) are intentionally
    /// absent — they are state, not iconography.
    ///
    /// Call sites still to migrate (owned by other lanes, not edited here):
    /// - tools IA: `Features/Tools/ToolsView.swift:35`, `Features/Terminals/TerminalsHubScreen.swift:194,216`,
    ///   `Features/Terminals/TerminalsHubQuickSection.swift:135,169,200`,
    ///   `Features/Workspace/WorkspaceHubView.swift:105,291,298`, `Features/Search/SearchFeatureView.swift:82,97`,
    ///   `Features/ScreenText/ScreenTextFeatureView.swift:113,138,147`, `Views/SessionScreen.swift:155,194`
    /// - workbench chrome: `Features/Workspace/NocturneToolChrome.swift:72`, `Views/RootView.swift:37,59,64`,
    ///   `Files/WorkspaceFileListView.swift:77,92,139`, `Files/FileBrowserView.swift:139`
    /// - desktop sheet + viewfinder: `Features/Desktop/DesktopFeatureView.swift:233,253`,
    ///   `Features/QuickTerminals/QuickTerminalConsoleView.swift:147`
    /// - design system internals: `App/DesignSystem/SeNocturneKit.swift:901` (`info.circle` → `.info`),
    ///   `Chat/ChatTimelineRows.swift:309`, `Chat/ChatView.swift:238,485,502`,
    ///   `Features/AgentChat/AgentChatView.swift:161,181`, `Terminal/TerminalInputDock.swift:252`,
    ///   `Terminal/TerminalTabStrip.swift:58`, `Features/QuickTerminals/QuickTerminalsFeatureView.swift:188,222,252`
    init?(systemName: String) {
        guard let icon = Self.systemNameMap[systemName] else { return nil }
        self = icon
    }

    /// The full migration table. Multiple symbols may collapse onto one glyph.
    nonisolated static let systemNameMap: [String: SeIcon] = [
        // Navigation and disclosure
        "chevron.left": .back,
        "arrow.left": .back,
        "chevron.right": .chevron,
        "chevron.down": .chevronDown,
        "chevron.up.chevron.down": .chevronUpDown,
        "arrow.up.arrow.down": .chevronUpDown,
        "ellipsis": .more,
        "ellipsis.circle": .more,
        "gearshape": .gear,
        "gearshape.fill": .gear,
        "wrench": .gear,
        "wrench.and.screwdriver": .gear,
        // Destinations
        "terminal": .terminal,
        "apple.terminal": .terminal,
        "folder": .folder,
        "folder.fill": .folder,
        "display": .display,
        "desktopcomputer": .desktop,
        "rectangle.on.rectangle": .display,
        "server.rack": .server,
        "person.2": .people,
        "person.2.fill": .people,
        "clock": .clock,
        "timer": .clock,
        "git.branch": .branch,
        // Actions
        "arrow.clockwise": .refresh,
        "arrow.counterclockwise": .refresh,
        "arrow.up": .up,
        "arrow.up.circle.fill": .up,
        "square.and.arrow.up": .up,
        "arrow.down": .down,
        "arrow.down.circle": .down,
        "square.and.arrow.down": .down,
        "arrow.right": .arrowRight,
        "paperplane": .send,
        "paperplane.fill": .send,
        "plus": .plus,
        "plus.circle": .plus,
        "xmark": .close,
        "xmark.circle": .close,
        "xmark.circle.fill": .close,
        "checkmark": .check,
        "checkmark.circle": .check,
        "play": .play,
        "play.fill": .play,
        "stop.fill": .stop,
        "bolt": .bolt,
        "bolt.fill": .bolt,
        "bolt.horizontal.circle": .bolt,
        "bolt.horizontal.circle.fill": .bolt,
        "magnifyingglass": .search,
        "info.circle": .info,
        "info.circle.fill": .info,
        "lock": .lock,
        "lock.fill": .lock,
        "keyboard": .keyboard,
        "keyboard.chevron.compact.down": .keyboardDown,
        "rectangle.portrait.and.arrow.right": .detach,
        "cursorarrow": .pointer,
        "cursorarrow.rays": .pointer,
        "square.on.square": .copy,
        "doc.on.doc": .copy,
        "doc.on.doc.fill": .copy,
        "line.3.horizontal.decrease": .filter,
        "arrow.up.left.and.arrow.down.right": .expand,
        "arrow.down.right.and.arrow.up.left": .expand,
        "rectangle.arrowtriangle.2.inward": .fit,
        // Preview and capture
        "doc.text": .doc,
        "doc.plaintext": .doc,
        "photo": .image,
        "photo.fill": .image,
        "photo.on.rectangle": .image,
        "photo.on.rectangle.angled": .image,
        "chevron.left.forwardslash.chevron.right": .code,
        "curlybraces": .code,
        "crop": .viewfinder,
        "viewfinder": .viewfinder,
        "arrow.up.left.and.down.right.magnifyingglass": .viewfinder,
    ]
}

/// Small filled status dot used by the strips and rows.
struct SeNeedDot: View {
    enum Kind: Sendable { case live, wait, fail, off, idle }

    let kind: Kind
    var size: CGFloat = 6

    @Environment(\.sePalette) private var palette

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }

    private var color: Color {
        switch kind {
        case .live: palette.live
        case .wait: palette.warning
        case .fail: palette.danger
        case .off: palette.inkTertiary
        case .idle: palette.inkTertiary
        }
    }
}
