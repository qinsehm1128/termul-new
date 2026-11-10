import Foundation

/// Frozen P1 H.264 packet: big-endian header length, JSON header, then SPS, PPS
/// (key frames only) and one AVCC sample. Every length is validated with
/// subtraction before any payload slice is allocated.
nonisolated struct DesktopH264Packet: Sendable {
    /// Matches the host cap (`h264.rs` rejects SPS/PPS above 1024 bytes), so a
    /// packet cannot make this client allocate 1 MiB per parameter set.
    static let maxParameterSetBytes = 1024

    let display: String
    let seq: UInt64
    let width: Int
    let height: Int
    let isKey: Bool
    /// SPS + PPS for key frames; empty for delta frames.
    let parameterSets: [Data]
    /// AVCC sample: repeated 4-byte big-endian NAL length followed by the NAL.
    let sample: Data

    private nonisolated struct Header: Decodable {
        let type: String
        let display: String
        let seq: UInt64
        let w: Int
        let h: Int
        let key: Bool
        let ps: [Int]
        let len: Int
    }

    static func decode(_ data: Data) throws -> DesktopH264Packet {
        guard data.count >= 4, data.count <= DesktopPacket.maxPacketBytes else {
            throw DesktopProtocolError.invalidPacket
        }
        let prefix = [UInt8](data.prefix(4))
        let headerLength = prefix.reduce(0) { ($0 << 8) | Int($1) }
        guard headerLength > 0, headerLength <= DesktopPacket.maxHeaderBytes,
              headerLength <= data.count - 4 else {
            throw DesktopProtocolError.invalidPacket
        }
        let headerData = Data(data.dropFirst(4).prefix(headerLength))
        let header: Header
        do { header = try JSONDecoder().decode(Header.self, from: headerData) }
        catch { throw DesktopProtocolError.invalidPacket }

        guard header.type == "h264",
              !header.display.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              header.display.utf8.count <= 256,
              header.w > 0, header.h > 0,
              header.w <= DesktopPacket.maxDimension, header.h <= DesktopPacket.maxDimension,
              header.w * header.h <= DesktopPacket.maxPixels,
              header.w % 2 == 0, header.h % 2 == 0 else {
            throw DesktopProtocolError.invalidPacket
        }
        if header.key {
            guard header.ps.count == 2 else { throw DesktopProtocolError.invalidPacket }
        } else {
            guard header.ps.isEmpty else { throw DesktopProtocolError.invalidPacket }
        }
        guard header.len > 0 else { throw DesktopProtocolError.invalidPacket }

        let payload = data[(data.startIndex + 4 + headerLength)...]
        let base = payload.startIndex
        var offset = 0
        for length in header.ps {
            guard length > 0, length <= maxParameterSetBytes,
                  length <= payload.count - offset else {
                throw DesktopProtocolError.invalidPacket
            }
            offset += length
        }
        guard header.len <= payload.count - offset,
              offset + header.len == payload.count else {
            throw DesktopProtocolError.invalidPacket
        }

        var parameterSets: [Data] = []
        parameterSets.reserveCapacity(header.ps.count)
        offset = 0
        for length in header.ps {
            parameterSets.append(payload.subdata(in: (base + offset)..<(base + offset + length)))
            offset += length
        }
        let sample = payload.subdata(in: (base + offset)..<(base + offset + header.len))
        try validateAVCC(sample)
        return DesktopH264Packet(display: header.display, seq: header.seq,
                                 width: header.w, height: header.h,
                                 isKey: header.key, parameterSets: parameterSets,
                                 sample: sample)
    }

    /// VideoToolbox consumes AVCC input: each NAL must be fully inside `len`.
    private static func validateAVCC(_ sample: Data) throws {
        let base = sample.startIndex
        var offset = 0
        while offset < sample.count {
            guard sample.count - offset >= 4 else { throw DesktopProtocolError.invalidPacket }
            let length = (Int(sample[base + offset]) << 24)
                | (Int(sample[base + offset + 1]) << 16)
                | (Int(sample[base + offset + 2]) << 8)
                | Int(sample[base + offset + 3])
            guard length > 0, length <= sample.count - offset - 4 else {
                throw DesktopProtocolError.invalidPacket
            }
            offset += 4 + length
        }
        guard offset == sample.count else { throw DesktopProtocolError.invalidPacket }
    }
}
