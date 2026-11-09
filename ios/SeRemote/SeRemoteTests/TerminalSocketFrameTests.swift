import Foundation
import Testing
@testable import SeRemote

/// The terminal WebSocket carries output as `data` (live bytes) and `replay`
/// (backlog chunks). Host and client disagreed on that shape once already, so
/// pin the client mapping: a mismatch must show up here, not as a blank canvas.
@Suite("Terminal socket output frames")
struct TerminalSocketFrameTests {
    @Test func liveFrameCarriesItsBytes() {
        let output = TerminalSocket.output(from: [
            "type": "data",
            "terminalId": "terminal-1",
            "seq": 7,
            "data": [65, 66, 10]
        ])
        #expect(output?.terminalId == "terminal-1")
        #expect(output?.data == Data([65, 66, 10]))
    }

    @Test func replayFrameConcatenatesChunksInOrder() {
        let output = TerminalSocket.output(from: [
            "type": "replay",
            "terminalId": "terminal-2",
            "gap": false,
            "latestSeq": 12,
            "chunks": [
                ["seq": 11, "data": [104, 105]],
                ["seq": 12, "data": [10]]
            ]
        ])
        #expect(output?.terminalId == "terminal-2")
        #expect(output?.data == Data([104, 105, 10]))
    }

    @Test func onlyOutputFramesMapToBytes() {
        #expect(TerminalSocket.output(from: ["type": "event"]) == nil)
        #expect(TerminalSocket.output(from: ["type": "gap", "terminalId": "terminal-3"]) == nil)
        #expect(TerminalSocket.output(from: ["type": "data"]) == nil)
        #expect(TerminalSocket.output(from: ["type": "replay", "chunks": []]) == nil)
    }

    @Test func stringPayloadStillDecodesAsUTF8() {
        let output = TerminalSocket.output(from: [
            "type": "data",
            "terminalId": "terminal-4",
            "data": "ok\n"
        ])
        #expect(output?.data == Data("ok\n".utf8))
    }
}
