//! Terminal Core: owns PtyManager and serves local IPC.
//!
//! This module is the desktop PTY owner. It must not import ACP. GUI and
//! shared-live talk to it over bounded framed messages; standalone keeps
//! in-process ownership through [`super::handles`].

use super::handles::TerminalRuntimeHandle;
use super::ipc::{
    prepare_runtime_dir, read_frame, read_json_frame, remove_stale_socket, validate_hello,
    write_frame, write_json_frame, CoreEndpoint, CoreError, CoreErrorPayload, CoreEvent, CoreHello,
    CoreRequest, CoreResponse, CoreRole, CURRENT_PROTOCOL_VERSION,
};
use crate::conversation::ConversationId;
use crate::pty::claims::RotatedClaim;
use crate::pty::manager::{
    SpawnedTerminal, TerminalAttachResult, TerminalResumeGrant, TerminalResumeRequest,
};
use crate::pty::{PtyManager, SpawnOptions};
use crate::trackers::{
    CwdTracker, ExitCodeTracker, GitTracker, TerminalDisplayMode, TerminalEvent, TerminalEventHub,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Live,
    Replay,
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

fn status_from_instance(
    pty: &PtyManager,
    instance: &crate::pty::manager::TerminalInstance,
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
        conversation_id: instance.conversation_id.to_string(),
        project_id: instance.project_id.clone(),
        workspace_ref_tracked: instance.workspace_ref_tracked,
        latest_seq,
        active: instance.is_active(),
        lifecycle: instance.lifecycle_state().as_str().to_string(),
        claim_generation: pty.claim_generation(&instance.id),
    }
}

fn construct_pty_manager() -> Arc<PtyManager> {
    let events = TerminalEventHub::standalone();
    let cwd = Arc::new(CwdTracker::new(events.clone()));
    let git = Arc::new(GitTracker::new(None, events.clone()));
    let exit = Arc::new(ExitCodeTracker::new(events.clone()));
    Arc::new(PtyManager::new(events, cwd, git, exit))
}

struct TerminalCoreState {
    pty: Arc<PtyManager>,
    shutdown: watch::Sender<bool>,
}

#[cfg(unix)]
pub async fn run_terminal_core(profile_root: PathBuf) -> Result<(), CoreError> {
    let endpoint = CoreEndpoint::for_profile(&profile_root, CoreRole::TerminalCore);
    run_terminal_core_on_endpoint(endpoint).await
}

#[cfg(not(unix))]
pub async fn run_terminal_core(_profile_root: PathBuf) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform)
}

#[cfg(unix)]
pub async fn run_terminal_core_on_endpoint(endpoint: CoreEndpoint) -> Result<(), CoreError> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    prepare_runtime_dir(&endpoint)?;
    let _ = remove_stale_socket(&endpoint);
    let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
    let listener = UnixListener::bind(path).map_err(CoreError::from)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(CoreError::from)?;

    let pty = construct_pty_manager();
    let (shutdown, shutdown_rx) = watch::channel(false);
    let state = Arc::new(TerminalCoreState {
        pty: Arc::clone(&pty),
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
                let (stream, _) = accepted.map_err(CoreError::from)?;
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

#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<TerminalCoreState>,
) -> Result<(), CoreError> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::unix::OwnedWriteHalf;

    let mut stream = stream;
    let hello: CoreHello = read_json_frame(&mut stream).await?;
    let ack = validate_hello(&hello, CoreRole::TerminalCore)?;
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

    let result = async {
        loop {
            let payload = match read_frame(&mut reader).await {
                Ok(payload) => payload,
                Err(CoreError::Io(_)) => break,
                Err(error) => return Err(error),
            };
            if is_output_frame(&payload) {
                continue;
            }
            let request: CoreRequest = serde_json::from_slice(&payload)
                .map_err(|error| CoreError::InvalidFrame(format!("invalid JSON frame: {error}")))?;
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
    let _ = OwnedWriteHalf::shutdown(&mut *guard).await;
    result
}

#[cfg(unix)]
async fn dispatch_request(
    state: &Arc<TerminalCoreState>,
    request: &CoreRequest,
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
) -> CoreResponse {
    match handle_method(state, request, writer, subscriptions).await {
        Ok(value) => ok_response(request.id, value),
        Err(error) => error_response(request.id, error),
    }
}

#[cfg(unix)]
async fn handle_method(
    state: &Arc<TerminalCoreState>,
    request: &CoreRequest,
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
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
            let statuses: Vec<TerminalStatus> = pty
                .get_all()
                .iter()
                .map(|instance| status_from_instance(pty, instance))
                .collect();
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
            pty.resize(&params.terminal_id, params.cols, params.rows)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
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
            let (grant, _replay) = pty
                .resume_for_conversation(
                    params.conversation_id,
                    &params.terminal_id,
                    params.last_seq,
                )
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

#[cfg(unix)]
async fn begin_output_stream(
    pty: &Arc<PtyManager>,
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
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

#[cfg(unix)]
async fn start_replay_forwarder(
    pty: &Arc<PtyManager>,
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    subscriptions: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    terminal_id: &str,
    replay: crate::pty::manager::TerminalReplay,
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
        for chunk in &replay.chunks {
            let Ok(frame) =
                encode_output_frame(OutputKind::Replay, &attached_id, chunk.seq, &chunk.data)
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
                            let message = CoreEvent {
                                topic: GAP_TOPIC.to_string(),
                                payload: json!(GapPayload {
                                    terminal_id: attached_id.clone(),
                                    last_seq: current_seq,
                                }),
                            };
                            let mut guard = writer.lock().await;
                            if write_json_frame(&mut *guard, &message).await.is_err() {
                                break;
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
}

struct ClientInner {
    #[cfg(unix)]
    writer: tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<CoreResponse>>>,
    streams: Mutex<HashMap<String, StreamSlot>>,
    live: Mutex<HashSet<String>>,
    events: broadcast::Sender<TerminalEvent>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reconnect: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct TerminalCoreClient {
    inner: Arc<ClientInner>,
}

impl TerminalCoreClient {
    #[cfg(unix)]
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

    #[cfg(not(unix))]
    pub async fn connect(_endpoint: &CoreEndpoint) -> Result<Self, CoreError> {
        Err(CoreError::UnsupportedPlatform)
    }

    /// Re-handshake against `endpoint` and swap the live stream. The event
    /// broadcast stays on the same inner Arc so the GUI event-hub mirror keeps
    /// receiving after a Core restart. After the socket swap we refresh `live`,
    /// drop stream senders whose terminal is gone, and re-watch remaining ids
    /// from each stream's last known seq so output resumes.
    #[cfg(unix)]
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

    #[cfg(not(unix))]
    pub async fn reconnect(&self, _endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        Err(CoreError::UnsupportedPlatform)
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
    ) -> Result<crate::pty::manager::DisplayModeState, CoreError> {
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

    pub async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError> {
        self.rpc(
            METHOD_RESIZE,
            json!({ "terminalId": terminal_id, "cols": cols, "rows": rows }),
        )
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

    async fn open_stream(
        &self,
        method: &str,
        params: Value,
        terminal_id: &str,
    ) -> Result<TerminalAttachSession, CoreError> {
        let (tx, rx) = mpsc::channel(256);
        self.inner.streams.lock().insert(
            terminal_id.to_string(),
            StreamSlot {
                sender: tx,
                last_seq: 0,
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
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(id, tx);
        let request = CoreRequest {
            id,
            method: method.to_string(),
            params,
        };
        {
            #[cfg(unix)]
            {
                let mut writer = self.inner.writer.lock().await;
                if let Err(error) = write_json_frame(&mut *writer, &request).await {
                    self.inner.pending.lock().remove(&id);
                    return Err(error);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (&request, id);
                return Err(CoreError::UnsupportedPlatform);
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
}

#[cfg(unix)]
async fn open_terminal_stream(
    endpoint: &CoreEndpoint,
) -> Result<
    (
        tokio::net::unix::OwnedReadHalf,
        tokio::net::unix::OwnedWriteHalf,
    ),
    CoreError,
> {
    use tokio::net::UnixStream;

    let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
    let mut stream = UnixStream::connect(path).await.map_err(CoreError::from)?;
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

#[cfg(unix)]
fn spawn_terminal_read_loop(inner: &Arc<ClientInner>, reader: tokio::net::unix::OwnedReadHalf) {
    let reader_inner = Arc::clone(inner);
    let handle = tokio::spawn(async move {
        client_read_loop(reader, reader_inner).await;
    });
    *inner.reader.lock() = Some(handle);
}

#[cfg(unix)]
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

#[cfg(unix)]
async fn client_read_loop(mut reader: tokio::net::unix::OwnedReadHalf, inner: Arc<ClientInner>) {
    loop {
        let payload = match read_frame(&mut reader).await {
            Ok(payload) => payload,
            Err(_) => break,
        };
        if is_output_frame(&payload) {
            if let Ok(frame) = decode_output_frame(&payload) {
                let tx = {
                    let mut streams = inner.streams.lock();
                    streams.get_mut(&frame.terminal_id).map(|slot| {
                        if frame.seq > slot.last_seq {
                            slot.last_seq = frame.seq;
                        }
                        slot.sender.clone()
                    })
                };
                if let Some(tx) = tx {
                    let _ = tx.send(frame).await;
                }
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

    async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError> {
        TerminalCoreClient::resize(self, terminal_id, cols, rows).await
    }

    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError> {
        TerminalCoreClient::terminate(self, terminal_id).await
    }

    fn is_live(&self, terminal_id: &str) -> bool {
        self.inner.live.lock().contains(terminal_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::manager::SCROLLBACK_CAP;

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
    fn scrollback_cap_is_256_kib() {
        assert_eq!(SCROLLBACK_CAP, 256 * 1024);
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
        let server =
            tokio::spawn(async move { run_terminal_core_on_endpoint(server_endpoint).await });

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
        let server =
            tokio::spawn(async move { run_terminal_core_on_endpoint(server_endpoint).await });

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
        let server =
            tokio::spawn(async move { run_terminal_core_on_endpoint(server_endpoint).await });

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
        let server =
            tokio::spawn(async move { run_terminal_core_on_endpoint(server_endpoint).await });

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
}
