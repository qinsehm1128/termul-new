import Foundation
import Testing
@testable import SeRemote

/// The phone asks the host for binary PTY output (`se-terminal-v2.binary`).
/// JSON output spends about four characters per byte, which is what queued
/// replies behind replays on slow links.
@Suite("Binary terminal output")
struct TerminalBinaryOutputTests {
    private typealias Frame = TerminalSocket.OutputFrame

    private static func wire(kind: UInt8, id: String, seq: UInt64, bytes: [UInt8]) -> Data {
        var data = Data("TML2".utf8)
        data.append(kind)
        let idBytes = Array(id.utf8)
        data.append(UInt8(idBytes.count >> 8))
        data.append(UInt8(idBytes.count & 0xFF))
        for shift in stride(from: 56, through: 0, by: -8) {
            data.append(UInt8((seq >> UInt64(shift)) & 0xFF))
        }
        data.append(contentsOf: idBytes)
        data.append(contentsOf: bytes)
        return data
    }

    private static func frame(_ bytes: [UInt8], gap: Bool = false, seq: Int? = nil) -> Frame {
        Frame(terminalId: "pty-1", data: Data(bytes), gap: gap, latestSeq: seq)
    }

    // MARK: - Wire format

    @Test("Decodes the host's reference frame")
    func decodesHostReferenceFrame() {
        // Same vector as `binary_output_frame_uses_stable_big_endian_envelope`
        // in src-tauri/src/web/terminal_ws.rs.
        let data = Self.wire(kind: 2, id: "pty-1", seq: 0x0102_0304_0506_0708, bytes: [0, 0xFF, 0x41])
        #expect(data.count == 23)
        let frame = TerminalSocket.binaryOutputFrame(from: data)
        #expect(frame?.terminalId == "pty-1")
        #expect(frame?.data == Data([0, 0xFF, 0x41]))
        #expect(frame?.latestSeq == 0x0102_0304_0506_0708)
        #expect(frame?.gap == false)
    }

    @Test("A live frame with no payload still carries its cursor")
    func decodesEmptyPayload() {
        let frame = TerminalSocket.binaryOutputFrame(from: Self.wire(kind: 1, id: "t", seq: 9, bytes: []))
        #expect(frame?.data.isEmpty == true)
        #expect(frame?.latestSeq == 9)
    }

    @Test("Decoding does not assume a zero-based buffer")
    func decodesASlice() {
        var padded = Data([0xAA, 0xBB, 0xCC])
        padded.append(Self.wire(kind: 1, id: "pty-1", seq: 7, bytes: [1, 2, 3]))
        let slice = padded.dropFirst(3)
        #expect(slice.startIndex == 3)
        let frame = TerminalSocket.binaryOutputFrame(from: slice)
        #expect(frame?.terminalId == "pty-1")
        #expect(frame?.data == Data([1, 2, 3]))
    }

    @Test("Anything that is not a well-formed frame is rejected")
    func rejectsMalformedFrames() {
        let good = Self.wire(kind: 1, id: "pty-1", seq: 1, bytes: [1])
        var wrongMagic = good
        wrongMagic[0] = 0x58
        #expect(TerminalSocket.binaryOutputFrame(from: wrongMagic) == nil)
        #expect(TerminalSocket.binaryOutputFrame(from: Self.wire(kind: 3, id: "pty-1", seq: 1, bytes: [1])) == nil)
        #expect(TerminalSocket.binaryOutputFrame(from: Self.wire(kind: 1, id: "", seq: 1, bytes: [1])) == nil)
        #expect(TerminalSocket.binaryOutputFrame(from: good.prefix(14)) == nil)
        // Header claims a 5-byte id but only 2 bytes follow.
        #expect(TerminalSocket.binaryOutputFrame(from: good.prefix(17)) == nil)
        #expect(TerminalSocket.binaryOutputFrame(from: Self.wire(kind: 1, id: "t", seq: UInt64.max, bytes: [])) == nil)
        #expect(TerminalSocket.binaryOutputFrame(from: Data("{\"type\":\"data\"}".utf8)) == nil)
    }

    // MARK: - Gap handling around an attach

    @Test("Without a gap the held output is released unchanged")
    func noGapReleasesUnchanged() {
        let held = [Self.frame([1], seq: 1), Self.frame([2], seq: 2)]
        let released = TerminalSocket.framesAfterAttach(held, replyGap: false)
        #expect(released.frames == held)
        #expect(released.gapCarries == false)
    }

    @Test("A gap in the reply resets before the first replay frame")
    func replyGapMarksFirstFrame() {
        let held = [Self.frame([1], seq: 1), Self.frame([2], seq: 2)]
        let released = TerminalSocket.framesAfterAttach(held, replyGap: true)
        #expect(released.frames.map(\.gap) == [true, false])
        #expect(released.frames.map(\.data) == held.map(\.data))
        #expect(released.frames.map(\.latestSeq) == [1, 2])
        #expect(released.gapCarries == false)
    }

    @Test("A gap reported before any replay frame is carried to the next one")
    func replyGapWithNothingHeldCarries() {
        let released = TerminalSocket.framesAfterAttach([], replyGap: true)
        #expect(released.frames.isEmpty)
        #expect(released.gapCarries == true)
    }

    @Test("A binary host's gap trailer does not erase the replay before it")
    func gapTrailerKeepsReplay() {
        // In-process hosts send the binary chunks, then a JSON `replay` frame
        // with no chunks that carries the gap verdict.
        let held = [Self.frame([1], seq: 1), Self.frame([2], seq: 2), Self.frame([], gap: true, seq: 2)]
        let released = TerminalSocket.framesAfterAttach(held, replyGap: false)
        #expect(released.frames.map(\.gap) == [true, false, false])
        #expect(released.frames.map(\.data) == [Data([1]), Data([2]), Data()])
    }

    @Test("A JSON host's gapped replay drops the previous stream's bytes")
    func gappedReplayDropsOlderBytes() {
        let held = [Self.frame([9], seq: 4), Self.frame([1, 2], gap: true, seq: 8), Self.frame([3], seq: 9)]
        let released = TerminalSocket.framesAfterAttach(held, replyGap: true)
        #expect(released.frames.map(\.data) == [Data([1, 2]), Data([3])])
        #expect(released.frames.map(\.gap) == [true, false])
    }
}
