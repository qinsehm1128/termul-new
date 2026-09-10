//! pi transcripts.
//!
//! Shape (verified against this project's 418-file store): the first record is
//! `{type:"session", id, timestamp, cwd, version}`; conversation records are
//! `{type:"message", id, parentId, timestamp, message:{role, content}}` with
//! roles `user`, `assistant` and `toolResult`. An assistant message's content
//! blocks are `thinking`, `text` and `toolCall {id, name, arguments}`; a
//! `toolResult` carries `toolCallId`, `toolName` and `isError` on the message
//! itself.
//!
//! ## Two traps this module exists to avoid
//!
//! 1. **Not every `.jsonl` here is a transcript.** The nested directories also
//!    hold `permissions.jsonl` (39 files, up to 9.8 MB, on the measured
//!    machine). Filtering by extension indexes those as sessions. Transcripts
//!    are identified by filename pattern instead — see [`is_transcript_name`].
//! 2. **`parentId` describes a graph, not a list.** Measured over 416 files and
//!    91,569 records there are zero forks, so v1 stores records in file order
//!    and treats the chain as linear — but it asserts the assumption instead of
//!    relying on it. A fork is reported as an issue, and because ordering comes
//!    from file position rather than from walking the chain, no branch is
//!    dropped even when one appears.
//!
//! ## Lineage
//!
//! pi records no depth field. The hierarchy is the directory nesting:
//! `<root>.jsonl`, then `<root-stem>/<uuid>/<child>.jsonl`, then
//! `<root-stem>/<uuid>/<child-stem>/<uuid>/<grandchild>.jsonl` — two path
//! segments per level, which is why the measured segment counts are exactly
//! 0, 2 and 4.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::super::paths::MemoryVendor;
use super::super::types::{
    CompactionRecord, FileIdentity, IndexedSession, LineageDepth, NormalizedMessage,
    NormalizedRole, SessionScope, SourcePointer, TimestampConfidence, SCHEMA_VERSION,
};
use super::{
    flatten_text, for_each_record, parse_timestamp, prepare_indexed_text, session_key,
    string_field, title_candidate, AdaptedTranscript, AdapterIssue, ResumeFrom,
    ISSUE_LINEAGE_FORK, ISSUE_NO_SESSION_HEADER,
};

/// Is this filename a pi transcript?
///
/// `<ISO8601 with dashes>_<uuid>.jsonl`, e.g.
/// `2026-08-14T02-12-05-826Z_019ffe0a-7442-7750-ad5a-7d82a3a48ab0.jsonl`.
/// Matching the pattern rather than the extension is what keeps
/// `permissions.jsonl` out of the index.
#[must_use]
pub fn is_transcript_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".jsonl") else {
        return false;
    };
    let Some((timestamp, id)) = stem.split_once('_') else {
        return false;
    };
    if id.len() < 8 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    // `YYYY-MM-DDTHH-MM-SS-mmmZ`
    let mut characters = timestamp.chars();
    let date_ok = (0..10).all(|index| {
        matches!(
            (index, characters.next()),
            (0..=3, Some(c)) | (5..=6, Some(c)) | (8..=9, Some(c)) if c.is_ascii_digit()
        ) || matches!(
            (index, timestamp.as_bytes().get(index)),
            (4 | 7, Some(b'-'))
        )
    });
    date_ok && timestamp.len() > 11 && timestamp.as_bytes().get(10) == Some(&b'T')
}

/// Lineage depth from a transcript's position under the project directory.
///
/// Two path segments per level: `<root-stem>/<uuid>/` for a child,
/// `<root-stem>/<uuid>/<child-stem>/<uuid>/` for a grandchild. An odd count is
/// a shape this adapter has never seen; it yields `None` rather than a rounded
/// guess.
#[must_use]
pub fn depth_from_path(project_dir: &Path, path: &Path) -> LineageDepth {
    let Ok(relative) = path.strip_prefix(project_dir) else {
        return LineageDepth::UNKNOWN;
    };
    let segments = relative.components().count().saturating_sub(1);
    match u8::try_from(segments) {
        Ok(segments) if segments % 2 == 0 => LineageDepth::nested(segments / 2),
        _ => LineageDepth::UNKNOWN,
    }
}

/// The top-level transcript a nested one belongs to.
///
/// The first path segment under the project directory is the root transcript's
/// stem, so the root file is `<project_dir>/<first-segment>.jsonl`. Returns
/// `None` for a transcript that is already at the root.
#[must_use]
pub fn root_file_for(project_dir: &Path, path: &Path) -> Option<std::path::PathBuf> {
    let relative = path.strip_prefix(project_dir).ok()?;
    let mut components = relative.components();
    let first = components.next()?;
    // A root transcript has no further components after its own file name.
    if components.next().is_none() {
        return None;
    }
    Some(project_dir.join(format!("{}.jsonl", first.as_os_str().to_string_lossy())))
}

/// Adapt one pi transcript.
pub fn adapt(
    path: &Path,
    identity: &FileIdentity,
    project_key: &str,
    scope: SessionScope,
    depth: LineageDepth,
    root_session_key: Option<String>,
    resume: Option<ResumeFrom>,
) -> AdaptedTranscript {
    let own_key = session_key(MemoryVendor::Pi, path);
    let root_session_key = root_session_key.unwrap_or_else(|| own_key.clone());
    let file_path = path.to_string_lossy().into_owned();

    let mut out = AdaptedTranscript::default();
    let mut vendor_session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    // Fallback when every user record is harness scaffolding, mirroring
    // `cli_session::parse`, which also falls back to the first assistant line.
    let mut assistant_title: Option<String> = None;
    let mut first: Option<(String, i64)> = None;
    let mut last: Option<(String, i64)> = None;
    let mut messages: Vec<NormalizedMessage> = Vec::new();
    let mut compactions: Vec<CompactionRecord> = Vec::new();
    let mut tool_count: u64 = 0;
    let mut ordinal: u32 = resume.map_or(0, |from| from.next_ordinal);
    // parentId -> how many records claim it. More than one is a fork.
    let mut children: HashMap<String, u32> = HashMap::new();

    let mut push = |emitted: Emitted,
                    stamp: &Option<(String, i64)>,
                    record: &super::RawRecord,
                    messages: &mut Vec<NormalizedMessage>,
                    ordinal: &mut u32| {
        let text = prepare_indexed_text(&emitted.text);
        if text.is_empty() {
            return;
        }
        if matches!(emitted.role, NormalizedRole::ToolCall) {
            tool_count += 1;
        }
        if title.is_none() && matches!(emitted.role, NormalizedRole::User) {
            title = title_candidate(&text);
        }
        if assistant_title.is_none() && matches!(emitted.role, NormalizedRole::Assistant) {
            assistant_title = title_candidate(&text);
        }
        messages.push(NormalizedMessage {
            schema_version: SCHEMA_VERSION,
            message_key: format!("{own_key}#{ordinal}"),
            session_key: own_key.clone(),
            root_session_key: root_session_key.clone(),
            lineage_depth: depth,
            ordinal: *ordinal,
            role: emitted.role,
            timestamp_utc: stamp.as_ref().map(|(utc, _)| utc.clone()),
            timestamp_ms: stamp.as_ref().map(|(_, ms)| *ms),
            timestamp_confidence: if stamp.is_some() {
                TimestampConfidence::Native
            } else {
                TimestampConfidence::Unknown
            },
            text,
            tool_name: emitted.tool_name,
            tool_call_id: emitted.tool_call_id,
            source: SourcePointer::for_record(
                identity,
                &file_path,
                record.byte_offset,
                &record.bytes,
            ),
        });
        *ordinal += 1;
    };

    let scan = for_each_record(path, resume.map_or(0, |from| from.byte_offset), |record| {
        let value = &record.value;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(parent) = value.get("parentId").and_then(Value::as_str) {
            *children.entry(parent.to_string()).or_default() += 1;
        }
        let stamp = string_field(value, &["timestamp"]).and_then(|raw| parse_timestamp(&raw));

        match kind {
            "session" => {
                if vendor_session_id.is_none() {
                    vendor_session_id = string_field(value, &["id"]);
                }
                if cwd.is_none() {
                    cwd = string_field(value, &["cwd"]);
                }
                return;
            }
            "compaction" => {
                let summary = string_field(value, &["summary"]).unwrap_or_default();
                if summary.is_empty() {
                    return;
                }
                compactions.push(CompactionRecord {
                    schema_version: SCHEMA_VERSION,
                    session_key: own_key.clone(),
                    root_session_key: root_session_key.clone(),
                    ordinal,
                    summary: prepare_indexed_text(&summary),
                    tokens_before: value.get("tokensBefore").and_then(Value::as_u64),
                    first_kept_entry_id: string_field(value, &["firstKeptEntryId"]),
                    timestamp_utc: stamp.as_ref().map(|(utc, _)| utc.clone()),
                    timestamp_ms: stamp.as_ref().map(|(_, ms)| *ms),
                    source: SourcePointer::for_record(
                        identity,
                        &file_path,
                        record.byte_offset,
                        &record.bytes,
                    ),
                });
                return;
            }
            "custom_message" => {
                // Human-readable lifecycle prose, e.g.
                // "● @code-exploration spawned @explore-terminal". Worth
                // keeping: it is the only place pi states a spawn in words.
                if value.get("display").and_then(Value::as_bool) != Some(true) {
                    return;
                }
                if let Some(text) = string_field(value, &["content"]) {
                    push(
                        Emitted {
                            role: NormalizedRole::System,
                            text,
                            tool_name: None,
                            tool_call_id: None,
                        },
                        &stamp,
                        &record,
                        &mut messages,
                        &mut ordinal,
                    );
                }
                return;
            }
            // `custom` holds opaque state blobs (goal-state and friends);
            // `model_change` / `thinking_level_change` are settings.
            "message" => {}
            _ => return,
        }

        let Some(message) = value.get("message") else {
            return;
        };
        if let Some(ref stamp) = stamp {
            if first.is_none() {
                first = Some(stamp.clone());
            }
            last = Some(stamp.clone());
        }
        for emitted in split_message(message) {
            push(emitted, &stamp, &record, &mut messages, &mut ordinal);
        }
    });

    let mut issues = scan.issues;
    let forks: Vec<&String> = children
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(parent, _)| parent)
        .collect();
    if !forks.is_empty() {
        issues.push(AdapterIssue::new(
            ISSUE_LINEAGE_FORK,
            path,
            format!(
                "{} parent record(s) have more than one child, so this transcript is a graph \
                 rather than a chain; records are indexed in file order so no branch is dropped, \
                 but the linear assumption no longer holds (first: {})",
                forks.len(),
                forks[0]
            ),
        ));
    }

    // See `claude::adapt` — a resumed run returns the tail only. For pi this is
    // also the only workable shape: `type:"session"` is the first record and
    // nothing else carries the id or the cwd, so a tail could not build a
    // session row even if it wanted to.
    if resume.is_some() {
        out.messages = messages;
        out.compactions = compactions;
        out.issues = issues;
        out.resume_offset = scan.resume_offset;
        return out;
    }
    let Some(vendor_session_id) = vendor_session_id else {
        issues.push(AdapterIssue::new(
            ISSUE_NO_SESSION_HEADER,
            path,
            "no session record with an id",
        ));
        out.issues = issues;
        return out;
    };

    out.session = Some(IndexedSession {
        schema_version: SCHEMA_VERSION,
        session_key: own_key,
        vendor: MemoryVendor::Pi.as_str().to_string(),
        vendor_session_id,
        root_session_key,
        lineage_depth: depth,
        project_key: project_key.to_string(),
        scope,
        cwd,
        title: title.or(assistant_title),
        first_message_at_utc: first.as_ref().map(|(utc, _)| utc.clone()),
        first_message_at_ms: first.as_ref().map(|(_, ms)| *ms),
        last_activity_at_utc: last.as_ref().map(|(utc, _)| utc.clone()),
        last_activity_at_ms: last.as_ref().map(|(_, ms)| *ms),
        timestamp_confidence: if first.is_some() {
            TimestampConfidence::Native
        } else {
            TimestampConfidence::Unknown
        },
        message_count: messages.len() as u64,
        tool_count,
        file_path,
        source: SourcePointer::for_file(identity, &path.to_string_lossy()),
    });
    out.messages = messages;
    out.compactions = compactions;
    out.issues = issues;
    out.resume_offset = scan.resume_offset;
    out
}

struct Emitted {
    role: NormalizedRole,
    text: String,
    tool_name: Option<String>,
    tool_call_id: Option<String>,
}

/// Split one pi message into normalized records.
///
/// `toolResult` is a **role** here, not a content block — it is 60.6% of all
/// records on the measured corpus, which is also why redaction matters more for
/// pi than for the other two vendors.
fn split_message(message: &Value) -> Vec<Emitted> {
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    if role == "toolResult" {
        return vec![Emitted {
            role: NormalizedRole::ToolResult,
            text: message.get("content").map(flatten_text).unwrap_or_default(),
            tool_name: string_field(message, &["toolName"]),
            tool_call_id: string_field(message, &["toolCallId"]),
        }];
    }

    let base_role = match role {
        "user" => NormalizedRole::User,
        "assistant" => NormalizedRole::Assistant,
        _ => NormalizedRole::System,
    };
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        let text = message.get("content").map(flatten_text).unwrap_or_default();
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![Emitted {
                role: base_role,
                text,
                tool_name: None,
                tool_call_id: None,
            }]
        };
    };

    let mut out = Vec::new();
    let mut prose = String::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                let text = flatten_text(block);
                if !text.is_empty() {
                    if !prose.is_empty() {
                        prose.push('\n');
                    }
                    prose.push_str(&text);
                }
            }
            "toolCall" => out.push(Emitted {
                role: NormalizedRole::ToolCall,
                text: match block.get("arguments") {
                    Some(Value::String(text)) => text.clone(),
                    Some(value) => serde_json::to_string(value).unwrap_or_default(),
                    None => String::new(),
                },
                tool_name: string_field(block, &["name"]),
                tool_call_id: string_field(block, &["id"]),
            }),
            // `thinking` carries a signature and prose that pi does not persist
            // in a form worth indexing.
            _ => {}
        }
    }
    if !prose.is_empty() {
        out.insert(
            0,
            Emitted {
                role: base_role,
                text: prose,
                tool_name: None,
                tool_call_id: None,
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = r#"{"type":"session","version":3,"id":"019ffe0a-7442-7750-ad5a-7d82a3a48ab0","timestamp":"2026-08-14T02:12:05.826Z","cwd":"/repo"}"#;
    const USER: &str = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-08-14T02:12:32.543Z","message":{"role":"user","content":[{"type":"text","text":"analyse the store"}],"timestamp":"2026-08-14T02:12:32.543Z"}}"#;
    const ASSISTANT: &str = r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-08-14T02:13:00.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm","thinkingSignature":"x"},{"type":"text","text":"running a search"},{"type":"toolCall","id":"call_1","name":"bash","arguments":"{'command': 'rg store'}"}]}}"#;
    const RESULT: &str = r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2026-08-14T02:13:05.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","isError":false,"content":[{"type":"text","text":"src/store.rs:12"}]}}"#;

    fn write(dir: &Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        path
    }

    /// AC5. The filename pattern is the identifier, not the extension.
    #[test]
    fn only_timestamped_uuid_filenames_are_transcripts() {
        assert!(is_transcript_name(
            "2026-08-14T02-12-05-826Z_019ffe0a-7442-7750-ad5a-7d82a3a48ab0.jsonl"
        ));
        for not_a_transcript in [
            "permissions.jsonl",
            "notes.jsonl",
            "2026-08-14T02-12-05-826Z.jsonl",
            "_019ffe0a.jsonl",
            "2026-08-14T02-12-05-826Z_short.jsonl",
            "2026-08-14T02-12-05-826Z_019ffe0a-7442.json",
            "random_019ffe0a-7442-7750-ad5a-7d82a3a48ab0.jsonl",
        ] {
            assert!(
                !is_transcript_name(not_a_transcript),
                "{not_a_transcript} must not be treated as a transcript"
            );
        }
    }

    /// The measured nesting: 0, 2 and 4 path segments — two per level.
    #[test]
    fn depth_comes_from_two_path_segments_per_level() {
        let project = Path::new("/pi/--Users-qs-project-me-termul--");
        assert_eq!(
            depth_from_path(
                project,
                &project.join("2026-08-14T02-12-05-826Z_aaaaaaaa.jsonl")
            ),
            LineageDepth::ROOT
        );
        assert_eq!(
            depth_from_path(
                project,
                &project
                    .join("2026-08-14T02-12-05-826Z_aaaaaaaa")
                    .join("01e3a87e-d806-4a82-85b2-f4331bcd8f65")
                    .join("2026-08-17T03-19-57-645Z_bbbbbbbb.jsonl")
            ),
            LineageDepth::nested(1)
        );
        assert_eq!(
            depth_from_path(
                project,
                &project
                    .join("2026-08-14T02-12-05-826Z_aaaaaaaa")
                    .join("01e3a87e")
                    .join("2026-08-17T03-19-57-645Z_bbbbbbbb")
                    .join("042c1fde")
                    .join("2026-08-18T00-00-00-000Z_cccccccc.jsonl")
            ),
            LineageDepth::nested(2)
        );
    }

    /// An unrecognised shape must not be rounded into a plausible depth.
    #[test]
    fn an_unexpected_nesting_shape_yields_unknown_depth() {
        let project = Path::new("/pi/proj");
        assert_eq!(
            depth_from_path(project, &project.join("odd").join("x.jsonl")),
            LineageDepth::UNKNOWN,
            "one segment is not a level; pi nests two at a time"
        );
        assert_eq!(
            depth_from_path(project, Path::new("/elsewhere/x.jsonl")),
            LineageDepth::UNKNOWN
        );
    }

    #[test]
    fn a_nested_transcript_resolves_its_root_file() {
        let project = Path::new("/pi/proj");
        let root = project.join("2026-08-14T02-12-05-826Z_aaaaaaaa.jsonl");
        assert_eq!(
            root_file_for(project, &root),
            None,
            "a root has no root file"
        );
        let child = project
            .join("2026-08-14T02-12-05-826Z_aaaaaaaa")
            .join("01e3a87e")
            .join("2026-08-17T03-19-57-645Z_bbbbbbbb.jsonl");
        assert_eq!(
            root_file_for(project, &child),
            Some(root),
            "the first path segment is the root transcript's stem"
        );
    }

    #[test]
    fn a_transcript_normalizes_roles_tools_and_first_message_time() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, ASSISTANT, RESULT],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        let session = adapted.session.expect("session");
        assert_eq!(
            session.vendor_session_id,
            "019ffe0a-7442-7750-ad5a-7d82a3a48ab0"
        );
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.title.as_deref(), Some("analyse the store"));
        assert_eq!(
            session.first_message_at_utc.as_deref(),
            Some("2026-08-14T02:12:32.543Z"),
            "the session header's own time is not the first message's"
        );
        assert_eq!(session.tool_count, 1);

        let roles: Vec<NormalizedRole> = adapted.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                NormalizedRole::User,
                NormalizedRole::Assistant,
                NormalizedRole::ToolCall,
                NormalizedRole::ToolResult,
            ]
        );
        assert_eq!(adapted.messages[2].tool_name.as_deref(), Some("bash"));
        assert_eq!(
            adapted.messages[3].tool_call_id.as_deref(),
            adapted.messages[2].tool_call_id.as_deref()
        );
        assert_eq!(adapted.messages[3].text, "src/store.rs:12");
        assert!(adapted.issues.is_empty());
    }

    /// AC6. A fork must be reported, and every branch must survive.
    #[test]
    fn a_forked_parent_is_reported_and_no_branch_is_dropped() {
        let temp = tempfile::tempdir().unwrap();
        let branch_a = r#"{"type":"message","id":"m2a","parentId":"m1","timestamp":"2026-08-14T02:13:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"branch alpha"}]}}"#;
        let branch_b = r#"{"type":"message","id":"m2b","parentId":"m1","timestamp":"2026-08-14T02:13:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"branch beta"}]}}"#;
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, branch_a, branch_b],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        let fork = adapted
            .issues
            .iter()
            .find(|issue| issue.code == ISSUE_LINEAGE_FORK)
            .expect("a fork must be reported, not assumed away");
        assert!(fork.detail.contains("m1"), "{}", fork.detail);

        let texts: Vec<&str> = adapted.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"branch alpha"));
        assert!(
            texts.contains(&"branch beta"),
            "silently taking one branch is exactly what the assertion forbids"
        );
    }

    /// A linear chain must not be misreported as a fork.
    #[test]
    fn a_linear_chain_reports_no_fork() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, ASSISTANT, RESULT],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        assert!(adapted
            .issues
            .iter()
            .all(|issue| issue.code != ISSUE_LINEAGE_FORK));
    }

    /// pi's compaction records are the one ready-made agent summary in any of
    /// the three corpora, and they name what was dropped.
    #[test]
    fn compactions_are_collected_with_their_dropped_context_markers() {
        let temp = tempfile::tempdir().unwrap();
        // `r##` because a real pi summary opens with a markdown heading, and
        // `"##` would close a single-hash raw string.
        let compaction = r###"{"type":"compaction","id":"c1","parentId":"m3","timestamp":"2026-08-14T06:33:54.322Z","summary":"## Session\n- summarised the migration attempt","tokensBefore":120000,"firstKeptEntryId":"entry-42","fromHook":false}"###;
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, compaction],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        assert_eq!(adapted.compactions.len(), 1);
        let record = &adapted.compactions[0];
        assert_eq!(record.tokens_before, Some(120_000));
        assert_eq!(record.first_kept_entry_id.as_deref(), Some("entry-42"));
        assert!(record.summary.contains("migration attempt"));
        assert!(
            !adapted
                .messages
                .iter()
                .any(|m| m.text.contains("migration attempt")),
            "a compaction belongs in its own table, not in the message stream"
        );
    }

    #[test]
    fn teammate_lifecycle_prose_is_kept_but_state_blobs_are_not() {
        let temp = tempfile::tempdir().unwrap();
        let spawn = r#"{"type":"custom_message","customType":"teammate-started","content":"● @code-exploration spawned @explore-terminal","display":true,"id":"a1","parentId":"m1","timestamp":"2026-08-14T02:23:58.819Z"}"#;
        let hidden = r#"{"type":"custom_message","customType":"internal","content":"not for humans","display":false,"id":"a2","parentId":"a1","timestamp":"2026-08-14T02:24:00.000Z"}"#;
        let blob = r#"{"type":"custom","customType":"goal-state","data":{"version":2,"goal":{"text":"do the thing"}},"id":"a3","parentId":"a2","timestamp":"2026-08-14T02:24:01.000Z"}"#;
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, spawn, hidden, blob],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        let texts: Vec<&str> = adapted.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts
            .iter()
            .any(|text| text.contains("spawned @explore-terminal")));
        assert!(!texts.iter().any(|text| text.contains("not for humans")));
        assert!(!texts.iter().any(|text| text.contains("do the thing")));
    }

    #[test]
    fn a_nested_transcript_carries_its_depth_and_root_onto_every_message() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            "2026-08-17T03-19-57-645Z_01a00dbb.jsonl",
            &[SESSION, USER, ASSISTANT],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::nested(2),
            Some("pi:/pi/proj/root.jsonl".to_string()), None,);
        let session = adapted.session.unwrap();
        assert_eq!(session.lineage_depth, LineageDepth::nested(2));
        assert_eq!(session.root_session_key, "pi:/pi/proj/root.jsonl");
        assert!(adapted.messages.iter().all(|message| {
            message.lineage_depth == LineageDepth::nested(2)
                && message.root_session_key == "pi:/pi/proj/root.jsonl"
        }));
    }

    #[test]
    fn a_transcript_without_a_session_record_reports_an_issue() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[USER],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        assert!(adapted.session.is_none());
        assert_eq!(adapted.issues[0].code, ISSUE_NO_SESSION_HEADER);
    }

    /// pi is the vendor where redaction matters most: tool results are 60.6% of
    /// its records.
    #[test]
    fn credentials_in_tool_results_are_masked() {
        let temp = tempfile::tempdir().unwrap();
        let leaky = r#"{"type":"message","id":"m9","parentId":"m1","timestamp":"2026-08-14T02:14:00.000Z","message":{"role":"toolResult","toolCallId":"c9","toolName":"bash","content":[{"type":"text","text":"GITHUB_TOKEN=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]}}"#;
        let path = write(
            temp.path(),
            "2026-08-14T02-12-05-826Z_019ffe0a.jsonl",
            &[SESSION, USER, leaky],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            LineageDepth::ROOT,
            None,
            None,
        );
        let result = adapted
            .messages
            .iter()
            .find(|m| m.role == NormalizedRole::ToolResult)
            .unwrap();
        assert!(!result.text.contains("ghp_AAAA"), "{}", result.text);
        assert!(result.text.contains("[redacted:"), "{}", result.text);
    }
}
