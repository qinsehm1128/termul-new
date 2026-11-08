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
}
