import Foundation

/// Wire codec negotiated in `start` and reported back in `meta`.
/// Unknown or absent values keep the legacy JPEG stream alive.
nonisolated enum DesktopCodec: String, Sendable, Equatable {
    case jpeg
    case h264

    init(wire: String?) {
        self = wire == DesktopCodec.h264.rawValue ? .h264 : .jpeg
    }
}

/// `meta.codec` is optional for old hosts; missing means JPEG.
nonisolated struct DesktopStreamMeta: Decodable, Sendable, Equatable {
    let codec: DesktopCodec

    private enum CodingKeys: String, CodingKey { case codec }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        codec = DesktopCodec(wire: try container.decodeIfPresent(String.self, forKey: .codec))
    }
}

/// Builds the frozen `start` request. H.264 fields only appear when the host
/// advertises it *and* this device can decode it; old hosts deny unknown keys.
nonisolated enum DesktopStartRequest {
    static let h264FPS = 30
    static let h264Bitrate = 4000
    static let legacyFPS = 3
    static let defaultMaxWidth = 1920
    static let defaultQuality = 80

    static func message(display: String, codecs: [String], h264Available: Bool,
                        maxWidth: Int = defaultMaxWidth, quality: Int = defaultQuality) -> [String: Any] {
        if codecs.contains(DesktopCodec.h264.rawValue), h264Available {
            return [
                "type": "start",
                "display": display,
                "fps": h264FPS,
                "maxWidth": maxWidth,
                "quality": quality,
                "codec": DesktopCodec.h264.rawValue,
                "bitrate": h264Bitrate,
            ]
        }
        return [
            "type": "start",
            "display": display,
            "fps": legacyFPS,
            "maxWidth": maxWidth,
            "quality": quality,
        ]
    }
}
