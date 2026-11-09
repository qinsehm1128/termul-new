//! Strict live JSONL reader. Reuses the index cursor shape, never indexed text.
use crate::memory_index::{
    adapters::{AdapterIssue, RawRecord, RecordScan},
    types::MAX_RECORD_BYTES,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

pub fn scan(path: &Path, start: u64, mut visit: impl FnMut(RawRecord)) -> RecordScan {
    let mut result = RecordScan {
        issues: vec![],
        resume_offset: start,
    };
    let fail = |result: &mut RecordScan, code: &'static str| {
        result.issues.push(AdapterIssue::new(
            code,
            path,
            "chat transcript read degraded",
        ));
    };
    let Ok(mut file) = File::open(path) else {
        fail(&mut result, "CHAT_UNREADABLE");
        return result;
    };
    let Ok(length) = file.metadata().map(|m| m.len()) else {
        fail(&mut result, "CHAT_UNREADABLE");
        return result;
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        fail(&mut result, "CHAT_UNREADABLE");
        return result;
    }
    // A growing writer cannot keep this scan alive forever.
    let mut reader = BufReader::new(file.take(length.saturating_sub(start)));
    let mut position = start;
    loop {
        let mut bytes = vec![];
        let mut consumed = 0u64;
        let mut complete = false;
        let mut oversized = false;
        loop {
            let buffer = match reader.fill_buf() {
                Ok(buffer) => buffer,
                Err(_) => {
                    fail(&mut result, "CHAT_UNREADABLE");
                    return result;
                }
            };
            if buffer.is_empty() {
                break;
            }
            let newline = buffer.iter().position(|b| *b == b'\n');
            let take = newline.map_or(buffer.len(), |n| n + 1);
            if bytes.len().saturating_add(take) <= MAX_RECORD_BYTES {
                bytes.extend_from_slice(&buffer[..take]);
            } else {
                oversized = true;
            }
            reader.consume(take);
            consumed += take as u64;
            if newline.is_some() {
                complete = true;
                break;
            }
        }
        if !complete {
            break;
        } // retain partial line for the next append, even valid JSON
        let offset = position;
        position += consumed;
        result.resume_offset = position; // malformed complete lines never spin
        if oversized {
            fail(&mut result, "CHAT_RECORD_OVERSIZED");
            continue;
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(&bytes) {
            Ok(value) => visit(RawRecord {
                byte_offset: offset,
                bytes,
                value,
            }),
            Err(_) => fail(&mut result, "CHAT_INVALID_JSON"),
        }
    }
    result
}
