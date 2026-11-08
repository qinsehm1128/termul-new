import Foundation
import Testing
@testable import SeRemote

@Suite("Shell connection lifetime")
struct ShellConnectionLifecycleTests {
    @Test("An animated disappearance cannot stop the active connection")
    func transitionKeepsActiveSession() {
        let linkID = UUID()
        #expect(!ShellConnectionLifecycle.shouldStop(disappearingLinkID: linkID, activeLinkID: linkID))
    }

    @Test("Explicit disconnect tears down the departing session")
    func disconnectStopsSession() {
        #expect(ShellConnectionLifecycle.shouldStop(disappearingLinkID: UUID(), activeLinkID: nil))
    }

    @Test("Replacing a host tears down only the old session")
    func replacementStopsOldSession() {
        let old = UUID()
        let replacement = UUID()
        #expect(ShellConnectionLifecycle.shouldStop(disappearingLinkID: old, activeLinkID: replacement))
        #expect(!ShellConnectionLifecycle.shouldStop(disappearingLinkID: replacement, activeLinkID: replacement))
    }
}
