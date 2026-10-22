//! Terminal Core: owns PtyManager and serves local IPC.
//!
//! This module is the desktop PTY owner. It must not import ACP. GUI and
//! shared-live talk to it over bounded framed messages; standalone keeps
//! in-process ownership through [`super::handles`].

use super::handles::{
    TerminalConversationObservation, TerminalConversationTermination, TerminalRuntimeHandle,
    TerminalTerminationOutcome,
};
use super::ipc::{
    prepare_runtime_dir, read_frame, read_json_frame, remove_stale_socket,
    validate_hello_with_runtime, write_frame, write_json_frame, CoreEndpoint, CoreError,
    CoreErrorPayload, CoreEvent, CoreHello, CoreRequest, CoreResponse, CoreRole,
    CURRENT_PROTOCOL_VERSION,
};
use super::transport::{connect_core, listen_core, CoreReadHalf, CoreServerStream, CoreWriteHalf};
use async_trait::async_trait;
use parking_lot::Mutex;
use se_foundation::ids::ConversationId;
use se_pty::claims::RotatedClaim;
use se_pty::manager::{
    SpawnedTerminal, TerminalAttachResult, TerminalResumeGrant, TerminalResumeRequest,
};
use se_pty::trackers::{
    CwdTracker, ExitCodeTracker, GitTracker, TerminalDisplayMode, TerminalEvent, TerminalEventHub,
};
use se_pty::{PtyManager, SpawnOptions, TerminalProgram};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

const OUTPUT_FRAME_MAGIC: &[u8; 4] = b"TML2";
const OUTPUT_KIND_LIVE: u8 = 1;
const OUTPUT_KIND_REPLAY: u8 = 2;
const EVENT_TOPIC: &str = "terminal.event";
const GAP_TOPIC: &str = "terminal.gap";
const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub const METHOD_SPAWN: &str = "spawn";
pub const METHOD_LIST: &str = "list";
pub const METHOD_STATUS: &str = "status";
pub const METHOD_ATTACH: &str = "attach";
pub const METHOD_WATCH: &str = "watch";
pub const METHOD_DETACH: &str = "detach";
pub const METHOD_WRITE: &str = "write";
pub const METHOD_RESIZE: &str = "resize";
pub const METHOD_TERMINATE: &str = "terminate";
pub const METHOD_ROTATE: &str = "rotate";
pub const METHOD_REVOKE: &str = "revoke";
pub const METHOD_RESUME: &str = "resume";
pub const METHOD_SHUTDOWN: &str = "shutdown";
pub const METHOD_CLOSE_VIEW: &str = "close_view";
pub const METHOD_SET_DISPLAY_MODE: &str = "set_display_mode";
pub const METHOD_SET_PROTECTED: &str = "set_protected";
pub const METHOD_OBSERVE_CONVERSATION: &str = "observeConversation";
pub const METHOD_TERMINATE_FOR_CONVERSATION: &str = "terminateForConversation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Live,
    Replay,
    Gap,
}

#[derive(Debug, Clone)]
pub struct OutputFrame {
    pub kind: OutputKind,
    pub terminal_id: String,
    pub seq: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStatus {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    /// Empty when the PTY is scope-less (project/SSH). The process-local
    /// ConversationId used for claims is not a conversation identity.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub workspace_ref_tracked: bool,
    pub latest_seq: u64,
    pub active: bool,
    pub lifecycle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationTerminalParams {
    conversation_id: ConversationId,
    #[serde(default)]
    terminal_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationTerminalTerminateParams {
    conversation_id: ConversationId,
    terminal_id: String,
    operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalIdParams {
    terminal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimParams {
    terminal_id: String,
    #[serde(default)]
    claim: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachParams {
    terminal_id: String,
    #[serde(default)]
    claim: String,
    #[serde(default)]
    last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchParams {
    terminal_id: String,
    #[serde(default)]
    last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteParams {
    terminal_id: String,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeParams {
    terminal_id: String,
    cols: u16,
    rows: u16,
    // Absent from GUIs that predate pixel sizes; the PTY then keeps its last
    // known cell size.
    #[serde(default)]
    cell_width: Option<u16>,
    #[serde(default)]
    cell_height: Option<u16>,
}

/// Cell fields go on the wire only when known, so a request without one is the
/// JSON older GUIs sent. Older Cores ignore the extra keys: their
/// `ResizeParams` never had `deny_unknown_fields`.
fn resize_params(terminal_id: &str, cols: u16, rows: u16, cell: Option<(u16, u16)>) -> Value {
    let mut params = json!({ "terminalId": terminal_id, "cols": cols, "rows": rows });
    if let Some((width, height)) = cell {
        params["cellWidth"] = json!(width);
        params["cellHeight"] = json!(height);
    }
    params
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtectedParams {
    terminal_id: String,
    protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayModeParams {
    terminal_id: String,
    mode: String,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachReply {
    id: String,
    shell: String,
    cwd: String,
    pid: u32,
    cols: u16,
    rows: u16,
    latest_seq: u64,
    gap: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_generation: Option<u64>,
    conversation_id: String,
}

impl AttachReply {
    fn from_parts(
        result: TerminalAttachResult,
        generation: Option<u64>,
        conversation_id: ConversationId,
    ) -> Self {
        Self {
            id: result.id,
            shell: result.shell,
            cwd: result.cwd,
            pid: result.pid,
            cols: result.cols,
            rows: result.rows,
            latest_seq: result.latest_seq,
            gap: result.gap,
            claim_generation: generation,
            conversation_id: conversation_id.to_string(),
        }
    }

    fn into_result(self) -> TerminalAttachResult {
        TerminalAttachResult {
            id: self.id,
            shell: self.shell,
            cwd: self.cwd,
            pid: self.pid,
            cols: self.cols,
            rows: self.rows,
            latest_seq: self.latest_seq,
            gap: self.gap,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GapPayload {
    terminal_id: String,
    last_seq: u64,
}

pub fn encode_output_frame(
    kind: OutputKind,
    terminal_id: &str,
    seq: u64,
    data: &[u8],
) -> Result<Vec<u8>, CoreError> {
    let id_bytes = terminal_id.as_bytes();
    let id_len = u16::try_from(id_bytes.len()).map_err(|_| {
        CoreError::InvalidFrame("terminal id is too long for binary output frame".into())
    })?;
    let kind_byte = match kind {
        OutputKind::Live => OUTPUT_KIND_LIVE,
        OutputKind::Replay => OUTPUT_KIND_REPLAY,
        OutputKind::Gap => return Err(invalid("gap is not a binary output frame")),
    };
    let mut frame = Vec::with_capacity(15 + id_bytes.len() + data.len());
    frame.extend_from_slice(OUTPUT_FRAME_MAGIC);
    frame.push(kind_byte);
    frame.extend_from_slice(&id_len.to_be_bytes());
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(id_bytes);
    frame.extend_from_slice(data);
    Ok(frame)
}

pub fn decode_output_frame(payload: &[u8]) -> Result<OutputFrame, CoreError> {
    if payload.len() < 15 || &payload[0..4] != OUTPUT_FRAME_MAGIC {
        return Err(CoreError::InvalidFrame(
            "payload is not a terminal output frame".into(),
        ));
    }
    let kind = match payload[4] {
        OUTPUT_KIND_LIVE => OutputKind::Live,
        OUTPUT_KIND_REPLAY => OutputKind::Replay,
        _ => {
            return Err(CoreError::InvalidFrame(
                "unknown terminal output kind".into(),
            ))
        }
    };
    let id_len = u16::from_be_bytes([payload[5], payload[6]]) as usize;
    if payload.len() < 15 + id_len {
        return Err(CoreError::InvalidFrame(
            "truncated terminal output frame".into(),
        ));
    }
    let seq =
        u64::from_be_bytes(payload[7..15].try_into().map_err(|_| {
            CoreError::InvalidFrame("invalid terminal output sequence header".into())
        })?);
    let terminal_id = std::str::from_utf8(&payload[15..15 + id_len])
        .map_err(|_| CoreError::InvalidFrame("terminal id is not utf-8".into()))?
        .to_string();
    let data = payload[15 + id_len..].to_vec();
    Ok(OutputFrame {
        kind,
        terminal_id,
        seq,
        data,
    })
}

fn is_output_frame(payload: &[u8]) -> bool {
    payload.len() >= 4 && &payload[0..4] == OUTPUT_FRAME_MAGIC
}

fn unauthorized() -> CoreError {
    CoreError::Unauthorized
}

fn invalid(detail: impl Into<String>) -> CoreError {
    CoreError::InvalidRequest(detail.into())
}

fn error_response(id: u64, error: CoreError) -> CoreResponse {
    CoreResponse {
        id,
        result: None,
        error: Some(CoreErrorPayload {
            code: error.code().to_string(),
            message: match &error {
                CoreError::InvalidRequest(detail) if !detail.is_empty() => detail.clone(),
                other => other.client_message().to_string(),
            },
        }),
    }
}

fn ok_response(id: u64, result: Value) -> CoreResponse {
    CoreResponse {
        id,
        result: Some(result),
        error: None,
    }
}

pub(crate) fn status_from_instance(
    pty: &PtyManager,
    instance: &se_pty::manager::TerminalInstance,
) -> TerminalStatus {
    let latest_seq = instance
        .output_log
        .read()
        .back()
        .map(|chunk| chunk.seq)
        .unwrap_or(0);
    let cwd = pty
        .cwd_tracker()
        .get_cwd(&instance.id)
        .unwrap_or_else(|| instance.cwd.clone());
    TerminalStatus {
        id: instance.id.clone(),
        shell: instance.shell.clone(),
        cwd,
        pid: instance.pid,
        cols: *instance.cols.read(),
        rows: *instance.rows.read(),
        conversation_id: if instance.workspace_ref_tracked {
            instance.conversation_id.to_string()
        } else {
            String::new()
        },
        project_id: instance.project_id.clone(),
        workspace_ref_tracked: instance.workspace_ref_tracked,
        latest_seq,
        active: instance.is_active(),
        lifecycle: instance.lifecycle_state().as_str().to_string(),
        claim_generation: pty.claim_generation(&instance.id),
    }
}

pub fn list_terminal_statuses(pty: &PtyManager) -> Vec<TerminalStatus> {
    pty.get_all()
        .iter()
        .map(|instance| status_from_instance(pty, instance))
        .collect()
}

fn construct_pty_manager(program: TerminalProgram) -> Arc<PtyManager> {
    let events = TerminalEventHub::standalone();
    let cwd = Arc::new(CwdTracker::new(events.clone()));
    let git = Arc::new(GitTracker::new(events.clone()));
    let exit = Arc::new(ExitCodeTracker::new(events.clone()));
    Arc::new(PtyManager::new(events, cwd, git, exit, program))
}

struct TerminalCoreState {
    pty: Arc<PtyManager>,
    /// Quick terminals live beside the PTYs they own. `None` when the store
    /// could not be opened; the rest of Terminal Core keeps working.
    quick: Option<Arc<crate::quick_terminal::QuickTerminalService>>,
    shutdown: watch::Sender<bool>,
}

/// `program` is what spawned shells see as `TERM_PROGRAM` / its version.
pub async fn run_terminal_core(
    profile_root: PathBuf,
    program: TerminalProgram,
) -> Result<(), CoreError> {
    #[cfg(any(unix, windows))]
    {
        let endpoint = CoreEndpoint::for_profile(&profile_root, CoreRole::TerminalCore);
        let workspace_base = super::process::workspace_base_from_env();
        run_terminal_core_with(endpoint, Some((profile_root, workspace_base)), program).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (profile_root, program);
        Err(CoreError::UnsupportedPlatform)
    }
}

pub async fn run_terminal_core_on_endpoint(
    endpoint: CoreEndpoint,
    program: TerminalProgram,
) -> Result<(), CoreError> {
    run_terminal_core_with(endpoint, None, program).await
}

/// `quick_roots` is `(profile_root, workspace_base)` for the quick terminal
/// store; without it Terminal Core serves PTYs only.
pub async fn run_terminal_core_with(
    endpoint: CoreEndpoint,
    quick_roots: Option<(PathBuf, PathBuf)>,
    program: TerminalProgram,
) -> Result<(), CoreError> {
    prepare_runtime_dir(&endpoint)?;
    let _ = remove_stale_socket(&endpoint);
    let mut listener = listen_core(&endpoint, CoreRole::TerminalCore).await?;

    let pty = construct_pty_manager(program);
    let quick = quick_roots.and_then(|(profile_root, workspace_base)| {
        match crate::quick_terminal::open_service(&profile_root, &workspace_base, Arc::clone(&pty))
        {
            Ok(service) => Some(service),
            Err(error) => {
                log::error!(
                    target: "se_manager::core",
                    "operation=quick_terminal_store role=terminal-core stable_code=QUICK_TERMINAL_UNAVAILABLE error={error}"
                );
                None
            }
        }
    });
    let (shutdown, shutdown_rx) = watch::channel(false);
    let state = Arc::new(TerminalCoreState {
        pty: Arc::clone(&pty),
        quick,
        shutdown,
    });

    log::info!(
        target: "se_manager::core",
        "operation=core_listen role=terminal-core stable_code=READY"
    );

    let mut shutdown_rx = shutdown_rx;
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok(stream) => stream,
                    Err(error) => {
                        log::error!(
                            target: "se_manager::core",
                            "operation=core_accept role=terminal-core stable_code={} error={error}",
                            error.code()
                        );
                        tokio::time::sleep(super::transport::ACCEPT_RETRY_BACKOFF).await;
                        continue;
                    }
                };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, state).await {
                        log::debug!(
                            target: "se_manager::core",
                            "operation=core_connection stable_code={}",
                            error.code()
                        );
                    }
                });
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    stream: CoreServerStream,
    state: Arc<TerminalCoreState>,
) -> Result<(), CoreError> {
    use tokio::io::AsyncWriteExt;

    let mut stream = stream;
    let hello: CoreHello = read_json_frame(&mut stream).await?;
    let active_resources = u32::try_from(state.pty.get_count()).unwrap_or(u32::MAX);
    let ack = validate_hello_with_runtime(&hello, CoreRole::TerminalCore, active_resources)?;
    write_json_frame(&mut stream, &ack).await?;

    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let mut subscriptions: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut events = state.pty.terminal_events().subscribe();
    let event_writer = Arc::clone(&writer);

    let event_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let payload = match serde_json::to_value(&event) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let message = CoreEvent {
                        topic: EVENT_TOPIC.to_string(),
                        payload,
                    };
                    let mut guard = event_writer.lock().await;
                    if write_json_frame(&mut *guard, &message).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut shutdown_rx = state.shutdown.subscribe();
    let result = async {
        loop {
            let payload = tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
                payload = read_frame(&mut reader) => match payload {
                    Ok(payload) => payload,
                    Err(CoreError::Io(_)) => break,
                    Err(error) => return Err(error),
                },
            };
            if is_output_frame(&payload) {
                continue;
            }
            let request: CoreRequest = serde_json::from_slice(&payload)
                .map_err(|error| CoreError::InvalidFrame(format!("invalid JSON frame: {error}")))?;
            if crate::quick_terminal::is_quick_terminal_method(&request.method) {
                // Opening or deleting may start or end a shell; answer on a
                // task so terminal input on this connection keeps flowing.
                spawn_quick_terminal_reply(Arc::clone(&state), request, Arc::clone(&writer));
                continue;
            }
            let method = request.method.clone();
            let response = dispatch_request(&state, &request, &writer, &mut subscriptions).await;
            {
                let mut guard = writer.lock().await;
                write_json_frame(&mut *guard, &response).await?;
            }
            if method == METHOD_SHUTDOWN {
                let _ = state.shutdown.send(true);
                break;
            }
        }
        Ok(())
    }
    .await;

    event_task.abort();
    for (terminal_id, handle) in subscriptions.drain() {
        handle.abort();
        state.pty.note_view_closed(&terminal_id);
    }
    let mut guard = writer.lock().await;
    let _ = guard.shutdown().await;
    result
}

fn spawn_quick_terminal_reply(
    state: Arc<TerminalCoreState>,
    request: CoreRequest,
    writer: Arc<tokio::sync::Mutex<CoreWriteHalf>>,
) {
    tokio::spawn(async move {
        let reply = match state.quick.as_deref() {
            Some(service) => {
                crate::quick_terminal::dispatch(service, &request.method, request.params).await
            }
            None => crate::quick_terminal::QuickTerminalReply::Err {
                code: "QUICK_TERMINAL_UNAVAILABLE".to_string(),
                message: "quick terminal store is unavailable".to_string(),
            },
        };
        let response = match serde_json::to_value(&reply) {
            Ok(value) => ok_response(request.id, value),
            Err(error) => error_response(request.id, invalid(error.to_string())),
        };
        let mut guard = writer.lock().await;
        if let Err(error) = write_json_frame(&mut *guard, &response).await {
            log::debug!(
                target: "se_manager::core",
                "operation=quick_terminal_reply stable_code={} error={error}",
                error.code()
            );
        }
    });
}

async fn dispatch_request(
    state: &Arc<TerminalCoreState>,
    request: &CoreRequest,
    writer: &Arc<tokio::sync::Mutex<CoreWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
) -> CoreResponse {
    match handle_method(state, request, writer, subscriptions).await {
        Ok(value) => ok_response(request.id, value),
        Err(error) => error_response(request.id, error),
    }
}

async fn handle_method(
    state: &Arc<TerminalCoreState>,
    request: &CoreRequest,
    writer: &Arc<tokio::sync::Mutex<CoreWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
) -> Result<Value, CoreError> {
    let pty = &state.pty;
    match request.method.as_str() {
        METHOD_SPAWN => {
            let options: SpawnOptions = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let spawned = pty.spawn(options, None).await.map_err(map_spawn_error)?;
            serde_json::to_value(&spawned).map_err(|error| invalid(error.to_string()))
        }
        METHOD_LIST => {
            let statuses = list_terminal_statuses(pty);
            serde_json::to_value(&statuses).map_err(|error| invalid(error.to_string()))
        }
        METHOD_STATUS => {
            let params: TerminalIdParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let instance = pty
                .get(&params.terminal_id)
                .ok_or_else(|| invalid("terminal not found"))?;
            serde_json::to_value(status_from_instance(pty, &instance))
                .map_err(|error| invalid(error.to_string()))
        }
        METHOD_ATTACH => {
            let params: AttachParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let reply = begin_output_stream(
                pty,
                writer,
                subscriptions,
                &params.terminal_id,
                Some(params.claim.as_str()),
                params.last_seq,
            )
            .await?;
            serde_json::to_value(&reply).map_err(|error| invalid(error.to_string()))
        }
        METHOD_WATCH => {
            let params: WatchParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let reply = begin_output_stream(
                pty,
                writer,
                subscriptions,
                &params.terminal_id,
                None,
                params.last_seq,
            )
            .await?;
            serde_json::to_value(&reply).map_err(|error| invalid(error.to_string()))
        }
        METHOD_DETACH => {
            let params: TerminalIdParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            if let Some(handle) = subscriptions.remove(&params.terminal_id) {
                handle.abort();
                pty.note_view_closed(&params.terminal_id);
            } else {
                pty.pause_tracking_if_unwatched(&params.terminal_id);
            }
            Ok(Value::Null)
        }
        METHOD_WRITE => {
            let params: WriteParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            pty.write(&params.terminal_id, &params.data)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_RESIZE => {
            let params: ResizeParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let cell = params.cell_width.zip(params.cell_height);
            pty.resize(&params.terminal_id, params.cols, params.rows, cell)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_OBSERVE_CONVERSATION => {
            let params: ConversationTerminalParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let live_terminal_ids = if params.terminal_ids.is_empty() {
                pty.get_all()
                    .into_iter()
                    .filter(|instance| {
                        instance.workspace_ref_tracked
                            && instance.conversation_matches(params.conversation_id)
                            && instance.is_active()
                    })
                    .map(|instance| instance.id.clone())
                    .collect()
            } else {
                let mut live_terminal_ids = Vec::new();
                for terminal_id in &params.terminal_ids {
                    let Some(instance) = pty.get(terminal_id) else {
                        continue;
                    };
                    if !instance.workspace_ref_tracked
                        || !instance.conversation_matches(params.conversation_id)
                    {
                        return Err(unauthorized());
                    }
                    if instance.is_active() {
                        live_terminal_ids.push(terminal_id.clone());
                    }
                }
                live_terminal_ids
            };
            serde_json::to_value(TerminalConversationObservation {
                conversation_id: params.conversation_id,
                live_terminal_ids,
            })
            .map_err(|error| invalid(error.to_string()))
        }
        METHOD_TERMINATE_FOR_CONVERSATION => {
            let params: ConversationTerminalTerminateParams =
                serde_json::from_value(request.params.clone())
                    .map_err(|error| invalid(error.to_string()))?;
            let Some(instance) = pty.get(&params.terminal_id) else {
                return serde_json::to_value(TerminalConversationTermination {
                    conversation_id: params.conversation_id,
                    terminal_id: params.terminal_id,
                    operation_id: params.operation_id,
                    outcome: TerminalTerminationOutcome::AlreadyGone,
                })
                .map_err(|error| invalid(error.to_string()));
            };
            if !instance.workspace_ref_tracked
                || !instance.conversation_matches(params.conversation_id)
            {
                return Err(unauthorized());
            }
            if !instance.is_active() {
                return serde_json::to_value(TerminalConversationTermination {
                    conversation_id: params.conversation_id,
                    terminal_id: params.terminal_id,
                    operation_id: params.operation_id,
                    outcome: TerminalTerminationOutcome::AlreadyGone,
                })
                .map_err(|error| invalid(error.to_string()));
            }
            pty.terminate(&params.terminal_id)
                .await
                .map_err(|error| invalid(error.to_string()))?;
            serde_json::to_value(TerminalConversationTermination {
                conversation_id: params.conversation_id,
                terminal_id: params.terminal_id,
                operation_id: params.operation_id,
                outcome: TerminalTerminationOutcome::Terminated,
            })
            .map_err(|error| invalid(error.to_string()))
        }
        METHOD_TERMINATE => {
            let params: TerminalIdParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            if let Some(handle) = subscriptions.remove(&params.terminal_id) {
                handle.abort();
                pty.note_view_closed(&params.terminal_id);
            }
            pty.terminate(&params.terminal_id)
                .await
                .map(|_| ())
                .map_err(|error| invalid(error.to_string()))?;
            Ok(Value::Null)
        }
        METHOD_ROTATE => {
            let params: ClaimParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let claim = pty
                .rotate_claim(&params.terminal_id, &params.claim)
                .map_err(|_| unauthorized())?;
            serde_json::to_value(RotatedClaim { claim }).map_err(|error| invalid(error.to_string()))
        }
        METHOD_REVOKE => {
            let params: ClaimParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            pty.revoke_claim(&params.terminal_id, &params.claim)
                .map_err(|_| unauthorized())?;
            Ok(Value::Null)
        }
        METHOD_RESUME => {
            let params: TerminalResumeRequest = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let (grant, _replay) = match params.conversation_id {
                Some(conversation_id) => pty.resume_for_conversation(
                    conversation_id,
                    &params.terminal_id,
                    params.last_seq,
                ),
                None => pty.resume_project_terminal(&params.terminal_id, params.last_seq),
            }
            .map_err(|_| unauthorized())?;
            serde_json::to_value(&grant).map_err(|error| invalid(error.to_string()))
        }
        METHOD_CLOSE_VIEW => {
            let params: TerminalIdParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            if let Some(handle) = subscriptions.remove(&params.terminal_id) {
                handle.abort();
                pty.note_view_closed(&params.terminal_id);
            } else {
                pty.pause_tracking_if_unwatched(&params.terminal_id);
            }
            Ok(Value::Null)
        }
        METHOD_SET_DISPLAY_MODE => {
            let params: DisplayModeParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let mode = TerminalDisplayMode::parse(&params.mode).map_err(invalid)?;
            let owner = params.owner.as_deref().unwrap_or("desktop-host");
            let state_value = pty
                .set_display_mode(
                    &params.terminal_id,
                    mode,
                    params.cols,
                    params.rows,
                    owner,
                    params.force,
                )
                .await
                .map_err(invalid)?;
            serde_json::to_value(&state_value).map_err(|error| invalid(error.to_string()))
        }
        METHOD_SET_PROTECTED => {
            let params: ProtectedParams = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            pty.set_protected(&params.terminal_id, params.protected);
            Ok(Value::Null)
        }
        METHOD_SHUTDOWN => {
            pty.kill_all().await;
            Ok(Value::Null)
        }
        other => Err(invalid(format!(
            "terminal method '{other}' is not exported over core IPC"
        ))),
    }
}

fn map_spawn_error(error: String) -> CoreError {
    if error.ends_with("scope is unauthorized") {
        unauthorized()
    } else {
        invalid(error)
    }
}

fn stream_should_stop(captured_generation: Option<u64>, current_generation: Option<u64>) -> bool {
    captured_generation != current_generation
}

const STREAM_OUTPUT_CAPACITY: usize = 256;

/// Lagged is a hole in seq space. Continuing would silently skip bytes;
/// fail the stream (emit `terminal.gap`) and let reconnect/rewatch heal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveOutputDecision {
    Forward { seq: u64, data: Vec<u8> },
    FailGap { last_seq: u64 },
    Stop,
}

pub fn decide_live_output(
    received: Result<se_pty::manager::TerminalOutputChunk, broadcast::error::RecvError>,
    current_seq: u64,
) -> LiveOutputDecision {
    match received {
        Ok(chunk) => LiveOutputDecision::Forward {
            seq: chunk.seq,
            data: chunk.data,
        },
        Err(broadcast::error::RecvError::Lagged(_)) => LiveOutputDecision::FailGap {
            last_seq: current_seq,
        },
        Err(broadcast::error::RecvError::Closed) => LiveOutputDecision::Stop,
    }
}

fn requested_stream_last_seq(params: &Value) -> u64 {
    params.get("lastSeq").and_then(Value::as_u64).unwrap_or(0)
}

/// Delivered ahead of a replay whose cursor was evicted (RIS), so the replay
/// paints a blank terminal instead of the screen the gap left behind.
const RESYNC_RESET: &[u8] = b"\x1bc";
/// Resync attempts in a row that deliver nothing before the slot fails closed.
const MAX_STALLED_RESYNCS: u32 = 3;
static NEXT_RESYNC_ID: AtomicU64 = AtomicU64::new(1);

/// A slot whose consumer fell behind. Removing it left the terminal frozen
/// for good, so it is rebuilt from the Core's output log instead: frames of
/// the overrun subscription are dropped while a task re-watches from
/// `last_seq`, and delivery resumes with the new subscription's replay.
struct StreamResync {
    id: u64,
    /// Kind and seq of the last frame dropped from the overrun subscription;
    /// `None` once it can send nothing more.
    dropped: Option<(OutputKind, u64)>,
    /// `(gap, latest_seq)` from the re-watch reply.
    reply: Option<(bool, u64)>,
    /// New-subscription frames that arrived before the reply said whether a
    /// reset goes in front of them.
    parked: Vec<OutputFrame>,
    /// Attempts in a row that delivered nothing.
    stalled: u32,
}

impl StreamResync {
    /// Seqs only grow within a subscription and its replay comes before its
    /// live output. So after a dropped live frame, the next replay frame is
    /// the new subscription's. An old Core sends one replay frame per chunk;
    /// after one of those, a replay frame is new when it restarts at or below
    /// the dropped seq instead of continuing past it.
    fn starts_new_subscription(&self, frame: &OutputFrame) -> bool {
        match self.dropped {
            None => true,
            Some((OutputKind::Replay, seq)) => frame.kind == OutputKind::Replay && frame.seq <= seq,
            Some(_) => frame.kind == OutputKind::Replay,
        }
    }
}

/// Hand-off from the read loop to the task that re-watches a slot.
struct ResyncStart {
    terminal_id: String,
    id: u64,
    cause: &'static str,
    seq: u64,
    sender: mpsc::Sender<OutputFrame>,
}

enum FrameRoute {
    Done,
    Resync(ResyncStart),
    /// The slot was removed: its consumer is gone or resync gave up.
    Closed {
        terminal_id: String,
        seq: u64,
        stable_code: &'static str,
    },
}

fn deliver_client_output_frame(
    streams: &mut HashMap<String, StreamSlot>,
    frame: OutputFrame,
) -> FrameRoute {
    let Some(slot) = streams.get_mut(&frame.terminal_id) else {
        return FrameRoute::Done;
    };
    let route = route_client_output_frame(slot, frame);
    remove_closed_slot(streams, &route);
    route
}

fn route_client_output_frame(slot: &mut StreamSlot, frame: OutputFrame) -> FrameRoute {
    let Some(resync) = slot.resync.as_mut() else {
        return match push_client_output_frame(slot, frame) {
            Ok(()) => FrameRoute::Done,
            Err(error) => overrun_route(slot, error, None, 0),
        };
    };
    if !resync.starts_new_subscription(&frame) {
        // Live output past the reply's latest seq is the new subscription's,
        // so its replay was missed: watch again.
        if resync
            .reply
            .is_some_and(|(_, latest)| frame.kind == OutputKind::Live && frame.seq > latest)
        {
            let stalled = resync.stalled + 1;
            return start_resync(
                slot,
                frame.terminal_id,
                Some((frame.kind, frame.seq)),
                stalled,
                "STREAM_RESYNC_MISSED",
                frame.seq,
            );
        }
        resync.dropped = Some((frame.kind, frame.seq));
        return FrameRoute::Done;
    }
    if resync.reply.is_none() {
        resync.parked.push(frame);
        return FrameRoute::Done;
    }
    finish_resync(slot, vec![frame])
}

fn push_client_output_frame(
    slot: &mut StreamSlot,
    frame: OutputFrame,
) -> Result<(), mpsc::error::TrySendError<OutputFrame>> {
    let seq = frame.seq;
    slot.sender.try_send(frame)?;
    // What this consumer has, even below a requested cursor that was ahead
    // of the log (a live hand-off): a resync replays from here.
    slot.last_seq = seq;
    Ok(())
}

fn overrun_route(
    slot: &mut StreamSlot,
    error: mpsc::error::TrySendError<OutputFrame>,
    dropped: Option<(OutputKind, u64)>,
    stalled: u32,
) -> FrameRoute {
    match error {
        mpsc::error::TrySendError::Full(frame) => {
            let dropped = dropped.unwrap_or((frame.kind, frame.seq));
            start_resync(
                slot,
                frame.terminal_id,
                Some(dropped),
                stalled,
                "STREAM_SLOT_FULL",
                frame.seq,
            )
        }
        mpsc::error::TrySendError::Closed(frame) => FrameRoute::Closed {
            terminal_id: frame.terminal_id,
            seq: frame.seq,
            stable_code: "STREAM_SLOT_CLOSED",
        },
    }
}

fn start_resync(
    slot: &mut StreamSlot,
    terminal_id: String,
    dropped: Option<(OutputKind, u64)>,
    stalled: u32,
    cause: &'static str,
    seq: u64,
) -> FrameRoute {
    if stalled >= MAX_STALLED_RESYNCS {
        return FrameRoute::Closed {
            terminal_id,
            seq,
            stable_code: "STREAM_RESYNC_FAILED",
        };
    }
    let id = NEXT_RESYNC_ID.fetch_add(1, Ordering::Relaxed);
    slot.resync = Some(StreamResync {
        id,
        dropped,
        reply: None,
        parked: Vec::new(),
        stalled,
    });
    FrameRoute::Resync(ResyncStart {
        terminal_id,
        id,
        cause,
        seq,
        sender: slot.sender.clone(),
    })
}

/// Deliver the new subscription's first frames, behind a reset when its
/// cursor was evicted, and leave resync.
fn finish_resync(slot: &mut StreamSlot, frames: Vec<OutputFrame>) -> FrameRoute {
    let Some(resync) = slot.resync.take() else {
        return FrameRoute::Done;
    };
    let Some(first) = frames.first() else {
        return FrameRoute::Done;
    };
    let terminal_id = first.terminal_id.clone();
    let dropped = frames.last().map(|frame| (frame.kind, frame.seq));
    let gap = resync.reply.is_some_and(|(gap, _)| gap);
    let reset = gap.then(|| OutputFrame {
        kind: OutputKind::Replay,
        terminal_id: terminal_id.clone(),
        seq: slot.last_seq,
        data: RESYNC_RESET.to_vec(),
    });
    let mut delivered = false;
    for frame in reset.into_iter().chain(frames) {
        if let Err(error) = push_client_output_frame(slot, frame) {
            let stalled = if delivered { 0 } else { resync.stalled + 1 };
            return overrun_route(slot, error, dropped, stalled);
        }
        delivered = true;
    }
    log::info!(
        target: "se_manager::core",
        "operation=terminal_output_resync stable_code=STREAM_RESYNC_DONE terminal_id={} last_seq={} gap={}",
        terminal_id,
        slot.last_seq,
        gap
    );
    FrameRoute::Done
}

/// Record the re-watch reply; frames that beat it here are delivered now.
fn apply_resync_reply(
    slot: &mut StreamSlot,
    last_seq: u64,
    gap: bool,
    latest_seq: u64,
) -> FrameRoute {
    let Some(resync) = slot.resync.as_mut() else {
        return FrameRoute::Done;
    };
    if latest_seq <= last_seq {
        // Nothing to replay, and nothing newer the overrun subscription
        // could still send: whatever comes next is the new subscription's.
        resync.dropped = None;
    }
    resync.reply = Some((gap, latest_seq));
    if resync.parked.is_empty() {
        return FrameRoute::Done;
    }
    let parked = std::mem::take(&mut resync.parked);
    finish_resync(slot, parked)
}

/// The Core ended a subscription whose broadcast lagged; it sends nothing
/// more, so its slot is rebuilt like an overrun one.
fn resync_after_gap(
    streams: &mut HashMap<String, StreamSlot>,
    terminal_id: &str,
    seq: u64,
) -> FrameRoute {
    let Some(slot) = streams.get_mut(terminal_id) else {
        return FrameRoute::Done;
    };
    let stalled = match slot.resync.as_mut() {
        None => 0,
        // The overrun subscription ended; the running task re-watches.
        Some(resync) if resync.reply.is_none() && resync.parked.is_empty() => {
            resync.dropped = None;
            return FrameRoute::Done;
        }
        // The re-watched subscription itself ended.
        Some(resync) => resync.stalled + 1,
    };
    let route = start_resync(
        slot,
        terminal_id.to_string(),
        None,
        stalled,
        "STREAM_GAP",
        seq,
    );
    remove_closed_slot(streams, &route);
    route
}

fn remove_closed_slot(streams: &mut HashMap<String, StreamSlot>, route: &FrameRoute) {
    if let FrameRoute::Closed { terminal_id, .. } = route {
        streams.remove(terminal_id);
    }
}

fn resyncing_slot<'a>(
    streams: &'a mut HashMap<String, StreamSlot>,
    terminal_id: &str,
    id: u64,
) -> Option<&'a mut StreamSlot> {
    streams
        .get_mut(terminal_id)
        .filter(|slot| slot.resync.as_ref().is_some_and(|resync| resync.id == id))
}

fn follow_frame_route(inner: &Arc<ClientInner>, route: FrameRoute) {
    match route {
        FrameRoute::Done => {}
        FrameRoute::Resync(start) => {
            log::warn!(
                target: "se_manager::core",
                "operation=terminal_output_backpressure stable_code={} terminal_id={} seq={}",
                start.cause,
                start.terminal_id,
                start.seq
            );
            tokio::spawn(run_stream_resync(Arc::downgrade(inner), start));
        }
        FrameRoute::Closed {
            terminal_id,
            seq,
            stable_code,
        } => {
            log::warn!(
                target: "se_manager::core",
                "operation=terminal_output_backpressure stable_code={} terminal_id={} seq={}",
                stable_code,
                terminal_id,
                seq
            );
        }
    }
}

/// Never runs on the read loop: the watch reply arrives there.
async fn run_stream_resync(inner: std::sync::Weak<ClientInner>, mut start: ResyncStart) {
    loop {
        let next = match resync_stream_once(&inner, start).await {
            FrameRoute::Resync(next) => next,
            FrameRoute::Done => return,
            route => {
                if let Some(inner) = inner.upgrade() {
                    follow_frame_route(&inner, route);
                }
                return;
            }
        };
        start = next;
    }
}

async fn resync_stream_once(
    inner: &std::sync::Weak<ClientInner>,
    start: ResyncStart,
) -> FrameRoute {
    let ResyncStart {
        terminal_id,
        id,
        cause,
        sender,
        ..
    } = start;
    // Wait until the consumer drained the slot so the replay fits; an old
    // Core sends it one frame per chunk.
    let drained = sender.reserve_many(STREAM_OUTPUT_CAPACITY).await.is_ok();
    drop(sender);
    let Some(inner) = inner.upgrade() else {
        return FrameRoute::Done;
    };
    let last_seq = {
        let mut streams = inner.streams.lock();
        let Some(slot) = resyncing_slot(&mut streams, &terminal_id, id) else {
            return FrameRoute::Done;
        };
        let last_seq = slot.last_seq;
        if !drained {
            streams.remove(&terminal_id);
            return FrameRoute::Done;
        }
        last_seq
    };
    log::info!(
        target: "se_manager::core",
        "operation=terminal_output_resync stable_code=STREAM_RESYNC_START cause={} terminal_id={} last_seq={}",
        cause,
        terminal_id,
        last_seq
    );
    let reply = client_rpc(
        &inner,
        METHOD_WATCH,
        json!({ "terminalId": terminal_id, "lastSeq": last_seq }),
    )
    .await
    .and_then(|value| {
        serde_json::from_value::<AttachReply>(value).map_err(|error| invalid(error.to_string()))
    });
    let mut streams = inner.streams.lock();
    let Some(slot) = resyncing_slot(&mut streams, &terminal_id, id) else {
        return FrameRoute::Done;
    };
    match reply {
        Ok(reply) => {
            let route = apply_resync_reply(slot, last_seq, reply.gap, reply.latest_seq);
            remove_closed_slot(&mut streams, &route);
            route
        }
        Err(error) => {
            streams.remove(&terminal_id);
            drop(streams);
            log::warn!(
                target: "se_manager::core",
                "operation=terminal_output_resync stable_code=STREAM_RESYNC_FAILED terminal_id={} last_seq={} error={}",
                terminal_id,
                last_seq,
                error.code()
            );
            FrameRoute::Done
        }
    }
}

async fn begin_output_stream(
    pty: &Arc<PtyManager>,
    writer: &Arc<tokio::sync::Mutex<CoreWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    terminal_id: &str,
    claim: Option<&str>,
    last_seq: u64,
) -> Result<AttachReply, CoreError> {
    let generation = if let Some(claim) = claim {
        let generation = pty.claim_generation(terminal_id).ok_or_else(unauthorized)?;
        if pty.verify_claim(terminal_id, claim).is_err() {
            return Err(unauthorized());
        }
        if pty.claim_generation(terminal_id) != Some(generation) {
            return Err(unauthorized());
        }
        Some(generation)
    } else {
        pty.get(terminal_id)
            .filter(|instance| instance.is_active())
            .ok_or_else(|| invalid("terminal not found"))?;
        pty.claim_generation(terminal_id)
    };
    let instance = pty.get(terminal_id).ok_or_else(|| {
        if claim.is_some() {
            unauthorized()
        } else {
            invalid("terminal not found")
        }
    })?;
    if claim.is_some() && !instance.is_active() {
        return Err(unauthorized());
    }
    let replay = instance.subscribe_from(last_seq);
    let result = pty.build_attach_result(&instance, &replay);
    let reply = AttachReply::from_parts(result, generation, instance.conversation_id);
    start_replay_forwarder(
        pty,
        writer,
        subscriptions,
        terminal_id,
        replay,
        reply.latest_seq,
        generation,
    )
    .await;
    Ok(reply)
}

/// Largest replay frame. Well under the IPC frame limit.
const REPLAY_FRAME_BYTES: usize = 64 * 1024;

/// Merge retained chunks into few large frames, each carrying the seq of its
/// last chunk. A TUI's history is hundreds of tiny chunks. Sent one frame
/// each, a full replay overran the client's output slot before the caller
/// began draining it, which dropped the rest of that terminal's output.
fn coalesce_replay(chunks: &[se_pty::manager::TerminalOutputChunk]) -> Vec<(u64, Vec<u8>)> {
    let mut frames: Vec<(u64, Vec<u8>)> = Vec::new();
    for chunk in chunks {
        match frames.last_mut() {
            Some((seq, data)) if data.len() + chunk.data.len() <= REPLAY_FRAME_BYTES => {
                *seq = chunk.seq;
                data.extend_from_slice(&chunk.data);
            }
            _ => frames.push((chunk.seq, chunk.data.clone())),
        }
    }
    frames
}

async fn start_replay_forwarder(
    pty: &Arc<PtyManager>,
    writer: &Arc<tokio::sync::Mutex<CoreWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    terminal_id: &str,
    replay: se_pty::manager::TerminalReplay,
    latest_seq: u64,
    generation: Option<u64>,
) {
    let replacing = if let Some(previous) = subscriptions.remove(terminal_id) {
        previous.abort();
        true
    } else {
        false
    };
    if !replacing {
        pty.note_view_opened(terminal_id);
    }

    let writer = Arc::clone(writer);
    let pty = Arc::clone(pty);
    let attached_id = terminal_id.to_string();
    let handle = tokio::spawn(async move {
        for (seq, data) in coalesce_replay(&replay.chunks) {
            let Ok(frame) = encode_output_frame(OutputKind::Replay, &attached_id, seq, &data)
            else {
                return;
            };
            let mut guard = writer.lock().await;
            if write_frame(&mut *guard, &frame).await.is_err() {
                return;
            }
        }

        let mut receiver = replay.receiver;
        let mut current_seq = latest_seq;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            if stream_should_stop(generation, pty.claim_generation(&attached_id)) {
                break;
            }
            tokio::select! {
                received = receiver.recv() => {
                    match received {
                        Ok(chunk) => {
                            if stream_should_stop(
                                generation,
                                pty.claim_generation(&attached_id),
                            ) {
                                break;
                            }
                            current_seq = chunk.seq;
                            let Ok(frame) = encode_output_frame(
                                OutputKind::Live,
                                &attached_id,
                                chunk.seq,
                                &chunk.data,
                            ) else {
                                break;
                            };
                            let mut guard = writer.lock().await;
                            if write_frame(&mut *guard, &frame).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            match decide_live_output(
                                Err(broadcast::error::RecvError::Lagged(0)),
                                current_seq,
                            ) {
                                LiveOutputDecision::FailGap { last_seq } => {
                                    log::warn!(
                                        target: "se_manager::core",
                                        "operation=terminal_output_lag stable_code=STREAM_GAP terminal_id={} last_seq={}",
                                        attached_id,
                                        last_seq
                                    );
                                    let message = CoreEvent {
                                        topic: GAP_TOPIC.to_string(),
                                        payload: json!(GapPayload {
                                            terminal_id: attached_id.clone(),
                                            last_seq,
                                        }),
                                    };
                                    let mut guard = writer.lock().await;
                                    let _ = write_json_frame(&mut *guard, &message).await;
                                    break;
                                }
                                _ => break,
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tick.tick() => {}
            }
        }
    });
    subscriptions.insert(terminal_id.to_string(), handle);
}

pub struct TerminalAttachSession {
    pub result: TerminalAttachResult,
    pub claim_generation: Option<u64>,
    pub conversation_id: String,
    pub output: mpsc::Receiver<OutputFrame>,
}

struct StreamSlot {
    sender: mpsc::Sender<OutputFrame>,
    last_seq: u64,
    resync: Option<StreamResync>,
}

struct ClientInner {
    writer: tokio::sync::Mutex<CoreWriteHalf>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<CoreResponse>>>,
    streams: Mutex<HashMap<String, StreamSlot>>,
    live: Mutex<HashSet<String>>,
    events: broadcast::Sender<TerminalEvent>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reconnect: tokio::sync::Mutex<()>,
    connected: AtomicBool,
}

#[derive(Clone)]
pub struct TerminalCoreClient {
    inner: Arc<ClientInner>,
}

impl TerminalCoreClient {
    pub async fn connect(endpoint: &CoreEndpoint) -> Result<Self, CoreError> {
        let (reader, writer) = open_terminal_stream(endpoint).await?;
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(ClientInner {
            writer: tokio::sync::Mutex::new(writer),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            live: Mutex::new(HashSet::new()),
            events,
            reader: Mutex::new(None),
            reconnect: tokio::sync::Mutex::new(()),
            connected: AtomicBool::new(true),
        });
        spawn_terminal_read_loop(&inner, reader);
        let client = Self { inner };
        if let Ok(list) = client.list().await {
            let mut live = client.inner.live.lock();
            live.clear();
            for status in list {
                if status.active {
                    live.insert(status.id);
                }
            }
        }
        Ok(client)
    }

    /// Re-handshake against `endpoint` and swap the live stream. The event
    /// broadcast stays on the same inner Arc so the GUI event-hub mirror keeps
    /// receiving after a Core restart. After the socket swap we refresh `live`,
    /// drop stream senders whose terminal is gone, and re-watch remaining ids
    /// from each stream's last known seq so output resumes.
    pub async fn reconnect(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        let _guard = self.inner.reconnect.lock().await;
        let (reader, writer) = open_terminal_stream(endpoint).await?;
        {
            let mut writer_guard = self.inner.writer.lock().await;
            if let Some(handle) = self.inner.reader.lock().take() {
                handle.abort();
            }
            fail_stale_pending(&self.inner);
            *writer_guard = writer;
        }
        spawn_terminal_read_loop(&self.inner, reader);
        self.inner.connected.store(true, Ordering::Release);

        let listed = self.list().await.unwrap_or_default();
        let active: HashSet<String> = listed
            .into_iter()
            .filter(|status| status.active)
            .map(|status| status.id)
            .collect();
        {
            let mut live = self.inner.live.lock();
            live.clear();
            live.extend(active.iter().cloned());
        }
        let mut to_rewatch = Vec::new();
        {
            let mut streams = self.inner.streams.lock();
            streams.retain(|id, slot| {
                if active.contains(id) {
                    // A resync in flight watched on the old connection; the
                    // re-watch below replaces it.
                    slot.resync = None;
                    to_rewatch.push((id.clone(), slot.last_seq));
                    true
                } else {
                    false
                }
            });
        }
        for (terminal_id, last_seq) in to_rewatch {
            let _ = self
                .rpc(
                    METHOD_WATCH,
                    json!({ "terminalId": terminal_id, "lastSeq": last_seq }),
                )
                .await;
        }
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<TerminalEvent> {
        self.inner.events.subscribe()
    }

    pub async fn spawn(&self, options: SpawnOptions) -> Result<SpawnedTerminal, CoreError> {
        let value = self
            .rpc(
                METHOD_SPAWN,
                serde_json::to_value(options).map_err(|error| invalid(error.to_string()))?,
            )
            .await?;
        let spawned: SpawnedTerminal =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        self.inner.live.lock().insert(spawned.info.id.clone());
        Ok(spawned)
    }

    pub async fn list(&self) -> Result<Vec<TerminalStatus>, CoreError> {
        let value = self.rpc(METHOD_LIST, Value::Null).await?;
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    pub async fn status(&self, terminal_id: &str) -> Result<TerminalStatus, CoreError> {
        let value = self
            .rpc(METHOD_STATUS, json!({ "terminalId": terminal_id }))
            .await?;
        let status: TerminalStatus =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        if status.active {
            self.inner.live.lock().insert(status.id.clone());
        } else {
            self.inner.live.lock().remove(&status.id);
        }
        Ok(status)
    }

    pub async fn attach(
        &self,
        terminal_id: &str,
        claim: &str,
        last_seq: u64,
    ) -> Result<TerminalAttachSession, CoreError> {
        self.open_stream(
            METHOD_ATTACH,
            json!({
                "terminalId": terminal_id,
                "claim": claim,
                "lastSeq": last_seq,
            }),
            terminal_id,
        )
        .await
    }

    pub async fn watch(
        &self,
        terminal_id: &str,
        last_seq: u64,
    ) -> Result<TerminalAttachSession, CoreError> {
        self.open_stream(
            METHOD_WATCH,
            json!({
                "terminalId": terminal_id,
                "lastSeq": last_seq,
            }),
            terminal_id,
        )
        .await
    }

    pub async fn detach(&self, terminal_id: &str) -> Result<(), CoreError> {
        self.inner.streams.lock().remove(terminal_id);
        self.rpc(METHOD_DETACH, json!({ "terminalId": terminal_id }))
            .await
            .map(|_| ())
    }

    pub async fn rotate(&self, terminal_id: &str, claim: &str) -> Result<String, CoreError> {
        let value = self
            .rpc(
                METHOD_ROTATE,
                json!({ "terminalId": terminal_id, "claim": claim }),
            )
            .await?;
        let rotated: RotatedClaim =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        Ok(rotated.claim)
    }

    pub async fn revoke(&self, terminal_id: &str, claim: &str) -> Result<(), CoreError> {
        self.rpc(
            METHOD_REVOKE,
            json!({ "terminalId": terminal_id, "claim": claim }),
        )
        .await
        .map(|_| ())
    }

    pub async fn resume(
        &self,
        request: TerminalResumeRequest,
    ) -> Result<TerminalResumeGrant, CoreError> {
        let value = self
            .rpc(
                METHOD_RESUME,
                serde_json::to_value(&request).map_err(|error| invalid(error.to_string()))?,
            )
            .await?;
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    pub async fn close_view(&self, terminal_id: &str) -> Result<(), CoreError> {
        self.inner.streams.lock().remove(terminal_id);
        self.rpc(METHOD_CLOSE_VIEW, json!({ "terminalId": terminal_id }))
            .await
            .map(|_| ())
    }

    pub async fn set_protected(&self, terminal_id: &str, protected: bool) -> Result<(), CoreError> {
        self.rpc(
            METHOD_SET_PROTECTED,
            json!({ "terminalId": terminal_id, "protected": protected }),
        )
        .await
        .map(|_| ())
    }

    pub async fn set_display_mode(
        &self,
        terminal_id: &str,
        mode: &str,
        cols: Option<u16>,
        rows: Option<u16>,
        force: bool,
    ) -> Result<se_pty::manager::DisplayModeState, CoreError> {
        let value = self
            .rpc(
                METHOD_SET_DISPLAY_MODE,
                json!({
                    "terminalId": terminal_id,
                    "mode": mode,
                    "cols": cols,
                    "rows": rows,
                    "force": force,
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    pub async fn shutdown(&self) -> Result<(), CoreError> {
        self.rpc(METHOD_SHUTDOWN, Value::Null).await.map(|_| ())
    }

    pub async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError> {
        self.rpc(
            METHOD_WRITE,
            json!({ "terminalId": terminal_id, "data": data }),
        )
        .await
        .map(|_| ())
    }

    pub async fn resize(
        &self,
        terminal_id: &str,
        cols: u16,
        rows: u16,
        cell: Option<(u16, u16)>,
    ) -> Result<(), CoreError> {
        self.rpc(METHOD_RESIZE, resize_params(terminal_id, cols, rows, cell))
            .await
            .map(|_| ())
    }

    pub async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError> {
        let result = self
            .rpc(METHOD_TERMINATE, json!({ "terminalId": terminal_id }))
            .await
            .map(|_| ());
        self.inner.live.lock().remove(terminal_id);
        self.inner.streams.lock().remove(terminal_id);
        result
    }

    pub async fn observe_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<TerminalConversationObservation, CoreError> {
        let value = self
            .rpc(
                METHOD_OBSERVE_CONVERSATION,
                json!({
                    "conversationId": conversation_id,
                    "terminalIds": terminal_ids,
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    pub async fn terminate_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        operation_id: &str,
    ) -> Result<TerminalConversationTermination, CoreError> {
        let value = self
            .rpc(
                METHOD_TERMINATE_FOR_CONVERSATION,
                json!({
                    "conversationId": conversation_id,
                    "terminalId": terminal_id,
                    "operationId": operation_id,
                }),
            )
            .await?;
        let result: TerminalConversationTermination =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        if matches!(
            result.outcome,
            TerminalTerminationOutcome::Terminated | TerminalTerminationOutcome::AlreadyGone
        ) {
            self.inner.live.lock().remove(terminal_id);
        }
        Ok(result)
    }

    async fn open_stream(
        &self,
        method: &str,
        params: Value,
        terminal_id: &str,
    ) -> Result<TerminalAttachSession, CoreError> {
        let last_seq = requested_stream_last_seq(&params);
        let (tx, rx) = mpsc::channel(STREAM_OUTPUT_CAPACITY);
        self.inner.streams.lock().insert(
            terminal_id.to_string(),
            StreamSlot {
                sender: tx,
                last_seq,
                resync: None,
            },
        );
        let value = match self.rpc(method, params).await {
            Ok(value) => value,
            Err(error) => {
                self.inner.streams.lock().remove(terminal_id);
                return Err(error);
            }
        };
        let reply: AttachReply =
            serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
        self.inner.live.lock().insert(reply.id.clone());
        Ok(TerminalAttachSession {
            result: reply.clone().into_result(),
            claim_generation: reply.claim_generation,
            conversation_id: reply.conversation_id,
            output: rx,
        })
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, CoreError> {
        client_rpc(&self.inner, method, params).await
    }

    /// Raw request for services Terminal Core hosts beside its PTYs
    /// (quick terminals); their replies carry their own error codes.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, CoreError> {
        self.rpc(method, params).await
    }
}

async fn client_rpc(
    inner: &Arc<ClientInner>,
    method: &str,
    params: Value,
) -> Result<Value, CoreError> {
    let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    inner.pending.lock().insert(id, tx);
    let request = CoreRequest {
        id,
        method: method.to_string(),
        params,
    };
    {
        let mut writer = inner.writer.lock().await;
        if let Err(error) = write_json_frame(&mut *writer, &request).await {
            inner.pending.lock().remove(&id);
            return Err(error);
        }
    }
    let response = tokio::time::timeout(RPC_TIMEOUT, rx)
        .await
        .map_err(|_| CoreError::Io("core RPC timed out".into()))?
        .map_err(|_| CoreError::Io("core RPC cancelled".into()))?;
    if let Some(error) = response.error {
        if error.code == CoreError::Unauthorized.code() {
            return Err(CoreError::Unauthorized);
        }
        return Err(CoreError::InvalidRequest(error.message));
    }
    Ok(response.result.unwrap_or(Value::Null))
}

async fn open_terminal_stream(
    endpoint: &CoreEndpoint,
) -> Result<(CoreReadHalf, CoreWriteHalf), CoreError> {
    let mut stream = connect_core(endpoint).await?;
    let hello = CoreHello {
        role: CoreRole::TerminalCore,
        protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
        client_name: "termul-gui".to_string(),
    };
    write_json_frame(&mut stream, &hello).await?;
    let ack: super::ipc::CoreHelloAck = read_json_frame(&mut stream).await?;
    if ack.role != CoreRole::TerminalCore || ack.protocol_version != CURRENT_PROTOCOL_VERSION {
        return Err(CoreError::InvalidHandshake(
            "core returned an incompatible handshake".into(),
        ));
    }
    Ok(stream.into_split())
}

fn spawn_terminal_read_loop(inner: &Arc<ClientInner>, reader: CoreReadHalf) {
    let reader_inner = Arc::clone(inner);
    let handle = tokio::spawn(async move {
        client_read_loop(reader, reader_inner).await;
    });
    *inner.reader.lock() = Some(handle);
}

fn fail_stale_pending(inner: &ClientInner) {
    let pending: Vec<_> = inner.pending.lock().drain().map(|(_, tx)| tx).collect();
    for tx in pending {
        let _ = tx.send(CoreResponse {
            id: 0,
            result: None,
            error: Some(CoreErrorPayload {
                code: CoreError::Io(String::new()).code().to_string(),
                message: CoreError::Io(String::new()).client_message().to_string(),
            }),
        });
    }
}

async fn client_read_loop(mut reader: CoreReadHalf, inner: Arc<ClientInner>) {
    loop {
        let payload = match read_frame(&mut reader).await {
            Ok(payload) => payload,
            Err(_) => break,
        };
        if is_output_frame(&payload) {
            if let Ok(frame) = decode_output_frame(&payload) {
                let route = {
                    let mut streams = inner.streams.lock();
                    deliver_client_output_frame(&mut streams, frame)
                };
                follow_frame_route(&inner, route);
            }
            continue;
        }
        if let Ok(response) = serde_json::from_slice::<CoreResponse>(&payload) {
            if let Some(tx) = inner.pending.lock().remove(&response.id) {
                let _ = tx.send(response);
            }
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<CoreEvent>(&payload) {
            if event.topic == GAP_TOPIC {
                if let Ok(gap) = serde_json::from_value::<GapPayload>(event.payload) {
                    // A lagged stream has lost data and ended. Ending the
                    // consumer's stream too froze the terminal; rebuild it
                    // from the Core's log instead, which loses nothing.
                    let route =
                        resync_after_gap(&mut inner.streams.lock(), &gap.terminal_id, gap.last_seq);
                    follow_frame_route(&inner, route);
                }
                continue;
            }
            if event.topic == EVENT_TOPIC {
                if let Ok(terminal_event) = serde_json::from_value::<TerminalEvent>(event.payload) {
                    match &terminal_event {
                        TerminalEvent::Spawned { terminal_id, .. } => {
                            inner.live.lock().insert(terminal_id.clone());
                        }
                        TerminalEvent::Exit { terminal_id, .. } => {
                            inner.live.lock().remove(terminal_id);
                        }
                        _ => {}
                    }
                    let _ = inner.events.send(terminal_event);
                }
            }
        }
    }
    fail_stale_pending(&inner);
    inner.connected.store(false, Ordering::Release);
    // A broken Core transport invalidates cached liveness. Keep stream slots
    // so the reconnect handshake can explicitly re-watch them from last_seq;
    // ordinary authorization still fails closed while the transport is down.
    inner.live.lock().clear();
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        if let Some(handle) = self.reader.lock().take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl TerminalRuntimeHandle for TerminalCoreClient {
    async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError> {
        TerminalCoreClient::write(self, terminal_id, data).await
    }

    async fn resize(
        &self,
        terminal_id: &str,
        cols: u16,
        rows: u16,
        cell: Option<(u16, u16)>,
    ) -> Result<(), CoreError> {
        TerminalCoreClient::resize(self, terminal_id, cols, rows, cell).await
    }

    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError> {
        TerminalCoreClient::terminate(self, terminal_id).await
    }

    async fn observe_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<TerminalConversationObservation, CoreError> {
        TerminalCoreClient::observe_conversation(self, conversation_id, terminal_ids).await
    }

    async fn terminate_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        operation_id: &str,
    ) -> Result<TerminalConversationTermination, CoreError> {
        TerminalCoreClient::terminate_for_conversation(
            self,
            conversation_id,
            terminal_id,
            operation_id,
        )
        .await
    }

    async fn spawn_trusted(&self, options: SpawnOptions) -> Result<String, CoreError> {
        let spawned = TerminalCoreClient::spawn(self, options).await?;
        Ok(spawned.info.id)
    }

    fn is_live(&self, terminal_id: &str) -> bool {
        self.inner.connected.load(Ordering::Acquire) && self.inner.live.lock().contains(terminal_id)
    }

    fn observes_live_terminals(&self) -> bool {
        self.inner.connected.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_pty::manager::SCROLLBACK_CAP;

    fn test_program() -> TerminalProgram {
        TerminalProgram {
            name: "se-test".to_string(),
            version: "0.0.0-test".to_string(),
        }
    }

    #[test]
    fn terminal_core_module_does_not_import_acp() {
        let source = include_str!("terminal.rs");
        let forbidden_import = ["crate::", "acp"].concat();
        assert!(
            !source.contains(&forbidden_import),
            "Terminal Core must not import ACP"
        );
    }

    #[test]
    fn resize_params_stay_compatible_across_gui_and_core_versions() {
        // Old GUI -> new Core: no cell fields at all.
        let old: ResizeParams =
            serde_json::from_value(json!({ "terminalId": "term-1", "cols": 80, "rows": 24 }))
                .expect("a request without cell size must still parse");
        assert_eq!((old.cols, old.rows), (80, 24));
        assert_eq!(old.cell_width.zip(old.cell_height), None);

        // New GUI without a measured cell sends exactly the old shape.
        assert_eq!(
            resize_params("term-1", 80, 24, None),
            json!({ "terminalId": "term-1", "cols": 80, "rows": 24 })
        );

        let new: ResizeParams =
            serde_json::from_value(resize_params("term-1", 80, 24, Some((8, 17)))).unwrap();
        assert_eq!(new.cell_width.zip(new.cell_height), Some((8, 17)));
    }

    #[test]
    fn round_trips_binary_output_frames_without_json() {
        let payload = encode_output_frame(OutputKind::Replay, "term-1", 9, b"hello").unwrap();
        assert!(payload.starts_with(OUTPUT_FRAME_MAGIC));
        assert!(!payload.contains(&b'{'));
        let decoded = decode_output_frame(&payload).unwrap();
        assert_eq!(decoded.kind, OutputKind::Replay);
        assert_eq!(decoded.terminal_id, "term-1");
        assert_eq!(decoded.seq, 9);
        assert_eq!(decoded.data, b"hello");
    }

    #[test]
    fn replay_merges_chunks_into_large_frames_that_keep_the_last_seq() {
        let chunk = |seq: u64, len: usize| se_pty::manager::TerminalOutputChunk {
            seq,
            data: vec![b'x'; len],
        };
        let chunks = vec![
            chunk(1, 10),
            chunk(2, 20),
            chunk(3, REPLAY_FRAME_BYTES),
            chunk(4, 5),
        ];
        let frames = coalesce_replay(&chunks);
        let shape: Vec<(u64, usize)> = frames
            .iter()
            .map(|(seq, data)| (*seq, data.len()))
            .collect();
        assert_eq!(shape, [(2, 30), (3, REPLAY_FRAME_BYTES), (4, 5)]);
    }

    #[test]
    fn scrollback_cap_is_256_kib() {
        assert_eq!(SCROLLBACK_CAP, 256 * 1024);
    }

    #[test]
    fn scoped_terminal_lifecycle_contract_uses_camel_case_and_idempotent_outcomes() {
        let conversation_id = ConversationId::new_v4();
        let termination = TerminalConversationTermination {
            conversation_id,
            terminal_id: "terminal-1".to_string(),
            operation_id: "op-1".to_string(),
            outcome: TerminalTerminationOutcome::AlreadyGone,
        };
        let value = serde_json::to_value(&termination).unwrap();
        assert_eq!(value["conversationId"], conversation_id.to_string());
        assert_eq!(value["terminalId"], "terminal-1");
        assert_eq!(value["operationId"], "op-1");
        assert_eq!(value["outcome"], "alreadyGone");
    }

    #[cfg(unix)]
    async fn wait_for_client(endpoint: &CoreEndpoint) -> TerminalCoreClient {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match TerminalCoreClient::connect(endpoint).await {
                Ok(client) => return client,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("terminal core did not become ready: {error}"),
            }
        }
    }

    #[cfg(unix)]
    async fn collect_until_marker(
        output: &mut mpsc::Receiver<OutputFrame>,
        marker: &str,
        timeout: std::time::Duration,
    ) -> String {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut collected = String::new();
        loop {
            if collected.contains(marker) {
                return collected;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for marker {marker:?} in {collected:?}");
            }
            match tokio::time::timeout(remaining, output.recv()).await {
                Ok(Some(frame)) => {
                    collected.push_str(&String::from_utf8_lossy(&frame.data));
                }
                Ok(None) => panic!("output stream closed before marker {marker:?}: {collected:?}"),
                Err(_) => panic!("timed out waiting for marker {marker:?} in {collected:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn core_socket_survives_disconnect_and_replays_marker() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client
            .spawn(SpawnOptions {
                cwd: Some(cwd),
                cols: Some(80),
                rows: Some(24),
                shell: Some("/bin/sh".into()),
                env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
                ..Default::default()
            })
            .await
            .expect("spawn through terminal core");
        let terminal_id = spawned.info.id.clone();
        let claim = spawned.claim.clone();
        assert!(!claim.is_empty());

        let listed = client.list().await.expect("list");
        assert!(listed.iter().any(|item| item.id == terminal_id));
        let status = client.status(&terminal_id).await.expect("status");
        assert!(status.active);

        client
            .write(&terminal_id, "printf 'TERMUL_CORE_MARKER\\n'\n")
            .await
            .expect("write marker");

        let mut session = client
            .attach(&terminal_id, &claim, 0)
            .await
            .expect("attach after write");
        let first = collect_until_marker(
            &mut session.output,
            "TERMUL_CORE_MARKER",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(first.contains("TERMUL_CORE_MARKER"));
        drop(session);
        drop(client);

        let client = wait_for_client(&endpoint).await;
        let mut session = client
            .attach(&terminal_id, &claim, 0)
            .await
            .expect("reattach after disconnect");
        assert!(!session.result.gap || session.result.latest_seq > 0);
        let replayed = collect_until_marker(
            &mut session.output,
            "TERMUL_CORE_MARKER",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(
            replayed.contains("TERMUL_CORE_MARKER"),
            "reconnect replay missing marker: {replayed:?}"
        );

        client
            .write(&terminal_id, "printf 'TERMUL_CORE_LIVE\\n'\n")
            .await
            .expect("write live marker");
        let live = collect_until_marker(
            &mut session.output,
            "TERMUL_CORE_LIVE",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(live.contains("TERMUL_CORE_LIVE"));

        let error = match client.attach(&terminal_id, "not-a-real-claim", 0).await {
            Ok(_) => panic!("bad claim is unauthorized"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "UNAUTHORIZED");
        assert_eq!(error.client_message(), "unauthorized");

        client
            .detach(&terminal_id)
            .await
            .expect("detach does not kill");
        assert!(
            client
                .status(&terminal_id)
                .await
                .expect("still live")
                .active
        );

        client.shutdown().await.expect("shutdown core");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    #[cfg(unix)]
    async fn wait_for_reconnect(client: &TerminalCoreClient, endpoint: &CoreEndpoint) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match client.reconnect(endpoint).await {
                Ok(()) => return,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("terminal core reconnect failed: {error}"),
            }
        }
    }

    #[cfg(unix)]
    fn spawn_test_shell(cwd: &str) -> SpawnOptions {
        SpawnOptions {
            cwd: Some(cwd.to_string()),
            cols: Some(80),
            rows: Some(24),
            shell: Some("/bin/sh".into()),
            env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
            ..Default::default()
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_core_client_reconnects_to_a_new_server_on_the_same_endpoint() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client
            .spawn(spawn_test_shell(&cwd))
            .await
            .expect("spawn before reconnect");
        let terminal_id = spawned.info.id.clone();
        let mut session = client
            .attach(&terminal_id, &spawned.claim, 0)
            .await
            .expect("attach before reconnect");
        client
            .write(&terminal_id, "printf 'TERMUL_RECONNECT_BEFORE\\n'\n")
            .await
            .expect("write before reconnect");
        let before = collect_until_marker(
            &mut session.output,
            "TERMUL_RECONNECT_BEFORE",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(before.contains("TERMUL_RECONNECT_BEFORE"));

        server.abort();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;

        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        wait_for_reconnect(&client, &endpoint).await;
        client.list().await.expect("list after reconnect");
        assert!(
            client.inner.streams.lock().is_empty(),
            "inactive streams must be dropped after reconnect to a new empty server"
        );

        let spawned = client
            .spawn(spawn_test_shell(&cwd))
            .await
            .expect("spawn after reconnect");
        let terminal_id = spawned.info.id.clone();
        let mut session = client
            .attach(&terminal_id, &spawned.claim, 0)
            .await
            .expect("attach after reconnect");
        client
            .write(&terminal_id, "printf 'TERMUL_RECONNECT_AFTER\\n'\n")
            .await
            .expect("write after reconnect");
        let after = collect_until_marker(
            &mut session.output,
            "TERMUL_RECONNECT_AFTER",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(
            after.contains("TERMUL_RECONNECT_AFTER"),
            "output frames must resume after reconnect write: {after:?}"
        );

        client.shutdown().await.expect("shutdown second server");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_core_client_rewatches_live_streams_on_reconnect() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        let mut session = client
            .attach(&terminal_id, &spawned.claim, 0)
            .await
            .expect("attach");
        client
            .write(&terminal_id, "printf 'TERMUL_REWATCH_ONE\\n'\n")
            .await
            .expect("write one");
        collect_until_marker(
            &mut session.output,
            "TERMUL_REWATCH_ONE",
            std::time::Duration::from_secs(10),
        )
        .await;

        wait_for_reconnect(&client, &endpoint).await;
        assert!(
            client.inner.streams.lock().contains_key(&terminal_id),
            "active stream must be retained across reconnect to the same server"
        );

        client
            .write(&terminal_id, "printf 'TERMUL_REWATCH_TWO\\n'\n")
            .await
            .expect("write two");
        let resumed = collect_until_marker(
            &mut session.output,
            "TERMUL_REWATCH_TWO",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert!(
            resumed.contains("TERMUL_REWATCH_TWO"),
            "rewatch must resume output on the existing stream: {resumed:?}"
        );

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    #[test]
    fn lagged_output_fails_the_stream_instead_of_continuing() {
        assert_eq!(
            decide_live_output(Err(broadcast::error::RecvError::Lagged(4)), 10),
            LiveOutputDecision::FailGap { last_seq: 10 }
        );
        assert_eq!(
            decide_live_output(Err(broadcast::error::RecvError::Closed), 10),
            LiveOutputDecision::Stop
        );
    }

    #[test]
    fn stream_slot_initial_last_seq_comes_from_attach_cursor() {
        assert_eq!(
            requested_stream_last_seq(&json!({ "terminalId": "t", "lastSeq": 42u64 })),
            42
        );
        assert_eq!(requested_stream_last_seq(&json!({ "terminalId": "t" })), 0);
    }

    fn test_frame(kind: OutputKind, seq: u64, data: &[u8]) -> OutputFrame {
        OutputFrame {
            kind,
            terminal_id: "t1".to_string(),
            seq,
            data: data.to_vec(),
        }
    }

    fn one_slot(sender: mpsc::Sender<OutputFrame>, last_seq: u64) -> HashMap<String, StreamSlot> {
        HashMap::from([(
            "t1".to_string(),
            StreamSlot {
                sender,
                last_seq,
                resync: None,
            },
        )])
    }

    fn slot_resync(streams: &HashMap<String, StreamSlot>) -> Option<&StreamResync> {
        streams.get("t1")?.resync.as_ref()
    }

    /// A full slot used to be removed, which froze the terminal for good.
    #[tokio::test]
    async fn full_output_slot_resyncs_instead_of_failing_closed() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut streams = one_slot(tx, 7);
        let first = test_frame(OutputKind::Live, 8, b"a");
        assert!(matches!(
            deliver_client_output_frame(&mut streams, first),
            FrameRoute::Done
        ));
        let second = test_frame(OutputKind::Live, 9, b"b");
        assert!(
            matches!(
                deliver_client_output_frame(&mut streams, second),
                FrameRoute::Resync(ResyncStart { seq: 9, .. })
            ),
            "a full slot must start a resync"
        );
        assert_eq!(
            streams.get("t1").map(|slot| slot.last_seq),
            Some(8),
            "the dropped frame must not advance last_seq"
        );
        // The overrun subscription keeps sending; its frames are dropped.
        let old = test_frame(OutputKind::Live, 10, b"c");
        deliver_client_output_frame(&mut streams, old);
        // The re-watch replays everything after 8, before its reply arrives.
        let replay = test_frame(OutputKind::Replay, 10, b"bc");
        deliver_client_output_frame(&mut streams, replay);
        assert_eq!(
            slot_resync(&streams).map(|resync| resync.parked.len()),
            Some(1)
        );

        assert_eq!(rx.recv().await.map(|frame| frame.data), Some(b"a".to_vec()));
        let slot = streams.get_mut("t1").expect("slot kept");
        assert!(matches!(
            apply_resync_reply(slot, 8, false, 10),
            FrameRoute::Done
        ));
        assert!(slot.resync.is_none(), "the reply must end the resync");
        assert_eq!(slot.last_seq, 10);
        assert_eq!(
            rx.recv().await.map(|frame| frame.data),
            Some(b"bc".to_vec())
        );
        assert!(
            rx.try_recv().is_err(),
            "the overrun subscription leaked a frame"
        );

        let live = test_frame(OutputKind::Live, 11, b"d");
        deliver_client_output_frame(&mut streams, live);
        assert_eq!(rx.recv().await.map(|frame| frame.data), Some(b"d".to_vec()));
    }

    #[tokio::test]
    async fn evicted_resync_cursor_resets_the_screen_before_the_replay() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut streams = one_slot(tx, 7);
        let slot = streams.get_mut("t1").expect("slot");
        start_resync(
            slot,
            "t1".to_string(),
            Some((OutputKind::Live, 9)),
            0,
            "test",
            9,
        );
        assert!(matches!(
            apply_resync_reply(slot, 7, true, 40),
            FrameRoute::Done
        ));
        let replay = test_frame(OutputKind::Replay, 40, b"retained");
        deliver_client_output_frame(&mut streams, replay);
        let reset = rx.recv().await.expect("reset");
        assert_eq!(reset.data, RESYNC_RESET);
        assert_eq!(
            reset.seq, 7,
            "the reset must not move the consumer's cursor"
        );
        assert_eq!(
            rx.recv().await.map(|frame| frame.data),
            Some(b"retained".to_vec())
        );
        assert!(slot_resync(&streams).is_none());
    }

    /// An old Core sends its replay one frame per chunk, so the overrun
    /// subscription can still be replaying when the new one starts.
    #[test]
    fn old_core_replay_overrun_resumes_at_the_restarted_replay() {
        let (tx, _rx) = mpsc::channel(4);
        let mut streams = one_slot(tx, 7);
        let slot = streams.get_mut("t1").expect("slot");
        start_resync(
            slot,
            "t1".to_string(),
            Some((OutputKind::Replay, 20)),
            0,
            "test",
            20,
        );
        deliver_client_output_frame(&mut streams, test_frame(OutputKind::Replay, 21, b"old"));
        assert_eq!(
            slot_resync(&streams).map(|resync| (resync.parked.len(), resync.dropped)),
            Some((0, Some((OutputKind::Replay, 21)))),
            "a replay frame continuing past the dropped one is the old subscription's"
        );
        deliver_client_output_frame(&mut streams, test_frame(OutputKind::Replay, 8, b"new"));
        assert_eq!(
            slot_resync(&streams).map(|resync| resync.parked.len()),
            Some(1),
            "a replay restarting below the dropped seq is the new subscription's"
        );
    }

    #[test]
    fn resync_that_misses_the_replay_watches_again_then_fails_closed() {
        let (tx, _rx) = mpsc::channel(4);
        let mut streams = one_slot(tx, 7);
        let slot = streams.get_mut("t1").expect("slot");
        start_resync(
            slot,
            "t1".to_string(),
            Some((OutputKind::Live, 9)),
            0,
            "test",
            9,
        );
        for stalled in 1..MAX_STALLED_RESYNCS {
            let slot = streams.get_mut("t1").expect("slot");
            apply_resync_reply(slot, 7, false, 20);
            // Live output past the reply's latest seq: the replay was missed.
            let live = test_frame(OutputKind::Live, 20 + u64::from(stalled), b"x");
            assert!(matches!(
                deliver_client_output_frame(&mut streams, live),
                FrameRoute::Resync(_)
            ));
            assert_eq!(
                slot_resync(&streams).map(|resync| resync.stalled),
                Some(stalled)
            );
        }
        let slot = streams.get_mut("t1").expect("slot");
        apply_resync_reply(slot, 7, false, 20);
        let live = test_frame(OutputKind::Live, 30, b"x");
        assert!(matches!(
            deliver_client_output_frame(&mut streams, live),
            FrameRoute::Closed {
                stable_code: "STREAM_RESYNC_FAILED",
                ..
            }
        ));
        assert!(
            !streams.contains_key("t1"),
            "a stalled resync must fail closed"
        );
    }

    /// The Core ends a lagged subscription with `terminal.gap`. That used to
    /// end the consumer's stream; now nothing more can come from it, so the
    /// next frame, live or not, is the re-watch's.
    #[test]
    fn lagged_stream_resyncs_and_takes_the_next_frame_as_new() {
        let (tx, _rx) = mpsc::channel(4);
        let mut streams = one_slot(tx, 7);
        assert!(matches!(
            resync_after_gap(&mut streams, "t1", 7),
            FrameRoute::Resync(ResyncStart {
                cause: "STREAM_GAP",
                ..
            })
        ));
        deliver_client_output_frame(&mut streams, test_frame(OutputKind::Live, 12, b"new"));
        assert_eq!(
            slot_resync(&streams).map(|resync| resync.parked.len()),
            Some(1)
        );
    }

    /// A live hand-off asks for a cursor past the log. Keeping it would make
    /// a resync replay nothing and lose what the overrun dropped.
    #[test]
    fn delivered_frame_sets_last_seq_below_a_handoff_cursor() {
        let (tx, _rx) = mpsc::channel(4);
        let mut streams = one_slot(tx, (1 << 53) - 1);
        deliver_client_output_frame(&mut streams, test_frame(OutputKind::Live, 5, b"x"));
        assert_eq!(streams.get("t1").map(|slot| slot.last_seq), Some(5));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_initializes_stream_slot_from_requested_last_seq() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        // Let the shell finish its prompt first: a frame delivered after the
        // attach moves last_seq to that frame's seq.
        let viewer = TerminalCoreClient::connect(&endpoint)
            .await
            .expect("viewer");
        let mut prompt = viewer.watch(&terminal_id, 0).await.expect("viewer watch");
        collect_until_marker(&mut prompt.output, "$ ", std::time::Duration::from_secs(10)).await;
        let _session = client
            .attach(&terminal_id, &spawned.claim, 99)
            .await
            .expect("attach from cursor 99");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let last_seq = client
            .inner
            .streams
            .lock()
            .get(&terminal_id)
            .map(|slot| slot.last_seq);
        assert_eq!(
            last_seq,
            Some(99),
            "StreamSlot must start at the attach cursor, not 0"
        );

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    /// A renderer that reloads re-watches every terminal from seq 0. A TUI's
    /// retained history is hundreds of small chunks, more than one output slot
    /// holds before the caller starts draining it. Dropping that overflow lost
    /// the middle of the screen, and closing the slot left the terminal frozen.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watching_a_long_history_keeps_every_chunk_and_the_live_stream() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        let chunks = STREAM_OUTPUT_CAPACITY * 2;
        // One line per flush interval, so the history is many small chunks.
        // The markers are split by `""` so the echoed command never matches.
        client
            .write(
                &terminal_id,
                &format!(
                    "i=0; while [ $i -lt {chunks} ]; do echo row$i; i=$((i+1)); sleep 0.01; done; echo HISTORY_\"\"DONE\n"
                ),
            )
            .await
            .expect("write");
        let mut first = client.watch(&terminal_id, 0).await.expect("first watch");
        collect_until_marker(
            &mut first.output,
            "HISTORY_DONE\r\n",
            std::time::Duration::from_secs(30),
        )
        .await;
        drop(first);

        let mut session = client.watch(&terminal_id, 0).await.expect("re-watch");
        // The caller is not draining yet, as when the GUI is still setting up.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let replayed = collect_until_marker(
            &mut session.output,
            "HISTORY_DONE\r\n",
            std::time::Duration::from_secs(10),
        )
        .await;
        let last_row = format!("row{}\r\n", chunks - 1);
        assert!(
            replayed.contains(&last_row),
            "replay lost the tail: {replayed:?}"
        );
        let rows = replayed.matches("\r\nrow").count();
        assert!(
            rows + 1 >= chunks,
            "replay dropped rows: {rows} of {chunks}"
        );

        client
            .write(&terminal_id, "echo LIVE_\"\"AFTER_REPLAY\n")
            .await
            .expect("write live");
        collect_until_marker(
            &mut session.output,
            "LIVE_AFTER_REPLAY\r\n",
            std::time::Duration::from_secs(10),
        )
        .await;

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    /// `n` numbered rows, one output chunk each, then `tail`, then `done` is
    /// touched. The row commands never echo a `row<digits>` line themselves.
    #[cfg(unix)]
    fn rows_command(n: usize, tail: &str, done: &std::path::Path) -> String {
        format!(
            "i=0; while [ $i -lt {n} ]; do echo row$i; i=$((i+1)); sleep 0.01; done; {tail}; touch '{}'",
            done.display()
        )
    }

    #[cfg(unix)]
    fn assert_slot_resyncing(client: &TerminalCoreClient, terminal_id: &str) {
        assert!(
            client
                .inner
                .streams
                .lock()
                .get(terminal_id)
                .is_some_and(|slot| slot.resync.is_some()),
            "the output must overflow the undrained slot"
        );
    }

    #[cfg(unix)]
    fn row_numbers(output: &str) -> Vec<usize> {
        output
            .split("\r\n")
            .filter_map(|line| line.strip_prefix("row")?.parse().ok())
            .collect()
    }

    #[cfg(unix)]
    async fn wait_for_file(path: &std::path::Path) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while !path.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "shell never finished: {}",
                path.display()
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// A consumer that falls behind overflows its output slot. Dropping the
    /// slot froze the terminal for good: input still worked, output never
    /// came back. The slot must be rebuilt from the Core's output log.
    #[cfg(unix)]
    async fn assert_overflowed_slot_resumes_every_row(cursor: u64) {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        let mut session = client.watch(&terminal_id, cursor).await.expect("watch");
        let rows = STREAM_OUTPUT_CAPACITY + 64;
        let done = profile.path().join("rows-done");
        // Nothing drains the slot while the shell prints, so it overflows
        // partway through the rows; the marker comes after the overflow.
        client
            .write(
                &terminal_id,
                &format!("{}\n", rows_command(rows, "echo AFTER_\"\"OVERFLOW", &done)),
            )
            .await
            .expect("write");
        wait_for_file(&done).await;
        assert_slot_resyncing(&client, &terminal_id);

        let output = collect_until_marker(
            &mut session.output,
            "AFTER_OVERFLOW\r\n",
            std::time::Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            row_numbers(&output),
            (0..rows).collect::<Vec<_>>(),
            "rows were lost, repeated or reordered: {output:?}"
        );
        assert!(
            !output.contains("\x1bc"),
            "a resync whose cursor is still retained must not reset the screen"
        );

        client
            .write(&terminal_id, "echo LIVE_\"\"AFTER_RESYNC\n")
            .await
            .expect("write live");
        collect_until_marker(
            &mut session.output,
            "LIVE_AFTER_RESYNC\r\n",
            std::time::Duration::from_secs(10),
        )
        .await;

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overflowed_slot_resumes_every_row_after_a_full_watch() {
        assert_overflowed_slot_resumes_every_row(0).await;
    }

    /// The renderer hands a freshly spawned terminal over with this cursor
    /// (`Number.MAX_SAFE_INTEGER`), so the Core replays nothing.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overflowed_slot_resumes_every_row_after_a_live_handoff() {
        assert_overflowed_slot_resumes_every_row((1u64 << 53) - 1).await;
    }

    /// When the overrun slot's cursor is evicted before the re-watch, the
    /// screen is reset and rebuilt from everything the Core still holds.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overflowed_slot_whose_cursor_was_evicted_resets_then_replays_the_log() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        let mut session = client.watch(&terminal_id, 0).await.expect("watch");
        let done = profile.path().join("rows-done");
        // Overflow the slot, then print more than the log retains. `exec`
        // keeps the shell from printing anything after the marker.
        let tail = format!(
            "yes x | head -c {}; echo GAP_\"\"END",
            SCROLLBACK_CAP + SCROLLBACK_CAP / 2
        );
        client
            .write(
                &terminal_id,
                &format!(
                    "{}; exec sleep 600\n",
                    rows_command(STREAM_OUTPUT_CAPACITY + 64, &tail, &done)
                ),
            )
            .await
            .expect("write");
        wait_for_file(&done).await;
        assert_slot_resyncing(&client, &terminal_id);

        let marker = "GAP_END\r\n";
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut frames: Vec<Vec<u8>> = Vec::new();
        while !String::from_utf8_lossy(&frames.concat()).contains(marker) {
            let frame = tokio::time::timeout_at(deadline, session.output.recv())
                .await
                .expect("timed out before the marker")
                .expect("output stream closed before the marker");
            frames.push(frame.data);
        }
        let resets: Vec<usize> = frames
            .iter()
            .enumerate()
            .filter(|(_, data)| data.as_slice() == RESYNC_RESET)
            .map(|(index, _)| index)
            .collect();
        let [reset] = resets[..] else {
            panic!("expected exactly one reset, got {resets:?}");
        };
        let before = String::from_utf8_lossy(&frames[..reset].concat()).into_owned();
        assert!(
            before.contains("row0\r\n") && !before.contains(marker),
            "the reset must come after the delivered prefix and before the replay"
        );
        let after = frames[reset + 1..].concat();

        // Everything the Core still retains, as a fresh full watch sees it.
        let viewer = TerminalCoreClient::connect(&endpoint)
            .await
            .expect("viewer");
        let mut full = viewer.watch(&terminal_id, 0).await.expect("full watch");
        let retained =
            collect_until_marker(&mut full.output, marker, std::time::Duration::from_secs(10))
                .await;
        assert!(
            after.ends_with(retained.as_bytes()),
            "the resync must replay the whole retained log after the reset"
        );

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_output_slot_does_not_hol_block_other_rpc() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let server = tokio::spawn(async move {
            run_terminal_core_on_endpoint(server_endpoint, test_program()).await
        });

        let client = wait_for_client(&endpoint).await;
        let cwd = profile.path().to_string_lossy().into_owned();
        let spawned = client.spawn(spawn_test_shell(&cwd)).await.expect("spawn");
        let terminal_id = spawned.info.id.clone();
        let _session = client
            .attach(&terminal_id, &spawned.claim, 0)
            .await
            .expect("attach");

        for index in 0..(STREAM_OUTPUT_CAPACITY as u32 + 40) {
            client
                .write(&terminal_id, &format!("printf '%s\\n' '{index}'\n"))
                .await
                .expect("write to fill the output slot");
        }

        let listed = tokio::time::timeout(std::time::Duration::from_secs(5), client.list())
            .await
            .expect("list must not HOL-block behind a full output slot")
            .expect("list");
        assert!(
            listed.iter().any(|item| item.id == terminal_id),
            "multiplexed reader must keep serving RPC after one stream backs up"
        );

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    }
}
