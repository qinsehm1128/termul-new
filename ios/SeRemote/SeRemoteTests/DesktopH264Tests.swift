import Foundation
import Testing
@testable import SeRemote

@Suite("Desktop H.264 P1")
struct DesktopH264Tests {
    private func header(width: Int = 1280, height: Int = 720, key: Bool = true,
                        ps: [Int], len: Int, seq: UInt64 = 1,
                        display: String = "display-1", type: String = "h264") -> [String: Any] {
        ["type": type, "display": display, "seq": seq, "w": width, "h": height,
         "key": key, "ps": ps, "len": len]
    }

    private func wire(_ header: [String: Any], parameterSets: [Data], sample: Data) -> Data {
        let headerData = (try? JSONSerialization.data(withJSONObject: header)) ?? Data()
        var data = Data()
        let length = UInt32(headerData.count)
        data.append(contentsOf: [
            UInt8((length >> 24) & 0xff), UInt8((length >> 16) & 0xff),
            UInt8((length >> 8) & 0xff), UInt8(length & 0xff),
        ])
        data.append(headerData)
        parameterSets.forEach { data.append($0) }
        data.append(sample)
        return data
    }

    private func avcc(_ bytes: [UInt8]) -> Data {
        var data = Data()
        let length = UInt32(bytes.count)
        data.append(contentsOf: [
            UInt8((length >> 24) & 0xff), UInt8((length >> 16) & 0xff),
            UInt8((length >> 8) & 0xff), UInt8(length & 0xff),
        ])
        data.append(contentsOf: bytes)
        return data
    }

    private func expectInvalid(_ data: Data) {
        #expect(throws: DesktopProtocolError.invalidPacket) {
            try DesktopH264Packet.decode(data)
        }
    }

    @Test func validKeyframeParses() throws {
        let sps = Data([0x67, 0x42, 0x00, 0x1f])
        let pps = Data([0x68, 0xce, 0x3c, 0x80])
        let sample = avcc([0x65, 0x88, 0x84])
        let data = wire(header(ps: [sps.count, pps.count], len: sample.count),
                        parameterSets: [sps, pps], sample: sample)
        let packet = try DesktopH264Packet.decode(data)
        #expect(packet.display == "display-1")
        #expect(packet.seq == 1)
        #expect(packet.width == 1280)
        #expect(packet.height == 720)
        #expect(packet.isKey)
        #expect(packet.parameterSets == [sps, pps])
        #expect(packet.sample == sample)
    }

    @Test func validDeltaFrameParses() throws {
        let sample = avcc([0x41, 0x9a])
        let data = wire(header(key: false, ps: [], len: sample.count, seq: 7),
                        parameterSets: [], sample: sample)
        let packet = try DesktopH264Packet.decode(data)
        #expect(!packet.isKey)
        #expect(packet.parameterSets.isEmpty)
        #expect(packet.seq == 7)
    }

    @Test func rejectsLengthMismatch() {
        let sample = avcc([0x65, 0x88])
        let data = wire(header(key: false, ps: [], len: sample.count + 1),
                        parameterSets: [], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsKeyWithoutParameterSets() {
        let sample = avcc([0x65, 0x88])
        let data = wire(header(key: true, ps: [], len: sample.count),
                        parameterSets: [], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsKeyWithOneParameterSet() {
        let sps = Data([0x67, 0x42])
        let sample = avcc([0x65, 0x88])
        let data = wire(header(key: true, ps: [sps.count], len: sample.count),
                        parameterSets: [sps], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsDeltaWithParameterSets() {
        let sps = Data([0x67, 0x42])
        let pps = Data([0x68, 0xce])
        let sample = avcc([0x41, 0x9a])
        let data = wire(header(key: false, ps: [sps.count, pps.count], len: sample.count),
                        parameterSets: [sps, pps], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsOddDimensions() {
        let sample = avcc([0x41, 0x9a])
        for (width, height) in [(1279, 720), (1280, 719)] {
            let data = wire(header(width: width, height: height, key: false, ps: [], len: sample.count),
                            parameterSets: [], sample: sample)
            expectInvalid(data)
        }
    }

    @Test func rejectsOversizedDimensions() {
        let sample = avcc([0x41, 0x9a])
        for (width, height) in [(16_386, 720), (1280, 16_384 + 2)] {
            let data = wire(header(width: width, height: height, key: false, ps: [], len: sample.count),
                            parameterSets: [], sample: sample)
            expectInvalid(data)
        }
    }

    @Test func rejectsOversizedPixelCount() {
        let sample = avcc([0x41, 0x9a])
        let data = wire(header(width: 8192, height: 4096, key: false, ps: [], len: sample.count),
                        parameterSets: [], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsOversizedPacket() {
        expectInvalid(Data(count: DesktopPacket.maxPacketBytes + 1))
    }

    @Test func rejectsBadAVCCLength() {
        let sample = Data([0x00, 0x00, 0x00, 0x05, 0x65, 0x88])
        let data = wire(header(key: false, ps: [], len: sample.count),
                        parameterSets: [], sample: sample)
        expectInvalid(data)
    }

    @Test func rejectsNonH264Type() {
        let sample = avcc([0x65, 0x88])
        let data = wire(header(key: false, ps: [], len: sample.count, type: "key"),
                        parameterSets: [], sample: sample)
        expectInvalid(data)
    }

    @Test func legacyStartOmitsH264Fields() {
        let message = DesktopStartRequest.message(display: "d1", codecs: ["jpeg"], h264Available: true)
        #expect(message["fps"] as? Int == 3)
        #expect(message["codec"] == nil)
        #expect(message["bitrate"] == nil)
        #expect(Set(message.keys) == ["type", "display", "fps", "maxWidth", "quality"])
    }

    @Test func h264StartIncludesCodecAndBitrate() {
        let message = DesktopStartRequest.message(display: "d1", codecs: ["jpeg", "h264"], h264Available: true)
        #expect(message["fps"] as? Int == 30)
        #expect(message["codec"] as? String == "h264")
        #expect(message["bitrate"] as? Int == 4000)
    }

    @Test func h264StartWithoutHardwareUsesLegacyPayload() {
        let message = DesktopStartRequest.message(display: "d1", codecs: ["h264"], h264Available: false)
        #expect(message["fps"] as? Int == 3)
        #expect(message["codec"] == nil)
        #expect(message["bitrate"] == nil)
    }

    @Test func infoWithoutCodecsDefaultsToJPEG() throws {
        let json = Data("""
        {"supported":true,"viewEnabled":true,"controlEnabled":false,
         "permissions":{"screenRecording":true,"accessibility":false},"displays":[]}
        """.utf8)
        let info = try JSONDecoder().decode(DesktopInfo.self, from: json)
        #expect(info.codecs == nil)
        #expect(info.supportedCodecs == ["jpeg"])
    }

    @Test func infoAdvertisesH264() throws {
        let json = Data("""
        {"supported":true,"viewEnabled":true,"controlEnabled":false,
         "permissions":{"screenRecording":true,"accessibility":false},
         "codecs":["h264","jpeg"],"displays":[]}
        """.utf8)
        let info = try JSONDecoder().decode(DesktopInfo.self, from: json)
        #expect(info.supportedCodecs.contains("h264"))
    }

    @Test func metaCodecDefaultsToJPEG() throws {
        #expect(DesktopCodec(wire: nil) == .jpeg)
        #expect(DesktopCodec(wire: "jpeg") == .jpeg)
        #expect(DesktopCodec(wire: "av1") == .jpeg)
        #expect(DesktopCodec(wire: "h264") == .h264)

        let missing = try JSONDecoder().decode(DesktopStreamMeta.self, from: Data("{}".utf8))
        #expect(missing.codec == .jpeg)
        let h264 = try JSONDecoder().decode(DesktopStreamMeta.self, from: Data(#"{"codec":"h264"}"#.utf8))
        #expect(h264.codec == .h264)
    }
}
