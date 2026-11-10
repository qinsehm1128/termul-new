//! Hook-backed terminal chat. One host worker owns cursors, the send queue and ack state.
//! No transcript or prompt is persisted in the search index or logged here.
pub mod adapters;
pub mod commands;
mod history;
mod paths;
mod scan;
#[cfg(test)]
pub(crate) mod tests;

use crate::agent_hooks::{self, AgentHookEvent, AgentKind, HookKind};
use crate::core::TerminalServiceHandle;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime},
};

/// Machine-readable chat failure codes shared by the Tauri command, the
/// authenticated web facade and every client. Clients branch on these codes,
/// never on free-text messages.
pub mod codes {
    /// No agent hook session is reporting for this terminal: an expected state.
    pub const NO_HOOK: &str = "AGENT_CHAT_NO_HOOK";
    /// A hook session exists, but its adapter has no transcript reader.
    pub const UNSUPPORTED: &str = "UNSUPPORTED_AGENT_CHAT";
    /// A real transport or transcript failure.
    pub const ERROR: &str = "AGENT_CHAT_ERROR";
}

/// Adapters whose transcripts this module can read. Anything else is a hook
/// session without a chat view, not a failure.
const CHAT_ADAPTERS: [&str; 3] = ["claude-code", "pi", "qin-code"];

/// Typed chat failure. Expected absence carries a stable code instead of an
/// error message so clients can render an empty state; `Failed` keeps the
/// internal detail server-side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatError {
    NoHook,
    UnsupportedAgent,
    Failed(String),
}

impl ChatError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoHook => codes::NO_HOOK,
            Self::UnsupportedAgent => codes::UNSUPPORTED,
            Self::Failed(_) => codes::ERROR,
        }
    }
    /// Client-visible reason. Generic by construction: never a terminal id,
    /// payload, prompt or transcript path.
    pub fn message(&self) -> &'static str {
        match self {
            Self::NoHook => "No agent is reporting a chat session for this terminal.",
            Self::UnsupportedAgent => "This agent session has no chat transcript view.",
            Self::Failed(_) => "Chat is temporarily unavailable.",
        }
    }
    /// Expected absence is a state, not a fault.
    pub fn is_expected(&self) -> bool {
        !matches!(self, Self::Failed(_))
    }
    /// Server-side detail for real failures only; expected states stay silent.
    /// These reasons are static strings, never terminal ids or payloads.
    pub fn log_detail(&self) {
        if let Self::Failed(reason) = self {
            log::debug!("agent chat request failed: {reason}");
        }
    }
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ChatError {}

impl From<String> for ChatError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

const ACK_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_QUEUE: usize = 64;
const MAX_RECEIPTS: usize = 64;
const MAX_PENDING_BYTES: usize = 512 * 1024;
const MAX_SESSIONS: usize = 128;
const MAX_TEXT: usize = 128 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
type Writer = Box<dyn Fn(&str, &str) -> Result<(), String> + Send>;
pub type SendGuard = Arc<dyn Fn() -> bool + Send + Sync>;
type Subscriber = Arc<dyn Fn(&ChatUpdated) + Send + Sync>;
static CURSOR: AtomicU64 = AtomicU64::new(1);
fn next_cursor() -> u64 {
    CURSOR.fetch_add(1, Ordering::Relaxed)
}

static SERVICE: OnceLock<Service> = OnceLock::new();
static SUBSCRIBERS: OnceLock<Mutex<Vec<Subscriber>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct ChatUpdated {
    pub terminal_id: String,
    pub cursor: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub blocks: Vec<Value>,
    pub streaming: bool,
    pub timestamp: u64,
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Delivery {
    pub id: String,
    pub text: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ChatSnapshot {
    pub terminal_id: String,
    pub session_id: String,
    pub agent: AgentKind,
    pub cursor: u64,
    pub messages: Vec<ChatMessage>,
    pub tool_calls: Vec<Value>,
    pub deliveries: Vec<Delivery>,
    pub state: &'static str,
    pub reason: Option<String>,
    pub hook_state: String,
    pub reset: bool,
    pub older_cursor: Option<u64>,
    pub can_force_send: bool,
}
struct Pending {
    delivery: Delivery,
    raw: String,
    sent: Option<Instant>,
    baseline: u64,
    baseline_offset: u64,
    guard: Option<SendGuard>,
}
struct Session {
    hook: AgentHookEvent,
    path: Option<PathBuf>,
    watcher: Option<RecommendedWatcher>,
    stamp: Option<FileStamp>,
    offset: u64,
    cursor: u64,
    reset_cursor: u64,
    last_emitted: Option<u64>,
    history: Option<history::History>,
    anchor: Vec<u8>,
    scan_state: scan::ScanState,
    retired_at: Option<Instant>,
    pending: VecDeque<Pending>,
    reason: Option<String>,
    hook_state: String,
    ready_credit: bool,
    ever_ready: bool,
    known_busy: bool,
}
#[derive(PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: (u64, u64),
}
impl FileStamp {
    fn read(path: &std::path::Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        };
        #[cfg(not(unix))]
        let identity = (0, 0);
        Some(Self {
            len: m.len(),
            modified: m.modified().ok(),
            identity,
        })
    }
}
impl Session {
    fn new(hook: AgentHookEvent) -> Self {
        let ready_credit = matches!(hook.kind, HookKind::Idle | HookKind::TurnDone);
        let known_busy = !matches!(
            hook.kind,
            HookKind::SessionStart | HookKind::Idle | HookKind::TurnDone
        );
        let hook_state = hook_name(hook.kind).to_owned();
        let cursor = next_cursor();
        Self {
            hook,
            path: None,
            watcher: None,
            stamp: None,
            offset: 0,
            cursor,
            reset_cursor: cursor,
            last_emitted: None,
            history: history::History::new().ok(),
            anchor: vec![],
            scan_state: scan::ScanState::default(),
            retired_at: None,
            pending: VecDeque::new(),
            reason: None,
            hook_state,
            ready_credit,
            ever_ready: ready_credit,
            known_busy,
        }
    }
    fn replace(&mut self, hook: AgentHookEvent) {
        self.finish();
        let receipts = std::mem::take(&mut self.pending);
        *self = Session::new(hook);
        self.reset_cursor = self.cursor;
        self.pending = receipts;
    }
    fn apply_hook(&mut self, hook: AgentHookEvent) {
        if self.hook.sequence == hook.sequence {
            return;
        }
        let changed_path = self.hook.transcript_path != hook.transcript_path;
        let was_ready = matches!(self.hook.kind, HookKind::Idle | HookKind::TurnDone);
        let is_ready = matches!(hook.kind, HookKind::Idle | HookKind::TurnDone);
        self.hook_state = hook_name(hook.kind).into();
        if is_ready {
            self.ever_ready = true;
            self.known_busy = false;
        } else if hook.kind != HookKind::SessionStart {
            self.known_busy = true;
        }
        if !is_ready {
            self.ready_credit = false;
        } else if !was_ready {
            self.ready_credit = true;
        }
        self.hook = hook;
        self.cursor = next_cursor();
        if self.hook.kind == HookKind::SessionEnd {
            self.finish();
        }
        if changed_path {
            self.path = None;
            self.stamp = None;
            self.offset = 0;
            self.clear_history();
            self.reason = None;
            self.reset_cursor = self.cursor;
            for item in &mut self.pending {
                if item.delivery.status == "pending" {
                    item.delivery.status = "unconfirmed".into();
                    item.raw = String::new();
                }
            }
        }
    }
    fn degrade(&mut self, reason: &str) {
        if self.reason.as_deref() != Some(reason) {
            tracing::warn!(
                event = "agent_chat_parse_degraded",
                kind = reason,
                count = 1
            );
            self.reason = Some(reason.to_owned());
            self.cursor = next_cursor();
        }
    }
    fn configure_path(&mut self, tx: &mpsc::SyncSender<Request>) {
        let path = paths::resolve(&self.hook);
        if self.path == path && self.watcher.is_some() {
            return;
        }
        self.path = path;
        self.watcher = None;
        let Some(path) = &self.path else {
            self.degrade("transcript_not_found");
            return;
        };
        let tx = tx.clone();
        match notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
            let _ = tx.try_send(Request::Wake);
        }) {
            Ok(mut watcher) => {
                let watched = path.parent().unwrap_or(path.as_path());
                if watcher.watch(watched, RecursiveMode::NonRecursive).is_ok() {
                    self.watcher = Some(watcher);
                } else {
                    self.degrade("watch_unavailable");
                }
            }
            Err(_) => self.degrade("watch_unavailable"),
        }
    }
    fn clear_history(&mut self) {
        self.anchor.clear();
        self.scan_state = scan::ScanState::default();
        if self.history.as_ref().is_some_and(|h| h.clear().is_err()) {
            self.history = None;
        }
    }
    fn finish(&mut self) {
        if self.retired_at.is_none() {
            self.cursor = next_cursor();
        }
        self.ready_credit = false;
        self.hook.kind = HookKind::SessionEnd;
        self.hook_state = "session_end".into();
        self.watcher = None;
        self.retired_at.get_or_insert_with(Instant::now);
        for item in &mut self.pending {
            if matches!(item.delivery.status.as_str(), "pending" | "queued") {
                item.delivery.status = "unconfirmed".into();
                item.raw = String::new();
                item.guard = None;
                self.cursor = next_cursor();
            }
        }
        self.trim_receipts();
    }
    fn reserve_receipt(&mut self, bytes: usize) -> bool {
        while self.pending.len() >= MAX_RECEIPTS
            || self
                .pending
                .iter()
                .map(|p| p.raw.len() + p.delivery.text.len())
                .sum::<usize>()
                + bytes
                > MAX_PENDING_BYTES
        {
            let Some(index) = self
                .pending
                .iter()
                .position(|p| !matches!(p.delivery.status.as_str(), "pending" | "queued"))
            else {
                return false;
            };
            self.pending.remove(index);
        }
        true
    }
    fn trim_receipts(&mut self) {
        while self.pending.len() > MAX_RECEIPTS
            || self
                .pending
                .iter()
                .map(|p| p.raw.len() + p.delivery.text.len())
                .sum::<usize>()
                > MAX_PENDING_BYTES
        {
            let Some(index) = self
                .pending
                .iter()
                .position(|p| !matches!(p.delivery.status.as_str(), "pending" | "queued"))
            else {
                break;
            };
            self.pending.remove(index);
        }
    }
    fn read(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(stamp) = FileStamp::read(&path) else {
            self.degrade("transcript_unreadable");
            return;
        };
        let Ok(mut file) = std::fs::File::open(&path) else {
            self.degrade("transcript_unreadable");
            return;
        };
        if paths::verify_opened(&self.hook, &path, &file).is_err() {
            self.degrade("transcript_unreadable");
            return;
        }
        // Validate bytes just before the previous committed read boundary even if
        // the inode survived a truncate/rewrite and the replacement grew longer.
        let anchor_matches =
            scan::anchor_file(&mut file, self.offset).is_some_and(|a| a == self.anchor);
        if self.stamp.as_ref().is_some_and(|old| {
            old.identity != stamp.identity
                || stamp.len < old.len
                || !anchor_matches
                || (stamp.len == old.len && old.modified != stamp.modified)
        }) {
            self.offset = 0;
            self.clear_history();
            self.reason = None;
            self.cursor = next_cursor();
            self.reset_cursor = self.cursor;
            for item in &mut self.pending {
                if item.delivery.status == "pending" {
                    item.delivery.status = "unconfirmed".into();
                    item.raw = String::new();
                    item.guard = None;
                }
            }
        } else if self.stamp.as_ref() == Some(&stamp) && self.offset >= stamp.len {
            return;
        }
        let mut invalid = false;
        let agent = self.hook.agent;
        // The scanner bounds work per turn, and applies records immediately: no
        // Vec containing the whole append/journal and no linear history dedupe.
        let Ok(scan_file) = file.try_clone() else {
            self.degrade("transcript_unreadable");
            return;
        };
        let mut state = std::mem::take(&mut self.scan_state);
        let scanned = scan::scan_file(scan_file, self.offset, &mut state, |record| {
            match adapters::parse_record(agent, &record.value, record.byte_offset) {
                Ok(parsed) => {
                    for entry in parsed {
                        self.apply_entry(record.byte_offset, entry);
                    }
                }
                Err(_) => invalid = true,
            }
        });
        self.scan_state = state;
        self.offset = scanned.resume_offset;
        self.anchor = scan::anchor_file(&mut file, self.offset).unwrap_or_default();
        self.stamp = Some(stamp);
        if invalid || !scanned.issues.is_empty() {
            self.degrade("invalid_transcript_record");
        } else if matches!(
            self.reason.as_deref(),
            Some("transcript_not_found" | "transcript_unreadable")
        ) {
            self.reason = None;
            self.cursor = next_cursor();
        }
        self.trim_receipts();
    }
    fn apply_entry(&mut self, record_offset: u64, entry: adapters::ParsedEntry) {
        let Some(history) = self.history.as_ref() else {
            self.degrade("history_unavailable");
            return;
        };
        match history.contains(&entry.id) {
            Ok(true) => return,
            Err(_) => {
                self.degrade("history_unavailable");
                return;
            }
            _ => {}
        }
        self.cursor = next_cursor();
        if let Some(text) = &entry.user_text {
            if let Some(item) = self.pending.iter_mut().find(|p| {
                p.delivery.status == "pending"
                    && record_offset >= p.baseline_offset
                    && self.cursor > p.baseline
                    && normalize(&p.raw) == normalize(text)
            }) {
                item.delivery.status = "delivered".into();
                item.raw = String::new();
                item.guard = None;
            }
        }
        for mut tool in entry.tool_calls {
            self.cursor = next_cursor();
            tool["seq"] = self.cursor.into();
            let tool_id = tool["toolCallId"].as_str().unwrap_or("").to_owned();
            let marker_id = format!("se-tool:{}:{}", entry.id, tool_id);
            if history.put_tool(self.cursor, tool).is_err()
                || history
                    .put_message(&ChatMessage {
                        id: marker_id,
                        role: "tool".into(),
                        tool_call_id: Some(tool_id),
                        blocks: vec![],
                        streaming: false,
                        timestamp: now_ms(),
                        seq: self.cursor,
                    })
                    .is_err()
            {
                self.degrade("history_unavailable");
                return;
            }
        }
        self.cursor = next_cursor();
        // Keep even tool-only entries in the indexed sequence so their older
        // values can be reached via the same bounded disk pages.
        if history
            .put_message(&ChatMessage {
                id: entry.id,
                role: entry.role,
                tool_call_id: None,
                blocks: entry.blocks,
                streaming: false,
                timestamp: now_ms(),
                seq: self.cursor,
            })
            .is_err()
        {
            self.degrade("history_unavailable");
        }
    }
    fn expire(&mut self, now: Instant) {
        for item in &mut self.pending {
            if item.delivery.status == "pending"
                && item
                    .sent
                    .is_some_and(|sent| now.saturating_duration_since(sent) >= ACK_TIMEOUT)
            {
                item.delivery.status = "unconfirmed".into();
                item.raw = String::new();
                item.guard = None;
                self.cursor = next_cursor();
                tracing::warn!(
                    event = "agent_chat_ack_timeout",
                    kind = "no_transcript_echo",
                    count = 1
                );
            }
        }
        self.trim_receipts();
    }
    fn flush(&mut self, writer: &Writer) {
        if !self.ready_credit || matches!(self.hook.kind, HookKind::SessionEnd) {
            return;
        }
        if self.pending.iter().any(|p| p.delivery.status == "pending") {
            return;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|p| p.delivery.status == "queued")
        else {
            return;
        };
        let _ = self.write_queued(index, writer, false);
    }
    fn can_force_send(&self) -> bool {
        !self.ever_ready
            && !self.known_busy
            && self.retired_at.is_none()
            && self.hook.kind != HookKind::SessionEnd
            && !self.pending.iter().any(|p| p.delivery.status == "pending")
    }
    fn force_send(
        &mut self,
        delivery_id: &str,
        guard: Option<SendGuard>,
        writer: &Writer,
    ) -> Result<Delivery, String> {
        if !self.can_force_send() {
            return Err("force send unavailable for this session".into());
        }
        if guard.as_ref().is_some_and(|check| !check()) {
            return Err("terminal.input scope denied".into());
        }
        let index = self
            .pending
            .iter()
            .position(|p| p.delivery.id == delivery_id && p.delivery.status == "queued")
            .ok_or_else(|| "queued delivery not found".to_string())?;
        // An explicit request re-authorizes this exact receipt. The client never
        // supplies/reconstructs its raw text and cannot create a second receipt.
        self.pending[index].guard = guard;
        self.write_queued(index, writer, true)
    }
    fn write_queued(
        &mut self,
        index: usize,
        writer: &Writer,
        force: bool,
    ) -> Result<Delivery, String> {
        if self.pending[index]
            .guard
            .as_ref()
            .is_some_and(|guard| !guard())
        {
            let item = &mut self.pending[index];
            item.delivery.status = "unconfirmed".into();
            item.raw = String::new();
            item.guard = None;
            self.cursor = next_cursor();
            return Err("terminal.input scope denied".into());
        }
        self.known_busy = true;
        self.ready_credit = false; // one prompt per idle/turn_done, never flood the TUI
        self.hook_state = "prompt_submitted".into();
        let item = &mut self.pending[index];
        item.baseline = self.cursor;
        item.baseline_offset = self
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(self.offset, |m| m.len());
        item.sent = Some(Instant::now());
        item.delivery.status = "pending".into();
        // Paste and Enter are one atomic runtime write. No retry, including partial failures.
        let data = format!("\x1b[200~{}\x1b[201~\r", item.raw);
        if writer(&self.hook.terminal_id, &data).is_err() {
            item.delivery.status = "unconfirmed".into();
            item.raw = String::new();
            item.guard = None;
        }
        self.cursor = next_cursor();
        if force {
            tracing::info!(event = "agent_chat_force_send", count = 1);
        } else {
            tracing::info!(
                event = "agent_chat_send",
                kind = "bracketed_paste",
                count = 1
            );
        }
        Ok(item.delivery.clone())
    }
    fn enqueue(
        &mut self,
        text: String,
        guard: Option<SendGuard>,
        writer: &Writer,
    ) -> Result<Delivery, String> {
        if text.trim().is_empty()
            || text.len() > MAX_TEXT
            || text
                .chars()
                .any(|c| (c < ' ' && c != '\n' && c != '\t' && c != '\r') || c == '\u{7f}')
        {
            return Err("invalid chat text".into());
        }
        if self.hook.kind == HookKind::SessionEnd {
            return Err("agent session ended".into());
        }
        if self
            .pending
            .iter()
            .filter(|p| matches!(p.delivery.status.as_str(), "pending" | "queued"))
            .count()
            >= MAX_QUEUE
        {
            return Err("chat queue full".into());
        }
        let redacted = crate::memory_index::redact::redact(&text).text;
        if !self.reserve_receipt(text.len() + redacted.len()) {
            return Err("chat queue full".into());
        }
        let delivery = Delivery {
            id: uuid::Uuid::new_v4().to_string(),
            text: redacted,
            status: "queued".into(),
        };
        self.pending.push_back(Pending {
            delivery: delivery.clone(),
            raw: text,
            sent: None,
            baseline: self.cursor,
            baseline_offset: self.offset,
            guard,
        });
        self.cursor = next_cursor();
        if !self.ready_credit {
            tracing::info!(event = "agent_chat_queue", kind = "busy", count = 1);
        }
        self.flush(writer);
        self.trim_receipts();
        Ok(self
            .pending
            .back()
            .expect("just inserted delivery")
            .delivery
            .clone())
    }
    fn snapshot(&self, after: Option<u64>) -> ChatSnapshot {
        self.page(after, None)
    }
    fn page(&self, after: Option<u64>, before: Option<u64>) -> ChatSnapshot {
        // Old-page bounds are sequence values too. A replacement/truncation
        // allocates its fence after every old seq, so stale history requests can
        // be replaced atomically even while clients have live polling paused.
        if before.is_some_and(|bound| bound <= self.reset_cursor || bound > self.cursor) {
            return self.page(None, None);
        }
        let after = after.unwrap_or(0);
        let mut reset =
            before.is_none() && (after == 0 || after > self.cursor || after < self.reset_cursor);
        let page = self.history.as_ref().and_then(|h| {
            h.page(if reset || before.is_some() { 0 } else { after }, before)
                .ok()
        });
        if !reset
            && before.is_none()
            && page
                .as_ref()
                .and_then(|p| p.messages.first())
                .is_some_and(|m| {
                    self.history
                        .as_ref()
                        .is_some_and(|h| h.has_between(after, m.seq).unwrap_or(true))
                })
        {
            // A lagging client overflowed the bounded delta. Explicitly replace
            // with a window, with older_cursor to retrieve the omitted history.
            reset = true;
        }
        ChatSnapshot {
            terminal_id: self.hook.terminal_id.clone(),
            session_id: self.hook.session_id.clone(),
            agent: self.hook.agent,
            cursor: self.cursor,
            older_cursor: page.as_ref().and_then(|p| p.older),
            can_force_send: self.can_force_send(),
            messages: page
                .as_ref()
                .map(|p| p.messages.clone())
                .unwrap_or_default(),
            tool_calls: page.map(|p| p.tools).unwrap_or_default(),
            deliveries: self.pending.iter().map(|p| p.delivery.clone()).collect(),
            state: if self.reason.is_some() || self.history.is_none() {
                "degraded"
            } else {
                "ready"
            },
            reason: self
                .reason
                .clone()
                .or_else(|| self.history.is_none().then(|| "history_unavailable".into())),
            hook_state: self.hook_state.clone(),
            reset,
        }
    }
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_owned()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn hook_name(kind: HookKind) -> &'static str {
    match kind {
        HookKind::SessionStart => "session_start",
        HookKind::SessionEnd => "session_end",
        HookKind::PromptSubmitted => "prompt_submitted",
        HookKind::ToolStart => "tool_start",
        HookKind::ToolEnd => "tool_end",
        HookKind::NeedsApproval => "needs_approval",
        HookKind::Blocked => "blocked",
        HookKind::Idle => "idle",
        HookKind::TurnDone => "turn_done",
        HookKind::Error => "error",
    }
}

/// Transcript readers are a closed builtin set; custom adapter state never selects
/// a parser by fallback or by a user-provided file extension.
fn classify_adapter(adapter: Option<&str>) -> ChatError {
    match adapter {
        Some(id) if !CHAT_ADAPTERS.contains(&id) => ChatError::UnsupportedAgent,
        _ => ChatError::NoHook,
    }
}

fn unsupported_or_missing(terminal: &str) -> ChatError {
    let adapter = agent_hooks::state::latest(terminal).map(|value| value.identity.adapter_id);
    classify_adapter(adapter.as_ref().map(|id| id.as_str()))
}
fn derived_chat_hook(value: agent_hooks::contract::HookStateDto) -> Option<AgentHookEvent> {
    use agent_hooks::contract::DerivedState;
    let agent = match value.identity.adapter_id.as_str() {
        "claude-code" => AgentKind::ClaudeCode,
        "pi" => AgentKind::Pi,
        "qin-code" => AgentKind::QinCode,
        _ => return None,
    };
    let kind = match value.state {
        DerivedState::Busy => HookKind::PromptSubmitted,
        DerivedState::Idle if value.ready => HookKind::Idle,
        DerivedState::Idle => HookKind::SessionStart,
        DerivedState::NeedsApproval => HookKind::NeedsApproval,
        DerivedState::Blocked => HookKind::Blocked,
        DerivedState::Error => HookKind::Error,
        DerivedState::Ended => HookKind::SessionEnd,
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

#[derive(Default)]
pub struct SendOptions {
    pub force: bool,
    pub delivery_id: Option<String>,
}
struct Service {
    tx: mpsc::SyncSender<Request>,
}
enum Request {
    Wake,
    Snapshot(
        String,
        Option<u64>,
        Option<u64>,
        mpsc::SyncSender<Result<ChatSnapshot, ChatError>>,
    ),
    Send(
        String,
        String,
        SendOptions,
        Option<SendGuard>,
        mpsc::SyncSender<Result<Delivery, ChatError>>,
    ),
    Control(
        String,
        String,
        Option<SendGuard>,
        mpsc::SyncSender<Result<(), ChatError>>,
    ),
}

pub fn subscribe(callback: Subscriber) {
    SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(callback);
}
fn emit(terminal_id: &str, cursor: u64) {
    let callbacks = SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let event = ChatUpdated {
        terminal_id: terminal_id.into(),
        cursor,
    };
    for callback in callbacks {
        callback(&event);
    }
}

/// Idempotent process-wide worker. Uses TerminalServiceHandle, including core-mode terminals.
pub fn start(terminal: TerminalServiceHandle) {
    SERVICE.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(256);
        let wake = tx.clone();
        agent_hooks::state::subscribe(Arc::new(move |_| {
            let _ = wake.try_send(Request::Wake);
        }));
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("agent-chat".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("agent chat runtime");
                let liveness = terminal.clone();
                let writer: Writer = Box::new(move |id, data| {
                    runtime.block_on(async {
                        tokio::time::timeout(
                            Duration::from_secs(2),
                            terminal.runtime().write(id, data),
                        )
                        .await
                        .map_err(|_| "terminal write timeout".to_string())?
                        .map_err(|_| "terminal write failed".to_string())
                    })
                });
                run(
                    rx,
                    worker_tx,
                    writer,
                    Box::new(move |id| {
                        !liveness.runtime().observes_live_terminals()
                            || liveness.runtime().is_live(id)
                    }),
                );
            })
            .expect("agent chat worker");
        Service { tx }
    });
}

fn run(
    rx: mpsc::Receiver<Request>,
    tx: mpsc::SyncSender<Request>,
    writer: Writer,
    live: Box<dyn Fn(&str) -> bool>,
) {
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut hook_cursor = agent_hooks::contract::HookStateSyncRequest::default();
    loop {
        let request = rx.recv_timeout(Duration::from_millis(250));
        retire_sessions(&mut sessions, Instant::now(), &*live);
        let response = agent_hooks::state::sync(&hook_cursor, &|_| true);
        let (instance_id, revision, states, tombstones) = match response {
            agent_hooks::contract::HookStateSyncResponse::Snapshot {
                instance_id,
                current_revision,
                states,
                tombstones,
            }
            | agent_hooks::contract::HookStateSyncResponse::Delta {
                instance_id,
                current_revision,
                states,
                tombstones,
            } => (instance_id, current_revision, states, tombstones),
        };
        hook_cursor.instance_id = Some(instance_id);
        hook_cursor.since_revision = Some(revision);
        for deleted in tombstones {
            sessions.remove(&deleted.terminal_id);
        }
        for value in states {
            let Some(hook) = derived_chat_hook(value) else {
                continue;
            };
            let id = hook.terminal_id.clone();
            if !sessions.contains_key(&id) && hook.kind == HookKind::SessionEnd {
                continue;
            }
            if !sessions.contains_key(&id) && !make_room(&mut sessions) {
                continue;
            }
            let session = sessions
                .entry(id)
                .or_insert_with(|| Session::new(hook.clone()));
            if session.hook.session_id != hook.session_id
                || session.hook.agent != hook.agent
                || (session.retired_at.is_some() && hook.kind != HookKind::SessionEnd)
            {
                session.replace(hook.clone());
            }
            session.apply_hook(hook);
            if session.retired_at.is_none() && (session.path.is_none() || session.watcher.is_none())
            {
                session.configure_path(&tx);
            }
        }
        for session in sessions.values_mut() {
            session.read();
            session.expire(Instant::now());
            session.flush(&writer);
            if session.last_emitted != Some(session.cursor) {
                emit(&session.hook.terminal_id, session.cursor);
                session.last_emitted = Some(session.cursor);
            }
        }
        match request {
            Ok(Request::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Request::Snapshot(id, after, before, reply)) => {
                let result = sessions
                    .get(&id)
                    .map(|s| s.page(after, before))
                    .ok_or_else(|| unsupported_or_missing(&id));
                let _ = reply.send(result);
            }
            Ok(Request::Send(id, text, options, guard, reply)) => {
                let result = sessions
                    .get_mut(&id)
                    .ok_or_else(|| unsupported_or_missing(&id))
                    .and_then(|s| {
                        let delivery = if options.force {
                            let id = options
                                .delivery_id
                                .as_deref()
                                .filter(|id| !id.is_empty())
                                .ok_or_else(|| {
                                    "force send requires a queued delivery".to_string()
                                })?;
                            s.force_send(id, guard, &writer)
                        } else if options.delivery_id.is_some() {
                            Err("delivery_id requires force".into())
                        } else {
                            s.enqueue(text, guard, &writer)
                        };
                        delivery.map_err(ChatError::Failed)
                    });
                if let Some(session) = sessions.get(&id) {
                    emit(&id, session.cursor);
                }
                let _ = reply.send(result);
            }
            Ok(Request::Control(id, action, guard, reply)) => {
                let result = if guard.as_ref().is_some_and(|guard| !guard()) {
                    Err(ChatError::Failed("terminal.input scope denied".into()))
                } else if sessions
                    .get(&id)
                    .is_some_and(|s| s.hook.kind != HookKind::SessionEnd)
                {
                    match action.as_str() {
                        "interrupt" => writer(&id, "\x1b").map_err(ChatError::Failed),
                        "stop" => writer(&id, "\x03").map_err(ChatError::Failed),
                        _ => Err(ChatError::Failed("unknown chat control".into())),
                    }
                } else {
                    Err(unsupported_or_missing(&id))
                };
                let _ = reply.send(result);
            }
        }
    }
}
// Keep final receipts observable for a grace period, but never let dead terminals
// turn the active cap into a process-lifetime limit. Global cursor allocation
// also fences a terminal reconstructed after retirement without a tombstone map.
fn retire_sessions(
    sessions: &mut HashMap<String, Session>,
    now: Instant,
    live: &dyn Fn(&str) -> bool,
) {
    for (id, session) in sessions.iter_mut() {
        if session.retired_at.is_none() && !live(id) {
            session.finish();
        }
    }
    sessions.retain(|_, s| {
        s.retired_at
            .is_none_or(|at| now.saturating_duration_since(at) < Duration::from_secs(30))
    });
}
fn make_room(sessions: &mut HashMap<String, Session>) -> bool {
    if sessions.len() < MAX_SESSIONS {
        return true;
    }
    let victim = sessions
        .iter()
        .filter_map(|(id, s)| s.retired_at.map(|at| (id.clone(), at)))
        .min_by_key(|(_, at)| *at)
        .map(|(id, _)| id);
    if let Some(id) = victim {
        sessions.remove(&id);
    }
    sessions.len() < MAX_SESSIONS
}
fn service() -> Result<&'static Service, ChatError> {
    SERVICE
        .get()
        .ok_or_else(|| ChatError::Failed("chat service unavailable".into()))
}
pub fn snapshot(id: &str, after: Option<u64>) -> Result<ChatSnapshot, ChatError> {
    snapshot_page(id, after, None)
}
pub fn snapshot_page(
    id: &str,
    after: Option<u64>,
    before: Option<u64>,
) -> Result<ChatSnapshot, ChatError> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Snapshot(id.into(), after, before, tx))
        .map_err(|_| ChatError::Failed("chat worker busy".into()))?;
    rx.recv_timeout(RPC_TIMEOUT)
        .map_err(|_| ChatError::Failed("chat worker timeout".into()))?
}
pub fn send(id: &str, text: String) -> Result<Delivery, ChatError> {
    send_authorized(id, text, None)
}
pub fn send_authorized(
    id: &str,
    text: String,
    guard: Option<SendGuard>,
) -> Result<Delivery, ChatError> {
    send_with_options(id, text, SendOptions::default(), guard)
}
pub fn send_with_options(
    id: &str,
    text: String,
    options: SendOptions,
    guard: Option<SendGuard>,
) -> Result<Delivery, ChatError> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Send(id.into(), text, options, guard, tx))
        .map_err(|_| ChatError::Failed("chat worker busy".into()))?;
    rx.recv_timeout(RPC_TIMEOUT).map_err(|_| {
        ChatError::Failed("chat worker timeout; delivery must not be retried automatically".into())
    })?
}
pub fn control(id: &str, action: &str) -> Result<(), ChatError> {
    control_authorized(id, action, None)
}
pub fn control_authorized(
    id: &str,
    action: &str,
    guard: Option<SendGuard>,
) -> Result<(), ChatError> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Control(id.into(), action.into(), guard, tx))
        .map_err(|_| ChatError::Failed("chat worker busy".into()))?;
    rx.recv_timeout(RPC_TIMEOUT)
        .map_err(|_| ChatError::Failed("chat worker timeout".into()))?
}

#[cfg(test)]
mod derived_state_tests {
    use super::*;
    use crate::agent_hooks::contract::*;
    fn value(adapter: &str, state: DerivedState, revision: u64) -> HookStateDto {
        HookStateDto {
            instance_id: "host".into(),
            terminal_id: "terminal".into(),
            generation: 1,
            adapter_install_id: "install".into(),
            agent_session_id: "session".into(),
            project_id: "project".into(),
            ready: state == DerivedState::Idle,
            state,
            identity: HookIdentityDto {
                adapter_id: AdapterId::try_from(adapter.to_owned()).unwrap(),
                agent_session_id: "session".into(),
                transcript_path: None,
                cwd: None,
                source: HookIdentitySource::Hook,
                descriptor_id: None,
                resume_argv: None,
            },
            restore_state: RestoreState::Confirmed,
            revision,
        }
    }
    #[test]
    fn agent_hooks_chat_ready_without_tool_events() {
        let idle = derived_chat_hook(value("pi", DerivedState::Idle, 1)).unwrap();
        let mut session = Session::new(idle.clone());
        session.apply_hook(idle);
        assert!(session.ready_credit);
        session.apply_hook(derived_chat_hook(value("pi", DerivedState::Busy, 2)).unwrap());
        assert!(!session.ready_credit);
        session.apply_hook(derived_chat_hook(value("pi", DerivedState::Idle, 3)).unwrap());
        assert!(session.ready_credit);
        assert_eq!(session.hook.tool_name, None);
    }
    #[test]
    fn agent_hooks_custom_state_does_not_select_builtin_reader() {
        assert!(derived_chat_hook(value("custom-agent", DerivedState::Idle, 1)).is_none());
    }
}
