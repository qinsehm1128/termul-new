import Foundation
import ImageIO

nonisolated struct DesktopDisplay: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let pixelWidth: Int
    let pixelHeight: Int
    let scale: Double
    let originX: Double
    let originY: Double
    let isPrimary: Bool

    enum CodingKeys: String, CodingKey {
        case id, name, scale
        case pixelWidth = "pixel_width", pixelHeight = "pixel_height"
        case originX = "origin_x", originY = "origin_y", isPrimary = "is_primary"
    }
}

nonisolated struct DesktopInfo: Decodable, Equatable, Sendable {
    nonisolated struct Permissions: Decodable, Equatable, Sendable {
        let screenRecording: Bool
        let accessibility: Bool
    }
    let supported: Bool
    let viewEnabled: Bool
    let controlEnabled: Bool
    let permissions: Permissions
    let displays: [DesktopDisplay]
    /// Optional transport codecs advertised by the host. Missing means legacy JPEG.
    let codecs: [String]?

    var supportedCodecs: [String] { codecs ?? ["jpeg"] }
}

nonisolated enum DesktopProtocolError: Error, Equatable {
    case invalidPacket
    case invalidJPEG
    case missingKeyframe
    case allocationFailed
}

/// The engine packet is one big-endian length, one JSON header and exact JPEG slices.
/// Limits mirror src/screen/codec.rs; metadata is checked before allocating pixels.
nonisolated struct DesktopPacket: Sendable {
    nonisolated enum Kind: String, Decodable, Sendable { case key, tiles }
    nonisolated struct Tile: Sendable {
        let x: Int
        let y: Int
        let width: Int
        let height: Int
        let jpeg: Data
    }
    static let maxHeaderBytes = 256 * 1024
    static let maxPacketBytes = 32 * 1024 * 1024
    static let maxTiles = 4096
    static let maxDimension = 16_384
    static let maxPixels = 16_777_216
    static let tileSize = 128

    let type: Kind
    let display: String
    let seq: UInt64
    let width: Int
    let height: Int
    let tiles: [Tile]

    private nonisolated struct Header: Decodable {
        nonisolated struct Rectangle: Decodable {
            let x: Int
            let y: Int
            let w: Int
            let h: Int
            let len: Int
        }
        let type: Kind
        let display: String
        let seq: UInt64
        let w: Int
        let h: Int
        let tiles: [Rectangle]
    }

    static func decode(_ data: Data) throws -> DesktopPacket {
        guard data.count >= 4, data.count <= maxPacketBytes else {
            throw DesktopProtocolError.invalidPacket
        }
        // A Data slice need not be aligned or start at index zero.
        let bytes = [UInt8](data.prefix(4))
        let length = bytes.reduce(0) { ($0 << 8) | Int($1) }
        guard length > 0, length <= maxHeaderBytes, length <= data.count - 4 else {
            throw DesktopProtocolError.invalidPacket
        }
        let headerData = Data(data.dropFirst(4).prefix(length))
        let header: Header
        do { header = try JSONDecoder().decode(Header.self, from: headerData) }
        catch { throw DesktopProtocolError.invalidPacket }
        guard !header.display.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              header.display.utf8.count <= 256,
              header.w > 0, header.h > 0,
              header.w <= maxDimension, header.h <= maxDimension,
              header.w * header.h <= maxPixels,
              !header.tiles.isEmpty, header.tiles.count <= maxTiles,
              header.type != .key || header.tiles.count == 1 else {
            throw DesktopProtocolError.invalidPacket
        }
        let payload = Data(data.dropFirst(4 + length))
        var offset = 0
        var seen = Set<Int>()
        var tiles: [Tile] = []
        tiles.reserveCapacity(header.tiles.count)
        for rect in header.tiles {
            // Subtraction avoids arithmetic overflow even for hostile Int.max JSON values.
            guard rect.x >= 0, rect.y >= 0, rect.w > 0, rect.h > 0,
                  rect.x < header.w, rect.y < header.h,
                  rect.w <= header.w - rect.x, rect.h <= header.h - rect.y,
                  rect.len > 0, rect.len <= payload.count - offset else {
                throw DesktopProtocolError.invalidPacket
            }
            if header.type == .key {
                guard rect.x == 0, rect.y == 0, rect.w == header.w, rect.h == header.h else {
                    throw DesktopProtocolError.invalidPacket
                }
            } else {
                guard rect.x % tileSize == 0, rect.y % tileSize == 0,
                      rect.w == min(tileSize, header.w - rect.x),
                      rect.h == min(tileSize, header.h - rect.y),
                      seen.insert(rect.y * maxDimension + rect.x).inserted else {
                    throw DesktopProtocolError.invalidPacket
                }
            }
            let jpeg = payload.subdata(in: offset..<(offset + rect.len))
            try validateJPEG(jpeg, width: rect.w, height: rect.h)
            tiles.append(Tile(x: rect.x, y: rect.y, width: rect.w, height: rect.h, jpeg: jpeg))
            offset += rect.len
        }
        guard offset == payload.count else { throw DesktopProtocolError.invalidPacket }
        return DesktopPacket(type: header.type, display: header.display, seq: header.seq,
                             width: header.w, height: header.h, tiles: tiles)
    }

    private static func validateJPEG(_ data: Data, width: Int, height: Int) throws {
        guard data.count >= 4, data.prefix(2) == Data([0xff, 0xd8]),
              data.suffix(2) == Data([0xff, 0xd9]),
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(source) == 1,
              CGImageSourceGetType(source) as String? == "public.jpeg",
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue == width,
              (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue == height else {
            throw DesktopProtocolError.invalidJPEG
        }
    }
}

/// Stable host codes, never raw server errors or credentials, become localized UI copy.
nonisolated enum DesktopIssue: Equatable, Sendable {
    case disabled, controlDisabled, unsupported, screenPermission, inputPermission
    case displayNotFound, viewForbidden, controlForbidden, captureFailed, inputFailed
    case invalidRequest, disconnected, unknown

    init(code: String, capability: String? = nil) {
        self = switch code {
        case "DESKTOP_DISABLED": .disabled
        case "CONTROL_DISABLED": .controlDisabled
        case "SCREEN_UNSUPPORTED": .unsupported
        case "SCREEN_PERMISSION_REQUIRED": .screenPermission
        case "INPUT_PERMISSION_REQUIRED": .inputPermission
        case "DISPLAY_NOT_FOUND": .displayNotFound
        case "FORBIDDEN": capability == "screen.control" ? .controlForbidden : .viewForbidden
        case "CAPTURE_FAILED": .captureFailed
        case "INPUT_FAILED": .inputFailed
        case "INVALID_REQUEST": .invalidRequest
        case "DISCONNECTED", "DESKTOP_STOPPED", "UNAUTHORIZED", "DEVICE_REVOKED": .disconnected
        default: .unknown
        }
    }

    var message: String {
        switch self {
        case .disabled: String(localized: "Remote desktop viewing is disabled on the computer.")
        case .controlDisabled: String(localized: "Enable remote control on the computer to use the mouse and keyboard.")
        case .unsupported: String(localized: "This host does not support remote desktop.")
        case .screenPermission: String(localized: "Allow Screen Recording on the computer to view its desktop.")
        case .inputPermission: String(localized: "Allow Accessibility on the computer to control its mouse and keyboard.")
        case .displayNotFound: String(localized: "This display is no longer available. Select another display.")
        case .viewForbidden: String(localized: "This device does not have permission to view the desktop.")
        case .controlForbidden: String(localized: "This device does not have permission to control the desktop.")
        case .captureFailed: String(localized: "The computer could not capture its desktop. Try again.")
        case .inputFailed: String(localized: "The computer could not apply that input. Try again.")
        case .invalidRequest: String(localized: "The desktop request could not be accepted.")
        case .disconnected: String(localized: "The desktop connection ended. Reconnect to continue.")
        case .unknown: String(localized: "The desktop is unavailable. Try again.")
        }
    }
}
