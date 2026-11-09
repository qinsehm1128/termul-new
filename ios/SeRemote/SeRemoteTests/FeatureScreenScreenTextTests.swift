import Foundation
import Testing
@testable import SeRemote

/// Resolve a catalog key the same way the app does, so label assertions stay
/// locale-independent.
private func L(_ key: String) -> String {
    String(localized: String.LocalizationValue(key))
}

/// Map a localized sentence back to its English source key, so wording checks
/// do not depend on the language the test process runs in.
private func englishSource(of localized: String) -> String {
    guard let path = Bundle.main.path(forResource: "zh-Hans", ofType: "lproj"),
          let table = NSDictionary(contentsOfFile: path + "/Localizable.strings") as? [String: String]
    else { return localized }
    return table.first { $0.value == localized }?.key ?? localized
}

@MainActor
struct FeatureScreenScreenTextTests {
    @Test func observationStatesNeverPresentDegradedAsSuccessful() {
        #expect(ScreenTextPresentation.semanticStatus(.ready) == .completed)
        #expect(ScreenTextPresentation.semanticStatus(.starting) == .working)
        #expect(ScreenTextPresentation.semanticStatus(.catchingUp) == .working)
        #expect(ScreenTextPresentation.semanticStatus(.degraded) == .needsApproval)
        #expect(ScreenTextPresentation.semanticStatus(.disabled) == .offline)
        #expect(ScreenTextPresentation.semanticStatus(.unsupported) == .offline)
    }

    @Test func everyObservationStateHasPlainLanguageExplanation() {
        for state: HostScreenObservation.State in [.ready, .starting, .catchingUp, .degraded, .disabled, .unsupported] {
            #expect(!ScreenTextPresentation.explanation(state).isEmpty)
        }
        #expect(englishSource(of: ScreenTextPresentation.explanation(.degraded)).contains("hidden"))
        #expect(englishSource(of: ScreenTextPresentation.explanation(.disabled)).contains("unaffected"))
        #expect(ScreenTextPresentation.explanation(.degraded) == L("The host's screen observation is incomplete or out of sync. Text is hidden until a consistent snapshot is available."))
        #expect(ScreenTextPresentation.explanation(.disabled) == L("Screen observation is disabled on this host. The terminal itself is unaffected."))
    }
}
