//! Pure framing for remote-desktop JPEG packets.
//!
//! Wire format: `[u32 big-endian JSON length][JSON header][concatenated JPEGs]`.
//! Keyframes contain exactly one full-canvas rectangle. Delta rectangles are
//! canonical 128px tiles, including smaller tiles at the right/bottom edges.

use image::ImageDecoder;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt, io::Cursor};

pub const TILE_SIZE: u32 = 128;
pub const MAX_HEADER_BYTES: usize = 256 * 1024;
pub const MAX_PACKET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TILES: usize = 4096;
pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_PIXELS: u64 = 16_777_216;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameType {
    Key,
    Tiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameHeader {
    pub r#type: FrameType,
    pub display: String,
    pub seq: u64,
    pub w: u32,
    pub h: u32,
    pub tiles: Vec<Tile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: FrameHeader,
    /// JPEGs in the same order as `header.tiles`, with lengths from each tile.
    pub jpeg: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn invalid(message: &str) -> CodecError {
    CodecError(message.to_owned())
}

/// Validates frame geometry, resource bounds, exact payload lengths and JPEG
/// dimensions. It does not decompress JPEG pixel data or mutate receiver state.
fn validate(frame: &Frame) -> Result<(), CodecError> {
    let h = &frame.header;
    if h.display.trim().is_empty() || h.display.len() > 256 {
        return Err(invalid("invalid display id"));
    }
    if h.w == 0
        || h.h == 0
        || h.w > MAX_DIMENSION
        || h.h > MAX_DIMENSION
        || u64::from(h.w) * u64::from(h.h) > MAX_PIXELS
    {
        return Err(invalid("invalid canvas dimensions"));
    }
    if h.tiles.is_empty() || h.tiles.len() > MAX_TILES || frame.jpeg.len() > MAX_PACKET_BYTES {
        return Err(invalid("invalid tile count or payload size"));
    }
    if h.r#type == FrameType::Key && h.tiles.len() != 1 {
        return Err(invalid("keyframe must contain one JPEG"));
    }
    let mut seen = BTreeSet::new();
    let mut offset = 0usize;
    for tile in &h.tiles {
        if tile.w == 0
            || tile.h == 0
            || tile.x.checked_add(tile.w).is_none_or(|x| x > h.w)
            || tile.y.checked_add(tile.h).is_none_or(|y| y > h.h)
        {
            return Err(invalid("rectangle outside canvas"));
        }
        match h.r#type {
            FrameType::Key => {
                if (tile.x, tile.y, tile.w, tile.h) != (0, 0, h.w, h.h) {
                    return Err(invalid("keyframe must cover canvas"));
                }
            }
            FrameType::Tiles => {
                if tile.x % TILE_SIZE != 0
                    || tile.y % TILE_SIZE != 0
                    || tile.w != TILE_SIZE.min(h.w - tile.x)
                    || tile.h != TILE_SIZE.min(h.h - tile.y)
                    || !seen.insert((tile.x, tile.y))
                {
                    return Err(invalid("invalid or duplicate tile geometry"));
                }
            }
        }
        let end = offset
            .checked_add(tile.len as usize)
            .filter(|end| *end <= frame.jpeg.len())
            .ok_or_else(|| invalid("JPEG length exceeds payload"))?;
        let jpeg = &frame.jpeg[offset..end];
        if jpeg.len() < 4 || !jpeg.starts_with(&[0xff, 0xd8]) || !jpeg.ends_with(&[0xff, 0xd9]) {
            return Err(invalid("invalid JPEG markers"));
        }
        let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(jpeg))
            .map_err(|_| invalid("invalid JPEG header"))?;
        if decoder.dimensions() != (tile.w, tile.h) {
            return Err(invalid("JPEG dimensions do not match rectangle"));
        }
        offset = end;
    }
    if offset != frame.jpeg.len() {
        return Err(invalid("trailing payload bytes"));
    }
    Ok(())
}

/// Serializes a complete validated frame; no platform capture APIs are used.
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, CodecError> {
    validate(frame)?;
    let json = serde_json::to_vec(&frame.header).map_err(|e| CodecError(e.to_string()))?;
    if json.len() > MAX_HEADER_BYTES {
        return Err(invalid("header too large"));
    }
    let total = 4usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(frame.jpeg.len()))
        .filter(|n| *n <= MAX_PACKET_BYTES)
        .ok_or_else(|| invalid("packet too large"))?;
    let mut packet = Vec::with_capacity(total);
    packet.extend_from_slice(&(json.len() as u32).to_be_bytes());
    packet.extend_from_slice(&json);
    packet.extend_from_slice(&frame.jpeg);
    Ok(packet)
}

/// Rejects oversized/truncated headers, invalid JSON, out-of-bounds rectangles,
/// duplicate tiles, length mismatches and JPEG/rectangle dimension mismatches.
pub fn decode_frame(packet: &[u8]) -> Result<Frame, CodecError> {
    if packet.len() < 4 || packet.len() > MAX_PACKET_BYTES {
        return Err(invalid("invalid packet length"));
    }
    let header_len = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(invalid("invalid header length"));
    }
    let end = 4usize
        .checked_add(header_len)
        .filter(|end| *end <= packet.len())
        .ok_or_else(|| invalid("truncated header"))?;
    let header = serde_json::from_slice(&packet[4..end]).map_err(|e| CodecError(e.to_string()))?;
    let frame = Frame {
        header,
        jpeg: packet[end..].to_vec(),
    };
    validate(&frame)?;
    Ok(frame)
}

// ---------------------------------------------------------------------------
// H.264 transport (frozen P1 protocol: plan `seremote-desktop-transport` §P1.4)
// ---------------------------------------------------------------------------

pub const DEFAULT_FPS_JPEG: u32 = 3;
pub const MAX_FPS_JPEG: u32 = 5;
pub const DEFAULT_FPS_H264: u32 = 30;
pub const MAX_FPS_H264: u32 = 30;
pub const DEFAULT_BITRATE_KBPS: u32 = 4000;
pub const MIN_BITRATE_KBPS: u32 = 500;
pub const MAX_BITRATE_KBPS: u32 = 20_000;
/// Periodic IDR bound required by the frozen protocol (seconds).
pub const H264_KEYFRAME_INTERVAL_SECONDS: u32 = 10;
/// AVCC NAL length prefix width; VideoToolbox always emits 4-byte lengths.
pub const AVCC_NAL_LENGTH_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamCodec {
    H264,
    #[default]
    Jpeg,
}

impl StreamCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Jpeg => "jpeg",
        }
    }
}

/// Absent/zero fps selects the codec default; every value is clamped per codec.
pub fn clamp_fps(codec: StreamCodec, fps: Option<u32>) -> u32 {
    let (default, max) = match codec {
        StreamCodec::H264 => (DEFAULT_FPS_H264, MAX_FPS_H264),
        StreamCodec::Jpeg => (DEFAULT_FPS_JPEG, MAX_FPS_JPEG),
    };
    fps.filter(|value| *value > 0)
        .unwrap_or(default)
        .clamp(1, max)
}

/// Absent/zero bitrate selects the default; clamped to the protocol range.
pub fn clamp_bitrate_kbps(bitrate: Option<u32>) -> u32 {
    bitrate
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_BITRATE_KBPS)
        .clamp(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum H264FrameType {
    H264,
}

/// Wire header of an H.264 packet. `ps` is `[spsLen, ppsLen]` on keyframes and
/// empty on delta frames; `len` is the AVCC sample length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct H264Header {
    pub r#type: H264FrameType,
    pub display: String,
    pub seq: u64,
    pub w: u32,
    pub h: u32,
    pub key: bool,
    pub ps: Vec<u32>,
    pub len: u32,
}

/// One H.264 packet: raw SPS (keyframes only) + raw PPS (keyframes only) +
/// AVCC sample. Parameter sets never carry start codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264Frame {
    pub header: H264Header,
    pub payload: Vec<u8>,
}

impl H264Frame {
    pub fn sps(&self) -> &[u8] {
        match self.header.ps.as_slice() {
            [sps, _] => &self.payload[..*sps as usize],
            _ => &[],
        }
    }

    pub fn pps(&self) -> &[u8] {
        match self.header.ps.as_slice() {
            [sps, pps] => {
                let start = *sps as usize;
                &self.payload[start..start + *pps as usize]
            }
            _ => &[],
        }
    }

    pub fn sample(&self) -> &[u8] {
        let offset: usize = self.header.ps.iter().map(|len| *len as usize).sum();
        &self.payload[offset.min(self.payload.len())..]
    }
}

fn validate_avcc(sample: &[u8]) -> Result<(), CodecError> {
    if sample.len() < AVCC_NAL_LENGTH_BYTES {
        return Err(invalid("h264 sample too short"));
    }
    let mut offset = 0usize;
    let mut nals = 0usize;
    while offset < sample.len() {
        if sample.len() - offset < AVCC_NAL_LENGTH_BYTES {
            return Err(invalid("truncated h264 NAL length"));
        }
        let len = u32::from_be_bytes(sample[offset..offset + 4].try_into().unwrap()) as usize;
        if len == 0 {
            return Err(invalid("empty h264 NAL unit"));
        }
        offset += AVCC_NAL_LENGTH_BYTES;
        if len > sample.len() - offset {
            return Err(invalid("h264 NAL length exceeds sample"));
        }
        offset += len;
        nals += 1;
    }
    if nals == 0 {
        return Err(invalid("h264 sample has no NAL units"));
    }
    Ok(())
}

/// Geometry, parameter-set bookkeeping, exact payload length and AVCC shape.
fn validate_h264(frame: &H264Frame) -> Result<(), CodecError> {
    let h = &frame.header;
    if h.display.trim().is_empty() || h.display.len() > 256 {
        return Err(invalid("invalid display id"));
    }
    if h.w == 0
        || h.h == 0
        || h.w % 2 != 0
        || h.h % 2 != 0
        || h.w > MAX_DIMENSION
        || h.h > MAX_DIMENSION
        || u64::from(h.w) * u64::from(h.h) > MAX_PIXELS
    {
        return Err(invalid("h264 dimensions must be even and bounded"));
    }
    if frame.payload.len() > MAX_PACKET_BYTES {
        return Err(invalid("packet too large"));
    }
    let (sps_len, pps_len) = match h.ps.as_slice() {
        [] if !h.key => (0usize, 0usize),
        [sps, pps] if h.key => {
            if *sps == 0 || *pps == 0 {
                return Err(invalid("keyframe parameter sets must be non-empty"));
            }
            (*sps as usize, *pps as usize)
        }
        [] => return Err(invalid("keyframe without parameter sets")),
        _ if !h.key => return Err(invalid("delta frame must not carry parameter sets")),
        _ => return Err(invalid("keyframe must carry exactly SPS and PPS")),
    };
    let expected = sps_len
        .checked_add(pps_len)
        .and_then(|n| n.checked_add(h.len as usize))
        .ok_or_else(|| invalid("packet length overflow"))?;
    if expected != frame.payload.len() {
        return Err(invalid("payload length does not match header"));
    }
    if h.len == 0 {
        return Err(invalid("empty h264 sample"));
    }
    if h.key {
        if frame.payload[0] & 0x1f != 7 {
            return Err(invalid("SPS NAL type mismatch"));
        }
        if frame.payload[sps_len] & 0x1f != 8 {
            return Err(invalid("PPS NAL type mismatch"));
        }
    }
    validate_avcc(&frame.payload[sps_len + pps_len..])
}

/// Serializes one validated H.264 packet: `[u32 BE header][JSON][payload]`.
pub fn encode_h264_frame(frame: &H264Frame) -> Result<Vec<u8>, CodecError> {
    validate_h264(frame)?;
    let json = serde_json::to_vec(&frame.header).map_err(|e| CodecError(e.to_string()))?;
    if json.len() > MAX_HEADER_BYTES {
        return Err(invalid("header too large"));
    }
    let total = 4usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(frame.payload.len()))
        .filter(|n| *n <= MAX_PACKET_BYTES)
        .ok_or_else(|| invalid("packet too large"))?;
    let mut packet = Vec::with_capacity(total);
    packet.extend_from_slice(&(json.len() as u32).to_be_bytes());
    packet.extend_from_slice(&json);
    packet.extend_from_slice(&frame.payload);
    Ok(packet)
}

/// Rejects truncated headers, unknown fields, odd dimensions, missing or stray
/// parameter sets, length mismatches and malformed AVCC samples.
pub fn decode_h264_frame(packet: &[u8]) -> Result<H264Frame, CodecError> {
    if packet.len() < 4 || packet.len() > MAX_PACKET_BYTES {
        return Err(invalid("invalid packet length"));
    }
    let header_len = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(invalid("invalid header length"));
    }
    let end = 4usize
        .checked_add(header_len)
        .filter(|end| *end <= packet.len())
        .ok_or_else(|| invalid("truncated header"))?;
    let header = serde_json::from_slice(&packet[4..end]).map_err(|e| CodecError(e.to_string()))?;
    let frame = H264Frame {
        header,
        payload: packet[end..].to_vec(),
    };
    validate_h264(&frame)?;
    Ok(frame)
}

/// Reports the packet kind without decoding the payload. JPEG frames return
/// `None`; unknown types are an error so a mismatched peer fails loudly.
pub fn packet_codec(packet: &[u8]) -> Result<Option<StreamCodec>, CodecError> {
    if packet.len() < 4 {
        return Err(invalid("invalid packet length"));
    }
    let header_len = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
    let end = 4usize
        .checked_add(header_len)
        .filter(|end| *end <= packet.len() && header_len <= MAX_HEADER_BYTES)
        .ok_or_else(|| invalid("truncated header"))?;
    // Only the discriminating field is read here; the full header is decoded
    // by the codec-specific parser afterwards.
    #[derive(Deserialize)]
    struct TypePeek {
        r#type: String,
    }
    let peek: TypePeek =
        serde_json::from_slice(&packet[4..end]).map_err(|e| CodecError(e.to_string()))?;
    match peek.r#type.as_str() {
        "h264" => Ok(Some(StreamCodec::H264)),
        "key" | "tiles" => Ok(Some(StreamCodec::Jpeg)),
        _ => Err(invalid("unknown frame type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::jpeg::JpegEncoder, Rgb, RgbImage};

    fn fixture(kind: FrameType) -> Frame {
        let image = RgbImage::from_pixel(64, 32, Rgb([30, 60, 90]));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 75)
            .encode_image(&image)
            .unwrap();
        Frame {
            header: FrameHeader {
                r#type: kind,
                display: "display-1".into(),
                seq: 42,
                w: 64,
                h: 32,
                tiles: vec![Tile {
                    x: 0,
                    y: 0,
                    w: 64,
                    h: 32,
                    len: jpeg.len() as u32,
                }],
            },
            jpeg,
        }
    }

    fn unchecked_packet(frame: &Frame) -> Vec<u8> {
        let header = serde_json::to_vec(&frame.header).unwrap();
        let mut packet = (header.len() as u32).to_be_bytes().to_vec();
        packet.extend(header);
        packet.extend_from_slice(&frame.jpeg);
        packet
    }

    #[test]
    fn roundtrip_key_and_tiles() {
        for kind in [FrameType::Key, FrameType::Tiles] {
            let frame = fixture(kind);
            let bytes = encode_frame(&frame).unwrap();
            assert_eq!(decode_frame(&bytes).unwrap(), frame);
            assert_eq!(bytes, unchecked_packet(&frame));
        }
    }

    #[test]
    fn multiple_jpegs_roundtrip_with_exact_boundaries() {
        let mut jpeg = Vec::new();
        let mut tiles = Vec::new();
        for (x, color) in [(0, [220, 0, 0]), (128, [0, 0, 220])] {
            let image = RgbImage::from_pixel(128, 128, Rgb(color));
            let start = jpeg.len();
            JpegEncoder::new_with_quality(&mut jpeg, 75)
                .encode_image(&image)
                .unwrap();
            tiles.push(Tile {
                x,
                y: 0,
                w: 128,
                h: 128,
                len: (jpeg.len() - start) as u32,
            });
        }
        let mut frame = Frame {
            header: FrameHeader {
                r#type: FrameType::Tiles,
                display: "1".into(),
                seq: 2,
                w: 256,
                h: 128,
                tiles,
            },
            jpeg,
        };
        assert_eq!(decode_frame(&encode_frame(&frame).unwrap()).unwrap(), frame);
        frame.header.tiles[0].len += 1;
        frame.header.tiles[1].len -= 1;
        assert!(decode_frame(&unchecked_packet(&frame)).is_err());
    }

    #[test]
    fn malformed_lengths_and_json_are_rejected() {
        for packet in [
            vec![],
            vec![0; 3],
            vec![0; 4],
            u32::MAX.to_be_bytes().to_vec(),
            vec![0, 0, 0, 2, b'{'],
        ] {
            assert!(decode_frame(&packet).is_err());
        }
        let good = encode_frame(&fixture(FrameType::Key)).unwrap();
        for length in [4, 20, good.len() - 1] {
            assert!(decode_frame(&good[..length]).is_err());
        }
        let mut trailing = good;
        trailing.push(0);
        assert!(decode_frame(&trailing).is_err());
    }

    #[test]
    fn malformed_rectangles_payloads_and_jpegs_are_rejected() {
        let baseline = fixture(FrameType::Tiles);
        let mutations: [fn(&mut Frame); 11] = [
            |f| f.header.w = 0,
            |f| f.header.w = MAX_DIMENSION + 1,
            |f| f.header.tiles[0].x = u32::MAX,
            |f| f.header.tiles[0].w = 65,
            |f| f.header.tiles[0].len = u32::MAX,
            |f| f.header.tiles[0].len = 0,
            |f| f.header.display.clear(),
            |f| f.header.tiles.clear(),
            |f| {
                f.header.tiles.push(f.header.tiles[0].clone());
                f.jpeg.extend(f.jpeg.clone());
            },
            |f| f.jpeg[0] = 0,
            |f| f.header.h += 1,
        ];
        for mutate in mutations {
            let mut frame = baseline.clone();
            mutate(&mut frame);
            assert!(encode_frame(&frame).is_err());
            assert!(decode_frame(&unchecked_packet(&frame)).is_err());
        }
        let mut key = fixture(FrameType::Key);
        key.header.tiles[0].x = 1;
        assert!(encode_frame(&key).is_err());
    }

    #[test]
    fn unknown_type_and_fields_are_rejected() {
        for json in [
            br#"{"type":"video","display":"1","seq":1,"w":1,"h":1,"tiles":[]}"#.as_slice(),
            br#"{"type":"key","display":"1","seq":1,"w":1,"h":1,"tiles":[],"extra":true}"#
                .as_slice(),
        ] {
            let mut packet = (json.len() as u32).to_be_bytes().to_vec();
            packet.extend(json);
            assert!(decode_frame(&packet).is_err());
        }
    }

    // -- H.264 framing ------------------------------------------------------

    fn avcc(nals: &[&[u8]]) -> Vec<u8> {
        let mut sample = Vec::new();
        for nal in nals {
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        sample
    }

    fn h264_frame(key: bool) -> H264Frame {
        let sps = [0x67u8, 0x64, 0x00, 0x1f];
        let pps = [0x68u8, 0xee, 0x3c, 0x80];
        let idr = [0x65u8, 0x88, 0x84, 0x00, 0x21];
        let slice = [0x41u8, 0x9a, 0x02, 0x11];
        let sample = if key {
            avcc(&[&idr])
        } else {
            avcc(&[&slice])
        };
        let mut payload = Vec::new();
        let mut ps = Vec::new();
        if key {
            payload.extend_from_slice(&sps);
            payload.extend_from_slice(&pps);
            ps = vec![sps.len() as u32, pps.len() as u32];
        }
        payload.extend_from_slice(&sample);
        H264Frame {
            header: H264Header {
                r#type: H264FrameType::H264,
                display: "display-1".into(),
                seq: 7,
                w: 1280,
                h: 720,
                key,
                ps,
                len: sample.len() as u32,
            },
            payload,
        }
    }

    fn h264_packet(frame: &H264Frame) -> Vec<u8> {
        let header = serde_json::to_vec(&frame.header).unwrap();
        let mut packet = (header.len() as u32).to_be_bytes().to_vec();
        packet.extend(header);
        packet.extend_from_slice(&frame.payload);
        packet
    }

    #[test]
    fn h264_key_and_delta_roundtrip() {
        for key in [true, false] {
            let frame = h264_frame(key);
            let packet = encode_h264_frame(&frame).unwrap();
            assert_eq!(packet, h264_packet(&frame));
            let decoded = decode_h264_frame(&packet).unwrap();
            assert_eq!(decoded, frame);
            assert_eq!(decoded.sample(), frame.sample());
            if key {
                assert_eq!(decoded.sps(), [0x67, 0x64, 0x00, 0x1f]);
                assert_eq!(decoded.pps(), [0x68, 0xee, 0x3c, 0x80]);
                assert_eq!(decoded.header.ps, vec![4, 4]);
            } else {
                assert!(decoded.header.ps.is_empty());
                assert_eq!(decoded.header.ps.len(), 0);
            }
            assert_eq!(packet_codec(&packet).unwrap(), Some(StreamCodec::H264));
        }
    }

    #[test]
    fn h264_wire_header_matches_frozen_protocol() {
        let packet = encode_h264_frame(&h264_frame(true)).unwrap();
        let header_len = u32::from_be_bytes(packet[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value =
            serde_json::from_slice(&packet[4..4 + header_len]).unwrap();
        assert_eq!(header["type"], "h264");
        assert_eq!(header["display"], "display-1");
        assert_eq!(header["seq"], 7);
        assert_eq!(header["w"], 1280);
        assert_eq!(header["h"], 720);
        assert_eq!(header["key"], true);
        assert_eq!(header["ps"], serde_json::json!([4, 4]));
        // AVCC sample = 4-byte NAL length + 5-byte IDR NAL.
        assert_eq!(header["len"], 9);
    }

    #[test]
    fn h264_length_mismatch_and_bad_ps_are_rejected() {
        let mutations: [fn(&mut H264Frame); 8] = [
            |f| f.header.len += 1,
            |f| f.header.len -= 1,
            |f| f.payload.push(0),
            |f| {
                f.header.ps.clear();
            },
            |f| f.header.ps = vec![4],
            |f| f.header.ps = vec![4, 4, 0],
            |f| f.header.ps[0] = 0,
            |f| f.header.ps[1] = 99,
        ];
        for mutate in mutations {
            let mut frame = h264_frame(true);
            mutate(&mut frame);
            assert!(encode_h264_frame(&frame).is_err());
            assert!(decode_h264_frame(&h264_packet(&frame)).is_err());
        }
        // A keyframe claiming to be a delta frame loses its parameter sets.
        let mut key = h264_frame(true);
        key.header.key = false;
        assert!(encode_h264_frame(&key).is_err());
        // A delta frame that carries parameter sets is rejected too.
        let mut delta = h264_frame(false);
        delta.header.ps = vec![4, 4];
        assert!(encode_h264_frame(&delta).is_err());
    }

    #[test]
    fn h264_odd_dimensions_and_unknown_fields_are_rejected() {
        for (w, h) in [(1279, 720), (1280, 719), (0, 720), (1280, 0)] {
            let mut frame = h264_frame(true);
            frame.header.w = w;
            frame.header.h = h;
            assert!(encode_h264_frame(&frame).is_err(), "{w}x{h} must be rejected");
        }
        let json = br#"{"type":"h264","display":"1","seq":1,"w":2,"h":2,"key":false,"ps":[],"len":5,"extra":1}"#;
        let mut packet = (json.len() as u32).to_be_bytes().to_vec();
        packet.extend_from_slice(json);
        packet.extend_from_slice(&avcc(&[&[0x41, 0x01, 0x02, 0x03, 0x04]]));
        assert!(decode_h264_frame(&packet).is_err());
        // JPEG packets are classified separately and never parse as H.264.
        let jpeg = encode_frame(&fixture(FrameType::Key)).unwrap();
        assert_eq!(packet_codec(&jpeg).unwrap(), Some(StreamCodec::Jpeg));
        assert!(decode_h264_frame(&jpeg).is_err());
    }

    #[test]
    fn h264_avcc_structure_is_enforced() {
        let mut frame = h264_frame(true);
        // Trim one byte off the IDR payload without touching the header.
        frame.payload.pop();
        frame.header.len -= 1;
        assert!(encode_h264_frame(&frame).is_err());
        // Trailing partial length prefix.
        let mut frame = h264_frame(false);
        frame.payload.extend_from_slice(&[0, 0]);
        frame.header.len += 2;
        assert!(encode_h264_frame(&frame).is_err());
    }

    #[test]
    fn codec_negotiation_clamps_match_the_plan() {
        assert_eq!(clamp_fps(StreamCodec::Jpeg, None), 3);
        assert_eq!(clamp_fps(StreamCodec::Jpeg, Some(30)), 5);
        assert_eq!(clamp_fps(StreamCodec::Jpeg, Some(0)), 3);
        assert_eq!(clamp_fps(StreamCodec::H264, None), 30);
        assert_eq!(clamp_fps(StreamCodec::H264, Some(0)), 30);
        assert_eq!(clamp_fps(StreamCodec::H264, Some(15)), 15);
        assert_eq!(clamp_fps(StreamCodec::H264, Some(120)), 30);
        assert_eq!(clamp_bitrate_kbps(None), 4000);
        assert_eq!(clamp_bitrate_kbps(Some(0)), 4000);
        assert_eq!(clamp_bitrate_kbps(Some(100)), 500);
        assert_eq!(clamp_bitrate_kbps(Some(99_000)), 20_000);
        assert_eq!(clamp_bitrate_kbps(Some(6000)), 6000);
        assert_eq!(StreamCodec::default(), StreamCodec::Jpeg);
        assert_eq!(
            serde_json::to_string(&StreamCodec::H264).unwrap(),
            "\"h264\""
        );
    }
}
