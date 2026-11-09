//! Hook-backed terminal chat. One host worker owns cursors, the send queue and ack state.
//! No transcript or prompt is persisted in the search index or logged here.
pub mod adapters;
pub mod commands;
mod paths;
mod scan;
#[cfg(test)]
mod tests;

use crate::agent_hooks::{self, AgentHookEvent, AgentKind, HookKind};
use crate::core::TerminalServiceHandle;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{mpsc, Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

const ACK_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_QUEUE: usize = 64;
const MAX_TEXT: usize = 128 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
type Writer = Box<dyn Fn(&str, &str) -> Result<(), String> + Send>;
pub type SendGuard = Arc<dyn Fn() -> bool + Send + Sync>;
type Subscriber = Arc<dyn Fn(&ChatUpdated) + Send + Sync>;
static SERVICE: OnceLock<Service> = OnceLock::new();
static SUBSCRIBERS: OnceLock<Mutex<Vec<Subscriber>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct ChatUpdated {
    pub terminal_id: String,
    pub cursor: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub blocks: Vec<Value>,
    pub streaming: bool,
    pub timestamp: u64,
    pub seq: u64,
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
    messages: Vec<ChatMessage>,
    tools: Vec<Value>,
    pending: VecDeque<Pending>,
    reason: Option<String>,
    hook_state: String,
    ready_credit: bool,
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
        let hook_state = hook_name(hook.kind).to_owned();
        Self {
            hook,
            path: None,
            watcher: None,
            stamp: None,
            offset: 0,
            cursor: 0,
            reset_cursor: 0,
            last_emitted: None,
            messages: vec![],
            tools: vec![],
            pending: VecDeque::new(),
            reason: None,
            hook_state,
            ready_credit,
        }
    }
    fn apply_hook(&mut self, hook: AgentHookEvent) {
        if self.hook.sequence == hook.sequence {
            return;
        }
        let changed_path = self.hook.transcript_path != hook.transcript_path;
        let was_ready = matches!(self.hook.kind, HookKind::Idle | HookKind::TurnDone);
        let is_ready = matches!(hook.kind, HookKind::Idle | HookKind::TurnDone);
        self.hook_state = hook_name(hook.kind).into();
        if !is_ready {
            self.ready_credit = false;
        } else if !was_ready {
            self.ready_credit = true;
        }
        self.hook = hook;
        self.cursor += 1;
        if changed_path {
            self.path = None;
            self.stamp = None;
            self.offset = 0;
            self.messages.clear();
            self.tools.clear();
            self.reason = None;
            self.reset_cursor = self.cursor;
            for item in &mut self.pending {
                if item.delivery.status == "pending" {
                    item.delivery.status = "unconfirmed".into();
                    item.raw.clear();
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
            self.cursor += 1;
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
    fn read(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(stamp) = FileStamp::read(&path) else {
            self.degrade("transcript_unreadable");
            return;
        };
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        if self.stamp.as_ref().is_some_and(|old| {
            old.identity != stamp.identity
                || stamp.len < old.len
                || (stamp.len == old.len && old.modified != stamp.modified)
        }) {
            self.offset = 0;
            self.messages.clear();
            self.tools.clear();
            self.reason = None;
            self.cursor += 1;
            self.reset_cursor = self.cursor;
            // An echo from a replacement file is not evidence for an earlier write.
            for item in &mut self.pending {
                if item.delivery.status == "pending" {
                    item.delivery.status = "unconfirmed".into();
                    item.raw.clear();
                }
            }
        }
        let mut entries = vec![];
        let mut invalid = false;
        let scanned = scan::scan(&path, self.offset, |record| {
            match adapters::parse_record(self.hook.agent, &record.value, record.byte_offset) {
                Ok(parsed) => {
                    entries.extend(parsed.into_iter().map(|entry| (record.byte_offset, entry)))
                }
                Err(_) => invalid = true,
            }
        });
        self.offset = scanned.resume_offset;
        self.stamp = Some(stamp);
        if invalid || !scanned.issues.is_empty() {
            self.degrade("invalid_transcript_record");
        } else if matches!(
            self.reason.as_deref(),
            Some("transcript_not_found" | "transcript_unreadable")
        ) {
            self.reason = None;
            self.cursor += 1;
        }
        for (record_offset, entry) in entries {
            if self.messages.iter().any(|m| m.id == entry.id) {
                continue;
            }
            self.cursor += 1;
            if let Some(text) = &entry.user_text {
                if let Some(item) = self.pending.iter_mut().find(|p| {
                    p.delivery.status == "pending"
                        && record_offset >= p.baseline_offset
                        && self.cursor > p.baseline
                        && normalize(&p.raw) == normalize(text)
                }) {
                    item.delivery.status = "delivered".into();
                    item.raw.clear();
                }
            }
            for mut tool in entry.tool_calls {
                let id = tool.get("toolCallId").cloned();
                tool["seq"] = self.cursor.into();
                if let Some(previous) = self
                    .tools
                    .iter_mut()
                    .find(|t| t.get("toolCallId") == id.as_ref())
                {
                    if let (Some(target), Some(update)) =
                        (previous.as_object_mut(), tool.as_object())
                    {
                        for (key, value) in update {
                            if (key == "title" && value == "Tool result")
                                || (key == "kind" && value == "other")
                            {
                                if target.contains_key(key) {
                                    continue;
                                }
                            }
                            target.insert(key.clone(), value.clone());
                        }
                    }
                } else {
                    self.tools.push(tool);
                }
            }
            if !entry.blocks.is_empty() {
                self.messages.push(ChatMessage {
                    id: entry.id,
                    role: entry.role,
                    blocks: entry.blocks,
                    streaming: false,
                    timestamp: now_ms(),
                    seq: self.cursor,
                });
            }
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
                item.raw.clear();
                self.cursor += 1;
                tracing::warn!(
                    event = "agent_chat_ack_timeout",
                    kind = "no_transcript_echo",
                    count = 1
                );
            }
        }
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
        if self.pending[index]
            .guard
            .as_ref()
            .is_some_and(|guard| !guard())
        {
            let item = &mut self.pending[index];
            item.delivery.status = "unconfirmed".into();
            item.raw.clear();
            self.cursor += 1;
            return;
        }
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
            item.raw.clear();
        }
        self.cursor += 1;
        tracing::info!(
            event = "agent_chat_send",
            kind = "bracketed_paste",
            count = 1
        );
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
            .filter(|p| p.delivery.status == "queued")
            .count()
            >= MAX_QUEUE
        {
            return Err("chat queue full".into());
        }
        let delivery = Delivery {
            id: uuid::Uuid::new_v4().to_string(),
            text: crate::memory_index::redact::redact(&text).text,
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
        self.cursor += 1;
        if !self.ready_credit {
            tracing::info!(event = "agent_chat_queue", kind = "busy", count = 1);
        }
        self.flush(writer);
        Ok(self
            .pending
            .back()
            .expect("just inserted delivery")
            .delivery
            .clone())
    }
    fn snapshot(&self, after: Option<u64>) -> ChatSnapshot {
        let after = after.unwrap_or(0);
        let reset = after == 0 || after > self.cursor || after < self.reset_cursor;
        ChatSnapshot {
            terminal_id: self.hook.terminal_id.clone(),
            session_id: self.hook.session_id.clone(),
            agent: self.hook.agent,
            cursor: self.cursor,
            messages: self
                .messages
                .iter()
                .filter(|m| reset || m.seq > after)
                .cloned()
                .collect(),
            tool_calls: self.tools.clone(),
            deliveries: self.pending.iter().map(|p| p.delivery.clone()).collect(),
            state: if self.reason.is_some() {
                "degraded"
            } else {
                "ready"
            },
            reason: self.reason.clone(),
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

struct Service {
    tx: mpsc::SyncSender<Request>,
}
enum Request {
    Wake,
    Snapshot(
        String,
        Option<u64>,
        mpsc::SyncSender<Result<ChatSnapshot, String>>,
    ),
    Send(
        String,
        String,
        Option<SendGuard>,
        mpsc::SyncSender<Result<Delivery, String>>,
    ),
    Control(
        String,
        String,
        Option<SendGuard>,
        mpsc::SyncSender<Result<(), String>>,
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
        agent_hooks::subscribe(Arc::new(move |_| {
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
                run(rx, worker_tx, writer);
            })
            .expect("agent chat worker");
        Service { tx }
    });
}

fn run(rx: mpsc::Receiver<Request>, tx: mpsc::SyncSender<Request>, writer: Writer) {
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut hook_cursor = 0;
    loop {
        let request = rx.recv_timeout(Duration::from_millis(250));
        for hook in agent_hooks::events(hook_cursor) {
            hook_cursor = hook_cursor.max(hook.sequence);
            let id = hook.terminal_id.clone();
            if !sessions.contains_key(&id) && sessions.len() >= 128 {
                continue;
            }
            let session = sessions
                .entry(id)
                .or_insert_with(|| Session::new(hook.clone()));
            if session.hook.session_id != hook.session_id || session.hook.agent != hook.agent {
                *session = Session::new(hook.clone());
            }
            session.apply_hook(hook);
            if session.path.is_none() || session.watcher.is_none() {
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
            Ok(Request::Snapshot(id, after, reply)) => {
                let result = sessions
                    .get(&id)
                    .map(|s| s.snapshot(after))
                    .ok_or_else(|| "terminal has no hooked agent".into());
                let _ = reply.send(result);
            }
            Ok(Request::Send(id, text, guard, reply)) => {
                let result = sessions
                    .get_mut(&id)
                    .ok_or_else(|| "terminal has no hooked agent".into())
                    .and_then(|s| s.enqueue(text, guard, &writer));
                if let Some(session) = sessions.get(&id) {
                    emit(&id, session.cursor);
                }
                let _ = reply.send(result);
            }
            Ok(Request::Control(id, action, guard, reply)) => {
                let result = if guard.as_ref().is_some_and(|guard| !guard()) {
                    Err("terminal.input scope denied".into())
                } else if sessions
                    .get(&id)
                    .is_some_and(|s| s.hook.kind != HookKind::SessionEnd)
                {
                    match action.as_str() {
                        "interrupt" => writer(&id, "\x1b"),
                        "stop" => writer(&id, "\x03"),
                        _ => Err("unknown chat control".into()),
                    }
                } else {
                    Err("terminal has no hooked agent".into())
                };
                let _ = reply.send(result);
            }
        }
    }
}
fn service() -> Result<&'static Service, String> {
    SERVICE
        .get()
        .ok_or_else(|| "chat service unavailable".into())
}
pub fn snapshot(id: &str, after: Option<u64>) -> Result<ChatSnapshot, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Snapshot(id.into(), after, tx))
        .map_err(|_| "chat worker busy")?;
    rx.recv_timeout(RPC_TIMEOUT)
        .map_err(|_| "chat worker timeout")?
}
pub fn send(id: &str, text: String) -> Result<Delivery, String> {
    send_authorized(id, text, None)
}
pub fn send_authorized(
    id: &str,
    text: String,
    guard: Option<SendGuard>,
) -> Result<Delivery, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Send(id.into(), text, guard, tx))
        .map_err(|_| "chat worker busy")?;
    rx.recv_timeout(RPC_TIMEOUT)
        .map_err(|_| "chat worker timeout; delivery must not be retried automatically")?
}
pub fn control(id: &str, action: &str) -> Result<(), String> {
    control_authorized(id, action, None)
}
pub fn control_authorized(id: &str, action: &str, guard: Option<SendGuard>) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(1);
    service()?
        .tx
        .try_send(Request::Control(id.into(), action.into(), guard, tx))
        .map_err(|_| "chat worker busy")?;
    rx.recv_timeout(RPC_TIMEOUT)
        .map_err(|_| "chat worker timeout")?
}
