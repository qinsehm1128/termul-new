//! Scan records come from filenames. Session ids are read later from the
//! first JSONL record only — transcript bodies are never consumed.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::SystemTime;

use serde_json::Value;

use super::paths::{is_under_dir, roots_for_agent};
use super::types::{
    normalize_session_id, session_list_id, CliSessionAgentId, DiscoveredCliSession, SCHEMA_VERSION,
};
use super::walk::WalkedFile;

pub fn scanned_session(
    agent: CliSessionAgentId,
    file: &WalkedFile,
    cwd: Option<String>,
    codex_home: Option<String>,
) -> DiscoveredCliSession {
    let file_path = file.path.to_string_lossy().to_string();
    let updated_at = system_time_rfc3339(file.modified);
    let resume_file_path = match agent {
        CliSessionAgentId::Pi => Some(file_path.clone()),
        _ => None,
    };
    DiscoveredCliSession {
        schema_version: SCHEMA_VERSION,
        id: session_list_id(agent, &file_path),
        agent_id: agent,
        title: String::new(),
        session_id: String::new(),
        cwd,
        created_at: updated_at.clone(),
        updated_at,
        message_count: 0,
        file_path,
        codex_home,
        resume_file_path,
        resumable: false,
    }
}

pub fn is_allowed_transcript_path(agent: CliSessionAgentId, path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return false;
        }
    }
    roots_for_agent(agent)
        .iter()
        .any(|root| is_under_dir(path, root))
}

pub fn hydrate_session(
    agent: CliSessionAgentId,
    path: &Path,
    cwd: Option<String>,
    codex_home: Option<String>,
) -> Option<DiscoveredCliSession> {
    let (session_id, first_sentence) = first_session_meta(agent, path);
    let session_id = session_id?;
    let file_path = path.to_string_lossy().to_string();
    let updated_at = file_mtime_rfc3339(path);
    let resume_file_path = match agent {
        CliSessionAgentId::Pi => Some(file_path.clone()),
        _ => None,
    };
    let title = first_sentence.unwrap_or_default();
    Some(DiscoveredCliSession {
        schema_version: SCHEMA_VERSION,
        id: session_list_id(agent, &file_path),
        agent_id: agent,
        title,
        session_id,
        cwd,
        created_at: updated_at.clone(),
        updated_at,
        message_count: 0,
        file_path,
        codex_home,
        resume_file_path,
        resumable: true,
    })
}

fn first_session_meta(agent: CliSessionAgentId, path: &Path) -> (Option<String>, Option<String>) {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if ext.eq_ignore_ascii_case("json") {
        return first_session_meta_from_json(path);
    }
    first_session_meta_from_jsonl(agent, path)
}

fn first_session_meta_from_jsonl(
    agent: CliSessionAgentId,
    path: &Path,
) -> (Option<String>, Option<String>) {
    const MAX_LINES: usize = 80;
    const MAX_BYTES: usize = 128 * 1024;
    let Ok(file) = fs::File::open(path) else {
        return (None, None);
    };
    let mut bytes = 0usize;
    let mut session_id = None;
    let mut user_title = None;
    let mut assistant_title = None;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        if index >= MAX_LINES || bytes >= MAX_BYTES {
            break;
        }
        let Ok(line) = line else {
            continue;
        };
        bytes = bytes.saturating_add(line.len());
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if session_id.is_none() {
            session_id = session_id_from_record(agent, &value);
        }
        if user_title.is_none() {
            user_title = first_user_text(&value).map(|text| title_from_text(&text));
        }
        if assistant_title.is_none() {
            assistant_title = first_assistant_text(&value).map(|text| title_from_text(&text));
        }
        if session_id.is_some() && user_title.is_some() {
            break;
        }
    }
    (session_id, user_title.or(assistant_title))
}

fn first_session_meta_from_json(path: &Path) -> (Option<String>, Option<String>) {
    const MAX_BYTES: usize = 64 * 1024;
    let Ok(file) = fs::File::open(path) else {
        return (None, None);
    };
    let mut buf = Vec::new();
    if file.take(MAX_BYTES as u64).read_to_end(&mut buf).is_err() {
        return (None, None);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&buf) else {
        return (None, None);
    };
    (
        session_id_from_record(CliSessionAgentId::Opencode, &value),
        first_user_text(&value)
            .or_else(|| first_assistant_text(&value))
            .map(|text| title_from_text(&text)),
    )
}

fn session_id_from_record(agent: CliSessionAgentId, value: &Value) -> Option<String> {
    let payload = value.get("payload").unwrap_or(value);
    let raw = match agent {
        CliSessionAgentId::Codex => string_field(payload, &["id", "session_id", "sessionId"])
            .or_else(|| string_field(value, &["sessionId", "session_id", "id"])),
        CliSessionAgentId::Pi => string_field(value, &["id", "sessionId", "session_id"])
            .or_else(|| string_field(payload, &["id", "sessionId", "session_id"])),
        _ => string_field(value, &["sessionId", "session_id", "id"])
            .or_else(|| string_field(payload, &["sessionId", "session_id", "id"])),
    };
    raw.and_then(|id| normalize_session_id(&id))
}

fn first_user_text(value: &Value) -> Option<String> {
    first_role_text(value, "user", Some("user"))
}

fn first_assistant_text(value: &Value) -> Option<String> {
    first_role_text(value, "assistant", None)
}

fn first_role_text(value: &Value, role: &str, type_alias: Option<&str>) -> Option<String> {
    let message = value.get("message").unwrap_or(value);
    let payload = value.get("payload").unwrap_or(value);
    for candidate in [value, message, payload] {
        if !looks_like_role_record(value, candidate, role, type_alias) {
            continue;
        }
        if let Some(text) = extract_text_content(candidate.get("content").unwrap_or(candidate)) {
            return Some(text);
        }
    }
    None
}

fn looks_like_role_record(
    root: &Value,
    candidate: &Value,
    role: &str,
    type_alias: Option<&str>,
) -> bool {
    let found_role = candidate
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| root.get("role").and_then(Value::as_str));
    if found_role == Some(role) {
        return true;
    }
    type_alias.is_some_and(|alias| root.get("type").and_then(Value::as_str) == Some(alias))
}

fn extract_text_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str().and_then(non_empty_text) {
        return Some(text);
    }
    let items = content.as_array()?;
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if let Some(text) = item
            .get("text")
            .and_then(Value::as_str)
            .and_then(non_empty_text)
        {
            return Some(text);
        }
    }
    None
}

fn non_empty_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn title_from_text(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(text);
    truncate(line, 80)
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn system_time_rfc3339(modified: SystemTime) -> Option<String> {
    let duration = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    chrono::DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
        .map(|dt| dt.to_rfc3339())
}

fn file_mtime_rfc3339(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    system_time_rfc3339(modified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn walked(path: &str) -> WalkedFile {
        WalkedFile {
            path: PathBuf::from(path),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn scan_does_not_read_jsonl() {
        let session = scanned_session(
            CliSessionAgentId::ClaudeCode,
            &walked("/missing/chat.jsonl"),
            Some("/repo".into()),
            None,
        );
        assert_eq!(session.session_id, "");
        assert!(!session.resumable);
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.id, "claude-code:/missing/chat.jsonl");
    }

    #[test]
    fn hydrate_reads_first_session_id_and_user_sentence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("chat.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"mode","sessionId":"sess-1","cwd":"/first"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"user","role":"user","content":[{{"type":"text","text":"fix the login bug"}}]}}"#
        )
        .unwrap();
        let session = hydrate_session(
            CliSessionAgentId::ClaudeCode,
            &path,
            Some("/repo".into()),
            None,
        )
        .expect("session");
        assert_eq!(session.session_id, "sess-1");
        assert_eq!(session.title, "fix the login bug");
    }

    #[test]
    fn pi_uses_first_record_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pi.jsonl");
        fs::write(
            &path,
            r#"{"type":"session","id":"01a01876-6135-78d2-92e5-1523e69bf9e8"}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"查看一下这个项目之前我像上游提交的pr"}]}}
"#,
        )
        .unwrap();
        let session = hydrate_session(CliSessionAgentId::Pi, &path, None, None).expect("pi");
        assert_eq!(session.session_id, "01a01876-6135-78d2-92e5-1523e69bf9e8");
        assert_eq!(session.title, "查看一下这个项目之前我像上游提交的pr");
    }

    #[test]
    fn codex_uses_first_payload_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"019dca8f-d1cf-7f03-8c23-32f4dc2d737a"}}
{"type":"event_msg","payload":{"id":"later"}}
"#,
        )
        .unwrap();
        let session = hydrate_session(CliSessionAgentId::Codex, &path, None, None).expect("codex");
        assert_eq!(session.session_id, "019dca8f-d1cf-7f03-8c23-32f4dc2d737a");
        assert_eq!(session.title, "");
    }

    #[test]
    fn pi_skips_blank_user_and_uses_assistant_sentence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pi.jsonl");
        fs::write(
            &path,
            r#"{"type":"session","id":"01a017fa-eaff-74f1-9c0a-ca85b660f07c"}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":" "}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"我已就绪。当前工作目录是 `/repo`。\n\n请告诉我你需要做什么。"}]}}
"#,
        )
        .unwrap();
        let session = hydrate_session(CliSessionAgentId::Pi, &path, None, None).expect("pi");
        assert_eq!(session.session_id, "01a017fa-eaff-74f1-9c0a-ca85b660f07c");
        assert_eq!(session.title, "我已就绪。当前工作目录是 `/repo`。");
    }

    #[test]
    fn pi_blank_transcript_keeps_title_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pi.jsonl");
        fs::write(
            &path,
            r#"{"type":"session","id":"01a01802-155c-7f61-890e-3b203f207b0e"}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":" "}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Understood."}]}}
"#,
        )
        .unwrap();
        let session = hydrate_session(CliSessionAgentId::Pi, &path, None, None).expect("pi");
        assert_eq!(session.session_id, "01a01802-155c-7f61-890e-3b203f207b0e");
        assert_eq!(session.title, "");
    }
}
