import SwiftUI
import Testing
@testable import SeRemote

@Suite("Design system")
struct DesignSystemTests {
    @Test("Compact or narrow widths stay single column", arguments: [
        (CGFloat(402), true), (CGFloat(700), true), (CGFloat(1200), false),
    ])
    func singleColumn(width: CGFloat, isRegular: Bool) {
        #expect(SeLayout.columns(width: width, isRegular: isRegular) == .single)
    }

    @Test("Regular width splits, and adds an inspector only with room")
    func splitThresholds() {
        #expect(SeLayout.columns(width: 720, isRegular: true) == .split)
        #expect(SeLayout.columns(width: 980, isRegular: true) == .split)
        #expect(SeLayout.columns(width: 1119, isRegular: true) == .split)
        #expect(SeLayout.columns(width: 1120, isRegular: true) == .splitWithInspector)
    }

    @Test("Large text folds earlier")
    func largeTextFoldsEarlier() {
        #expect(SeLayout.columns(width: 800, isRegular: true, largeText: false) == .split)
        #expect(SeLayout.columns(width: 800, isRegular: true, largeText: true) == .single)
    }

    @Test("Spacing scale matches the spec")
    func spacingScale() {
        let scale = [SeSpace.xxs, SeSpace.xs, SeSpace.sm, SeSpace.md, SeSpace.lg,
                     SeSpace.xl, SeSpace.xxl, SeSpace.xxxl, SeSpace.huge]
        #expect(scale == [4, 8, 12, 16, 20, 24, 32, 40, 48])
        #expect(SeSpace.hitTarget >= 44)
    }

    @Test("Hex colors decode channel by channel")
    func hexColor() {
        let resolved = Color(hex: 0x4E6431).resolve(in: EnvironmentValues())
        #expect(abs(Double(resolved.red) - 0x4E / 255.0) < 0.01)
        #expect(abs(Double(resolved.green) - 0x64 / 255.0) < 0.01)
        #expect(abs(Double(resolved.blue) - 0x31 / 255.0) < 0.01)
    }

    @Test("Every status pairs a title with a symbol")
    func statusesHaveTextAndSymbol() {
        let all: [SeStatus] = [.online, .connecting, .reconnecting, .offline, .working,
                               .needsApproval, .completed, .failed, .idle, .paused]
        for status in all {
            #expect(!status.title.isEmpty)
            #expect(!status.symbol.isEmpty)
        }
        #expect(HostRowStatus.need.seStatus == .needsApproval)
    }

    @Test("Haptic events map to distinct feedback kinds")
    func hapticMap() {
        #expect(SeHaptic.success.feedback == .success)
        #expect(SeHaptic.warning.feedback == .warning)
        #expect(SeHaptic.error.feedback == .error)
        #expect(SeHaptic.selection.feedback == .selection)
    }

    @Test("Nocturne mode follows the effective color scheme")
    func themeModeResolution() {
        #expect(SeThemeMode.resolved(colorScheme: .dark) == .night)
        #expect(SeThemeMode.resolved(colorScheme: .light) == .day)
    }

    @Test("Day/night toggle persists an explicit appearance")
    func themeModePersistence() {
        #expect(SeThemeMode.day.appearance == .light)
        #expect(SeThemeMode.night.appearance == .dark)
        #expect(SeThemeMode.day.toggled == .night)
        #expect(SeThemeMode.night.toggled == .day)
        // A persisted appearance round-trips through the color scheme it pins.
        for mode in SeThemeMode.allCases {
            let scheme = mode.appearance.colorScheme
            #expect(scheme != nil)
            if let scheme { #expect(SeThemeMode.resolved(colorScheme: scheme) == mode) }
        }
    }

    // MARK: - Icons (Phase 2 lane A)

    @Test("Every icon draws inside the 24pt grid", arguments: SeIcon.allCases)
    func iconsDraw(icon: SeIcon) {
        let path = SeIconShape(icon: icon).path(in: CGRect(x: 0, y: 0, width: 24, height: 24))
        #expect(!path.isEmpty, "\(icon.rawValue)")
        let bounds = path.boundingRect
        #expect(bounds.width > 0 && bounds.height > 0, "\(icon.rawValue)")
        #expect(bounds.minX >= 0 && bounds.minY >= 0, "\(icon.rawValue)")
        #expect(bounds.maxX <= 24 && bounds.maxY <= 24, "\(icon.rawValue)")
    }

    @Test("The desktop, capture and preview glyphs exist")
    func phaseTwoGlyphsExist() {
        let required: [SeIcon] = [
            .desktop, .viewfinder, .send, .image, .doc, .code, .markdown,
            .close, .expand, .keyboard, .pointer, .display, .fit,
        ]
        let all = Set(SeIcon.allCases)
        for icon in required { #expect(all.contains(icon), "\(icon.rawValue)") }
    }

    @Test("SF Symbol mapping covers the migration list")
    func symbolMapping() {
        let expected: [String: SeIcon] = [
            "chevron.left": .back,
            "chevron.right": .chevron,
            "chevron.up.chevron.down": .chevronUpDown,
            "ellipsis": .more,
            "gearshape": .gear,
            "wrench": .gear,
            "terminal": .terminal,
            "folder": .folder,
            "arrow.clockwise": .refresh,
            "square.and.arrow.up": .up,
            "arrow.up": .up,
            "arrow.down": .down,
            "arrow.right": .arrowRight,
            "xmark": .close,
            "xmark.circle.fill": .close,
            "checkmark": .check,
            "play": .play,
            "stop.fill": .stop,
            "plus": .plus,
            "magnifyingglass": .search,
            "info.circle": .info,
            "lock": .lock,
            "keyboard": .keyboard,
            "keyboard.chevron.compact.down": .keyboardDown,
            "display": .display,
            "desktopcomputer": .desktop,
            "server.rack": .server,
            "bolt.horizontal.circle": .bolt,
            "rectangle.portrait.and.arrow.right": .detach,
            "line.3.horizontal.decrease": .filter,
            "doc.text": .doc,
            "photo": .image,
            "paperplane": .send,
            "chevron.left.forwardslash.chevron.right": .code,
            "crop": .viewfinder,
        ]
        for (symbol, icon) in expected {
            #expect(SeIcon(systemName: symbol) == icon, "\(symbol)")
        }
        // Selection controls and unknown symbols stay unmapped on purpose.
        #expect(SeIcon(systemName: "circle") == nil)
        #expect(SeIcon(systemName: "largecircle.fill.circle") == nil)
        #expect(SeIcon(systemName: "definitely.not.a.symbol") == nil)
    }

    @Test("Every mapped symbol resolves to a drawable glyph")
    func mappedGlyphsDraw() {
        for (symbol, icon) in SeIcon.systemNameMap {
            let path = SeIconShape(icon: icon).path(in: CGRect(x: 0, y: 0, width: 24, height: 24))
            #expect(!path.isEmpty, "\(symbol)")
            #expect(SeIcon(systemName: symbol) == icon, "\(symbol)")
        }
    }

    // MARK: - Type scale (Phase 2 lane A)

    @Test("Type scale covers every in-app prototype size")
    func typeScaleCoversPrototype() {
        let expected: [CGFloat] = [
            7, 8, 8.5, 9, 9.5, 10, 10.5, 11, 11.5, 12, 12.5,
            13, 13.5, 14, 15, 17, 19, 24, 26, 30, 33,
        ]
        #expect(SeNocturneType.scale == expected)
        #expect(Set(SeNocturneType.scale).count == SeNocturneType.scale.count)
    }

    @Test("Mono tokens cover the console and label sizes")
    func monoTokens() {
        #expect(SeNocturneType.monoBodySize == 12)
        #expect(SeNocturneType.monoSize == 10)
        #expect(SeNocturneType.monoLabelSize == 9.5)
        #expect(SeNocturneType.monoSmallSize == 9)
        #expect(SeNocturneType.monoEyebrowSize == 8.5)
        #expect(SeNocturneType.microSize == 8)
        #expect(SeNocturneType.badgeSize == 7)
    }
}
