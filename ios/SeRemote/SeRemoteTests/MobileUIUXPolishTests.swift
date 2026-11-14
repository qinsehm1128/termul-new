import Foundation
import Testing
@testable import SeRemote

/// Item #4 — mobile UI/UX polish rules that must stay true as pure logic:
/// no duplicated status text, no false empty states, no misleading labels.
@Suite("Mobile UI/UX polish")
struct MobileUIUXPolishTests {
    // MARK: - Conversations row meta

    @Test("Conversation meta joins agent and project without repeating status text")
    func conversationRowMetaFormatsAgentAndProject() {
        #expect(ConversationsScreen.rowMeta(agent: "Claude Code", project: "termul") == "Claude Code · termul")
    }

    @Test("Conversation meta omits the agent when it is empty")
    func conversationRowMetaHandlesEmptyAgent() {
        #expect(ConversationsScreen.rowMeta(agent: "", project: "termul") == "termul")
        #expect(ConversationsScreen.rowMeta(agent: "   ", project: "termul") == "termul")
    }

    @Test("Conversation meta never contains a status word")
    func conversationRowMetaNeverDuplicatesStatus() {
        // The status ("Ready"/"Running"/…) renders in SeSessionRow.trailing;
        // the meta line must stay free of it even for well-known words.
        let meta = ConversationsScreen.rowMeta(agent: "Claude Code", project: "termul")
        #expect(!meta.contains("Ready"))
        #expect(!meta.contains("Running"))
        #expect(!meta.contains("· ·"))
    }

    // MARK: - Terminals landing subtitle

    @Test("Landing subtitle is nil at zero terminals: no duplicated host status")
    func landingSubtitleIsNilWhenEmpty() {
        #expect(TerminalLandingScreen.subtitle(for: 0) == nil)
    }

    @Test("Landing subtitle is a plain count once terminals exist")
    func landingSubtitleCountsTerminals() {
        let countOne = 1
        let countThree = 3
        #expect(TerminalLandingScreen.subtitle(for: countOne) == String(localized: "1 terminal"))
        #expect(TerminalLandingScreen.subtitle(for: countThree) == String(localized: "\(countThree) terminals"))
    }

    @Test("Landing subtitle never carries a second host-status word")
    func landingSubtitleNeverRepeatsHostMeta() {
        for count in 0...5 {
            let subtitle = TerminalLandingScreen.subtitle(for: count)
            #expect(subtitle?.contains("Connected") != true)
            #expect(subtitle?.contains("Connecting") != true)
        }
    }

    // MARK: - Initial-load state

    @Test("Hub reports loading while idle/loading with no summaries yet")
    func hubIsLoadingBeforeFirstHostAnswer() {
        #expect(TerminalsHubScreen.isLoading(state: .idle, summaryCount: 0))
        #expect(TerminalsHubScreen.isLoading(state: .loading, summaryCount: 0))
    }

    @Test("Hub stops loading once the host answered, even with an empty list")
    func hubIsLoadedAfterHostAnswer() {
        #expect(!TerminalsHubScreen.isLoading(state: .empty, summaryCount: 0))
        #expect(!TerminalsHubScreen.isLoading(state: .loaded, summaryCount: 0))
        #expect(!TerminalsHubScreen.isLoading(state: .loaded, summaryCount: 2))
        #expect(!TerminalsHubScreen.isLoading(state: .failed("boom"), summaryCount: 0))
    }

    // MARK: - Settings back target

    @Test("Settings back returns to the last real tab, never to Settings itself")
    func settingsBackReturnsToLastTab() {
        #expect(ConnectedShellScreen.settingsBackTarget(from: .tools) == .tools)
        #expect(ConnectedShellScreen.settingsBackTarget(from: .conversations) == .conversations)
        #expect(ConnectedShellScreen.settingsBackTarget(from: .terminals) == .terminals)
        // A remembered Settings (never stored in practice) falls back safely.
        #expect(ConnectedShellScreen.settingsBackTarget(from: .settings) == .terminals)
    }
}
