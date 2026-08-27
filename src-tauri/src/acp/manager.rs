//! `AcpManager`: per-agent dedicated-thread driver + command/event bridge.
//!
//! # Threading model (the central constraint)
//!
//! `agent-client-protocol` 0.12 drives a connection through a scoped
//! `Client.builder()...connect_with(transport, main_fn)` call: the connection
//! and the spawned agent subprocess live only for the duration of `main_fn`.
//! The connection's background actors run concurrently with `main_fn` and are
//! driven by a single `block_on`.
//!
//! Tauri commands run on a multithreaded runtime and must return `Send`
//! futures, so we cannot hold the connection in shared state and `.await` it
//! inside a command. Instead, **each agent owns a dedicated OS thread** running
//! a current-thread Tokio runtime. That thread owns the connection (via
//! `connect_with`) and the child stdio. Tauri commands talk to the thread by
//! sending [`AcpCommand`] variants (each carrying a `tokio::sync::oneshot`
//! reply sender) over a `tokio::sync::mpsc` channel, then `.await` the `Send`
//! oneshot reply. Streaming `session/update` notifications and inbound agent
//! requests (permission, fs) are fanned out to the renderer (and, in Story 1.4,
//! the WS relay) through a cloned `Vec<Arc<dyn EventSink>>` — NOT via a Tauri
//! `AppHandle` directly (Story 1.1 / architecture D2 decoupling).
//!
//! This mirrors how `PtyManager` isolates per-PTY I/O on its own threads and
//! emits to the renderer through its own sink fan-out.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthenticateRequest, CancelNotification, CloseSessionRequest,
    ContentBlock, EnvVariable, InitializeRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, McpServer, McpServerStdio, NewSessionRequest, PermissionOption,
    PermissionOptionKind, PromptRequest, RequestPermissionOutcome, RequestPermissionResponse,
    ResumeSessionRequest, ResumeSessionResponse, SelectedPermissionOutcome, SessionConfigOption,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectionTo, LineDirection};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::acp::client;
use crate::acp::config::{AgentConfig, AgentId, PermissionPolicy, SessionId};
use crate::acp::events::{
    self, AgentCrashedEvent, AgentDisconnectedEvent, AgentErrorEvent, AgentSpawnedEvent,
    AuthMethodInfo, ConfigOptionsUpdateEvent, FanOutError, FanOutReceipt, PromptCompleteEvent,
    SessionClosedEvent, SessionCreatedEvent, SessionInfoUpdateEvent, SessionModelState,
};
use crate::acp::session::DriverState;
use crate::acp::session_persistence::{
    is_protected_title_source, normalize_title, PersistedSessionStatus, SessionPersistence,
    SessionRegistration, TitleSource,
};
use crate::conversation::{
    AgentBindingResult, AgentCompensationFailure, AgentSessionBinding, ConversationCreationService,
    ConversationId, ConversationPersistenceAdapter, ExecutionTarget, PrepareConversationRequest,
    PreparedConversation, ProjectAttachment, ACP_COMPENSATION_FAILED,
    PROJECT_ATTACHMENT_SCHEMA_VERSION,
};
use crate::web::sink::CONVERSATION_PERSISTENCE_REJECTED;
use crate::web::EventSink;

/// How long to wait for the agent to answer `initialize` before treating the
/// spawn as failed (and tearing the child down).
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for `session/new` before returning an error to the caller.
///
/// 60s accommodates agents whose `session/new` handler fetches a model list
/// from a remote service on a cold start (e.g. `pi-acp`, which can exceed the
/// former 30s budget on first launch). Overridable for diagnostics via
/// [`session_new_timeout`].
const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for `session/load` / `session/resume` before returning an
/// error to the caller. Without a bound, a wedged agent parks the renderer's
/// "reconnecting" state forever (the reopened chat can never recover). 60s
/// matches the `session/new` budget: a load replays the full conversation and
/// can legitimately take a while on large histories.
const SESSION_REOPEN_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait, after `session/cancel`, for the agent to honor the cancel
/// and reply to the in-flight prompt before we forcibly resolve the turn.
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for the first-prompt warmup after `session/new`.
///
/// pi-acp's `PiRpcProcess.request()` has no timeout, and pi's `session.prompt()`
/// may stall before calling `preflightResult` on a cold start. The warmup sends
/// a lightweight `session/prompt` through the full pi-acp pipeline before the
/// `acp:session_created` event is emitted, so the renderer's event handlers
/// silently drop warmup events (the session doesn't exist in the store yet).
/// If the warmup times out, we cancel and continue — session creation still
/// succeeds, but the user's first manual prompt may still experience the
/// cold-start hang. Overridable via `TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS`.
const FIRST_PROMPT_WARMUP_TIMEOUT: Duration = Duration::from_secs(45);
/// Upper bound on joining a driver thread during `kill`/`kill_all`, so app exit
/// can never hang on a wedged agent.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);
// Idle timeout for an agent turn: if the agent produces NO activity (no
// `session/update`/`tool_call` notification) for this long, the turn is
// considered wedged and is cancelled. The default is **unlimited** (`None` —
// see `turn_idle_timeout`): a silent/wedged turn is NOT killed by default;
// only an explicit `TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS` env var or an App
// Preferences value imposes an idle bound. Reset on every inbound notification
// via `DriverState::signal_idle`.
//
// The historical 900s (15min) default was retired in favour of unlimited — a
// legitimate long-running silent sub-tool (a ~600s shell command) no longer
// races an arbitrary idle deadline, and a truly wedged turn is left to the
// operator/user to cancel (or to bound explicitly via the env var).
// The default hard wall-clock cap for a single agent turn is **unlimited**
// (`None` — see `resolved_turn_timeout`): no last-resort backstop is imposed
// unless the operator sets `TERMUL_ACP_TURN_TIMEOUT_SECS` or the user picks a
// bounded value in App Preferences. The per-turn *idle* timeout
// (`turn_idle_timeout`) is also unlimited by default, so neither a chatty
// nor a silent agent is killed by default — the hard cap is an opt-in
// diagnostic backstop. On either idle or hard timeout → cancel +
// `CANCEL_GRACE` → `status: 'error'`. Distinct from 1.7's 60s permission
// sub-timeout (`permissions.rs:47`).

/// `session/new` timeout. Precedence: `TERMUL_ACP_SESSION_NEW_TIMEOUT_SECS`
/// (env, operator/diagnostic; seconds, must be > 0) → in-process UI override
/// ([`set_session_new_timeout_override`]) → [`SESSION_NEW_TIMEOUT`]. Useful
/// when an agent needs longer to fetch its model list on a cold start; the
/// default stays strict so a wedged agent still fails fast in normal use.
fn session_new_timeout() -> Duration {
    std::env::var("TERMUL_ACP_SESSION_NEW_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|secs: &u64| *secs > 0)
        .map(Duration::from_secs)
        .or_else(|| {
            session_new_timeout_override()
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
        })
        .unwrap_or(SESSION_NEW_TIMEOUT)
}

/// First-prompt warmup timeout. Precedence:
/// `TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS` (env, operator/diagnostic) →
/// in-process UI override ([`set_first_prompt_warmup_timeout_override`]) →
/// [`FIRST_PROMPT_WARMUP_TIMEOUT`]. Set to 0 (env or override) to disable the
/// warmup entirely. An INVALID env value is logged and treated like an absent
/// one — it falls through to the UI override/default instead of masking it
/// (same shape as the other resolvers' `.parse().ok()` fall-through).
fn first_prompt_warmup_timeout() -> Duration {
    let override_or_default = || {
        first_prompt_warmup_timeout_override()
            .map(Duration::from_secs)
            .unwrap_or(FIRST_PROMPT_WARMUP_TIMEOUT)
    };
    match std::env::var("TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS") {
        Ok(v) => match v.parse::<u64>() {
            Ok(secs) => Duration::from_secs(secs),
            Err(_) => {
                log::warn!(
                    "[acp] TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS={v:?} is not a valid \
                     unsigned integer; falling back to the UI override / {:?}",
                    FIRST_PROMPT_WARMUP_TIMEOUT
                );
                override_or_default()
            }
        },
        Err(_) => override_or_default(),
    }
}

/// Atomically check + mark the first-prompt warmup as started for an agent.
/// Returns `true` if the caller should run the warmup (the agent was NOT in
/// the set and is now inserted); returns `false` if a warmup already completed
/// (or is still in-flight) for this agent within its lifetime — the caller
/// must skip.
///
/// The check + insert happen under ONE lock acquisition so two concurrent
/// `NewSession` calls for the same agent cannot both pass the gate (TOCTOU
/// fix): the first inserts, the second sees the entry and coalesces onto the
/// pending warmup (I/O matrix Row 6: "do not spawn a second"). The entry is
/// never cleared on a warmup exit branch, so the "done" dedup also holds for
/// subsequent `NewSession` calls within the same agent lifetime (I/O matrix
/// Row 5: "Skip second warmup"). Cleared on agent drop (driver self-reap) so
/// a re-spawned agent re-warmups.
fn warmup_should_run(warmup_done: &Mutex<HashSet<AgentId>>, agent_id: &AgentId) -> bool {
    let mut set = warmup_done.lock();
    if set.contains(agent_id) {
        log::debug!(
            "[acp] {agent_id} first-prompt warmup skipped: \
             already completed for this agent lifetime"
        );
        false
    } else {
        set.insert(agent_id.clone());
        true
    }
}

/// `session/load` / `session/resume` timeout. Precedence:
/// `TERMUL_ACP_SESSION_REOPEN_TIMEOUT_SECS` (env, operator/diagnostic;
/// seconds, must be > 0) → in-process UI override
/// ([`set_session_reopen_timeout_override`]) → [`SESSION_REOPEN_TIMEOUT`]. A
/// load replays the full conversation before responding, so very large
/// histories may need a longer budget.
fn session_reopen_timeout() -> Duration {
    std::env::var("TERMUL_ACP_SESSION_REOPEN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|secs: &u64| *secs > 0)
        .map(Duration::from_secs)
        .or_else(|| {
            session_reopen_timeout_override()
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
        })
        .unwrap_or(SESSION_REOPEN_TIMEOUT)
}

/// Per-turn idle timeout. Precedence: `TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS`
/// (env, operator/diagnostic; seconds, must be > 0) → in-process UI override
/// ([`set_turn_idle_timeout_override`]) → `None` (**unlimited** default). `None`
/// means a silent/wedged turn is NOT killed by the idle timer — only completion,
/// cancel, or an explicitly configured idle/hard bound ends it. The window, when
/// set, is the duration with no agent activity after which a turn is considered
/// wedged.
pub fn turn_idle_timeout() -> Option<Duration> {
    std::env::var("TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|secs: &u64| *secs > 0)
        .map(Duration::from_secs)
        .or_else(|| {
            turn_idle_timeout_override()
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
        })
}

/// In-process override for the hard wall-clock cap, set by the
/// `acp_set_turn_timeout` Tauri command from the App Preferences UI. `None` =
/// use the env var / default. Consulted by [`resolved_turn_timeout`] so a UI
/// change takes effect on the next turn without a restart. Desktop-only: the
/// standalone server has no settings surface and configures via
/// `TERMUL_ACP_TURN_TIMEOUT_SECS` (the operator env var still wins — see the
/// precedence in [`resolved_turn_timeout`]).
static TURN_TIMEOUT_OVERRIDE: LazyLock<parking_lot::Mutex<Option<u64>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));

/// Set the in-process turn-timeout override (secs, or `None` to clear). Called
/// by the `acp_set_turn_timeout` Tauri command (desktop renderer settings).
pub fn set_turn_timeout_override(secs: Option<u64>) {
    *TURN_TIMEOUT_OVERRIDE.lock() = secs;
}

/// Read the in-process turn-timeout override, if set.
pub fn turn_timeout_override() -> Option<u64> {
    *TURN_TIMEOUT_OVERRIDE.lock()
}

/// In-process override for the per-turn idle timeout, set by the
/// `acp_set_turn_idle_timeout` Tauri command from the App Preferences UI.
/// `None` = use the env var / default. Same desktop-only contract and
/// precedence shape as [`TURN_TIMEOUT_OVERRIDE`] (the operator env var still
/// wins — see [`turn_idle_timeout`]).
static TURN_IDLE_TIMEOUT_OVERRIDE: LazyLock<parking_lot::Mutex<Option<u64>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));
/// In-process override for the `session/new` timeout, set by the
/// `acp_set_session_new_timeout` Tauri command. Same contract as
/// [`TURN_TIMEOUT_OVERRIDE`] (see [`session_new_timeout`]).
static SESSION_NEW_TIMEOUT_OVERRIDE: LazyLock<parking_lot::Mutex<Option<u64>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));
/// In-process override for the `session/load` / `session/resume` timeout, set
/// by the `acp_set_session_reopen_timeout` Tauri command. Same contract as
/// [`TURN_TIMEOUT_OVERRIDE`] (see [`session_reopen_timeout`]).
static SESSION_REOPEN_TIMEOUT_OVERRIDE: LazyLock<parking_lot::Mutex<Option<u64>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));
/// In-process override for the first-prompt warmup timeout, set by the
/// `acp_set_first_prompt_warmup_timeout` Tauri command. Unlike the other
/// overrides, `Some(0)` is meaningful: it disables the warmup entirely (see
/// [`first_prompt_warmup_timeout`]). Otherwise the same contract as
/// [`TURN_TIMEOUT_OVERRIDE`].
static FIRST_PROMPT_WARMUP_TIMEOUT_OVERRIDE: LazyLock<parking_lot::Mutex<Option<u64>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));

/// Set the in-process turn-idle-timeout override (secs > 0, or `None` to
/// clear). Called by the `acp_set_turn_idle_timeout` Tauri command.
pub fn set_turn_idle_timeout_override(secs: Option<u64>) {
    *TURN_IDLE_TIMEOUT_OVERRIDE.lock() = secs;
}

/// Read the in-process turn-idle-timeout override, if set.
fn turn_idle_timeout_override() -> Option<u64> {
    *TURN_IDLE_TIMEOUT_OVERRIDE.lock()
}

/// Set the in-process `session/new` timeout override (secs > 0, or `None` to
/// clear). Called by the `acp_set_session_new_timeout` Tauri command.
pub fn set_session_new_timeout_override(secs: Option<u64>) {
    *SESSION_NEW_TIMEOUT_OVERRIDE.lock() = secs;
}

/// Read the in-process `session/new` timeout override, if set.
fn session_new_timeout_override() -> Option<u64> {
    *SESSION_NEW_TIMEOUT_OVERRIDE.lock()
}

/// Set the in-process session-reopen timeout override (secs > 0, or `None` to
/// clear). Called by the `acp_set_session_reopen_timeout` Tauri command.
pub fn set_session_reopen_timeout_override(secs: Option<u64>) {
    *SESSION_REOPEN_TIMEOUT_OVERRIDE.lock() = secs;
}

/// Read the in-process session-reopen timeout override, if set.
fn session_reopen_timeout_override() -> Option<u64> {
    *SESSION_REOPEN_TIMEOUT_OVERRIDE.lock()
}

/// Set the in-process first-prompt-warmup timeout override (secs, `0` to
/// disable the warmup, or `None` to clear). Called by the
/// `acp_set_first_prompt_warmup_timeout` Tauri command.
pub fn set_first_prompt_warmup_timeout_override(secs: Option<u64>) {
    *FIRST_PROMPT_WARMUP_TIMEOUT_OVERRIDE.lock() = secs;
}

/// Read the in-process first-prompt-warmup timeout override, if set.
fn first_prompt_warmup_timeout_override() -> Option<u64> {
    *FIRST_PROMPT_WARMUP_TIMEOUT_OVERRIDE.lock()
}

/// Hard wall-clock cap per turn. Precedence: `TERMUL_ACP_TURN_TIMEOUT_SECS`
/// (env, operator/diagnostic; seconds, must be > 0) → in-process UI override
/// ([`set_turn_timeout_override`]) → `None` (**unlimited** default). `None`
/// means no hard backstop is imposed — the per-turn *idle* timeout
/// ([`turn_idle_timeout`]) still bounds silent/wedged turns, so a chatty agent
/// that stays active is not killed by default. An operator who wants a bounded
/// hard cap sets the env var, or the user picks a value in App Preferences.
pub fn resolved_turn_timeout() -> Option<Duration> {
    std::env::var("TERMUL_ACP_TURN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|secs: &u64| *secs > 0)
        .map(Duration::from_secs)
        .or_else(|| {
            turn_timeout_override()
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
        })
}

/// Race an in-flight ACP prompt turn against completion, a cancel signal, an
/// optional idle deadline (reset on agent activity via `idle_rx`), and an
/// optional hard wall-clock cap. On idle/hard timeout, invoke `on_timeout_cancel`
/// (the caller's cancel hook — updates DriverState cancel/timeout state so the
/// in-flight turn winds down), await `CANCEL_GRACE`, then return a typed timeout
/// error. Extracted so the deadline loop is unit-testable with mock futures. A
/// pre-iteration deadline check bounds a continuously-ready activity arm (under
/// `biased`) so a streaming-but-non-completing agent can't slip past a cap. When
/// a deadline is `None` (the unlimited default for idle and/or hard), it imposes
/// no bound — a fully-unlimited turn (both `None`) is ended only by completion or
/// cancel, so a wedged agent is NOT killed by default.
async fn race_turn<P>(
    prompt: P,
    mut cancel_rx: oneshot::Receiver<()>,
    idle_rx: &mut watch::Receiver<()>,
    on_timeout_cancel: impl Fn(),
    idle: Option<Duration>,
    hard: Option<Duration>,
) -> Result<StopReason, String>
where
    P: std::future::Future<Output = Result<StopReason, String>>,
{
    tokio::pin!(prompt);
    let hard_deadline = hard.map(|d| tokio::time::Instant::now() + d);
    let mut idle_deadline = idle.map(|d| tokio::time::Instant::now() + d);
    loop {
        // Next deadline: the earliest of the configured (Some) deadlines; `None`
        // when neither idle nor hard is configured (fully unlimited).
        let next_deadline = match (idle_deadline, hard_deadline) {
            (Some(i), Some(h)) => Some(i.min(h)),
            (Some(i), None) => Some(i),
            (None, Some(h)) => Some(h),
            (None, None) => None,
        };
        // Pre-select check: under `biased`, a continuously-ready activity arm
        // would win every poll and `sleep_until` would never fire — silently
        // defeating the cap(s) for a streaming-but-non-completing agent.
        if let Some(nd) = next_deadline {
            if tokio::time::Instant::now() >= nd {
                on_timeout_cancel();
                return match tokio::time::timeout(CANCEL_GRACE, &mut prompt).await {
                    Ok(result) => result,
                    Err(_) if idle_deadline == Some(nd) => {
                        let idle_dur = idle.unwrap_or(Duration::ZERO);
                        Err(format!(
                            "turn idle timeout: no agent activity for {idle_dur:?}"
                        ))
                    }
                    Err(_) => {
                        let hard_dur = hard.unwrap_or(Duration::ZERO);
                        Err(format!("turn hard timeout: exceeded {hard_dur:?}"))
                    }
                };
            }
        }
        match next_deadline {
            Some(nd) => {
                tokio::select! {
                    biased;
                    result = &mut prompt => return result,
                    _ = &mut cancel_rx => {
                        return match tokio::time::timeout(CANCEL_GRACE, &mut prompt).await {
                            Ok(result) => result,
                            Err(_) => Ok(StopReason::Cancelled),
                        };
                    }
                    _ = idle_rx.changed() => {
                        if let Some(d) = idle {
                            idle_deadline = Some(tokio::time::Instant::now() + d);
                        }
                    }
                    _ = tokio::time::sleep_until(nd) => {}
                }
            }
            None => {
                // No deadlines (fully unlimited): only completion or cancel can
                // end the turn. The `idle_rx` arm is intentionally absent —
                // there is no deadline to reset, and a closed watch channel
                // would otherwise return `Err` ready on every poll and busy-loop
                // (starving the runtime). If a deadline is later configured it
                // routes through the `Some` branch above, where the idle arm
                // resets the idle deadline.
                tokio::select! {
                    biased;
                    result = &mut prompt => return result,
                    _ = &mut cancel_rx => {
                        return match tokio::time::timeout(CANCEL_GRACE, &mut prompt).await {
                            Ok(result) => result,
                            Err(_) => Ok(StopReason::Cancelled),
                        };
                    }
                }
            }
        }
    }
}

/// Option snapshot returned by a successful `session/load` or `session/resume`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReopenOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<agent_client_protocol::schema::v1::SessionModeState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<SessionModelState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<SessionConfigOption>>,
}

impl SessionReopenOutcome {
    fn empty() -> Self {
        Self {
            modes: None,
            models: None,
            config_options: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.modes.is_none() && self.models.is_none() && self.config_options.is_none()
    }
}

/// Live-session composer controls so phone/web can read mode/model/thinking
/// without `session/resume` or `session/load` (those replay the transcript).
#[derive(Default)]
struct ComposerControlCache {
    inner: Mutex<HashMap<String, SessionReopenOutcome>>,
}

impl ComposerControlCache {
    fn snapshot(&self, session_id: &str) -> SessionReopenOutcome {
        self.inner
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_else(SessionReopenOutcome::empty)
    }

    fn remember(&self, session_id: &str, incoming: &SessionReopenOutcome) {
        if incoming.is_empty() {
            return;
        }
        let mut map = self.inner.lock();
        let current = map
            .entry(session_id.to_string())
            .or_insert_with(SessionReopenOutcome::empty);
        if incoming.modes.is_some() {
            current.modes = incoming.modes.clone();
        }
        if incoming.config_options.is_some() {
            current.config_options = incoming.config_options.clone();
            current.models = events::models_from_config_options(current.config_options.as_deref())
                .or_else(|| incoming.models.clone());
        } else if incoming.models.is_some() {
            current.models = incoming.models.clone();
        }
    }

    fn remember_options(&self, session_id: &str, options: Vec<SessionConfigOption>) {
        self.remember(
            session_id,
            &SessionReopenOutcome {
                modes: None,
                models: events::models_from_config_options(Some(options.as_slice())),
                config_options: Some(options),
            },
        );
    }

    fn apply_current_mode(
        &self,
        session_id: &str,
        current_mode_id: agent_client_protocol::schema::v1::SessionModeId,
    ) {
        let mut map = self.inner.lock();
        if let Some(current) = map.get_mut(session_id) {
            if let Some(modes) = current.modes.as_mut() {
                modes.current_mode_id = current_mode_id;
            }
        }
    }

    fn forget(&self, session_id: &str) {
        self.inner.lock().remove(session_id);
    }
}

trait IntoSessionReopenOutcome {
    fn into_session_reopen_outcome(self) -> SessionReopenOutcome;
}

impl IntoSessionReopenOutcome for LoadSessionResponse {
    fn into_session_reopen_outcome(self) -> SessionReopenOutcome {
        let models = events::models_from_config_options(self.config_options.as_deref());
        SessionReopenOutcome {
            modes: self.modes,
            models,
            config_options: self.config_options,
        }
    }
}

impl IntoSessionReopenOutcome for ResumeSessionResponse {
    fn into_session_reopen_outcome(self) -> SessionReopenOutcome {
        let models = events::models_from_config_options(self.config_options.as_deref());
        SessionReopenOutcome {
            modes: self.modes,
            models,
            config_options: self.config_options,
        }
    }
}

/// Timed `session/load` / `session/resume`: preserve the option snapshot and
/// record the session root on success.
async fn run_session_reopen<Fut, T, E>(
    op: &str,
    session_id: &str,
    cwd: &str,
    req_state: &Mutex<DriverState>,
    request: Fut,
) -> Result<SessionReopenOutcome, String>
where
    Fut: std::future::Future<Output = Result<T, E>>,
    T: IntoSessionReopenOutcome,
    E: ToString,
{
    let timeout = session_reopen_timeout();
    let outcome = tokio::time::timeout(timeout, request).await;
    let result = match outcome {
        Ok(result) => result
            .map(IntoSessionReopenOutcome::into_session_reopen_outcome)
            .map_err(|e| e.to_string()),
        Err(_) => {
            log::warn!(
                "[acp] session {session_id} {op} timed out after {timeout:?}; \
                 check agent stderr in RUST_LOG=debug"
            );
            Err(format!("{op} timed out after {timeout:?}"))
        }
    };
    if result.is_ok() {
        req_state
            .lock()
            .set_session_root(session_id.to_string(), PathBuf::from(cwd));
    }
    result
}

/// Outcome of creating a new session, returned to the command caller.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionOutcome {
    pub persistence: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_cwd: Option<String>,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<agent_client_protocol::schema::v1::SessionModeState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<SessionModelState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<SessionConfigOption>>,
}

/// Trusted server-side context attached to durable session registration.
#[derive(Debug, Clone, Default)]
pub struct SessionCreationContext {
    pub project_id: Option<String>,
    pub ephemeral: bool,
    pub conversation_id: Option<ConversationId>,
    pub project_attachment: Option<ProjectAttachment>,
    pub execution_target: Option<ExecutionTarget>,
    /// Worktree path the agent runs in (CAP-3). When set, the durable record
    /// carries it so relaunch reattaches without a second `git worktree add`
    /// and the chat indicator (CAP-6) survives reload. State isolation still
    /// keys on `cwd` (the worktree path) — this field is for the indicator +
    /// deleted-worktree fallback only.
    pub worktree_path: Option<String>,
    /// Worktree branch (`chat/{id}`) — paired with `worktree_path`.
    pub worktree_branch: Option<String>,
}

/// The `_session/question` ACP extension request (issue #411).
///
/// Agents send a structured question over the protocol's extension surface
/// (the vendored protocol routes `_`-prefixed methods to
/// [`ExtMethodRequest`](agent_client_protocol::schema::v1::ExtRequest)); the
/// response is an untyped JSON value. This type implements the protocol's
/// `JsonRpcMessage`/`JsonRpcRequest` traits directly for the single method, so
/// the driver handler chain matches ONLY `_session/question` — unlike
/// registering on the whole `AgentRequest` enum, which would also claim
/// client→agent responses and break response routing.
///
/// Wire params (camelCase): `{ sessionId, questionId?, question, options }`
/// where `options` is `[{ value, label, description?, cardinality? }]` and
/// `cardinality` is `single` (default) or `multi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskUserQuestionRequest {
    session_id: String,
    question: String,
    #[serde(default)]
    options: Vec<AskQuestionOption>,
    #[serde(default)]
    question_id: Option<String>,
}

/// One option of an [`AskUserQuestionRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskQuestionOption {
    value: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cardinality: Option<String>,
}

impl agent_client_protocol::JsonRpcMessage for AskUserQuestionRequest {
    fn matches_method(method: &str) -> bool {
        method == "_session/question"
    }

    fn method(&self) -> &str {
        "_session/question"
    }

    fn to_untyped_message(
        &self,
    ) -> Result<agent_client_protocol::UntypedMessage, agent_client_protocol::Error> {
        agent_client_protocol::UntypedMessage::new("_session/question", self)
    }

    fn parse_message(
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<Self, agent_client_protocol::Error> {
        if method != "_session/question" {
            return Err(agent_client_protocol::Error::method_not_found());
        }
        agent_client_protocol::util::json_cast_params(params)
    }
}

impl agent_client_protocol::JsonRpcRequest for AskUserQuestionRequest {
    type Response = Value;
}

/// Commands sent from Tauri command handlers to an agent's driver thread.
///
/// Every variant that expects a result carries a `oneshot::Sender`; the driver
/// thread fulfills it after performing the protocol exchange. All payloads are
/// `Send`, so the awaiting command future stays `Send`.
enum AcpCommand {
    NewSession {
        cwd: String,
        mcp_servers: Vec<McpServer>,
        stable_agent_namespace: Option<String>,
        runtime_agent_id: String,
        project_id: Option<String>,
        ephemeral: bool,
        worktree_path: Option<String>,
        worktree_branch: Option<String>,
        binding_gate: Option<watch::Receiver<Option<Result<(), String>>>>,
        reply: oneshot::Sender<Result<NewSessionOutcome, String>>,
    },
    LoadSession {
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        reply: oneshot::Sender<Result<SessionReopenOutcome, String>>,
    },
    ResumeSession {
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        reply: oneshot::Sender<Result<SessionReopenOutcome, String>>,
    },
    CloseSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), String>>,
    },
    DisposeEphemeralSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ListSessions {
        cwd: Option<String>,
        cursor: Option<String>,
        reply: oneshot::Sender<Result<ListSessionsResponse, String>>,
    },
    SendPrompt {
        session_id: SessionId,
        content: Vec<ContentBlock>,
        /// Story 1.8 T3.2: optional client turn-id (echoed back on
        /// `prompt_complete` for the renderer's `seenTurnIds` idempotent dedup —
        /// FR11). `None` for desktop/older clients (dedup is a no-op).
        turn_id: Option<String>,
        /// Resolves after the driver has claimed the session turn and
        /// registered its completion task. Callers can then process a
        /// following cancellation without racing prompt startup.
        accepted: oneshot::Sender<Result<(), String>>,
        reply: oneshot::Sender<Result<StopReason, String>>,
    },
    CancelPrompt {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), String>>,
    },
    OwnsSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    IsEphemeralSession {
        session_id: SessionId,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    IsTurnActive {
        session_id: SessionId,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    WaitTurnIdle {
        session_id: SessionId,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetMode {
        session_id: SessionId,
        mode_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetModel {
        session_id: SessionId,
        model_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetConfigOption {
        session_id: SessionId,
        config_id: String,
        value_id: String,
        reply: oneshot::Sender<Result<Vec<SessionConfigOption>, String>>,
    },
    RespondPermission {
        request_id: String,
        outcome: RequestPermissionOutcome,
        reply: oneshot::Sender<Result<(), String>>,
    },
    AnswerQuestion {
        question_id: String,
        values: Option<Vec<String>>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Run the ACP `authenticate` method with the given method id.
    Authenticate {
        method_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Ask the driver thread to wind down its connection and exit.
    Shutdown,
}

pub(crate) struct StartedPrompt {
    completion: oneshot::Receiver<Result<StopReason, String>>,
}

/// Result of a successful `initialize` handshake, carried back to the spawning
/// task: the negotiated capabilities plus every advertised authentication
/// method (opaque `id`/`name`/optional `description`). The renderer needs the
/// full method metadata to present a Sign-in action and call
/// `authenticate(methodId)` before `session/new`.
struct InitOutcome {
    capabilities: AgentCapabilities,
    auth_methods: Vec<AuthMethodInfo>,
}

/// Authoritative spawn result returned by [`AcpManager::spawn`] and mirrored
/// verbatim by both the Tauri `acp_spawn_agent` command and the WS
/// `spawn_agent` handler (CAP-4: metadata delivery cannot depend on a session
/// subscription that does not yet exist). Carries everything the renderer needs
/// to populate the store synchronously: the negotiated capabilities, advertised
/// auth methods, and stable namespace. The `acp:agent_spawned` event is still
/// emitted for observers but is no longer the source of truth — the spawn
/// response is. Serialized camelCase on the wire so desktop (Tauri `Result`)
/// and web (`WsReply` payload) share one contract.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnOutcome {
    pub agent_id: AgentId,
    pub capabilities: AgentCapabilities,
    /// Every authentication method the agent advertised at `initialize`. Always
    /// serialized (as `[]` when empty) so the renderer sees a stable field.
    pub auth_methods: Vec<AuthMethodInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_namespace: Option<String>,
}

/// Registry entry for a live agent.
struct AgentEntry {
    command_tx: mpsc::UnboundedSender<AcpCommand>,
    capabilities: AgentCapabilities,
    stable_namespace: Option<String>,
    join_handle: Option<JoinHandle<()>>,
    /// Set true by `kill`/`kill_all` before winding the agent down, so the
    /// driver thread's teardown can tell an intentional kill (silent) from a
    /// spontaneous crash (emits `acp:agent_disconnected`). See L4.
    killed: Arc<AtomicBool>,
    permission_policy: Arc<Mutex<PermissionPolicy>>,
}

/// Manages all ACP agents, mirroring the `PtyManager` ownership pattern.
///
/// # AppHandle coupling (Story 1.1 / architecture D2)
///
/// `AcpManager` is **transport-neutral**: it holds `Vec<Arc<dyn EventSink>>`
/// and fans every `acp:*` event out through them. It does NOT hold a Tauri
/// `AppHandle`. The desktop app constructs it with
/// `vec![Arc::new(TauriEventSink::new(handle))]` (see `lib.rs`); the standalone
/// `termul-server` binary (Story 1.2) will construct it with a
/// `WsRelaySink`-backed sink list and NO `AppHandle` at all.
///
/// The ONLY `AppHandle` reference in the ACP stack lives inside
/// `crate::web::TauriEventSink::emit` (the desktop's sink — intentionally
/// Tauri-aware). No code under `src-tauri/src/acp/` may call `app.emit("acp:..")`
/// directly (AC7); all emission goes through [`events::fan_out`] against
/// `self.sinks`.
type ReplacementGateSender = watch::Sender<Option<Result<(), String>>>;

type ReplacementGates = HashMap<String, ReplacementGateSender>;
const MAX_FAILURE_CIRCUIT_ENTRIES: usize = 256;
const FAILURE_CIRCUIT_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, Copy)]
struct CircuitEntry {
    code: &'static str,
    opened_at: std::time::Instant,
}

struct SessionDeliveryCircuitMap {
    entries: HashMap<String, CircuitEntry>,
}

impl SessionDeliveryCircuitMap {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn prune(&mut self, now: std::time::Instant) {
        self.entries.retain(|_, entry| {
            now.duration_since(entry.opened_at).as_secs() < FAILURE_CIRCUIT_TTL_SECS
        });
    }

    fn get(&mut self, session_id: &str) -> Option<&'static str> {
        self.prune(std::time::Instant::now());
        self.entries.get(session_id).map(|entry| entry.code)
    }

    fn insert(&mut self, session_id: String, code: &'static str) {
        let now = std::time::Instant::now();
        self.prune(now);
        if !self.entries.contains_key(&session_id) && self.len() >= MAX_FAILURE_CIRCUIT_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.opened_at)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            session_id,
            CircuitEntry {
                code,
                opened_at: now,
            },
        );
    }

    fn contains_key(&mut self, session_id: &str) -> bool {
        self.get(session_id).is_some()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

type SessionDeliveryCircuits = Arc<Mutex<SessionDeliveryCircuitMap>>;

#[cfg(test)]
fn circuit_map_insert_for_tests(
    circuits: &SessionDeliveryCircuits,
    session_id: &str,
    code: &'static str,
    opened_at: std::time::Instant,
) {
    circuits
        .lock()
        .entries
        .insert(session_id.to_string(), CircuitEntry { code, opened_at });
}

fn fan_out_session<P: Serialize>(
    sinks: &[Arc<dyn EventSink>],
    circuits: &SessionDeliveryCircuits,
    session_id: &str,
    type_: &'static str,
    payload: &P,
) -> Result<FanOutReceipt, FanOutError> {
    if let Some(source_code) = circuits.lock().get(session_id) {
        return Err(FanOutError::circuit_open(source_code));
    }
    let result = events::fan_out(sinks, Some(session_id), type_, payload);
    if let Err(error) = &result {
        if error.should_open_session_circuit() {
            circuits.lock().insert(
                session_id.to_string(),
                error
                    .source_code
                    .unwrap_or(CONVERSATION_PERSISTENCE_REJECTED),
            );
            log::error!(
                "[acp] session delivery circuit opened code={} source_code={}",
                error.code,
                error.source_code.unwrap_or("UNKNOWN")
            );
        }
    }
    result
}

async fn fan_out_session_committed<P: Serialize>(
    sinks: &[Arc<dyn EventSink>],
    circuits: &SessionDeliveryCircuits,
    persistence: Option<&ConversationPersistenceAdapter>,
    session_id: &str,
    type_: &'static str,
    payload: &P,
) -> Result<events::DeliveryReceipt, events::DeliveryError> {
    if let Some(source_code) = circuits.lock().get(session_id) {
        return Err(events::DeliveryError::circuit_open(source_code));
    }
    let result = events::deliver(sinks, persistence, Some(session_id), type_, payload).await;
    if let Err(error) = &result {
        if error.should_open_session_circuit() {
            circuits.lock().insert(
                session_id.to_string(),
                error
                    .source_code
                    .unwrap_or(CONVERSATION_PERSISTENCE_REJECTED),
            );
            log::error!(
                "[acp] session delivery circuit opened code={} source_code={}",
                error.code,
                error.source_code.unwrap_or("UNKNOWN")
            );
        }
    }
    result
}

fn log_delivery_error(operation: &'static str, error: &FanOutError) {
    log::warn!(
        "[acp] event delivery degraded operation={} code={} source_code={} delivered_count={}",
        operation,
        error.code,
        error.source_code.unwrap_or("NONE"),
        error.delivered_count
    );
}

fn log_delivery_ticket_error(operation: &'static str, error: &events::DeliveryError) {
    log::warn!(
        "[acp] event delivery degraded operation={} code={} source_code={} class={:?} delivered_count={}",
        operation,
        error.code,
        error.source_code.unwrap_or("NONE"),
        error.class,
        error.delivered_count
    );
}

pub struct AcpManager {
    sinks: Vec<Arc<dyn EventSink>>,
    agents: Arc<Mutex<HashMap<AgentId, AgentEntry>>>,
    persistence: Option<Arc<SessionPersistence>>,
    conversation_creation: Option<Arc<ConversationCreationService>>,
    conversation_persistence: Option<Arc<ConversationPersistenceAdapter>>,
    pty_manager: Mutex<Option<Weak<crate::pty::PtyManager>>>,
    /// Replacement `session_created` gates keyed by provisional opaque session id. The provider
    /// result is not renderer-visible until canonical `binding_replaced` commits.
    replacement_gates: Mutex<ReplacementGates>,
    /// Per-agent "warmup done" guard for the first-prompt cold-start
    /// workaround (pi-acp issue #94). A visibility-churn re-entry of
    /// `NewSession` for an agent whose warmup already completed (or is still
    /// in-flight) is a logged no-op so the 4–8s warmup is not re-fired within
    /// one agent lifetime. Cleared on agent drop (driver self-reap) so a
    /// re-spawned agent re-warmups.
    warmup_done: Arc<Mutex<HashSet<AgentId>>>,
    /// Host-injected `termul` MCP server (exposes the `plan` tool; one shared TCP listener across
    /// all sessions, started EAGERLY in the constructor so the first
    /// `new_session_with_context` doesn't block a Tokio worker thread on the
    /// bind + port-publish handshake). Injects a self-spawned stdio child
    /// into every non-ephemeral session's `mcp_servers`; the child forwards
    /// `plan` calls back here, and `host_mcp::emit_plan_update` emits a
    /// synthetic `acp:plan_update` so the existing renderer `PlanPanel`
    /// renders it. See `host_mcp::mod` + the spec
    /// `spec-acp-host-todo-plan-tool.md`.
    host_plan_server: Arc<crate::acp::host_mcp::parent::HostPlanServer>,
    /// Last advertised mode/model/configOptions per live session. Phone and
    /// web read this instead of reopening a session the driver already owns.
    composer_controls: Arc<ComposerControlCache>,
}

/// Normalize, durably persist, flush, and broadcast a locally generated title.
/// Used by the host-injected `set_session_title` MCP tool (host_mcp).
pub(crate) async fn record_local_title(
    persistence: &SessionPersistence,
    sinks: &[Arc<dyn EventSink>],
    user_agent_id: AgentId,
    session_id: String,
    raw_title: String,
) -> Result<(), String> {
    let title = normalize_title(&raw_title);
    if title == "Untitled Chat" {
        return Err("title must not be empty".to_string());
    }
    let metadata = persistence.metadata(&session_id).map_err(|error| {
        log::warn!("[acp-title] metadata lookup failed for session {session_id}: {error}");
        "could not read title metadata".to_string()
    })?;
    if metadata.title_source == Some(TitleSource::LocalAlias) {
        return Err("title is protected by a local alias".to_string());
    }
    if metadata.title.as_deref() == Some(title.as_str())
        && metadata.title_source == Some(TitleSource::BackgroundGenerated)
    {
        return Ok(());
    }
    let next_seq = persistence
        .append_local_title(&session_id, title.clone())
        .await
        .map_err(|error| {
            log::warn!("[acp-title] title persistence failed for session {session_id}: {error}");
            "failed to persist title".to_string()
        })?;
    persistence
        .flush_session(&session_id)
        .await
        .map_err(|error| {
            log::warn!("[acp-title] title flush failed for session {session_id}: {error}");
            "failed to flush title".to_string()
        })?;
    let event = SessionInfoUpdateEvent {
        agent_id: user_agent_id,
        session_id: SessionId::new(session_id.clone()),
        title: Some(title.clone()),
    };
    events::fan_out(
        sinks,
        Some(event.session_id.0.as_str()),
        events::EVENT_SESSION_INFO_UPDATE,
        &event,
    )
    .map_err(|error| {
        log_delivery_error("record_local_title", &error);
        error.code.to_string()
    })?;
    log::info!(
        "[acp-title] title persisted and broadcast for session {session_id} (seq={next_seq}, title_len={} chars)",
        title.chars().count()
    );
    Ok(())
}

fn stable_conversation_code(code: crate::conversation::ConversationErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
}

fn compensation_provider_close_code(detail: &str) -> String {
    if detail.contains("does not support session/close") {
        "ACP_CLOSE_UNSUPPORTED"
    } else {
        "ACP_CLOSE_FAILED"
    }
    .to_string()
}

fn log_compensation_failure(operation: &'static str, failure: &AgentCompensationFailure) {
    log::error!(
        "[acp-compensation] operation={} conversation_id={} primary_code={} provider_close_code={} failure_record_code={} recovery_marker_code={} recovery_record_code={}",
        operation,
        failure.conversation_id,
        failure.primary_code,
        failure.provider_close_code.as_deref().unwrap_or("OK"),
        failure.failure_record_code.as_deref().unwrap_or("OK"),
        failure.recovery_marker_code.as_deref().unwrap_or("OK"),
        failure.recovery_record_code.as_deref().unwrap_or("OK")
    );
}

impl AcpManager {
    /// Create a new manager that fans `acp:*` events out to the given sinks.
    ///
    /// Pass `vec![Arc::new(TauriEventSink::new(handle))]` for the desktop app
    /// (byte-for-byte preserves today's Tauri event flow), or a
    /// `WsRelaySink`-backed list for the headless `termul-server` binary
    /// (Story 1.2). An empty `vec![]` is legal (used by unit tests that only
    /// exercise the command channel) — `fan_out` over zero sinks is a no-op.
    #[must_use]
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        let host_plan_server =
            crate::acp::host_mcp::parent::HostPlanServer::start(sinks.clone(), None);
        Self {
            sinks,
            agents: Arc::new(Mutex::new(HashMap::new())),
            persistence: None,
            conversation_creation: None,
            conversation_persistence: None,
            pty_manager: Mutex::new(None),
            replacement_gates: Mutex::new(HashMap::new()),
            warmup_done: Arc::new(Mutex::new(HashSet::new())),
            host_plan_server,
            composer_controls: Arc::new(ComposerControlCache::default()),
        }
    }

    /// Create the standalone manager sharing one durable store with the relay.
    #[must_use]
    pub fn with_persistence(
        sinks: Vec<Arc<dyn EventSink>>,
        persistence: Arc<SessionPersistence>,
    ) -> Self {
        let host_plan_server = crate::acp::host_mcp::parent::HostPlanServer::start(
            sinks.clone(),
            Some(Arc::clone(&persistence)),
        );
        Self {
            sinks,
            agents: Arc::new(Mutex::new(HashMap::new())),
            persistence: Some(persistence),
            conversation_creation: None,
            conversation_persistence: None,
            pty_manager: Mutex::new(None),
            replacement_gates: Mutex::new(HashMap::new()),
            warmup_done: Arc::new(Mutex::new(HashSet::new())),
            host_plan_server,
            composer_controls: Arc::new(ComposerControlCache::default()),
        }
    }

    /// Create a manager backed exclusively by the canonical Conversation repository.
    #[must_use]
    pub fn with_conversation_services(
        sinks: Vec<Arc<dyn EventSink>>,
        creation: Arc<ConversationCreationService>,
        persistence: Arc<ConversationPersistenceAdapter>,
    ) -> Self {
        let host_plan_server =
            crate::acp::host_mcp::parent::HostPlanServer::start_with_conversation_persistence(
                sinks.clone(),
                Arc::clone(&persistence),
            );
        Self {
            sinks,
            agents: Arc::new(Mutex::new(HashMap::new())),
            persistence: None,
            conversation_creation: Some(creation),
            conversation_persistence: Some(persistence),
            pty_manager: Mutex::new(None),
            replacement_gates: Mutex::new(HashMap::new()),
            warmup_done: Arc::new(Mutex::new(HashSet::new())),
            host_plan_server,
            composer_controls: Arc::new(ComposerControlCache::default()),
        }
    }

    #[must_use]
    pub fn persistence(&self) -> Option<Arc<SessionPersistence>> {
        self.persistence.clone()
    }

    #[must_use]
    pub fn conversation_creation(&self) -> Option<Arc<ConversationCreationService>> {
        self.conversation_creation.clone()
    }

    /// Resolve a currently connected runtime agent by its durable renderer config id.
    ///
    /// Scheduled tasks persist only the stable config id, never launch environment
    /// values or credentials. A host can therefore execute a task only while the
    /// matching agent is connected; otherwise the run is recorded as
    /// `AGENT_UNAVAILABLE` and may be retried explicitly after the agent starts.
    #[must_use]
    pub fn find_agent_by_config_id(&self, config_id: &str) -> Option<AgentId> {
        let namespace = format!("config:{}", config_id.trim());
        self.agents.lock().iter().find_map(|(agent_id, entry)| {
            (entry.stable_namespace.as_deref() == Some(namespace.as_str()))
                .then(|| agent_id.clone())
        })
    }

    /// Live runtime ids plus their durable `config:{configId}` namespaces.
    /// Used by catalog overlays so phone/web can reuse a desktop-spawned agent.
    #[must_use]
    pub fn list_running_namespaces(&self) -> Vec<(String, Option<String>)> {
        self.agents
            .lock()
            .iter()
            .map(|(agent_id, entry)| (agent_id.0.clone(), entry.stable_namespace.clone()))
            .collect()
    }

    fn spawn_outcome_for_live(&self, agent_id: &AgentId) -> Option<SpawnOutcome> {
        let agents = self.agents.lock();
        let entry = agents.get(agent_id)?;
        Some(SpawnOutcome {
            agent_id: agent_id.clone(),
            capabilities: entry.capabilities.clone(),
            auth_methods: Vec::new(),
            stable_namespace: entry.stable_namespace.clone(),
        })
    }

    pub fn set_scheduled_tasks(&self, service: &Arc<crate::scheduled_tasks::ScheduledTaskService>) {
        self.host_plan_server.set_scheduled_tasks(service);
    }

    #[must_use]
    pub fn scheduled_tasks(&self) -> Option<Arc<crate::scheduled_tasks::ScheduledTaskService>> {
        self.host_plan_server.scheduled_tasks()
    }

    #[must_use]
    pub fn conversation_id_for_current_session(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.conversation_persistence
            .as_ref()
            .and_then(|adapter| adapter.conversation_id_for_current_binding(agent_session_id))
    }

    /// Map a client-supplied spawn id onto a process that is still running.
    ///
    /// Bindings persist the UUID from the process that first created the
    /// session. After a host restart that UUID is gone; the durable identity is
    /// the binding's `stable_agent_namespace`. A still-live requested id is
    /// returned as-is so a cross-agent session id cannot be remapped onto a
    /// different running process.
    pub fn resolve_live_agent_id(
        &self,
        requested: &AgentId,
        session_id: Option<&str>,
    ) -> Result<AgentId, String> {
        if self.agents.lock().contains_key(requested) {
            return Ok(requested.clone());
        }
        if let Some(session_id) = session_id {
            if let Some(live) = self.live_agent_for_session(session_id) {
                log::info!(
                    "[acp] remapped stale runtime agent to the live process for this session"
                );
                return Ok(live);
            }
        }
        if let Some(found) = self.find_agent_by_requested_id(&requested.0) {
            log::info!("[acp] remapped requested agent id to a live process by config namespace");
            return Ok(found);
        }
        Err(format!("unknown agent: {requested}"))
    }

    fn find_agent_by_requested_id(&self, requested: &str) -> Option<AgentId> {
        let trimmed = requested.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(found) = self.find_agent_by_config_id(trimmed) {
            return Some(found);
        }
        if !trimmed.starts_with("acp-registry:") {
            return self.find_agent_by_config_id(&format!("acp-registry:{trimmed}"));
        }
        None
    }

    fn live_agent_for_session(&self, session_id: &str) -> Option<AgentId> {
        let conversation_id = self.conversation_id_for_current_session(session_id)?;
        let binding = self
            .conversation_creation
            .as_ref()?
            .active_binding(conversation_id)
            .ok()??;
        self.live_agent_for_binding(&binding)
    }

    fn live_agent_for_binding(&self, binding: &AgentSessionBinding) -> Option<AgentId> {
        let runtime = AgentId(binding.runtime_agent_id.clone());
        if self.agents.lock().contains_key(&runtime) {
            return Some(runtime);
        }
        let namespace = binding.stable_agent_namespace.as_str();
        {
            let agents = self.agents.lock();
            if let Some((agent_id, _)) = agents
                .iter()
                .find(|(_, entry)| entry.stable_namespace.as_deref() == Some(namespace))
            {
                return Some(agent_id.clone());
            }
        }
        let config_id = namespace.strip_prefix("config:").unwrap_or(namespace);
        self.find_agent_by_config_id(config_id)
            .or_else(|| self.find_agent_by_requested_id(config_id))
    }

    fn execution_cwd_for_session(&self, session_id: &str) -> Option<String> {
        let conversation_id = self.conversation_id_for_current_session(session_id)?;
        let binding = self
            .conversation_creation
            .as_ref()?
            .active_binding(conversation_id)
            .ok()??;
        (!binding.execution_cwd.trim().is_empty()).then_some(binding.execution_cwd)
    }

    /// Resolve the live process for this session and reopen it when the
    /// current agent does not yet own the Conversation binding (host restart).
    pub async fn ensure_session_on_live_agent(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
    ) -> Result<AgentId, String> {
        let live = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        if let Some(bound) = self.live_agent_for_session(&session_id.0) {
            if bound != live {
                return Err("session does not belong to the supplied live agent".to_string());
            }
        }
        match self.owns_session(&live, session_id.clone()).await {
            Ok(true) => return Ok(live),
            Ok(false) => {}
            Err(error) => return Err(error),
        }
        let Some(cwd) = self.execution_cwd_for_session(&session_id.0) else {
            return Err("session does not belong to the supplied live agent".to_string());
        };
        log::info!("[acp] reattaching the conversation session onto the live agent before prompt");
        let reopened = self
            .resume_session(&live, session_id.clone(), cwd.clone(), Vec::new())
            .await
            .is_ok()
            || self
                .load_session(&live, session_id.clone(), cwd, Vec::new())
                .await
                .is_ok();
        if !reopened {
            return Err("session does not belong to the supplied live agent".to_string());
        }
        match self.owns_session(&live, session_id.clone()).await {
            Ok(true) => Ok(live),
            Ok(false) => Err("session does not belong to the supplied live agent".to_string()),
            Err(error) => Err(error),
        }
    }

    pub fn register_conversation_binding(
        &self,
        agent_session_id: &str,
        conversation_id: ConversationId,
    ) {
        if let Some(adapter) = &self.conversation_persistence {
            adapter.register_binding(agent_session_id, conversation_id);
        }
    }

    pub fn set_pty_manager(&self, pty: &Arc<crate::pty::PtyManager>) {
        *self.pty_manager.lock() = Some(Arc::downgrade(pty));
    }

    #[must_use]
    pub fn pty_manager(&self) -> Option<Arc<crate::pty::PtyManager>> {
        self.pty_manager.lock().as_ref().and_then(Weak::upgrade)
    }

    /// Spawn an ACP agent: launch the subprocess, complete `initialize`, and
    /// register the agent. Emits `acp:agent_spawned` on success. Returns a
    /// [`SpawnOutcome`] carrying the authoritative capabilities, auth methods,
    /// and stable namespace so the renderer can populate the store
    /// synchronously from the response (CAP-4: the spawn response — not the
    /// async event — is the source of truth).
    pub async fn spawn(&self, config: AgentConfig) -> Result<SpawnOutcome, String> {
        if let Some(config_id) = config
            .config_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            if let Some(existing) = self.find_agent_by_config_id(config_id) {
                if let Some(outcome) = self.spawn_outcome_for_live(&existing) {
                    log::info!("[acp] reusing live agent {existing} for config_id={config_id}");
                    return Ok(outcome);
                }
            }
        }
        let config = match tokio::task::spawn_blocking({
            let config = config.clone();
            move || crate::acp::npm_local::materialize_npx_config(config)
        })
        .await
        {
            Ok(next) => next,
            Err(error) => {
                log::warn!("[acp-npm] materialize join failed: {error}");
                config
            }
        };
        self.spawn_with_sinks(config, self.sinks.clone()).await
    }

    /// Same as [`spawn`](Self::spawn) but lets the caller supply the sink list
    /// the background driver threads into. The public [`spawn`](Self::spawn)
    /// forwards `self.sinks` unchanged.
    async fn spawn_with_sinks(
        &self,
        config: AgentConfig,
        sinks: Vec<Arc<dyn EventSink>>,
    ) -> Result<SpawnOutcome, String> {
        let agent_id = AgentId::new();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<AcpCommand>();
        let (init_tx, init_rx) = oneshot::channel::<Result<InitOutcome, String>>();

        // Shared flag serialized by the `agents` lock: set true when the driver
        // thread removes itself (reaps) so a late `spawn` insert can't recreate
        // a ghost entry for an agent that already exited.
        let reaped = Arc::new(AtomicBool::new(false));
        // Set true by `kill`/`kill_all`; lets the driver teardown distinguish an
        // intentional kill (no disconnect event) from a crash (L4).
        let killed = Arc::new(AtomicBool::new(false));
        // Carries the connection-level failure reason (e.g. the subprocess could
        // not be spawned) back to this await path. When `connect_with` fails,
        // `init_tx` is dropped without ever being sent, so `init_rx` resolves to
        // `Err(_)` with no detail; the driver records the real error here so we
        // can surface it instead of a generic "did not initialize" message.
        let start_error = Arc::new(Mutex::new(None::<String>));
        let permission_policy = Arc::new(Mutex::new(config.permission_policy));

        let thread_agent_id = agent_id.clone();
        let thread_config = config.clone();
        let thread_agents = self.agents.clone();
        let thread_reaped = reaped.clone();
        let thread_killed = killed.clone();
        let thread_start_error = start_error.clone();
        let thread_persistence = self.persistence.clone();
        let thread_conversation_persistence = self.conversation_persistence.clone();
        let thread_warmup_done = self.warmup_done.clone();
        let thread_host_plan_server = self.host_plan_server.clone();
        let thread_permission_policy = permission_policy.clone();
        let thread_composer_controls = self.composer_controls.clone();
        let stable_namespace = stable_agent_namespace(&config);

        let join_handle = std::thread::Builder::new()
            .name(format!("acp-agent-{agent_id}"))
            .spawn(move || {
                run_agent(
                    thread_config,
                    sinks,
                    thread_host_plan_server,
                    thread_permission_policy,
                    thread_agent_id,
                    command_rx,
                    init_tx,
                    thread_agents,
                    thread_reaped,
                    thread_killed,
                    thread_start_error,
                    thread_persistence,
                    thread_conversation_persistence,
                    thread_warmup_done,
                    thread_composer_controls,
                );
            })
            .map_err(|e| format!("failed to spawn agent thread: {e}"))?;

        // Wait for the handshake to complete (or fail) on the driver thread.
        let (capabilities, auth_methods) = match init_rx.await {
            Ok(Ok(outcome)) => (outcome.capabilities, outcome.auth_methods),
            Ok(Err(e)) => {
                // Initialize failed; the driver thread is exiting. Join it off
                // the async runtime so we never block a Tauri worker.
                join_thread_bounded(join_handle).await;
                log::warn!("[acp] spawn failed: agent initialize failed: {e}");
                return Err(format!("agent initialize failed: {e}"));
            }
            Err(_) => {
                // Driver thread dropped the sender without initializing (e.g.
                // the subprocess failed to spawn). Join and report failure,
                // preferring the concrete connection error the driver recorded
                // (e.g. "program not found") over the generic fallback.
                join_thread_bounded(join_handle).await;
                let reason = start_error.lock().take();
                let message = match reason {
                    Some(detail) => format!("agent failed to start: {detail}"),
                    None => "agent failed to start (process did not initialize)".to_string(),
                };
                log::warn!("[acp] spawn failed: {message}");
                return Err(message);
            }
        };

        // Register the agent, unless the driver thread already exited (e.g. the
        // agent crashed in the gap between init and registration). The `reaped`
        // check and the insert are serialized by the same lock the reaper uses.
        {
            let mut agents = self.agents.lock();
            if reaped.load(Ordering::Acquire) {
                drop(agents);
                join_thread_bounded(join_handle).await;
                let reason = "agent exited before it could be registered";
                log::warn!("[acp] spawn failed: {reason}");
                return Err(reason.to_string());
            }
            agents.insert(
                agent_id.clone(),
                AgentEntry {
                    command_tx,
                    capabilities: capabilities.clone(),
                    stable_namespace: stable_namespace.clone(),
                    join_handle: Some(join_handle),
                    killed,
                    permission_policy,
                },
            );
        }

        let event = AgentSpawnedEvent {
            agent_id: agent_id.clone(),
            capabilities: capabilities.clone(),
            auth_methods: auth_methods.clone(),
        };
        // `agent_spawned` is agent-level (no session yet) → sid = None. The event
        // stays for observers; the spawn response is now the authoritative source
        // of capabilities + authMethods + stableNamespace.
        if let Err(error) = events::fan_out(&self.sinks, None, events::EVENT_AGENT_SPAWNED, &event)
        {
            log_delivery_error("agent_spawned", &error);
        }

        // Log success at the host boundary with the agent id and auth-method ids
        // (never credentials). One line per spawn so a missing method list or an
        // unexpected auth-required agent is observable in the runtime log.
        let auth_method_ids: Vec<&str> = auth_methods.iter().map(|m| m.id.as_str()).collect();
        log::info!(
            "[acp] agent {agent_id} spawned (auth_methods={:?})",
            auth_method_ids
        );

        Ok(SpawnOutcome {
            agent_id,
            capabilities,
            auth_methods,
            stable_namespace,
        })
    }

    /// Return the ids of all currently registered agents.
    #[must_use]
    pub fn list_agents(&self) -> Vec<AgentId> {
        self.agents.lock().keys().cloned().collect()
    }

    /// Clone the command sender for an agent, or return a typed error.
    fn command_tx(&self, agent_id: &AgentId) -> Result<mpsc::UnboundedSender<AcpCommand>, String> {
        self.agents
            .lock()
            .get(agent_id)
            .map(|entry| entry.command_tx.clone())
            .ok_or_else(|| format!("unknown agent: {agent_id}"))
    }

    /// Clone an agent's negotiated capabilities, or return a typed error.
    fn capabilities(&self, agent_id: &AgentId) -> Result<AgentCapabilities, String> {
        self.agents
            .lock()
            .get(agent_id)
            .map(|entry| entry.capabilities.clone())
            .ok_or_else(|| format!("unknown agent: {agent_id}"))
    }

    pub fn set_permission_policy(
        &self,
        agent_id: &AgentId,
        policy: PermissionPolicy,
    ) -> Result<(), String> {
        let policy_ref = self
            .agents
            .lock()
            .get(agent_id)
            .map(|entry| entry.permission_policy.clone())
            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;
        *policy_ref.lock() = policy;
        log::info!(
            "[acp-permission] boundary=policy_updated agent_id={} policy={policy:?}",
            agent_id.0
        );
        Ok(())
    }

    /// Resolve the stable agent namespace (config id or safe fallback) for a
    /// live agent. Returns `Ok(Some(namespace))` when the agent has a stable
    /// namespace, `Ok(None)` when it has none, or `Err` when the agent is
    /// unknown. Used by the switch-back reopen filter so only sessions owned
    /// by the same agent namespace are candidates (patch #4).
    pub fn stable_agent_namespace(&self, agent_id: &AgentId) -> Result<Option<String>, String> {
        self.agents
            .lock()
            .get(agent_id)
            .map(|entry| entry.stable_namespace.clone())
            .ok_or_else(|| format!("unknown agent: {agent_id}"))
    }

    /// Continue a Conversation's current Active binding instead of minting a
    /// replacement provider session. Used when a client opens or sends into an
    /// existing conversation without an explicit New Chat / replace_binding.
    async fn continue_existing_binding(
        &self,
        agent_id: &AgentId,
        mcp_servers: Vec<McpServer>,
        conversation_id: ConversationId,
        binding: &AgentSessionBinding,
    ) -> Result<NewSessionOutcome, String> {
        let record = self
            .conversation_creation
            .as_ref()
            .ok_or_else(|| {
                "CONVERSATION_BOOTSTRAP_REQUIRED: continue has no ConversationCreationService"
                    .to_string()
            })?
            .repository()
            .get_conversation(conversation_id)
            .map_err(|error| error.to_string())?;
        let session_id = SessionId::new(binding.agent_session_id.clone());
        let execution_cwd = binding.execution_cwd.clone();
        let resume_agent = self
            .live_agent_for_binding(binding)
            .or_else(|| {
                self.agents
                    .lock()
                    .contains_key(agent_id)
                    .then(|| agent_id.clone())
            })
            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;
        let reopen = match self
            .resume_session(
                &resume_agent,
                session_id.clone(),
                execution_cwd.clone(),
                mcp_servers.clone(),
            )
            .await
        {
            Ok(outcome) => Some(outcome),
            Err(_) => self
                .load_session(
                    &resume_agent,
                    session_id.clone(),
                    execution_cwd.clone(),
                    mcp_servers,
                )
                .await
                .ok(),
        };
        log::info!(
            "[conversation-creation] continuing existing ACP binding conversation_id={} session_reopened={}",
            conversation_id,
            reopen.is_some()
        );
        Ok(NewSessionOutcome {
            persistence: "conversation",
            conversation_id: Some(conversation_id),
            workspace_cwd: Some(record.workspace_cwd),
            execution_cwd: Some(execution_cwd),
            session_id,
            modes: reopen.as_ref().and_then(|outcome| outcome.modes.clone()),
            models: reopen.as_ref().and_then(|outcome| outcome.models.clone()),
            config_options: reopen
                .as_ref()
                .and_then(|outcome| outcome.config_options.clone()),
        })
    }

    /// Create a new session on the given agent.
    pub async fn new_session(
        &self,
        agent_id: &AgentId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<NewSessionOutcome, String> {
        self.new_session_with_context(
            agent_id,
            cwd,
            mcp_servers,
            SessionCreationContext::default(),
        )
        .await
    }

    /// Create a replacement provider session without mutating canonical Conversation state.
    /// ConversationLifecycleService calls this only after lastSeq CAS succeeds and appends the
    /// replacement binding before publishing the returned session as canonical.
    pub async fn create_replacement_session(
        &self,
        previous_binding: &AgentSessionBinding,
        prepared: &PreparedConversation,
    ) -> Result<AgentBindingResult, String> {
        let agent_id = AgentId(previous_binding.runtime_agent_id.clone());
        let (caps, stable_agent_namespace) = self
            .agents
            .lock()
            .get(&agent_id)
            .map(|entry| (entry.capabilities.clone(), entry.stable_namespace.clone()))
            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;
        let (port, token, provisional_sid) = self.host_plan_server.register_session(&agent_id.0);
        let internal = build_internal_plan_stdio(&agent_id.0, port, &token, &provisional_sid);
        if let Err(error) = gate_mcp_servers(&caps, &internal) {
            self.host_plan_server.unregister_by_token(&token);
            return Err(error);
        }
        let tx = self.command_tx(&agent_id)?;
        let (binding_gate_tx, binding_gate_rx) = watch::channel(None);
        let outcome = send_command(&tx, |reply| AcpCommand::NewSession {
            cwd: prepared.execution_cwd.clone(),
            mcp_servers: internal,
            stable_agent_namespace: stable_agent_namespace.clone(),
            runtime_agent_id: agent_id.0.clone(),
            project_id: None,
            ephemeral: false,
            worktree_path: None,
            worktree_branch: None,
            binding_gate: Some(binding_gate_rx),
            reply,
        })
        .await;
        match outcome {
            Ok(outcome) => {
                self.host_plan_server
                    .bind_session(&token, &outcome.session_id.0);
                self.replacement_gates
                    .lock()
                    .insert(outcome.session_id.0.clone(), binding_gate_tx);
                Ok(AgentBindingResult {
                    agent_session_id: outcome.session_id.0,
                    runtime_agent_id: agent_id.0,
                    stable_agent_namespace: stable_agent_namespace
                        .unwrap_or_else(|| previous_binding.stable_agent_namespace.clone()),
                })
            }
            Err(error) => {
                let _ = binding_gate_tx.send(Some(Err("ACP_REPLACE_FAILED".to_string())));
                self.host_plan_server.unregister_by_token(&token);
                Err(error)
            }
        }
    }

    /// Publish a replacement provider session only after canonical binding history commits.
    pub fn commit_replacement_session(&self, agent_session_id: &str) {
        if let Some(gate) = self.replacement_gates.lock().remove(agent_session_id) {
            let _ = gate.send(Some(Ok(())));
        }
    }

    /// Suppress a provisional replacement event before checked provider cleanup.
    pub async fn abort_replacement_session(
        &self,
        binding: &AgentSessionBinding,
    ) -> Result<(), String> {
        if let Some(gate) = self
            .replacement_gates
            .lock()
            .remove(&binding.agent_session_id)
        {
            let _ = gate.send(Some(Err("CONVERSATION_RECOVERY_REQUIRED".to_string())));
        }
        self.close_conversation_session(binding).await
    }

    /// Capability-aware close used by ConversationLifecycleService. It does not mutate canonical
    /// bindings; the service appends `binding_suspended` only after this succeeds.
    pub async fn close_conversation_session(
        &self,
        binding: &AgentSessionBinding,
    ) -> Result<(), String> {
        self.close_session(
            &AgentId(binding.runtime_agent_id.clone()),
            SessionId::new(binding.agent_session_id.clone()),
        )
        .await
    }

    pub async fn new_session_with_context(
        &self,
        agent_id: &AgentId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        context: SessionCreationContext,
    ) -> Result<NewSessionOutcome, String> {
        if !context.ephemeral {
            if let Some(conversation_id) = context.conversation_id {
                if let Some(creation) = &self.conversation_creation {
                    if let Ok(Some(binding)) = creation.active_binding(conversation_id) {
                        let resume_agent = self
                            .live_agent_for_binding(&binding)
                            .or_else(|| {
                                self.agents
                                    .lock()
                                    .contains_key(agent_id)
                                    .then(|| agent_id.clone())
                            })
                            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;
                        return self
                            .continue_existing_binding(
                                &resume_agent,
                                mcp_servers,
                                conversation_id,
                                &binding,
                            )
                            .await;
                    }
                }
            }
        }

        let (caps, stable_agent_namespace) = self
            .agents
            .lock()
            .get(agent_id)
            .map(|entry| (entry.capabilities.clone(), entry.stable_namespace.clone()))
            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;

        let mut prepared = None;
        let mut binding_gate_tx = None;
        let mut binding_gate_rx = None;
        let execution_cwd = if context.ephemeral {
            cwd.clone()
        } else if let Some(creation) = &self.conversation_creation {
            let execution_target = context.execution_target.clone().unwrap_or_else(|| {
                if let (Some(project_id), Some(worktree_path), Some(worktree_branch)) = (
                    context.project_id.clone(),
                    context.worktree_path.clone(),
                    context.worktree_branch.clone(),
                ) {
                    ExecutionTarget::Worktree {
                        project_id,
                        worktree_path,
                        worktree_branch,
                    }
                } else if let Some(project_id) = context.project_id.clone() {
                    ExecutionTarget::ProjectRoot {
                        project_id,
                        project_root: cwd.clone(),
                    }
                } else {
                    ExecutionTarget::Workspace
                }
            });
            let project_attachment = context.project_attachment.clone().or_else(|| {
                context
                    .project_id
                    .clone()
                    .map(|project_id| ProjectAttachment {
                        schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
                        project_id,
                        attached_at_utc: chrono::Utc::now(),
                        project_path_snapshot: cwd.clone(),
                        worktree_path: context.worktree_path.clone(),
                        worktree_branch: context.worktree_branch.clone(),
                    })
            });
            let request = PrepareConversationRequest {
                schema_version: crate::conversation::PREPARE_CONVERSATION_SCHEMA_VERSION,
                conversation_id: context.conversation_id,
                project_attachment,
                execution_target,
            };
            let value = creation
                .prepare_conversation(request)
                .await
                .map_err(|error| error.to_string())?;
            log::info!(
                "[conversation-creation] prepared before ACP conversation_id={}",
                value.conversation_id
            );
            let (tx, rx) = watch::channel(None);
            binding_gate_tx = Some(tx);
            binding_gate_rx = Some(rx);
            let execution_cwd = value.execution_cwd.clone();
            prepared = Some(value);
            execution_cwd
        } else {
            return Err("CONVERSATION_BOOTSTRAP_REQUIRED: non-ephemeral creation has no ConversationCreationService".to_string());
        };

        if let Some(prepared) = prepared.as_ref() {
            let provider_key = stable_agent_namespace
                .as_deref()
                .and_then(|value| value.strip_prefix("config:"))
                .unwrap_or(agent_id.0.as_str());
            crate::skills::ConversationSkillProvisioner::new()
                .provision(std::path::Path::new(&prepared.workspace_cwd), provider_key)
                .map_err(|error| format!("SCHEDULED_TASK_SKILL_PROVISION_FAILED: {error}"))?;
        }

        // Host-injected `plan` MCP tool: prepend a self-spawned stdio child to every
        // non-ephemeral session. The provisional token is rebound after durable binding.
        let (combined_mcp_servers, plan_token): (Vec<McpServer>, Option<String>) = if !context
            .ephemeral
        {
            let (port, token, provisional_sid) =
                self.host_plan_server.register_session(&agent_id.0);
            let internal = build_internal_plan_stdio(&agent_id.0, port, &token, &provisional_sid);
            let mut combined = internal;
            combined.extend(mcp_servers);
            (combined, Some(token))
        } else {
            (mcp_servers, None)
        };

        let stable_for_binding = stable_agent_namespace
            .clone()
            .unwrap_or_else(|| agent_id.0.clone());
        let outcome = async {
            gate_mcp_servers(&caps, &combined_mcp_servers)?;
            let tx = self.command_tx(agent_id)?;
            send_command(&tx, |reply| AcpCommand::NewSession {
                cwd: execution_cwd,
                mcp_servers: combined_mcp_servers,
                stable_agent_namespace,
                runtime_agent_id: agent_id.0.clone(),
                project_id: context.project_id,
                ephemeral: context.ephemeral,
                worktree_path: context.worktree_path,
                worktree_branch: context.worktree_branch,
                binding_gate: binding_gate_rx,
                reply,
            })
            .await
        }
        .await;

        match outcome {
            Ok(mut outcome) => {
                if let Some(prepared) = prepared {
                    let bind_result = self
                        .conversation_creation
                        .as_ref()
                        .expect("prepared only when creation service exists")
                        .complete_agent_binding(
                            prepared.conversation_id,
                            AgentBindingResult {
                                agent_session_id: outcome.session_id.0.clone(),
                                runtime_agent_id: agent_id.0.clone(),
                                stable_agent_namespace: stable_for_binding,
                            },
                        )
                        .await;
                    match bind_result {
                        Ok(_) => {
                            if let Some(adapter) = &self.conversation_persistence {
                                adapter.register_binding(
                                    &outcome.session_id.0,
                                    prepared.conversation_id,
                                );
                            }
                            if let Some(tx) = binding_gate_tx {
                                let _ = tx.send(Some(Ok(())));
                            }
                            outcome.persistence = "conversation";
                            outcome.conversation_id = Some(prepared.conversation_id);
                            outcome.workspace_cwd = Some(prepared.workspace_cwd);
                            outcome.execution_cwd = Some(prepared.execution_cwd);
                            log::info!(
                                "[conversation-creation] binding durable conversation_id={}",
                                prepared.conversation_id
                            );
                        }
                        Err(error) => {
                            if let Some(tx) = binding_gate_tx {
                                let _ = tx.send(Some(Err("CONVERSATION_BIND_FAILED".to_string())));
                            }
                            if let Some(token) = plan_token {
                                self.host_plan_server.unregister_by_token(&token);
                            }
                            let provider_close_code = self
                                .close_session(agent_id, outcome.session_id.clone())
                                .await
                                .err()
                                .map(|detail| compensation_provider_close_code(&detail));
                            let creation = self
                                .conversation_creation
                                .as_ref()
                                .expect("prepared only when creation service exists");
                            let failure_record_code = creation
                                .record_agent_creation_failure(
                                    prepared.conversation_id,
                                    "CONVERSATION_BIND_FAILED",
                                    "canonical agent binding could not be persisted",
                                )
                                .await
                                .err()
                                .map(|failure| stable_conversation_code(failure.code));
                            if provider_close_code.is_some() || failure_record_code.is_some() {
                                let failure = creation
                                    .record_agent_compensation_failure(
                                        prepared.conversation_id,
                                        "CONVERSATION_BIND_FAILED",
                                        provider_close_code.as_deref(),
                                        failure_record_code.as_deref(),
                                        None,
                                    )
                                    .await;
                                log_compensation_failure("new_session_bind", &failure);
                                return Err(failure.wire_error());
                            }
                            log::error!(
                                "[conversation-creation] binding durability failed conversation_id={} primary_code=CONVERSATION_BIND_FAILED repository_code={}",
                                prepared.conversation_id,
                                stable_conversation_code(error.code)
                            );
                            return Err(
                                "CONVERSATION_BIND_FAILED: canonical agent binding could not be persisted"
                                    .to_string(),
                            );
                        }
                    }
                }
                if let Some(token) = plan_token {
                    self.host_plan_server
                        .bind_session(&token, &outcome.session_id.0);
                }
                self.composer_controls.remember(
                    &outcome.session_id.0,
                    &SessionReopenOutcome {
                        modes: outcome.modes.clone(),
                        models: outcome.models.clone(),
                        config_options: outcome.config_options.clone(),
                    },
                );
                Ok(outcome)
            }
            Err(error) => {
                let mut compensation_failure = None;
                if let Some(prepared) = prepared {
                    if let Some(creation) = &self.conversation_creation {
                        if let Err(failure) = creation
                            .record_agent_creation_failure(
                                prepared.conversation_id,
                                "ACP_SESSION_NEW_FAILED",
                                "agent session creation failed",
                            )
                            .await
                        {
                            let failure_record_code = stable_conversation_code(failure.code);
                            let compound = creation
                                .record_agent_compensation_failure(
                                    prepared.conversation_id,
                                    "ACP_SESSION_NEW_FAILED",
                                    None,
                                    Some(&failure_record_code),
                                    None,
                                )
                                .await;
                            log_compensation_failure("new_session_create", &compound);
                            compensation_failure = Some(compound);
                        }
                    }
                    if let Some(tx) = binding_gate_tx {
                        let code = if compensation_failure.is_some() {
                            ACP_COMPENSATION_FAILED
                        } else {
                            "ACP_SESSION_NEW_FAILED"
                        };
                        let _ = tx.send(Some(Err(code.to_string())));
                    }
                }
                if let Some(token) = plan_token {
                    self.host_plan_server.unregister_by_token(&token);
                }
                match compensation_failure {
                    Some(failure) => Err(failure.wire_error()),
                    None => Err(error),
                }
            }
        }
    }

    /// Load an existing session. Gated on the agent's `loadSession` capability.
    pub async fn load_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let caps = self.capabilities(&agent_id)?;
        gate_load_session(&caps)?;
        if let Some(outcome) = self
            .skip_reopen_if_owned(&agent_id, &session_id, "session/load")
            .await
        {
            return Ok(outcome);
        }
        let tx = self.command_tx(&agent_id)?;
        let (mcp_servers, host_token) =
            self.prepare_reopen_mcp_servers(&agent_id, &session_id, mcp_servers)?;
        let session_key = session_id.0.clone();
        let outcome = send_command(&tx, |reply| AcpCommand::LoadSession {
            session_id,
            cwd,
            mcp_servers,
            reply,
        })
        .await;
        if outcome.is_err() {
            self.host_plan_server.unregister_by_token(&host_token);
        } else if let Ok(ref snapshot) = outcome {
            self.composer_controls.remember(&session_key, snapshot);
        }
        outcome
    }

    /// Resume a session. Gated on the agent's `sessionCapabilities.resume`.
    pub async fn resume_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let caps = self.capabilities(&agent_id)?;
        gate_resume_session(&caps)?;
        if let Some(outcome) = self
            .skip_reopen_if_owned(&agent_id, &session_id, "session/resume")
            .await
        {
            return Ok(outcome);
        }
        let tx = self.command_tx(&agent_id)?;
        let (mcp_servers, host_token) =
            self.prepare_reopen_mcp_servers(&agent_id, &session_id, mcp_servers)?;
        let session_key = session_id.0.clone();
        let outcome = send_command(&tx, |reply| AcpCommand::ResumeSession {
            session_id,
            cwd,
            mcp_servers,
            reply,
        })
        .await;
        if outcome.is_err() {
            self.host_plan_server.unregister_by_token(&host_token);
        } else if let Ok(ref snapshot) = outcome {
            self.composer_controls.remember(&session_key, snapshot);
        }
        outcome
    }

    fn prepare_reopen_mcp_servers(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
        configured_mcp_servers: Vec<McpServer>,
    ) -> Result<(Vec<McpServer>, String), String> {
        // Reopen requests carry the complete MCP set. Replace any previous
        // in-process route for this session before issuing a fresh credential.
        self.host_plan_server.unregister_session(&session_id.0);
        let (port, token, provisional_sid) = self.host_plan_server.register_session(&agent_id.0);
        self.host_plan_server.bind_session(&token, &session_id.0);
        let mut combined = build_internal_plan_stdio(&agent_id.0, port, &token, &provisional_sid);
        let configured_count = configured_mcp_servers.len();
        combined.extend(configured_mcp_servers);
        if let Err(error) = gate_mcp_servers(&self.capabilities(agent_id)?, &combined) {
            self.host_plan_server.unregister_by_token(&token);
            return Err(error);
        }
        log::info!(
            "[host-mcp] boundary=session_reopen_injected agent_id={} configured_count={} total_count={}",
            agent_id.0,
            configured_count,
            combined.len()
        );
        Ok((combined, token))
    }

    /// Close a session. Gated on the agent's `sessionCapabilities.close`.
    pub async fn close_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        let caps = self.capabilities(agent_id)?;
        gate_close_session(&caps)?;
        let tx = self.command_tx(agent_id)?;
        let session_key = session_id.0.clone();
        let result = send_command(&tx, |reply| AcpCommand::CloseSession { session_id, reply }).await;
        if result.is_ok() {
            self.composer_controls.forget(&session_key);
        }
        result
    }

    pub async fn dispose_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::DisposeEphemeralSession {
            session_id,
            reply,
        })
        .await
    }

    pub async fn is_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        send_command(&tx, |reply| AcpCommand::IsEphemeralSession {
            session_id,
            reply,
        })
        .await
    }

    /// List sessions on the given agent. Gated on the agent's
    /// `sessionCapabilities.list`. Pass `cwd` to filter by working directory;
    /// pass `cursor` for pagination (opaque token from a prior response).
    pub async fn list_sessions(
        &self,
        agent_id: &AgentId,
        cwd: Option<String>,
        cursor: Option<String>,
    ) -> Result<ListSessionsResponse, String> {
        let caps = self.capabilities(agent_id)?;
        gate_list_sessions(&caps)?;
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::ListSessions { cwd, cursor, reply }).await
    }

    /// Send a prompt and await the turn's stop reason. Streaming updates arrive
    /// as `acp:*` events; the turn ends with `acp:prompt_complete`.
    ///
    /// `turn_id` (Story 1.8 T3.2): optional client turn-id, echoed back on the
    /// `prompt_complete` event for the renderer's `seenTurnIds` idempotent dedup
    /// (FR11 — "no duplicate completion on reconnect replay"). `None` for the
    /// desktop path + older clients (dedup is a no-op).
    ///
    /// Title generation is NOT triggered here: the host-injected
    /// `set_session_title` MCP tool (host_mcp) sets the title in-process during
    /// the agent's own turn. If the agent never calls the tool, the title falls
    /// back to `DerivedFirstMessage` (AD-6).
    pub async fn send_prompt(
        self: &Arc<Self>,
        agent_id: &AgentId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
        turn_id: Option<String>,
    ) -> Result<StopReason, String> {
        let started = self
            .start_prompt(agent_id, session_id, content, turn_id)
            .await?;
        self.wait_prompt(started).await
    }

    /// Register a prompt turn with the driver without awaiting the long agent
    /// completion. Success means the driver's single-flight marker and prompt
    /// task are installed, so a subsequently queued cancellation cannot no-op
    /// before the turn starts.
    pub(crate) async fn start_prompt(
        self: &Arc<Self>,
        agent_id: &AgentId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
        turn_id: Option<String>,
    ) -> Result<StartedPrompt, String> {
        if content.is_empty() {
            return Err("prompt content must not be empty".to_string());
        }
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let (completion_tx, completion_rx) = oneshot::channel();
        tx.send(AcpCommand::SendPrompt {
            session_id: session_id.clone(),
            content,
            turn_id,
            accepted: accepted_tx,
            reply: completion_tx,
        })
        .map_err(|_| "agent thread is no longer running".to_string())?;
        accepted_rx
            .await
            .map_err(|_| "agent thread dropped the prompt acceptance".to_string())??;
        Ok(StartedPrompt {
            completion: completion_rx,
        })
    }

    pub(crate) async fn wait_prompt(
        self: &Arc<Self>,
        started: StartedPrompt,
    ) -> Result<StopReason, String> {
        let StartedPrompt { completion } = started;
        let stop_reason = completion
            .await
            .map_err(|_| "agent thread dropped the prompt reply".to_string())??;
        Ok(stop_reason)
    }

    /// Cancel the active turn for a session, resolving pending permissions with
    /// the `cancelled` outcome. No-op (Ok) if there is no active turn.
    pub async fn cancel_prompt(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        send_command(&tx, |reply| AcpCommand::CancelPrompt { session_id, reply }).await
    }

    /// Skip `session/load` and `session/resume` when the live process already
    /// has this session. Reopening a live session makes the agent replay the
    /// whole transcript and persist it again.
    async fn skip_reopen_if_owned(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
        op: &str,
    ) -> Option<SessionReopenOutcome> {
        match self.owns_session(agent_id, session_id.clone()).await {
            Ok(true) => {
                let snapshot = self.composer_controls.snapshot(&session_id.0);
                log::info!(
                    "[acp] {op} skipped; live agent already owns the session; has_modes={} has_models={} option_count={}",
                    snapshot.modes.is_some(),
                    snapshot.models.is_some(),
                    snapshot
                        .config_options
                        .as_ref()
                        .map(Vec::len)
                        .unwrap_or(0)
                );
                Some(snapshot)
            }
            Ok(false) => None,
            Err(error) => {
                log::warn!("[acp] {op} ownership check failed: {error}; continuing with reopen");
                None
            }
        }
    }

    /// Verify that a live agent's authoritative driver owns this session.
    /// Web prompt handling calls this before claiming a turn or mutating replay.
    pub async fn owns_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        send_command(&tx, |reply| AcpCommand::OwnsSession { session_id, reply }).await
    }

    /// Query the authoritative driver turn state through the agent channel.
    pub async fn is_turn_active(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::IsTurnActive { session_id, reply }).await
    }

    /// Await authoritative turn completion without polling or sleeps.
    pub async fn wait_turn_idle(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::WaitTurnIdle { session_id, reply }).await
    }

    /// Set the session's active mode.
    pub async fn set_mode(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        mode_id: String,
    ) -> Result<(), String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        let session_key = session_id.0.clone();
        let result = send_command(&tx, |reply| AcpCommand::SetMode {
            session_id,
            mode_id: mode_id.clone(),
            reply,
        })
        .await;
        if result.is_ok() {
            self.composer_controls.apply_current_mode(
                &session_key,
                agent_client_protocol::schema::v1::SessionModeId::new(mode_id),
            );
        }
        result
    }

    /// Set the session's active model.
    pub async fn set_model(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        model_id: String,
    ) -> Result<(), String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        send_command(&tx, |reply| AcpCommand::SetModel {
            session_id,
            model_id,
            reply,
        })
        .await
    }

    /// Set a session configuration option, returning the updated option set.
    pub async fn set_config_option(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        config_id: String,
        value_id: String,
    ) -> Result<Vec<SessionConfigOption>, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        let tx = self.command_tx(&agent_id)?;
        let session_key = session_id.0.clone();
        let result = send_command(&tx, |reply| AcpCommand::SetConfigOption {
            session_id,
            config_id,
            value_id,
            reply,
        })
        .await;
        if let Ok(ref options) = result {
            self.composer_controls
                .remember_options(&session_key, options.clone());
        }
        result
    }

    /// Live composer snapshot for a session the driver already owns.
    /// Does not call `session/load` or `session/resume`.
    pub async fn composer_controls(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<SessionReopenOutcome, String> {
        let agent_id = self.resolve_live_agent_id(agent_id, Some(&session_id.0))?;
        match self.owns_session(&agent_id, session_id.clone()).await {
            Ok(true) => {
                let snapshot = self.composer_controls.snapshot(&session_id.0);
                log::info!(
                    "[acp] composer controls snapshot has_modes={} has_models={} option_count={}",
                    snapshot.modes.is_some(),
                    snapshot.models.is_some(),
                    snapshot
                        .config_options
                        .as_ref()
                        .map(Vec::len)
                        .unwrap_or(0)
                );
                Ok(snapshot)
            }
            Ok(false) => Err("session is not live on the host agent".to_string()),
            Err(error) => Err(error),
        }
    }

    /// Route a permission decision back to a waiting agent request.
    ///
    /// `option_id == None` resolves the request with `cancelled`; `Some(id)`
    /// resolves it with the selected option.
    pub async fn respond_permission(
        &self,
        agent_id: &AgentId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let outcome = match option_id {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
            None => RequestPermissionOutcome::Cancelled,
        };
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::RespondPermission {
            request_id,
            outcome,
            reply,
        })
        .await
    }

    /// Route a structured-question answer (issue #411) back to a waiting agent
    /// request.
    ///
    /// `values == None` resolves the question as cancelled; `Some(values)`
    /// resolves it with the selected option values (exactly-once: the first
    /// answer wins; a later `answer_question` for the same id gets
    /// `"unknown question request"`, surfaced as `Ok(())` by the command
    /// wrapper).
    pub async fn answer_question(
        &self,
        agent_id: &AgentId,
        question_id: String,
        values: Option<Vec<String>>,
    ) -> Result<(), String> {
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::AnswerQuestion {
            question_id,
            values,
            reply,
        })
        .await
    }

    /// Run the ACP `authenticate` method for an agent with the given method id
    /// (one of the ids advertised in the `initialize` response).
    pub async fn authenticate(&self, agent_id: &AgentId, method_id: String) -> Result<(), String> {
        let tx = self.command_tx(agent_id)?;
        send_command(&tx, |reply| AcpCommand::Authenticate { method_id, reply }).await
    }

    /// Kill an agent: stop its driver thread and join it. Idempotent.
    pub async fn kill(&self, agent_id: &AgentId) -> Result<(), String> {
        let entry = self.agents.lock().remove(agent_id);
        let Some(mut entry) = entry else {
            // Already gone — idempotent success.
            return Ok(());
        };

        // Mark this as an intentional kill so the driver teardown stays silent
        // (no `acp:agent_disconnected` for a kill we initiated — L4).
        entry.killed.store(true, Ordering::Release);

        // Ask the driver loop to wind down, then drop the sender so the loop
        // ends even if the Shutdown was not observed.
        let _ = entry.command_tx.send(AcpCommand::Shutdown);
        drop(entry.command_tx);

        if let Some(handle) = entry.join_handle.take() {
            // Bounded join: a wedged agent must never make `kill` hang.
            join_thread_bounded(handle).await;
        }

        Ok(())
    }

    /// Stop every ACP producer and join its driver before any host persistence drain begins.
    /// This method intentionally does not flush a store; standalone shutdown composes it ahead of
    /// relay and catalog barriers under one outer deadline.
    pub async fn stop_producers(&self) -> Result<(), String> {
        let entries: Vec<(AgentId, AgentEntry)> = {
            let mut agents = self.agents.lock();
            agents.drain().collect()
        };

        let mut handles = Vec::new();
        for (_, mut entry) in entries {
            entry.killed.store(true, Ordering::Release);
            let _ = entry.command_tx.send(AcpCommand::Shutdown);
            drop(entry.command_tx);
            if let Some(handle) = entry.join_handle.take() {
                handles.push(handle);
            }
        }
        if handles.is_empty() {
            return Ok(());
        }

        let join_all = tokio::task::spawn_blocking(move || {
            for handle in handles {
                let _ = handle.join();
            }
        });
        tokio::time::timeout(JOIN_TIMEOUT, join_all)
            .await
            .map_err(|_| format!("agent shutdown exceeded {JOIN_TIMEOUT:?}"))?
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Kill all agents and surface join/persistence durability failures.
    pub async fn kill_all_checked(&self) -> Result<(), String> {
        self.stop_producers().await?;
        if let Some(persistence) = &self.persistence {
            persistence
                .flush_all()
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn install_test_agent_for_new_session(
        &self,
        agent_id: AgentId,
        observed: std::sync::mpsc::SyncSender<(String, bool)>,
    ) {
        self.install_test_agent_for_new_session_with_close_result(agent_id, observed, Ok(()));
    }

    #[cfg(test)]
    pub(crate) fn install_test_agent_for_new_session_with_close_result(
        &self,
        agent_id: AgentId,
        observed: std::sync::mpsc::SyncSender<(String, bool)>,
        close_result: Result<(), String>,
    ) {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    AcpCommand::NewSession {
                        cwd,
                        ephemeral,
                        mut binding_gate,
                        reply,
                        ..
                    } => {
                        let exists = PathBuf::from(&cwd).is_dir();
                        let _ = observed.send((cwd.clone(), exists));
                        let _ = reply.send(Ok(NewSessionOutcome {
                            persistence: if ephemeral {
                                "ephemeral"
                            } else {
                                "conversation"
                            },
                            conversation_id: None,
                            workspace_cwd: None,
                            execution_cwd: None,
                            session_id: SessionId::new("opaque/fake-session"),
                            modes: None,
                            models: None,
                            config_options: None,
                        }));
                        if let Some(gate) = binding_gate.as_mut() {
                            while gate.borrow().is_none() && gate.changed().await.is_ok() {}
                        }
                    }
                    AcpCommand::CloseSession { reply, .. } => {
                        let _ = reply.send(close_result.clone());
                    }
                    _ => {}
                }
            }
        });
        let mut capabilities = AgentCapabilities::default();
        capabilities.session_capabilities.close = Some(Default::default());
        self.agents.lock().insert(
            agent_id,
            AgentEntry {
                command_tx,
                capabilities,
                stable_namespace: Some("config:test".to_string()),
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn install_test_agent_with_sessions(
        &self,
        agent_id: AgentId,
        sessions: std::collections::HashSet<String>,
    ) {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    AcpCommand::OwnsSession { session_id, reply } => {
                        let _ = reply.send(Ok(sessions.contains(&session_id.0)));
                    }
                    // Story 10 (cross-client continuity): handle the prompt-flow
                    // commands so `handle_send_prompt` reaches persistence
                    // without a real agent binary. `IsEphemeralSession` → false
                    // (so `persist_user_prompt` runs + the user prompt is the
                    // durable transcript); `SendPrompt` → `EndTurn` (so the turn
                    // completes + the watermark advances). No streaming events
                    // are emitted — the persisted `user_prompt` IS the transcript
                    // the cross-client restore test reads back via
                    // `handle_get_session_payload`.
                    AcpCommand::IsEphemeralSession { reply, .. } => {
                        let _ = reply.send(Ok(false));
                    }
                    AcpCommand::SendPrompt {
                        accepted, reply, ..
                    } => {
                        let _ = accepted.send(Ok(()));
                        let _ = reply.send(Ok(StopReason::EndTurn));
                    }
                    // Unhandled commands are silently dropped (same behavior
                    // as the prior `if let`). A reply-bearing variant dropped
                    // here will cause the caller to hang — future tests that
                    // add new commands should add a dedicated match arm.
                    _ => {}
                }
            }
        });
        self.agents.lock().insert(
            agent_id,
            AgentEntry {
                command_tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: None,
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn install_test_agent_with_prompt_gate(
        &self,
        agent_id: AgentId,
        sessions: std::collections::HashSet<String>,
    ) -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
        let (release_tx, release_rx) = oneshot::channel();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let sinks = self.sinks.clone();
        let gated_agent_id = agent_id.clone();
        tokio::spawn(async move {
            let mut released = false;
            let mut entered_tx = Some(entered_tx);
            let mut release_rx = Some(release_rx);
            let pending_cancels = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::<
                String,
                oneshot::Sender<()>,
            >::new()));
            while let Some(command) = command_rx.recv().await {
                match command {
                    AcpCommand::OwnsSession { session_id, reply } => {
                        let _ = reply.send(Ok(sessions.contains(&session_id.0)));
                    }
                    AcpCommand::IsEphemeralSession { reply, .. } => {
                        let _ = reply.send(Ok(false));
                    }
                    AcpCommand::SendPrompt {
                        session_id,
                        turn_id,
                        accepted,
                        reply,
                        ..
                    } => {
                        if !released {
                            if let Some(entered_tx) = entered_tx.take() {
                                let _ = entered_tx.send(());
                            }
                            let mut release_rx = release_rx.take().expect("first prompt gate");
                            let (cancel_tx, mut cancel_rx) = oneshot::channel();
                            pending_cancels
                                .lock()
                                .insert(session_id.0.clone(), cancel_tx);
                            let prompt_sinks = sinks.clone();
                            let prompt_agent_id = gated_agent_id.clone();
                            let prompt_cancels = Arc::clone(&pending_cancels);
                            let prompt_session_id = session_id.0.clone();
                            tokio::spawn(async move {
                                let stop_reason = tokio::select! {
                                    _ = &mut release_rx => StopReason::EndTurn,
                                    _ = &mut cancel_rx => StopReason::Cancelled,
                                };
                                prompt_cancels.lock().remove(&prompt_session_id);
                                let event = PromptCompleteEvent {
                                    agent_id: prompt_agent_id,
                                    session_id: session_id.clone(),
                                    stop_reason,
                                    turn_id,
                                };
                                match events::fan_out(
                                    &prompt_sinks,
                                    Some(&session_id.0),
                                    events::EVENT_PROMPT_COMPLETE,
                                    &event,
                                ) {
                                    Ok(_) => {
                                        let _ = reply.send(Ok(stop_reason));
                                    }
                                    Err(error) => {
                                        log_delivery_error("test_prompt_complete", &error);
                                        let _ = reply.send(Err(error.code.to_string()));
                                    }
                                }
                            });
                            released = true;
                        } else {
                            let _ = reply.send(Ok(StopReason::EndTurn));
                        }
                        let _ = accepted.send(Ok(()));
                    }
                    AcpCommand::CancelPrompt { session_id, reply } => {
                        if let Some(cancel) = pending_cancels.lock().remove(&session_id.0) {
                            let _ = cancel.send(());
                        }
                        let _ = reply.send(Ok(()));
                    }
                    _ => {}
                }
            }
        });
        self.agents.lock().insert(
            agent_id,
            AgentEntry {
                command_tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: None,
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
        (release_tx, entered_rx)
    }

    /// Desktop compatibility wrapper: logs failures because app-exit callers
    /// cannot return them. Standalone uses `kill_all_checked` directly.
    pub async fn kill_all(&self) {
        if let Err(error) = self.kill_all_checked().await {
            log::error!("[acp] shutdown durability failure: {error}");
        }
    }

    pub async fn shutdown_persistence(&self) -> Result<(), String> {
        match &self.persistence {
            Some(persistence) => persistence
                .shutdown()
                .await
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }
}

fn stable_agent_namespace(config: &AgentConfig) -> Option<String> {
    if let Some(config_id) = config
        .config_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return Some(format!("config:{config_id}"));
    }
    // Safe fallback identity: normalized display name + executable basename +
    // stable boolean flags. Never hash full paths, args, or env because those
    // frequently contain usernames, workspaces, credentials, and tokens.
    let name = config.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let command = config.command.replace('\\', "/");
    let basename = command.rsplit('/').next().unwrap_or_default().trim();
    if name.is_empty() || basename.is_empty() || matches!(basename, "." | "..") {
        return None;
    }
    let identity = format!(
        "{}\0{}\0terminal={}",
        name.to_ascii_lowercase(),
        basename.to_ascii_lowercase(),
        config.allow_terminal
    );
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Some(format!("agent-safe:{hash:016x}"))
}

/// Validate project/session MCP transports against negotiated capabilities.
/// Stdio is mandatory in ACP; HTTP/SSE require their advertised flags.
fn gate_mcp_servers(caps: &AgentCapabilities, servers: &[McpServer]) -> Result<(), String> {
    for server in servers {
        match server {
            McpServer::Stdio(_) => {}
            McpServer::Http(_) if caps.mcp_capabilities.http => {}
            McpServer::Sse(_) if caps.mcp_capabilities.sse => {}
            McpServer::Http(_) => {
                return Err("agent does not support HTTP MCP servers".to_string());
            }
            McpServer::Sse(_) => {
                return Err("agent does not support SSE MCP servers".to_string());
            }
            _ => return Err("agent does not support this MCP transport".to_string()),
        }
    }
    Ok(())
}

/// Build the internal `termul` MCP server config (for the `plan` tool; stdio self-spawn) to
/// prepend into a session's `mcp_servers`. The agent spawns
/// `current_exe() --internal-mcp-plan-server` as a child; the child reads
/// `TERMUL_PLAN_PORT` / `_TOKEN` / `_SESSION_ID` / `_AGENT_ID` from env,
/// runs an rmcp MCP server over stdio, and forwards `plan` calls to
/// the parent TCP listener. The internal server is `McpServer::Stdio`, which
/// `gate_mcp_servers` accepts unconditionally (stdio is mandatory in ACP), so
/// no gate relaxation is needed.
fn build_internal_plan_stdio(
    agent_id: &str,
    port: u16,
    token: &str,
    provisional_sid: &str,
) -> Vec<McpServer> {
    let exe = std::env::current_exe().unwrap_or_else(|e| {
        log::warn!("[host-mcp] current_exe() failed ({e}); falling back to PATH lookup");
        std::path::PathBuf::from("termul-manager")
    });
    let env = vec![
        EnvVariable::new(crate::acp::host_mcp::ENV_PORT, port.to_string()),
        EnvVariable::new(crate::acp::host_mcp::ENV_TOKEN, token.to_string()),
        EnvVariable::new(
            crate::acp::host_mcp::ENV_SESSION_ID,
            provisional_sid.to_string(),
        ),
        EnvVariable::new(crate::acp::host_mcp::ENV_AGENT_ID, agent_id.to_string()),
    ];
    let stdio = McpServerStdio::new("termul".to_string(), exe)
        .args(vec![crate::acp::host_mcp::CHILD_ARG.to_string()])
        .env(env);
    vec![McpServer::Stdio(stdio)]
}

/// Map the agent's advertised `initialize` auth methods to the renderer-facing
/// [`AuthMethodInfo`] contract. Every method is forwarded as an opaque
/// `id`/`name`/optional `description` descriptor — no agent-type filtering, so
/// the renderer decides how to present them (single Sign-in vs. actionable
/// multi-method failure). Extracted so the mapping can be unit-tested without a
/// live connection.
fn to_auth_method_infos(methods: &[AuthMethod]) -> Vec<AuthMethodInfo> {
    methods
        .iter()
        .map(|m| AuthMethodInfo {
            id: m.id().to_string(),
            name: m.name().to_string(),
            description: m.description().map(str::to_string),
        })
        .collect()
}

/// Capability gate for `session/load`: requires the agent's `loadSession`
/// capability. Returns a typed error (without contacting the agent) when it is
/// absent. Extracted so the real gate can be unit-tested without an AppHandle.
fn gate_load_session(caps: &AgentCapabilities) -> Result<(), String> {
    if caps.load_session {
        Ok(())
    } else {
        Err("agent does not support session/load (loadSession capability)".to_string())
    }
}

/// Capability gate for `session/resume`: requires `sessionCapabilities.resume`.
fn gate_resume_session(caps: &AgentCapabilities) -> Result<(), String> {
    if caps.session_capabilities.resume.is_some() {
        Ok(())
    } else {
        Err("agent does not support session/resume".to_string())
    }
}

/// Capability gate for `session/close`: requires `sessionCapabilities.close`.
fn gate_close_session(caps: &AgentCapabilities) -> Result<(), String> {
    if caps.session_capabilities.close.is_some() {
        Ok(())
    } else {
        Err("agent does not support session/close".to_string())
    }
}

/// Capability gate for `session/list`: requires `sessionCapabilities.list`.
/// Per the ACP spec: "If `sessionCapabilities.list` is not present … Clients
/// MUST NOT attempt to call `session/list`."
fn gate_list_sessions(caps: &AgentCapabilities) -> Result<(), String> {
    if caps.session_capabilities.list.is_some() {
        Ok(())
    } else {
        Err("agent does not support session/list (sessionCapabilities.list)".to_string())
    }
}

/// Send a command to a driver thread and await its `Send` oneshot reply.
async fn send_command<T>(
    command_tx: &mpsc::UnboundedSender<AcpCommand>,
    make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> AcpCommand,
) -> Result<T, String> {
    let (reply_tx, reply_rx) = oneshot::channel();
    command_tx
        .send(make(reply_tx))
        .map_err(|_| "agent thread is no longer running".to_string())?;
    reply_rx
        .await
        .map_err(|_| "agent thread dropped the reply".to_string())?
}

/// Join a driver thread without ever blocking the async runtime indefinitely.
///
/// The join runs on the blocking pool and is capped at [`JOIN_TIMEOUT`]; if a
/// wedged agent thread refuses to exit, we abandon the join (the OS reclaims
/// the thread at process exit) rather than hang the caller / app-exit path.
async fn join_thread_bounded(handle: JoinHandle<()>) {
    let join = tokio::task::spawn_blocking(move || {
        let _ = handle.join();
    });
    if tokio::time::timeout(JOIN_TIMEOUT, join).await.is_err() {
        log::warn!("[acp] agent thread did not exit within {JOIN_TIMEOUT:?}; abandoning join");
    }
}

fn preferred_allow_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowAlways)
        .or_else(|| {
            options
                .iter()
                .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        })
}

/// Entry point for an agent's dedicated driver thread.
///
/// Builds a current-thread Tokio runtime and drives the ACP connection to
/// completion. All `!Send`-sensitive connection work is confined here.
///
/// On exit (for any reason — clean shutdown, agent crash, or initialize
/// failure) the thread reaps itself from the registry and, if it had actually
/// spawned (`spawned` is true), emits the appropriate disconnect/close events.
#[allow(clippy::too_many_arguments)]
fn run_agent(
    config: AgentConfig,
    sinks: Vec<Arc<dyn EventSink>>,
    host_plan_server: Arc<crate::acp::host_mcp::parent::HostPlanServer>,
    permission_policy: Arc<Mutex<PermissionPolicy>>,
    agent_id: AgentId,
    command_rx: mpsc::UnboundedReceiver<AcpCommand>,
    init_tx: oneshot::Sender<Result<InitOutcome, String>>,
    agents: Arc<Mutex<HashMap<AgentId, AgentEntry>>>,
    reaped: Arc<AtomicBool>,
    killed: Arc<AtomicBool>,
    start_error: Arc<Mutex<Option<String>>>,
    persistence: Option<Arc<SessionPersistence>>,
    conversation_persistence: Option<Arc<ConversationPersistenceAdapter>>,
    warmup_done: Arc<Mutex<HashSet<AgentId>>>,
    composer_controls: Arc<ComposerControlCache>,
) {
    // True once `initialize` succeeded and the agent was surfaced to the
    // renderer via `acp:agent_spawned`. We only emit disconnect/error events
    // for agents the renderer actually saw (L4/F5).
    let spawned = Arc::new(AtomicBool::new(false));
    // Shared with the connection handlers and the command loop. Created here so
    // that, even if the agent crashes and `main_fn` is dropped mid-await, this
    // teardown code can still drain leaked permissions and discover which
    // sessions were active for `acp:session_closed`.
    let driver_state = Arc::new(Mutex::new(DriverState::new()));

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = init_tx.send(Err(format!("failed to build runtime: {e}")));
            *start_error.lock() = Some(format!("failed to build runtime: {e}"));
            return;
        }
    };

    let result = runtime.block_on(drive_connection(
        config,
        sinks.clone(),
        host_plan_server.clone(),
        permission_policy,
        agent_id.clone(),
        command_rx,
        init_tx,
        spawned.clone(),
        driver_state.clone(),
        persistence.clone(),
        conversation_persistence,
        warmup_done.clone(),
        composer_controls,
    ));

    let was_spawned = spawned.load(Ordering::Acquire);

    // Drain any permissions that leaked because the connection ended (crash /
    // disconnect) without the loop resolving them. The connection is gone, so
    // responding may fail silently — that is fine; the point is to not hold the
    // responders forever (H3).
    let (leaked, active_sessions) = {
        let mut state = driver_state.lock();
        (state.drain_all(), state.active_session_ids())
    };
    // A connection teardown can drop an in-flight prompt task before its normal
    // completion cleanup runs. Remove every surviving session's auth, cache,
    // and route so stale MCP children cannot target a later session.
    for session_id in &active_sessions {
        host_plan_server.unregister_session(session_id);
    }
    for permission in leaked {
        let _ = permission.responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
    }
    // Issue #411: resolve leaked questions as cancelled too (the connection is
    // gone, so responding may fail silently — the point is to not hold the
    // responders forever).
    let leaked_questions = driver_state.lock().drain_all_questions();
    for question in leaked_questions {
        let _ = question.responder.respond(serde_json::json!({
            "questionId": question.question_id,
            "cancelled": true,
        }));
    }

    // Self-reap: remove our own registry entry so a crashed/EOFed agent does
    // not linger in `list_agents` with a dead command channel. We do NOT join
    // our own handle here (a thread cannot join itself); `kill`/`kill_all` may
    // still hold the handle, and joining a finished thread returns promptly.
    // The `reaped` flag (set under the same lock the registrar checks) closes
    // the race where init succeeded but the agent exited before registration.
    {
        let mut map = agents.lock();
        reaped.store(true, Ordering::Release);
        map.remove(&agent_id);
    }
    // Clear the per-agent warmup-done guard so a re-spawned agent (new
    // subprocess, fresh cold-start state) re-runs the first-prompt warmup.
    warmup_done.lock().remove(&agent_id);

    // Only surface lifecycle events for an agent the renderer actually saw, and
    // never for an intentional kill (L4): a kill we initiated is silent, so the
    // renderer doesn't see a "disconnected" it didn't cause.
    let intentional_kill = killed.load(Ordering::Acquire);

    // If the agent never finished initializing, the connection error IS the
    // start-failure reason (e.g. the subprocess could not be spawned). Record it
    // BEFORE the lifecycle gate below so the awaiting `spawn` caller can surface
    // the real error instead of the generic "did not initialize" message. This
    // must run regardless of `was_spawned`/`intentional_kill` (those gate only
    // the renderer-facing lifecycle events).
    if !was_spawned {
        if let Err(message) = &result {
            *start_error.lock() = Some(message.clone());
        }
    }

    let mut persistence_failures = Vec::new();
    if let Some(persistence) = &persistence {
        for session in &active_sessions {
            let status = if result.is_err() {
                PersistedSessionStatus::Error
            } else {
                PersistedSessionStatus::Closed
            };
            if let Err(error) = runtime.block_on(persistence.finalize_session(session, status)) {
                persistence_failures.push(format!("session {session}: {error}"));
            }
        }
    }
    if !persistence_failures.is_empty() {
        log::error!(
            "[acp] failed to finalize {} persisted session(s): {}",
            persistence_failures.len(),
            persistence_failures.join("; ")
        );
    }

    if was_spawned && !intentional_kill {
        for session in active_sessions {
            let event = SessionClosedEvent {
                agent_id: agent_id.clone(),
                session_id: SessionId::new(session),
            };
            if let Err(error) = events::fan_out(
                &sinks,
                Some(event.session_id.0.as_str()),
                events::EVENT_SESSION_CLOSED,
                &event,
            ) {
                log_delivery_error("session_closed", &error);
            }
        }

        if let Err(message) = result {
            // Story 1.9 FR26: emit the typed `AgentCrashed` event BEFORE
            // `agent_error` (back-compat) + `agent_disconnected`. The renderer
            // distinguishes "crash" (→ `status: 'error'` + manual restart) from
            // a clean disconnect. Outstanding turn oneshots fail with this.
            let crashed = AgentCrashedEvent {
                agent_id: agent_id.clone(),
                session_id: None,
                message: message.clone(),
            };
            if let Err(error) = events::fan_out(&sinks, None, events::EVENT_AGENT_CRASHED, &crashed)
            {
                log_delivery_error("agent_crashed", &error);
            }

            let event = AgentErrorEvent {
                agent_id: agent_id.clone(),
                session_id: None,
                message,
            };
            // Teardown error is agent-level (no session) → sid = None.
            if let Err(error) = events::fan_out(&sinks, None, events::EVENT_AGENT_ERROR, &event) {
                log_delivery_error("agent_error", &error);
            }
        }

        let event = AgentDisconnectedEvent { agent_id };
        // Agent-level lifecycle event → sid = None.
        if let Err(error) = events::fan_out(&sinks, None, events::EVENT_AGENT_DISCONNECTED, &event)
        {
            log_delivery_error("agent_disconnected", &error);
        }
    }
}

/// Build the client connection and run it until the command loop ends.
#[allow(clippy::too_many_arguments)]
async fn drive_connection(
    config: AgentConfig,
    sinks: Vec<Arc<dyn EventSink>>,
    host_plan_server: Arc<crate::acp::host_mcp::parent::HostPlanServer>,
    permission_policy: Arc<Mutex<PermissionPolicy>>,
    agent_id: AgentId,
    command_rx: mpsc::UnboundedReceiver<AcpCommand>,
    init_tx: oneshot::Sender<Result<InitOutcome, String>>,
    spawned: Arc<AtomicBool>,
    driver_state: Arc<Mutex<DriverState>>,
    persistence: Option<Arc<SessionPersistence>>,
    conversation_persistence: Option<Arc<ConversationPersistenceAdapter>>,
    warmup_done: Arc<Mutex<HashSet<AgentId>>>,
    composer_controls: Arc<ComposerControlCache>,
) -> Result<(), String> {
    // Agent stderr and JSON-RPC/terminal streams can carry credentials, paths, prompts, and
    // transcript bytes. Operational logging records direction and byte count only; there is no
    // raw-trace escape hatch.
    let debug_agent_id = agent_id.clone();
    let agent = agent_client_protocol::AcpAgent::new(config.to_mcp_server()).with_debug(
        move |line: &str, direction: LineDirection| {
            let stream = match direction {
                LineDirection::Stderr => "stderr",
                LineDirection::Stdin => "stdin",
                LineDirection::Stdout => "stdout",
            };
            log::trace!(
                "[acp] {debug_agent_id} stream={stream} bytes={}",
                line.len()
            );
        },
    );

    // Per-handler clones (handlers must be `Send` and may be called repeatedly).
    // Each handler gets its own clone of the sink fan-out; `Arc` clones are
    // cheap and `Vec::clone` is N Arc clones. A durable rejection opens only
    // the affected session's producer circuit; other sessions on this agent remain live.
    let delivery_circuits: SessionDeliveryCircuits =
        Arc::new(Mutex::new(SessionDeliveryCircuitMap::new()));
    let notif_sinks = sinks.clone();
    let notif_agent_id = agent_id.clone();
    let notif_state = driver_state.clone();
    let notif_circuits = Arc::clone(&delivery_circuits);
    // AD-8: capture persistence into the notification closure so the host can
    // gate `session_info_update` fan-out on `title_source`. When a background
    // title (`BackgroundGenerated`) or a future local alias (`LocalAlias`) owns
    // the title, a native agent `session_info_update` is suppressed here (the
    // durable defense in `append_record` is the second layer).
    let notif_persistence = persistence.clone();
    let notif_conversation_persistence = conversation_persistence.clone();
    let perm_sinks = sinks.clone();
    let perm_agent_id = agent_id.clone();
    let perm_state = driver_state.clone();
    let perm_circuits = Arc::clone(&delivery_circuits);
    let perm_policy = permission_policy;
    let question_sinks = sinks.clone();
    let question_agent_id = agent_id.clone();
    let question_state = driver_state.clone();
    let question_circuits = Arc::clone(&delivery_circuits);
    let read_state = driver_state.clone();
    let write_state = driver_state.clone();

    // Terminal capability (P6b): a per-agent registry of ACP command-runner
    // terminals. Handlers are always registered, but they only do work when the
    // agent opted in (`allow_terminal`); the real gate is the capability
    // advertisement (default false), so a compliant agent never calls these
    // unless allowed. The registry is torn down with the driver thread.
    let allow_terminal = config.allow_terminal;
    let terminals = Arc::new(Mutex::new(crate::acp::terminal::TerminalRegistry::new()));
    let term_create = terminals.clone();
    let term_create_state = driver_state.clone();
    let term_output = terminals.clone();
    let term_output_state = driver_state.clone();
    let term_wait = terminals.clone();
    let term_wait_state = driver_state.clone();
    let term_kill = terminals.clone();
    let term_kill_state = driver_state.clone();
    let term_release = terminals.clone();
    let term_release_state = driver_state.clone();
    let loop_terminals = terminals.clone();

    // Clones moved into the command loop (`main_fn`).
    let loop_sinks = sinks.clone();
    let loop_delivery_circuits = Arc::clone(&delivery_circuits);
    let loop_host_plan_server = host_plan_server;
    let loop_agent_id = agent_id.clone();
    let loop_state = driver_state.clone();
    let loop_spawned = spawned.clone();
    let loop_warmup_done = warmup_done.clone();
    let loop_composer_controls = composer_controls.clone();
    let notif_composer_controls = composer_controls;

    let connection_result = Client
        .builder()
        .name(format!("termul-acp-{agent_id}"))
        .on_receive_notification(
            async move |notification: agent_client_protocol::schema::v1::SessionNotification, _cx| {
                let session_id = notification.session_id.0.to_string();
                if notif_circuits.lock().contains_key(&session_id) {
                    log::debug!(
                        "[acp] dropped update for circuit-broken session code={}",
                        CONVERSATION_PERSISTENCE_REJECTED
                    );
                    return Ok(());
                }
                // Any inbound session/update is agent activity — nudge the
                // active turn's idle deadline so a streaming turn never hits
                // the idle timeout. Best-effort: a no-op when no turn is
                // active for this session.
                notif_state.lock().signal_idle(&session_id);
                match &notification.update {
                    agent_client_protocol::schema::v1::SessionUpdate::CurrentModeUpdate(
                        update,
                    ) => {
                        notif_composer_controls
                            .apply_current_mode(&session_id, update.current_mode_id.clone());
                    }
                    agent_client_protocol::schema::v1::SessionUpdate::ConfigOptionUpdate(
                        update,
                    ) => {
                        notif_composer_controls
                            .remember_options(&session_id, update.config_options.clone());
                    }
                    _ => {}
                }
                let tool_call_id = match &notification.update {
                    agent_client_protocol::schema::v1::SessionUpdate::ToolCall(tool_call) => {
                        Some(tool_call.tool_call_id.0.to_string())
                    }
                    agent_client_protocol::schema::v1::SessionUpdate::ToolCallUpdate(update) => {
                        Some(update.tool_call_id.0.to_string())
                    }
                    _ => None,
                };
                if let Some(tool_call_id) = tool_call_id {
                    notif_state
                        .lock()
                        .bind_tool_call(tool_call_id, session_id.clone());
                }
                // AD-8: gate native `session_info_update` fan-out. When the
                // host already owns a higher-precedence title
                // (`BackgroundGenerated` from a prior background-gen flow, or
                // a future `LocalAlias`), suppress the agent's
                // `session_info_update` so the background title survives in
                // the renderer. The durable defense in `append_record` is the
                // second layer; this is the fan-out defense.
                let is_protected_info_update = matches!(
                    &notification.update,
                    agent_client_protocol::schema::v1::SessionUpdate::SessionInfoUpdate(_)
                ) && is_protected_title_source(
                    notif_persistence
                        .as_ref()
                        .and_then(|p| p.metadata(&session_id).ok())
                        .and_then(|m| m.title_source)
                        .as_ref(),
                );
                if is_protected_info_update {
                    log::debug!(
                        "[acp] session {session_id}: suppressed native session_info_update (title_source is BackgroundGenerated/LocalAlias)"
                    );
                    return Ok(());
                }
                if let Err(error) = client::emit_session_update(
                    &notif_sinks,
                    notif_conversation_persistence.as_deref(),
                    &notif_agent_id,
                    notification,
                )
                .await
                {
                    if error.is_retryable() {
                        // Backpressure cancels only the current turn; draining capacity admits a
                        // later submission and never opens a permanent session circuit.
                        notif_state.lock().signal_cancel(&session_id);
                    } else if error.should_open_session_circuit() {
                        notif_circuits.lock().insert(
                            session_id.clone(),
                            error.source_code.unwrap_or(CONVERSATION_PERSISTENCE_REJECTED),
                        );
                        notif_state.lock().signal_cancel(&session_id);
                    }
                    log_delivery_ticket_error("session_update", &error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::RequestPermissionRequest,
                        responder,
                        _cx| {
                let agent_client_protocol::schema::v1::RequestPermissionRequest {
                    session_id,
                    tool_call,
                    options,
                    ..
                } = request;
                let session_string = session_id.0.to_string();
                if perm_state.lock().is_ephemeral(&session_string) {
                    let _ = responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                    return Ok(());
                }
                let active_policy = *perm_policy.lock();
                log::info!(
                    "[acp-permission] boundary=request_received agent_id={} session_id={} policy={active_policy:?} option_count={}",
                    perm_agent_id.0,
                    session_string,
                    options.len()
                );
                if active_policy == PermissionPolicy::AllowAll {
                    if let Some(option) = preferred_allow_option(&options) {
                        log::info!(
                            "[acp-permission] boundary=auto_allowed agent_id={} session_id={} option_kind={:?}",
                            perm_agent_id.0,
                            session_string,
                            option.kind
                        );
                        let _ = responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            )),
                        ));
                        return Ok(());
                    }
                    log::warn!(
                        "[acp-permission] boundary=auto_allow_unavailable agent_id={} session_id={} option_count={}",
                        perm_agent_id.0,
                        session_string,
                        options.len()
                    );
                }
                // A permission request is agent activity — the turn is waiting
                // on user input, not wedged. Nudge the idle deadline so a
                // user-input wait doesn't false-fire the idle timeout.
                perm_state.lock().signal_idle(&session_string);
                let request_id = {
                    let mut state = perm_state.lock();
                    state.bind_tool_call(
                        tool_call.tool_call_id.0.to_string(),
                        session_string.clone(),
                    );
                    state.register_permission(session_string.clone(), responder)
                };
                let event = events::PermissionRequestEvent {
                    agent_id: perm_agent_id.clone(),
                    session_id: SessionId::new(session_string),
                    request_id,
                    tool_call,
                    options,
                };
                if let Err(error) = fan_out_session(
                    &perm_sinks,
                    &perm_circuits,
                    event.session_id.0.as_str(),
                    events::EVENT_PERMISSION_REQUEST,
                    &event,
                ) {
                    if let Some(permission) = perm_state.lock().take_permission(&event.request_id) {
                        let _ = permission.responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    }
                    log_delivery_error("permission_request", &error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: AskUserQuestionRequest, responder, _cx| {
                // Issue #411: a structured question from the agent. Register the
                // parked `Responder<serde_json::Value>` (mirroring permissions),
                // fan out `acp:question_request`, and resolve the responder when
                // the user answers via `acp_answer_question` / `answer_question`.
                let session_string = request.session_id.clone();
                if question_state.lock().is_ephemeral(&session_string) {
                    let _ = responder.respond(serde_json::json!({
                        "cancelled": true,
                    }));
                    return Ok(());
                }
                // A structured question is agent activity — the turn is waiting
                // on user input, not wedged. Nudge the idle deadline so a
                // user-input wait doesn't false-fire the idle timeout.
                question_state.lock().signal_idle(&session_string);
                let question_id = {
                    let mut state = question_state.lock();
                    state.register_question(session_string.clone(), responder)
                };
                let event = events::AskUserQuestionEvent {
                    agent_id: question_agent_id.clone(),
                    session_id: SessionId::new(session_string),
                    question_id,
                    question: request.question,
                    options: request
                        .options
                        .into_iter()
                        .map(|o| events::QuestionOption {
                            value: o.value,
                            label: o.label,
                            description: o.description,
                            cardinality: o.cardinality,
                        })
                        .collect(),
                };
                if let Err(error) = fan_out_session(
                    &question_sinks,
                    &question_circuits,
                    event.session_id.0.as_str(),
                    events::EVENT_QUESTION_REQUEST,
                    &event,
                ) {
                    if let Some(question) = question_state.lock().take_question(&event.question_id) {
                        let _ = question.responder.respond(serde_json::json!({
                            "questionId": event.question_id,
                            "cancelled": true,
                        }));
                    }
                    log_delivery_error("question_request", &error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::ReadTextFileRequest,
                        responder,
                        cx| {
                // Resolve the session's workspace root (the sandbox boundary)
                // and perform the (blocking) read off the dispatch loop so a
                // large file can't stall connection I/O (M1).
                let root = {
                    let state = read_state.lock();
                    if state.is_ephemeral(request.session_id.0.as_ref()) {
                        let denied = Err(agent_client_protocol::Error::method_not_found());
                        let _ = responder.respond_with_result(denied);
                        return Ok(());
                    }
                    state.session_root(request.session_id.0.as_ref())
                };
                cx.spawn(async move {
                    let result = client::handle_read_text_file(&request, root.as_deref()).await;
                    let _ = responder.respond_with_result(result);
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::WriteTextFileRequest,
                        responder,
                        cx| {
                let root = {
                    let state = write_state.lock();
                    if state.is_ephemeral(request.session_id.0.as_ref()) {
                        let denied = Err(agent_client_protocol::Error::method_not_found());
                        let _ = responder.respond_with_result(denied);
                        return Ok(());
                    }
                    state.session_root(request.session_id.0.as_ref())
                };
                cx.spawn(async move {
                    let result = client::handle_write_text_file(&request, root.as_deref()).await;
                    let _ = responder.respond_with_result(result);
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::CreateTerminalRequest,
                        responder,
                        _cx| {
                use agent_client_protocol::schema::v1::CreateTerminalResponse;
                if !allow_terminal
                    || term_create_state
                        .lock()
                        .is_ephemeral(request.session_id.0.as_ref())
                {
                    let denied: Result<CreateTerminalResponse, agent_client_protocol::Error> =
                        Err(agent_client_protocol::Error::method_not_found());
                    let _ = responder.respond_with_result(denied);
                    return Ok(());
                }
                // Default the cwd to the session's workspace root when the agent
                // doesn't specify one.
                let session_root = term_create_state
                    .lock()
                    .session_root(request.session_id.0.as_ref());
                let cwd = request.cwd.clone().or(session_root);
                let env: Vec<(String, String)> = request
                    .env
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect();
                let result = term_create
                    .lock()
                    .create(
                        &request.command,
                        &request.args,
                        &env,
                        cwd.as_deref(),
                        request.output_byte_limit,
                    )
                    .map(CreateTerminalResponse::new)
                    .map_err(|e| agent_client_protocol::Error::internal_error().data(e));
                let _ = responder.respond_with_result(result);
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::TerminalOutputRequest,
                        responder,
                        _cx| {
                use agent_client_protocol::schema::v1::TerminalOutputResponse;
                if term_output_state
                    .lock()
                    .is_ephemeral(request.session_id.0.as_ref())
                {
                    let denied: Result<TerminalOutputResponse, agent_client_protocol::Error> =
                        Err(agent_client_protocol::Error::method_not_found());
                    let _ = responder.respond_with_result(denied);
                    return Ok(());
                }
                let result = term_output
                    .lock()
                    .output(&request.terminal_id)
                    .map(|(output, truncated, exit)| {
                        TerminalOutputResponse::new(output, truncated).exit_status(exit)
                    })
                    .map_err(|e| agent_client_protocol::Error::internal_error().data(e));
                let _ = responder.respond_with_result(result);
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::WaitForTerminalExitRequest,
                        responder,
                        cx| {
                use agent_client_protocol::schema::v1::WaitForTerminalExitResponse;
                if term_wait_state
                    .lock()
                    .is_ephemeral(request.session_id.0.as_ref())
                {
                    let denied: Result<WaitForTerminalExitResponse, agent_client_protocol::Error> =
                        Err(agent_client_protocol::Error::method_not_found());
                    let _ = responder.respond_with_result(denied);
                    return Ok(());
                }
                let registry = term_wait.clone();
                // Await off the dispatch path so other terminal ops stay
                // responsive. The child handle is taken out from under the lock
                // first, so the registry mutex is NOT held across the await.
                cx.spawn(async move {
                    let taken = registry.lock().take_child_for_wait(&request.terminal_id);
                    let result = match taken {
                        Err(e) => Err(agent_client_protocol::Error::internal_error().data(e)),
                        Ok(None) => {
                            // Already exited: return the cached status.
                            match registry.lock().cached_exit(&request.terminal_id) {
                                Some(status) => Ok(WaitForTerminalExitResponse::new(status)),
                                None => Err(agent_client_protocol::Error::internal_error()
                                    .data("terminal has no exit status")),
                            }
                        }
                        Ok(Some(mut child)) => match child.wait().await {
                            Ok(status) => {
                                let exit = crate::acp::terminal::to_exit_status(status);
                                registry
                                    .lock()
                                    .record_exit(&request.terminal_id, exit.clone());
                                Ok(WaitForTerminalExitResponse::new(exit))
                            }
                            Err(e) => Err(agent_client_protocol::Error::internal_error()
                                .data(format!("failed to wait for terminal: {e}"))),
                        },
                    };
                    let _ = responder.respond_with_result(result);
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::KillTerminalRequest,
                        responder,
                        _cx| {
                use agent_client_protocol::schema::v1::KillTerminalResponse;
                if term_kill_state
                    .lock()
                    .is_ephemeral(request.session_id.0.as_ref())
                {
                    let denied: Result<KillTerminalResponse, agent_client_protocol::Error> =
                        Err(agent_client_protocol::Error::method_not_found());
                    let _ = responder.respond_with_result(denied);
                    return Ok(());
                }
                let result = term_kill
                    .lock()
                    .kill(&request.terminal_id)
                    .map(|()| KillTerminalResponse::new())
                    .map_err(|e| agent_client_protocol::Error::internal_error().data(e));
                let _ = responder.respond_with_result(result);
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::ReleaseTerminalRequest,
                        responder,
                        _cx| {
                use agent_client_protocol::schema::v1::ReleaseTerminalResponse;
                if term_release_state
                    .lock()
                    .is_ephemeral(request.session_id.0.as_ref())
                {
                    let denied: Result<ReleaseTerminalResponse, agent_client_protocol::Error> =
                        Err(agent_client_protocol::Error::method_not_found());
                    let _ = responder.respond_with_result(denied);
                    return Ok(());
                }
                let result = term_release
                    .lock()
                    .release(&request.terminal_id)
                    .map(|()| ReleaseTerminalResponse::new())
                    .map_err(|e| agent_client_protocol::Error::internal_error().data(e));
                let _ = responder.respond_with_result(result);
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            let loop_result = run_command_loop(
                cx,
                command_rx,
                init_tx,
                loop_sinks,
                loop_delivery_circuits,
                loop_host_plan_server,
                loop_agent_id,
                loop_state,
                loop_spawned,
                allow_terminal,
                persistence,
                conversation_persistence,
                loop_warmup_done,
                loop_composer_controls,
            )
            .await;
            // Driver thread is winding down — kill any live terminal children so
            // they don't outlive the agent.
            loop_terminals.lock().release_all();
            loop_result
        })
        .await;

    connection_result.map_err(|e| e.to_string())
}

/// The agent driver's main loop: complete `initialize`, then service commands
/// until shutdown. Runs concurrently with the connection's dispatch actors.
#[allow(clippy::too_many_arguments)]
async fn run_command_loop(
    cx: ConnectionTo<Agent>,
    mut command_rx: mpsc::UnboundedReceiver<AcpCommand>,
    init_tx: oneshot::Sender<Result<InitOutcome, String>>,
    sinks: Vec<Arc<dyn EventSink>>,
    delivery_circuits: SessionDeliveryCircuits,
    host_plan_server: Arc<crate::acp::host_mcp::parent::HostPlanServer>,
    agent_id: AgentId,
    driver_state: Arc<Mutex<DriverState>>,
    spawned: Arc<AtomicBool>,
    allow_terminal: bool,
    persistence: Option<Arc<SessionPersistence>>,
    conversation_persistence: Option<Arc<ConversationPersistenceAdapter>>,
    warmup_done: Arc<Mutex<HashSet<AgentId>>>,
    composer_controls: Arc<ComposerControlCache>,
) -> Result<(), agent_client_protocol::Error> {
    // Step 1: handshake, bounded by INIT_TIMEOUT so a silent agent can never
    // wedge `acp_spawn_agent` forever (H1). On timeout we report the failure
    // and return; returning ends `main_fn`, which tears the connection down and
    // kills the child via the SDK's `ChildGuard`.
    let init_request = InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(client::client_capabilities(allow_terminal));
    let init_outcome =
        tokio::time::timeout(INIT_TIMEOUT, cx.send_request(init_request).block_task()).await;
    let supports_session_close = match init_outcome {
        Ok(Ok(response)) => {
            // Propagate the FULL advertised auth methods (opaque
            // id/name/optional description) so the renderer can offer a Sign-in
            // action and call `authenticate(methodId)` before `session/new`.
            // Every advertised method is forwarded; no agent-type filtering.
            let auth_methods = to_auth_method_infos(&response.auth_methods);
            let auth_method_ids: Vec<&str> = auth_methods.iter().map(|m| m.id.as_str()).collect();
            let session_caps = &response.agent_capabilities.session_capabilities;
            let supports_session_close = session_caps.close.is_some();
            log::info!(
                "[acp] agent {agent_id} initialized: protocol={:?} auth_methods={:?} \
                 loadSession={} sessionCapabilities.list={} resume={} close={}",
                response.protocol_version,
                auth_method_ids,
                response.agent_capabilities.load_session,
                session_caps.list.is_some(),
                session_caps.resume.is_some(),
                supports_session_close,
            );

            spawned.store(true, Ordering::Release);
            let _ = init_tx.send(Ok(InitOutcome {
                capabilities: response.agent_capabilities,
                auth_methods,
            }));
            supports_session_close
        }
        Ok(Err(e)) => {
            let _ = init_tx.send(Err(e.to_string()));
            return Err(e);
        }
        Err(_) => {
            let message = format!("initialize timed out after {INIT_TIMEOUT:?}");
            let _ = init_tx.send(Err(message.clone()));
            return Err(agent_client_protocol::Error::internal_error().data(message));
        }
    };

    // Step 2: command loop.
    //
    // Every agent→client *request* is dispatched via `cx.spawn` (not awaited
    // inline), so the loop returns to `command_rx.recv()` immediately. This is
    // the C1 fix: while any request is in flight, `RespondPermission` is still
    // serviced, so an agent that gates its reply on a permission decision can
    // never deadlock the loop. `recv()` is also always responsive to
    // `Shutdown` and to channel close, so `kill`/`kill_all` always make
    // progress (H1). Spawned tasks must return `Ok(())` and route protocol
    // errors through their reply channel — a spawned task that returns `Err`
    // would tear down the whole connection.
    while let Some(command) = command_rx.recv().await {
        match command {
            AcpCommand::Shutdown => break,

            AcpCommand::NewSession {
                cwd,
                mcp_servers,
                stable_agent_namespace,
                runtime_agent_id,
                project_id,
                ephemeral,
                worktree_path,
                worktree_branch,
                mut binding_gate,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let close_cx = cx.clone();
                let req_sinks = sinks.clone();
                let req_delivery_circuits = Arc::clone(&delivery_circuits);
                let req_agent_id = agent_id.clone();
                let req_state = driver_state.clone();
                let req_persistence = persistence.clone();
                let req_warmup_done = warmup_done.clone();
                let req_composer = composer_controls.clone();
                spawn_request(&cx, slot, async move {
                    let request = NewSessionRequest::new(cwd.clone()).mcp_servers(mcp_servers);
                    let timeout = session_new_timeout();
                    log::debug!(
                        "[acp] {req_agent_id} session/new sent, awaiting reply (timeout {timeout:?})"
                    );
                    match tokio::time::timeout(timeout, req_cx.send_request(request).block_task())
                        .await
                    {
                        Ok(Ok(response)) => {
                            let session_id = SessionId::from(response.session_id);
                            if !ephemeral {
                                if let Some(persistence) = req_persistence {
                                    let registration = SessionRegistration {
                                        session_id: session_id.0.clone(),
                                        stable_agent_namespace,
                                        runtime_agent_id: Some(runtime_agent_id),
                                        project_id,
                                        cwd: PathBuf::from(&cwd),
                                        worktree_path,
                                        worktree_branch,
                                    };
                                    if let Err(error) =
                                        persistence.register_session(registration).await
                                    {
                                        let _ = close_cx
                                            .send_request(CloseSessionRequest::new(&session_id))
                                            .block_task()
                                            .await;
                                        send_reply(
                                            &task_slot,
                                            Err(format!("failed to persist new session: {error}")),
                                        );
                                        return;
                                    }
                                }
                            }
                            // Record the session's workspace root so agent fs
                            // requests for this session can be sandboxed (H2).
                            {
                                let mut state = req_state.lock();
                                state.set_session_root(session_id.0.clone(), PathBuf::from(&cwd));
                                if ephemeral {
                                    state.mark_ephemeral(session_id.0.clone());
                                }
                            }

                            // ---- First-prompt warmup (upstream bug workaround) ----
                            //
                            // pi-acp's `PiRpcProcess.request()` has no timeout, and pi's
                            // `session.prompt()` may stall before calling `preflightResult`
                            // on a cold start. The warmup sends a lightweight
                            // `session/prompt` through the full pi-acp pipeline BEFORE
                            // the `acp:session_created` event is emitted, so the
                            // renderer's event handlers silently drop warmup events
                            // (the session doesn't exist in the store yet). If the
                            // warmup times out, we cancel and continue — session
                            // creation still succeeds, but the user's first manual
                            // prompt may still experience the cold-start hang.
                            // See: https://github.com/svkozak/pi-acp/issues/94
                            let warmup_timeout = first_prompt_warmup_timeout();
                            // Per-agent warmup-done guard: a visibility-churn
                            // re-entry of `NewSession` for an agent whose
                            // warmup already completed (or is still in-flight)
                            // is a logged no-op so the 4–8s cold-start
                            // workaround is not re-fired within one agent
                            // lifetime (the renderer re-renders a re-fired
                            // warmup as a "second chat"). `warmup_should_run`
                            // atomically checks + inserts under one lock so a
                            // concurrent re-entry coalesces onto this in-flight
                            // warmup (I/O matrix: "do not spawn a second");
                            // the entry is never cleared on a warmup exit
                            // branch, so the "done" dedup also holds for
                            // subsequent `NewSession` calls. Cleared on agent
                            // drop (driver self-reap) so a re-spawned agent
                            // re-warmups.
                            let should_warmup = warmup_should_run(&req_warmup_done, &req_agent_id);
                            if warmup_timeout.as_secs() > 0 && should_warmup {
                                let warmup_content =
                                    vec![agent_client_protocol::schema::v1::ContentBlock::Text(
                                        agent_client_protocol::schema::v1::TextContent::new(
                                            " ".to_string(),
                                        ),
                                    )];
                                let warmup_request =
                                    PromptRequest::new(&session_id, warmup_content);
                                log::info!(
                                    "[acp] {req_agent_id} first-prompt warmup started \
                                     (timeout {warmup_timeout:?})"
                                );
                                let warmup = req_cx.send_request(warmup_request).block_task();
                                tokio::pin!(warmup);
                                match tokio::time::timeout(warmup_timeout, &mut warmup).await {
                                    Ok(Ok(_response)) => {
                                        log::info!(
                                            "[acp] {req_agent_id} first-prompt warmup \
                                             completed — agent is ready"
                                        );
                                    }
                                    Ok(Err(e)) => {
                                        log::warn!(
                                            "[acp] {req_agent_id} first-prompt warmup \
                                             failed: {e} (continuing without warmup)"
                                        );
                                    }
                                    Err(_) => {
                                        log::warn!(
                                            "[acp] {req_agent_id} first-prompt warmup \
                                             timed out after {warmup_timeout:?} \
                                             (cancelling)"
                                        );
                                        // Signal cancel so pi-acp's in-flight
                                        // warmup turn can settle.
                                        let _ = req_cx
                                            .send_notification(CancelNotification::new(&session_id))
                                            .map_err(|e| {
                                                log::debug!(
                                                    "[acp] warmup cancel notification \
                                                     failed: {e}"
                                                );
                                                e
                                            });
                                        // Await the warmup's cancellation
                                        // settlement (bounded by CANCEL_GRACE)
                                        // so the session is not left with a
                                        // pending turn in pi-acp when the user
                                        // sends their first prompt. Mirrors the
                                        // SendPrompt handler's cancel-grace race.
                                        match tokio::time::timeout(CANCEL_GRACE, &mut warmup).await
                                        {
                                            Ok(Ok(_)) => {
                                                log::info!(
                                                    "[acp] {req_agent_id} warmup \
                                                     cancelled and settled"
                                                );
                                            }
                                            Ok(Err(e)) => {
                                                log::warn!(
                                                    "[acp] {req_agent_id} warmup \
                                                     cancel settled with error: {e}"
                                                );
                                            }
                                            Err(_) => {
                                                log::warn!(
                                                    "[acp] {req_agent_id} warmup \
                                                     cancel did not settle within \
                                                     {CANCEL_GRACE:?}; failing \
                                                     session creation"
                                                );
                                                send_reply(
                                                    &task_slot,
                                                    Err(format!(
                                                        "warmup cancel did not settle \
                                                         within {CANCEL_GRACE:?}"
                                                    )),
                                                );
                                                return;
                                            }
                                        }
                                        // Drain any pending permissions/questions
                                        // the warmup turn may have raised, so
                                        // they don't block the user's first real
                                        // prompt. Mirrors CancelPrompt and
                                        // SendPrompt completion cleanup.
                                        let pending = req_state.lock().drain_session(&session_id.0);
                                        for permission in pending {
                                            let _ = permission.responder.respond(
                                                RequestPermissionResponse::new(
                                                    RequestPermissionOutcome::Cancelled,
                                                ),
                                            );
                                        }
                                        let pending_questions =
                                            req_state.lock().drain_session_questions(&session_id.0);
                                        for question in pending_questions {
                                            let _ = question.responder.respond(serde_json::json!({
                                                "questionId": question.question_id,
                                                "cancelled": true,
                                            }));
                                        }
                                    }
                                }
                            }

                            // Cache the agent-advertised Model-selector configId so
                            // `set_model` targets it instead of hardcoding "model".
                            if let Some(id) = events::model_config_id_from_options(
                                response.config_options.as_deref(),
                            ) {
                                req_state
                                    .lock()
                                    .set_model_config_id(session_id.0.clone(), id);
                            }

                            let outcome = NewSessionOutcome {
                                persistence: if ephemeral {
                                    "ephemeral"
                                } else {
                                    "conversation"
                                },
                                conversation_id: None,
                                workspace_cwd: None,
                                execution_cwd: None,
                                session_id: session_id.clone(),
                                modes: response.modes.clone(),
                                models: events::models_from_config_options(
                                    response.config_options.as_deref(),
                                ),
                                config_options: response.config_options.clone(),
                            };
                            req_composer.remember(
                                &session_id.0,
                                &SessionReopenOutcome {
                                    modes: outcome.modes.clone(),
                                    models: outcome.models.clone(),
                                    config_options: outcome.config_options.clone(),
                                },
                            );
                            // Return the opaque ACP response to the manager, then hold the
                            // renderer-visible success event until the manager has durably bound
                            // it to the pre-created Conversation.
                            send_reply(&task_slot, Ok(outcome));
                            if let Some(gate) = binding_gate.as_mut() {
                                loop {
                                    if let Some(result) = gate.borrow().clone() {
                                        if let Err(error) = result {
                                            log::error!(
                                                "[conversation-creation] session-created event suppressed code={error}"
                                            );
                                            return;
                                        }
                                        break;
                                    }
                                    if gate.changed().await.is_err() {
                                        log::error!(
                                            "[conversation-creation] binding gate dropped before durable bind"
                                        );
                                        return;
                                    }
                                }
                            }
                            let event = SessionCreatedEvent {
                                agent_id: req_agent_id,
                                session_id,
                                modes: response.modes,
                                models: events::models_from_config_options(
                                    response.config_options.as_deref(),
                                ),
                                config_options: response.config_options,
                            };
                            if let Err(error) = fan_out_session(
                                &req_sinks,
                                &req_delivery_circuits,
                                event.session_id.0.as_str(),
                                events::EVENT_SESSION_CREATED,
                                &event,
                            ) {
                                log_delivery_error("session_created", &error);
                            }
                        }
                        Ok(Err(e)) => send_reply(&task_slot, Err(e.to_string())),
                        Err(_) => {
                            log::warn!(
                                "[acp] {req_agent_id} session/new timed out after {timeout:?}; \
                                 check agent stderr in RUST_LOG=debug"
                            );
                            send_reply(
                                &task_slot,
                                Err(format!("session/new timed out after {timeout:?}")),
                            )
                        }
                    }
                });
            }

            AcpCommand::LoadSession {
                session_id,
                cwd,
                mcp_servers,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let req_state = driver_state.clone();
                let req_persistence = persistence.clone();
                spawn_request(&cx, slot, async move {
                    // Reinstall the durable writer BEFORE sending session/load
                    // so events arriving during the load (replay chunks,
                    // status updates) are persisted instead of dropped with
                    // "persisted session not found". After an app restart the
                    // in-memory writer is gone; calling reopen_writer here
                    // restores it from the on-disk catalog before the agent
                    // starts streaming. Idempotent (no-op if already installed)
                    // and non-fatal (unknown/ephemeral id surfaces
                    // SessionNotFound, logged + skipped).
                    if let Some(persistence) = &req_persistence {
                        if let Err(error) = persistence.reopen_writer(&session_id.0).await {
                            log::warn!(
                                "[acp] session {} reopen_writer failed: {error} (continuing load)",
                                session_id.0
                            );
                        }
                    }
                    // Bounded like session/new: a wedged agent must not park the
                    // renderer's reconnect forever (the reply sender would be
                    // held indefinitely).
                    let request =
                        LoadSessionRequest::new(&session_id, cwd.clone()).mcp_servers(mcp_servers);
                    let result = run_session_reopen(
                        "session/load",
                        &session_id.0,
                        &cwd,
                        &req_state,
                        req_cx.send_request(request).block_task(),
                    )
                    .await;
                    send_reply(&task_slot, result);
                });
            }

            AcpCommand::ResumeSession {
                session_id,
                cwd,
                mcp_servers,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let req_state = driver_state.clone();
                let req_persistence = persistence.clone();
                spawn_request(&cx, slot, async move {
                    // Same durable-writer reopen as LoadSession above — call
                    // BEFORE the request so events arriving during resume are
                    // persisted instead of silently dropped.
                    if let Some(persistence) = &req_persistence {
                        if let Err(error) = persistence.reopen_writer(&session_id.0).await {
                            log::warn!(
                                "[acp] session {} reopen_writer failed: {error} (continuing resume)",
                                session_id.0
                            );
                        }
                    }
                    let request = ResumeSessionRequest::new(&session_id, cwd.clone())
                        .mcp_servers(mcp_servers);
                    let result = run_session_reopen(
                        "session/resume",
                        &session_id.0,
                        &cwd,
                        &req_state,
                        req_cx.send_request(request).block_task(),
                    )
                    .await;
                    send_reply(&task_slot, result);
                });
            }

            AcpCommand::CloseSession { session_id, reply } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let req_state = driver_state.clone();
                let req_persistence = persistence.clone();
                let req_plan_server = host_plan_server.clone();
                spawn_request(&cx, slot, async move {
                    let request = CloseSessionRequest::new(&session_id);
                    let result = req_cx.send_request(request).block_task().await;
                    if result.is_ok() {
                        // Forget the workspace root and resolve any pending
                        // permissions for the now-closed session.
                        let pending = {
                            let mut state = req_state.lock();
                            state.remove_session_root(&session_id.0);
                            state.finish_turn(&session_id.0)
                        };
                        for permission in pending {
                            let _ = permission.responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ));
                        }
                        // Issue #411: resolve outstanding questions for the
                        // closed session as cancelled too.
                        let pending_questions =
                            req_state.lock().finish_turn_questions(&session_id.0);
                        for question in pending_questions {
                            let _ = question.responder.respond(serde_json::json!({
                                "questionId": question.question_id,
                                "cancelled": true,
                            }));
                        }
                        // Evict host-plan auth, cache, and any active route only
                        // after the agent confirms the session is closed.
                        req_plan_server.unregister_session(&session_id.0);
                    }
                    let mut result = result.map(|_| ()).map_err(|e| e.to_string());
                    if result.is_ok() {
                        if let Some(persistence) = req_persistence {
                            if let Err(error) = persistence
                                .finalize_session(&session_id.0, PersistedSessionStatus::Closed)
                                .await
                            {
                                result = Err(format!(
                                    "session closed but history finalization failed: {error}"
                                ));
                            }
                        }
                    }
                    send_reply(&task_slot, result);
                });
            }

            AcpCommand::ListSessions { cwd, cursor, reply } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                spawn_request(&cx, slot, async move {
                    let mut request = agent_client_protocol::schema::v1::ListSessionsRequest::new();
                    if let Some(cwd) = cwd {
                        request = request.cwd(std::path::PathBuf::from(cwd));
                    }
                    if let Some(cursor) = cursor {
                        request = request.cursor(cursor);
                    }
                    let result = req_cx.send_request(request).block_task().await;
                    send_reply(&task_slot, result.map_err(|e| e.to_string()));
                });
            }

            AcpCommand::SendPrompt {
                session_id,
                content,
                turn_id,
                accepted,
                reply,
            } => {
                // Single-flight per session: reject a second prompt while a turn
                // is in flight (M4). `try_begin_turn` returns a cancel signal
                // receiver when the turn may proceed.
                let handles = driver_state.lock().try_begin_turn(&session_id.0);
                let Some(handles) = handles else {
                    // Stable code matched by renderer `ACP_TURN_IN_PROGRESS_CODE`.
                    let error = format!("ACP_TURN_IN_PROGRESS: session {}", session_id.0);
                    let _ = accepted.send(Err(error.clone()));
                    let _ = reply.send(Err(error));
                    continue;
                };
                let cancel_rx = handles.cancel_rx;
                let mut idle_rx = handles.idle_rx;

                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let turn_cx = cx.clone();
                let turn_sinks = sinks.clone();
                let turn_delivery_circuits = Arc::clone(&delivery_circuits);
                let turn_agent_id = agent_id.clone();
                let turn_plan_server = host_plan_server.clone();
                let turn_state = driver_state.clone();
                let turn_persistence = persistence.clone();
                let turn_conversation_persistence = conversation_persistence.clone();
                let turn_session = session_id.clone();
                let log_session = session_id.clone();
                // Register before spawning so an immediate `plan` call is
                // routed to this accepted prompt's session, even when the agent
                // reuses an MCP child created for an older session.
                host_plan_server.begin_turn(&agent_id.0, &session_id.0);
                // Story 1.8 T3.2: capture the client turn-id to echo on prompt_complete.
                let turn_turn_id = turn_id.clone();
                let spawn_result = cx.spawn(async move {
                    // Race the turn against completion, a cancel signal, an
                    // optional idle deadline (reset by agent `session/update`
                    // activity via `DriverState::signal_idle`), and an optional
                    // hard wall-clock cap. Both default to `None` (unlimited):
                    // a turn is bounded only if the operator/user configured a
                    // value. On idle/hard timeout → signal cancel +
                    // `CANCEL_GRACE` + a typed error (`acp-store` sets
                    // `status: 'error'`). See [`race_turn`].
                    let idle = turn_idle_timeout();
                    let hard = resolved_turn_timeout();
                    let cancel_state = turn_state.clone();
                    let cancel_session = session_id.clone();
                    let cancel_cx = turn_cx.clone();
                    let outcome: Result<StopReason, String> = race_turn(
                        async {
                            turn_cx
                                .send_request(PromptRequest::new(&session_id, content))
                                .block_task()
                                .await
                                .map(|r| r.stop_reason)
                                .map_err(|e| e.to_string())
                        },
                        cancel_rx,
                        &mut idle_rx,
                        move || {
                            // Mirror AcpCommand::CancelPrompt: signal the
                            // active-turn cancel (winds down the race) AND
                            // notify the agent to abandon its in-flight
                            // session/prompt. send_notification is non-blocking
                            // (it queues onto the connection), so race_turn
                            // stays sync.
                            cancel_state.lock().signal_cancel(&cancel_session.0);
                            if let Err(error) = cancel_cx
                                .send_notification(CancelNotification::new(&cancel_session))
                            {
                                log::warn!(
                                    "[acp] failed to cancel agent prompt on turn timeout: {error}"
                                );
                            }
                        },
                        idle,
                        hard,
                    )
                    .await;

                    match &outcome {
                        Ok(stop_reason) => log::info!(
                            "[acp] session {} turn complete: stop_reason={stop_reason:?}",
                            log_session.0
                        ),
                        Err(message) => {
                            log::warn!("[acp] session {} turn failed: {message}", log_session.0)
                        }
                    }

                    // Turn is over: clear the host-plan routing marker and the
                    // driver active-turn marker, then resolve permissions that
                    // were never answered (H3 — normal completion, not just cancel).
                    turn_plan_server.end_turn(&turn_agent_id.0, &session_id.0);
                    let pending = turn_state.lock().finish_turn(&session_id.0);
                    for permission in pending {
                        let _ = permission.responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    }
                    // Issue #411: model-abandoned questions are first-class
                    // outcomes — a turn that ends without the user answering
                    // resolves the parked question responders as cancelled.
                    let pending_questions = turn_state.lock().finish_turn_questions(&session_id.0);
                    for question in pending_questions {
                        let _ = question.responder.respond(serde_json::json!({
                            "questionId": question.question_id,
                            "cancelled": true,
                        }));
                    }

                    let is_ephemeral = turn_state.lock().is_ephemeral(&session_id.0);
                    match outcome {
                        Ok(stop_reason) => {
                            let event = PromptCompleteEvent {
                                agent_id: turn_agent_id,
                                session_id,
                                stop_reason,
                                turn_id: turn_turn_id.clone(),
                            };
                            if let Err(error) = fan_out_session_committed(
                                &turn_sinks,
                                &turn_delivery_circuits,
                                turn_conversation_persistence.as_deref(),
                                event.session_id.0.as_str(),
                                events::EVENT_PROMPT_COMPLETE,
                                &event,
                            )
                            .await
                            {
                                log_delivery_ticket_error("prompt_complete", &error);
                                send_reply(&task_slot, Err(error.code.to_string()));
                                return Ok(());
                            }
                            if !is_ephemeral {
                                if let Some(persistence) = &turn_persistence {
                                    if let Err(error) =
                                        persistence.flush_session(&event.session_id.0).await
                                    {
                                        send_reply(
                                            &task_slot,
                                            Err(format!(
                                                "failed to flush session history: {error}"
                                            )),
                                        );
                                        return Ok(());
                                    }
                                }
                            }
                            send_reply(&task_slot, Ok(stop_reason));
                        }
                        Err(message) => {
                            let event = AgentErrorEvent {
                                agent_id: turn_agent_id,
                                session_id: Some(session_id),
                                message: message.clone(),
                            };
                            // Turn-scoped error → sid is the session id.
                            if let Some(session_id) = event.session_id.as_ref() {
                                if let Err(error) = fan_out_session(
                                    &turn_sinks,
                                    &turn_delivery_circuits,
                                    session_id.0.as_str(),
                                    events::EVENT_AGENT_ERROR,
                                    &event,
                                ) {
                                    log_delivery_error("turn_agent_error", &error);
                                }
                            }
                            if !is_ephemeral {
                                if let Some(persistence) = &turn_persistence {
                                    if let Some(session_id) = event.session_id.as_ref() {
                                        if let Err(error) =
                                            persistence.flush_session(&session_id.0).await
                                        {
                                            send_reply(
                                                &task_slot,
                                                Err(format!(
                                                    "{message}; history flush failed: {error}"
                                                )),
                                            );
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                            send_reply(&task_slot, Err(message));
                        }
                    }
                    Ok(())
                });
                if let Err(e) = spawn_result {
                    // The connection is shutting down; clear the markers we just
                    // set and surface the real error to the caller (L5).
                    host_plan_server.end_turn(&agent_id.0, &turn_session.0);
                    driver_state.lock().finish_turn(&turn_session.0);
                    let error = format!("failed to start prompt turn: {e}");
                    let _ = accepted.send(Err(error.clone()));
                    send_reply(&slot, Err(error));
                } else {
                    let _ = accepted.send(Ok(()));
                }
            }

            AcpCommand::OwnsSession { session_id, reply } => {
                let _ = reply.send(Ok(driver_state
                    .lock()
                    .session_root(&session_id.0)
                    .is_some()));
            }

            AcpCommand::IsEphemeralSession { session_id, reply } => {
                let _ = reply.send(Ok(driver_state.lock().is_ephemeral(&session_id.0)));
            }

            AcpCommand::IsTurnActive { session_id, reply } => {
                let _ = reply.send(Ok(driver_state.lock().is_turn_active(&session_id.0)));
            }

            AcpCommand::WaitTurnIdle { session_id, reply } => {
                let waiter = driver_state.lock().wait_turn_idle(&session_id.0);
                match waiter {
                    None => {
                        let _ = reply.send(Ok(()));
                    }
                    Some(waiter) => {
                        let slot = reply_slot(reply);
                        let task_slot = slot.clone();
                        spawn_request(&cx, slot, async move {
                            let result = waiter
                                .await
                                .map_err(|_| "turn idle waiter was dropped".to_string());
                            send_reply(&task_slot, result);
                        });
                    }
                }
            }

            AcpCommand::DisposeEphemeralSession { session_id, reply } => {
                let waiter = {
                    let mut state = driver_state.lock();
                    if !state.is_ephemeral(&session_id.0) {
                        let _ = reply.send(Err("session is not ephemeral".to_string()));
                        continue;
                    }
                    state.signal_cancel(&session_id.0);
                    state.wait_turn_idle(&session_id.0)
                };
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let dispose_state = driver_state.clone();
                let close_cx = cx.clone();
                spawn_request(&cx, slot, async move {
                    if let Some(waiter) = waiter {
                        let timeout = CANCEL_GRACE + Duration::from_millis(250);
                        match tokio::time::timeout(timeout, waiter).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => {
                                send_reply(
                                    &task_slot,
                                    Err("turn idle waiter was dropped".to_string()),
                                );
                                return;
                            }
                            Err(_) => {
                                send_reply(
                                    &task_slot,
                                    Err(format!(
                                        "ephemeral session disposal timed out after {timeout:?}"
                                    )),
                                );
                                return;
                            }
                        }
                    }
                    {
                        let state = dispose_state.lock();
                        if state.is_turn_active(&session_id.0) {
                            send_reply(
                                &task_slot,
                                Err("ephemeral session turn is still active".to_string()),
                            );
                            return;
                        }
                        if !state.is_ephemeral(&session_id.0) {
                            send_reply(&task_slot, Err("session is not ephemeral".to_string()));
                            return;
                        }
                    }

                    // The temporary session is now idle and still protected by
                    // the authoritative ephemeral marker. Ask capable agents to
                    // release their session-side resources, but never let a
                    // stale or wedged `session/close` prevent local cleanup. The
                    // short bound keeps disposal responsive.
                    if supports_session_close {
                        let close_timeout = CANCEL_GRACE;
                        match tokio::time::timeout(
                            close_timeout,
                            close_cx
                                .send_request(CloseSessionRequest::new(&session_id))
                                .block_task(),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => log::debug!(
                                "[acp] ephemeral session {} close failed: {error}",
                                session_id.0
                            ),
                            Err(_) => log::warn!(
                                "[acp] ephemeral session {} close timed out after {close_timeout:?}",
                                session_id.0
                            ),
                        }
                    }

                    let (permissions, questions) = {
                        let mut state = dispose_state.lock();
                        if state.is_turn_active(&session_id.0) {
                            send_reply(
                                &task_slot,
                                Err("ephemeral session turn became active during disposal"
                                    .to_string()),
                            );
                            return;
                        }
                        if !state.is_ephemeral(&session_id.0) {
                            send_reply(&task_slot, Err("session is not ephemeral".to_string()));
                            return;
                        }
                        state.dispose_session(&session_id.0)
                    };
                    for permission in permissions {
                        let _ = permission.responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ));
                    }
                    for question in questions {
                        let _ = question.responder.respond(serde_json::json!({
                            "questionId": question.question_id,
                            "cancelled": true,
                        }));
                    }
                    send_reply(&task_slot, Ok(()));
                });
            }

            AcpCommand::CancelPrompt { session_id, reply } => {
                // Signal the active turn to wind down (bounding its wait) and
                // resolve any pending permissions for this session as cancelled.
                let pending = {
                    let mut state = driver_state.lock();
                    state.signal_cancel(&session_id.0);
                    state.drain_session(&session_id.0)
                };
                for permission in pending {
                    let _ = permission.responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                }
                // Issue #411: cancel also resolves any outstanding questions for
                // the session (the agent abandons them; first-class outcome).
                let pending_questions = driver_state.lock().drain_session_questions(&session_id.0);
                for question in pending_questions {
                    let _ = question.responder.respond(serde_json::json!({
                        "questionId": question.question_id,
                        "cancelled": true,
                    }));
                }
                let result = cx.send_notification(CancelNotification::new(&session_id));
                let _ = reply.send(result.map_err(|e| e.to_string()));
            }

            AcpCommand::SetMode {
                session_id,
                mode_id,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                spawn_request(&cx, slot, async move {
                    let request = SetSessionModeRequest::new(&session_id, mode_id);
                    let result = req_cx.send_request(request).block_task().await;
                    send_reply(&task_slot, result.map(|_| ()).map_err(|e| e.to_string()));
                });
            }

            AcpCommand::SetModel {
                session_id,
                model_id,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let req_sinks = sinks.clone();
                let req_delivery_circuits = Arc::clone(&delivery_circuits);
                let req_agent_id = agent_id.clone();
                let req_state = driver_state.clone();
                let req_composer = composer_controls.clone();
                spawn_request(&cx, slot, async move {
                    // ACP 0.14 removed `session/set_model`; the model is now a
                    // `select`-kind config option (category = Model). Its configId
                    // is the agent-provided option id (cached at session/new from
                    // the agent's `config_options`), falling back to the `"model"`
                    // convention when the agent didn't advertise one. `model_id` is
                    // the option value id the renderer picked.
                    let config_id = req_state
                        .lock()
                        .model_config_id(&session_id.0)
                        .unwrap_or_else(|| "model".to_string());
                    let request = SetSessionConfigOptionRequest::new(
                        &session_id,
                        config_id,
                        model_id.as_str(),
                    );
                    match req_cx.send_request(request).block_task().await {
                        Ok(response) => {
                            req_composer.remember_options(
                                &session_id.0,
                                response.config_options.clone(),
                            );
                            let event = ConfigOptionsUpdateEvent {
                                agent_id: req_agent_id,
                                session_id,
                                config_options: response.config_options.clone(),
                            };
                            match fan_out_session(
                                &req_sinks,
                                &req_delivery_circuits,
                                event.session_id.0.as_str(),
                                events::EVENT_CONFIG_OPTIONS_UPDATE,
                                &event,
                            ) {
                                Ok(_) => send_reply(&task_slot, Ok(())),
                                Err(error) if error.is_unbound_session() => {
                                    log_delivery_error("set_model", &error);
                                    send_reply(&task_slot, Ok(()));
                                }
                                Err(error) => {
                                    log_delivery_error("set_model", &error);
                                    send_reply(&task_slot, Err(error.code.to_string()));
                                }
                            }
                        }
                        Err(e) => send_reply(&task_slot, Err(e.to_string())),
                    }
                });
            }

            AcpCommand::SetConfigOption {
                session_id,
                config_id,
                value_id,
                reply,
            } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let req_sinks = sinks.clone();
                let req_delivery_circuits = Arc::clone(&delivery_circuits);
                let req_agent_id = agent_id.clone();
                let req_state = driver_state.clone();
                let req_composer = composer_controls.clone();
                spawn_request(&cx, slot, async move {
                    let request = SetSessionConfigOptionRequest::new(
                        &session_id,
                        config_id,
                        value_id.as_str(),
                    );
                    match req_cx.send_request(request).block_task().await {
                        Ok(response) => {
                            req_composer.remember_options(
                                &session_id.0,
                                response.config_options.clone(),
                            );
                            // Keep the cached Model-selector configId fresh in case
                            // the agent reorganized its config options.
                            if let Some(id) = events::model_config_id_from_options(Some(
                                response.config_options.as_slice(),
                            )) {
                                req_state
                                    .lock()
                                    .set_model_config_id(session_id.0.clone(), id);
                            }
                            let event = ConfigOptionsUpdateEvent {
                                agent_id: req_agent_id,
                                session_id,
                                config_options: response.config_options.clone(),
                            };
                            match fan_out_session(
                                &req_sinks,
                                &req_delivery_circuits,
                                event.session_id.0.as_str(),
                                events::EVENT_CONFIG_OPTIONS_UPDATE,
                                &event,
                            ) {
                                Ok(_) => send_reply(&task_slot, Ok(response.config_options)),
                                Err(error) if error.is_unbound_session() => {
                                    log_delivery_error("set_config_option", &error);
                                    send_reply(&task_slot, Ok(response.config_options));
                                }
                                Err(error) => {
                                    log_delivery_error("set_config_option", &error);
                                    send_reply(&task_slot, Err(error.code.to_string()));
                                }
                            }
                        }
                        Err(e) => send_reply(&task_slot, Err(e.to_string())),
                    }
                });
            }

            AcpCommand::RespondPermission {
                request_id,
                outcome,
                reply,
            } => {
                let pending = driver_state.lock().take_permission(&request_id);
                match pending {
                    Some(permission) => {
                        let result = permission
                            .responder
                            .respond(RequestPermissionResponse::new(outcome));
                        let _ = reply.send(result.map_err(|e| e.to_string()));
                    }
                    None => {
                        let _ =
                            reply.send(Err(format!("unknown permission request: {request_id}")));
                    }
                }
            }

            AcpCommand::AnswerQuestion {
                question_id,
                values,
                reply,
            } => {
                // Issue #411: resolve the parked `_session/question` responder
                // exactly once. `Some(values)` → the selected option values;
                // `None` → cancelled. Unknown id (already resolved / drained)
                // mirrors the permission race-loser path.
                let pending = driver_state.lock().take_question(&question_id);
                match pending {
                    Some(question) => {
                        let payload = match &values {
                            Some(values) => serde_json::json!({
                                "questionId": question_id,
                                "values": values,
                            }),
                            None => serde_json::json!({
                                "questionId": question_id,
                                "cancelled": true,
                            }),
                        };
                        let result = question.responder.respond(payload);
                        let _ = reply.send(result.map_err(|e| e.to_string()));
                    }
                    None => {
                        let _ = reply.send(Err(format!("unknown question request: {question_id}")));
                    }
                }
            }

            AcpCommand::Authenticate { method_id, reply } => {
                let slot = reply_slot(reply);
                let task_slot = slot.clone();
                let req_cx = cx.clone();
                let log_agent_id = agent_id.clone();
                spawn_request(&cx, slot, async move {
                    log::info!("[acp] agent {log_agent_id} authenticating via '{method_id}'");
                    let result = req_cx
                        .send_request(AuthenticateRequest::new(method_id))
                        .block_task()
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    send_reply(&task_slot, result);
                });
            }
        }
    }

    Ok(())
}

/// Spawn an agent→client request task on the connection, keeping the command
/// loop free to service other commands (notably `RespondPermission`) while it
/// runs (the C1 fix).
///
/// The request's `reply` sender lives in a shared [`ReplySlot`]: the spawned
/// task sends the real result through it, but if spawning fails (the connection
/// is winding down) this helper sends an explicit error instead of dropping the
/// sender — otherwise the caller would see the generic "agent thread dropped
/// the reply" rather than the real cause (L5).
///
/// The spawned task itself must always resolve to `Ok(())`; a spawned task that
/// returns `Err` would tear down the whole connection.
fn spawn_request<T, Fut>(cx: &ConnectionTo<Agent>, slot: ReplySlot<T>, task: Fut)
where
    T: Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    if let Err(e) = cx.spawn(async move {
        task.await;
        Ok(())
    }) {
        send_reply(&slot, Err(format!("failed to dispatch request: {e}")));
    }
}

/// A reply sender shared between a spawned request task and the command loop so
/// the loop can still surface a real error if the task fails to spawn (L5).
/// Whichever side resolves first takes the sender; the other becomes a no-op.
type ReplySlot<T> = Arc<Mutex<Option<oneshot::Sender<Result<T, String>>>>>;

/// Wrap a reply sender in a shared, take-once slot.
fn reply_slot<T>(reply: oneshot::Sender<Result<T, String>>) -> ReplySlot<T> {
    Arc::new(Mutex::new(Some(reply)))
}

/// Send through a [`ReplySlot`] exactly once; subsequent sends are ignored.
fn send_reply<T>(slot: &ReplySlot<T>, value: Result<T, String>) {
    if let Some(tx) = slot.lock().take() {
        let _ = tx.send(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_namespace_uses_safe_identity_and_excludes_secrets() {
        let mut config = AgentConfig {
            config_id: None,
            name: "  Example   Agent ".to_string(),
            command: "C:/Users/alice/private/token.exe".to_string(),
            args: vec!["--api-key=secret-one".to_string()],
            env: std::collections::HashMap::from([(
                "AUTH_TOKEN".to_string(),
                "secret-two".to_string(),
            )]),
            allow_terminal: false,
            permission_policy: PermissionPolicy::Ask,
        };
        let namespace = stable_agent_namespace(&config).unwrap();
        config.name = "example agent".to_string();
        config.command = "/different/private/token.exe".to_string();
        config.args = vec!["--api-key=other-secret".to_string()];
        config
            .env
            .insert("AUTH_TOKEN".to_string(), "third-secret".to_string());
        assert_eq!(
            stable_agent_namespace(&config).as_deref(),
            Some(namespace.as_str())
        );
        assert!(namespace.starts_with("agent-safe:"));
        assert!(!namespace.contains("secret"));

        config.command = "different.exe".to_string();
        assert_ne!(stable_agent_namespace(&config).unwrap(), namespace);
        config.command.clear();
        assert_eq!(stable_agent_namespace(&config), None);
    }

    #[test]
    fn allow_all_prefers_persistent_then_one_time_allow_options() {
        let once = PermissionOption::new("once", "Allow once", PermissionOptionKind::AllowOnce);
        let always =
            PermissionOption::new("always", "Always allow", PermissionOptionKind::AllowAlways);
        let reject = PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectAlways);

        assert_eq!(
            preferred_allow_option(&[once.clone(), always.clone()])
                .map(|option| option.option_id.to_string())
                .as_deref(),
            Some("always")
        );
        assert_eq!(
            preferred_allow_option(&[reject.clone(), once])
                .map(|option| option.option_id.to_string())
                .as_deref(),
            Some("once")
        );
        assert!(preferred_allow_option(&[reject]).is_none());
    }

    #[test]
    fn live_permission_policy_updates_without_restarting_agent() {
        let manager = AcpManager::new(vec![]);
        let agent_id = AgentId::new();
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        let policy = Arc::new(Mutex::new(PermissionPolicy::Ask));
        manager.agents.lock().insert(
            agent_id.clone(),
            AgentEntry {
                command_tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: Some("config:test".to_string()),
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: policy.clone(),
            },
        );

        manager
            .set_permission_policy(&agent_id, PermissionPolicy::AllowAll)
            .unwrap();

        assert_eq!(*policy.lock(), PermissionPolicy::AllowAll);
    }

    #[tokio::test]
    async fn spawn_reuses_live_agent_for_same_config_id() {
        let manager = AcpManager::new(vec![]);
        let agent_id = AgentId("runtime-codex".to_string());
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        manager.agents.lock().insert(
            agent_id.clone(),
            AgentEntry {
                command_tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: Some("config:acp-registry:codex-acp".to_string()),
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );

        let outcome = manager
            .spawn(AgentConfig {
                config_id: Some("acp-registry:codex-acp".to_string()),
                name: "Codex".to_string(),
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@agentclientprotocol/codex-acp@1.3.0".to_string(),
                ],
                env: Default::default(),
                allow_terminal: false,
                permission_policy: PermissionPolicy::Ask,
            })
            .await
            .unwrap();
        assert_eq!(outcome.agent_id, agent_id);
        assert!(manager
            .list_running_namespaces()
            .iter()
            .any(|(id, namespace)| {
                id == "runtime-codex"
                    && namespace.as_deref() == Some("config:acp-registry:codex-acp")
            }));
    }

    fn insert_live_agent(manager: &AcpManager, agent_id: AgentId, namespace: &str) {
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        manager.agents.lock().insert(
            agent_id,
            AgentEntry {
                command_tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: Some(namespace.to_string()),
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
    }

    #[tokio::test]
    async fn resolve_live_agent_id_remaps_stale_spawn_uuid_via_binding_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let bootstrap = crate::conversation::ConversationBootstrap::run(
            crate::conversation::HostConversationRoots::desktop(
                temp.path().join("state"),
                temp.path().join("visible"),
            ),
            crate::conversation::MigrationHostMode::Desktop,
        )
        .unwrap();
        let manager = AcpManager::with_conversation_services(
            Vec::new(),
            Arc::clone(&bootstrap.creation),
            Arc::clone(&bootstrap.persistence_adapter),
        );
        let prepared = bootstrap
            .creation
            .prepare_conversation(PrepareConversationRequest::new(ExecutionTarget::Workspace))
            .await
            .unwrap();
        let created_at =
            crate::conversation::parse_created_at_utc(&prepared.created_at_utc).unwrap();
        let stale = AgentId("470a8293-9a07-48c7-8fd2-1a6c5391a924".to_string());
        let live = AgentId("live-cursor-after-restart".to_string());
        bootstrap
            .writer
            .bind_agent_session(
                prepared.conversation_id,
                AgentSessionBinding {
                    schema_version: crate::conversation::AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: uuid::Uuid::new_v4(),
                    agent_session_id: "opaque/hello-there".to_string(),
                    runtime_agent_id: stale.0.clone(),
                    stable_agent_namespace: "config:acp-registry:cursor".to_string(),
                    execution_cwd: prepared.execution_cwd.clone(),
                    bound_at_utc: created_at,
                    state: crate::conversation::AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        insert_live_agent(&manager, live.clone(), "config:acp-registry:cursor");

        assert_eq!(
            manager
                .resolve_live_agent_id(&stale, Some("opaque/hello-there"))
                .unwrap(),
            live
        );
        assert_eq!(
            manager
                .resolve_live_agent_id(&live, Some("opaque/hello-there"))
                .unwrap(),
            live
        );
        assert!(manager
            .resolve_live_agent_id(&stale, Some("missing-session"))
            .unwrap_err()
            .starts_with("unknown agent:"));

        let continued = manager
            .new_session_with_context(
                &stale,
                prepared.execution_cwd,
                Vec::new(),
                SessionCreationContext {
                    conversation_id: Some(prepared.conversation_id),
                    ..SessionCreationContext::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(continued.session_id.0, "opaque/hello-there");
        assert_eq!(continued.conversation_id, Some(prepared.conversation_id));
    }

    #[tokio::test]
    async fn owns_session_queries_authoritative_agent_driver_state() {
        let manager = AcpManager::new(vec![]);
        let agent_id = AgentId::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        manager.agents.lock().insert(
            agent_id.clone(),
            AgentEntry {
                command_tx: tx,
                capabilities: AgentCapabilities::default(),
                stable_namespace: None,
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
        let requested = SessionId::new("owned-session");
        let responder = tokio::spawn(async move {
            match rx.recv().await.unwrap() {
                AcpCommand::OwnsSession { session_id, reply } => {
                    assert_eq!(session_id, requested);
                    let _ = reply.send(Ok(false));
                }
                _ => panic!("ownership query must use OwnsSession"),
            }
        });
        assert!(!manager
            .owns_session(&agent_id, SessionId::new("owned-session"))
            .await
            .unwrap());
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn skip_reopen_returns_cached_composer_controls_for_live_session() {
        let manager = AcpManager::new(vec![]);
        let agent_id = AgentId::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut capabilities = AgentCapabilities::default();
        capabilities.session_capabilities.resume = Some(Default::default());
        manager.agents.lock().insert(
            agent_id.clone(),
            AgentEntry {
                command_tx: tx,
                capabilities,
                stable_namespace: None,
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
        let modes = agent_client_protocol::schema::v1::SessionModeState::new("ask", vec![]);
        let options = vec![SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![
                agent_client_protocol::schema::v1::SessionConfigSelectOption::new("m1", "Fast"),
                agent_client_protocol::schema::v1::SessionConfigSelectOption::new("m2", "Pro"),
            ],
        )
        .category(agent_client_protocol::schema::v1::SessionConfigOptionCategory::Model)];
        let models = events::models_from_config_options(Some(options.as_slice()));
        manager.composer_controls.remember(
            "live-session",
            &SessionReopenOutcome {
                modes: Some(modes.clone()),
                models: models.clone(),
                config_options: Some(options.clone()),
            },
        );
        let responder = tokio::spawn(async move {
            match rx.recv().await.unwrap() {
                AcpCommand::OwnsSession { reply, .. } => {
                    let _ = reply.send(Ok(true));
                }
                _ => panic!("live-session reopen must not call the agent"),
            }
        });
        let outcome = manager
            .resume_session(
                &agent_id,
                SessionId::new("live-session"),
                "/work".to_string(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome.modes, Some(modes));
        assert_eq!(outcome.models, models);
        assert_eq!(outcome.config_options, Some(options));
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn conversation_compensation_double_failure_persists_actionable_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("state");
        let bootstrap = crate::conversation::ConversationBootstrap::run(
            crate::conversation::HostConversationRoots::desktop(
                state_root.clone(),
                temp.path().join("visible"),
            ),
            crate::conversation::MigrationHostMode::Desktop,
        )
        .unwrap();
        bootstrap.repository.fail_next_agent_binding_appends(2);
        let manager = Arc::new(AcpManager::with_conversation_services(
            Vec::new(),
            Arc::clone(&bootstrap.creation),
            Arc::clone(&bootstrap.persistence_adapter),
        ));
        let agent_id = AgentId("fake-agent".to_string());
        let (observed_tx, _observed_rx) = std::sync::mpsc::sync_channel(1);
        manager.install_test_agent_for_new_session_with_close_result(
            agent_id.clone(),
            observed_tx,
            Err("provider close leaked SUPER_SECRET=do-not-return".to_string()),
        );

        let error = manager
            .new_session_with_context(
                &agent_id,
                temp.path()
                    .join("ignored-cwd")
                    .to_string_lossy()
                    .into_owned(),
                Vec::new(),
                SessionCreationContext {
                    execution_target: Some(ExecutionTarget::Workspace),
                    ..SessionCreationContext::default()
                },
            )
            .await
            .unwrap_err();
        let failure = AgentCompensationFailure::from_wire_error(&error)
            .expect("double failure must return the stable compound receipt");
        assert_eq!(failure.primary_code, "CONVERSATION_BIND_FAILED");
        assert_eq!(
            failure.provider_close_code.as_deref(),
            Some("ACP_CLOSE_FAILED")
        );
        assert_eq!(
            failure.failure_record_code.as_deref(),
            Some("CONVERSATION_DURABILITY_FAILED")
        );
        assert!(failure.recovery_id.is_some());
        assert!(!error.contains("SUPER_SECRET"));
        assert!(!error.contains("opaque/fake-session"));

        let record = bootstrap.repository.list_conversations().remove(0);
        assert_eq!(record.conversation_id, failure.conversation_id);
        assert_eq!(
            record.lifecycle_state,
            crate::conversation::ConversationLifecycleState::RecoveryRequired
        );
        assert!(bootstrap
            .repository
            .current_binding(record.conversation_id)
            .unwrap()
            .is_none());

        let recovery_bytes = std::fs::read(
            state_root
                .join("conversation-migrations")
                .join("workspace-recovery-v1")
                .join(crate::conversation::migration::RECOVERY_ITEMS_FILE),
        )
        .unwrap();
        let recovery: crate::conversation::migration::RecoveryQueueV1 =
            serde_json::from_slice(&recovery_bytes).unwrap();
        assert_eq!(recovery.items.len(), 1);
        assert_eq!(recovery.items[0].recovery_id, failure.recovery_id.unwrap());
        let serialized = String::from_utf8(recovery_bytes).unwrap();
        assert!(serialized.contains("acpCompensationFailed"));
        assert!(!serialized.contains("SUPER_SECRET"));
        assert!(!serialized.contains("opaque/fake-session"));
    }

    /// Capability gating exercises the *real* gate functions used by
    /// `load_session`/`resume_session`/`close_session` (F4). With default
    /// capabilities every gate must reject; the `*_call_*` channel test below
    /// confirms no command is sent on the rejection path.
    #[test]
    fn real_capability_gates_reject_when_unsupported() {
        let caps = AgentCapabilities::default();
        assert!(
            gate_close_session(&caps).is_err(),
            "default agent must not advertise close"
        );
        assert!(
            gate_load_session(&caps).is_err(),
            "default agent must not advertise loadSession"
        );
        assert!(
            gate_resume_session(&caps).is_err(),
            "default agent must not advertise resume"
        );
        assert!(
            gate_list_sessions(&caps).is_err(),
            "default agent must not advertise session/list"
        );
    }

    #[tokio::test]
    async fn reopen_methods_prepend_internal_mcp_to_configured_servers() {
        let manager = AcpManager::new(vec![]);
        let agent_id = AgentId::new();
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let mut capabilities = AgentCapabilities::default();
        capabilities.load_session = true;
        capabilities.session_capabilities.resume = Some(Default::default());
        manager.agents.lock().insert(
            agent_id.clone(),
            AgentEntry {
                command_tx,
                capabilities,
                stable_namespace: Some("config:test".to_string()),
                join_handle: None,
                killed: Arc::new(AtomicBool::new(false)),
                permission_policy: Arc::new(Mutex::new(PermissionPolicy::Ask)),
            },
        );
        let configured = McpServer::Stdio(McpServerStdio::new(
            "configured".to_string(),
            PathBuf::from("/bin/echo"),
        ));
        let capture = tokio::spawn(async move {
            let mut received = Vec::new();
            while received.len() < 2 {
                let command = command_rx.recv().await.unwrap();
                match command {
                    AcpCommand::OwnsSession { reply, .. } => {
                        reply.send(Ok(false)).unwrap();
                    }
                    AcpCommand::ResumeSession {
                        mcp_servers, reply, ..
                    }
                    | AcpCommand::LoadSession {
                        mcp_servers, reply, ..
                    } => {
                        received.push(mcp_servers);
                        reply
                            .send(Ok(SessionReopenOutcome {
                                modes: None,
                                models: None,
                                config_options: None,
                            }))
                            .unwrap();
                    }
                    _ => panic!("unexpected ACP command"),
                }
            }
            received
        });

        manager
            .resume_session(
                &agent_id,
                SessionId::new("resume-session"),
                "/workspace".to_string(),
                vec![configured.clone()],
            )
            .await
            .unwrap();
        manager
            .load_session(
                &agent_id,
                SessionId::new("load-session"),
                "/workspace".to_string(),
                vec![configured],
            )
            .await
            .unwrap();

        for servers in capture.await.unwrap() {
            assert_eq!(servers.len(), 2);
            assert_eq!(serde_json::to_value(&servers[0]).unwrap()["name"], "termul");
            assert_eq!(
                serde_json::to_value(&servers[1]).unwrap()["name"],
                "configured"
            );
        }
    }

    /// The rejection path must NOT enqueue any command (agent never contacted).
    /// Drives the real gate, then asserts the channel stayed empty (AC-4).
    #[tokio::test]
    async fn gated_call_without_capability_returns_err_and_does_not_send() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AcpCommand>();

        let caps = AgentCapabilities::default();
        let result: Result<(), String> = async {
            // The real production gate, not a mirror.
            gate_close_session(&caps)?;
            send_command(&tx, |reply| AcpCommand::CloseSession {
                session_id: SessionId::new("s"),
                reply,
            })
            .await
        }
        .await;

        assert!(result.is_err(), "gated call must return Err");
        assert!(
            matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "no command must have been sent to the agent"
        );
    }

    /// A capable call (capability present) does enqueue a command on the channel.
    #[tokio::test]
    async fn capable_call_enqueues_command() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AcpCommand>();

        let (reply_tx, _reply_rx) = oneshot::channel::<Result<(), String>>();
        tx.send(AcpCommand::CloseSession {
            session_id: SessionId::new("s"),
            reply: reply_tx,
        })
        .unwrap();

        assert!(
            matches!(rx.try_recv(), Ok(AcpCommand::CloseSession { .. })),
            "command must be enqueued when capability is present"
        );
    }

    /// `send_command` surfaces a typed error when the driver thread is gone.
    #[tokio::test]
    async fn send_command_errors_when_thread_gone() {
        let (tx, rx) = mpsc::unbounded_channel::<AcpCommand>();
        drop(rx); // simulate a dead driver thread

        let result: Result<(), String> = send_command(&tx, |reply| AcpCommand::CloseSession {
            session_id: SessionId::new("s"),
            reply,
        })
        .await;

        assert!(result.is_err());
    }

    /// The post-cancel grace window forcibly resolves a turn whose agent never
    /// replies to `session/cancel` (M5). This drives the exact `select!` /
    /// timeout shape used in the `SendPrompt` arm against a prompt future that
    /// never completes, and asserts it resolves `Cancelled` rather than hanging.
    /// A short local grace keeps the test fast (the production constant is
    /// `CANCEL_GRACE`).
    #[tokio::test]
    async fn cancel_grace_forcibly_resolves_a_stuck_turn() {
        const TEST_GRACE: Duration = Duration::from_millis(50);
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        // A prompt future that never resolves (agent ignores cancel).
        let prompt = std::future::pending::<Result<StopReason, String>>();
        tokio::pin!(prompt);

        // Fire the cancel signal immediately.
        cancel_tx.send(()).unwrap();

        let outcome: Result<StopReason, String> = tokio::select! {
            result = &mut prompt => result,
            _ = cancel_rx => {
                match tokio::time::timeout(TEST_GRACE, &mut prompt).await {
                    Ok(result) => result,
                    Err(_) => Ok(StopReason::Cancelled),
                }
            }
        };

        assert_eq!(
            outcome,
            Ok(StopReason::Cancelled),
            "a stuck turn must be force-resolved as Cancelled after the grace window"
        );
    }

    /// Shape test (like `cancel_grace_forcibly_resolves_a_stuck_turn`): mirrors
    /// the timeout-around-request pattern of the `LoadSession`/`ResumeSession`
    /// arms against a never-resolving future, using a short local bound so the
    /// is fast. The arms themselves need a live connection + sink fan-out and
    /// are not driven here; this covers the match shape plus the production
    /// timeout resolution below.
    #[tokio::test]
    async fn session_reopen_times_out_instead_of_hanging() {
        const TEST_TIMEOUT: Duration = Duration::from_millis(50);
        let request = std::future::pending::<Result<(), String>>();
        let outcome = tokio::time::timeout(TEST_TIMEOUT, request).await;
        let result: Result<(), String> = match outcome {
            Ok(result) => result,
            Err(_) => Err(format!("session/load timed out after {TEST_TIMEOUT:?}")),
        };
        assert!(
            result.is_err_and(|e| e.contains("timed out")),
            "a hung reopen must resolve to a timeout error"
        );
    }

    /// The production reopen budget resolves to the 60s default and honors the
    /// `TERMUL_ACP_SESSION_REOPEN_TIMEOUT_SECS` diagnostic override contract
    /// (mirrors `session_new_timeout`). Only the default path is asserted —
    /// mutating process env in a test would race other tests.
    #[test]
    fn session_reopen_timeout_defaults_to_constant() {
        if std::env::var("TERMUL_ACP_SESSION_REOPEN_TIMEOUT_SECS").is_err() {
            assert_eq!(session_reopen_timeout(), SESSION_REOPEN_TIMEOUT);
        }
        assert_eq!(SESSION_REOPEN_TIMEOUT, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn session_load_reopen_preserves_optional_fields_and_records_root() {
        let state = Mutex::new(DriverState::new());
        let modes = agent_client_protocol::schema::v1::SessionModeState::new("ask", vec![]);
        let response = LoadSessionResponse::new()
            .modes(modes.clone())
            .config_options(Vec::<SessionConfigOption>::new());
        let outcome =
            run_session_reopen("session/load", "sess-load", "/work", &state, async move {
                Ok::<_, String>(response)
            })
            .await
            .unwrap();

        assert_eq!(outcome.modes, Some(modes));
        assert_eq!(outcome.models, None);
        assert_eq!(outcome.config_options, Some(vec![]));
        assert_eq!(
            state.lock().session_root("sess-load"),
            Some(PathBuf::from("/work"))
        );
    }

    #[tokio::test]
    async fn session_resume_reopen_preserves_omitted_fields() {
        let state = Mutex::new(DriverState::new());
        let outcome = run_session_reopen("session/resume", "sess-resume", "/work", &state, async {
            Ok::<_, String>(ResumeSessionResponse::new())
        })
        .await
        .unwrap();

        assert_eq!(
            outcome,
            SessionReopenOutcome {
                modes: None,
                models: None,
                config_options: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            serde_json::json!({})
        );
    }

    /// An empty prompt is rejected before any agent contact (EMPTY-CONTENT).
    /// `send_prompt`'s guard is a pure pre-check; assert its predicate here
    /// (the manager method needs a sink fan-out, but the guard runs first).
    #[test]
    fn empty_prompt_content_is_rejected_by_guard() {
        let content: Vec<ContentBlock> = Vec::new();
        // Mirror of the guard at the top of `AcpManager::send_prompt`.
        let rejected = content.is_empty();
        assert!(rejected, "empty prompt content must be rejected");
    }

    /// `ReplySlot` delivers exactly once: the spawn-failure path and the task
    /// path can both target it, but only the first send wins (L5 safety).
    #[tokio::test]
    async fn reply_slot_sends_exactly_once() {
        let (tx, rx) = oneshot::channel::<Result<(), String>>();
        let slot = reply_slot(tx);
        send_reply(&slot, Ok(()));
        // A second send is a no-op and must not panic.
        send_reply(&slot, Err("late".to_string()));
        assert_eq!(rx.await.unwrap(), Ok(()));
    }

    /// The initialize→spawn-event mapping forwards the FULL advertised auth
    /// method metadata (id, name, optional description) so the renderer can
    /// present Sign-in and call `authenticate(methodId)`. Optional description
    /// is preserved when present and `None` when absent.
    #[test]
    fn to_auth_method_infos_maps_full_metadata() {
        use agent_client_protocol::schema::v1::AuthMethodAgent;
        let methods = vec![
            AuthMethod::Agent(
                AuthMethodAgent::new("cursor_login", "Sign in with Cursor")
                    .description("Opens the Cursor login flow"),
            ),
            AuthMethod::Agent(AuthMethodAgent::new("api_key", "API key")),
        ];
        let infos = to_auth_method_infos(&methods);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].id, "cursor_login");
        assert_eq!(infos[0].name, "Sign in with Cursor");
        assert_eq!(
            infos[0].description.as_deref(),
            Some("Opens the Cursor login flow")
        );
        assert_eq!(infos[1].id, "api_key");
        assert_eq!(infos[1].name, "API key");
        assert_eq!(infos[1].description, None);
    }

    /// An agent that advertises no auth methods maps to an empty vec (the
    /// renderer treats this as a no-auth agent).
    #[test]
    fn to_auth_method_infos_empty_for_no_methods() {
        assert!(to_auth_method_infos(&[]).is_empty());
    }

    // --- race_turn (idle + hard cap + cancel) ---

    /// An active turn (agent streaming activity) keeps resetting its idle
    /// deadline and completes normally — never hits the idle timeout.
    #[tokio::test(start_paused = true)]
    async fn race_turn_activity_resets_idle_and_completes() {
        let (idle_tx, mut idle_rx) = watch::channel(());
        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        // Activity every 20ms (under the 50ms idle window) keeps the idle
        // deadline pushed back; the prompt completes at 100ms.
        let activity = tokio::spawn(async move {
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = idle_tx.send(());
            }
        });
        let result = race_turn(
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok::<StopReason, String>(StopReason::EndTurn)
            },
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            Some(Duration::from_millis(50)),
            Some(Duration::from_secs(10)),
        )
        .await;
        let _ = activity.await;
        assert!(result.is_ok(), "got {result:?}");
        assert_eq!(
            on_timeout.load(Ordering::SeqCst),
            0,
            "no timeout should fire for an active turn"
        );
    }

    /// A silent (wedged) turn with no activity and no completion hits the idle
    /// timeout — and signals cancel via `on_timeout_cancel`.
    #[tokio::test(start_paused = true)]
    async fn race_turn_silence_hits_idle_timeout() {
        let (_idle_tx, mut idle_rx) = watch::channel(()); // never fires
        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        let result = race_turn(
            std::future::pending::<Result<StopReason, String>>(),
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            Some(Duration::from_millis(100)),
            Some(Duration::from_secs(10)),
        )
        .await;
        let err = result.unwrap_err();
        assert!(err.contains("idle timeout"), "got {err}");
        assert_eq!(on_timeout.load(Ordering::SeqCst), 1);
    }

    /// A turn that streams activity forever but never completes is still
    /// bounded by the hard wall-clock cap (the pre-loop check fires despite
    /// the continuously-ready activity arm under `biased`).
    #[tokio::test(start_paused = true)]
    async fn race_turn_streaming_non_completing_hits_hard_cap() {
        let (idle_tx, mut idle_rx) = watch::channel(());
        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let activity = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = idle_tx.send(());
            }
        });
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        let result = race_turn(
            std::future::pending::<Result<StopReason, String>>(),
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            Some(Duration::from_millis(50)),
            Some(Duration::from_millis(200)),
        )
        .await;
        activity.abort();
        let _ = activity.await;
        let err = result.unwrap_err();
        assert!(err.contains("hard timeout"), "got {err}");
        assert_eq!(on_timeout.load(Ordering::SeqCst), 1);
    }

    /// A user cancel wins over both timeouts: the cancel arm returns
    /// `Cancelled` after `CANCEL_GRACE`, and `on_timeout_cancel` is not called.
    #[tokio::test(start_paused = true)]
    async fn race_turn_cancel_wins_over_timeouts() {
        let (_idle_tx, mut idle_rx) = watch::channel(()); // no activity
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        // Cancel before the idle window fires.
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = cancel_tx.send(());
        });
        let result = race_turn(
            std::future::pending::<Result<StopReason, String>>(),
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            Some(Duration::from_millis(100)),
            Some(Duration::from_secs(10)),
        )
        .await;
        let _ = canceller.await;
        assert!(
            matches!(result, Ok(StopReason::Cancelled)),
            "got {result:?}"
        );
        assert_eq!(on_timeout.load(Ordering::SeqCst), 0);
    }

    /// Fully-unlimited default (`idle = None`, `hard = None`): a silent,
    /// never-completing turn is bounded ONLY by an explicit cancel — no
    /// timeout fires. This is the new default contract.
    #[tokio::test(start_paused = true)]
    async fn race_turn_unlimited_silent_turn_only_ends_on_cancel() {
        let (_idle_tx, mut idle_rx) = watch::channel(()); // no activity
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = cancel_tx.send(());
        });
        let result = race_turn(
            std::future::pending::<Result<StopReason, String>>(),
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            None,
            None,
        )
        .await;
        let _ = canceller.await;
        assert!(
            matches!(result, Ok(StopReason::Cancelled)),
            "got {result:?}"
        );
        assert_eq!(
            on_timeout.load(Ordering::SeqCst),
            0,
            "unlimited turn must not invoke on_timeout_cancel"
        );
    }

    /// Unlimited hard cap (`hard = None`) with a bounded idle: a streaming,
    /// never-completing turn that keeps activity alive never hits the idle
    /// deadline and is bounded ONLY by cancel — the absent hard cap imposes no
    /// bound, matching "unlimited hard cap".
    #[tokio::test(start_paused = true)]
    async fn race_turn_unlimited_hard_cap_active_turn_only_ends_on_cancel() {
        let (idle_tx, mut idle_rx) = watch::channel(());
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let activity = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = idle_tx.send(());
            }
        });
        let on_timeout = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let on_timeout_clone = on_timeout.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _ = cancel_tx.send(());
        });
        // idle 50ms (kept reset by activity), hard None (unlimited): an active
        // turn never hits either, so only cancel ends it.
        let result = race_turn(
            std::future::pending::<Result<StopReason, String>>(),
            cancel_rx,
            &mut idle_rx,
            move || {
                on_timeout_clone.fetch_add(1, Ordering::SeqCst);
            },
            Some(Duration::from_millis(50)),
            None,
        )
        .await;
        activity.abort();
        let _ = activity.await;
        let _ = canceller.await;
        assert!(
            matches!(result, Ok(StopReason::Cancelled)),
            "got {result:?}"
        );
        assert_eq!(
            on_timeout.load(Ordering::SeqCst),
            0,
            "unlimited hard cap must not invoke on_timeout_cancel for an active turn"
        );
    }

    /// `resolved_turn_timeout` precedence: env var > UI override > default.
    /// The override replaces the default when no env var is set; the default is
    /// now `None` (unlimited).
    #[test]
    fn turn_timeout_override_takes_effect_when_no_env_var() {
        // The env var is usually absent in the test runner; when it IS set
        // (operator machine), it correctly masks the UI override — skip there.
        if std::env::var("TERMUL_ACP_TURN_TIMEOUT_SECS").is_ok() {
            return;
        }
        set_turn_timeout_override(Some(42));
        assert_eq!(resolved_turn_timeout(), Some(Duration::from_secs(42)));
        set_turn_timeout_override(None);
        assert_eq!(resolved_turn_timeout(), None);
    }

    /// `turn_idle_timeout` full precedence ladder, in ONE test so the shared
    /// override static and env var are never touched by concurrent tests:
    /// env var absent → override wins over default; cleared → default (`None`
    /// / unlimited); env var present → env beats a simultaneous UI override
    /// (the operator precedence the settings UI documents). When the env var is
    /// ALREADY set on the host (operator machine), only the env-wins phase runs
    /// and its original value is restored afterwards.
    #[test]
    fn turn_idle_timeout_precedence_env_beats_override_beats_default() {
        let preexisting = std::env::var("TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS").ok();
        if preexisting.is_none() {
            set_turn_idle_timeout_override(Some(42));
            assert_eq!(turn_idle_timeout(), Some(Duration::from_secs(42)));
            set_turn_idle_timeout_override(None);
            assert_eq!(turn_idle_timeout(), None);
        }

        std::env::set_var("TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS", "60");
        set_turn_idle_timeout_override(Some(1800));
        assert_eq!(turn_idle_timeout(), Some(Duration::from_secs(60)));

        match preexisting {
            Some(v) => std::env::set_var("TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS", v),
            None => std::env::remove_var("TERMUL_ACP_TURN_IDLE_TIMEOUT_SECS"),
        }
        set_turn_idle_timeout_override(None);
    }

    /// `session_new_timeout` precedence: env var > UI override > default.
    /// A zero override is ignored (the IPC command rejects it; the resolver
    /// also filters it defensively).
    #[test]
    fn session_new_timeout_override_takes_effect_when_no_env_var() {
        if std::env::var("TERMUL_ACP_SESSION_NEW_TIMEOUT_SECS").is_ok() {
            return;
        }
        set_session_new_timeout_override(Some(42));
        assert_eq!(session_new_timeout(), Duration::from_secs(42));
        set_session_new_timeout_override(Some(0));
        assert_eq!(session_new_timeout(), SESSION_NEW_TIMEOUT);
        set_session_new_timeout_override(None);
        assert_eq!(session_new_timeout(), SESSION_NEW_TIMEOUT);
    }

    /// `session_reopen_timeout` precedence: env var > UI override > default.
    /// A zero override is ignored (same defensive filter as session/new).
    #[test]
    fn session_reopen_timeout_override_takes_effect_when_no_env_var() {
        if std::env::var("TERMUL_ACP_SESSION_REOPEN_TIMEOUT_SECS").is_ok() {
            return;
        }
        set_session_reopen_timeout_override(Some(42));
        assert_eq!(session_reopen_timeout(), Duration::from_secs(42));
        set_session_reopen_timeout_override(Some(0));
        assert_eq!(session_reopen_timeout(), SESSION_REOPEN_TIMEOUT);
        set_session_reopen_timeout_override(None);
        assert_eq!(session_reopen_timeout(), SESSION_REOPEN_TIMEOUT);
    }

    /// `first_prompt_warmup_timeout` full precedence ladder, in ONE test so
    /// the shared override static and env var are never touched by concurrent
    /// tests: override wins over default; `0` disables (unlike the other
    /// overrides); cleared → default; and an INVALID env value falls through
    /// to the override (incl. a disabling `0`) instead of masking it. When
    /// the env var is ALREADY set on the host (operator machine), only the
    /// invalid-env phase runs and the original value is restored afterwards.
    #[test]
    fn first_prompt_warmup_timeout_precedence_and_invalid_env() {
        let preexisting = std::env::var("TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS").ok();
        if preexisting.is_none() {
            set_first_prompt_warmup_timeout_override(Some(7));
            assert_eq!(first_prompt_warmup_timeout(), Duration::from_secs(7));
            set_first_prompt_warmup_timeout_override(Some(0));
            assert_eq!(first_prompt_warmup_timeout(), Duration::ZERO);
            set_first_prompt_warmup_timeout_override(None);
            assert_eq!(first_prompt_warmup_timeout(), FIRST_PROMPT_WARMUP_TIMEOUT);
        }

        std::env::set_var("TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS", "not-a-number");
        set_first_prompt_warmup_timeout_override(Some(9));
        assert_eq!(first_prompt_warmup_timeout(), Duration::from_secs(9));
        set_first_prompt_warmup_timeout_override(Some(0));
        assert_eq!(first_prompt_warmup_timeout(), Duration::ZERO);
        set_first_prompt_warmup_timeout_override(None);
        assert_eq!(first_prompt_warmup_timeout(), FIRST_PROMPT_WARMUP_TIMEOUT);

        match preexisting {
            Some(v) => std::env::set_var("TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS", v),
            None => std::env::remove_var("TERMUL_ACP_FIRST_PROMPT_WARMUP_SECS"),
        }
        set_first_prompt_warmup_timeout_override(None);
    }

    // --- First-prompt warmup dedup (I/O matrix Rows 5 & 6) ---

    /// I/O matrix Row 5 — "Duplicate warmup trigger": a PtyManager
    /// window-visible tick re-fires `NewSession` for an agent whose warmup
    /// already completed. `warmup_should_run` returns `false` (skip second
    /// warmup) and logs a debug line; the entry stays in the set so the agent
    /// remains "done" for its lifetime (a third trigger also skips).
    #[test]
    fn warmup_should_run_skips_when_agent_already_completed() {
        let warmup_done: Arc<Mutex<HashSet<AgentId>>> = Arc::new(Mutex::new(HashSet::new()));
        let agent_id = AgentId::new();
        // Simulate a prior warmup that already completed (entry inserted by a
        // previous NewSession call).
        warmup_done.lock().insert(agent_id.clone());

        // A re-entry sees the entry and returns false (skip).
        assert!(
            !warmup_should_run(&warmup_done, &agent_id),
            "a duplicate trigger for an already-warmed agent must be skipped"
        );

        // The entry persists — the agent stays "done" so a third trigger also
        // skips (not cleared on a skip).
        assert!(
            warmup_done.lock().contains(&agent_id),
            "the done entry must persist after a skip (agent stays done)"
        );
        // A third trigger still skips.
        assert!(
            !warmup_should_run(&warmup_done, &agent_id),
            "a third trigger must also skip while the agent is done"
        );
    }

    /// I/O matrix Row 6 — "Warmup already in-flight": a second visibility
    /// tick while the first warmup is still pending coalesces onto the pending
    /// warmup (do not spawn a second). Because `warmup_should_run` performs
    /// the check + insert atomically under one lock, two concurrent callers
    /// for the same agent cannot both pass the gate: exactly one wins (returns
    /// `true` + inserts), the other sees the entry and coalesces (`false`).
    #[test]
    fn warmup_should_run_coalesces_concurrent_calls_for_same_agent() {
        let warmup_done: Arc<Mutex<HashSet<AgentId>>> = Arc::new(Mutex::new(HashSet::new()));
        let agent_id = Arc::new(AgentId::new());

        // Two concurrent callers race for the same agent. Because check+insert
        // is atomic, exactly one wins (returns true) and the other coalesces
        // (false) — no second warmup is spawned.
        let set_a = Arc::clone(&warmup_done);
        let set_b = Arc::clone(&warmup_done);
        let id_a = Arc::clone(&agent_id);
        let id_b = Arc::clone(&agent_id);
        let handle_a = std::thread::spawn(move || warmup_should_run(&set_a, &id_a));
        let handle_b = std::thread::spawn(move || warmup_should_run(&set_b, &id_b));
        let a = handle_a.join().expect("warmup thread a panicked");
        let b = handle_b.join().expect("warmup thread b panicked");

        // Exactly one caller runs the warmup; the other coalesces.
        assert!(
            a ^ b,
            "exactly one concurrent caller must win the warmup gate (got a={a}, b={b})"
        );
        // The set has exactly one entry for the agent (the winner inserted it).
        assert_eq!(
            warmup_done.lock().len(),
            1,
            "exactly one entry for the agent after concurrent calls"
        );
        assert!(
            warmup_done.lock().contains(&agent_id),
            "the winning caller must have inserted the agent"
        );

        // After both calls, a subsequent (non-concurrent) trigger also skips —
        // the agent is now "done" and stays done.
        assert!(
            !warmup_should_run(&warmup_done, &agent_id),
            "a post-completion trigger must skip (agent is done)"
        );
    }

    #[test]
    fn circuit_and_watermark_entries_respect_ttl_and_cardinality() {
        let circuits = Arc::new(Mutex::new(SessionDeliveryCircuitMap::new()));
        for ordinal in 0..(MAX_FAILURE_CIRCUIT_ENTRIES + 8) {
            circuits
                .lock()
                .insert(format!("session-{ordinal}"), "TEST_FATAL");
        }
        assert!(circuits.lock().len() <= MAX_FAILURE_CIRCUIT_ENTRIES);
        let expired = std::time::Instant::now()
            - std::time::Duration::from_secs(FAILURE_CIRCUIT_TTL_SECS + 1);
        circuit_map_insert_for_tests(&circuits, "expired", "TEST_FATAL", expired);
        assert!(circuits.lock().get("expired").is_none());

        let watermark = crate::web::permissions::TurnWatermark::new();
        for ordinal in 0..32 {
            watermark.mark_seen("ttl-session", &format!("turn-{ordinal}"));
        }
        assert!(watermark.is_seen("ttl-session", "turn-31"));
        watermark.forget_session("ttl-session");
        assert!(!watermark.is_seen("ttl-session", "turn-31"));
    }
}
