//! The one normalized shape every vendor transcript is projected into.
//!
//! Deliberately **not** `ConversationEventRecordV2`. That type denies unknown
//! fields, validates its `conversationId` against the directory it was read
//! from, and requires a gapless monotonic `seq`. Manufacturing a `seq` for a
//! finished external transcript is exactly what that mechanism was built to
//! reject, so this module reuses its *ideas* — a bounded envelope, an opaque
//! payload, a sparse byte pointer — and none of its types.
//!
//! Field shapes mirror `conversation::catalog::ConversationCatalogEntryV1`
//! (`created_at_utc` / `message_count` / `tool_count`, RFC3339 strings on the
//! wire) so a session summary from this index reads the same as one of Termul's
//! own conversations.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::paths::MemoryVendor;

pub const SCHEMA_VERSION: u32 = 1;

/// What a normalized record is.
///
/// `ToolCall` and `ToolResult` are separate roles rather than one "tool" role:
/// a search for an error string should be able to hit the result without also
/// matching every invocation that produced one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    System,
    /// An agent-generated summary of context it was forced to drop. Only pi
    /// records these; Claude and Codex have no equivalent structured signal.
    Compaction,
}

impl NormalizedRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::System => "system",
            Self::Compaction => "compaction",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool_call" => Some(Self::ToolCall),
            "tool_result" => Some(Self::ToolResult),
            "system" => Some(Self::System),
            "compaction" => Some(Self::Compaction),
            _ => None,
        }
    }
}

/// How much a timestamp can be trusted.
///
/// v1 only ever produces [`TimestampConfidence::Native`] — all three vendors
/// stamp their records. The weaker variants are kept in the schema because the
/// vendor that needs them (Cursor, whose records carry no timestamp field at
/// all) is a known future case, and adding a confidence column later would be a
/// schema migration on a multi-gigabyte index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampConfidence {
    /// Read from the record itself.
    Native,
    /// Inherited from the session's start time; ordering within a session is
    /// positional, not temporal.
    SessionStart,
    /// The file's mtime. Says when the transcript was last touched, not when
    /// the message was sent.
    FileMtime,
    /// No time information at all.
    Unknown,
}

impl TimestampConfidence {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::SessionStart => "session_start",
            Self::FileMtime => "file_mtime",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "native" => Self::Native,
            "session_start" => Self::SessionStart,
            "file_mtime" => Self::FileMtime,
            _ => Self::Unknown,
        }
    }
}

/// How deep a message sits in the subagent hierarchy: `0` root, `1` subagent,
/// `2` a subagent's subagent, `None` when the vendor records no depth.
///
/// A newtype rather than a bare `Option<u8>` for one reason: `None` must stay
/// `None`. Codex's `agent_job` shape — 1382 of its 1844 subagent sessions on the
/// measured machine — carries neither a depth nor a parent pointer. Defaulting
/// those to `0` does not produce a harmless approximation; it produces the
/// sentence "this is a root message", which is false. [`Self::UNKNOWN`] is the
/// only way to spell that, and it is not reachable by `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LineageDepth(Option<u8>);

impl LineageDepth {
    /// A top-level session's own messages.
    pub const ROOT: Self = Self(Some(0));
    /// The vendor does not record a depth for this message.
    pub const UNKNOWN: Self = Self(None);

    /// A nested session at `depth` levels below its root.
    #[must_use]
    pub const fn nested(depth: u8) -> Self {
        Self(Some(depth))
    }

    #[must_use]
    pub const fn value(self) -> Option<u8> {
        self.0
    }

    #[must_use]
    pub const fn is_known(self) -> bool {
        self.0.is_some()
    }

    #[must_use]
    pub const fn is_root(self) -> bool {
        matches!(self.0, Some(0))
    }

    /// Rebuild from a stored column. `None` in the database is unknown, not
    /// zero — the round trip has to preserve that distinction.
    #[must_use]
    pub const fn from_stored(value: Option<u8>) -> Self {
        Self(value)
    }
}

/// File identity plus the byte range of one record inside it.
///
/// `(path, offset)` alone is not enough: a transcript that gets rewritten leaves
/// the offset valid and the content wrong, which would silently return another
/// session's bytes. The guard is layered.
///
/// * `device` / `inode` — is this even the same file? (Both `0` on Windows,
///   which has no such pair; there the remaining three layers carry the check.)
/// * `size_bytes` / `modified_unix_ms` — has the file changed at all? Cheap
///   enough to check on every read.
/// * `content_hash` — a digest of **the pointed-to bytes**, not of the whole
///   file. That is both affordable on a 2.6 GB transcript and a stronger test
///   for the failure that matters: an append leaves the record's own bytes
///   intact and still verifies, while a rewrite changes them and is caught.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePointer {
    pub file_path: String,
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_unix_ms: i64,
    /// Lowercase hex SHA-256 of `file_path[byte_offset .. byte_offset+byte_len]`.
    pub content_hash: String,
    pub byte_offset: u64,
    pub byte_len: u64,
}

impl SourcePointer {
    /// Build a pointer for a record whose bytes are already in memory.
    #[must_use]
    pub fn for_record(
        identity: &FileIdentity,
        file_path: &str,
        byte_offset: u64,
        record_bytes: &[u8],
    ) -> Self {
        Self {
            file_path: file_path.to_string(),
            device: identity.device,
            inode: identity.inode,
            size_bytes: identity.size_bytes,
            modified_unix_ms: identity.modified_unix_ms,
            content_hash: hash_bytes(record_bytes),
            byte_offset,
            byte_len: record_bytes.len() as u64,
        }
    }
}

/// Whether a stored pointer still describes the bytes it was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerFreshness {
    /// The bytes at the recorded range still hash to the recorded digest.
    Fresh,
    /// They do not. The record may still exist somewhere in the file, but this
    /// pointer no longer locates it, so nothing may be returned from it.
    Stale,
}

/// Identity of a whole transcript file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_unix_ms: i64,
}

impl FileIdentity {
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let (device, inode) = device_and_inode(&metadata);
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|delta| delta.as_millis() as i64)
            })
            .unwrap_or_default();
        Ok(Self {
            device,
            inode,
            size_bytes: metadata.len(),
            modified_unix_ms,
        })
    }

    /// True when a previously indexed file is byte-identical in identity terms,
    /// so ingest may skip re-reading it.
    ///
    /// Conservative on purpose: any doubt re-indexes. Re-reading a file costs
    /// time; skipping a changed one costs correctness.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.size_bytes == other.size_bytes
            && self.modified_unix_ms == other.modified_unix_ms
    }
}

#[cfg(unix)]
fn device_and_inode(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

/// Windows has no `(dev, ino)` pair reachable from `std`. Reporting `0` is
/// honest — it says "this layer is unavailable here" rather than inventing an
/// identity — and the size/mtime/content-hash layers still run.
#[cfg(not(unix))]
fn device_and_inode(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One normalized record: a user message, an assistant reply, a tool call, a
/// tool result, or a compaction summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedMessage {
    pub schema_version: u32,
    /// `<session_key>#<ordinal>` — stable across re-ingest of an unchanged file,
    /// which is what makes upsert idempotent.
    pub message_key: String,
    pub session_key: String,
    /// The top-level session this record ultimately belongs to. After
    /// flattening, this is the only thing that can put a subagent message back
    /// with the conversation it came from.
    pub root_session_key: String,
    pub lineage_depth: LineageDepth,
    pub ordinal: u32,
    pub role: NormalizedRole,
    /// RFC3339 UTC, for display and the wire.
    pub timestamp_utc: Option<String>,
    /// Epoch milliseconds, for ordering. Kept alongside the string rather than
    /// derived from it so ordering never depends on two RFC3339 renderings
    /// being lexicographically comparable.
    pub timestamp_ms: Option<i64>,
    pub timestamp_confidence: TimestampConfidence,
    /// Searchable text, already redacted. Never the raw record.
    pub text: String,
    pub tool_name: Option<String>,
    /// The vendor's own correlation id, so a call and its result can be paired.
    pub tool_call_id: Option<String>,
    pub source: SourcePointer,
}

/// A pi `compaction` record.
///
/// Kept in its own table and surfaced ahead of ordinary matches: it is the only
/// ready-made, agent-authored summary in any of the three corpora, and it names
/// which entries were pushed out of the active context. That turns "memory
/// bank" from a slogan into something specific — recovering what the agent was
/// forced to forget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionRecord {
    pub schema_version: u32,
    pub session_key: String,
    pub root_session_key: String,
    pub ordinal: u32,
    pub summary: String,
    pub tokens_before: Option<u64>,
    pub first_kept_entry_id: Option<String>,
    pub timestamp_utc: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub source: SourcePointer,
}

/// Whether a session's project ownership could be proven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionScope {
    /// The vendor folder encodes this project, or the transcript's own recorded
    /// cwd is inside it.
    Scoped,
    /// Ownership could not be proven. Indexed but excluded from results by
    /// default — an open UI panel is not evidence that a transcript belongs to
    /// the project it happens to be showing.
    Unscoped,
}

/// Session-level summary. Mirrors the catalog projection's field shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedSession {
    pub schema_version: u32,
    /// `<vendor>:<absolute file path>`. The file is the session's identity here:
    /// pi nests subagent transcripts as separate files under one root, and two
    /// vendors can hand out the same session id.
    pub session_key: String,
    pub vendor: String,
    /// The id the vendor itself uses, for resume and cross-referencing.
    pub vendor_session_id: String,
    pub root_session_key: String,
    pub lineage_depth: LineageDepth,
    pub project_key: String,
    pub scope: SessionScope,
    /// The transcript's own recorded working directory, when it has one.
    pub cwd: Option<String>,
    pub title: Option<String>,
    /// **The sort key.** First message's native timestamp — not the file mtime,
    /// which reorders 9 of this project's 14 Claude sessions and drifts by up to
    /// six days.
    pub first_message_at_utc: Option<String>,
    pub first_message_at_ms: Option<i64>,
    pub last_activity_at_utc: Option<String>,
    pub last_activity_at_ms: Option<i64>,
    pub timestamp_confidence: TimestampConfidence,
    pub message_count: u64,
    pub tool_count: u64,
    pub file_path: String,
    pub source: SourcePointer,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> FileIdentity {
        FileIdentity {
            device: 7,
            inode: 99,
            size_bytes: 4096,
            modified_unix_ms: 1_757_000_000_000,
        }
    }

    /// AC2. The distinction this newtype exists to protect. A serialized
    /// unknown depth must be `null`; anything else asserts a hierarchy position
    /// the vendor never recorded.
    #[test]
    fn unknown_lineage_depth_serializes_as_null_not_zero() {
        assert_eq!(
            serde_json::to_string(&LineageDepth::UNKNOWN).unwrap(),
            "null"
        );
        assert_eq!(serde_json::to_string(&LineageDepth::ROOT).unwrap(), "0");
        assert_eq!(
            serde_json::to_string(&LineageDepth::nested(2)).unwrap(),
            "2"
        );
    }

    #[test]
    fn lineage_depth_round_trips_unknown_through_storage() {
        assert_eq!(LineageDepth::from_stored(None), LineageDepth::UNKNOWN);
        assert_eq!(LineageDepth::from_stored(Some(0)), LineageDepth::ROOT);
        assert!(!LineageDepth::UNKNOWN.is_known());
        assert!(!LineageDepth::UNKNOWN.is_root());
        assert!(LineageDepth::ROOT.is_root());
        assert!(
            !LineageDepth::nested(1).is_root(),
            "a subagent message is not a root message"
        );
    }

    #[test]
    fn source_pointer_hashes_the_record_not_the_file() {
        let record = br#"{"type":"user","text":"hello"}"#;
        let pointer = SourcePointer::for_record(&identity(), "/tmp/a.jsonl", 1234, record);
        assert_eq!(pointer.byte_offset, 1234);
        assert_eq!(pointer.byte_len, record.len() as u64);
        assert_eq!(pointer.content_hash, hash_bytes(record));
        // The file is 4096 bytes; the digest is over 30 of them.
        assert_ne!(pointer.content_hash, hash_bytes(&vec![0u8; 4096]));
        assert_eq!(pointer.size_bytes, 4096);
    }

    #[test]
    fn file_identity_mismatch_is_detected_on_every_layer() {
        let base = identity();
        for changed in [
            FileIdentity { device: 8, ..base },
            FileIdentity { inode: 100, ..base },
            FileIdentity {
                size_bytes: 4097,
                ..base
            },
            FileIdentity {
                modified_unix_ms: base.modified_unix_ms + 1,
                ..base
            },
        ] {
            assert!(
                !base.matches(&changed),
                "a change in one identity field must not read as unchanged: {changed:?}"
            );
        }
        assert!(base.matches(&identity()));
    }

    #[test]
    fn file_identity_reads_real_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        fs::write(&path, b"0123456789").unwrap();
        let first = FileIdentity::read(&path).unwrap();
        assert_eq!(first.size_bytes, 10);
        #[cfg(unix)]
        assert!(
            first.inode != 0,
            "a real unix file must report a real inode"
        );

        fs::write(&path, b"0123456789abc").unwrap();
        let second = FileIdentity::read(&path).unwrap();
        assert!(
            !first.matches(&second),
            "an appended file must not read as unchanged"
        );
    }

    #[test]
    fn role_and_confidence_survive_a_string_round_trip() {
        for role in [
            NormalizedRole::User,
            NormalizedRole::Assistant,
            NormalizedRole::ToolCall,
            NormalizedRole::ToolResult,
            NormalizedRole::System,
            NormalizedRole::Compaction,
        ] {
            assert_eq!(NormalizedRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(NormalizedRole::parse("thinking"), None);
        for confidence in [
            TimestampConfidence::Native,
            TimestampConfidence::SessionStart,
            TimestampConfidence::FileMtime,
            TimestampConfidence::Unknown,
        ] {
            assert_eq!(
                TimestampConfidence::parse(confidence.as_str()),
                confidence
            );
        }
    }

    /// The wire shape is what the renderer and the MCP clients consume, so the
    /// camelCase contract is worth pinning.
    #[test]
    fn session_summary_uses_the_catalog_field_shape() {
        let session = IndexedSession {
            schema_version: SCHEMA_VERSION,
            session_key: "pi:/t/a.jsonl".into(),
            vendor: MemoryVendor::Pi.as_str().into(),
            vendor_session_id: "sess-1".into(),
            root_session_key: "pi:/t/a.jsonl".into(),
            lineage_depth: LineageDepth::ROOT,
            project_key: "termul-0011223344556677".into(),
            scope: SessionScope::Scoped,
            cwd: Some("/t".into()),
            title: Some("hello".into()),
            first_message_at_utc: Some("2026-09-01T00:00:00Z".into()),
            first_message_at_ms: Some(1_756_684_800_000),
            last_activity_at_utc: Some("2026-09-01T01:00:00Z".into()),
            last_activity_at_ms: Some(1_756_688_400_000),
            timestamp_confidence: TimestampConfidence::Native,
            message_count: 3,
            tool_count: 1,
            file_path: "/t/a.jsonl".into(),
            source: SourcePointer::for_record(&identity(), "/t/a.jsonl", 0, b"{}"),
        };
        let value = serde_json::to_value(&session).unwrap();
        for key in [
            "sessionKey",
            "rootSessionKey",
            "lineageDepth",
            "firstMessageAtUtc",
            "firstMessageAtMs",
            "lastActivityAtUtc",
            "messageCount",
            "toolCount",
            "projectKey",
        ] {
            assert!(value.get(key).is_some(), "missing wire field {key}");
        }
        assert_eq!(value["lineageDepth"], 0);
        assert_eq!(value["scope"], "scoped");
        assert_eq!(value["vendor"], "pi");
    }
}
