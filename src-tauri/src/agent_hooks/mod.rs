//! Explicitly managed CLI integrations. Starting the receiver never installs hooks.
pub mod adapter;
pub mod commands;
pub mod contract;
pub mod install;
pub mod resources;
pub(crate) mod server;
pub mod state;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use server::start;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Pi,
    QinCode,
}
impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Pi => "pi",
            Self::QinCode => "qin-code",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    SessionStart,
    SessionEnd,
    PromptSubmitted,
    ToolStart,
    ToolEnd,
    NeedsApproval,
    Blocked,
    Idle,
    TurnDone,
    Error,
}

/// Metadata only: prompt/tool payloads are intentionally never retained.
/// `sequence` is the cursor for a future transcript tail consumer (C1).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AgentHookEvent {
    pub sequence: u64,
    pub terminal_id: String,
    pub project_id: String,
    pub agent: AgentKind,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    pub kind: HookKind,
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// Compatibility projection for process detection. Authority lives in state.rs,
/// never in a raw-event history. Tools cannot change this projection's cursor.
fn legacy_event(value: contract::HookStateDto) -> Option<AgentHookEvent> {
    let agent = match value.identity.adapter_id.as_str() {
        "claude-code" => AgentKind::ClaudeCode,
        "pi" => AgentKind::Pi,
        "qin-code" => AgentKind::QinCode,
        _ => return None,
    };
    let kind = match value.state {
        contract::DerivedState::Busy => HookKind::PromptSubmitted,
        contract::DerivedState::Idle => HookKind::Idle,
        contract::DerivedState::NeedsApproval => HookKind::NeedsApproval,
        contract::DerivedState::Blocked => HookKind::Blocked,
        contract::DerivedState::Error => HookKind::Error,
        contract::DerivedState::Ended => HookKind::SessionEnd,
    };
    Some(AgentHookEvent {
        sequence: value.revision,
        terminal_id: value.terminal_id,
        project_id: value.project_id,
        agent,
        session_id: value.agent_session_id,
        transcript_path: value.identity.transcript_path,
        kind,
        at: 0,
        tool_name: None,
        ok: None,
    })
}
#[deprecated(note = "use state::sync; this is a latest-state compatibility projection")]
pub fn events(after: u64) -> Vec<AgentHookEvent> {
    let response = state::sync(&contract::HookStateSyncRequest::default(), &|_| true);
    let states = match response {
        contract::HookStateSyncResponse::Snapshot { states, .. }
        | contract::HookStateSyncResponse::Delta { states, .. } => states,
    };
    states
        .into_iter()
        .filter(|s| after == u64::MAX || s.revision > after)
        .filter_map(legacy_event)
        .collect()
}
pub fn latest(terminal: &str) -> Option<AgentHookEvent> {
    state::latest(terminal).and_then(legacy_event)
}
#[deprecated(note = "use state::subscribe; notifications are derived-state changes only")]
pub fn subscribe(callback: Arc<dyn Fn(&AgentHookEvent) + Send + Sync>) {
    state::subscribe(Arc::new(move |response| {
        let states = match response {
            contract::HookStateSyncResponse::Snapshot { states, .. }
            | contract::HookStateSyncResponse::Delta { states, .. } => states,
        };
        for value in states {
            if let Some(event) = legacy_event(value.clone()) {
                callback(&event);
            }
        }
    }));
}

fn text<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Each agent sends its native lifecycle shape; unknown events fail closed.
fn normalize(
    agent: AgentKind,
    terminal: &str,
    project: &str,
    raw: serde_json::Value,
) -> Result<AgentHookEvent, &'static str> {
    if !identifier(terminal) || !identifier(project) {
        return Err("invalid_identity");
    }
    let session = text(&raw, &["session_id", "sessionId"])
        .or_else(|| raw.get("session").and_then(|v| text(v, &["id"])))
        .ok_or("missing_session")?;
    if !identifier(session) {
        return Err("invalid_session");
    }
    let event = text(&raw, &["hook_event_name", "event", "type"]).ok_or("missing_event")?;
    let kind = match (agent, event) {
        (_, "SessionStart" | "session_start") => HookKind::SessionStart,
        (_, "SessionEnd" | "session_end" | "session_shutdown") => HookKind::SessionEnd,
        (_, "UserPromptSubmit" | "turn_start" | "agent_start" | "prompt_submitted") => {
            HookKind::PromptSubmitted
        }
        (_, "PreToolUse" | "tool_execution_start" | "pre_tool" | "tool_call" | "tool_start") => {
            HookKind::ToolStart
        }
        (_, "PostToolUse" | "tool_execution_end" | "post_tool" | "tool_end") => HookKind::ToolEnd,
        (_, "PermissionRequest" | "tool_approval_requested" | "needs_approval") => {
            HookKind::NeedsApproval
        }
        (_, "ui_prompt_start" | "blocked") => HookKind::Blocked,
        (_, "Stop" | "turn_end" | "agent_end" | "turn_done") => HookKind::TurnDone,
        (_, "ui_prompt_end" | "tool_approval_resolved" | "idle") => HookKind::Idle,
        (_, "PostToolUseFailure" | "StopFailure" | "error") => HookKind::Error,
        (AgentKind::ClaudeCode, "Notification") => match text(&raw, &["notification_type"]) {
            Some("permission_prompt") => HookKind::NeedsApproval,
            Some("idle_prompt") => HookKind::Idle,
            _ => return Err("unsupported_notification"),
        },
        _ => return Err("unsupported_event"),
    };
    let bounded =
        |v: &str| (!v.chars().any(char::is_control) && v.len() <= 4096).then(|| v.to_owned());
    Ok(AgentHookEvent {
        sequence: 0,
        terminal_id: terminal.to_owned(),
        project_id: project.to_owned(),
        agent,
        session_id: session.to_owned(),
        transcript_path: text(&raw, &["transcript_path", "session_file"]).and_then(bounded),
        kind,
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        tool_name: text(&raw, &["tool_name", "toolName"])
            .filter(|s| s.len() <= 128)
            .and_then(bounded),
        ok: raw
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .or_else(|| {
                raw.get("isError")
                    .and_then(serde_json::Value::as_bool)
                    .map(|v| !v)
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_native_agent_fixtures_and_drops_content() {
        for (agent, raw, kind) in [
            (
                AgentKind::ClaudeCode,
                r#"{"hook_event_name":"SessionStart","session_id":"claude-1","transcript_path":"/tmp/session.jsonl","prompt":"SECRET"}"#,
                HookKind::SessionStart,
            ),
            (
                AgentKind::Pi,
                r#"{"type":"tool_execution_end","session_id":"pi-1","toolName":"read","isError":false}"#,
                HookKind::ToolEnd,
            ),
            (
                AgentKind::QinCode,
                r#"{"event":"turn_end","session_id":"qin-1","payload":{"prompt":"SECRET"}}"#,
                HookKind::TurnDone,
            ),
        ] {
            let event = normalize(
                agent,
                "terminal",
                "project",
                serde_json::from_str(raw).unwrap(),
            )
            .unwrap();
            assert_eq!(event.kind, kind);
            assert!(!serde_json::to_string(&event).unwrap().contains("SECRET"));
        }
    }
    #[test]
    fn normalizes_lifecycle_and_approval_without_guessing() {
        for (name, kind) in [
            ("UserPromptSubmit", HookKind::PromptSubmitted),
            ("PreToolUse", HookKind::ToolStart),
            ("PostToolUse", HookKind::ToolEnd),
            ("PermissionRequest", HookKind::NeedsApproval),
            ("Stop", HookKind::TurnDone),
            ("SessionEnd", HookKind::SessionEnd),
            ("PostToolUseFailure", HookKind::Error),
        ] {
            assert_eq!(
                normalize(
                    AgentKind::ClaudeCode,
                    "t",
                    "p",
                    serde_json::json!({"session_id":"s", "hook_event_name":name})
                )
                .unwrap()
                .kind,
                kind
            );
        }
        assert!(normalize(AgentKind::Pi, "", "p", serde_json::json!({})).is_err());
        assert!(normalize(
            AgentKind::Pi,
            "t",
            "p",
            serde_json::json!({"session_id":"s", "type":"unknown"})
        )
        .is_err());
    }
}
