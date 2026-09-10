//! Codex transcripts.
//!
//! Shape (verified against a 2646-file store): `{timestamp, type, payload}`
//! envelopes. The first record is `session_meta`, whose payload carries `id`,
//! `cwd`, `timestamp` and `source`. Conversation content arrives as
//! `response_item` envelopes whose payload type is `message`, `reasoning`,
//! `function_call` / `function_call_output`, `custom_tool_call` /
//! `custom_tool_call_output`, `web_search_call`, or `tool_search_call` /
//! `tool_search_output`. `event_msg`, `turn_context` and `world_state` are
//! runtime bookkeeping.
//!
//! ## Project ownership
//!
//! Codex is the one vendor with no per-project directory (`cli_session::paths`
//! skips it for exactly that reason), so ownership comes from
//! `session_meta.payload.cwd` — present on all 2646 sessions measured,
//! including all 1844 subagent ones.
//!
//! ## Lineage
//!
//! `payload.source` is either a plain string (`exec`, `cli`, `vscode`,
//! `unknown` — a root session) or an object with a `subagent` member in one of
//! two shapes:
//!
//! | shape | count | depth | parent |
//! |---|---|---|---|
//! | `subagent.thread_spawn` | 462 | `depth` field (1:428, 2:30, 3:4) | `parent_thread_id` |
//! | `subagent.other` = `agent_job:<uuid>` | 1382 | **none** | **none** |
//!
//! The `agent_job` shape is 75% of Codex's subagents and records neither, so its
//! depth is [`LineageDepth::UNKNOWN`]. Defaulting it to `0` would not be an
//! approximation — it would assert "this is a root message" about 1382 sessions
//! that are not.

use std::path::Path;

use serde_json::Value;

use super::super::paths::MemoryVendor;
use super::super::types::{
    FileIdentity, IndexedSession, LineageDepth, NormalizedMessage, NormalizedRole, SessionScope,
    SourcePointer, TimestampConfidence, SCHEMA_VERSION,
};
use super::{
    flatten_text, for_each_record, parse_timestamp, prepare_indexed_text, session_key,
    string_field, title_candidate, AdaptedTranscript, AdapterIssue, ISSUE_NO_SESSION_HEADER,
};

/// What `session_meta` says about a transcript, without reading its body.
///
/// Ingest reads this for every file in the store to decide project ownership
/// and to build the thread-id map, then only adapts the ones in scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexMeta {
    pub session_id: String,
    pub cwd: Option<String>,
    pub thread_depth: Option<u8>,
    pub parent_thread_id: Option<String>,
    /// Present only for the `agent_job` shape; the only sibling signal it has.
    pub agent_job_id: Option<String>,
    pub agent_role: Option<String>,
    pub is_subagent: bool,
    pub started_at_utc: Option<String>,
}

impl CodexMeta {
    /// Depth from the vendor's own record, never from the path.
    #[must_use]
    pub fn lineage_depth(&self) -> LineageDepth {
        match (self.is_subagent, self.thread_depth) {
            (false, _) => LineageDepth::ROOT,
            (true, Some(depth)) => LineageDepth::nested(depth),
            (true, None) => LineageDepth::UNKNOWN,
        }
    }
}

/// Read only the first record of a transcript.
///
/// Cheap by design: the Codex store has 2646 files and none of them are indexed
/// by project, so every build has to ask all of them "are you mine?".
#[must_use]
pub fn read_meta(path: &Path) -> Option<CodexMeta> {
    use std::io::{BufRead, BufReader, Read};
    const MAX_META_BYTES: u64 = 256 * 1024;

    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file.take(MAX_META_BYTES))
        .read_line(&mut line)
        .ok()?;
    let record: Value = serde_json::from_str(line.trim()).ok()?;
    if record.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = record.get("payload")?;
    let session_id = string_field(payload, &["id", "session_id", "sessionId"])?;
    let source = payload.get("source");
    let subagent = source.and_then(|value| value.get("subagent"));
    let spawn = subagent.and_then(|value| value.get("thread_spawn"));
    Some(CodexMeta {
        session_id,
        cwd: string_field(payload, &["cwd"]),
        thread_depth: spawn
            .and_then(|value| value.get("depth"))
            .and_then(Value::as_u64)
            .and_then(|depth| u8::try_from(depth).ok()),
        parent_thread_id: spawn.and_then(|value| string_field(value, &["parent_thread_id"])),
        agent_job_id: subagent
            .and_then(|value| string_field(value, &["other"]))
            .and_then(|raw| raw.strip_prefix("agent_job:").map(str::to_string)),
        agent_role: spawn.and_then(|value| string_field(value, &["agent_role"])),
        is_subagent: subagent.is_some(),
        started_at_utc: string_field(payload, &["timestamp"])
            .or_else(|| string_field(&record, &["timestamp"])),
    })
}

/// Adapt one Codex transcript.
///
/// `resolve_root` maps a thread id to the session key of its transcript, so a
/// `thread_spawn` child at depth 2 or 3 can be walked back to a real root by the
/// caller. An unresolvable parent makes the child its own root rather than a
/// dangling key.
pub fn adapt(
    path: &Path,
    identity: &FileIdentity,
    project_key: &str,
    scope: SessionScope,
    meta: &CodexMeta,
    resolve_root: &dyn Fn(&str) -> Option<String>,
) -> AdaptedTranscript {
    let own_key = session_key(MemoryVendor::Codex, path);
    let file_path = path.to_string_lossy().into_owned();
    let depth = meta.lineage_depth();
    let root_session_key = match (depth.is_root(), meta.parent_thread_id.as_deref()) {
        (true, _) | (false, None) => own_key.clone(),
        (false, Some(parent)) => resolve_root(parent).unwrap_or_else(|| own_key.clone()),
    };

    let mut out = AdaptedTranscript::default();
    let mut messages: Vec<NormalizedMessage> = Vec::new();
    let mut title: Option<String> = None;
    // Fallback when every user record is harness scaffolding, mirroring
    // `cli_session::parse`, which also falls back to the first assistant line.
    let mut assistant_title: Option<String> = None;
    let mut first: Option<(String, i64)> = None;
    let mut last: Option<(String, i64)> = None;
    let mut tool_count: u64 = 0;
    let mut ordinal: u32 = 0;

    let issues = for_each_record(path, |record| {
        let value = &record.value;
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            return;
        }
        let Some(payload) = value.get("payload") else {
            return;
        };
        let Some(emitted) = normalize_item(payload) else {
            return;
        };
        let text = prepare_indexed_text(&emitted.text);
        if text.is_empty() {
            return;
        }
        let stamp = string_field(value, &["timestamp"]).and_then(|raw| parse_timestamp(&raw));
        if let Some(ref stamp) = stamp {
            if first.is_none() {
                first = Some(stamp.clone());
            }
            last = Some(stamp.clone());
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
            source: SourcePointer::for_record(
                identity,
                &file_path,
                record.byte_offset,
                &record.bytes,
            ),
        });
        ordinal += 1;
    });

    if meta.session_id.is_empty() {
        let mut issues = issues;
        issues.push(AdapterIssue::new(
            ISSUE_NO_SESSION_HEADER,
            path,
            "session_meta carried no id",
        ));
        out.issues = issues;
        return out;
    }

    // The session's own start time is authoritative for ordering when the
    // transcript holds no timestamped conversation record yet.
    let started = meta.started_at_utc.as_deref().and_then(parse_timestamp);
    let first = first.or(started);

    out.session = Some(IndexedSession {
        schema_version: SCHEMA_VERSION,
        session_key: own_key,
        vendor: MemoryVendor::Codex.as_str().to_string(),
        vendor_session_id: meta.session_id.clone(),
        root_session_key,
        lineage_depth: depth,
        project_key: project_key.to_string(),
        scope,
        cwd: meta.cwd.clone(),
        // An `agent_role` is worth showing: it says what the subagent was for.
        title: title
            .or(assistant_title)
            .or_else(|| meta.agent_role.as_ref().map(|role| format!("[{role}]"))),
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
    out.issues = issues;
    out
}

struct Emitted {
    role: NormalizedRole,
    text: String,
    tool_name: Option<String>,
    tool_call_id: Option<String>,
}

/// Map one `response_item` payload to a normalized record.
///
/// `developer` and `system` roles fold into [`NormalizedRole::System`]: they are
/// injected instructions, not conversation, and folding them keeps a search for
/// "what did I ask" from matching the permissions preamble in every session.
fn normalize_item(payload: &Value) -> Option<Emitted> {
    let kind = payload.get("type").and_then(Value::as_str)?;
    match kind {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            Some(Emitted {
                role: match role {
                    "user" => NormalizedRole::User,
                    "assistant" => NormalizedRole::Assistant,
                    _ => NormalizedRole::System,
                },
                text: flatten_text(payload.get("content")?),
                tool_name: None,
                tool_call_id: None,
            })
        }
        "function_call" | "custom_tool_call" => Some(Emitted {
            role: NormalizedRole::ToolCall,
            text: argument_text(payload),
            tool_name: string_field(payload, &["name"]),
            tool_call_id: string_field(payload, &["call_id"]),
        }),
        "function_call_output" | "custom_tool_call_output" => Some(Emitted {
            role: NormalizedRole::ToolResult,
            text: payload.get("output").map(flatten_text).unwrap_or_default(),
            tool_name: None,
            tool_call_id: string_field(payload, &["call_id"]),
        }),
        "web_search_call" => Some(Emitted {
            role: NormalizedRole::ToolCall,
            text: payload
                .get("action")
                .map(|action| serde_json::to_string(action).unwrap_or_default())
                .unwrap_or_default(),
            tool_name: Some("web_search".to_string()),
            tool_call_id: string_field(payload, &["call_id"]),
        }),
        "tool_search_call" => Some(Emitted {
            role: NormalizedRole::ToolCall,
            text: argument_text(payload),
            tool_name: Some("tool_search".to_string()),
            tool_call_id: string_field(payload, &["call_id"]),
        }),
        // `reasoning` bodies are encrypted (`encrypted_content`), so there is
        // nothing to index. `tool_search_output` is a tool catalogue, not
        // conversation.
        _ => None,
    }
}

/// Codex writes `arguments` as a JSON string for `function_call` and as an
/// object for `tool_search_call`. Both need to end up searchable.
fn argument_text(payload: &Value) -> String {
    for key in ["arguments", "input"] {
        match payload.get(key) {
            Some(Value::String(text)) => return text.clone(),
            Some(Value::Null) | None => continue,
            Some(value) => return serde_json::to_string(value).unwrap_or_default(),
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const META_ROOT: &str = r#"{"type":"session_meta","timestamp":"2026-09-09T08:41:06.090Z","payload":{"id":"01a08553-f117-79d1-a4a1-864ffe89d21d","cwd":"/repo","timestamp":"2026-09-09T08:41:06.090Z","source":"exec"}}"#;
    const META_SPAWN: &str = r#"{"type":"session_meta","timestamp":"2026-09-09T08:41:06.090Z","payload":{"id":"child-1","cwd":"/repo","timestamp":"2026-09-09T08:41:06.090Z","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-1","depth":2,"agent_role":"explorer"}}}}}"#;
    const META_JOB: &str = r#"{"type":"session_meta","timestamp":"2026-09-09T08:41:06.090Z","payload":{"id":"job-1","cwd":"/repo","timestamp":"2026-09-09T08:41:06.090Z","source":{"subagent":{"other":"agent_job:83157d15-70cf-41bf-ba34-9f0494570595"}}}}"#;

    fn write(lines: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rollout.jsonl");
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        (temp, path)
    }

    fn no_root(_: &str) -> Option<String> {
        None
    }

    /// AC3's Codex half. All three source shapes, with the one that matters
    /// most: `agent_job` has no depth and must stay unknown.
    #[test]
    fn lineage_depth_comes_from_the_source_shape() {
        let (_t, root) = write(&[META_ROOT]);
        assert_eq!(
            read_meta(&root).unwrap().lineage_depth(),
            LineageDepth::ROOT
        );

        let (_t, spawn) = write(&[META_SPAWN]);
        let spawn_meta = read_meta(&spawn).unwrap();
        assert_eq!(spawn_meta.lineage_depth(), LineageDepth::nested(2));
        assert_eq!(spawn_meta.parent_thread_id.as_deref(), Some("parent-1"));
        assert_eq!(spawn_meta.agent_role.as_deref(), Some("explorer"));

        let (_t, job) = write(&[META_JOB]);
        let job_meta = read_meta(&job).unwrap();
        assert_eq!(
            job_meta.lineage_depth(),
            LineageDepth::UNKNOWN,
            "agent_job records no depth; claiming 0 would assert it is a root"
        );
        assert!(!job_meta.lineage_depth().is_root());
        assert_eq!(
            job_meta.agent_job_id.as_deref(),
            Some("83157d15-70cf-41bf-ba34-9f0494570595")
        );
    }

    /// Ownership is uniform across root and subagent sessions: every measured
    /// Codex session carries a cwd, including the ones missing a depth.
    #[test]
    fn every_source_shape_still_reports_a_cwd() {
        for line in [META_ROOT, META_SPAWN, META_JOB] {
            let (_t, path) = write(&[line]);
            assert_eq!(read_meta(&path).unwrap().cwd.as_deref(), Some("/repo"));
        }
    }

    #[test]
    fn a_file_that_does_not_start_with_session_meta_has_no_meta() {
        let (_t, path) = write(&[r#"{"type":"event_msg","payload":{"type":"task_started"}}"#]);
        assert!(read_meta(&path).is_none());
        assert!(read_meta(Path::new("/nonexistent.jsonl")).is_none());
    }

    #[test]
    fn response_items_normalize_to_roles_and_tool_pairs() {
        let (_t, path) = write(&[
            META_ROOT,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:10.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the tests"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:11.000Z","payload":{"type":"reasoning","summary":[],"encrypted_content":"gAAAA"}}"#,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:12.000Z","payload":{"type":"function_call","name":"exec_command","call_id":"call_1","arguments":"{\"cmd\":\"cargo test\"}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:13.000Z","payload":{"type":"function_call_output","call_id":"call_1","output":"test result: ok. 55 passed"}}"#,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:14.000Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"all green"}]}}"#,
            r#"{"type":"event_msg","timestamp":"2026-09-09T08:41:15.000Z","payload":{"type":"task_started"}}"#,
        ]);
        let identity = FileIdentity::read(&path).unwrap();
        let meta = read_meta(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            &meta,
            &no_root,
        );

        let roles: Vec<NormalizedRole> = adapted.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                NormalizedRole::User,
                NormalizedRole::ToolCall,
                NormalizedRole::ToolResult,
                NormalizedRole::Assistant,
            ],
            "reasoning and event_msg must not become messages"
        );
        let call = &adapted.messages[1];
        assert_eq!(call.tool_name.as_deref(), Some("exec_command"));
        assert!(call.text.contains("cargo test"));
        assert_eq!(call.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(
            adapted.messages[2].tool_call_id.as_deref(),
            Some("call_1"),
            "a result must be pairable with its call"
        );

        let session = adapted.session.unwrap();
        assert_eq!(session.tool_count, 1);
        assert_eq!(session.title.as_deref(), Some("run the tests"));
        assert_eq!(
            session.first_message_at_utc.as_deref(),
            Some("2026-09-09T08:41:10.000Z")
        );
        assert_eq!(
            session.last_activity_at_utc.as_deref(),
            Some("2026-09-09T08:41:14.000Z")
        );
    }

    /// The injected permissions preamble arrives as a `developer` message in
    /// every session. Folding it to `system` keeps it out of "what did I ask".
    #[test]
    fn injected_developer_instructions_are_system_not_user() {
        let (_t, path) = write(&[
            META_ROOT,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:10.000Z","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>"}]}}"#,
        ]);
        let identity = FileIdentity::read(&path).unwrap();
        let meta = read_meta(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            &meta,
            &no_root,
        );
        assert_eq!(adapted.messages[0].role, NormalizedRole::System);
        assert!(
            adapted.session.unwrap().title.is_none(),
            "a preamble must not become the session title"
        );
    }

    /// A depth-2 child walks back to a root the caller resolved.
    #[test]
    fn a_spawned_child_points_at_the_resolved_root() {
        let (_t, path) = write(&[META_SPAWN]);
        let identity = FileIdentity::read(&path).unwrap();
        let meta = read_meta(&path).unwrap();
        let resolve =
            |thread: &str| (thread == "parent-1").then(|| "codex:/store/root.jsonl".to_string());
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            &meta,
            &resolve,
        );
        let session = adapted.session.unwrap();
        assert_eq!(session.root_session_key, "codex:/store/root.jsonl");
        assert_eq!(session.lineage_depth, LineageDepth::nested(2));
        // An empty transcript still orders by the session's own start time.
        assert_eq!(
            session.first_message_at_utc.as_deref(),
            Some("2026-09-09T08:41:06.090Z")
        );
    }

    /// The `agent_job` shape has no parent pointer at all, so it is its own
    /// root — the honest answer, since its real parent is unrecorded.
    #[test]
    fn an_agent_job_child_is_its_own_root_with_unknown_depth() {
        let (_t, path) = write(&[META_JOB]);
        let identity = FileIdentity::read(&path).unwrap();
        let meta = read_meta(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            &meta,
            &no_root,
        );
        let session = adapted.session.unwrap();
        assert_eq!(session.root_session_key, session.session_key);
        assert_eq!(session.lineage_depth, LineageDepth::UNKNOWN);
    }

    #[test]
    fn credentials_in_tool_arguments_are_masked() {
        let (_t, path) = write(&[
            META_ROOT,
            r#"{"type":"response_item","timestamp":"2026-09-09T08:41:12.000Z","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"c1","input":"+  \"XAI_API_KEY\": \"xai-ziyhFXhJ9KTrUDl8Fv8CNNzx4tAAAA\""}}"#,
        ]);
        let identity = FileIdentity::read(&path).unwrap();
        let meta = read_meta(&path).unwrap();
        let adapted = adapt(
            &path,
            &identity,
            "proj",
            SessionScope::Scoped,
            &meta,
            &no_root,
        );
        let text = &adapted.messages[0].text;
        assert!(!text.contains("xai-ziyhFXhJ9KTrUDl8Fv8CNNzx4t"), "{text}");
        assert!(text.contains("[redacted:"), "{text}");
    }
}
