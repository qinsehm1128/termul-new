import Testing
@testable import SeRemote

@Suite("Workspace terminal input")
struct WorkspaceTerminalControlModifierTests {
    @Test("A short tap latches exactly one compatible key")
    func latchedControl() {
        var modifier = TerminalControlModifier()
        modifier.beginPress()
        #expect(modifier.isHeld)
        modifier.endPress(isTap: true)
        #expect(modifier.isLatched)
        #expect(!modifier.isHeld)
        #expect(modifier.apply(to: "c") == "\u{03}")
        #expect(!modifier.isActive)
        #expect(modifier.apply(to: "c") == "c")
    }

    @Test("A second tap unlatches without sending terminal bytes")
    func unlatch() {
        var modifier = TerminalControlModifier()
        modifier.toggleLatch()
        modifier.beginPress()
        modifier.endPress(isTap: true)
        #expect(!modifier.isActive)
    }

    @Test("Held control supports repeated keys and does not latch on release")
    func heldControl() {
        var modifier = TerminalControlModifier()
        modifier.beginPress()
        #expect(modifier.apply(to: "C") == "\u{03}")
        #expect(modifier.apply(to: "d") == "\u{04}")
        #expect(modifier.isHeld)
        modifier.endPress(isTap: false)
        #expect(!modifier.isActive)
    }

    @Test("Input during even a short hold prevents an accidental latch")
    func usedShortHold() {
        var modifier = TerminalControlModifier()
        modifier.beginPress()
        #expect(modifier.apply(to: "z") == "\u{1a}")
        modifier.endPress(isTap: true)
        #expect(!modifier.isActive)
    }

    @Test("A cancelled hold releases while preserving an existing latch")
    func cancelledHold() {
        var modifier = TerminalControlModifier()
        modifier.beginPress()
        modifier.releaseHold()
        #expect(!modifier.isActive)
        modifier.toggleLatch()
        modifier.beginPress()
        modifier.releaseHold()
        #expect(modifier.isLatched)
        #expect(!modifier.isHeld)
    }

    @Test("Paste, escape sequences, existing control bytes and IME text are unchanged")
    func safeInput() {
        var modifier = TerminalControlModifier()
        modifier.toggleLatch()
        for input in ["npm test\r", "\u{1b}[A", "\u{03}", "é", "中", "🙂", "", "1"] {
            #expect(modifier.apply(to: input) == input)
            #expect(modifier.isLatched)
        }
        #expect(modifier.apply(to: "l") == "\u{0c}")
        #expect(!modifier.isLatched)
    }

    @Test("ASCII punctuation follows standard terminal control mappings")
    func punctuation() {
        let pairs: [(String, String)] = [
            ("@", "\u{00}"), (" ", "\u{00}"), ("[", "\u{1b}"),
            ("\\", "\u{1c}"), ("]", "\u{1d}"), ("^", "\u{1e}"),
            ("_", "\u{1f}"), ("?", "\u{7f}")
        ]
        for (input, output) in pairs {
            #expect(TerminalControlModifier.controlSequence(for: input) == output)
        }
    }

    @Test("Existing accessory keys keep their exact PTY byte sequences")
    func accessoryCatalog() {
        let bytes = TerminalAccessoryCatalog.keys.map(\.bytes)
        #expect(bytes == [
            "\u{1b}", "\t", "\r", "\u{7f}", "\u{1b}[A", "\u{1b}[B", "\u{1b}[D", "\u{1b}[C",
            "\u{1b}[5~", "\u{1b}[6~",
            "\u{03}", "\u{04}", "\u{1a}", "\u{0c}", "\u{12}", "\u{01}", "\u{05}", "\u{17}", "\u{15}"
        ])
        #expect(TerminalAccessoryCatalog.keys.filter(\.repeatable).count == 7)
    }
}
