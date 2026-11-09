//! Explicitly managed CLI integrations. Starting the receiver never installs hooks.
pub mod commands;
pub mod install;
mod server;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

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

const CAPACITY: usize = 4096;
type Subscriber = Arc<dyn Fn(&AgentHookEvent) + Send + Sync>;
#[derive(Default)]
pub struct EventStore {
    sequence: u64,
    events: VecDeque<AgentHookEvent>,
    latest: HashMap<String, AgentHookEvent>,
}
impl EventStore {
    fn push(&mut self, mut event: AgentHookEvent) -> AgentHookEvent {
        self.sequence += 1;
        event.sequence = self.sequence;
        if event.transcript_path.is_none() {
            event.transcript_path = self
                .latest
                .get(&event.terminal_id)
                .filter(|old| old.session_id == event.session_id && old.agent == event.agent)
                .and_then(|old| old.transcript_path.clone());
        }
        if self.events.len() >= CAPACITY {
            self.events.pop_front();
        }
        // Bound live metadata as well as history. Eviction simply restores polling.
        if !self.latest.contains_key(&event.terminal_id) && self.latest.len() >= CAPACITY {
            if let Some(key) = self
                .latest
                .values()
                .min_by_key(|e| e.sequence)
                .map(|e| e.terminal_id.clone())
            {
                self.latest.remove(&key);
            }
        }
        self.latest.insert(event.terminal_id.clone(), event.clone());
        self.events.push_back(event.clone());
        event
    }
    pub fn after(&self, after: u64) -> Vec<AgentHookEvent> {
        // A browser can outlive the host. A cursor beyond this launch's head
        // requests a recovery snapshot; the client compares with its requested
        // cursor, resets generation-local authority, then tails from the new head.
        let after = if after > self.sequence { 0 } else { after };
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.sequence > after)
            .cloned()
            .collect();
        if after == 0 {
            let retained: std::collections::HashSet<_> =
                events.iter().map(|e| e.sequence).collect();
            events.extend(
                self.latest
                    .values()
                    .filter(|e| !retained.contains(&e.sequence))
                    .cloned(),
            );
            events.sort_by_key(|e| e.sequence);
        }
        events
    }
    pub fn latest(&self, terminal: &str) -> Option<AgentHookEvent> {
        self.latest.get(terminal).cloned()
    }
}
static STORE: OnceLock<Mutex<EventStore>> = OnceLock::new();
static SUBSCRIBERS: OnceLock<Mutex<Vec<Subscriber>>> = OnceLock::new();
pub fn events(after: u64) -> Vec<AgentHookEvent> {
    STORE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .after(after)
}
pub fn latest(terminal: &str) -> Option<AgentHookEvent> {
    STORE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .latest(terminal)
}
pub fn subscribe(callback: Subscriber) {
    SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(callback);
}
fn publish(event: AgentHookEvent) {
    let event = STORE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(event);
    log::info!(target: "se_manager::agent_hooks", "operation=agent_hook_event agent={} kind={:?} count=1", event.agent.as_str(), event.kind);
    let callbacks = SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone();
    for callback in callbacks {
        callback(&event);
    }
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
    #[test]
    fn stale_launch_cursor_recovers_latest_sessions_even_after_history_eviction() {
        let mut store = EventStore::default();
        let original = normalize(AgentKind::Pi, "quiet-terminal", "p", serde_json::json!({"session_id":"quiet", "type":"session_start", "session_file":"/sessions/quiet.jsonl"})).unwrap();
        store.push(original);
        let active = normalize(
            AgentKind::QinCode,
            "busy-terminal",
            "p",
            serde_json::json!({"session_id":"busy", "event":"turn_start"}),
        )
        .unwrap();
        for _ in 0..CAPACITY {
            store.push(active.clone());
        }
        let recovered = store.after(u64::MAX);
        assert_eq!(recovered.len(), CAPACITY + 1);
        assert_eq!(recovered.first().unwrap().terminal_id, "quiet-terminal");
        assert_eq!(recovered.last().unwrap().sequence, store.sequence);
        assert!(store.after(store.sequence).is_empty());
        let mut next = normalize(
            AgentKind::Pi,
            "quiet-terminal",
            "p",
            serde_json::json!({"session_id":"quiet", "type":"agent_end"}),
        )
        .unwrap();
        next.transcript_path = None;
        assert_eq!(
            store.push(next).transcript_path.as_deref(),
            Some("/sessions/quiet.jsonl")
        );
    }

    #[test]
    fn store_is_bounded_and_cursors_are_monotonic() {
        let mut store = EventStore::default();
        let event = normalize(
            AgentKind::Pi,
            "t",
            "p",
            serde_json::json!({"session_id":"s", "type":"session_start"}),
        )
        .unwrap();
        for _ in 0..CAPACITY + 5 {
            store.push(event.clone());
        }
        assert_eq!(store.after(0).len(), CAPACITY);
        assert_eq!(store.after(CAPACITY as u64).len(), 5);
        assert_eq!(store.latest("t").unwrap().sequence, (CAPACITY + 5) as u64);
    }
}
