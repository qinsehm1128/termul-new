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
use super::types::{CompactionRecord, IndexedSession, NormalizedMessage, MAX_RECORD_BYTES};

/// Longest text stored in the full-text index for one record.
///
/// Tool results reach megabytes. The index keeps searchable text plus a
/// pointer, not a copy of the corpus — so a record longer than this is
/// truncated for search while its [`super::types::SourcePointer`] still locates
/// the complete original on disk.
pub const MAX_INDEXED_TEXT_BYTES: usize = 32 * 1024;

/// Longest title derived from a first message.
const MAX_TITLE_CHARS: usize = 80;

/// Where a resumed adapt run should pick up.
///
/// Present only when the previous pass over this exact file can be trusted:
/// the file grew, its device/inode are unchanged, and the last record it
/// indexed still hashes to what was stored. Ingest decides that; an adapter
/// just honours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeFrom {
    /// A boundary previously reported through [`RecordScan::resume_offset`].
    pub byte_offset: u64,
    /// The ordinal to give the first newly-read message.
    pub next_ordinal: u32,
}

/// What an adapter produced for one file.
///
/// On a resumed run this describes only the **newly read tail**: `session`
/// carries whatever the tail could determine and `None` for anything that lives
/// in the file's header, and `messages` starts at [`ResumeFrom::next_ordinal`].
/// Merging it onto the stored row is ingest's job — see `merge_resumed`.
#[derive(Debug, Clone, Default)]
pub struct AdaptedTranscript {
    pub session: Option<IndexedSession>,
    pub messages: Vec<NormalizedMessage>,
    pub compactions: Vec<CompactionRecord>,
    pub issues: Vec<AdapterIssue>,
    /// Where the next pass over this file may start.
    pub resume_offset: u64,
}

/// Something worth reporting that did not stop the adapter.
///
/// Issues are surfaced in the build report rather than swallowed: a silently
/// dropped branch or an unparseable record is the kind of thing that makes an
/// index quietly incomplete.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterIssue {
    /// One of the `ISSUE_*` constants below. Owned rather than `&'static str`
    /// so the report it rides in can be deserialized on the way back.
    pub code: String,
    pub path: String,
    pub detail: String,
}

impl AdapterIssue {
    pub fn new(code: &'static str, path: &Path, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            path: path.to_string_lossy().into_owned(),
            detail: detail.into(),
        }
    }
}

pub const ISSUE_UNREADABLE: &str = "MEMORY_INDEX_TRANSCRIPT_UNREADABLE";
pub const ISSUE_OVERSIZED_RECORD: &str = "MEMORY_INDEX_RECORD_OVERSIZED";
pub const ISSUE_NO_SESSION_HEADER: &str = "MEMORY_INDEX_NO_SESSION_HEADER";
pub const ISSUE_LINEAGE_FORK: &str = "MEMORY_INDEX_LINEAGE_FORK";
/// Post-build compaction failed. The index is correct, just larger than it
/// needs to be — which is why this is an issue on the report rather than an
/// error that discards a finished build.
pub const ISSUE_COMPACT_FAILED: &str = "MEMORY_INDEX_COMPACT_FAILED";

/// One raw JSONL record with the byte range it occupies.
#[derive(Debug, Clone)]
pub struct RawRecord {
    pub byte_offset: u64,
    pub bytes: Vec<u8>,
    pub value: Value,
}

/// What one pass over a transcript found, and where the next pass may start.
#[derive(Debug, Clone, Default)]
pub struct RecordScan {
    pub issues: Vec<AdapterIssue>,
    /// Byte offset just past the last **newline-terminated** line.
    ///
    /// Deliberately not "end of file". A transcript that is being written to
    /// right now can end in a half-written line, and a JSON parse of half a
    /// record either fails or — worse — succeeds on a prefix that happens to be
    /// valid. Resuming from the last complete line means the partial one is read
    /// again when the rest of it lands, which is the only way an append-resume
    /// can be correct on a live file.
    ///
    /// A complete line that failed to parse still advances this: it was whole,
    /// re-reading it would fail again, and refusing to move past it would stall
    /// the session forever.
    pub resume_offset: u64,
}

/// Stream a JSONL file from `start_offset` with a hard per-record memory bound.
///
/// `BufRead::read_until` grows its buffer without limit, so one pathological
/// line in a 2.6 GB transcript could allocate gigabytes. This reader
/// accumulates up to [`MAX_RECORD_BYTES`] and then drains the rest of the line
/// without keeping it, reporting the record as oversized. Bounded memory is not
/// a nicety here: single transcripts on the measured machine reach 2.6 GB, and
/// several nested pi files reach 1.8 GB.
///
/// `start_offset` must be a boundary a previous scan reported through
/// [`RecordScan::resume_offset`]; starting anywhere else lands mid-record.
pub fn for_each_record<F>(path: &Path, start_offset: u64, mut visit: F) -> RecordScan
where
    F: FnMut(RawRecord),
{
    let mut scan = RecordScan {
        issues: Vec::new(),
        resume_offset: start_offset,
    };
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            scan
                .issues
                .push(AdapterIssue::new(ISSUE_UNREADABLE, path, error.to_string()));
            return scan;
        }
    };
    if start_offset > 0 {
        use std::io::{Seek, SeekFrom};
        if let Err(error) = file.seek(SeekFrom::Start(start_offset)) {
            scan
                .issues
                .push(AdapterIssue::new(ISSUE_UNREADABLE, path, error.to_string()));
            return scan;
        }
    }
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut offset: u64 = start_offset;

    loop {
        let mut line = Vec::new();
        let mut consumed: u64 = 0;
        let mut overflowed = false;
        let mut hit_eof = false;
        let mut newline_terminated = false;

        loop {
            let available = match reader.fill_buf() {
                Ok([]) => {
                    hit_eof = true;
                    break;
                }
                Ok(buffer) => buffer,
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    scan
                        .issues
                        .push(AdapterIssue::new(ISSUE_UNREADABLE, path, error.to_string()));
                    return scan;
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
                    newline_terminated = true;
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
            scan.issues.push(AdapterIssue::new(
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
        if newline_terminated {
            scan.resume_offset = offset;
        }
        if hit_eof {
            break;
        }
    }
    scan
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
    format!(
        "{}\n[truncated for search; full record on disk]",
        &text[..end]
    )
}

/// Wrapper tags whose entire contents are harness bookkeeping.
///
/// Measured on this project's real corpus: the first user-role record of a
/// Claude session is almost never the user's first message. It is a
/// `<local-command-caveat>`, a `<command-name>/clear</command-name>` triple, or
/// a `<system-reminder>`. Taking it verbatim made most session titles read
/// `Caveat: The messages below were generated by the user...`.
///
/// `command-args` is deliberately absent: when a session starts with a slash
/// command, the user's actual request is the argument, and it is the best title
/// the transcript has.
const DROPPED_WRAPPER_TAGS: &[&str] = &[
    "local-command-caveat",
    "local-command-stdout",
    "command-name",
    "command-message",
    "command-stdout",
    "system-reminder",
    "teammate-message",
    "thinking",
];

/// Whole injected documents that arrive under a `user` role.
///
/// Each is a literal a harness emits verbatim, so matching it is recognition of
/// a known artifact rather than a guess about prose.
/// `<permissions instructions>` is in this list rather than
/// [`DROPPED_WRAPPER_TAGS`] because it is not a tag — the name contains a space
/// and nothing ever closes it. It is a literal marker, so it is matched as one.
const INJECTED_PREAMBLES: &[&str] = &[
    "# AGENTS.md instructions",
    "# CLAUDE.md instructions",
    "# Analysis Mode Protocol",
    "Caveat: The messages below were generated",
    "Base directory for this skill:",
    "<permissions instructions>",
];

/// Strip harness wrappers, keeping whatever a person actually wrote.
///
/// Line-based rather than a paired-tag regex: the `regex` crate has no
/// backreferences, so `<(\w+)>.*</\1>` is not expressible. Line scanning covers
/// both observed shapes — a tag pair closed on its own line, and a block opened
/// and closed on separate lines — and degrades to "keep the line" on anything
/// unrecognised, which is the safe direction.
#[must_use]
pub fn strip_scaffolding(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping: Option<&str> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(open) = skipping {
            if line.starts_with("</") && line[2..].starts_with(open) {
                skipping = None;
            }
            continue;
        }
        // A line that is only a closing tag carries nothing. Reaching here means
        // no block was open, so it is a stray closer.
        if line.starts_with("</") {
            continue;
        }
        let Some(tag) = opening_tag_name(line) else {
            if !line.is_empty() {
                kept.push(line);
            }
            continue;
        };
        let closing = format!("</{tag}>");
        match line.find(&closing) {
            // `<tag ...>inner</tag>` all on one line.
            Some(close_at) => {
                if DROPPED_WRAPPER_TAGS.contains(&tag) {
                    continue;
                }
                let open_end = line.find('>').map(|index| index + 1).unwrap_or(0);
                // Malformed input must not be able to invert the slice bounds.
                if open_end > close_at {
                    continue;
                }
                let inner = line[open_end..close_at].trim();
                if !inner.is_empty() {
                    kept.push(inner);
                }
            }
            // A block that closes on a later line.
            None => {
                if DROPPED_WRAPPER_TAGS.contains(&tag) {
                    skipping = Some(tag);
                }
                // A non-dropped opening tag contributes nothing itself; its
                // body is kept by the following iterations.
            }
        }
    }
    kept.join("\n")
}

/// The tag name of `<name ...>` or `</name>`, when the line opens with one.
///
/// Returns `None` for `<- an arrow` and `3 < 5`, which are prose.
fn opening_tag_name(line: &str) -> Option<&str> {
    let after = line.strip_prefix('<')?;
    let after = after.strip_prefix('/').unwrap_or(after);
    if !after.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    let end = after
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .unwrap_or(after.len());
    // Require the tag to actually close, or this is just prose starting with
    // `<` followed by a word.
    line.contains('>').then_some(&after[..end])
}

/// Is this an injected document rather than something a person typed?
///
/// Checks the preamble list against both the raw first line and the stripped
/// one: a marker that is not a well-formed tag survives stripping, while a
/// document wrapped in one only becomes visible after it.
#[must_use]
pub fn is_scaffolding_text(text: &str) -> bool {
    if first_line_is_preamble(text) {
        return true;
    }
    let stripped = strip_scaffolding(text);
    if stripped.trim().is_empty() {
        return true;
    }
    first_line_is_preamble(&stripped)
}

fn first_line_is_preamble(text: &str) -> bool {
    let Some(first_line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return false;
    };
    INJECTED_PREAMBLES
        .iter()
        .any(|preamble| first_line.starts_with(preamble))
}

/// A title candidate from one message's text, or `None` when the message is
/// entirely harness scaffolding.
#[must_use]
pub fn title_candidate(text: &str) -> Option<String> {
    if is_scaffolding_text(text) {
        return None;
    }
    title_from_text(&strip_scaffolding(text))
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
mod flattening_invariants {
    use super::super::types::{FileIdentity, SessionScope};
    use std::path::Path;

    /// The store stopped keeping `root_session_key` and `lineage_depth` on every
    /// message and reads them from the session row instead — 146,755 rows'
    /// worth of duplication on the measured corpus. That is only sound because
    /// the adapters flatten **per file**: one transcript is one session, and
    /// every message in it carries that session's lineage verbatim.
    ///
    /// This pins the property down rather than trusting it. An adapter that
    /// ever emits mixed lineage inside one file has to change the schema back,
    /// and this is what will say so.
    fn assert_uniform(adapted: &super::AdaptedTranscript, label: &str) {
        let session = adapted.session.as_ref().expect(label);
        for message in &adapted.messages {
            assert_eq!(
                message.lineage_depth, session.lineage_depth,
                "{label}: message {} has a different depth from its session",
                message.ordinal
            );
            assert_eq!(
                message.root_session_key, session.root_session_key,
                "{label}: message {} has a different root from its session",
                message.ordinal
            );
            assert_eq!(
                message.session_key, session.session_key,
                "{label}: message {} belongs to another session",
                message.ordinal
            );
            // And the pointer's file identity is the session's, which is why
            // `messages` no longer stores a path at all.
            assert_eq!(message.source.file_path, session.source.file_path, "{label}");
            assert_eq!(message.source.device, session.source.device, "{label}");
            assert_eq!(message.source.inode, session.source.inode, "{label}");
            assert_eq!(message.source.size_bytes, session.source.size_bytes, "{label}");
            assert_eq!(
                message.source.modified_unix_ms, session.source.modified_unix_ms,
                "{label}"
            );
        }
    }

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn adapters_assign_one_lineage_per_file() {
        let dir = tempfile::tempdir().unwrap();

        let claude = write(
            dir.path(),
            "c.jsonl",
            "{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"/r\",\"isSidechain\":true,\
             \"timestamp\":\"2026-09-01T00:00:00.000Z\",\"message\":{\"role\":\"user\",\
             \"content\":[{\"type\":\"text\",\"text\":\"one\"},\
             {\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"c\":\"ls\"}}]}}\n",
        );
        let identity = FileIdentity::read(&claude).unwrap();
        assert_uniform(
            &super::claude::adapt(&claude, &identity, "p", SessionScope::Scoped, &|_| None, None),
            "claude",
        );

        let pi = write(
            dir.path(),
            "2026-09-01T00-00-00-000Z_abcdef12.jsonl",
            "{\"type\":\"session\",\"id\":\"s\",\"timestamp\":\"2026-09-01T00:00:00.000Z\",\"cwd\":\"/r\"}\n\
             {\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-09-01T00:00:01.000Z\",\
             \"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"one\"}]}}\n\
             {\"type\":\"message\",\"id\":\"m2\",\"timestamp\":\"2026-09-01T00:00:02.000Z\",\
             \"message\":{\"role\":\"toolResult\",\"toolName\":\"Bash\",\"toolCallId\":\"t1\",\
             \"content\":\"done\"}}\n",
        );
        let identity = FileIdentity::read(&pi).unwrap();
        assert_uniform(
            &super::pi::adapt(
                &pi,
                &identity,
                "p",
                SessionScope::Scoped,
                super::super::types::LineageDepth::nested(1),
                Some("pi:/root.jsonl".to_string()),
                None,
            ),
            "pi",
        );

        let codex = write(
            dir.path(),
            "x.jsonl",
            "{\"type\":\"session_meta\",\"timestamp\":\"2026-09-01T00:00:00.000Z\",\
             \"payload\":{\"id\":\"s\",\"cwd\":\"/r\",\"source\":\"exec\"}}\n\
             {\"type\":\"response_item\",\"timestamp\":\"2026-09-01T00:00:01.000Z\",\
             \"payload\":{\"type\":\"message\",\"role\":\"user\",\
             \"content\":[{\"type\":\"input_text\",\"text\":\"one\"}]}}\n\
             {\"type\":\"response_item\",\"timestamp\":\"2026-09-01T00:00:02.000Z\",\
             \"payload\":{\"type\":\"function_call\",\"name\":\"bash\",\"call_id\":\"c1\",\
             \"arguments\":\"{}\"}}\n",
        );
        let identity = FileIdentity::read(&codex).unwrap();
        let meta = super::codex::read_meta(&codex).expect("meta");
        assert_uniform(
            &super::codex::adapt(
                &codex,
                &identity,
                "p",
                SessionScope::Scoped,
                &meta,
                &|_| None,
                None,
            ),
            "codex",
        );
    }
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
        let issues = for_each_record(&path, 0, |record| {
            seen.push((record.byte_offset, record.value["n"].as_u64().unwrap()));
        });
        assert!(issues.issues.is_empty());
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
        for_each_record(&path, 0, |record| {
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
        assert_eq!(
            pointers[2].read_bytes().as_deref(),
            Some(&b"{\"n\":333}"[..])
        );
    }

    #[test]
    fn blank_lines_and_unparseable_records_are_skipped_without_stopping() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        std::fs::write(&path, "{\"n\":1}\n\nnot json\n{\"n\":2}\n").unwrap();
        let mut count = 0;
        let issues = for_each_record(&path, 0, |_| count += 1);
        assert_eq!(count, 2);
        assert!(issues.issues.is_empty());
    }

    #[test]
    fn a_file_without_a_trailing_newline_still_yields_its_last_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        std::fs::write(&path, "{\"n\":1}\n{\"n\":2}").unwrap();
        let mut seen = Vec::new();
        for_each_record(&path, 0, |record| {
            seen.push(record.value["n"].as_u64().unwrap())
        });
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
        let issues = for_each_record(&path, 0, |record| {
            seen.push(record.value["n"].as_u64().unwrap_or(0));
        });
        assert_eq!(seen, vec![1, 2], "records after the oversized one are lost");
        assert_eq!(issues.issues.len(), 1);
        assert_eq!(issues.issues[0].code, ISSUE_OVERSIZED_RECORD);
    }

    #[test]
    fn a_missing_file_is_one_issue_and_no_records() {
        let scan = for_each_record(Path::new("/nonexistent/x.jsonl"), 0, |_| {
            panic!("must not visit any record")
        });
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(scan.issues[0].code, ISSUE_UNREADABLE);
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

    /// Measured against the real corpus: without this filter the most visible
    /// field in the index — the session title — reads
    /// `# AGENTS.md instructions for /Users/...` or
    /// `<local-command-caveat>Caveat: ...` for most sessions.
    #[test]
    fn injected_scaffolding_is_not_mistaken_for_a_first_message() {
        for scaffolding in [
            "# AGENTS.md instructions for /Users/qs/project/me/termul",
            "<local-command-caveat>Caveat: The messages below were generated by the user",
            "<teammate-message teammate_id=\"team-lead\" summary=\"Explore chat history\">",
            "<system-reminder>\nAs you answer the user's questions",
            "</thinking>",
            "<permissions instructions>\nFilesystem sandboxing defines",
            "# Analysis Mode Protocol",
            "   ",
        ] {
            assert!(
                is_scaffolding_text(scaffolding),
                "must be recognised as scaffolding: {scaffolding:.60}"
            );
        }
    }

    /// The point of stripping rather than rejecting: when a session starts with
    /// a slash command, the user's real request is inside `<command-args>`, and
    /// it is the best title the transcript has.
    #[test]
    fn a_slash_command_invocation_yields_the_argument_as_the_title() {
        let invocation = "<command-name>/maestro-odyssey</command-name>\n\
             <command-message>maestro-odyssey</command-message>\n\
             <command-args>把三家 agent 的历史会话统一为标准结构</command-args>";
        assert_eq!(
            title_candidate(invocation).as_deref(),
            Some("把三家 agent 的历史会话统一为标准结构")
        );
    }

    #[test]
    fn a_multi_line_wrapper_block_is_dropped_whole() {
        let wrapped = "<system-reminder>\nbackground context you must not act on\n</system-reminder>\nthe actual question";
        assert_eq!(strip_scaffolding(wrapped), "the actual question");
        assert_eq!(
            title_candidate(wrapped).as_deref(),
            Some("the actual question")
        );
    }

    /// An unrecognised wrapper keeps its body: stripping is allowed to lose the
    /// tag, never the content.
    #[test]
    fn an_unknown_wrapper_keeps_what_it_wraps() {
        assert_eq!(
            strip_scaffolding("<unknown-tag>\nreal content\n</unknown-tag>"),
            "real content"
        );
        assert_eq!(
            strip_scaffolding("<note>inline content</note>"),
            "inline content"
        );
    }

    /// Malformed markup must not be able to panic the stripper: it runs over
    /// every record of every transcript, so a crash here is a crash of the whole
    /// build.
    #[test]
    fn malformed_markup_is_survived_rather_than_panicking() {
        for malformed in [
            "</stray-closer>",
            "</a>inner<a>",
            "<a>",
            "<>",
            "< a>text</a>",
            "<a></a>",
            "<a>unclosed forever",
            "<a b='>'>x</a>",
        ] {
            let stripped = strip_scaffolding(malformed);
            let _ = title_candidate(&stripped);
        }
    }

    /// The filter has to stay narrow. A user really can open with a heading, a
    /// comparison, or an arrow.
    #[test]
    fn ordinary_first_messages_are_not_treated_as_scaffolding() {
        for real in [
            "fix the login redirect",
            "# Refactor plan\n\nstep one",
            "<- this arrow is not a tag",
            "3 < 5 should be true",
            "查看下现有项目,我现在想增加一个功能",
            "AGENTS.md needs a new section",
        ] {
            assert!(
                !is_scaffolding_text(real),
                "must not be treated as scaffolding: {real:.60}"
            );
        }
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
