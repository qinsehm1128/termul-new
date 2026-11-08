import Testing
@testable import SeRemote

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
        #expect(ScreenTextPresentation.explanation(.degraded).contains("hidden"))
        #expect(ScreenTextPresentation.explanation(.disabled).contains("unaffected"))
    }
}
