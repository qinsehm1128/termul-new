//! Bounded-work incremental JSONL scanning. Partial valid records are retried;
//! oversized lines are discarded resumably rather than rescanned or drained forever.
use crate::memory_index::{
    adapters::{AdapterIssue, RawRecord, RecordScan},
    types::MAX_RECORD_BYTES,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

pub const MAX_SCAN_RECORDS: usize = 128;
pub const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;
const ANCHOR_BYTES: u64 = 256;
#[derive(Default)]
pub struct ScanState {
    pub discarding: bool,
}

pub fn anchor_file(file: &mut File, boundary: u64) -> Option<Vec<u8>> {
    let start = boundary.saturating_sub(ANCHOR_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = vec![0; (boundary - start) as usize];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
pub fn scan(path: &Path, start: u64, visit: impl FnMut(RawRecord)) -> RecordScan {
    match File::open(path) {
        Ok(file) => scan_file(file, start, &mut ScanState::default(), visit),
        Err(_) => RecordScan {
            issues: vec![AdapterIssue::new(
                "CHAT_UNREADABLE",
                path,
                "chat transcript read degraded",
            )],
            resume_offset: start,
        },
    }
}
pub fn scan_file(
    mut file: File,
    start: u64,
    state: &mut ScanState,
    mut visit: impl FnMut(RawRecord),
) -> RecordScan {
    let mut result = RecordScan {
        issues: vec![],
        resume_offset: start,
    };
    let fail = |result: &mut RecordScan, code| {
        // At most one diagnostic per code per batch; never include transcript text.
        if !result.issues.iter().any(|issue| issue.code == code) {
            result.issues.push(AdapterIssue::new(
                code,
                Path::new("agent_chat"),
                "chat transcript read degraded",
            ));
        }
    };
    let length = file.metadata().map(|m| m.len()).unwrap_or(start);
    if file.seek(SeekFrom::Start(start)).is_err() {
        fail(&mut result, "CHAT_SEEK");
        return result;
    }
    let mut reader = BufReader::new(file.take(length.saturating_sub(start)));
    let mut position = start;
    let mut records = 0;
    loop {
        if records >= MAX_SCAN_RECORDS || position.saturating_sub(start) >= MAX_SCAN_BYTES {
            break;
        }
        let mut bytes = vec![];
        let mut consumed = 0u64;
        let mut complete = false;
        let mut oversized = state.discarding;
        loop {
            let available = match reader.fill_buf() {
                Ok(value) => value,
                Err(_) => {
                    fail(&mut result, "CHAT_READ");
                    return result;
                }
            };
            if available.is_empty() {
                break;
            }
            // Valid single records may exceed the batch target (up to 8 MiB).
            // Once proved oversized, bound all further draining to the batch budget.
            let allowance = if oversized {
                MAX_SCAN_BYTES
                    .saturating_sub(position.saturating_sub(start) + consumed)
                    .max(1) as usize
            } else {
                available.len()
            };
            let available = &available[..available.len().min(allowance)];
            let take = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|p| p + 1)
                .unwrap_or(available.len());
            complete = available[take - 1] == b'\n';
            consumed += take as u64;
            if !oversized && bytes.len() + take <= MAX_RECORD_BYTES {
                bytes.extend_from_slice(&available[..take]);
            } else {
                oversized = true;
                bytes.clear();
            }
            reader.consume(take);
            if complete {
                break;
            }
            if oversized && position.saturating_sub(start) + consumed >= MAX_SCAN_BYTES {
                break;
            }
        }
        if oversized {
            fail(&mut result, "CHAT_RECORD_OVERSIZED");
            state.discarding = !complete;
            position += consumed;
            result.resume_offset = position;
            if !complete {
                break;
            }
        } else if !complete {
            break; // retry this bounded partial record when more bytes arrive
        } else {
            let offset = position;
            position += consumed;
            result.resume_offset = position;
            if !bytes.iter().all(u8::is_ascii_whitespace) {
                match serde_json::from_slice(&bytes) {
                    Ok(value) => visit(RawRecord {
                        value,
                        bytes,
                        byte_offset: offset,
                    }),
                    Err(_) => fail(&mut result, "CHAT_JSON"),
                }
            }
        }
        records += 1;
    }
    result
}
