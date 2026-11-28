import Foundation
import Testing
@testable import SeRemote

/// The canvas feed is installed by a long-lived store, so it is the one place a
/// dismantled SwiftTerm view could stay alive and keep parsing host output.
@Suite("Terminal feed lifecycle")
@MainActor
struct TerminalFeedLifecycleTests {
    private func liveTerminal(_ id: String) -> LiveTerminal {
        LiveTerminal(
            id: id,
            claim: nil,
            lastSeq: 0,
            cols: 80,
            rows: 24,
            title: "zsh",
            cwd: "/repo",
            gitBranch: nil,
            owned: false
        )
    }

    @Test("Dismantling a canvas stops delivery and buffers instead")
    func uninstallStopsDelivery() {
        let store = TerminalStore()
        store.terminals = [liveTerminal("A")]
        let canvas = NSObject()
        let owner = TerminalCanvasOwner(canvas)
        var received = Data()
        store.installFeed(for: "A", owner: owner) { data, _ in received.append(data) }

        store.deliverOutput(terminalId: "A", data: Data("live".utf8))
        #expect(received == Data("live".utf8))

        store.uninstallFeed(owner: owner)
        store.deliverOutput(terminalId: "A", data: Data("pending".utf8))

        #expect(received == Data("live".utf8), "an unmounted canvas must not receive bytes")
        #expect(store.pendingOutput["A"] == Data("pending".utf8))
    }

    @Test("A stale teardown cannot clear a newer canvas feed")
    func staleTeardownIsIgnored() {
        let store = TerminalStore()
        store.terminals = [liveTerminal("A")]
        let oldCanvas = NSObject()
        let newCanvas = NSObject()
        store.installFeed(for: "A", owner: TerminalCanvasOwner(oldCanvas)) { _, _ in }
        var received = Data()
        store.installFeed(for: "A", owner: TerminalCanvasOwner(newCanvas)) { data, _ in
            received.append(data)
        }

        store.uninstallFeed(owner: TerminalCanvasOwner(oldCanvas))
        store.deliverOutput(terminalId: "A", data: Data("still-live".utf8))
        #expect(received == Data("still-live".utf8))
    }

    @Test("Uninstalling releases the canvas the store was retaining")
    func uninstallReleasesCanvas() {
        final class Canvas {}
        let store = TerminalStore()
        store.terminals = [liveTerminal("A")]
        weak var probe: Canvas?
        do {
            let canvas = Canvas()
            probe = canvas
            let owner = TerminalCanvasOwner(canvas)
            store.installFeed(for: "A", owner: owner) { [canvas] _, _ in _ = canvas }
            #expect(probe != nil)
            store.uninstallFeed(owner: owner)
        }
        #expect(probe == nil, "the store must not keep an unmounted canvas alive")
    }

    @Test("Stale backlogs are evicted least-recently-touched first")
    func aggregateBudgetEvictsStaleBacklogs() {
        let megabyte = 1024 * 1024
        let buffers = [
            "A": Data(repeating: 1, count: 4 * megabyte),
            "B": Data(repeating: 2, count: 4 * megabyte),
            "C": Data(repeating: 3, count: 4 * megabyte),
        ]
        let evicted = TerminalOutputBudget.evictedIDs(
            buffers: buffers,
            limit: 8 * megabyte,
            protecting: ["C"],
            order: ["A", "B", "C"]
        )
        #expect(evicted == ["A"])
        #expect(
            TerminalOutputBudget.evictedIDs(
                buffers: buffers,
                limit: 12 * megabyte,
                protecting: [],
                order: ["A", "B", "C"]
            ).isEmpty
        )
    }

    @Test("A catalog refresh prunes unlisted shells but never the live feed")
    func staleIDsSkipProtectedTerminals() {
        let stale = TerminalOutputBudget.staleIDs(
            buffers: ["A": Data(), "B": Data(), "C": Data()],
            listed: ["C"],
            protecting: ["A"]
        )
        #expect(stale.sorted() == ["B"])
    }
}
