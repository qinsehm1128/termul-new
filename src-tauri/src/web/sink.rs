//! Transport-neutral event sink for the ACP dispatcher.
//!
//! The dispatcher (see `crate::acp::manager` + `crate::acp::client`) emits every
//! agent/session event through a fan-out of [`EventSink`] trait objects instead
//! of calling `AppHandle::emit` directly. That decouples the live session stream
//! from Tauri so the same dispatcher can feed:
//!
//! - the desktop's Tauri events ([`TauriEventSink`] — byte-for-byte preserves the
//!   existing `acp:*` event names + payloads the renderer depends on), and
//! - the future web's WebSocket relay ([`WsRelaySink`] — stubbed here as an
//!   in-memory recorder; wired live in Story 1.4).
//!
//! # Design rules baked in (do not deviate)
//!
//! - **Serialize ONCE, fan out N.** [`fan_out`] serializes the payload to a
//!   `serde_json::Value` once; every sink emits the same `Value`, so
//!   `TauriEventSink` and `WsRelaySink` emit byte-identical payloads.
//! - **`type_` keeps the `acp:` prefix.** [`TauriEventSink`] emits it verbatim
//!   (today's behavior); `WsRelaySink` will strip the prefix when the WS relay
//!   lands in Story 1.4. For this story the stub records the full string.
//! - **`sid` is `Option<String>`.** `None` for agent-level events
//!   (`agent_spawned`, `agent_disconnected`, `agent_error` without a session);
//!   `Some(session_id)` for session-scoped events. Matches the WS envelope
//!   `{sid, seq, type, payload}` shape (Story 1.4).
//!
//! `AcpManager` holds `Vec<Arc<dyn EventSink>>` and threads clones into every
//! driver spawn site, so `AppHandle` no longer reaches the driver thread. The
//! ONLY remaining `AppHandle` reference in the ACP stack lives inside
//! [`TauriEventSink`] (the desktop's sink — intentionally Tauri-aware).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

use crate::acp::session_persistence::{
    now_millis, PersistedEventRecord, SessionPersistence, SESSION_SCHEMA_VERSION,
};
use crate::web::project_registry::ProjectsChangedPayload;
use crate::web::ws::{tier_of, ReliabilityTier, SequencedEvent};

// Global lock order for WsRelaySink (never invert — avoids deadlock):
// 1. `sessions`  2. `clients`  3. `session_subs`
// Prefer releasing a lock before acquiring the next when both are not required
// for the critical section.

/// A single ACP event ready for fan-out.
///
/// `sid` is the session id (`None` for agent-level events like `agent_spawned`
/// / `agent_disconnected`). `type_` is the existing `acp:*` event name with the
/// `acp:` prefix (e.g. `"acp:message_chunk"`) — [`TauriEventSink`] emits it
/// verbatim; `WsRelaySink` will strip the prefix when the WS relay lands in
/// Story 1.4. `payload` is the serialized JSON value so every sink emits
/// byte-identical bytes (serialize ONCE, fan out N times).
#[derive(Clone, Debug)]
pub struct AcpEvent {
    pub sid: Option<String>,
    pub type_: &'static str,
    pub payload: Value,
}

pub const CONVERSATION_PERSISTENCE_REJECTED: &str = "CONVERSATION_PERSISTENCE_REJECTED";
pub const CONVERSATION_BINDING_NOT_FOUND: &str = "CONVERSATION_BINDING_NOT_FOUND";
pub const EVENT_DELIVERY_FAILED: &str = "EVENT_DELIVERY_FAILED";
pub const EVENT_SERIALIZATION_FAILED: &str = "EVENT_SERIALIZATION_FAILED";
pub const CLIENT_OUTBOUND_RECORDS: usize = 512;
pub const CLIENT_OUTBOUND_BYTES: usize = 8 * 1024 * 1024;
pub const RELIABLE_CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_RELAY_SESSIONS: usize = 256;
pub const MAX_RELAY_BYTES: usize = 64 * 1024 * 1024;
pub const SESSION_GATE_STRIPES: usize = 64;
pub const MAX_CONNECTION_SUBSCRIPTIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSinkPriority {
    DurableAdmission,
    LiveDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventDeliveryReceipt {
    pub delivered: bool,
    pub durable_admission: bool,
    pub session_seq: Option<u64>,
}

impl EventDeliveryReceipt {
    #[must_use]
    pub const fn delivered(session_seq: Option<u64>, durable_admission: bool) -> Self {
        Self {
            delivered: true,
            durable_admission,
            session_seq,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSinkError {
    pub code: &'static str,
    pub source_code: Option<&'static str>,
    pub durable_rejection: bool,
    pub detail: String,
}

impl EventSinkError {
    fn persistence_rejected(source_code: &'static str) -> Self {
        Self {
            code: CONVERSATION_PERSISTENCE_REJECTED,
            source_code: Some(source_code),
            durable_rejection: true,
            detail: "canonical Conversation persistence rejected event admission".to_string(),
        }
    }

    fn serialization_failed() -> Self {
        Self {
            code: EVENT_SERIALIZATION_FAILED,
            source_code: None,
            durable_rejection: false,
            detail: "ACP event payload could not be serialized".to_string(),
        }
    }

    fn delivery_failed(detail: impl Into<String>) -> Self {
        Self {
            code: EVENT_DELIVERY_FAILED,
            source_code: None,
            durable_rejection: false,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for EventSinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for EventSinkError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanOutReceipt {
    pub sink_count: usize,
    pub delivered_count: usize,
    pub durable_admission_count: usize,
    pub session_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanOutError {
    pub code: &'static str,
    pub source_code: Option<&'static str>,
    pub durable_rejection: bool,
    pub delivered_count: usize,
    pub detail: String,
}

impl FanOutError {
    #[must_use]
    pub const fn is_durable_rejection(&self) -> bool {
        self.durable_rejection
    }

    /// Warm-pool / pre-bind sessions have no Conversation yet. Durable
    /// admission fail-closes, but that must not latch a delivery circuit or
    /// fail user-facing `set_model` / `set_config_option` — the agent already
    /// applied the change.
    #[must_use]
    pub fn is_unbound_session(&self) -> bool {
        self.source_code == Some(CONVERSATION_BINDING_NOT_FOUND)
    }

    #[must_use]
    pub fn should_open_session_circuit(&self) -> bool {
        let retryable = matches!(
            self.source_code,
            Some(
                "CONVERSATION_PERSISTENCE_BYTES_SATURATED"
                    | "CONVERSATION_PERSISTENCE_QUEUE_SATURATED"
                    | "SESSION_PERSISTENCE_QUEUE_FULL"
            )
        );
        self.is_durable_rejection() && !retryable && !self.is_unbound_session()
    }

    #[must_use]
    pub fn circuit_open(source_code: &'static str) -> Self {
        Self {
            code: CONVERSATION_PERSISTENCE_REJECTED,
            source_code: Some(source_code),
            durable_rejection: true,
            delivered_count: 0,
            detail: "event delivery circuit is open for this session".to_string(),
        }
    }
}

impl fmt::Display for FanOutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for FanOutError {}

/// Transport-neutral sink for ACP events.
///
/// Durable-admission sinks run before live-only sinks. A canonical rejection therefore prevents
/// Tauri, relay history, and subscriber state from observing an event that was never admitted.
pub trait EventSink: Send + Sync {
    fn priority(&self) -> EventSinkPriority {
        EventSinkPriority::LiveDelivery
    }

    fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError>;
}

/// Desktop sink: forwards events to the Tauri renderer as `acp:*` events.
///
/// Byte-for-byte preserves the existing `events::emit(app, event, payload)`
/// behavior — same event names, same payloads (the `Value` was produced by the
/// same `serde_json::to_value` the old free function used implicitly via
/// `app.emit`), same error-logging-not-propagating semantics.
pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    /// Wrap a Tauri app handle. The sink is cheap to construct and `Clone`-free
    /// (it shares the handle via `AppHandle`'s internal `Arc`).
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError> {
        self.app
            .emit(event.type_, event.payload.clone())
            .map_err(|_| EventSinkError::delivery_failed("desktop renderer emission failed"))?;
        Ok(EventDeliveryReceipt::delivered(None, false))
    }
}

/// Live WS relay sink (Story 1.4 — replaces the Story 1.1 in-memory recorder).
///
/// Owns the per-session append-only bounded event logs (the canonical replay
/// source, D5), per-session monotonic `seq` counters, and the per-client
/// subscriber set. `emit` is called from the per-agent driver thread (via
/// [`fan_out`]); it assigns `seq`, submits canonical Conversation events through a bounded
/// per-session writer (blocking only that session's producer when its durable queue is full), and
/// then fans out to each subscribed client's `tokio::sync::mpsc::UnboundedSender`.
///
/// # Tier handling (AC5)
///
/// - **Lossy** events (`message_chunk`, `tool_call_update`, `commands_update`,
///   `plan_update`) are buffered in a per-client bounded ring; when the ring
///   is full the OLDEST lossy event is dropped. The write loop drains the ring
///   via [`Self::flush_lossy`]. Under a slow WS peer the write loop stalls, the
///   ring fills, and drop-oldest triggers — the lossy backpressure path.
/// - **Reliable** events are sent on the unbounded per-client channel and are
///   never dropped (full ack/backpressure + timeout=deny lands in Story 1.7).
/// - **Idempotent** (`prompt_complete`) is deduped by turn-id when the payload
///   carries one; the current `PromptCompleteEvent` has no turn-id field, so
///   the relay sends it through (like reliable) and the client dedups by
///   `seq` (Dev Notes #6).
///
/// Constructible WITHOUT a Tauri `AppHandle` (Story 1.1 invariant — the
/// standalone `se-server` binary has no Tauri app). `Send + Sync` so clones
/// of `Arc<WsRelaySink>` can cross from the Tauri command thread into each
/// agent's dedicated driver thread.
pub struct WsRelaySink {
    /// Per-session seq counter + append-only bounded ring (canonical replay
    /// source, D5). Combined under one mutex so seq assignment and log append
    /// are atomic w.r.t. concurrent emits (AC4).
    sessions: Mutex<HashMap<String, SessionState>>,
    /// Per-client subscription: client_id → client state (sender + sessions).
    clients: Arc<Mutex<HashMap<ClientId, ClientSub>>>,
    /// Reverse index: session_id → set of subscribed client_ids.
    session_subs: Arc<Mutex<HashMap<String, HashSet<ClientId>>>>,
    /// A rejected durable admission opens a circuit only for the affected opaque session.
    delivery_circuits: Mutex<HashMap<String, &'static str>>,
    history_clock: AtomicU64,
    /// Bounded per-session ring capacity (default 4096, AC4).
    event_log_capacity: usize,
    /// Per-client lossy ring capacity (drop-oldest threshold, AC5).
    lossy_capacity: usize,
    /// Server-side permission rendezvous (Story 1.7). `None` on the desktop
    /// path (the browser-less flow uses the `acp_respond_permission` Tauri
    /// command directly). When set, `emit` snapshots `acp:permission_request`
    /// events into a relay-side ticket table that enforces the rendezvous
    /// policy (timeout, at-most-one, first-wins, disconnect-deny, TOCTOU).
    rendezvous: Mutex<Option<Arc<crate::web::permissions::PermissionRendezvous>>>,
    /// Server-side question rendezvous (issue #411). `None` on the desktop
    /// path (the browser-less flow uses the `acp_answer_question` Tauri
    /// command directly). When set, `emit` snapshots `acp:question_request`
    /// events into a relay-side ticket table (first-wins, TOCTOU, timeout).
    question_rendezvous: Mutex<Option<Arc<crate::web::permissions::QuestionRendezvous>>>,
    /// Server-side turn-id watermark (Story 1.7 T7.2 — FR13/FR11 plumbing).
    /// Always present (cheap to construct; no external handle). 1.8's
    /// `prompt_complete` / `send_prompt` handlers read this via
    /// [`Self::turn_watermark`] to dedup agent turns by client turn-id.
    turn_watermark: crate::web::permissions::TurnWatermark,
    /// Read-only legacy history provider retained for compatibility reads.
    persistence: Option<Arc<SessionPersistence>>,
    /// Canonical Conversation adapter used for reads and binding resolution.
    conversation_persistence: Option<Arc<crate::conversation::ConversationPersistenceAdapter>>,
    /// Bounded retained writers for the canonical live ACP append path.
    ordered_conversation_persistence:
        Option<Arc<crate::conversation::OrderedConversationPersistence>>,
    /// Fixed striped gates spanning canonical cursor selection, ticket acknowledgement, and live
    /// publication. The stripe count is constant under unbounded session churn.
    persistence_submission_gates: [Mutex<()>; SESSION_GATE_STRIPES],
    /// Fixed striped gates serializing durable replay/catch-up/register handoff.
    replay_gates: [tokio::sync::Mutex<()>; SESSION_GATE_STRIPES],
}

/// Per-session seq + append-only bounded ring (held under [`WsRelaySink::sessions`]).
struct SessionState {
    /// Last assigned seq (0 = none yet; next emit assigns `last_seq + 1`).
    last_seq: u64,
    /// Bounded ring; oldest evicted when `len > capacity`.
    events: VecDeque<SequencedEvent>,
    /// Complete in-memory session event snapshot for atomic stale recovery on
    /// desktop shared-live, where no file-backed event persistence exists.
    snapshot_events: Vec<SequencedEvent>,
    /// `seq` of the oldest event currently in the ring (for cursor-gap detect).
    base_seq: u64,
    retained_bytes: usize,
    reserved_bytes: usize,
    last_used: u64,
}

fn sequenced_event_bytes(event: &SequencedEvent) -> Result<usize, EventSinkError> {
    crate::conversation::contracts::encoded_json_len_bounded(
        event,
        crate::conversation::MAX_CONVERSATION_RECORD_BYTES,
    )
    .map(|encoded| encoded.saturating_add(std::mem::size_of::<SequencedEvent>()))
    .ok_or_else(|| {
        EventSinkError::delivery_failed("relay history record exceeds the 262144-byte host bound")
    })
}

fn sequenced_event_bytes_len(event: &SequencedEvent) -> usize {
    sequenced_event_bytes(event).unwrap_or(usize::MAX)
}

fn relay_history_bytes(sessions: &HashMap<String, SessionState>) -> usize {
    sessions.values().fold(0usize, |total, state| {
        total
            .saturating_add(state.retained_bytes)
            .saturating_add(state.reserved_bytes)
    })
}

fn repository_runtime_handle_for_relay() -> tokio::runtime::Handle {
    tokio::runtime::Handle::try_current()
        .unwrap_or_else(|_| tauri::async_runtime::handle().inner().clone())
}

fn session_persistence_error_code(
    error: &crate::acp::session_persistence::SessionPersistenceError,
) -> &'static str {
    use crate::acp::session_persistence::SessionPersistenceError;
    match error {
        SessionPersistenceError::Io(_) => "SESSION_PERSISTENCE_IO_FAILED",
        SessionPersistenceError::Json(_) => "SESSION_PERSISTENCE_SERIALIZATION_FAILED",
        SessionPersistenceError::UnsupportedVersion { .. } => "SESSION_PERSISTENCE_UNSUPPORTED",
        SessionPersistenceError::SessionNotFound => "SESSION_PERSISTENCE_NOT_FOUND",
        SessionPersistenceError::CorruptSession => "SESSION_PERSISTENCE_CORRUPT",
        SessionPersistenceError::InvalidStorageKey => "SESSION_PERSISTENCE_INVALID_KEY",
        SessionPersistenceError::QueueFull => "SESSION_PERSISTENCE_QUEUE_FULL",
        SessionPersistenceError::WriterStopped => "SESSION_PERSISTENCE_WRITER_STOPPED",
        SessionPersistenceError::PersistenceUnhealthy(_) => "SESSION_PERSISTENCE_UNHEALTHY",
        SessionPersistenceError::StaleCursor { .. } => "SESSION_PERSISTENCE_STALE_CURSOR",
    }
}

#[derive(Debug, Default)]
struct ClientOutboundBudgetState {
    records: usize,
    bytes: usize,
    max_records: usize,
    max_bytes: usize,
}

#[derive(Debug, Default)]
struct ClientOutboundBudget {
    state: Mutex<ClientOutboundBudgetState>,
}

impl ClientOutboundBudget {
    fn try_reserve(self: &Arc<Self>, bytes: usize) -> Option<ClientOutboundPermit> {
        if bytes > CLIENT_OUTBOUND_BYTES {
            return None;
        }
        let mut state = self.state.lock();
        if state.records >= CLIENT_OUTBOUND_RECORDS
            || state.bytes.saturating_add(bytes) > CLIENT_OUTBOUND_BYTES
        {
            return None;
        }
        state.records += 1;
        state.bytes += bytes;
        state.max_records = state.max_records.max(state.records);
        state.max_bytes = state.max_bytes.max(state.bytes);
        Some(ClientOutboundPermit {
            budget: Arc::clone(self),
            bytes,
        })
    }

    fn snapshot(&self) -> ClientOutboundStats {
        let state = self.state.lock();
        ClientOutboundStats {
            records: state.records,
            bytes: state.bytes,
            max_records: state.max_records,
            max_bytes: state.max_bytes,
        }
    }
}

struct ClientOutboundPermit {
    budget: Arc<ClientOutboundBudget>,
    bytes: usize,
}

impl Drop for ClientOutboundPermit {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock();
        state.records = state.records.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
    }
}

struct QueuedClientEvent {
    event: Option<SequencedEvent>,
    _permit: ClientOutboundPermit,
}

pub struct ClientEventReceiver {
    inner: mpsc::Receiver<QueuedClientEvent>,
}

impl ClientEventReceiver {
    pub async fn recv(&mut self) -> Option<SequencedEvent> {
        let mut queued = self.inner.recv().await?;
        queued.event.take()
    }

    pub fn try_recv(&mut self) -> std::result::Result<SequencedEvent, mpsc::error::TryRecvError> {
        let mut queued = self.inner.try_recv()?;
        Ok(queued
            .event
            .take()
            .expect("queued client event is consumed exactly once"))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientOutboundStats {
    pub records: usize,
    pub bytes: usize,
    pub max_records: usize,
    pub max_bytes: usize,
}

/// Per-client subscription state.
struct ClientSub {
    tx: mpsc::Sender<QueuedClientEvent>,
    budget: Arc<ClientOutboundBudget>,
    /// Sessions this client is subscribed to.
    sessions: HashSet<String>,
    /// Bounded buffer for lossy events (drop-oldest when full).
    lossy_ring: VecDeque<QueuedClientEvent>,
    reliable_saturation_generation: u64,
}

/// Opaque per-connection client id (uuid v4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(Uuid);

impl ClientId {
    /// Generate a new random client id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ClientId {
    fn default() -> Self {
        Self::new()
    }
}

/// Cursor-replay result for [`WsRelaySink::subscribe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayResult {
    /// Replay succeeded; carries the number of events replayed from the log tail.
    Ok(u64),
    /// `last_seq` is older than the log's oldest (evicted) event — the client
    /// must re-sync (AC4).
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayHistoryStats {
    pub sessions: usize,
    pub bytes: usize,
    pub reserved_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayAuxiliaryStats {
    pub relay_sessions: usize,
    pub clients: usize,
    pub subscription_sessions: usize,
    pub subscriptions: usize,
    pub delivery_circuits: usize,
    pub ordered_sessions: usize,
    pub submission_gate_stripes: usize,
    pub replay_gate_stripes: usize,
    pub turn_watermarks: crate::web::permissions::TurnWatermarkStats,
}

struct HistoryReservation {
    sid: String,
    bytes: usize,
    newly_created: bool,
    active: bool,
}

/// Default per-session event-log capacity (AC4).
pub const DEFAULT_EVENT_LOG_CAPACITY: usize = 4096;
/// Default per-client lossy ring capacity (drop-oldest threshold).
const DEFAULT_LOSSY_CAPACITY: usize = 256;

impl WsRelaySink {
    /// Create a live relay sink with default capacities
    /// (`event_log_capacity = 4096`, `lossy_capacity = 256`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EVENT_LOG_CAPACITY, DEFAULT_LOSSY_CAPACITY)
    }

    /// Create a live relay sink with explicit capacities (AC4 + AC5).
    #[must_use]
    pub fn with_capacity(event_log_capacity: usize, lossy_capacity: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            clients: Arc::new(Mutex::new(HashMap::new())),
            session_subs: Arc::new(Mutex::new(HashMap::new())),
            delivery_circuits: Mutex::new(HashMap::new()),
            history_clock: AtomicU64::new(0),
            event_log_capacity: event_log_capacity.max(1),
            lossy_capacity: lossy_capacity.max(1),
            rendezvous: Mutex::new(None),
            question_rendezvous: Mutex::new(None),
            turn_watermark: crate::web::permissions::TurnWatermark::new(),
            persistence: None,
            conversation_persistence: None,
            ordered_conversation_persistence: None,
            persistence_submission_gates: std::array::from_fn(|_| Mutex::new(())),
            replay_gates: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        }
    }

    /// Create a live relay sink with a custom per-session event-log capacity and
    /// the default per-client lossy ring capacity (AC4 + AC5). Used by
    /// `se-server` to thread `ServerConfig::event_log_capacity`.
    #[must_use]
    pub fn with_log_capacity(event_log_capacity: usize) -> Self {
        Self::with_capacity(event_log_capacity, DEFAULT_LOSSY_CAPACITY)
    }

    #[must_use]
    pub fn with_persistence(
        event_log_capacity: usize,
        persistence: Arc<SessionPersistence>,
    ) -> Self {
        let mut sink = Self::with_capacity(event_log_capacity, DEFAULT_LOSSY_CAPACITY);
        for entry in persistence.list_sessions() {
            if let Ok(turn_ids) = persistence.completed_turn_ids(&entry.session_id) {
                sink.turn_watermark
                    .restore_completed(&entry.session_id, turn_ids);
            }
        }
        sink.persistence = Some(persistence);
        sink
    }

    #[must_use]
    pub fn with_conversation_persistence(
        event_log_capacity: usize,
        persistence: Arc<crate::conversation::ConversationPersistenceAdapter>,
        legacy_read_only: Option<Arc<SessionPersistence>>,
    ) -> Self {
        let mut sink = Self::with_capacity(event_log_capacity, DEFAULT_LOSSY_CAPACITY);
        sink.ordered_conversation_persistence = Some(Arc::new(
            crate::conversation::OrderedConversationPersistence::new(Arc::clone(&persistence)),
        ));
        sink.conversation_persistence = Some(persistence);
        sink.persistence = legacy_read_only;
        sink
    }

    #[must_use]
    pub fn conversation_persistence(
        &self,
    ) -> Option<Arc<crate::conversation::ConversationPersistenceAdapter>> {
        self.conversation_persistence.clone()
    }

    #[must_use]
    pub fn ordered_conversation_persistence(
        &self,
    ) -> Option<Arc<crate::conversation::OrderedConversationPersistence>> {
        self.ordered_conversation_persistence.clone()
    }

    /// Await the canonical ordered-writer durability frontier.
    pub async fn flush_conversation_persistence(&self) -> Result<(), String> {
        match &self.ordered_conversation_persistence {
            Some(persistence) => persistence
                .flush_all()
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Await the canonical ordered-writer frontier under the host's absolute deadline.
    pub async fn flush_conversation_persistence_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        match &self.ordered_conversation_persistence {
            Some(persistence) => persistence
                .flush_all_until(deadline)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Final host-owned drain for the canonical ordered writers.
    pub async fn shutdown_conversation_persistence(&self) -> Result<(), String> {
        match &self.ordered_conversation_persistence {
            Some(persistence) => persistence
                .shutdown()
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Final host-owned drain under the same absolute deadline used by catalog shutdown.
    pub async fn shutdown_conversation_persistence_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        match &self.ordered_conversation_persistence {
            Some(persistence) => persistence
                .shutdown_until(deadline)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Flush the latest disposable catalog generation under the host deadline.
    pub async fn flush_catalog_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<crate::conversation::CatalogFlushReceipt, String> {
        match &self.conversation_persistence {
            Some(persistence) => persistence
                .flush_catalog_until(deadline)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(crate::conversation::CatalogFlushReceipt {
                requested_generation: 0,
                flushed_generation: 0,
                write_count: 0,
            }),
        }
    }

    #[must_use]
    pub fn persistence(&self) -> Option<Arc<SessionPersistence>> {
        self.persistence.clone()
    }

    #[must_use]
    pub fn has_persisted_history(&self) -> bool {
        self.conversation_persistence.is_some() || self.persistence.is_some()
    }

    /// The configured per-session event-log capacity (AC4).
    #[must_use]
    pub fn event_log_capacity(&self) -> usize {
        self.event_log_capacity
    }

    /// Attach the server-side permission rendezvous (Story 1.7). Server-only;
    /// the desktop path leaves this unset (the browser-less flow uses the
    /// `acp_respond_permission` Tauri command directly). Once attached, `emit`
    /// snapshots `acp:permission_request` events into the rendezvous so the
    /// `/ws` `respond_permission` handler + disconnect cleanup can enforce the
    /// rendezvous policy.
    pub fn set_rendezvous(&self, rendezvous: Arc<crate::web::permissions::PermissionRendezvous>) {
        *self.rendezvous.lock() = Some(rendezvous);
    }

    /// The attached rendezvous, if any (server path). Used by the `/ws`
    /// `respond_permission` handler + disconnect deny-all cleanup.
    #[must_use]
    pub fn rendezvous(&self) -> Option<Arc<crate::web::permissions::PermissionRendezvous>> {
        self.rendezvous.lock().clone()
    }

    /// Attach the server-side question rendezvous (issue #411). Server-only;
    /// when set, `emit` snapshots `acp:question_request` events into the
    /// rendezvous so the `/ws` `answer_question` handler can enforce the
    /// rendezvous policy (first-wins, TOCTOU, timeout).
    pub fn set_question_rendezvous(
        &self,
        rendezvous: Arc<crate::web::permissions::QuestionRendezvous>,
    ) {
        *self.question_rendezvous.lock() = Some(rendezvous);
    }

    /// The attached question rendezvous, if any (server path). Used by the `/ws`
    /// `answer_question` handler + disconnect deny-all cleanup.
    #[must_use]
    pub fn question_rendezvous(&self) -> Option<Arc<crate::web::permissions::QuestionRendezvous>> {
        self.question_rendezvous.lock().clone()
    }

    /// Number of clients currently subscribed to `session_id` (Story 1.7
    /// disconnect-deny: a pending permission is denied only when the
    /// disconnecting client was the LAST subscriber on its session — otherwise a
    /// remaining client can still legitimately respond).
    #[must_use]
    pub fn session_subscriber_count(&self, session_id: &str) -> usize {
        self.session_subs
            .lock()
            .get(session_id)
            .map_or(0, HashSet::len)
    }

    /// The server-side turn-id watermark (Story 1.7 T7.2 — FR13/FR11 plumbing).
    /// 1.8's `prompt_complete` / `send_prompt` handlers call this to dedup agent
    /// turns by client turn-id (the wire-level `turnId` field lands in 1.8).
    #[must_use]
    pub fn turn_watermark(&self) -> &crate::web::permissions::TurnWatermark {
        &self.turn_watermark
    }

    /// Current session sequence frontier. Used as the snapshot watermark.
    #[must_use]
    pub fn session_watermark(&self, session_id: &str) -> u64 {
        let durable_frontier = self.durable_history_frontier(session_id).unwrap_or(0);
        self.sessions
            .lock()
            .get(session_id)
            .map_or(durable_frontier, |state| {
                state.last_seq.max(durable_frontier)
            })
    }

    fn session_gate_index(sid: &str) -> usize {
        sid.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        }) as usize
            % SESSION_GATE_STRIPES
    }

    fn persistence_submission_gate(&self, sid: &str) -> &Mutex<()> {
        &self.persistence_submission_gates[Self::session_gate_index(sid)]
    }

    fn replay_gate(&self, sid: &str) -> &tokio::sync::Mutex<()> {
        &self.replay_gates[Self::session_gate_index(sid)]
    }

    fn durable_history_frontier(&self, sid: &str) -> Option<u64> {
        self.conversation_persistence
            .as_ref()
            .and_then(|persistence| persistence.history_last_seq(sid).ok())
            .or_else(|| {
                self.persistence
                    .as_ref()
                    .and_then(|persistence| persistence.last_seq(sid).ok())
            })
    }

    fn next_sequenced_event(&self, sid: &str, type_: &str, payload: Value) -> SequencedEvent {
        let next_seq = self.session_watermark(sid).saturating_add(1);
        let ticket = crate::conversation::CanonicalSequenceTicket::from_allocated_seq(next_seq);
        SequencedEvent::new(Some(sid.to_string()), ticket.seq, type_, payload)
    }

    fn circuit_error(&self, sid: &str) -> Option<EventSinkError> {
        self.delivery_circuits
            .lock()
            .get(sid)
            .copied()
            .map(EventSinkError::persistence_rejected)
    }

    fn open_delivery_circuit(&self, sid: &str, source_code: &'static str) {
        self.delivery_circuits
            .lock()
            .entry(sid.to_string())
            .or_insert(source_code);
        log::error!(
            "[conversation-persistence] session delivery circuit opened code={} source_code={}",
            CONVERSATION_PERSISTENCE_REJECTED,
            source_code
        );
    }

    fn is_retryable_persistence_code(code: &str) -> bool {
        matches!(
            code,
            "CONVERSATION_PERSISTENCE_BYTES_SATURATED"
                | "CONVERSATION_PERSISTENCE_QUEUE_SATURATED"
                | "SESSION_PERSISTENCE_QUEUE_FULL"
                | "CONVERSATION_CONFLICT"
        )
    }

    /// Sessions without a canonical binding (pre-rebind legacy reopens) have no
    /// durable home; drop their events without latching the delivery circuit so
    /// a later rebind can recover the session.
    fn is_unbound_drop_code(code: &str) -> bool {
        code == CONVERSATION_BINDING_NOT_FOUND
    }

    /// Reserve relay retention, acquire durable admission, then commit the live frontier.
    /// The per-session submission gate prevents a rejected record from consuming a sequence and
    /// prevents another producer from observing the reservation as live state.
    fn admit_session_event(
        &self,
        sid: &str,
        type_: &str,
        payload: Value,
    ) -> Result<SequencedEvent, EventSinkError> {
        if let Some(error) = self.circuit_error(sid) {
            return Err(error);
        }
        let gate = self.persistence_submission_gate(sid);
        let _submission_guard = gate.lock();
        if let Some(error) = self.circuit_error(sid) {
            return Err(error);
        }

        let mut sequenced = self.next_sequenced_event(sid, type_, payload);
        let mut reservation = self.reserve_history(sid, &sequenced)?;
        let durable_result: Result<Option<u64>, (&'static str, String)> =
            if let Some(persistence) = &self.ordered_conversation_persistence {
                persistence
                    .submit(sid, sequenced.seq, type_, sequenced.payload.clone())
                    .and_then(|ticket| ticket.wait())
                    .map(Some)
                    .map_err(|error| (error.code, error.to_string()))
            } else if let Some(persistence) = &self.persistence {
                // The pre-cutover store historically retained every relay event. Keep compatibility
                // reads exact while still rejecting before live commit if its queue refuses admission.
                persistence
                    .enqueue_event(PersistedEventRecord {
                        schema_version: SESSION_SCHEMA_VERSION,
                        session_id: sid.to_string(),
                        seq: sequenced.seq,
                        type_: type_.to_string(),
                        recorded_at: now_millis(),
                        payload: sequenced.payload.clone(),
                    })
                    .map(|()| None)
                    .map_err(|error| (session_persistence_error_code(&error), error.to_string()))
            } else {
                Ok(None)
            };

        match durable_result {
            Ok(Some(canonical_seq)) if canonical_seq != sequenced.seq => {
                sequenced = SequencedEvent::new(
                    Some(sid.to_string()),
                    crate::conversation::CanonicalSequenceTicket::from_allocated_seq(canonical_seq)
                        .seq,
                    type_,
                    sequenced.payload.clone(),
                );
            }
            Ok(_) => {}
            Err((source_code, _detail)) => {
                self.rollback_history(&mut reservation);
                if !Self::is_retryable_persistence_code(source_code)
                    && !Self::is_unbound_drop_code(source_code)
                {
                    self.open_delivery_circuit(sid, source_code);
                }
                return Err(EventSinkError::persistence_rejected(source_code));
            }
        }
        if let Err(error) = self.commit_history(&mut reservation, sequenced.clone()) {
            self.rollback_history(&mut reservation);
            return Err(error);
        }
        Ok(sequenced)
    }

    fn reserve_history(
        &self,
        sid: &str,
        event: &SequencedEvent,
    ) -> Result<HistoryReservation, EventSinkError> {
        let event_bytes = sequenced_event_bytes(event)?;
        let multiplier = if self.persistence.is_none() && self.conversation_persistence.is_none() {
            2
        } else {
            1
        };
        let bytes = event_bytes.saturating_mul(multiplier);
        if bytes > MAX_RELAY_BYTES {
            return Err(EventSinkError::delivery_failed(
                "relay history record exceeds the process byte budget",
            ));
        }
        let durable_last = self.durable_history_frontier(sid).unwrap_or(0);
        let auxiliary_bytes = self.auxiliary_charged_bytes();
        let mut sessions = self.sessions.lock();
        let newly_created = !sessions.contains_key(sid);
        while (newly_created && sessions.len() >= MAX_RELAY_SESSIONS)
            || relay_history_bytes(&sessions)
                .saturating_add(bytes)
                .saturating_add(auxiliary_bytes)
                > MAX_RELAY_BYTES
        {
            let subscriber_counts = self.session_subs.lock();
            let candidate = sessions
                .iter()
                .filter(|(candidate_sid, state)| {
                    candidate_sid.as_str() != sid
                        && subscriber_counts
                            .get(candidate_sid.as_str())
                            .is_none_or(HashSet::is_empty)
                        && state.reserved_bytes == 0
                        && self.session_is_durably_flushed(candidate_sid, state.last_seq)
                })
                .min_by_key(|(_, state)| state.last_used)
                .map(|(candidate_sid, _)| candidate_sid.clone());
            drop(subscriber_counts);
            let Some(candidate) = candidate else {
                log::error!(
                    "[ws-relay] retention rejected code=RELAY_HISTORY_BUDGET_EXCEEDED session_count={} retained_bytes={}",
                    sessions.len(),
                    relay_history_bytes(&sessions)
                );
                return Err(EventSinkError::delivery_failed(
                    "relay history budget is exhausted by non-evictable sessions",
                ));
            };
            sessions.remove(&candidate);
            let retained_sessions = sessions.len();
            let retained_bytes = relay_history_bytes(&sessions);
            drop(sessions);
            // Payload-cache eviction only. Semantic circuits/watermarks stay until
            // confirmed close/delete/dispose.
            log::info!(
                "[ws-relay] durable history payload evicted session_count={} retained_bytes={}",
                retained_sessions,
                retained_bytes
            );
            sessions = self.sessions.lock();
        }
        let last_used = self.history_clock.fetch_add(1, Ordering::AcqRel) + 1;
        let state = sessions
            .entry(sid.to_string())
            .or_insert_with(|| SessionState {
                last_seq: durable_last,
                events: VecDeque::new(),
                snapshot_events: Vec::new(),
                base_seq: durable_last.saturating_add(1),
                retained_bytes: 0,
                reserved_bytes: 0,
                last_used,
            });
        state.reserved_bytes = state.reserved_bytes.saturating_add(bytes);
        state.last_used = last_used;
        Ok(HistoryReservation {
            sid: sid.to_string(),
            bytes,
            newly_created,
            active: true,
        })
    }

    fn commit_history(
        &self,
        reservation: &mut HistoryReservation,
        event: SequencedEvent,
    ) -> Result<(), EventSinkError> {
        let mut sessions = self.sessions.lock();
        let state = sessions.get_mut(&reservation.sid).ok_or_else(|| {
            EventSinkError::delivery_failed("relay history reservation disappeared")
        })?;
        if event.seq <= state.last_seq {
            return Err(EventSinkError::delivery_failed(
                "relay history sequence did not advance",
            ));
        }
        state.reserved_bytes = state.reserved_bytes.saturating_sub(reservation.bytes);
        state.last_seq = event.seq;
        if state.events.is_empty() {
            state.base_seq = event.seq;
        }
        state.retained_bytes = state
            .retained_bytes
            .saturating_add(sequenced_event_bytes_len(&event));
        state.events.push_back(event.clone());
        if self.persistence.is_none() && self.conversation_persistence.is_none() {
            state.retained_bytes = state
                .retained_bytes
                .saturating_add(sequenced_event_bytes_len(&event));
            state.snapshot_events.push(event.clone());
            while state.snapshot_events.len() > self.event_log_capacity {
                if let Some(evicted) = state.snapshot_events.first() {
                    state.retained_bytes = state
                        .retained_bytes
                        .saturating_sub(sequenced_event_bytes_len(evicted));
                }
                state.snapshot_events.remove(0);
            }
        }
        while state.events.len() > self.event_log_capacity {
            if let Some(evicted) = state.events.pop_front() {
                state.retained_bytes = state
                    .retained_bytes
                    .saturating_sub(sequenced_event_bytes_len(&evicted));
            }
            state.base_seq = state
                .events
                .front()
                .map(|event| event.seq)
                .unwrap_or(state.last_seq.saturating_add(1));
        }
        reservation.active = false;
        Ok(())
    }

    fn rollback_history(&self, reservation: &mut HistoryReservation) {
        if !reservation.active {
            return;
        }
        let mut sessions = self.sessions.lock();
        if let Some(state) = sessions.get_mut(&reservation.sid) {
            state.reserved_bytes = state.reserved_bytes.saturating_sub(reservation.bytes);
            if reservation.newly_created
                && state.events.is_empty()
                && state.snapshot_events.is_empty()
                && state.reserved_bytes == 0
            {
                sessions.remove(&reservation.sid);
            }
        }
        reservation.active = false;
    }

    fn session_is_durably_flushed(&self, sid: &str, last_seq: u64) -> bool {
        if let Some(ordered) = &self.ordered_conversation_persistence {
            match ordered.health(sid) {
                Ok(Some(health)) => health.last_persisted_source_seq >= last_seq,
                Ok(None) | Err(_) => self
                    .conversation_persistence
                    .as_ref()
                    .and_then(|persistence| persistence.history_last_seq(sid).ok())
                    .is_some_and(|frontier| frontier >= last_seq),
            }
        } else {
            self.persistence
                .as_ref()
                .and_then(|persistence| persistence.last_seq(sid).ok())
                .is_some_and(|frontier| frontier >= last_seq)
        }
    }

    #[must_use]
    pub fn relay_history_stats(&self) -> RelayHistoryStats {
        let sessions = self.sessions.lock();
        RelayHistoryStats {
            sessions: sessions.len(),
            bytes: sessions.values().map(|state| state.retained_bytes).sum(),
            reserved_bytes: sessions.values().map(|state| state.reserved_bytes).sum(),
        }
    }

    fn auxiliary_charged_bytes(&self) -> usize {
        let circuit_bytes = self.delivery_circuits.lock().len().saturating_mul(
            std::mem::size_of::<String>().saturating_add(std::mem::size_of::<&'static str>()),
        );
        let watermark = self.turn_watermark.stats();
        let watermark_bytes = watermark
            .seen_turns
            .saturating_add(watermark.completed_turns)
            .saturating_mul(std::mem::size_of::<String>());
        let ordered_bytes = self
            .ordered_conversation_persistence
            .as_ref()
            .map_or(0, |ordered| ordered.retained_worker_count())
            .saturating_mul(std::mem::size_of::<crate::conversation::ConversationId>());
        circuit_bytes
            .saturating_add(watermark_bytes)
            .saturating_add(ordered_bytes)
    }

    #[must_use]
    pub fn auxiliary_stats(&self) -> RelayAuxiliaryStats {
        let relay_sessions = self.sessions.lock().len();
        let clients = self.clients.lock().len();
        let (subscription_sessions, subscriptions) = {
            let subscriptions = self.session_subs.lock();
            (
                subscriptions.len(),
                subscriptions.values().map(HashSet::len).sum(),
            )
        };
        RelayAuxiliaryStats {
            relay_sessions,
            clients,
            subscription_sessions,
            subscriptions,
            delivery_circuits: self.delivery_circuits.lock().len(),
            ordered_sessions: self
                .ordered_conversation_persistence
                .as_ref()
                .map_or(0, |ordered| ordered.retained_worker_count()),
            submission_gate_stripes: self.persistence_submission_gates.len(),
            replay_gate_stripes: self.replay_gates.len(),
            turn_watermarks: self.turn_watermark.stats(),
        }
    }

    fn queued_client_event(sub: &ClientSub, event: SequencedEvent) -> Option<QueuedClientEvent> {
        let permit = sub.budget.try_reserve(sequenced_event_bytes_len(&event))?;
        Some(QueuedClientEvent {
            event: Some(event),
            _permit: permit,
        })
    }

    /// Push a lossy event into a client's bounded ring, evicting oldest lossy records until both
    /// the record and byte budgets admit the newest value.
    fn push_lossy(&self, sub: &mut ClientSub, event: SequencedEvent) {
        while sub.lossy_ring.len() >= self.lossy_capacity {
            sub.lossy_ring.pop_front();
            log::warn!(
                "[ws-relay] lossy record dropped code=CLIENT_OUTBOUND_LOSSY_DROP records={} bytes={}",
                sub.budget.snapshot().records,
                sub.budget.snapshot().bytes
            );
        }
        loop {
            if let Some(queued) = Self::queued_client_event(sub, event.clone()) {
                sub.lossy_ring.push_back(queued);
                return;
            }
            if sub.lossy_ring.pop_front().is_none() {
                log::warn!(
                    "[ws-relay] lossy record rejected code=CLIENT_OUTBOUND_BUDGET records={} bytes={}",
                    sub.budget.snapshot().records,
                    sub.budget.snapshot().bytes
                );
                return;
            }
        }
    }

    /// Enqueue an event to a client according to its tier. Every stage is bounded to 512 records
    /// and 8 MiB. Reliable saturation arms an exact five-second disconnect; reconnect replays the
    /// canonical cursor rather than retaining an unbounded reliable backlog.
    fn enqueue(&self, client_id: ClientId, event: SequencedEvent, tier: ReliabilityTier) {
        let mut slow_generation = None;
        let dead_sids = {
            let mut clients = self.clients.lock();
            let Some(sub) = clients.get_mut(&client_id) else {
                return;
            };
            let send_ok = match tier {
                ReliabilityTier::Lossy => {
                    self.push_lossy(sub, event);
                    self.flush_lossy_sub(sub)
                }
                ReliabilityTier::Reliable | ReliabilityTier::Idempotent => {
                    if !self.flush_lossy_sub(sub) {
                        false
                    } else if let Some(queued) = Self::queued_client_event(sub, event) {
                        match sub.tx.try_send(queued) {
                            Ok(()) => true,
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                if sub.reliable_saturation_generation == 0 {
                                    sub.reliable_saturation_generation = 1;
                                    slow_generation = Some(1);
                                }
                                true
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => false,
                        }
                    } else {
                        if sub.reliable_saturation_generation == 0 {
                            sub.reliable_saturation_generation = 1;
                            slow_generation = Some(1);
                        }
                        true
                    }
                }
            };
            if send_ok {
                None
            } else {
                clients
                    .remove(&client_id)
                    .map(|sub| sub.sessions.into_iter().collect::<Vec<_>>())
            }
        };
        if let Some(generation) = slow_generation {
            self.schedule_slow_client_disconnect(client_id, generation);
        }
        if let Some(sids) = dead_sids {
            self.remove_client_from_session_subs(client_id, &sids);
        }
    }

    fn schedule_slow_client_disconnect(&self, client_id: ClientId, generation: u64) {
        let clients = Arc::clone(&self.clients);
        let session_subs = Arc::clone(&self.session_subs);
        repository_runtime_handle_for_relay().spawn(async move {
            tokio::time::sleep(RELIABLE_CLIENT_TIMEOUT).await;
            let sids = {
                let mut clients = clients.lock();
                let disconnect = clients.get(&client_id).is_some_and(|sub| {
                    sub.reliable_saturation_generation == generation
                });
                if disconnect {
                    clients
                        .remove(&client_id)
                        .map(|sub| sub.sessions.into_iter().collect::<Vec<_>>())
                } else {
                    None
                }
            };
            if let Some(sids) = sids {
                let mut session_subs = session_subs.lock();
                for sid in &sids {
                    if let Some(subscribers) = session_subs.get_mut(sid) {
                        subscribers.remove(&client_id);
                        if subscribers.is_empty() {
                            session_subs.remove(sid);
                        }
                    }
                }
                log::warn!(
                    "[ws-relay] reliable slow client disconnected code=CLIENT_OUTBOUND_TIMEOUT duration_ms={}",
                    RELIABLE_CLIENT_TIMEOUT.as_millis()
                );
            }
        });
    }

    /// Remove `client_id` from the reverse index for each session (no `clients` lock).
    fn remove_client_from_session_subs(&self, client_id: ClientId, sids: &[String]) {
        let mut session_subs = self.session_subs.lock();
        for sid in sids {
            if let Some(set) = session_subs.get_mut(sid) {
                set.remove(&client_id);
                if set.is_empty() {
                    session_subs.remove(sid);
                }
            }
        }
    }

    /// Subscribe a new client to a session with an optional cursor (AC4).
    ///
    /// `last_seq = None` → live-only (no replay). `last_seq = Some(n)` → replay
    /// the log tail from `n + 1` then live-stream. If `n` is older than the
    /// log's oldest (evicted) event, returns [`ReplayResult::Stale`] (the
    /// client must re-sync) and DOES NOT register the subscription.
    ///
    /// Holds the sessions lock across stale-check + register + replay so an
    /// emit cannot slip into the gap between unlock and register (TOCTOU).
    ///
    /// Returns the new client id + the receiver the write loop drains.
    pub async fn subscribe(
        &self,
        sid: &str,
        last_seq: Option<u64>,
    ) -> (ClientId, ClientEventReceiver, ReplayResult) {
        let (client_id, rx) = self.open_client();
        let replay = self.subscribe_existing(client_id, sid, last_seq).await;
        if replay == ReplayResult::Stale {
            self.unregister_client(client_id);
        }
        (client_id, rx, replay)
    }

    /// Subscribe another session on an existing connection-owned relay client. All sessions share
    /// the same 512-record/8-MiB budget and one receiver/forwarding task.
    pub async fn subscribe_existing(
        &self,
        client_id: ClientId,
        sid: &str,
        last_seq: Option<u64>,
    ) -> ReplayResult {
        if !self.clients.lock().contains_key(&client_id) {
            return ReplayResult::Stale;
        }
        let Some(cursor) = last_seq else {
            if !self.register_existing(client_id, sid) {
                return ReplayResult::Stale;
            }
            return ReplayResult::Ok(0);
        };

        let _replay_guard = self.replay_gate(sid).lock().await;
        let mut by_seq = std::collections::BTreeMap::new();
        loop {
            if let Some(ordered) = &self.ordered_conversation_persistence {
                if ordered.flush_all().await.is_err() {
                    return ReplayResult::Stale;
                }
            }
            if let Some(persistence) = &self.conversation_persistence {
                let durable = match persistence.replay_after(sid, cursor) {
                    Ok(records) => records,
                    Err(_) => return ReplayResult::Stale,
                };
                for record in durable {
                    by_seq.insert(
                        record.seq,
                        SequencedEvent::new(
                            Some(record.session_id),
                            record.seq,
                            record.type_,
                            record.payload,
                        ),
                    );
                }
            }
            if let Some(persistence) = &self.persistence {
                // Flush is a queue barrier for everything assigned before it.
                // The JSONL scan itself runs on spawn_blocking.
                if persistence.flush_session(sid).await.is_err() {
                    return ReplayResult::Stale;
                }
                let durable = match persistence
                    .replay_after_async(sid.to_string(), cursor)
                    .await
                {
                    Ok(records) => records,
                    Err(_) => return ReplayResult::Stale,
                };
                for record in durable {
                    by_seq.insert(
                        record.seq,
                        SequencedEvent::new(
                            Some(record.session_id),
                            record.seq,
                            record.type_,
                            record.payload,
                        ),
                    );
                }
            }

            let sessions = self.sessions.lock();
            if self.persistence.is_none()
                && sessions.get(sid).is_some_and(|state| {
                    cursor
                        .checked_add(1)
                        .is_some_and(|next| next < state.base_seq)
                })
            {
                return ReplayResult::Stale;
            }
            let (ring, last_seq, base_seq) = sessions.get(sid).map_or_else(
                || (Vec::new(), cursor, cursor.saturating_add(1)),
                |state| {
                    (
                        state
                            .events
                            .iter()
                            .filter(|event| event.seq > cursor)
                            .cloned()
                            .collect::<Vec<_>>(),
                        state.last_seq,
                        state.base_seq,
                    )
                },
            );
            for event in ring {
                by_seq.insert(event.seq, event);
            }
            // A high max sequence is not proof of coverage: validate every
            // sequence from cursor+1 through the observed frontier. If any hole
            // was evicted while disk replay was in flight, drop the state lock,
            // flush/re-read durable history, and retry before registering.
            let frontier = last_seq;
            let first_missing = cursor
                .checked_add(1)
                .and_then(|start| (start..=frontier).find(|seq| !by_seq.contains_key(seq)));
            if self.conversation_persistence.is_none() && first_missing.is_some() {
                if self.persistence.is_none() || base_seq <= cursor.saturating_add(1) {
                    return ReplayResult::Stale;
                }
                drop(sessions);
                continue;
            }

            if !self.register_existing(client_id, sid) {
                return ReplayResult::Stale;
            }
            let count = by_seq.len() as u64;
            for event in by_seq.into_values() {
                self.enqueue(client_id, event.clone(), tier_of(&event.type_));
                if !self.clients.lock().contains_key(&client_id) {
                    drop(sessions);
                    return ReplayResult::Stale;
                }
            }
            return ReplayResult::Ok(count);
        }
    }

    /// Atomically register a client and capture the complete session event
    /// snapshot plus its sequence watermark. The sessions lock is held across
    /// capture + registration, so subsequent emits are strictly post-watermark.
    ///
    /// When persistence is available, the snapshot is rebuilt from durable
    /// history (the in-memory `snapshot_events` is NOT maintained on that path
    /// — see `assign_and_append`). If the session is truly unknown to
    /// persistence, an `Err` is propagated so the caller returns `not_found`
    /// instead of an empty snapshot that would wipe transcripts.
    pub async fn subscribe_snapshot(
        &self,
        sid: &str,
    ) -> Result<(ClientId, ClientEventReceiver, Vec<SequencedEvent>, u64), String> {
        let client_id = ClientId::new();
        let (tx, rx) = mpsc::channel::<QueuedClientEvent>(CLIENT_OUTBOUND_RECORDS);
        let rx = ClientEventReceiver { inner: rx };
        let _replay_guard = self.replay_gate(sid).lock().await;
        if let Some(ordered) = &self.ordered_conversation_persistence {
            ordered
                .flush_all()
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(persistence) = &self.conversation_persistence {
            let watermark = persistence
                .last_seq(sid)
                .map_err(|error| error.to_string())?;
            let records = persistence
                .replay_after(sid, 0)
                .map_err(|error| error.to_string())?;
            let snapshot = records
                .into_iter()
                .map(|record| {
                    SequencedEvent::new(
                        Some(record.session_id),
                        record.seq,
                        record.type_,
                        record.payload,
                    )
                })
                .collect();
            self.register(client_id, sid, tx);
            return Ok((client_id, rx, snapshot, watermark));
        }
        if let Some(persistence) = &self.persistence {
            // Persistence is available: rebuild the snapshot from durable
            // history (do NOT maintain `snapshot_events` on this path).
            let _ = persistence.flush_session(sid).await;
            let watermark = persistence
                .last_seq(sid)
                .map_err(|error| error.to_string())?;
            let records = persistence
                .replay_after_async(sid.to_string(), 0)
                .await
                .map_err(|error| error.to_string())?;
            let snapshot: Vec<SequencedEvent> = records
                .into_iter()
                .map(|record| {
                    SequencedEvent::new(
                        Some(record.session_id),
                        record.seq,
                        record.type_,
                        record.payload,
                    )
                })
                .collect();
            self.register(client_id, sid, tx);
            return Ok((client_id, rx, snapshot, watermark));
        }
        // Desktop shared-live: use the bounded in-memory `snapshot_events`.
        let sessions = self.sessions.lock();
        let (snapshot, watermark) = sessions.get(sid).map_or_else(
            || (Vec::new(), 0),
            |state| (state.snapshot_events.clone(), state.last_seq),
        );
        self.register(client_id, sid, tx);
        drop(sessions);
        Ok((client_id, rx, snapshot, watermark))
    }

    /// Authoritative server-authored user prompt: assign the relay sequence,
    /// persist it, and synchronously wait for the durability boundary before
    /// ACP dispatch.
    pub async fn persist_user_prompt(
        &self,
        sid: &str,
        payload: Value,
    ) -> Result<SequencedEvent, String> {
        let event = self
            .admit_session_event(sid, "user_prompt", payload)
            .map_err(|error| error.code.to_string())?;
        if let Some(persistence) = &self.ordered_conversation_persistence {
            persistence
                .flush_all()
                .await
                .map_err(|error| error.to_string())?;
        } else if let Some(persistence) = &self.persistence {
            persistence
                .flush_session(sid)
                .await
                .map_err(|error| error.to_string())?;
        }
        let targets: Vec<ClientId> = self
            .session_subs
            .lock()
            .get(sid)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();
        for client_id in targets {
            self.enqueue(client_id, event.clone(), ReliabilityTier::Reliable);
        }
        Ok(event)
    }

    fn open_client(&self) -> (ClientId, ClientEventReceiver) {
        let client_id = ClientId::new();
        let (tx, rx) = mpsc::channel::<QueuedClientEvent>(CLIENT_OUTBOUND_RECORDS);
        self.clients.lock().insert(
            client_id,
            ClientSub {
                tx,
                budget: Arc::new(ClientOutboundBudget::default()),
                sessions: HashSet::new(),
                lossy_ring: VecDeque::new(),
                reliable_saturation_generation: 0,
            },
        );
        (client_id, ClientEventReceiver { inner: rx })
    }

    fn register_existing(&self, client_id: ClientId, sid: &str) -> bool {
        {
            let mut clients = self.clients.lock();
            let Some(client) = clients.get_mut(&client_id) else {
                return false;
            };
            client.sessions.insert(sid.to_string());
        }
        self.session_subs
            .lock()
            .entry(sid.to_string())
            .or_default()
            .insert(client_id);
        true
    }

    /// Register a client + its sender under a session and the reverse index.
    /// Lock order: `clients` then `session_subs` (see module lock-order note).
    fn register(&self, client_id: ClientId, sid: &str, tx: mpsc::Sender<QueuedClientEvent>) {
        {
            let mut clients = self.clients.lock();
            clients.insert(
                client_id,
                ClientSub {
                    tx,
                    budget: Arc::new(ClientOutboundBudget::default()),
                    sessions: HashSet::from([sid.to_string()]),
                    lossy_ring: VecDeque::new(),
                    reliable_saturation_generation: 0,
                },
            );
        }
        let mut session_subs = self.session_subs.lock();
        session_subs
            .entry(sid.to_string())
            .or_default()
            .insert(client_id);
    }

    /// Unsubscribe a client from a session (AC4). Removes the client entirely
    /// when it has no remaining sessions.
    pub fn unsubscribe(&self, sid: &str, client_id: ClientId) {
        {
            let mut clients = self.clients.lock();
            let Some(sub) = clients.get_mut(&client_id) else {
                return;
            };
            sub.sessions.remove(sid);
            if sub.sessions.is_empty() {
                clients.remove(&client_id);
            }
        }
        // Release `clients` before `session_subs` (lock order / no dual-hold).
        let mut session_subs = self.session_subs.lock();
        if let Some(set) = session_subs.get_mut(sid) {
            set.remove(&client_id);
            if set.is_empty() {
                session_subs.remove(sid);
            }
        }
    }

    fn retire_auxiliary(&self, sid: &str) -> Result<(), EventSinkError> {
        if let Some(ordered) = &self.ordered_conversation_persistence {
            ordered
                .retire_session(sid)
                .map_err(|error| EventSinkError::persistence_rejected(error.code))?;
        }
        self.delivery_circuits.lock().remove(sid);
        let affected_clients = self.session_subs.lock().remove(sid).unwrap_or_default();
        if !affected_clients.is_empty() {
            let mut clients = self.clients.lock();
            for client_id in affected_clients {
                if let Some(client) = clients.get_mut(&client_id) {
                    client.sessions.remove(sid);
                    if client.sessions.is_empty() {
                        clients.remove(&client_id);
                    }
                }
            }
        }
        self.turn_watermark.forget_session(sid);
        Ok(())
    }

    /// Retire every auxiliary structure keyed by one successfully closed/deleted/disposed ACP
    /// session. Admitted ordered work is observed first; canonical Conversation JSON/JSONL is
    /// deliberately untouched. Repeated calls are successful no-ops.
    pub async fn retire_session(&self, sid: &str) -> Result<(), String> {
        {
            let _submission_guard = self.persistence_submission_gate(sid).lock();
        }
        if let Some(ordered) = &self.ordered_conversation_persistence {
            ordered
                .retire_session_async(sid)
                .await
                .map_err(|error| error.code.to_string())?;
        }
        let _submission_guard = self.persistence_submission_gate(sid).lock();
        self.retire_auxiliary(sid)
            .map_err(|error| error.code.to_string())?;
        self.sessions.lock().remove(sid);
        Ok(())
    }

    /// Compatibility alias for older call sites; all lifecycle owners use `retire_session`.
    pub async fn forget_session(&self, sid: &str) {
        let _ = self.retire_session(sid).await;
    }

    /// Remove a client entirely (e.g. on WS close).
    pub fn unregister_client(&self, client_id: ClientId) {
        let sids: Vec<String> = {
            let mut clients = self.clients.lock();
            clients
                .remove(&client_id)
                .map(|sub| sub.sessions.into_iter().collect())
                .unwrap_or_default()
        };
        self.remove_client_from_session_subs(client_id, &sids);
    }

    /// Flush a client's buffered lossy events into its outbound channel (AC5).
    ///
    /// Called by the WS write loop (and by tests). Under a slow peer the write
    /// loop can stall before flush; the ring fills and drop-oldest triggers in
    /// [`Self::push_lossy`]. Returns the number of events flushed.
    pub fn flush_lossy(&self, client_id: ClientId) -> usize {
        let (n, dead_sids) = {
            let mut clients = self.clients.lock();
            let Some(sub) = clients.get_mut(&client_id) else {
                return 0;
            };
            let pending = sub.lossy_ring.len();
            let ok = self.flush_lossy_sub(sub);
            if ok {
                (pending, None)
            } else {
                (
                    pending,
                    clients
                        .remove(&client_id)
                        .map(|s| s.sessions.into_iter().collect::<Vec<_>>()),
                )
            }
        };
        if let Some(sids) = dead_sids {
            self.remove_client_from_session_subs(client_id, &sids);
        }
        n
    }

    /// Flush the lossy ring for a borrowed client sub.
    /// Returns `true` if every event was sent (or the ring was empty).
    fn flush_lossy_sub(&self, sub: &mut ClientSub) -> bool {
        while let Some(event) = sub.lossy_ring.pop_front() {
            match sub.tx.try_send(event) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(event)) => {
                    sub.lossy_ring.push_front(event);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    sub.lossy_ring.clear();
                    return false;
                }
            }
        }
        true
    }

    /// Test helper: fill the lossy ring without flushing (exercises drop-oldest).
    #[cfg(test)]
    fn push_lossy_no_flush_for_test(&self, client_id: ClientId, se: SequencedEvent) {
        let mut clients = self.clients.lock();
        if let Some(sub) = clients.get_mut(&client_id) {
            self.push_lossy(sub, se);
        }
    }

    #[must_use]
    pub fn client_outbound_stats(&self, client_id: ClientId) -> Option<ClientOutboundStats> {
        self.clients
            .lock()
            .get(&client_id)
            .map(|sub| sub.budget.snapshot())
    }

    #[must_use]
    pub fn has_client(&self, client_id: ClientId) -> bool {
        self.clients.lock().contains_key(&client_id)
    }

    /// Test helper: current lossy-ring length for a client.
    #[cfg(test)]
    fn lossy_ring_len_for_test(&self, client_id: ClientId) -> usize {
        self.clients
            .lock()
            .get(&client_id)
            .map(|s| s.lossy_ring.len())
            .unwrap_or(0)
    }
}

impl Default for WsRelaySink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for WsRelaySink {
    fn priority(&self) -> EventSinkPriority {
        if self.ordered_conversation_persistence.is_some() || self.persistence.is_some() {
            EventSinkPriority::DurableAdmission
        } else {
            EventSinkPriority::LiveDelivery
        }
    }

    fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError> {
        // Strip the `acp:` prefix to get the WS `type` (AC2).
        let type_ = event.type_.strip_prefix("acp:").unwrap_or(event.type_);
        let tier = tier_of(type_);
        let session_seq = match &event.sid {
            Some(sid) => {
                if crate::conversation::contracts::encoded_json_len_bounded(
                    &event.payload,
                    crate::conversation::MAX_CONVERSATION_RECORD_BYTES,
                )
                .is_none()
                {
                    return Err(EventSinkError::delivery_failed(
                        "relay history record exceeds the 262144-byte host bound",
                    ));
                }
                let sequenced = self.admit_session_event(sid, type_, event.payload.clone())?;
                let targets: Vec<ClientId> = self
                    .session_subs
                    .lock()
                    .get(sid)
                    .map(|set| set.iter().copied().collect())
                    .unwrap_or_default();
                for client_id in targets {
                    self.enqueue(client_id, sequenced.clone(), tier);
                }
                Some(sequenced.seq)
            }
            None => {
                // Agent-level: seq=0, sid=null, NOT in any per-session log (AC4).
                let sequenced = SequencedEvent::new(None, 0, type_, event.payload.clone());
                let targets: Vec<ClientId> = self.clients.lock().keys().copied().collect();
                for client_id in targets {
                    self.enqueue(client_id, sequenced.clone(), tier);
                }
                None
            }
        };

        // Register human-input rendezvous only after relay admission succeeds. A rejected durable
        // event must not leave a permission/question ticket that no client could have observed.
        if type_ == "permission_request" {
            if let Some(rdz) = self.rendezvous() {
                let payload = &event.payload;
                let request_id = payload
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if request_id.is_empty() {
                    warn!(
                        "[permissions] dropping permission_request with no requestId (dispatcher bug?)"
                    );
                } else {
                    let agent_id = payload
                        .get("agentId")
                        .and_then(Value::as_str)
                        .map(|value| crate::acp::AgentId(value.to_string()))
                        .unwrap_or_else(|| crate::acp::AgentId("unknown".to_string()));
                    let session_id = payload
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let options = payload
                        .get("options")
                        .cloned()
                        .unwrap_or(Value::Array(vec![]));
                    rdz.register(request_id, agent_id, session_id, options);
                }
            }
        }
        if type_ == "question_request" {
            if let Some(rdz) = self.question_rendezvous() {
                let payload = &event.payload;
                let question_id = payload
                    .get("questionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if question_id.is_empty() {
                    warn!(
                        "[questions] dropping question_request with no questionId (dispatcher bug?)"
                    );
                } else {
                    let agent_id = payload
                        .get("agentId")
                        .and_then(Value::as_str)
                        .map(|value| crate::acp::AgentId(value.to_string()))
                        .unwrap_or_else(|| crate::acp::AgentId("unknown".to_string()));
                    let session_id = payload
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let options = payload
                        .get("options")
                        .cloned()
                        .unwrap_or(Value::Array(vec![]));
                    rdz.register(question_id, agent_id, session_id, options);
                }
            }
        }

        if self.has_persisted_history()
            && matches!(
                type_,
                "session_created"
                    | "session_closed"
                    | "session_info_update"
                    | "local_title_generated"
            )
        {
            self.notify_history_changed();
        }

        Ok(EventDeliveryReceipt::delivered(
            session_seq,
            self.priority() == EventSinkPriority::DurableAdmission,
        ))
    }
}

impl WsRelaySink {
    /// Agent-level `chat_history_changed` fan-out (mirrors
    /// `broadcast_chat_history_changed`, but callable from inside `emit` where
    /// no `Arc<Self>` is available). Empty payload; clients refetch the index.
    fn notify_history_changed(&self) {
        let type_ = "chat_history_changed";
        let se = SequencedEvent::new(None, 0, type_, json!({}));
        let tier = tier_of(type_);
        let targets: Vec<ClientId> = self.clients.lock().keys().copied().collect();
        for client_id in targets {
            self.enqueue(client_id, se.clone(), tier);
        }
    }
}

enum BoundedPayloadEncoding {
    WithinLimit,
    TooLarge,
    SerializationFailed,
}

struct BoundedPayloadCounter {
    bytes: usize,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedPayloadCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(next) = self.bytes.checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded JSON length overflow",
            ));
        };
        if next > self.limit {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded JSON exceeds configured limit",
            ));
        }
        self.bytes = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn classify_payload_encoding<P: Serialize>(payload: &P, limit: usize) -> BoundedPayloadEncoding {
    let mut counter = BoundedPayloadCounter {
        bytes: 0,
        limit,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, payload) {
        Ok(()) => BoundedPayloadEncoding::WithinLimit,
        Err(_) if counter.exceeded => BoundedPayloadEncoding::TooLarge,
        Err(_) => BoundedPayloadEncoding::SerializationFailed,
    }
}

/// Fan an event out to every sink, serializing the payload ONCE so each sink
/// emits byte-identical JSON.
///
/// `sid` is `None` for agent-level events, `Some(session_id)` for
/// session-scoped events. `type_` is the `acp:*` event name (with prefix).
/// `payload` is serialized to a `serde_json::Value` here, then handed to each
/// sink by reference — `TauriEventSink` re-serializes via `app.emit` (which
/// accepts a `Value` directly) and `WsRelaySink` records the `Value` as-is.
///
/// Returns early without emitting when `sinks` is empty (avoids a wasted
/// serialization + allocation on the `vec![]` path blessed for unit tests) or
/// when the payload fails to serialize. A serialization failure is logged and
/// the event is dropped — preserving the old `events::emit` drop-and-log
/// semantics (a non-JSON-serializable payload must NOT be emitted as a `null`
/// payload on the wire).
pub fn fan_out<P: Serialize>(
    sinks: &[Arc<dyn EventSink>],
    sid: Option<&str>,
    type_: &'static str,
    payload: &P,
) -> Result<FanOutReceipt, FanOutError> {
    if sinks.is_empty() {
        return Ok(FanOutReceipt {
            sink_count: 0,
            delivered_count: 0,
            durable_admission_count: 0,
            session_seq: None,
        });
    }
    match classify_payload_encoding(payload, crate::conversation::MAX_CONVERSATION_RECORD_BYTES) {
        BoundedPayloadEncoding::WithinLimit => {}
        BoundedPayloadEncoding::TooLarge => {
            let error = EventSinkError::delivery_failed(
                "relay history record exceeds the 262144-byte host bound",
            );
            return Err(FanOutError {
                code: error.code,
                source_code: error.source_code,
                durable_rejection: error.durable_rejection,
                delivered_count: 0,
                detail: error.detail,
            });
        }
        BoundedPayloadEncoding::SerializationFailed => {
            log::error!("[acp] skipping {type_} event: payload failed to serialize");
            let error = EventSinkError::serialization_failed();
            return Err(FanOutError {
                code: error.code,
                source_code: error.source_code,
                durable_rejection: error.durable_rejection,
                delivered_count: 0,
                detail: error.detail,
            });
        }
    }
    let payload = serde_json::to_value(payload).map_err(|error| {
        log::error!("[acp] skipping {type_} event: payload failed to serialize: {error}");
        let error = EventSinkError::serialization_failed();
        FanOutError {
            code: error.code,
            source_code: error.source_code,
            durable_rejection: error.durable_rejection,
            delivered_count: 0,
            detail: error.detail,
        }
    })?;
    let event = AcpEvent {
        sid: sid.map(str::to_string),
        type_,
        payload,
    };
    let mut delivered_count = 0usize;
    let mut durable_admission_count = 0usize;
    let mut session_seq = None;
    for priority in [
        EventSinkPriority::DurableAdmission,
        EventSinkPriority::LiveDelivery,
    ] {
        for sink in sinks.iter().filter(|sink| sink.priority() == priority) {
            match sink.emit(&event) {
                Ok(receipt) => {
                    delivered_count += usize::from(receipt.delivered);
                    durable_admission_count += usize::from(receipt.durable_admission);
                    if receipt.session_seq.is_some() {
                        session_seq = receipt.session_seq;
                    }
                }
                Err(error) => {
                    return Err(FanOutError {
                        code: error.code,
                        source_code: error.source_code,
                        durable_rejection: error.durable_rejection,
                        delivered_count,
                        detail: error.detail,
                    });
                }
            }
        }
    }
    Ok(FanOutReceipt {
        sink_count: sinks.len(),
        delivered_count,
        durable_admission_count,
        session_seq,
    })
}

/// Broadcast a `projects_changed` agent-level event to every connected client.
///
/// Called by the `remote_sync_projects` command (desktop-hosted push — the
/// desktop's active IS the default) and the explicit `set_default_project`
/// operation (Tauri command + WS request + HTTP route) after they update the
/// [`crate::web::project_registry::ProjectRegistry`]. The event is agent-level
/// (`sid: None`, `seq: 0`) so [`WsRelaySink::emit`] fans it out to ALL connected
/// clients (the wire `type` is `projects_changed` — the `acp:` prefix is
/// stripped by `emit`). The payload carries only the new `defaultProjectId`;
/// the web client refetches `GET /projects` for the full list. On the initial
/// load a client seeds `activeProjectId` from `defaultProjectId`; on subsequent
/// events it preserves its own `activeProjectId` (no silent retarget).
///
/// `default_project_id` is `None` when the host has no default project.
pub fn broadcast_projects_changed(relay: &Arc<WsRelaySink>, default_project_id: Option<&str>) {
    // Use the typed `ProjectsChangedPayload` (single source of truth for the
    // wire shape) rather than hand-rolled `json!` — its `skip_serializing_if`
    // omits `defaultProjectId` when `None` (the web client ignores the payload
    // + refetches `GET /projects`, so omit-vs-null is cosmetic, but the
    // struct stays the canonical shape if fields are added later).
    let payload = ProjectsChangedPayload {
        default_project_id: default_project_id.map(str::to_string),
    };
    // Clone into a concrete `Arc<WsRelaySink>` first so `Arc::clone` infers
    // `T = WsRelaySink` (not `dyn EventSink`); the unsized coercion to
    // `Arc<dyn EventSink>` then happens at the vec push.
    let relay_arc: Arc<WsRelaySink> = Arc::clone(relay);
    let sinks: Vec<Arc<dyn EventSink>> = vec![relay_arc];
    if let Err(error) = fan_out(&sinks, None, "acp:projects_changed", &payload) {
        log::warn!(
            "[ws-relay] projects_changed delivery degraded code={}",
            error.code
        );
    }
}

/// Broadcast a `chat_history_changed` agent-level event to every connected
/// client.
///
/// Called after desktop history mutations or compatibility sync requests. The event is
/// agent-level (`sid: None`, `seq: 0`) so [`WsRelaySink::emit`] fans it out to
/// ALL connected clients (the wire `type` is `chat_history_changed` — the
/// `acp:` prefix is stripped by `emit`). The payload is empty `{}`; the web
/// client refetches the session index (`list_persisted_sessions`) for the full
/// list (the desktop is the source of truth).
pub fn broadcast_chat_history_changed(relay: &Arc<WsRelaySink>) {
    let payload = serde_json::json!({});
    let relay_arc: Arc<WsRelaySink> = Arc::clone(relay);
    let sinks: Vec<Arc<dyn EventSink>> = vec![relay_arc];
    if let Err(error) = fan_out(&sinks, None, "acp:chat_history_changed", &payload) {
        log::warn!(
            "[ws-relay] chat_history_changed delivery degraded code={}",
            error.code
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::session_persistence::{
        now_millis, PersistedEventRecord, SessionPersistence, SessionRegistration,
        SESSION_SCHEMA_VERSION,
    };
    use chrono::Utc;
    use serde::Serialize;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Drain a receiver into a Vec in arrival order (test helper for the live
    /// relay API — replaces the old `WsRelaySink::drain` recorder).
    fn drain_rx(rx: &mut ClientEventReceiver) -> Vec<SequencedEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    /// A minimal serializable payload for sink tests — exercises the same
    /// `serde_json::to_value` path the real event structs use.
    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestPayload {
        agent_id: String,
        session_id: String,
        message: String,
    }

    impl TestPayload {
        fn new(agent: &str, session: &str, msg: &str) -> Self {
            Self {
                agent_id: agent.to_string(),
                session_id: session.to_string(),
                message: msg.to_string(),
            }
        }
    }

    #[test]
    fn unbound_session_does_not_open_delivery_circuit() {
        let unbound = FanOutError {
            code: CONVERSATION_PERSISTENCE_REJECTED,
            source_code: Some(CONVERSATION_BINDING_NOT_FOUND),
            durable_rejection: true,
            delivered_count: 0,
            detail: "unbound".to_string(),
        };
        assert!(unbound.is_unbound_session());
        assert!(!unbound.should_open_session_circuit());

        let fatal = FanOutError {
            code: CONVERSATION_PERSISTENCE_REJECTED,
            source_code: Some("CONVERSATION_RECOVERY_REQUIRED"),
            durable_rejection: true,
            delivered_count: 0,
            detail: "recovery".to_string(),
        };
        assert!(fatal.should_open_session_circuit());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn durable_rejection_does_not_advance_live_frontier() {
        struct CapturingLiveSink {
            seen: Mutex<Vec<AcpEvent>>,
        }

        impl EventSink for CapturingLiveSink {
            fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError> {
                self.seen.lock().push(event.clone());
                Ok(EventDeliveryReceipt::delivered(None, false))
            }
        }

        let (root, repository, adapter, _conversation_id) =
            conversation_fixture("durable-rejection", "mapped-session").await;
        let relay = Arc::new(WsRelaySink::with_conversation_persistence(
            16, adapter, None,
        ));
        let live = Arc::new(CapturingLiveSink {
            seen: Mutex::new(Vec::new()),
        });
        let (_client, mut rx, replay) = relay.subscribe("unmapped-session", None).await;
        assert_eq!(replay, ReplayResult::Ok(0));
        let sinks: Vec<Arc<dyn EventSink>> = vec![live.clone(), relay.clone()];
        let before = relay
            .ordered_conversation_persistence()
            .expect("ordered Conversation persistence")
            .metrics();

        let error = fan_out(
            &sinks,
            Some("unmapped-session"),
            "acp:message_chunk",
            &TestPayload::new("agent", "unmapped-session", "rejected"),
        )
        .expect_err("unmapped durable admission must fail closed");

        assert_eq!(error.code, CONVERSATION_PERSISTENCE_REJECTED);
        assert!(error.is_durable_rejection());
        assert_eq!(error.delivered_count, 0);
        assert_eq!(relay.session_watermark("unmapped-session"), 0);
        assert!(drain_rx(&mut rx).is_empty());
        assert!(live.seen.lock().is_empty());
        let after = relay
            .ordered_conversation_persistence()
            .expect("ordered Conversation persistence")
            .metrics();
        assert_eq!(after.pending_records, before.pending_records);
        assert_eq!(after.pending_bytes, before.pending_bytes);
        assert_eq!(relay.relay_history_stats().reserved_bytes, 0);

        relay
            .shutdown_conversation_persistence()
            .await
            .expect("ordered persistence shutdown");
        drop(relay);
        drop(repository);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(start_paused = true)]
    async fn sixty_four_sessions_share_one_connection_outbound_budget() {
        let relay = Arc::new(WsRelaySink::new());
        let (client, _rx, replay) = relay.subscribe("aggregate-0", None).await;
        assert_eq!(replay, ReplayResult::Ok(0));
        for ordinal in 1..MAX_CONNECTION_SUBSCRIPTIONS {
            assert_eq!(
                relay
                    .subscribe_existing(client, &format!("aggregate-{ordinal}"), None)
                    .await,
                ReplayResult::Ok(0)
            );
        }
        assert_eq!(
            relay.clients.lock().get(&client).unwrap().sessions.len(),
            MAX_CONNECTION_SUBSCRIPTIONS
        );
        let sinks: Vec<Arc<dyn EventSink>> = vec![relay.clone()];
        for ordinal in 0..=CLIENT_OUTBOUND_RECORDS {
            let sid = format!("aggregate-{}", ordinal % MAX_CONNECTION_SUBSCRIPTIONS);
            fan_out(
                &sinks,
                Some(&sid),
                "acp:permission_request",
                &TestPayload::new("agent", &sid, &format!("reliable-{ordinal}")),
            )
            .unwrap();
        }
        let stats = relay.client_outbound_stats(client).unwrap();
        assert!(stats.records <= CLIENT_OUTBOUND_RECORDS);
        assert!(stats.bytes <= CLIENT_OUTBOUND_BYTES);
        assert!(stats.max_records <= CLIENT_OUTBOUND_RECORDS);
        assert!(stats.max_bytes <= CLIENT_OUTBOUND_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn production_slow_client_is_bounded() {
        let relay = Arc::new(WsRelaySink::new());
        let (client, _rx, replay) = relay.subscribe("slow-client-session", None).await;
        assert_eq!(replay, ReplayResult::Ok(0));
        let sinks: Vec<Arc<dyn EventSink>> = vec![relay.clone()];

        for ordinal in 0..=CLIENT_OUTBOUND_RECORDS {
            fan_out(
                &sinks,
                Some("slow-client-session"),
                "acp:permission_request",
                &TestPayload::new(
                    "agent",
                    "slow-client-session",
                    &format!("reliable-{ordinal}"),
                ),
            )
            .expect("reliable relay admission remains bounded");
        }

        let stats = relay
            .client_outbound_stats(client)
            .expect("slow client remains registered until timeout");
        assert!(stats.records <= CLIENT_OUTBOUND_RECORDS);
        assert!(stats.bytes <= CLIENT_OUTBOUND_BYTES);
        assert!(stats.max_records <= CLIENT_OUTBOUND_RECORDS);
        assert!(stats.max_bytes <= CLIENT_OUTBOUND_BYTES);
        assert!(relay.has_client(client));

        tokio::task::yield_now().await;
        tokio::time::advance(RELIABLE_CLIENT_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert!(
            !relay.has_client(client),
            "reliable saturation disconnects at the five-second deadline"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lru_eviction_retires_all_auxiliary_state_idempotently() {
        let root = temp_dir("global-history-lru");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd).expect("session cwd");
        let persistence = SessionPersistence::open(root.join("sessions"))
            .await
            .expect("legacy persistence open");
        for ordinal in 0..=MAX_RELAY_SESSIONS {
            persistence
                .register_session(SessionRegistration {
                    session_id: format!("lru-session-{ordinal}"),
                    stable_agent_namespace: None,
                    runtime_agent_id: None,
                    project_id: None,
                    cwd: cwd.clone(),
                    ..Default::default()
                })
                .await
                .expect("session registration");
        }
        let relay = Arc::new(WsRelaySink::with_persistence(1, persistence.clone()));
        let sinks: Vec<Arc<dyn EventSink>> = vec![relay.clone()];
        for ordinal in 0..MAX_RELAY_SESSIONS {
            let sid = format!("lru-session-{ordinal}");
            fan_out(
                &sinks,
                Some(&sid),
                "acp:tool_call",
                &TestPayload::new("agent", &sid, "durable"),
            )
            .expect("durable history admission");
        }
        persistence
            .flush_all()
            .await
            .expect("durable frontier flush");

        relay.turn_watermark().mark_seen("lru-session-0", "turn-0");
        relay
            .delivery_circuits
            .lock()
            .insert("lru-session-0".to_string(), "TEST_FATAL");
        let newest = format!("lru-session-{MAX_RELAY_SESSIONS}");
        fan_out(
            &sinks,
            Some(&newest),
            "acp:tool_call",
            &TestPayload::new("agent", &newest, "evicts-oldest"),
        )
        .expect("LRU admission after durable eviction");
        persistence.flush_all().await.expect("newest durable flush");

        let stats = relay.relay_history_stats();
        assert!(stats.sessions <= MAX_RELAY_SESSIONS);
        assert!(stats.bytes <= MAX_RELAY_BYTES);
        assert_eq!(stats.reserved_bytes, 0);
        let (_client, mut replay_rx, replay) = relay.subscribe("lru-session-0", Some(0)).await;
        assert_eq!(replay, ReplayResult::Ok(1));
        let replayed = replay_rx
            .recv()
            .await
            .expect("canonical replay after eviction");
        assert_eq!(replayed.seq, 1);
        assert_eq!(replayed.type_, "tool_call");
        assert_eq!(replayed.payload["agentId"], "agent");
        assert_eq!(replayed.payload["sessionId"], "lru-session-0");
        assert!(
            relay.turn_watermark().is_seen("lru-session-0", "turn-0"),
            "LRU payload eviction must not semantically retire watermarks"
        );
        assert!(
            relay.delivery_circuits.lock().contains_key("lru-session-0"),
            "LRU payload eviction must not semantically retire circuits"
        );
        assert_eq!(
            relay.auxiliary_stats().submission_gate_stripes,
            SESSION_GATE_STRIPES
        );
        assert_eq!(
            relay.auxiliary_stats().replay_gate_stripes,
            SESSION_GATE_STRIPES
        );
        relay.retire_session("lru-session-0").await.unwrap();
        relay.retire_session("lru-session-0").await.unwrap();

        persistence
            .shutdown()
            .await
            .expect("legacy persistence shutdown");
        drop(relay);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn forget_session_removes_relay_subscription_and_replay_state() {
        let ws = Arc::new(WsRelaySink::new());
        let (client, _rx, _) = ws.subscribe("temp", Some(0)).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        fan_out(
            &sinks,
            Some("temp"),
            "acp:message_chunk",
            &TestPayload::new("a1", "temp", "secret"),
        )
        .expect("message chunk fan-out");
        ws.turn_watermark().mark_seen("temp", "turn-1");
        assert_eq!(ws.session_watermark("temp"), 1);
        assert_eq!(ws.session_subscriber_count("temp"), 1);
        assert!(ws.turn_watermark().is_seen("temp", "turn-1"));
        assert_eq!(ws.persistence_submission_gates.len(), SESSION_GATE_STRIPES);
        assert_eq!(ws.replay_gates.len(), SESSION_GATE_STRIPES);

        ws.forget_session("temp").await;

        assert_eq!(ws.session_watermark("temp"), 0);
        assert_eq!(ws.session_subscriber_count("temp"), 0);
        assert!(!ws.clients.lock().contains_key(&client));
        assert!(!ws.turn_watermark().is_seen("temp", "turn-1"));
        assert_eq!(ws.persistence_submission_gates.len(), SESSION_GATE_STRIPES);
        assert_eq!(ws.replay_gates.len(), SESSION_GATE_STRIPES);
    }

    /// AC: `WsRelaySink` delivers session + agent-level events in emission
    /// order to a subscribed client (Story 1.4 live API; was Task 8.1).
    #[tokio::test]
    async fn ws_relay_sink_delivers_events_in_order() {
        let ws = Arc::new(WsRelaySink::new());
        // Subscribe BEFORE emitting so the client receives events live.
        let (client, mut rx, replay) = ws.subscribe("sess-1", None).await;
        assert_eq!(replay, ReplayResult::Ok(0), "fresh session has no replay");
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        fan_out(
            &sinks,
            Some("sess-1"),
            "acp:message_chunk",
            &TestPayload::new("a1", "sess-1", "first"),
        )
        .expect("first message fan-out");
        fan_out(
            &sinks,
            Some("sess-1"),
            "acp:message_chunk",
            &TestPayload::new("a1", "sess-1", "second"),
        )
        .expect("second message fan-out");
        fan_out(
            &sinks,
            None,
            "acp:agent_disconnected",
            &TestPayload::new("a1", "sess-1", "third"),
        )
        .expect("agent disconnect fan-out");

        // Lossy events are pushed + flushed to the channel on enqueue (AC5/AC6),
        // so an explicit flush is a no-op here; reliable agent_disconnected is last.
        assert_eq!(
            ws.flush_lossy(client),
            0,
            "lossy ring already drained on enqueue"
        );

        let drained = drain_rx(&mut rx);
        assert_eq!(drained.len(), 3, "exactly three events were delivered");
        // Session-scoped events get monotonic seq; agent-level gets seq=0.
        assert_eq!(drained[0].type_, "message_chunk");
        assert_eq!(drained[0].sid.as_deref(), Some("sess-1"));
        assert_eq!(drained[0].seq, 1);
        assert_eq!(drained[0].payload["message"], "first");
        assert_eq!(drained[1].seq, 2);
        assert_eq!(drained[1].payload["message"], "second");
        assert_eq!(drained[2].type_, "agent_disconnected");
        assert_eq!(drained[2].seq, 0);
        assert!(
            drained[2].sid.is_none(),
            "agent-level event must carry no sid"
        );
        // camelCase wire shape is preserved end-to-end (AC3).
        assert_eq!(drained[0].payload["agentId"], "a1");
        assert_eq!(drained[0].payload["sessionId"], "sess-1");
    }

    /// AC: `WsRelaySink` + `TauriEventSink` in the same fan-out both receive
    /// the SAME payload (Story 1.1 byte-identity invariant). We can't
    /// construct a real `AppHandle` in a unit test, so a custom sink records
    /// the `AcpEvent` the way `TauriEventSink` would emit it; we then assert
    /// the WS relay delivered an identical `Value` to a subscribed client.
    /// The relay strips the `acp:` prefix from `type_` (AC2) but passes the
    /// `payload` `Value` through verbatim (AC3 — byte-identity invariant).
    #[tokio::test]
    async fn fan_out_delivers_identical_payload_to_every_sink() {
        /// A second recorder used as a stand-in for `TauriEventSink`'s view of
        /// the event (we can't build a real `AppHandle` here). It captures the
        /// exact `AcpEvent` handed to `emit`.
        struct CapturingSink {
            seen: Mutex<Vec<AcpEvent>>,
        }
        impl EventSink for CapturingSink {
            fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError> {
                self.seen.lock().push(event.clone());
                Ok(EventDeliveryReceipt::delivered(None, false))
            }
        }

        let ws = Arc::new(WsRelaySink::new());
        let tauri_stand_in = Arc::new(CapturingSink {
            seen: Mutex::new(Vec::new()),
        });
        let sinks: Vec<Arc<dyn EventSink>> = vec![tauri_stand_in.clone(), ws.clone()];

        // Subscribe BEFORE emitting so the WS client receives the event live.
        let (_client, mut rx, _replay) = ws.subscribe("sess-7", None).await;

        fan_out(
            &sinks,
            Some("sess-7"),
            "acp:tool_call",
            &TestPayload::new("a2", "sess-7", "hello"),
        )
        .expect("tool-call fan-out");

        let tauri_view = tauri_stand_in.seen.lock().drain(..).collect::<Vec<_>>();
        let ws_view = drain_rx(&mut rx);

        assert_eq!(tauri_view.len(), 1);
        assert_eq!(ws_view.len(), 1);
        // The relay strips the `acp:` prefix from the WS event type (AC2).
        assert_eq!(tauri_view[0].type_, "acp:tool_call");
        assert_eq!(ws_view[0].type_, "tool_call");
        assert_eq!(ws_view[0].sid.as_deref(), tauri_view[0].sid.as_deref());
        assert_eq!(
            ws_view[0].payload, tauri_view[0].payload,
            "both sinks must see the SAME serialized Value (serialize-once-fan-out-N)"
        );
        assert_eq!(ws_view[0].payload["message"], "hello");
    }

    /// `fan_out` with an empty sink list is a no-op (the dispatcher must not
    /// panic when constructed with `vec![]`, e.g. in unit tests of the manager).
    #[test]
    fn fan_out_with_no_sinks_is_a_no_op() {
        let sinks: Vec<Arc<dyn EventSink>> = vec![];
        fan_out(
            &sinks,
            Some("sess-x"),
            "acp:message_chunk",
            &TestPayload::new("a", "sess-x", "m"),
        )
        .expect("empty fan-out is a successful no-op");
        // No panic, no assertion needed beyond reaching this point.
    }

    /// `WsRelaySink` live receiver drains only currently-queued events;
    /// subsequent emits produce new events on the next drain (AC6).
    #[tokio::test]
    async fn ws_relay_sink_live_drain_is_incremental() {
        let ws = Arc::new(WsRelaySink::new());
        let (client, mut rx, _replay) = ws.subscribe("sess-d", None).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        fan_out(
            &sinks,
            Some("sess-d"),
            "acp:message_chunk",
            &TestPayload::new("a", "sess-d", "m1"),
        )
        .expect("first incremental fan-out");
        // Lossy events are flushed to the channel on enqueue.
        assert_eq!(ws.lossy_ring_len_for_test(client), 0);
        let first = drain_rx(&mut rx);
        assert_eq!(first.len(), 1);
        // A second drain without a new emit yields nothing.
        let between = drain_rx(&mut rx);
        assert!(
            between.is_empty(),
            "drain must not re-deliver already-drained events"
        );
        fan_out(
            &sinks,
            Some("sess-d"),
            "acp:message_chunk",
            &TestPayload::new("a", "sess-d", "m2"),
        )
        .expect("second incremental fan-out");
        let second = drain_rx(&mut rx);
        assert_eq!(second.len(), 1, "a new emit must produce a new event");
        assert_eq!(second[0].seq, 2);
    }

    /// A payload whose `Serialize` impl always errors — deterministically
    /// exercises `fan_out`'s serialization-failure branch. The real-world
    /// trigger is an `f64::NaN`/`Infinity` in a field like
    /// `UsageCostEvent.amount`, but a custom failing serializer avoids
    /// depending on `serde_json`'s float policy.
    struct AlwaysFailsPayload;
    impl Serialize for AlwaysFailsPayload {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(<S::Error as serde::ser::Error>::custom(
                "intentional serialization failure for test",
            ))
        }
    }

    /// P1: a serialization failure must NOT emit a `null` payload on the wire
    /// — the event is dropped (preserving the old `events::emit` semantics).
    #[tokio::test]
    async fn fan_out_skips_emission_when_payload_fails_to_serialize() {
        let ws = Arc::new(WsRelaySink::new());
        let (_client, mut rx, _replay) = ws.subscribe("sess-nan", None).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        let error = fan_out(
            &sinks,
            Some("sess-nan"),
            "acp:usage_update",
            &AlwaysFailsPayload,
        )
        .expect_err("serialization failure must be surfaced");
        assert_eq!(error.code, EVENT_SERIALIZATION_FAILED);
        assert!(
            drain_rx(&mut rx).is_empty(),
            "serialization failure must not emit a null payload"
        );
    }

    /// Mirrors the real event structs' `#[serde(skip_serializing_if = ...)]`
    /// pattern (e.g. `SessionCreatedEvent`/`AgentErrorEvent`/`UsageUpdateEvent`)
    /// without coupling this test to `crate::acp::events` internals.
    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SkipIfPayload {
        agent_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        optional_field: Option<String>,
    }

    /// P2: the `Value` produced by `fan_out` must match a direct
    /// `serde_json::to_value` of the same struct, including `skip_serializing_if`
    /// fields (a `None` optional field must be ABSENT, not emitted as `null`).
    /// This guards against any future `Value`-intermediate regression that would
    /// silently break byte-identity for real event structs.
    #[tokio::test]
    async fn fan_out_preserves_skip_serializing_if_byte_identity() {
        let ws = Arc::new(WsRelaySink::new());
        let (_client, mut rx, _replay) = ws.subscribe("sess-skip", None).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        let payload = SkipIfPayload {
            agent_id: "a1".to_string(),
            optional_field: None,
        };
        let direct = serde_json::to_value(&payload).unwrap();
        fan_out(&sinks, Some("sess-skip"), "acp:session_created", &payload)
            .expect("skip-serialization fan-out");
        let recorded = drain_rx(&mut rx);
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].payload, direct,
            "fan_out's Value must match direct to_value, including skip_serializing_if"
        );
        assert!(
            recorded[0].payload.get("optionalField").is_none(),
            "skipped Option::None field must be absent from the wire payload, not null"
        );
        assert_eq!(recorded[0].payload["agentId"], "a1");
    }

    /// P2: compile-time proof that `EventSink` and its implementations are
    /// `Send + Sync` (the trait requires it, so `Arc<dyn EventSink>` can cross
    /// from the Tauri command thread into each agent's dedicated driver
    /// thread). A future field change that breaks this would fail to compile.
    #[test]
    fn event_sink_trait_and_impls_are_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<AcpEvent>();
        assert_send_sync::<WsRelaySink>();
        assert_send_sync::<dyn EventSink>();
        assert_send_sync::<Arc<dyn EventSink>>();
        assert_send_sync::<Vec<Arc<dyn EventSink>>>();
    }

    /// AC11: bounded per-session ring evicts oldest events and bumps `base_seq`.
    #[tokio::test]
    async fn event_log_evicts_oldest_when_over_capacity() {
        let ws = Arc::new(WsRelaySink::with_capacity(2, 256));
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        for msg in ["a", "b", "c"] {
            fan_out(
                &sinks,
                Some("sess-evict"),
                "acp:tool_call",
                &TestPayload::new("a1", "sess-evict", msg),
            )
            .expect("eviction fan-out");
        }
        // Cursor pointing at evicted seq 0 must be stale (base_seq is now 2;
        // next wanted seq 1 was evicted).
        let (_c, mut rx, replay) = ws.subscribe("sess-evict", Some(0)).await;
        assert_eq!(replay, ReplayResult::Stale);
        assert!(drain_rx(&mut rx).is_empty());

        // Cursor at seq 1 → next wanted is 2, still in the ring → replay 2+3.
        let (_c2, mut rx2, replay2) = ws.subscribe("sess-evict", Some(1)).await;
        assert_eq!(replay2, ReplayResult::Ok(2));
        let drained2 = drain_rx(&mut rx2);
        assert_eq!(drained2.len(), 2);
        assert_eq!(drained2[0].seq, 2);
        assert_eq!(drained2[1].seq, 3);

        // Cursor at seq 2 → replay only seq 3.
        let (_c3, mut rx3, replay3) = ws.subscribe("sess-evict", Some(2)).await;
        assert_eq!(replay3, ReplayResult::Ok(1));
        let drained = drain_rx(&mut rx3);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].seq, 3);
        assert_eq!(drained[0].payload["message"], "c");
    }

    fn temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("se-manager-sink-{label}-{stamp}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn durable_replay_catches_more_than_ring_capacity_then_streams_live() {
        let root = temp_dir("replay-catchup");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let persistence = SessionPersistence::open(root.join("sessions"))
            .await
            .unwrap();
        persistence
            .register_session(SessionRegistration {
                session_id: "sess-durable".to_string(),
                stable_agent_namespace: None,
                runtime_agent_id: None,
                project_id: None,
                cwd,
                ..Default::default()
            })
            .await
            .unwrap();
        let relay = Arc::new(WsRelaySink::with_persistence(2, persistence.clone()));
        let sinks: Vec<Arc<dyn EventSink>> = vec![relay.clone()];
        for index in 1..=2 {
            fan_out(
                &sinks,
                Some("sess-durable"),
                "acp:tool_call",
                &TestPayload::new("a", "sess-durable", &index.to_string()),
            )
            .expect("initial durable replay fan-out");
        }
        persistence.flush_session("sess-durable").await.unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let hook = crate::acp::session_persistence::ReplayTestHook::new(entered_tx);
        persistence.set_replay_test_hook(hook.clone());
        let subscribe_relay = relay.clone();
        let subscribe =
            tokio::spawn(async move { subscribe_relay.subscribe("sess-durable", Some(0)).await });
        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();
        // The first disk snapshot is now blocked. Inject more than ring capacity,
        // forcing the handoff to detect missing 3..8 and retry durable replay.
        for index in 3..=8 {
            fan_out(
                &sinks,
                Some("sess-durable"),
                "acp:tool_call",
                &TestPayload::new("a", "sess-durable", &index.to_string()),
            )
            .expect("concurrent durable replay fan-out");
        }
        hook.release();
        let (_client, mut rx, replay) = subscribe.await.unwrap();
        assert_eq!(replay, ReplayResult::Ok(8));
        let replayed = drain_rx(&mut rx);
        assert_eq!(
            replayed.iter().map(|event| event.seq).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        fan_out(
            &sinks,
            Some("sess-durable"),
            "acp:tool_call",
            &TestPayload::new("a", "sess-durable", "live"),
        )
        .expect("post-replay live fan-out");
        assert_eq!(rx.recv().await.unwrap().seq, 9);
        persistence.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// AC11: reconnect with `last_seq` replays the log tail then streams live.
    #[tokio::test]
    async fn cursor_replay_then_live() {
        let ws = Arc::new(WsRelaySink::new());
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        fan_out(
            &sinks,
            Some("sess-rp"),
            "acp:tool_call",
            &TestPayload::new("a1", "sess-rp", "one"),
        )
        .expect("first replay fan-out");
        fan_out(
            &sinks,
            Some("sess-rp"),
            "acp:tool_call",
            &TestPayload::new("a1", "sess-rp", "two"),
        )
        .expect("second replay fan-out");

        let (_c, mut rx, replay) = ws.subscribe("sess-rp", Some(1)).await;
        assert_eq!(replay, ReplayResult::Ok(1));
        let replayed = drain_rx(&mut rx);
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].seq, 2);
        assert_eq!(replayed[0].payload["message"], "two");

        fan_out(
            &sinks,
            Some("sess-rp"),
            "acp:tool_call",
            &TestPayload::new("a1", "sess-rp", "three"),
        )
        .expect("live replay fan-out");
        let live = drain_rx(&mut rx);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].seq, 3);
    }

    /// AC11: lossy ring drop-oldest under pressure (ring filled without flush).
    #[tokio::test]
    async fn lossy_ring_drop_oldest_under_pressure() {
        let ws = Arc::new(WsRelaySink::with_capacity(4096, 2));
        let (client, mut rx, _) = ws.subscribe("sess-lossy", None).await;
        for i in 1..=5 {
            let se = SequencedEvent::new(
                Some("sess-lossy".to_string()),
                i,
                "message_chunk",
                serde_json::json!({"message": format!("m{i}")}),
            );
            ws.push_lossy_no_flush_for_test(client, se);
        }
        assert_eq!(
            ws.lossy_ring_len_for_test(client),
            2,
            "capacity 2 keeps only newest"
        );
        assert_eq!(ws.flush_lossy(client), 2);
        let drained = drain_rx(&mut rx);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].seq, 4);
        assert_eq!(drained[1].seq, 5);
    }

    /// AC11: reliable events are never dropped even when lossy ring is full.
    #[tokio::test]
    async fn reliable_events_never_dropped() {
        let ws = Arc::new(WsRelaySink::with_capacity(4096, 1));
        let (client, mut rx, _) = ws.subscribe("sess-rel", None).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        // Fill lossy ring without flush, then emit a reliable event.
        ws.push_lossy_no_flush_for_test(
            client,
            SequencedEvent::new(
                Some("sess-rel".to_string()),
                99,
                "message_chunk",
                serde_json::json!({"message": "buffered"}),
            ),
        );
        fan_out(
            &sinks,
            Some("sess-rel"),
            "acp:permission_request",
            &TestPayload::new("a1", "sess-rel", "must-arrive"),
        )
        .expect("reliable permission fan-out");
        let drained = drain_rx(&mut rx);
        assert!(
            drained.iter().any(|e| e.type_ == "permission_request"),
            "reliable event must be delivered"
        );
        assert!(
            drained.iter().any(|e| e.type_ == "message_chunk"),
            "buffered lossy is flushed before the reliable event"
        );
    }

    /// AC11: client on session A does not receive session B events.
    #[tokio::test]
    async fn cross_session_isolation() {
        let ws = Arc::new(WsRelaySink::new());
        let (_ca, mut rx_a, _) = ws.subscribe("sess-a", None).await;
        let (_cb, mut rx_b, _) = ws.subscribe("sess-b", None).await;
        let sinks: Vec<Arc<dyn EventSink>> = vec![ws.clone()];
        fan_out(
            &sinks,
            Some("sess-a"),
            "acp:tool_call",
            &TestPayload::new("a1", "sess-a", "only-a"),
        )
        .expect("session A fan-out");
        fan_out(
            &sinks,
            Some("sess-b"),
            "acp:tool_call",
            &TestPayload::new("a1", "sess-b", "only-b"),
        )
        .expect("session B fan-out");
        let a = drain_rx(&mut rx_a);
        let b = drain_rx(&mut rx_b);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].payload["message"], "only-a");
        assert_eq!(b[0].payload["message"], "only-b");
        assert_eq!(a[0].sid.as_deref(), Some("sess-a"));
        assert_eq!(b[0].sid.as_deref(), Some("sess-b"));
    }

    /// Epic-4 bridge: `broadcast_projects_changed` fans an agent-level
    /// `projects_changed` event (sid=null, seq=0) to every connected client.
    /// A client subscribed to ANY session receives it (the web client then
    /// refetches `GET /projects`).
    #[tokio::test]
    async fn broadcast_projects_changed_reaches_subscribed_client() {
        let relay = Arc::new(WsRelaySink::new());
        // Subscribe a client to a session so it is in the relay's client set.
        let (_client, mut rx, _replay) = relay.subscribe("sess-1", None).await;

        broadcast_projects_changed(&relay, Some("p-3"));

        let drained = drain_rx(&mut rx);
        assert_eq!(drained.len(), 1, "exactly one projects_changed event");
        let evt = &drained[0];
        assert_eq!(evt.type_, "projects_changed");
        assert!(evt.sid.is_none(), "agent-level event: sid must be null");
        assert_eq!(evt.seq, 0, "agent-level event: seq must be 0");
        assert_eq!(evt.payload["defaultProjectId"], "p-3");
    }

    /// `broadcast_projects_changed` with no default project still fans out;
    /// the `ProjectsChangedPayload` struct's `skip_serializing_if` OMITS the
    /// `defaultProjectId` key entirely (not `null`).
    #[tokio::test]
    async fn broadcast_projects_changed_null_default_id() {
        let relay = Arc::new(WsRelaySink::new());
        let (_client, mut rx, _replay) = relay.subscribe("sess-1", None).await;

        broadcast_projects_changed(&relay, None);

        let drained = drain_rx(&mut rx);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].type_, "projects_changed");
        // `skip_serializing_if = "Option::is_none"` → the key is omitted, not null.
        assert!(
            drained[0].payload.get("defaultProjectId").is_none(),
            "defaultProjectId must be omitted (not null) when None"
        );
    }

    /// `broadcast_chat_history_changed` fans an agent-level event (`sid: None`,
    /// `seq: 0`) to every connected client so the web sidebar refetches the
    /// session index. Mirrors `broadcast_projects_changed`.
    #[tokio::test]
    async fn broadcast_chat_history_changed_reaches_subscribed_client() {
        let relay = Arc::new(WsRelaySink::new());
        let (_client, mut rx, _replay) = relay.subscribe("sess-1", None).await;

        broadcast_chat_history_changed(&relay);

        let drained = drain_rx(&mut rx);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].type_, "chat_history_changed");
        assert_eq!(drained[0].seq, 0, "agent-level event: seq must be 0");
        // The payload is empty `{}` — the web client refetches the index.
        assert!(drained[0].payload.as_object().unwrap().is_empty());
    }

    /// CAP-2: with host persistence attached, session lifecycle events fan an
    /// agent-level `chat_history_changed` so connected sidebars refetch the
    /// host-owned index (browser-origin sessions never flow through a desktop
    /// renderer save).
    #[tokio::test]
    async fn session_lifecycle_broadcasts_history_changed_when_persistent() {
        let root = std::env::temp_dir().join(format!(
            "se-manager-sink-history-broadcast-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let persistence = SessionPersistence::open(root.join("sessions"))
            .await
            .unwrap();
        persistence
            .register_session(SessionRegistration {
                session_id: "sess-1".to_string(),
                stable_agent_namespace: None,
                runtime_agent_id: None,
                project_id: None,
                cwd,
                ..Default::default()
            })
            .await
            .unwrap();
        let relay = Arc::new(WsRelaySink::with_persistence(8, persistence.clone()));
        let (_client, mut rx, _replay) = relay.subscribe("sess-1", None).await;

        for type_ in ["acp:session_created", "acp:session_closed"] {
            relay
                .emit(&AcpEvent {
                    sid: Some("sess-1".to_string()),
                    type_,
                    payload: json!({"agentId": "a-1", "sessionId": "sess-1"}),
                })
                .expect("session lifecycle relay admission");
        }

        let drained = drain_rx(&mut rx);
        let notifications = drained
            .iter()
            .filter(|event| event.type_ == "chat_history_changed")
            .count();
        assert_eq!(
            notifications, 2,
            "each lifecycle event fans one chat_history_changed"
        );
        persistence.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Live-only relays (no host persistence) must NOT fan history-changed
    /// notifications — there is no durable index to refetch.
    #[tokio::test]
    async fn session_lifecycle_is_silent_without_persistence() {
        let relay = Arc::new(WsRelaySink::new());
        let (_client, mut rx, _replay) = relay.subscribe("sess-1", None).await;

        relay
            .emit(&AcpEvent {
                sid: Some("sess-1".to_string()),
                type_: "acp:session_closed",
                payload: json!({"agentId": "a-1", "sessionId": "sess-1"}),
            })
            .expect("live-only session close relay admission");

        let drained = drain_rx(&mut rx);
        assert!(
            drained
                .iter()
                .all(|event| event.type_ != "chat_history_changed"),
            "no history notification without durable persistence"
        );
    }

    /// Regression: background title generation writes a durable
    /// `local_title_generated` event directly through
    /// `SessionPersistence::enqueue_event` (advancing durable `last_seq`)
    /// BEFORE the synthetic `session_info_update` reaches the relay. The relay
    /// must reconcile its cached `last_seq` with the durable frontier so it
    /// assigns the NEXT unique seq — not a colliding one that the fail-closed
    /// `append_record` check would reject.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn relay_reconciles_cached_seq_with_durable_frontier_after_background_title() {
        let root = temp_dir("seq-reconcile");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let persistence = SessionPersistence::open(root.join("sessions"))
            .await
            .unwrap();
        persistence
            .register_session(SessionRegistration {
                session_id: "sess-coll".to_string(),
                stable_agent_namespace: None,
                runtime_agent_id: None,
                project_id: None,
                cwd,
                ..Default::default()
            })
            .await
            .unwrap();
        let relay = Arc::new(WsRelaySink::with_persistence(8, persistence.clone()));
        let sinks: Vec<Arc<dyn EventSink>> = vec![relay.clone()];

        // 1. Relay emits a session-scoped event → assigns seq 1 (durable
        //    last_seq advances to 1 via assign_and_append's enqueue).
        fan_out(
            &sinks,
            Some("sess-coll"),
            "acp:tool_call",
            &TestPayload::new("a", "sess-coll", "first"),
        )
        .expect("initial sequence fan-out");
        // Flush so the async durable writer processes the enqueued event
        // before we assert on `last_seq`.
        persistence.flush_session("sess-coll").await.unwrap();
        assert_eq!(relay.session_watermark("sess-coll"), 1);
        assert_eq!(persistence.last_seq("sess-coll").unwrap(), 1);

        // 2. The `set_session_title` MCP tool writes durable seq 2 directly
        //    through `SessionPersistence` (mirrors `record_local_title`).
        //    The relay's cached `last_seq` is still 1.
        let record = PersistedEventRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: "sess-coll".to_string(),
            seq: 2,
            type_: "local_title_generated".to_string(),
            recorded_at: now_millis(),
            payload: json!({"sessionId": "sess-coll", "title": "Generated Title"}),
        };
        persistence.enqueue_event(record).unwrap();
        persistence.flush_session("sess-coll").await.unwrap();
        assert_eq!(persistence.last_seq("sess-coll").unwrap(), 2);

        // 3. Synthetic `session_info_update` through the relay must assign
        //    seq 3 (reconciled: max(cached=1, durable=2) + 1 = 3), NOT a
        //    colliding seq 2. Without reconciliation the durable enqueue
        //    would be rejected by the fail-closed `seq > last_seq` check.
        fan_out(
            &sinks,
            Some("sess-coll"),
            "acp:session_info_update",
            &TestPayload::new("a", "sess-coll", "title-sync"),
        )
        .expect("reconciled sequence fan-out");
        // Flush so the async durable writer processes the enqueued event
        // before we assert on `last_seq`.
        persistence.flush_session("sess-coll").await.unwrap();
        assert_eq!(
            relay.session_watermark("sess-coll"),
            3,
            "relay reconciles cached frontier with durable frontier"
        );
        assert_eq!(
            persistence.last_seq("sess-coll").unwrap(),
            3,
            "durable enqueue accepted (no collision)"
        );

        // 4. Persistence stays healthy: a subsequent flush + direct enqueue
        //    at the next seq succeeds (proving no corrupt/gapped log).
        persistence.flush_session("sess-coll").await.unwrap();
        let next_record = PersistedEventRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: "sess-coll".to_string(),
            seq: 4,
            type_: "tool_call".to_string(),
            recorded_at: now_millis(),
            payload: json!({"sessionId": "sess-coll"}),
        };
        assert!(
            persistence.enqueue_event(next_record).is_ok(),
            "subsequent durable enqueue succeeds (healthy sequence)"
        );

        persistence.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    async fn conversation_fixture(
        label: &str,
        session_id: &str,
    ) -> (
        PathBuf,
        Arc<crate::conversation::ConversationRepository>,
        Arc<crate::conversation::ConversationPersistenceAdapter>,
        crate::conversation::ConversationId,
    ) {
        use crate::conversation::{
            AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
            ConversationLifecycleState, ConversationMutation, ConversationRecordV2,
            ConversationWriter, CreationPartition, ExecutionTarget, ReaderPrecedence,
            AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
        };

        let root = temp_dir(label).canonicalize().unwrap();
        let private = root.join("private");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let (repository, _) = crate::conversation::ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id = crate::conversation::ConversationId::new_v4();
        let created_at = Utc::now();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: workspace.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::InitializingAgent,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        writer
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: session_id.to_string(),
                    runtime_agent_id: "runtime-test".to_string(),
                    stable_agent_namespace: "stable-test".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(crate::conversation::ConversationPersistenceAdapter::new(
            writer, reader,
        ));
        (root, repository, adapter, conversation_id)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn conversation_events_persist_in_emission_order() {
        let (root, repository, adapter, conversation_id) =
            conversation_fixture("ordered-emission", "opaque-ordered-session").await;
        let relay = Arc::new(WsRelaySink::with_conversation_persistence(
            32, adapter, None,
        ));
        let event_types = [
            "acp:message_chunk",
            "acp:tool_call",
            "acp:tool_call_update",
            "acp:prompt_complete",
            "acp:session_info_update",
        ];
        let expected_payloads = (1..=100_u64)
            .map(|ordinal| json!({"ordinal": ordinal, "text": format!("event-{ordinal}")}))
            .collect::<Vec<_>>();
        for (index, payload) in expected_payloads.iter().enumerate() {
            relay
                .emit(&AcpEvent {
                    sid: Some("opaque-ordered-session".to_string()),
                    type_: event_types[index % event_types.len()],
                    payload: payload.clone(),
                })
                .expect("ordered Conversation relay admission");
        }
        relay.flush_conversation_persistence().await.unwrap();
        let ordered = relay.ordered_conversation_persistence().unwrap();
        assert!(ordered.health("opaque-ordered-session").unwrap().is_none());
        assert_eq!(ordered.metrics().pending_records, 0);
        assert_eq!(
            ordered.active_worker_count(),
            crate::conversation::WRITER_SHARDS
        );
        let durable_payloads = repository
            .read_events(conversation_id, 0)
            .unwrap()
            .into_iter()
            .filter(|event| event.payload.get("ordinal").is_some())
            .map(|event| event.payload)
            .collect::<Vec<_>>();
        assert_eq!(durable_payloads, expected_payloads);
        relay.shutdown_conversation_persistence().await.unwrap();
        assert_eq!(ordered.active_worker_count(), 0);
        assert_eq!(ordered.metrics().pending_records, 0);
        drop(relay);
        drop(repository);
        let (restarted_repository, _) =
            crate::conversation::ConversationRepository::open(root.join("private")).unwrap();
        let restarted_payloads = restarted_repository
            .read_events(conversation_id, 0)
            .unwrap()
            .into_iter()
            .filter(|event| event.payload.get("ordinal").is_some())
            .map(|event| event.payload)
            .collect::<Vec<_>>();
        assert_eq!(restarted_payloads, expected_payloads);
        drop(restarted_repository);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_backpressure_and_shutdown_drain() {
        let (root, repository, adapter, conversation_id) =
            conversation_fixture("bounded-drain", "opaque-drain-session").await;
        let relay = Arc::new(WsRelaySink::with_conversation_persistence(
            16, adapter, None,
        ));
        for ordinal in 1..=40_u64 {
            relay
                .emit(&AcpEvent {
                    sid: Some("opaque-drain-session".to_string()),
                    type_: "acp:message_chunk",
                    payload: json!({"ordinal": ordinal}),
                })
                .expect("bounded drain relay admission");
            let pending = relay
                .ordered_conversation_persistence()
                .unwrap()
                .health("opaque-drain-session")
                .unwrap()
                .unwrap()
                .pending_count;
            assert!(pending <= crate::conversation::QUEUE_CAPACITY);
        }
        relay.shutdown_conversation_persistence().await.unwrap();
        let ordered = relay.ordered_conversation_persistence().unwrap();
        assert!(ordered.health("opaque-drain-session").unwrap().is_none());
        assert_eq!(ordered.active_worker_count(), 0);
        assert_eq!(ordered.metrics().pending_records, 0);
        let durable = repository
            .read_events(conversation_id, 0)
            .unwrap()
            .into_iter()
            .filter_map(|event| event.payload.get("ordinal").and_then(Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(durable, (1..=40).collect::<Vec<_>>());
        drop(relay);
        drop(repository);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Title metadata events (`session_info_update` for agent-supplied titles,
    /// `local_title_generated` for background titles) fan a
    /// `chat_history_changed` notification so connected sidebars refetch the
    /// host index and pick up the new title. Extends the
    /// `session_lifecycle_broadcasts_history_changed_when_persistent` coverage.
    #[tokio::test]
    async fn title_metadata_events_broadcast_history_changed_when_persistent() {
        let root = std::env::temp_dir().join(format!(
            "se-manager-sink-title-broadcast-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let persistence = SessionPersistence::open(root.join("sessions"))
            .await
            .unwrap();
        persistence
            .register_session(SessionRegistration {
                session_id: "sess-title".to_string(),
                stable_agent_namespace: None,
                runtime_agent_id: None,
                project_id: None,
                cwd,
                ..Default::default()
            })
            .await
            .unwrap();
        let relay = Arc::new(WsRelaySink::with_persistence(8, persistence.clone()));
        let (_client, mut rx, _replay) = relay.subscribe("sess-title", None).await;

        for type_ in ["acp:session_info_update", "acp:local_title_generated"] {
            relay
                .emit(&AcpEvent {
                    sid: Some("sess-title".to_string()),
                    type_,
                    payload: json!({"agentId": "a-1", "sessionId": "sess-title", "title": "T"}),
                })
                .expect("title metadata relay admission");
        }

        let drained = drain_rx(&mut rx);
        let notifications = drained
            .iter()
            .filter(|event| event.type_ == "chat_history_changed")
            .count();
        assert_eq!(
            notifications, 2,
            "each title metadata event fans one chat_history_changed"
        );
        persistence.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn emit_rejects_oversized_event_before_payload_clone() {
        let relay = WsRelaySink::new();
        let oversized = "x".repeat(crate::conversation::MAX_CONVERSATION_RECORD_BYTES + 8);
        let error = relay
            .emit(&AcpEvent {
                sid: Some("oversize".to_string()),
                type_: "acp:message_chunk",
                payload: json!({ "pad": oversized }),
            })
            .expect_err("oversized payload must reject before live clone");
        assert_eq!(error.code, EVENT_DELIVERY_FAILED);
        assert!(relay.sessions.lock().is_empty());
    }

    #[test]
    fn reserve_history_rejects_at_262144_before_unbounded_serialize() {
        let relay = WsRelaySink::new();
        let oversized = SequencedEvent::new(
            Some("oversize".to_string()),
            1,
            "message_chunk",
            json!({ "pad": "y".repeat(crate::conversation::MAX_CONVERSATION_RECORD_BYTES + 8) }),
        );
        let error = match relay.reserve_history("oversize", &oversized) {
            Ok(_) => panic!("reserve must reject at 262144"),
            Err(error) => error,
        };
        assert_eq!(error.code, EVENT_DELIVERY_FAILED);
        assert!(relay.sessions.lock().is_empty());
    }

    #[test]
    fn next_sequenced_event_consumes_live_watermark_ticket_seq() {
        let relay = WsRelaySink::new();
        {
            let mut sessions = relay.sessions.lock();
            sessions.insert(
                "ticket-sid".to_string(),
                SessionState {
                    last_seq: 9,
                    events: VecDeque::new(),
                    snapshot_events: Vec::new(),
                    base_seq: 1,
                    retained_bytes: 0,
                    reserved_bytes: 0,
                    last_used: 1,
                },
            );
        }
        let event = relay.next_sequenced_event("ticket-sid", "message_chunk", json!({}));
        assert_eq!(
            event.seq,
            crate::conversation::CanonicalSequenceTicket::from_allocated_seq(10).seq,
            "next_sequenced_event must wrap the next live watermark in a repository ticket"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lru_payload_eviction_does_not_retire_recreated_session_semantics() {
        let relay = Arc::new(WsRelaySink::new());
        relay.turn_watermark().mark_seen("keep-semantics", "turn-a");
        relay
            .delivery_circuits
            .lock()
            .insert("keep-semantics".to_string(), "TEST_FATAL");
        {
            let mut sessions = relay.sessions.lock();
            sessions.insert(
                "keep-semantics".to_string(),
                SessionState {
                    last_seq: 1,
                    events: VecDeque::new(),
                    snapshot_events: Vec::new(),
                    base_seq: 1,
                    retained_bytes: 0,
                    reserved_bytes: 0,
                    last_used: 1,
                },
            );
        }
        let event = SequencedEvent::new(
            Some("other".to_string()),
            1,
            "message_chunk",
            json!({ "ok": true }),
        );
        let _ = relay.reserve_history("other", &event);
        assert!(
            relay.turn_watermark().is_seen("keep-semantics", "turn-a"),
            "recreated/active session semantics survive payload eviction"
        );
        assert!(relay
            .delivery_circuits
            .lock()
            .contains_key("keep-semantics"));
    }
}
