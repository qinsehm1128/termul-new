import Foundation
import Testing
@testable import SeRemote

@MainActor
@Suite("Nocturne session presentation")
struct NocturneSessionPresentationTests {
    @Test("Only a connected transport can advertise live work")
    func connectivityPrecedesAttention() {
        for phase in [SessionPhase.idle, .connecting, .failed("Host unavailable")] {
            let status = NocturneSessionPresentation.status(
                phase: phase, connected: true, permissions: 2, questions: 1, streaming: true
            )
            #expect(status.tone != .live)
            #expect(status.tone != .attention)
        }
        let status = NocturneSessionPresentation.status(
            phase: .connected, connected: false, permissions: 1, questions: 1, streaming: true
        )
        #expect(status.tone == .quiet)
        #expect(status.title == String(localized: "Reconnecting…"))
    }

    @Test("Permission and question requests take priority over streaming")
    func attentionPriority() {
        let permission = NocturneSessionPresentation.status(
            phase: .connected, connected: true, permissions: 1, questions: 1, streaming: true
        )
        #expect(permission.tone == .attention)
        #expect(permission.title == String(localized: "Permission required"))
        let question = NocturneSessionPresentation.status(
            phase: .connected, connected: true, permissions: 0, questions: 1, streaming: true
        )
        #expect(question.title == String(localized: "Answer required"))
        let working = NocturneSessionPresentation.status(
            phase: .connected, connected: true, permissions: 0, questions: 0, streaming: true
        )
        #expect(working.title == String(localized: "Working"))
        let ready = NocturneSessionPresentation.status(
            phase: .connected, connected: true, permissions: 0, questions: 0, streaming: false
        )
        #expect(ready.title == String(localized: "Connected"))
    }

    @Test("Unavailable chat cannot be selected in project-only workspaces")
    func modeAvailability() {
        for tab in WorkspaceTab.allCases {
            #expect(NocturneSessionPresentation.modeAvailable(tab, workspace: .conversation))
            #expect(NocturneSessionPresentation.modeAvailable(tab, workspace: .project) == (tab != .chat))
            #expect(NocturneSessionPresentation.modeAvailable(tab, workspace: .home) == (tab != .chat))
        }
    }

    @Test("Unknown metadata is omitted, never replaced by preview agent or model")
    func honestMetadata() {
        #expect(NocturneSessionPresentation.subtitle(host: "Mac", agent: nil, model: nil) == "Mac")
        #expect(NocturneSessionPresentation.subtitle(host: "Mac", agent: "  ", model: "") == "Mac")
        #expect(NocturneSessionPresentation.subtitle(host: "Mac", agent: "Claude Code", model: "Sonnet")
                == "Mac · Claude Code · Sonnet")
    }

    @Test("Failed or still-pending permission submissions have no completion haptic")
    func ackOnlyPermissionFeedback() {
        for option in [nil, "allow_once", "reject_once"] as [String?] {
            #expect(WorkspaceChatPresentation.acknowledgedPermissionFeedback(
                optionID: option, optionName: nil, acknowledged: false
            ) == nil)
        }
        #expect(WorkspaceChatPresentation.acknowledgedPermissionFeedback(
            optionID: "allow_once", optionName: "Allow once", acknowledged: true
        ) == .approval)
        #expect(WorkspaceChatPresentation.acknowledgedPermissionFeedback(
            optionID: nil, optionName: nil, acknowledged: true
        ) == .denial)
    }
}
