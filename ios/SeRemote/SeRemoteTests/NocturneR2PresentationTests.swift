import Foundation
import Testing
@testable import SeRemote

@Suite("Nocturne R2 presentation")
struct NocturneR2PresentationTests {
    private func entry(_ name: String, directory: Bool = false) -> DirectoryEntry {
        DirectoryEntry(name: name, path: "/root/\(name)", type: directory ? "directory" : "file")
    }

    @Test("Agent mark uses the real name and never invents a brand")
    func agentMark() {
        #expect(NocturneR2Presentation.agentMark("Claude Code") == "CC")
        #expect(NocturneR2Presentation.agentMark("qin-code") == "QI")
        #expect(NocturneR2Presentation.agentMark("pi") == "PI")
        #expect(NocturneR2Presentation.agentMark("  ") == "Ag")
        #expect(NocturneR2Presentation.agentMark("") == "Ag")
    }

    @Test("Tool status maps to the prototype's four state tones")
    func toolTone() {
        #expect(NocturneR2Presentation.toolTone("failed") == .failed)
        #expect(NocturneR2Presentation.toolTone("pending") == .attention)
        #expect(NocturneR2Presentation.toolTone("in_progress") == .attention)
        #expect(NocturneR2Presentation.toolTone("completed") == .live)
        #expect(NocturneR2Presentation.toolTone("success") == .live)
        #expect(NocturneR2Presentation.toolTone("cancelled") == .quiet)
        #expect(NocturneR2Presentation.toolTone("host_cancelled") == .quiet)
    }

    @Test("Folder summary counts the host listing, not a prototype sample")
    func folderSummary() {
        #expect(NocturneR2Presentation.folderSummary(name: "net", count: 8) == "net · 8 files")
        #expect(NocturneR2Presentation.folderSummary(name: "  ", count: 0) == "Files · 0 files")
        #expect(NocturneR2Presentation.folderSummary(name: nil, count: 3) == "Files · 3 files")
    }

    @Test("PTY meta label reflects the live transport state")
    func terminalMeta() {
        #expect(NocturneR2Presentation.terminalMetaLabel(isConnecting: true, isConnected: false) == "PTY · Connecting")
        #expect(NocturneR2Presentation.terminalMetaLabel(isConnecting: true, isConnected: true) == "PTY · Connecting")
        #expect(NocturneR2Presentation.terminalMetaLabel(isConnecting: false, isConnected: true) == "PTY · Ready")
        #expect(NocturneR2Presentation.terminalMetaLabel(isConnecting: false, isConnected: false) == "PTY · Offline")
    }

    @Test("Only non-directory entries count as readable files")
    func readableFileCount() {
        let entries = [
            entry("src", directory: true),
            entry("a.swift"),
            entry("b.swift"),
            entry("assets", directory: true)
        ]
        #expect(NocturneR2Presentation.readableFileCount(entries) == 2)
        #expect(NocturneR2Presentation.readableFileCount([]) == 0)
    }

    @Test("Ribbon detail omits empty or duplicated metadata")
    func ribbonDetail() {
        #expect(NocturneR2Presentation.ribbonDetail(agent: nil, model: nil, readableFiles: 0) == nil)
        #expect(NocturneR2Presentation.ribbonDetail(agent: "  ", model: "", readableFiles: 0) == nil)
        #expect(NocturneR2Presentation.ribbonDetail(agent: "Claude Code", model: "Claude Code", readableFiles: 0) == "Claude Code")
        #expect(NocturneR2Presentation.ribbonDetail(agent: "Claude Code", model: "Sonnet 4.5", readableFiles: 3)
                == "Claude Code · Sonnet 4.5 · 3 readable files")
        #expect(NocturneR2Presentation.ribbonDetail(agent: nil, model: nil, readableFiles: 2) == "2 readable files")
    }

    @Test("Permission footer counts host options honestly")
    func optionSummary() {
        #expect(NocturneR2Presentation.optionSummary(0) == "0 options")
        #expect(NocturneR2Presentation.optionSummary(1) == "1 option")
        #expect(NocturneR2Presentation.optionSummary(2) == "2 options")
        #expect(NocturneR2Presentation.optionSummary(7) == "7 options")
    }

    @Test("Reasoning label distinguishes live and settled work")
    func reasonLabel() {
        #expect(NocturneR2Presentation.reasonLabel(thoughtCount: 2, active: true) == "Reasoning… · 2 steps")
        #expect(NocturneR2Presentation.reasonLabel(thoughtCount: 2, active: false) == "Reasoning summary · 2 steps")
    }
}
