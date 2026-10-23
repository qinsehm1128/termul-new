//! Private stdio protocol. No terminal, filesystem, clipboard or input authority.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const GHOSTTY_REVISION: &str = "44f2a44";
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_LINES: usize = 200;
pub const MAX_CELLS: usize = 262_144;
pub const MAX_AXIS: u16 = 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    Init {
        cols: u16,
        rows: u16,
    },
    Output {
        event_seq: u64,
        data_base64: String,
    },
    Resize {
        event_seq: u64,
        cols: u16,
        rows: u16,
    },
    Read {
        request_id: u64,
        barrier_seq: u64,
        max_lines: usize,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Response {
    Hello {
        protocol_version: u32,
        observer_build_id: String,
        ghostty_revision: String,
    },
    Ack {
        applied_seq: u64,
    },
    Reply {
        request_id: u64,
        applied_seq: u64,
        text: String,
        truncated: bool,
    },
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "observer frame limit",
        ));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "observer frame schema"))
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, frame: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("observer frame limit"));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

pub fn valid_grid(cols: u16, rows: u16) -> bool {
    cols > 0
        && rows > 0
        && cols <= MAX_AXIS
        && rows <= MAX_AXIS
        && (cols as usize) * (rows as usize) <= MAX_CELLS
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_limits_match_native_wrapper_before_process_start() {
        assert!(valid_grid(1024, 256));
        assert!(!valid_grid(1025, 1));
        assert!(!valid_grid(1, 1025));
        assert!(!valid_grid(1024, 257));
        assert!(!valid_grid(0, 24));
    }
    #[test]
    fn oversized_frame_rejected_before_payload_allocation() {
        let bytes = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        assert!(read_frame::<Request>(&mut bytes.as_slice()).is_err());
    }
    #[test]
    fn protocol_roundtrip() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &Request::Resize {
                event_seq: 2,
                cols: 80,
                rows: 24,
            },
        )
        .unwrap();
        assert!(matches!(
            read_frame::<Request>(&mut bytes.as_slice()).unwrap(),
            Request::Resize { event_seq: 2, .. }
        ));
    }
}
