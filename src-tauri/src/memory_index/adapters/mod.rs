//! Vendor adapters: one transcript file in, one normalized session out.
//!
//! Each vendor gets its own entry point rather than a shared "read a
//! transcript" abstraction, because the one thing they must not share is how
//! lineage depth is decided. Claude records a boolean, Codex records a number,
//! pi records nothing and the depth is in the directory nesting. A common
//! parameter for it would invite exactly the mistake the design forbids —
//! inferring depth from path structure for all three, which mislabels the 122
//! Claude files under `subagents/workflows/wf_*` whose grouping directory is not
//! a hierarchy level.
//!
//! What they do share is here: a memory-bounded JSONL reader, timestamp
//! parsing, text-block flattening, and the redaction + truncation that every
//! adapter must apply before text becomes searchable.

pub mod claude;
pub mod codex;
pub mod pi;

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use super::paths::MemoryVendor;
use super::redact;
use super::types::{
    CompactionRecord, IndexedSession, NormalizedMessage, MAX_RECORD_BYTES,
};

/// Longest text stored in the full-text index for one record.
///
/// Tool results reach megabytes. The index keeps searchable text plus a
/// pointer, not a copy of the corpus — so a record longer than this is
/// truncated for search while its [`super::types::SourcePointer`] still locates
/// the complete original on disk.
pub const MAX_INDEXED_TEXT_BYTES: usize = 32 * 1024;

/// Longest title derived from a first message.
const MAX_TITLE_CHARS: usize = 80;

/// What an adapter produced for one file.
#[derive(Debug, Clone, Default)]
pub struct AdaptedTranscript {
    pub session: Option<IndexedSession>,
    pub messages: Vec<NormalizedMessage>,
    pub compactions: Vec<CompactionRecord>,
    pub issues: Vec<AdapterIssue>,
}

/// Something worth reporting that did not stop the adapter.
///
/// Issues are surfaced in the build report rather than swallowed: a silently
/// dropped branch or an unparseable record is the kind of thing that makes an
/// index quietly incomplete.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterIssue {
    pub code: &'static str,
    pub path: String,
    pub detail: String,
}

impl AdapterIssue {
    pub fn new(code: &'static str, path: &Path, detail: impl Into<String>) -> Self {
        Self {
            code,
            path: path.to_string_lossy().into_owned(),
            detail: detail.into(),
        }
    }
}

pub const ISSUE_UNREADABLE: &str = "MEMORY_INDEX_TRANSCRIPT_UNREADABLE";
pub const ISSUE_OVERSIZED_RECORD: &str = "MEMORY_INDEX_RECORD_OVERSIZED";
pub const ISSUE_NO_SESSION_HEADER: &str = "MEMORY_INDEX_NO_SESSION_HEADER";
pub const ISSUE_LINEAGE_FORK: &str = "MEMORY_INDEX_LINEAGE_FORK";

/// One raw JSONL record with the byte range it occupies.
#[derive(Debug, Clone)]
pub struct RawRecord {
    pub byte_offset: u64,
    pub bytes: Vec<u8>,
    pub value: Value,
}

/// Stream a JSONL file with a hard per-record memory bound.
///
/// `BufRead::read_until` grows its buffer without limit, so one pathological
/// line in a 2.6 GB transcript could allocate gigabytes. This reader
/// accumulates up to [`MAX_RECORD_BYTES`] and then drains the rest of the line
/// without keeping it, reporting the record as oversized. Bounded memory is not
/// a nicety here: single transcripts on the measured machine reach 2.6 GB, and
/// several nested pi files reach 1.8 GB.
pub fn for_each_record<F>(path: &Path, mut visit: F) -> Vec<AdapterIssue>
where
    F: FnMut(RawRecord),
{
    let mut issues = Vec::new();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            issues.push(AdapterIssue::new(ISSUE_UNREADABLE, path, error.to_string()));
            return issues;
        }
    };
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut offset: u64 = 0;

    loop {
        let mut line = Vec::new();
        let mut consumed: u64 = 0;
        let mut overflowed = false;
        let mut hit_eof = false;

        loop {
            let available = match reader.fill_buf() {
                Ok([]) => {
                    hit_eof = true;
                    break;
                }
                Ok(buffer) => buffer,
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    issues.push(AdapterIssue::new(ISSUE_UNREADABLE, path, error.to_string()));
                    return issues;
                }
            };
            match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => {
                    let take = &available[..index];
                    if line.len() + take.len() <= MAX_RECORD_BYTES {
                        line.extend_from_slice(take);
                    } else {
                        overflowed = true;
                    }
                    let step = index + 1;
                    reader.consume(step);
                    consumed += step as u64;
                    break;
                }
                None => {
                    let step = available.len();
                    if line.len() + step <= MAX_RECORD_BYTES {
                        line.extend_from_slice(available);
                    } else {
                        overflowed = true;
                    }
                    reader.consume(step);
                    consumed += step as u64;
                }
            }
        }

        if overflowed {
            issues.push(AdapterIssue::new(
                ISSUE_OVERSIZED_RECORD,
                path,
                format!("record at byte {offset} exceeds {MAX_RECORD_BYTES} bytes"),
            ));
        } else if !line.is_empty() {
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                visit(RawRecord {
                    byte_offset: offset,
                    bytes: line,
                    value,
                });
            }
        }

        offset += consumed;
        if hit_eof {
            break;
        }
    }
    issues
}

/// `<vendor>:<absolute path>`.
///
/// The file is the session's identity: pi stores each subagent transcript as its
/// own file under one root, and two vendors can hand out the same session id.
#[must_use]
pub fn session_key(vendor: MemoryVendor, path: &Path) -> String {
    format!("{}:{}", vendor.as_str(), path.to_string_lossy())
}

/// Parse an RFC3339 timestamp into its canonical UTC rendering and epoch
/// milliseconds.
///
/// Both are kept: the string for display, the integer for ordering. Deriving
/// the order from the string would make it depend on two RFC3339 renderings
/// being lexicographically comparable, which they are not in general.
#[must_use]
pub fn parse_timestamp(raw: &str) -> Option<(String, i64)> {
    let parsed = chrono::DateTime::parse_from_rfc3339(raw.trim()).ok()?;
    let utc = parsed.with_timezone(&chrono::Utc);
    Some((
        utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        utc.timestamp_millis(),
    ))
}

/// Flatten a vendor content value into plain text.
///
/// Handles a bare string, a list of blocks, and the block spellings all three
/// vendors use (`text`, `input_text`, `output_text`). Reasoning content is
/// deliberately excluded by the callers: Claude ships every `thinking` block
/// with an empty body and Codex encrypts its `reasoning` payload, so there is
/// nothing to index either way.
#[must_use]
pub fn flatten_text(content: &Value) -> String {
    let mut out = String::new();
    append_text(content, &mut out);
    out.trim().to_string()
}

fn append_text(content: &Value, out: &mut String) {
    match content {
        Value::String(text) => push_segment(out, text),
        Value::Array(items) => {
            for item in items {
                append_text(item, out);
            }
        }
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str).unwrap_or("");
            match kind {
                "text" | "input_text" | "output_text" => {
                    if let Some(text) = map.get("text").and_then(Value::as_str) {
                        push_segment(out, text);
                    }
                }
                // A nested content list, e.g. a tool result whose body is itself
                // a block array.
                _ => {
                    if let Some(nested) = map.get("content") {
                        append_text(nested, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn push_segment(out: &mut String, segment: &str) {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(trimmed);
}

/// Redact, then bound the length of text destined for the full-text index.
#[must_use]
pub fn prepare_indexed_text(raw: &str) -> String {
    let redacted = redact::redact(raw).text;
    truncate_bytes(&redacted, MAX_INDEXED_TEXT_BYTES)
}

/// Truncate on a character boundary, marking that it happened.
fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated for search; full record on disk]", &text[..end])
}

/// A session title from the first user text.
#[must_use]
pub fn title_from_text(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut out = String::new();
    for (index, character) in line.chars().enumerate() {
        if index >= MAX_TITLE_CHARS {
            out.push('…');
            break;
        }
        out.push(character);
    }
    Some(out)
}

/// Field lookup across a few candidate names, returning the first non-blank.
#[must_use]
pub fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn records_carry_their_own_byte_offsets() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, r#"{{"n":1}}"#).unwrap();
        writeln!(file, r#"{{"n":2}}"#).unwrap();
        writeln!(file, r#"{{"n":3}}"#).unwrap();
        drop(file);

        let mut seen = Vec::new();
        let issues = for_each_record(&path, |record| {
            seen.push((record.byte_offset, record.value["n"].as_u64().unwrap()));
        });
        assert!(issues.is_empty());
        // `{"n":1}` is 7 bytes plus its newline.
        assert_eq!(seen, vec![(0, 1), (8, 2), (16, 3)]);
    }

    /// The offsets have to be usable, not merely present: a pointer built from
    /// one must read back the same record.
    #[test]
    fn reported_offsets_locate_the_record_on_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        std::fs::write(&path, "{\"n\":1}\n{\"n\":22}\n{\"n\":333}\n").unwrap();
        let identity = crate::memory_index::types::FileIdentity::read(&path).unwrap();
        let mut pointers = Vec::new();
        for_each_record(&path, |record| {
            pointers.push(crate::memory_index::types::SourcePointer::for_record(
                &identity,
                path.to_str().unwrap(),
                record.byte_offset,
                &record.bytes,
            ));
        });
        assert_eq!(pointers.len(), 3);
        for pointer in &pointers {
            assert_eq!(
                pointer.verify(),
                crate::memory_index::types::PointerFreshness::Fresh
            );
        }
        assert_eq!(pointers[2].read_bytes().as_deref(), Some(&b"{\"n\":333}"[..]));
    }

    #[test]
    fn blank_lines_and_unparseable_records_are_skipped_without_stopping() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        std::fs::write(&path, "{\"n\":1}\n\nnot json\n{\"n\":2}\n").unwrap();
        let mut count = 0;
        let issues = for_each_record(&path, |_| count += 1);
        assert_eq!(count, 2);
        assert!(issues.is_empty());
    }

    #[test]
    fn a_file_without_a_trailing_newline_still_yields_its_last_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        std::fs::write(&path, "{\"n\":1}\n{\"n\":2}").unwrap();
        let mut seen = Vec::new();
        for_each_record(&path, |record| seen.push(record.value["n"].as_u64().unwrap()));
        assert_eq!(seen, vec![1, 2]);
    }

    /// A single oversized line must be reported and skipped, and must not stop
    /// the records after it. This is the 2.6 GB transcript's failure mode in
    /// miniature.
    #[test]
    fn an_oversized_record_is_reported_and_does_not_block_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        let huge = format!("{{\"b\":\"{}\"}}", "x".repeat(MAX_RECORD_BYTES + 64));
        std::fs::write(&path, format!("{{\"n\":1}}\n{huge}\n{{\"n\":2}}\n")).unwrap();

        let mut seen = Vec::new();
        let issues = for_each_record(&path, |record| {
            seen.push(record.value["n"].as_u64().unwrap_or(0));
        });
        assert_eq!(seen, vec![1, 2], "records after the oversized one are lost");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, ISSUE_OVERSIZED_RECORD);
    }

    #[test]
    fn a_missing_file_is_one_issue_and_no_records() {
        let issues = for_each_record(Path::new("/nonexistent/x.jsonl"), |_| {
            panic!("must not visit any record")
        });
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, ISSUE_UNREADABLE);
    }

    #[test]
    fn text_flattening_handles_every_vendor_block_spelling() {
        assert_eq!(flatten_text(&Value::String("hi".into())), "hi");
        assert_eq!(
            flatten_text(&serde_json::json!([
                {"type": "thinking", "thinking": "ignored"},
                {"type": "text", "text": "first"},
                {"type": "input_text", "text": "second"},
                {"type": "output_text", "text": "third"},
            ])),
            "first\nsecond\nthird"
        );
        assert_eq!(
            flatten_text(&serde_json::json!({"content": [{"type": "text", "text": "nested"}]})),
            "nested"
        );
        assert_eq!(flatten_text(&serde_json::json!([])), "");
    }

    /// Reasoning blocks carry no body worth indexing in either vendor, and the
    /// flattener must not invent one from an adjacent field.
    #[test]
    fn reasoning_blocks_contribute_nothing() {
        assert_eq!(
            flatten_text(&serde_json::json!([
                {"type": "thinking", "thinking": "claude ships these empty"},
                {"type": "reasoning", "encrypted_content": "gAAAAA..."},
            ])),
            ""
        );
    }

    #[test]
    fn indexed_text_is_redacted_and_bounded() {
        let prepared = prepare_indexed_text("TOKEN=hunter2");
        assert!(!prepared.contains("hunter2"));

        // Prose, not one unbroken token run: an unbroken run is caught earlier
        // by the blob filter, which would make this assert nothing about
        // truncation.
        let long = "word ".repeat(MAX_INDEXED_TEXT_BYTES / 4);
        let bounded = prepare_indexed_text(&long);
        assert!(bounded.len() < long.len());
        assert!(bounded.contains("[truncated for search"), "{bounded:.120}");
    }

    /// The other order: an unbroken encoded run is omitted by redaction before
    /// truncation ever sees it, so the result is short rather than truncated.
    #[test]
    fn an_unbroken_encoded_run_is_omitted_not_truncated() {
        let blob = "a".repeat(MAX_INDEXED_TEXT_BYTES + 100);
        let prepared = prepare_indexed_text(&blob);
        assert!(prepared.contains("[omitted:blob:"), "{prepared:.120}");
        assert!(!prepared.contains("[truncated for search"));
    }

    #[test]
    fn truncation_lands_on_a_character_boundary() {
        // A 3-byte character straddling the cut point must not be split.
        let text = format!("{}好", "a".repeat(MAX_INDEXED_TEXT_BYTES - 1));
        let bounded = truncate_bytes(&text, MAX_INDEXED_TEXT_BYTES);
        assert!(bounded.starts_with(&"a".repeat(MAX_INDEXED_TEXT_BYTES - 1)));
        assert!(bounded.contains("[truncated for search"));
    }

    #[test]
    fn titles_take_the_first_non_blank_line_and_are_bounded() {
        assert_eq!(
            title_from_text("\n\n  fix the login bug\nmore"),
            Some("fix the login bug".to_string())
        );
        assert_eq!(title_from_text("   \n  "), None);
        let long = title_from_text(&"x".repeat(200)).unwrap();
        assert!(long.ends_with('…'));
        assert_eq!(long.chars().count(), MAX_TITLE_CHARS + 1);
    }

    #[test]
    fn timestamps_normalize_to_utc_with_a_matching_epoch() {
        let (utc, ms) = parse_timestamp("2026-09-01T08:00:00.000+08:00").unwrap();
        assert_eq!(utc, "2026-09-01T00:00:00.000Z");
        assert_eq!(ms, 1_788_220_800_000);
        assert!(parse_timestamp("not a time").is_none());
        assert!(parse_timestamp("").is_none());
    }

    #[test]
    fn session_keys_are_vendor_qualified() {
        assert_eq!(
            session_key(MemoryVendor::Pi, Path::new("/a/b.jsonl")),
            "pi:/a/b.jsonl"
        );
        assert_ne!(
            session_key(MemoryVendor::Pi, Path::new("/a/b.jsonl")),
            session_key(MemoryVendor::Codex, Path::new("/a/b.jsonl"))
        );
    }
}
