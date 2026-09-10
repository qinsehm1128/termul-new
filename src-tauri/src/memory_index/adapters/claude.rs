//! Claude Code transcripts.
//!
//! Shape (verified against this project's own store): newline-delimited records
//! of `{type, uuid, parentUuid, sessionId, cwd, timestamp, isSidechain,
//! message: {role, content}}`. Content is a block list whose members are
//! `text`, `thinking`, `tool_use {id, name, input}` and
//! `tool_result {tool_use_id, content, is_error}`.
//!
//! ## Lineage
//!
//! From `isSidechain`, and from nothing else. A subagent transcript lives at
//! `<session>/subagents/<agent>.jsonl` — but also at
//! `<session>/subagents/workflows/wf_<id>/<agent>.jsonl`, where
//! `workflows/wf_<id>` is a *grouping* directory, not a hierarchy level. On the
//! measured machine that second shape is 122 files; deriving depth from path
//! segments labels every one of them depth 2 when they are depth 1. There is no
//! `subagents/subagents` nesting, so the boolean is complete information.
//!
//! Each subagent record's `sessionId` is its **parent** session's id, which is
//! what lets a flattened child be put back with the conversation it came from.

use std::path::Path;

use serde_json::Value;

use super::super::paths::MemoryVendor;
use super::super::types::{
    CompactionRecord, FileIdentity, IndexedSession, LineageDepth, NormalizedMessage,
    NormalizedRole, SessionScope, SourcePointer, TimestampConfidence, SCHEMA_VERSION,
};
use super::{
    flatten_text, for_each_record, parse_timestamp, prepare_indexed_text, session_key,
    string_field, title_from_text, AdaptedTranscript, AdapterIssue, ISSUE_NO_SESSION_HEADER,
};

/// Adapt one Claude transcript.
///
/// `resolve_root` maps a parent session id to the session key of its top-level
/// transcript, so a flattened subagent points at a real root. When the parent is
/// not in the scanned set the child becomes its own root — an honest fallback,
/// since claiming a root that was never indexed would produce a dangling key.
pub fn adapt(
    path: &Path,
    identity: &FileIdentity,
    project_key: &str,
    scope: SessionScope,
    resolve_root: &dyn Fn(&str) -> Option<String>,
) -> AdaptedTranscript {
    let own_key = session_key(MemoryVendor::ClaudeCode, path);
    let file_path = path.to_string_lossy().into_owned();
    let mut out = AdaptedTranscript::default();

    let mut vendor_session_id: Option<String> = None;
    let mut is_sidechain: Option<bool> = None;
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    let mut first: Option<(String, i64)> = None;
    let mut last: Option<(String, i64)> = None;
    let mut messages: Vec<NormalizedMessage> = Vec::new();
    let mut tool_count: u64 = 0;
    let mut ordinal: u32 = 0;

    let mut issues = for_each_record(path, |record| {
        let value = &record.value;
        // Metadata that every record carries; the first sighting wins so a
        // mid-transcript `cd` cannot retroactively move the session.
        if vendor_session_id.is_none() {
            vendor_session_id = string_field(value, &["sessionId", "session_id"]);
        }
        if is_sidechain.is_none() {
            is_sidechain = value.get("isSidechain").and_then(Value::as_bool);
        }
        if cwd.is_none() {
            cwd = string_field(value, &["cwd"]);
        }

        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        // Everything else in the file is UI bookkeeping: last-prompt, mode,
        // permission-mode, attachment, ai-title, queue-operation,
        // file-history-snapshot / -delta, and `system` hook records.
        if kind != "user" && kind != "assistant" {
            return;
        }
        let Some(message) = value.get("message") else {
            return;
        };
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or(kind);
        let stamp = string_field(value, &["timestamp"]).and_then(|raw| parse_timestamp(&raw));
        if let Some(ref stamp) = stamp {
            if first.is_none() {
                first = Some(stamp.clone());
            }
            last = Some(stamp.clone());
        }

        let pointer = || {
            SourcePointer::for_record(identity, &file_path, record.byte_offset, &record.bytes)
        };
        let depth = LineageDepth::UNKNOWN; // replaced below, once isSidechain is known

        for emitted in split_blocks(message, role) {
            let text = prepare_indexed_text(&emitted.text);
            if text.is_empty() {
                continue;
            }
            if matches!(emitted.role, NormalizedRole::ToolCall) {
                tool_count += 1;
            }
            if title.is_none() && matches!(emitted.role, NormalizedRole::User) {
                title = title_from_text(&text);
            }
            messages.push(NormalizedMessage {
                schema_version: SCHEMA_VERSION,
                message_key: format!("{own_key}#{ordinal}"),
                session_key: own_key.clone(),
                root_session_key: own_key.clone(),
                lineage_depth: depth,
                ordinal,
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
                source: pointer(),
            });
            ordinal += 1;
        }
    });

    let Some(vendor_session_id) = vendor_session_id else {
        issues.push(AdapterIssue::new(
            ISSUE_NO_SESSION_HEADER,
            path,
            "no sessionId in any record",
        ));
        out.issues = issues;
        return out;
    };

    // `isSidechain` absent entirely means no user/assistant record was read —
    // an empty or bookkeeping-only file. Unknown, not root.
    let depth = match is_sidechain {
        Some(false) => LineageDepth::ROOT,
        Some(true) => LineageDepth::nested(1),
        None => LineageDepth::UNKNOWN,
    };
    // A sidechain's `sessionId` is its parent's id.
    let root_session_key = if depth.is_root() {
        own_key.clone()
    } else {
        resolve_root(&vendor_session_id).unwrap_or_else(|| own_key.clone())
    };

    for message in &mut messages {
        message.lineage_depth = depth;
        message.root_session_key = root_session_key.clone();
    }

    out.session = Some(IndexedSession {
        schema_version: SCHEMA_VERSION,
        session_key: own_key,
        vendor: MemoryVendor::ClaudeCode.as_str().to_string(),
        vendor_session_id,
        root_session_key,
        lineage_depth: depth,
        project_key: project_key.to_string(),
        scope,
        cwd,
        title,
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
        source: SourcePointer::for_record(identity, &path.to_string_lossy(), 0, &[]),
    });
    out.messages = messages;
    out.compactions = Vec::<CompactionRecord>::new();
    out.issues = issues;
    out
}

/// One normalized record extracted from a content block.
struct Emitted {
    role: NormalizedRole,
    text: String,
    tool_name: Option<String>,
    tool_call_id: Option<String>,
}

/// Split one Claude message into normalized records.
///
/// A single assistant message routinely holds prose and several `tool_use`
/// blocks. Emitting them separately is what lets a search for an error string
/// hit the tool result without also matching every invocation that produced one.
fn split_blocks(message: &Value, role: &str) -> Vec<Emitted> {
    let content = message.get("content");
    let base_role = match role {
        "user" => NormalizedRole::User,
        "assistant" => NormalizedRole::Assistant,
        _ => NormalizedRole::System,
    };

    let Some(blocks) = content.and_then(Value::as_array) else {
        // A plain string body.
        let text = content.map(flatten_text).unwrap_or_default();
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
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                let text = flatten_text(block);
                if !text.is_empty() {
                    if !prose.is_empty() {
                        prose.push('\n');
                    }
                    prose.push_str(&text);
                }
            }
            "tool_use" => out.push(Emitted {
                role: NormalizedRole::ToolCall,
                // The arguments are the searchable part of a call: the command
                // that ran, the file that was read, the pattern that matched.
                text: describe_tool_input(block.get("input")),
                tool_name: string_field(block, &["name"]),
                tool_call_id: string_field(block, &["id"]),
            }),
            "tool_result" => out.push(Emitted {
                role: NormalizedRole::ToolResult,
                text: flatten_text(block.get("content").unwrap_or(&Value::Null)),
                tool_name: None,
                tool_call_id: string_field(block, &["tool_use_id"]),
            }),
            // `thinking` bodies ship empty in every record on the measured
            // machine; `image` bodies are base64 and 42% of the payload.
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

/// Render a tool's input object as searchable text.
///
/// Compact JSON rather than a pretty tree: the point is that a later search for
/// a command, path or flag finds the call that used it.
fn describe_tool_input(input: Option<&Value>) -> String {
    match input {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_index::types::PointerFreshness;

    fn write(path: &Path, lines: &[&str]) {
        std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn no_root(_: &str) -> Option<String> {
        None
    }

    fn adapt_lines(lines: &[&str]) -> (AdaptedTranscript, std::path::PathBuf, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chat.jsonl");
        write(&path, lines);
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(&path, &identity, "proj", SessionScope::Scoped, &no_root);
        (adapted, path, temp)
    }

    const ROOT_USER: &str = r#"{"type":"user","sessionId":"sess-root","cwd":"/repo","isSidechain":false,"timestamp":"2026-09-01T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"fix the login redirect"}]}}"#;
    const ROOT_ASSISTANT: &str = r#"{"type":"assistant","sessionId":"sess-root","isSidechain":false,"timestamp":"2026-09-01T00:01:00.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"text","text":"looking at the router"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"rg redirect src/"}}]}}"#;
    const ROOT_RESULT: &str = r#"{"type":"user","sessionId":"sess-root","isSidechain":false,"timestamp":"2026-09-01T00:02:00.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"src/router.ts:42","is_error":false}]}}"#;

    /// AC3 + AC4 for Claude: roles, tool pairing, native first-message time.
    #[test]
    fn a_root_transcript_normalizes_roles_tools_and_first_message_time() {
        let (adapted, _path, _temp) = adapt_lines(&[ROOT_USER, ROOT_ASSISTANT, ROOT_RESULT]);
        let session = adapted.session.expect("session");
        assert_eq!(session.vendor_session_id, "sess-root");
        assert_eq!(session.lineage_depth, LineageDepth::ROOT);
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.title.as_deref(), Some("fix the login redirect"));
        assert_eq!(
            session.first_message_at_utc.as_deref(),
            Some("2026-09-01T00:00:00.000Z")
        );
        assert_eq!(
            session.last_activity_at_utc.as_deref(),
            Some("2026-09-01T00:02:00.000Z")
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
        let call = &adapted.messages[2];
        assert_eq!(call.tool_name.as_deref(), Some("Bash"));
        assert!(call.text.contains("rg redirect src/"));
        let result = &adapted.messages[3];
        assert_eq!(
            result.tool_call_id.as_deref(),
            call.tool_call_id.as_deref(),
            "a result must be pairable with its call"
        );
        assert_eq!(result.text, "src/router.ts:42");
        assert!(adapted.issues.is_empty());
    }

    /// The prose and the tool call come from the same record, so both pointers
    /// must locate that record.
    #[test]
    fn every_message_pointer_verifies_against_the_file() {
        let (adapted, _path, _temp) = adapt_lines(&[ROOT_USER, ROOT_ASSISTANT, ROOT_RESULT]);
        for message in &adapted.messages {
            assert_eq!(
                message.source.verify(),
                PointerFreshness::Fresh,
                "pointer for ordinal {} does not locate its record",
                message.ordinal
            );
        }
    }

    /// AC3's Claude half: depth comes from `isSidechain`.
    #[test]
    fn a_sidechain_transcript_is_depth_one_and_points_at_its_parent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-x.jsonl");
        write(
            &path,
            &[r#"{"type":"user","sessionId":"sess-root","agentId":"aw5-ts","isSidechain":true,"timestamp":"2026-09-01T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"explore the store"}]}}"#],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let resolve = |parent: &str| {
            (parent == "sess-root").then(|| "claude-code:/store/root.jsonl".to_string())
        };
        let adapted = adapt(&path, &identity, "proj", SessionScope::Scoped, &resolve);
        let session = adapted.session.unwrap();
        assert_eq!(session.lineage_depth, LineageDepth::nested(1));
        assert!(!session.lineage_depth.is_root());
        assert_eq!(session.root_session_key, "claude-code:/store/root.jsonl");
        assert!(adapted
            .messages
            .iter()
            .all(|m| m.root_session_key == "claude-code:/store/root.jsonl"));
    }

    /// The grouping-directory trap. A `workflows/wf_*` path is two segments
    /// deeper but still depth 1, and the adapter must not look at the path at
    /// all to decide that.
    #[test]
    fn a_grouped_workflow_subagent_is_still_depth_one() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp
            .path()
            .join("subagents")
            .join("workflows")
            .join("wf_abc");
        std::fs::create_dir_all(&nested).unwrap();
        let path = nested.join("agent-y.jsonl");
        write(
            &path,
            &[r#"{"type":"user","sessionId":"sess-root","isSidechain":true,"timestamp":"2026-09-01T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"grouped agent"}]}}"#],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(&path, &identity, "proj", SessionScope::Scoped, &no_root);
        assert_eq!(
            adapted.session.unwrap().lineage_depth,
            LineageDepth::nested(1),
            "workflows/wf_<id> is a grouping directory, not a hierarchy level"
        );
    }

    /// An unresolvable parent must not produce a dangling root key.
    #[test]
    fn an_unresolved_parent_makes_the_child_its_own_root() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("orphan.jsonl");
        write(
            &path,
            &[r#"{"type":"user","sessionId":"sess-missing","isSidechain":true,"timestamp":"2026-09-01T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"orphan"}]}}"#],
        );
        let identity = FileIdentity::read(&path).unwrap();
        let adapted = adapt(&path, &identity, "proj", SessionScope::Scoped, &no_root);
        let session = adapted.session.unwrap();
        assert_eq!(session.root_session_key, session.session_key);
        assert_eq!(session.lineage_depth, LineageDepth::nested(1));
    }

    /// AC11's honesty rule, applied at the source: a file with no
    /// user/assistant record has an unknown depth, not a root one.
    #[test]
    fn a_bookkeeping_only_transcript_has_unknown_depth() {
        let (adapted, _path, _temp) = adapt_lines(&[
            r#"{"type":"mode","sessionId":"sess-x","mode":"default"}"#,
            r#"{"type":"ai-title","sessionId":"sess-x","title":"whatever"}"#,
        ]);
        let session = adapted.session.expect("a sessionId was present");
        assert_eq!(session.lineage_depth, LineageDepth::UNKNOWN);
        assert!(adapted.messages.is_empty());
        assert_eq!(session.message_count, 0);
    }

    #[test]
    fn bookkeeping_record_types_are_not_indexed_as_messages() {
        let (adapted, _path, _temp) = adapt_lines(&[
            ROOT_USER,
            r#"{"type":"system","sessionId":"sess-root","isSidechain":false,"subtype":"hook","hookCount":1}"#,
            r#"{"type":"attachment","sessionId":"sess-root","message":{"role":"user","content":[{"type":"text","text":"attached noise"}]}}"#,
            r#"{"type":"last-prompt","sessionId":"sess-root","message":{"role":"user","content":[{"type":"text","text":"prompt echo"}]}}"#,
        ]);
        assert_eq!(adapted.messages.len(), 1);
        assert!(!adapted.messages[0].text.contains("attached noise"));
        assert!(!adapted.messages[0].text.contains("prompt echo"));
    }

    #[test]
    fn a_transcript_without_a_session_id_reports_an_issue_and_no_session() {
        let (adapted, _path, _temp) =
            adapt_lines(&[r#"{"type":"user","message":{"role":"user","content":"orphaned"}}"#]);
        assert!(adapted.session.is_none());
        assert_eq!(adapted.issues.len(), 1);
        assert_eq!(adapted.issues[0].code, ISSUE_NO_SESSION_HEADER);
    }

    /// Redaction has to happen inside the adapter, before text is ever handed
    /// to the store.
    #[test]
    fn credentials_in_tool_output_are_masked_by_the_adapter() {
        let (adapted, _path, _temp) = adapt_lines(&[
            ROOT_USER,
            r#"{"type":"user","sessionId":"sess-root","isSidechain":false,"timestamp":"2026-09-01T00:02:00.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_9","content":"ANTHROPIC_API_KEY=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA"}]}}"#,
        ]);
        let result = adapted
            .messages
            .iter()
            .find(|m| m.role == NormalizedRole::ToolResult)
            .expect("tool result");
        assert!(!result.text.contains("sk-ant-api03"), "{}", result.text);
        assert!(result.text.contains("[redacted:"), "{}", result.text);
    }

    /// A mid-session `cd` must not move the session: the first cwd wins.
    #[test]
    fn the_first_recorded_cwd_wins() {
        let (adapted, _path, _temp) = adapt_lines(&[
            ROOT_USER,
            r#"{"type":"user","sessionId":"sess-root","cwd":"/elsewhere","isSidechain":false,"timestamp":"2026-09-01T00:05:00.000Z","message":{"role":"user","content":[{"type":"text","text":"later"}]}}"#,
        ]);
        assert_eq!(adapted.session.unwrap().cwd.as_deref(), Some("/repo"));
    }
}
