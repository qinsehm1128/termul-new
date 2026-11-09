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
}
