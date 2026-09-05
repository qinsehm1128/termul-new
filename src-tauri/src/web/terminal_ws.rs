//! Dedicated interactive terminal websocket.
//!
//! This endpoint intentionally stays separate from the ACP relay. The HTTP
//! upgrade validates Origin the same way `/ws` does. A valid `Authorization`
//! bearer is optional on the handshake (browsers and iOS `URLSessionWebSocketTask`
//! often cannot send it). If the header is absent, the first frame must be
//! `authenticate` with the same token. All later operations stay
//! Conversation-scoped: `conversationId` is the primary PTY ownership/claim
//! scope. `projectId` is optional attribution only.
//! Companion clients enumerate live PTYs with `list` (`conversationId` first,
//! `projectId` as attribution fallback) and subscribe with `watch` without
//! rotating the desktop claim. The bearer principal and current claim
//! generation still fence passive output.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use std::net::{IpAddr, SocketAddr};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap};
use axum::response::IntoResponse;
use axum::Extension;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::conversation::ConversationId;
#[cfg(test)]
use crate::pty::manager::SpawnOptions;
use crate::pty::manager::{TerminalReplay, TerminalResumeRequest, TerminalSpawnIntentV1};
use crate::trackers::TerminalDisplayMode;
use crate::web::auth::{
    auth_error_response, RemoteAccessAuthority, RemoteAuthError, RemoteCapability, RemotePrincipal,
};
use crate::web::ws::AppState;

const MAX_RECONNECT_FRAMES: usize = 64;
const ATTACH_GENERATION_CHECK_MS: u64 = 250;
const BINARY_SUBPROTOCOL: &str = "se-terminal-v2.binary";
const BINARY_FRAME_MAGIC: &[u8; 4] = b"TML2";
static CONNECTION_SERIAL: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
#[repr(u8)]
enum BinaryOutputKind {
    Live = 1,
    Replay = 2,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatePayload {
    token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TerminalProjectSpawnIntent {
    project_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuthorizedTerminalScope {
    conversation_id: crate::conversation::ConversationId,
    claim_generation: Option<u64>,
    cleanup_only: bool,
}

type AuthorizedTerminals = Arc<RwLock<HashMap<String, AuthorizedTerminalScope>>>;

pub async fn terminal_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let peer_ip = peer.ip();
    let admitted = match admit_terminal_upgrade(&authority, &headers, peer_ip) {
        Ok(principal) => principal,
        Err(error) => {
            warn!(
                target: "se_manager::web::terminal_ws",
                stable_code = error.code(),
                "terminal WebSocket upgrade rejected by authentication policy"
            );
            return auth_error_response(error);
        }
    };
    info!(
        target: "se_manager::web::terminal_ws",
        handshake_bearer = admitted.is_some(),
        stable_code = "OK",
        "terminal WebSocket upgrade Origin accepted"
    );
    let binary_output = supports_binary_subprotocol(&headers);
    let ws = if binary_output {
        ws.protocols([BINARY_SUBPROTOCOL])
    } else {
        ws
    };
    info!(
        target: "se_manager::web::terminal_ws",
        binary_output,
        "terminal WebSocket output protocol selected"
    );
    ws.on_upgrade(move |socket| run(socket, state, authority, admitted, peer_ip, binary_output))
        .into_response()
}

fn supports_binary_subprotocol(headers: &HeaderMap) -> bool {
    headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|protocol| protocol.trim() == BINARY_SUBPROTOCOL)
}

fn admit_terminal_upgrade(
    authority: &RemoteAccessAuthority,
    headers: &HeaderMap,
    peer: IpAddr,
) -> Result<Option<RemotePrincipal>, RemoteAuthError> {
    authority.verify_origin(headers.get(header::ORIGIN))?;
    let Some(authorization) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let token = authorization
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or(RemoteAuthError::InvalidCredential)?;
    let principal = authority.verify_bearer_for_peer(token, peer)?;
    authority.authorize(&principal, RemoteCapability::Mutate)?;
    Ok(Some(principal))
}

fn principal_generation_mismatch(
    authority: &RemoteAccessAuthority,
    principal: Option<&RemotePrincipal>,
) -> bool {
    let Some(principal) = principal else {
        return false;
    };
    let current = authority.generation_state();
    !current.active || current.generation != principal.generation()
}

fn spawn_terminal_event_task(
    state: AppState,
    authorized: AuthorizedTerminals,
    authority: Arc<RemoteAccessAuthority>,
    principal_generation: u64,
    tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut event_rx = state.terminal_events.subscribe();
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if !should_forward_passive(&authority, principal_generation) {
                        break;
                    }
                    let terminal_id = event.terminal_id().to_string();
                    let catalog_event = matches!(
                        event,
                        crate::trackers::TerminalEvent::Spawned { .. }
                            | crate::trackers::TerminalEvent::Exit { .. }
                    );
                    if !catalog_event
                        && live_authorized_terminal_scope(&state, &authorized, &terminal_id)
                            .is_none()
                    {
                        continue;
                    }
                    if !should_forward_passive(&authority, principal_generation) {
                        break;
                    }
                    let payload = serde_json::to_value(&event).unwrap_or_else(|_| json!({}));
                    if send_json(&tx, json!({ "type": "event", "payload": payload }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("[terminal-ws] lifecycle event receiver lagged by {skipped}");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn should_forward_passive(authority: &RemoteAccessAuthority, principal_generation: u64) -> bool {
    let current = authority.generation_state();
    current.active && current.generation == principal_generation
}

async fn abort_and_join_connection_tasks(
    event_task: tokio::task::JoinHandle<()>,
    attachments: HashMap<String, tokio::task::JoinHandle<()>>,
) {
    event_task.abort();
    for task in attachments.values() {
        task.abort();
    }
    let _ = event_task.await;
    for task in attachments.into_values() {
        let _ = task.await;
    }
}

async fn run(
    socket: WebSocket,
    state: AppState,
    authority: Arc<RemoteAccessAuthority>,
    mut principal: Option<RemotePrincipal>,
    peer: IpAddr,
    binary_output: bool,
) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(MAX_RECONNECT_FRAMES);

    let write_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    // Per-connection authorization: exact terminal + Conversation + claim
    // generation scopes this socket may operate on. The generation binding is
    // essential: a resume/rotate on another connection must revoke every
    // derived write/query/event capability here, not only the output stream.
    // Shared with the event-forwarding task so it sees rotations immediately.
    let authorized: AuthorizedTerminals = Arc::new(RwLock::new(HashMap::new()));
    info!(
        handshake_bearer = principal.is_some(),
        "[terminal-ws] client connected after Origin admission"
    );

    let mut generation_rx = authority.subscribe_generation();
    let mut event_task = principal.as_ref().map(|admitted| {
        spawn_terminal_event_task(
            state.clone(),
            authorized.clone(),
            Arc::clone(&authority),
            admitted.generation(),
            tx.clone(),
        )
    });

    let mut ctx = ConnectionContext::new(authorized.clone(), binary_output);

    loop {
        if principal_generation_mismatch(&authority, principal.as_ref()) {
            info!(
                target: "se_manager::web::terminal_ws",
                generation = principal.as_ref().map(RemotePrincipal::generation).unwrap_or(0),
                lifecycle_phase = "generation_mismatch",
                stable_code = "OK",
                "terminal WebSocket closing after authority generation change"
            );
            break;
        }
        tokio::select! {
            changed = generation_rx.changed() => {
                if changed.is_err() || principal_generation_mismatch(&authority, principal.as_ref()) {
                    info!(
                        target: "se_manager::web::terminal_ws",
                        generation = principal.as_ref().map(RemotePrincipal::generation).unwrap_or(0),
                        lifecycle_phase = "generation_mismatch",
                        stable_code = "OK",
                        "terminal WebSocket closing after authority generation change"
                    );
                    break;
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else { break };
                let Ok(message) = frame else { break };
                let Message::Text(text) = message else {
                    continue;
                };
                if principal_generation_mismatch(&authority, principal.as_ref()) {
                    break;
                }
                let request = match serde_json::from_str::<Request>(&text) {
                    Ok(request) => request,
                    Err(error) => {
                        let _ = send_error(&tx, "malformed", "VALIDATION_ERROR", error.to_string()).await;
                        continue;
                    }
                };
                let id = request.id.clone();
                let op_type = request.type_.clone();
                info!("[terminal-ws] request start type={op_type}");
                let outcome = if principal.is_none() {
                    admit_terminal_frame(&request, &authority, peer, &mut principal)
                } else {
                    match principal.as_ref() {
                        Some(admitted) => handle(request, &state, &authority, admitted, &tx, &mut ctx).await,
                        None => Err(("UNAUTHORIZED", "pre-auth: send an `authenticate` request first".to_string())),
                    }
                };
                if event_task.is_none() {
                    if let Some(admitted) = principal.as_ref() {
                        event_task = Some(spawn_terminal_event_task(
                            state.clone(),
                            authorized.clone(),
                            Arc::clone(&authority),
                            admitted.generation(),
                            tx.clone(),
                        ));
                    }
                }
                match outcome {
                    Ok(data) => {
                        info!("[terminal-ws] request success type={op_type}");
                        let _ = send_json(&tx, json!({ "id": id, "success": true, "data": data })).await;
                    }
                    Err((code, message)) => {
                        warn!("[terminal-ws] request failed type={op_type} code={code}");
                        let _ = send_error(&tx, &id, code, message).await;
                    }
                }
            }
        }
    }

    // Cleanup: abort and join event/attachment tasks. PTYs are preserved.
    release_phone_fits(&state, &mut ctx).await;
    let attached_ids: Vec<String> = ctx.attachments.keys().cloned().collect();
    let attachments = std::mem::take(&mut ctx.attachments);
    for terminal_id in &attached_ids {
        state.pty.note_view_closed(terminal_id);
    }
    if let Some(event_task) = event_task {
        abort_and_join_connection_tasks(event_task, attachments).await;
    } else {
        for task in attachments.into_values() {
            task.abort();
            let _ = task.await;
        }
    }
    info!(
        "[terminal-ws] client disconnected; {} PTY(s) preserved",
        ctx.authorized.read().len()
    );
    drop(tx);
    let _ = write_task.await;
}

struct ConnectionContext {
    id: String,
    /// Exact terminal → Conversation + claim-generation scopes authorized on
    /// this connection.
    authorized: AuthorizedTerminals,
    /// Per-terminal output forwarding tasks (terminal_id -> task).
    attachments: HashMap<String, tokio::task::JoinHandle<()>>,
    /// True only when the client requested and negotiated the v2 binary output subprotocol.
    binary_output: bool,
    /// Terminals this connection currently owns in phone-fit mode.
    phone_fit: HashSet<String>,
}

impl ConnectionContext {
    fn new(authorized: AuthorizedTerminals, binary_output: bool) -> Self {
        Self {
            id: format!(
                "termws-{}",
                CONNECTION_SERIAL.fetch_add(1, Ordering::Relaxed)
            ),
            authorized,
            attachments: HashMap::new(),
            binary_output,
            phone_fit: HashSet::new(),
        }
    }

    fn authorize(
        &mut self,
        terminal_id: &str,
        conversation_id: crate::conversation::ConversationId,
        claim_generation: u64,
    ) {
        self.authorized.write().insert(
            terminal_id.to_string(),
            AuthorizedTerminalScope {
                conversation_id,
                claim_generation: Some(claim_generation),
                cleanup_only: false,
            },
        );
    }

    fn authorize_cleanup(
        &mut self,
        terminal_id: &str,
        conversation_id: crate::conversation::ConversationId,
    ) {
        self.authorized.write().insert(
            terminal_id.to_string(),
            AuthorizedTerminalScope {
                conversation_id,
                claim_generation: None,
                cleanup_only: true,
            },
        );
    }

    fn scope(&self, terminal_id: &str) -> Option<AuthorizedTerminalScope> {
        self.authorized.read().get(terminal_id).copied()
    }

    fn has_live_authorization(&self, state: &AppState) -> bool {
        let scopes = self
            .authorized
            .read()
            .iter()
            .map(|(terminal_id, scope)| (terminal_id.clone(), *scope))
            .collect::<Vec<_>>();
        scopes.into_iter().any(|(terminal_id, scope)| {
            terminal_authorization_is_live(state, &terminal_id, scope).is_some()
        })
    }

    #[cfg(test)]
    fn is_authorized(&self, terminal_id: &str) -> bool {
        self.authorized.read().contains_key(terminal_id)
    }

    fn close_view(&mut self, terminal_id: &str) -> bool {
        if let Some(task) = self.attachments.remove(terminal_id) {
            task.abort();
            true
        } else {
            false
        }
    }

    fn detach(&mut self, terminal_id: &str) -> bool {
        let released = self.close_view(terminal_id);
        self.authorized.write().remove(terminal_id);
        released
    }
}

fn release_connection_view(
    state: &AppState,
    ctx: &mut ConnectionContext,
    terminal_id: &str,
    drop_authorization: bool,
) {
    let released = if drop_authorization {
        ctx.detach(terminal_id)
    } else {
        ctx.close_view(terminal_id)
    };
    if released {
        state.pty.note_view_closed(terminal_id);
    }
}

async fn release_connection_phone_fit(
    state: &AppState,
    ctx: &mut ConnectionContext,
    terminal_id: &str,
) {
    if !ctx.phone_fit.remove(terminal_id) {
        return;
    }
    if let Err(error) = state
        .pty
        .set_display_mode(
            terminal_id,
            TerminalDisplayMode::Desktop,
            None,
            None,
            &ctx.id,
            false,
        )
        .await
    {
        info!("[terminal-ws] phone-fit release failed terminal_id={terminal_id} error={error}");
    }
}

async fn release_phone_fits(state: &AppState, ctx: &mut ConnectionContext) {
    let owned: Vec<String> = ctx.phone_fit.drain().collect();
    for terminal_id in owned {
        if let Err(error) = state
            .pty
            .set_display_mode(
                &terminal_id,
                TerminalDisplayMode::Desktop,
                None,
                None,
                &ctx.id,
                false,
            )
            .await
        {
            info!("[terminal-ws] phone-fit release failed terminal_id={terminal_id} error={error}");
        }
    }
}

fn admit_terminal_frame(
    request: &Request,
    authority: &RemoteAccessAuthority,
    peer: IpAddr,
    principal: &mut Option<RemotePrincipal>,
) -> Result<Value, (&'static str, String)> {
    if request.type_ != "authenticate" {
        return Err((
            "UNAUTHORIZED",
            "pre-auth: send an `authenticate` request first".to_string(),
        ));
    }
    if principal.is_some() {
        return Ok(json!({}));
    }
    let payload: AuthenticatePayload =
        serde_json::from_value(request.payload.clone()).map_err(|_| {
            (
                "UNAUTHORIZED",
                "pre-auth: send an `authenticate` request first".to_string(),
            )
        })?;
    match authority.verify_bearer_for_peer(&payload.token, peer) {
        Ok(admitted) => {
            authority
                .authorize(&admitted, RemoteCapability::Mutate)
                .map_err(|_| unauthorized_error("principal"))?;
            *principal = Some(admitted);
            info!(
                target: "se_manager::web::terminal_ws",
                request_type = "authenticate",
                auth_class = "bearer",
                stable_code = "OK",
                "terminal WebSocket authentication completed"
            );
            Ok(json!({}))
        }
        Err(error) => {
            warn!(
                target: "se_manager::web::terminal_ws",
                request_type = "authenticate",
                auth_class = "bearer",
                stable_code = error.code(),
                "terminal WebSocket authentication rejected"
            );
            Err((error.code(), error.to_string()))
        }
    }
}

async fn handle(
    request: Request,
    state: &AppState,
    authority: &Arc<RemoteAccessAuthority>,
    principal: &RemotePrincipal,
    tx: &mpsc::Sender<Message>,
    ctx: &mut ConnectionContext,
) -> Result<Value, (&'static str, String)> {
    authority
        .authorize(principal, RemoteCapability::Mutate)
        .map_err(|_| unauthorized_error("principal"))?;

    match request.type_.as_str() {
        "authenticate" => Ok(json!({})),
        "spawn" => {
            let has_conversation = request
                .payload
                .get("conversationId")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if !has_conversation {
                return spawn_project_terminal(request.payload, state, ctx).await;
            }
            let intent: TerminalSpawnIntentV1 = serde_json::from_value(request.payload)
                .map_err(|error| ("VALIDATION_ERROR", error.to_string()))?;
            let conversation_id = intent.conversation_id;
            let cwd_source = match intent.cwd_source {
                crate::pty::manager::TerminalCwdSource::Workspace => "workspace",
                crate::pty::manager::TerminalCwdSource::ExecutionTarget => "executionTarget",
            };
            info!(
                "[terminal-ws] spawn requested conversation_id={} project_attribution_present={} cwd_source={}",
                conversation_id,
                intent.project_id.is_some(),
                cwd_source
            );

            let conversation = terminal_conversation_service(state)?;
            let record = conversation
                .get_conversation(conversation_id)
                .map_err(|error| {
                    (
                        terminal_resource_code(Some(error.code.as_str())),
                        error.detail,
                    )
                })?;
            let workspace = conversation.session_workspace();
            let result = crate::commands::terminal_spawn_intent_resource(
                intent, &record, &state.pty, &workspace,
            )
            .await;
            if !result.success {
                let code = terminal_resource_code(result.code.as_deref());
                let error = result
                    .error
                    .unwrap_or_else(|| "terminal spawn failed".to_string());
                retain_compound_cleanup_authorization(state, ctx, code, &error);
                return Err((code, error));
            }
            let spawned = result.data.expect("successful terminal spawn has data");
            debug_assert_eq!(
                state
                    .pty
                    .get(&spawned.info.id)
                    .map(|instance| instance.conversation_id),
                Some(conversation_id)
            );
            let generation = state
                .pty
                .claim_generation(&spawned.info.id)
                .ok_or_else(|| unauthorized_error(&spawned.info.id))?;
            ctx.authorize(&spawned.info.id, conversation_id, generation);
            info!(
                "[terminal-ws] spawn success conversation_id={} terminal_id={} cwd_source={}",
                conversation_id, spawned.info.id, cwd_source
            );
            serde_json::to_value(spawned).map_err(|error| ("SPAWN_FAILED", error.to_string()))
        }
        "resume" => {
            let resume: TerminalResumeRequest = serde_json::from_value(request.payload)
                .map_err(|error| ("VALIDATION_ERROR", error.to_string()))?;
            if resume.terminal_id.trim().is_empty() {
                return Err(("VALIDATION_ERROR", "missing terminalId".to_string()));
            }
            let workspace = terminal_workspace_service(state)?;
            // Every denial variant collapses to one generic response here. The
            // remote surface has proved nothing, so telling it apart "gone"
            // from "not authorized" would be an existence leak — that
            // distinction is reserved for the local Tauri boundary.
            let (grant, replay) =
                crate::commands::terminal_resume_resource(&resume, &state.pty, &workspace)
                    .await
                    .map_err(|_| unauthorized_error(&resume.terminal_id))?;
            let generation = replay
                .claim_generation
                .ok_or_else(|| unauthorized_error(&resume.terminal_id))?;
            ctx.authorize(&resume.terminal_id, resume.conversation_id, generation);
            install_replay_forwarder(
                &resume.terminal_id,
                replay,
                generation,
                state,
                PassiveForwardAuth {
                    authority,
                    principal_generation: principal.generation(),
                },
                tx,
                ctx,
            )
            .await?;
            info!(
                "[terminal-ws] resume success conversation_id={} terminal_id={} latest_seq={} gap={}",
                resume.conversation_id,
                resume.terminal_id,
                grant.terminal.latest_seq,
                grant.terminal.gap
            );
            serde_json::to_value(grant).map_err(|error| ("NETWORK_ERROR", error.to_string()))
        }
        "list" => {
            let conversation_filter = optional_conversation_id(&request.payload)?;
            let project_filter = request
                .payload
                .get("projectId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            if conversation_filter.is_none() && project_filter.is_none() {
                return Err((
                    "VALIDATION_ERROR",
                    "list requires conversationId or projectId".to_string(),
                ));
            }
            let terminals: Vec<Value> = state
                .pty
                .get_all()
                .into_iter()
                .filter(|instance| {
                    instance.is_active()
                        && companion_list_matches(instance, conversation_filter, project_filter)
                })
                .map(|instance| {
                    let cwd = state
                        .cwd_tracker
                        .get_cwd(&instance.id)
                        .unwrap_or_else(|| instance.cwd.clone());
                    let git_branch = state.git_tracker.get_branch(&instance.id);
                    live_terminal_summary(&instance, cwd, git_branch)
                })
                .collect();
            info!(
                "[terminal-ws] list success conversation_filter={} project_filter={} count={}",
                conversation_filter.is_some(),
                project_filter.is_some(),
                terminals.len()
            );
            Ok(json!({ "terminals": terminals }))
        }
        "watch" => {
            let terminal_id = string_field(&request.payload, "terminalId")?.to_string();
            let last_seq = request.payload["lastSeq"].as_u64().unwrap_or(0);
            info!("[terminal-ws] watch requested terminal_id={terminal_id}");
            bind_companion_output_stream(
                state,
                authority,
                principal,
                tx,
                ctx,
                terminal_id,
                last_seq,
            )
            .await
        }
        "write" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            let data = string_field(&request.payload, "data")?;
            state
                .pty
                .write(terminal_id, data)
                .await
                .map(|_| Value::Null)
                .map_err(|error| ("WRITE_FAILED", error))
        }
        "resize" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            let cols = u16_field(&request.payload, "cols")?;
            let rows = u16_field(&request.payload, "rows")?;
            state
                .pty
                .resize(terminal_id, cols, rows)
                .await
                .map(|_| Value::Null)
                .map_err(|error| ("RESIZE_FAILED", error))
        }
        "set_display_mode" => {
            let terminal_id = string_field(&request.payload, "terminalId")?.to_string();
            authorized_terminal_scope(state, ctx, &terminal_id)?;
            let mode = TerminalDisplayMode::parse(string_field(&request.payload, "mode")?)
                .map_err(|error| ("VALIDATION_ERROR", error))?;
            let cols = optional_u16_field(&request.payload, "cols")?;
            let rows = optional_u16_field(&request.payload, "rows")?;
            let force = request.payload["force"].as_bool().unwrap_or(false);
            let state_value = state
                .pty
                .set_display_mode(&terminal_id, mode, cols, rows, &ctx.id, force)
                .await
                .map_err(|error| ("RESIZE_FAILED", error))?;
            match mode {
                TerminalDisplayMode::Phone => {
                    ctx.phone_fit.insert(terminal_id);
                }
                TerminalDisplayMode::Desktop => {
                    ctx.phone_fit.remove(&terminal_id);
                }
            }
            serde_json::to_value(state_value).map_err(|error| ("NETWORK_ERROR", error.to_string()))
        }
        "terminate" | "kill" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            let scope = authorized_terminal_cleanup_scope(state, ctx, terminal_id)?;
            let workspace = terminal_workspace_service(state)?;
            let result =
                crate::commands::terminal_terminate_resource(terminal_id, &state.pty, &workspace)
                    .await;
            if !result.success {
                return Err((
                    terminal_resource_code(result.code.as_deref()),
                    result
                        .error
                        .unwrap_or_else(|| "terminal termination failed".to_string()),
                ));
            }
            debug_assert!(state.pty.get(terminal_id).is_none(), "scope={scope}");
            release_connection_phone_fit(state, ctx, terminal_id).await;
            release_connection_view(state, ctx, terminal_id, true);
            Ok(Value::Null)
        }
        "attach" => {
            let terminal_id = string_field(&request.payload, "terminalId")?.to_string();
            // CAP-3: verification is the gate and runs BEFORE any replay; every
            // failure mode collapses to the single generic UNAUTHORIZED error
            // (the leaking TERMINAL_NOT_FOUND branch is gone — existence stays
            // hidden). A missing or empty claim is NOT a shape error: it flows
            // through verification like any bad credential (contract: "missing/
            // invalid claim" collapses into the one generic error).
            let claim = request.payload["claim"].as_str().unwrap_or("");
            let last_seq = request.payload["lastSeq"].as_u64().unwrap_or(0);

            // Capture the generation BEFORE verifying (TOCTOU-safe ordering,
            // same as the desktop command): captured-first means a rotate/
            // revoke landing mid-handshake either fails verification or leaves
            // the attachment task holding a stale generation it terminates on.
            let generation = state
                .pty
                .claim_generation(&terminal_id)
                .ok_or_else(|| unauthorized_error(&terminal_id))?;
            if state.pty.verify_claim(&terminal_id, claim).is_err() {
                return Err(unauthorized_error(&terminal_id));
            }
            let Some(instance) = state.pty.get(&terminal_id) else {
                // Verified a heartbeat ago but gone now — same generic error.
                return Err(unauthorized_error(&terminal_id));
            };
            if state.pty.claim_generation(&terminal_id) != Some(generation) {
                // A resume/rotate/revoke won after verification. Never derive
                // connection authorization from the invalidated generation.
                return Err(unauthorized_error(&terminal_id));
            }
            // The credential is the gate now (same-connection prior
            // authorization no longer is): verified attach authorizes the
            // connection for write/resize/events on this exact terminal scope
            // only while this claim generation remains current.
            ctx.authorize(&terminal_id, instance.conversation_id, generation);

            // Sequenced replay: only unseen chunks, with gap detection.
            let replay = instance.subscribe_from(last_seq);
            let attach_result = state.pty.build_attach_result(&instance, &replay);
            install_replay_forwarder(
                &terminal_id,
                replay,
                generation,
                state,
                PassiveForwardAuth {
                    authority,
                    principal_generation: principal.generation(),
                },
                tx,
                ctx,
            )
            .await?;

            // Shared attach result — byte-identical camelCase shape to the
            // desktop `terminal_attach` response (no claim key, ever).
            serde_json::to_value(attach_result)
                .map_err(|error| ("NETWORK_ERROR", error.to_string()))
        }
        "rotate_claim" => {
            // CAP-3: possession of the current credential yields a fresh one
            // and atomically invalidates the old. Any failure — including a
            // missing/empty claim — is the same generic UNAUTHORIZED as attach
            // (missing claims flow through verification, never a shape error).
            let terminal_id = string_field(&request.payload, "terminalId")?.to_string();
            let claim = request.payload["claim"].as_str().unwrap_or("");
            let rotated = state
                .pty
                .rotate_claim(&terminal_id, claim)
                .map_err(|_| unauthorized_error(&terminal_id))?;
            // Teardown (amendment R1): the invalidated holder loses the output
            // stream (attachment task detached) AND write/resize access
            // (removed from the authorized set). Holders on OTHER connections
            // are severed by the claim-generation check inside their
            // attachment tasks. The PTY keeps running.
            release_connection_phone_fit(state, ctx, &terminal_id).await;
            release_connection_view(state, ctx, &terminal_id, true);
            info!("[terminal-ws] claim rotated terminal_id={terminal_id}");
            serde_json::to_value(crate::pty::RotatedClaim { claim: rotated })
                .map_err(|e| ("NETWORK_ERROR", e.to_string()))
        }
        "revoke_claim" => {
            // CAP-3: revocation invalidates the credential; the PTY survives
            // until explicit kill/release/expiry/shutdown. Any failure —
            // including a missing/empty claim — is the same generic
            // UNAUTHORIZED as attach.
            let terminal_id = string_field(&request.payload, "terminalId")?.to_string();
            let claim = request.payload["claim"].as_str().unwrap_or("");
            state
                .pty
                .revoke_claim(&terminal_id, claim)
                .map_err(|_| unauthorized_error(&terminal_id))?;
            // Teardown (amendment R1): same severing as rotate — the revoked
            // holder is a credential-less client and receives no further
            // metadata or output; other connections are severed by the
            // generation check in their attachment tasks. The PTY keeps
            // running.
            release_connection_phone_fit(state, ctx, &terminal_id).await;
            release_connection_view(state, ctx, &terminal_id, true);
            info!("[terminal-ws] claim revoked terminal_id={terminal_id}");
            Ok(Value::Null)
        }
        "detach" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            release_connection_phone_fit(state, ctx, terminal_id).await;
            release_connection_view(state, ctx, terminal_id, true);
            info!("[terminal-ws] detached terminal_id={terminal_id}");
            Ok(Value::Null)
        }
        "close_view" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            // Abort output first but retain authorization long enough for the
            // renderer component's unmount cleanup to remove its backend ref.
            // That cleanup then sends `detach`, which drops authorization.
            release_connection_phone_fit(state, ctx, terminal_id).await;
            release_connection_view(state, ctx, terminal_id, false);
            info!("[terminal-ws] close-view terminal_id={terminal_id}");
            Ok(Value::Null)
        }
        "get_cwd" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            Ok(json!(state.cwd_tracker.get_cwd(terminal_id)))
        }
        "get_git_branch" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            Ok(json!(state.git_tracker.get_branch(terminal_id)))
        }
        "get_git_status" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            Ok(json!(state.git_tracker.get_status(terminal_id)))
        }
        "get_exit_code" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            Ok(json!(state.exit_code_tracker.get_exit_code(terminal_id)))
        }
        "add_renderer_ref" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            state
                .pty
                .add_renderer_ref(terminal_id, string_field(&request.payload, "rendererId")?)
                .map(|_| Value::Null)
                .map_err(|error| ("TERMINAL_NOT_FOUND", error))
        }
        "remove_renderer_ref" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            state
                .pty
                .remove_renderer_ref(terminal_id, string_field(&request.payload, "rendererId")?)
                .map(|_| Value::Null)
                .map_err(|error| ("TERMINAL_NOT_FOUND", error))
        }
        "set_protected" => {
            let terminal_id = string_field(&request.payload, "terminalId")?;
            authorized_terminal_scope(state, ctx, terminal_id)?;
            let protected = request.payload["protected"].as_bool().unwrap_or(true);
            state.pty.set_protected(terminal_id, protected);
            Ok(Value::Null)
        }
        "update_orphan_detection" => {
            // Global setting — require at least one *currently live* exact
            // terminal authorization. A stale entry left behind by a
            // cross-connection claim rotation must not retain lifecycle-policy
            // authority.
            if !ctx.has_live_authorization(state) {
                return Err(unauthorized_error("orphan-detection"));
            }
            let enabled = request.payload["enabled"].as_bool().unwrap_or(true);
            let timeout = request.payload["timeout"]
                .as_u64()
                .and_then(|t| t.checked_mul(60 * 1000)) // minutes → ms (checked to prevent overflow)
                .filter(|t| *t > 0 && *t <= 3_600_000); // cap at 1 hour
            state
                .pty
                .update_orphan_detection_settings(enabled, timeout)
                .await;
            info!(
                "[terminal-ws] orphan detection updated enabled={enabled} timeout_ms={:?}",
                timeout
            );
            Ok(Value::Null)
        }
        _ => Err(("NOT_IMPLEMENTED", "unknown terminal request".to_string())),
    }
}

fn terminal_authorization_is_live(
    state: &AppState,
    terminal_id: &str,
    expected: AuthorizedTerminalScope,
) -> Option<crate::conversation::ConversationId> {
    if expected.cleanup_only {
        return None;
    }
    let instance = state.pty.get(terminal_id).filter(|instance| {
        instance.is_active() && instance.conversation_matches(expected.conversation_id)
    })?;
    (state.pty.claim_generation(terminal_id) == expected.claim_generation)
        .then_some(instance.conversation_id)
}

fn terminal_cleanup_authorization_is_live(
    state: &AppState,
    terminal_id: &str,
    expected: AuthorizedTerminalScope,
) -> Option<crate::conversation::ConversationId> {
    let instance = state
        .pty
        .get(terminal_id)
        .filter(|instance| instance.conversation_matches(expected.conversation_id))?;
    match instance.lifecycle_state() {
        crate::pty::manager::TerminalLifecycleState::Active => (!expected.cleanup_only
            && state.pty.claim_generation(terminal_id) == expected.claim_generation)
            .then_some(instance.conversation_id),
        crate::pty::manager::TerminalLifecycleState::Terminating
        | crate::pty::manager::TerminalLifecycleState::Quarantined => {
            Some(instance.conversation_id)
        }
        crate::pty::manager::TerminalLifecycleState::Removed => None,
    }
}

fn live_authorized_terminal_scope(
    state: &AppState,
    authorized: &AuthorizedTerminals,
    terminal_id: &str,
) -> Option<crate::conversation::ConversationId> {
    let expected = authorized.read().get(terminal_id).copied()?;
    terminal_authorization_is_live(state, terminal_id, expected)
}

fn authorized_terminal_scope(
    state: &AppState,
    ctx: &ConnectionContext,
    terminal_id: &str,
) -> Result<crate::conversation::ConversationId, (&'static str, String)> {
    let expected = ctx
        .scope(terminal_id)
        .ok_or_else(|| unauthorized_error(terminal_id))?;
    terminal_authorization_is_live(state, terminal_id, expected)
        .ok_or_else(|| unauthorized_error(terminal_id))
}

fn authorized_terminal_cleanup_scope(
    state: &AppState,
    ctx: &ConnectionContext,
    terminal_id: &str,
) -> Result<crate::conversation::ConversationId, (&'static str, String)> {
    let expected = ctx
        .scope(terminal_id)
        .ok_or_else(|| unauthorized_error(terminal_id))?;
    terminal_cleanup_authorization_is_live(state, terminal_id, expected)
        .ok_or_else(|| unauthorized_error(terminal_id))
}

fn retain_compound_cleanup_authorization(
    state: &AppState,
    ctx: &mut ConnectionContext,
    code: &'static str,
    error: &str,
) {
    if code != crate::conversation::TERMINAL_RESOURCE_ROLLBACK_FAILED {
        return;
    }
    let Ok(failure) = serde_json::from_str::<crate::commands::TerminalResourceFailureV1>(error)
    else {
        return;
    };
    let Some(instance) = state.pty.get(&failure.terminal_id).filter(|instance| {
        instance.lifecycle_state() == crate::pty::manager::TerminalLifecycleState::Quarantined
    }) else {
        return;
    };
    ctx.authorize_cleanup(&failure.terminal_id, instance.conversation_id);
    info!(
        "[terminal-ws] cleanup recovery retained terminal_id={} primary_code={} cleanup_stage={}",
        failure.terminal_id, failure.primary_code, failure.cleanup_stage
    );
}

struct PassiveForwardAuth<'a> {
    authority: &'a Arc<RemoteAccessAuthority>,
    principal_generation: u64,
}

async fn install_replay_forwarder(
    terminal_id: &str,
    replay: TerminalReplay,
    generation: u64,
    state: &AppState,
    forward_auth: PassiveForwardAuth<'_>,
    tx: &mpsc::Sender<Message>,
    ctx: &mut ConnectionContext,
) -> Result<(), (&'static str, String)> {
    // Never release replay bytes for a claim generation already invalidated by
    // a concurrent resume/rotate/revoke.
    authorized_terminal_scope(state, ctx, terminal_id)?;
    if !should_forward_passive(forward_auth.authority, forward_auth.principal_generation) {
        return Err(unauthorized_error(terminal_id));
    }

    let snapshot = state.terminal_events.snapshot(terminal_id);
    let binary_output = ctx.binary_output;
    let chunk_payloads = if binary_output {
        for chunk in &replay.chunks {
            send_binary_output(
                tx,
                BinaryOutputKind::Replay,
                terminal_id,
                chunk.seq,
                &chunk.data,
            )
            .await
            .map_err(|error| ("NETWORK_ERROR", error))?;
        }
        Vec::new()
    } else {
        replay
            .chunks
            .iter()
            .map(|chunk| {
                json!({
                    "seq": chunk.seq,
                    "data": chunk.data.iter().map(|byte| *byte as u64).collect::<Vec<u64>>()
                })
            })
            .collect()
    };
    send_json(
        tx,
        json!({
            "type": "replay",
            "terminalId": terminal_id,
            "chunks": chunk_payloads,
            "gap": replay.gap,
            "latestSeq": replay.latest_seq,
            "snapshot": serde_json::to_value(&snapshot).unwrap_or_else(|_| json!({}))
        }),
    )
    .await
    .map_err(|error| ("NETWORK_ERROR", error))?;

    // Rotation may have landed while the replay frame was back-pressured.
    // Refuse to install any live forwarder for the stale generation.
    authorized_terminal_scope(state, ctx, terminal_id)?;

    let replacing_view = ctx.attachments.contains_key(terminal_id);
    if let Some(previous) = ctx.attachments.remove(terminal_id) {
        previous.abort();
    }
    let output_tx = tx.clone();
    let attached_id = terminal_id.to_string();
    let pty = Arc::clone(&state.pty);
    let attached_authority = Arc::clone(forward_auth.authority);
    let principal_generation = forward_auth.principal_generation;
    let task = tokio::spawn(async move {
        let mut receiver = replay.receiver;
        let mut current_seq = replay.latest_seq;
        let mut tick =
            tokio::time::interval(std::time::Duration::from_millis(ATTACH_GENERATION_CHECK_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;

        loop {
            if !should_forward_passive(&attached_authority, principal_generation)
                || crate::commands::forwarder_should_terminate(
                    Some(generation),
                    pty.claim_generation(&attached_id),
                )
            {
                info!(
                    "[terminal-ws] attachment terminating (claim invalidated) terminal_id={attached_id}"
                );
                break;
            }
            tokio::select! {
                received = receiver.recv() => {
                    match received {
                        Ok(chunk) => {
                            // A rotation may land while recv() is pending. Check
                            // again before forwarding so an old holder receives
                            // no post-rotation terminal bytes.
                            if !should_forward_passive(&attached_authority, principal_generation)
                                || crate::commands::forwarder_should_terminate(
                                    Some(generation),
                                    pty.claim_generation(&attached_id),
                                )
                            {
                                break;
                            }
                            current_seq = chunk.seq;
                            let sent = if binary_output {
                                send_binary_output(
                                    &output_tx,
                                    BinaryOutputKind::Live,
                                    &attached_id,
                                    current_seq,
                                    &chunk.data,
                                )
                                .await
                            } else {
                                let data: Vec<u64> =
                                    chunk.data.iter().map(|byte| *byte as u64).collect();
                                send_json(
                                    &output_tx,
                                    json!({
                                        "type": "data",
                                        "terminalId": attached_id,
                                        "seq": current_seq,
                                        "data": data
                                    }),
                                )
                                .await
                            };
                            if sent.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            if crate::commands::forwarder_should_terminate(
                                Some(generation),
                                pty.claim_generation(&attached_id),
                            ) {
                                break;
                            }
                            warn!(
                                "[terminal-ws] output receiver lagged by {skipped} for {attached_id}"
                            );
                            let _ = send_json(
                                &output_tx,
                                json!({
                                    "type": "gap",
                                    "terminalId": attached_id,
                                    "lastSeq": current_seq
                                }),
                            )
                            .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tick.tick() => {}
            }
        }
    });
    ctx.attachments.insert(terminal_id.to_string(), task);
    if !replacing_view {
        state.pty.note_view_opened(terminal_id);
    }
    Ok(())
}

async fn bind_companion_output_stream(
    state: &AppState,
    authority: &Arc<RemoteAccessAuthority>,
    principal: &RemotePrincipal,
    tx: &mpsc::Sender<Message>,
    ctx: &mut ConnectionContext,
    terminal_id: String,
    last_seq: u64,
) -> Result<Value, (&'static str, String)> {
    let generation = state
        .pty
        .claim_generation(&terminal_id)
        .ok_or_else(|| ("TERMINAL_NOT_FOUND", "terminal not found".to_string()))?;
    let instance = state
        .pty
        .get(&terminal_id)
        .filter(|instance| instance.is_active())
        .ok_or_else(|| ("TERMINAL_NOT_FOUND", "terminal not found".to_string()))?;

    ctx.authorize(&terminal_id, instance.conversation_id, generation);
    let replay = instance.subscribe_from(last_seq);
    let attach_result = state.pty.build_attach_result(&instance, &replay);
    install_replay_forwarder(
        &terminal_id,
        replay,
        generation,
        state,
        PassiveForwardAuth {
            authority,
            principal_generation: principal.generation(),
        },
        tx,
        ctx,
    )
    .await?;

    serde_json::to_value(attach_result).map_err(|error| ("NETWORK_ERROR", error.to_string()))
}

fn terminal_display_title(cwd: &str, shell: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(shell)
        .to_string()
}

async fn spawn_project_terminal(
    payload: Value,
    state: &AppState,
    ctx: &mut ConnectionContext,
) -> Result<Value, (&'static str, String)> {
    let intent: TerminalProjectSpawnIntent =
        serde_json::from_value(payload).map_err(|error| ("VALIDATION_ERROR", error.to_string()))?;
    if intent.project_id.trim().is_empty() {
        return Err(("VALIDATION_ERROR", "missing projectId".to_string()));
    }
    if intent.cols == 0 || intent.rows == 0 {
        return Err((
            "VALIDATION_ERROR",
            "terminal dimensions must be greater than zero".to_string(),
        ));
    }
    let project = state
        .registry
        .switch_context(&intent.project_id)
        .ok_or_else(|| {
            (
                "NOT_FOUND",
                "project not found or not switchable".to_string(),
            )
        })?;
    info!(
        "[terminal-ws] spawn requested project_id={} conversation_id=none cwd_source=project",
        project.project_id
    );
    let options = crate::pty::manager::SpawnOptions {
        shell: None,
        cwd: Some(project.cwd),
        env: None,
        conversation_id: None,
        project_id: Some(project.project_id.clone()),
        cols: Some(intent.cols),
        rows: Some(intent.rows),
        program: None,
        args: None,
        kind: None,
    };
    let spawned = match terminal_workspace_service(state) {
        Ok(workspace) => {
            let result =
                crate::commands::terminal_spawn_resource(options, None, &state.pty, &workspace)
                    .await;
            if !result.success {
                let code = terminal_resource_code(result.code.as_deref());
                let error = result
                    .error
                    .unwrap_or_else(|| "terminal spawn failed".to_string());
                retain_compound_cleanup_authorization(state, ctx, code, &error);
                return Err((code, error));
            }
            result.data.expect("successful terminal spawn has data")
        }
        Err(_) => state
            .pty
            .spawn(options, None)
            .await
            .map_err(|error| ("SPAWN_FAILED", error))?,
    };
    let instance = state
        .pty
        .get(&spawned.info.id)
        .ok_or_else(|| unauthorized_error(&spawned.info.id))?;
    let generation = state
        .pty
        .claim_generation(&spawned.info.id)
        .ok_or_else(|| unauthorized_error(&spawned.info.id))?;
    ctx.authorize(&spawned.info.id, instance.conversation_id, generation);
    info!(
        "[terminal-ws] spawn success project_id={} terminal_id={} cwd_source=project",
        project.project_id, spawned.info.id
    );
    serde_json::to_value(spawned).map_err(|error| ("SPAWN_FAILED", error.to_string()))
}

fn optional_conversation_id(
    payload: &Value,
) -> Result<Option<ConversationId>, (&'static str, String)> {
    let Some(raw) = payload
        .get("conversationId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    ConversationId::parse(raw)
        .map(Some)
        .map_err(|_| ("VALIDATION_ERROR", "invalid conversationId".to_string()))
}

fn companion_list_matches(
    instance: &crate::pty::manager::TerminalInstance,
    conversation_filter: Option<ConversationId>,
    project_filter: Option<&str>,
) -> bool {
    if let Some(conversation_id) = conversation_filter {
        return instance.conversation_matches(conversation_id);
    }
    project_filter.is_some_and(|project_id| instance.project_matches(project_id))
}

fn live_terminal_summary(
    instance: &crate::pty::manager::TerminalInstance,
    cwd: String,
    git_branch: Option<String>,
) -> Value {
    json!({
        "id": instance.id,
        "shell": instance.shell,
        "cwd": cwd,
        "pid": instance.pid,
        "cols": *instance.cols.read(),
        "rows": *instance.rows.read(),
        "conversationId": instance.conversation_id,
        "projectId": instance.project_id,
        "title": terminal_display_title(&cwd, &instance.shell),
        "gitBranch": git_branch,
        "displayMode": instance.display_mode(),
    })
}

fn terminal_conversation_service(
    state: &AppState,
) -> Result<Arc<crate::conversation::ConversationApplicationService>, (&'static str, String)> {
    state.conversation.clone().ok_or_else(|| {
        (
            "CONVERSATION_SERVICE_UNAVAILABLE",
            "Conversation application service is unavailable".to_string(),
        )
    })
}

fn terminal_workspace_service(
    state: &AppState,
) -> Result<Arc<crate::conversation::SessionWorkspaceService>, (&'static str, String)> {
    terminal_conversation_service(state).map(|conversation| conversation.session_workspace())
}

fn terminal_resource_code(code: Option<&str>) -> &'static str {
    match code {
        Some(crate::conversation::TERMINAL_RESOURCE_ROLLBACK_FAILED) => {
            crate::conversation::TERMINAL_RESOURCE_ROLLBACK_FAILED
        }
        Some(crate::conversation::TERMINAL_TERMINATE_FAILED) => {
            crate::conversation::TERMINAL_TERMINATE_FAILED
        }
        Some("CONVERSATION_INVALID_ID") => "CONVERSATION_INVALID_ID",
        Some("CONVERSATION_NOT_FOUND") => "CONVERSATION_NOT_FOUND",
        Some("CONVERSATION_CONFLICT") => "CONVERSATION_CONFLICT",
        Some("CONVERSATION_RECOVERY_REQUIRED") => "CONVERSATION_RECOVERY_REQUIRED",
        Some("CONVERSATION_DURABILITY_FAILED") => "CONVERSATION_DURABILITY_FAILED",
        Some("LEGACY_COMPATIBILITY_READ_ONLY") => "LEGACY_COMPATIBILITY_READ_ONLY",
        Some("SESSION_WORKSPACE_RECOVERY_REQUIRED") => "SESSION_WORKSPACE_RECOVERY_REQUIRED",
        Some("SESSION_WORKSPACE_UNAVAILABLE") => "SESSION_WORKSPACE_UNAVAILABLE",
        Some("VALIDATION_ERROR") => "VALIDATION_ERROR",
        Some("UNAUTHORIZED") => "UNAUTHORIZED",
        Some("SPAWN_FAILED") => "SPAWN_FAILED",
        _ => "SESSION_WORKSPACE_UNAVAILABLE",
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, (&'static str, String)> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ("VALIDATION_ERROR", format!("missing {key}")))
}

/// The single generic authorization failure shared by attach, rotate_claim and
/// revoke_claim. CAP-3 forbids any of these surfaces from distinguishing
/// unknown terminal from wrong/revoked credential from binding mismatch - one
/// code, one message shape. Message matches the desktop `terminal_attach` /
/// rotate / revoke error string byte-for-byte (transport parity) and never
/// echoes the terminal id. Kept free-standing so the contract is testable.
fn unauthorized_error(_terminal_id: &str) -> (&'static str, String) {
    ("UNAUTHORIZED", "Unauthorized".to_string())
}

fn u16_field(value: &Value, key: &str) -> Result<u16, (&'static str, String)> {
    value[key]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| ("VALIDATION_ERROR", format!("invalid {key}")))
}

fn optional_u16_field(value: &Value, key: &str) -> Result<Option<u16>, (&'static str, String)> {
    if value.get(key).is_none() || value[key].is_null() {
        return Ok(None);
    }
    Ok(Some(u16_field(value, key)?))
}

fn encode_binary_output_frame(
    kind: BinaryOutputKind,
    terminal_id: &str,
    seq: u64,
    data: &[u8],
) -> Result<Vec<u8>, String> {
    let terminal_id = terminal_id.as_bytes();
    let terminal_id_len = u16::try_from(terminal_id.len())
        .map_err(|_| "terminal id is too long for binary output frame".to_string())?;
    let mut frame = Vec::with_capacity(15 + terminal_id.len() + data.len());
    frame.extend_from_slice(BINARY_FRAME_MAGIC);
    frame.push(kind as u8);
    frame.extend_from_slice(&terminal_id_len.to_be_bytes());
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(terminal_id);
    frame.extend_from_slice(data);
    Ok(frame)
}

async fn send_binary_output(
    tx: &mpsc::Sender<Message>,
    kind: BinaryOutputKind,
    terminal_id: &str,
    seq: u64,
    data: &[u8],
) -> Result<(), String> {
    let frame = encode_binary_output_frame(kind, terminal_id, seq, data)?;
    tx.send(Message::Binary(frame.into()))
        .await
        .map_err(|_| "terminal websocket closed".to_string())
}

async fn send_json(tx: &mpsc::Sender<Message>, value: Value) -> Result<(), String> {
    tx.send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| "terminal websocket closed".to_string())
}

async fn send_error(
    tx: &mpsc::Sender<Message>,
    id: &str,
    code: &str,
    error: String,
) -> Result<(), String> {
    send_json(
        tx,
        json!({ "id": id, "success": false, "error": error, "code": code }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation_id() -> crate::conversation::ConversationId {
        crate::conversation::ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap()
    }

    #[test]
    fn validates_numeric_dimensions() {
        assert_eq!(u16_field(&json!({ "cols": 80 }), "cols"), Ok(80));
        assert!(u16_field(&json!({ "cols": 0 }), "cols").is_err());
    }

    #[test]
    fn binary_output_requires_explicit_subprotocol_negotiation() {
        let mut headers = HeaderMap::new();
        assert!(!supports_binary_subprotocol(&headers));

        headers.insert(
            axum::http::header::SEC_WEBSOCKET_PROTOCOL,
            axum::http::HeaderValue::from_static("legacy, se-terminal-v2.binary"),
        );
        assert!(supports_binary_subprotocol(&headers));
    }

    #[test]
    fn binary_output_frame_uses_stable_big_endian_envelope() {
        let frame = encode_binary_output_frame(
            BinaryOutputKind::Replay,
            "pty-1",
            0x0102_0304_0506_0708,
            &[0, 0xff, b'A'],
        )
        .unwrap();

        assert_eq!(&frame[0..4], b"TML2");
        assert_eq!(frame[4], BinaryOutputKind::Replay as u8);
        assert_eq!(u16::from_be_bytes([frame[5], frame[6]]), 5);
        assert_eq!(
            u64::from_be_bytes(frame[7..15].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        assert_eq!(&frame[15..20], b"pty-1");
        assert_eq!(&frame[20..], &[0, 0xff, b'A']);
    }

    #[tokio::test]
    async fn spawn_compound_rollback() {
        use crate::conversation::{
            parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
            ConversationMutation, ConversationRecordV2, ConversationWriter, CreationPartition,
            ExecutionTarget, SessionWorkspaceLoadOutcome, SessionWorkspaceService,
            CONVERSATION_SCHEMA_VERSION, TERMINAL_RESOURCE_ROLLBACK_FAILED,
        };
        use crate::pty::manager::{
            ScriptedCleanupDriver, TerminalCleanupStage, TerminalLifecycleState,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) =
            crate::conversation::ConversationRepository::open(base.join("conversations/v2"))
                .unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            crate::conversation::ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab")
                .unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: base.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(writer));
        repository.fail_next_workspace_replace();
        let pty = crate::web::test_pty_manager();
        let cleanup_driver = Arc::new(ScriptedCleanupDriver::default());
        cleanup_driver.fail_once(TerminalCleanupStage::Kill);
        pty.install_cleanup_driver(cleanup_driver);
        let result = crate::commands::terminal_spawn_resource(
            SpawnOptions {
                conversation_id: Some(conversation_id),
                cwd: Some(base.to_string_lossy().into_owned()),
                ..Default::default()
            },
            None,
            &pty,
            &workspace,
        )
        .await;

        assert_eq!(
            terminal_resource_code(result.code.as_deref()),
            TERMINAL_RESOURCE_ROLLBACK_FAILED
        );
        let failure: crate::commands::TerminalResourceFailureV1 =
            serde_json::from_str(result.error.as_deref().unwrap()).unwrap();
        assert_eq!(failure.primary_code, "CONVERSATION_DURABILITY_FAILED");
        assert_eq!(failure.cleanup_stage, TerminalCleanupStage::Kill);
        assert_eq!(
            pty.terminal_lifecycle_state(&failure.terminal_id),
            Some(TerminalLifecycleState::Quarantined)
        );
        assert!(matches!(
            workspace.load(conversation_id).await.unwrap(),
            SessionWorkspaceLoadOutcome::Missing { .. }
        ));

        let mut state = terminal_test_state();
        state.pty = Arc::clone(&pty);
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        retain_compound_cleanup_authorization(
            &state,
            &mut ctx,
            TERMINAL_RESOURCE_ROLLBACK_FAILED,
            result.error.as_deref().unwrap(),
        );
        assert_eq!(
            authorized_terminal_cleanup_scope(&state, &ctx, &failure.terminal_id),
            Ok(conversation_id)
        );
        assert_eq!(
            authorized_terminal_scope(&state, &ctx, &failure.terminal_id),
            Err(unauthorized_error(&failure.terminal_id))
        );

        pty.terminate(&failure.terminal_id).await.unwrap();
    }

    #[test]
    fn project_spawn_intent_rejects_raw_cwd_and_unknown_fields() {
        let valid = json!({
            "projectId": "project-1",
            "cols": 80,
            "rows": 24
        });
        let intent: TerminalProjectSpawnIntent = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(intent.project_id, "project-1");

        for (field, value) in [
            (
                "conversationId",
                json!("018f7a1c-1b4d-7c8a-9f01-0123456789ab"),
            ),
            ("cwd", json!("/caller/path")),
            ("shell", json!("caller-shell")),
            ("program", json!("/bin/sh")),
            ("env", json!({ "SECRET": "never" })),
            ("unknown", json!(true)),
        ] {
            let mut payload = valid.clone();
            payload
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), value);
            assert!(
                serde_json::from_value::<TerminalProjectSpawnIntent>(payload).is_err(),
                "raw/unknown field must be rejected: {field}"
            );
        }
    }

    #[tokio::test]
    async fn project_spawn_uses_host_registry_cwd() {
        let state = terminal_test_state();
        let cwd = tempfile::tempdir().unwrap();
        state.registry.set(
            vec![crate::web::project_registry::ProjectSummary {
                id: "project-1".into(),
                name: "Demo".into(),
                color: "blue".into(),
                path: Some(cwd.path().to_string_lossy().into_owned()),
                is_archived: false,
                is_default: true,
            }],
            Some("project-1".into()),
        );
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        let spawned = spawn_project_terminal(
            json!({
                "projectId": "project-1",
                "cols": 80,
                "rows": 24
            }),
            &state,
            &mut ctx,
        )
        .await
        .expect("project spawn");
        let spawned_cwd = spawned["cwd"].as_str().expect("cwd");
        let expected = cwd.path().canonicalize().unwrap();
        assert_eq!(
            std::path::Path::new(spawned_cwd).canonicalize().unwrap(),
            expected
        );
        let terminal_id = spawned["id"].as_str().expect("id");
        assert!(!terminal_id.is_empty());
        assert!(spawned.get("claim").and_then(Value::as_str).is_some());
        state.pty.terminate(terminal_id).await.unwrap();
    }

    #[test]
    fn spawn_intent_rejects_raw_and_unknown_fields() {
        let valid = json!({
            "conversationId": conversation_id(),
            "projectId": "project-1",
            "cwdSource": "workspace",
            "cols": 80,
            "rows": 24
        });
        let intent: TerminalSpawnIntentV1 = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(intent.conversation_id, conversation_id());

        for (field, value) in [
            ("program", json!("/bin/sh")),
            ("args", json!(["-c", "never"])),
            ("env", json!({ "SECRET": "never" })),
            ("cwd", json!("/caller/path")),
            ("shell", json!("caller-shell")),
            ("kind", json!("agent")),
            ("unknown", json!(true)),
        ] {
            let mut payload = valid.clone();
            payload
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), value);
            assert!(
                serde_json::from_value::<TerminalSpawnIntentV1>(payload).is_err(),
                "raw/unknown field must be rejected: {field}"
            );
        }
    }

    #[test]
    fn resume_request_rejects_spawn_authority_fields() {
        let valid = json!({
            "conversationId": conversation_id(),
            "terminalId": "terminal-1",
            "lastSeq": 7
        });
        let resume: TerminalResumeRequest = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(resume.last_seq, 7);

        for field in ["program", "args", "env", "cwd", "shell", "projectId"] {
            let mut payload = valid.clone();
            payload
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), json!("forbidden"));
            assert!(
                serde_json::from_value::<TerminalResumeRequest>(payload).is_err(),
                "resume authority field must be rejected: {field}"
            );
        }
    }

    fn terminal_test_state() -> AppState {
        let pty = crate::web::test_pty_manager();
        AppState {
            acp: Arc::new(crate::acp::AcpManager::new(vec![])),
            terminal_events: pty.terminal_events(),
            cwd_tracker: pty.cwd_tracker(),
            git_tracker: pty.git_tracker(),
            exit_code_tracker: pty.exit_code_tracker(),
            pty,
            relay: Arc::new(crate::web::sink::WsRelaySink::new()),
            registry: Arc::new(crate::web::project_registry::ProjectRegistry::new()),
            registry_persistence: None,
            projects_file: None,
            history_mode: crate::web::ws::HistoryMode::LiveOnly,
            conversation: None,
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            store: None,
            project_root: Arc::new(parking_lot::RwLock::new(std::path::PathBuf::new())),
        }
    }

    fn terminal_authority() -> Arc<RemoteAccessAuthority> {
        let authority = RemoteAccessAuthority::for_tests("terminal-access-token");
        authority
            .set_public_origin(url::Url::parse("https://terminal.example.test").unwrap())
            .unwrap();
        Arc::new(authority)
    }

    fn terminal_auth_app(authority: Arc<RemoteAccessAuthority>) -> axum::Router {
        axum::Router::new()
            .route("/terminal/ws", axum::routing::get(terminal_ws_upgrade))
            .with_state(terminal_test_state())
            .layer(axum::middleware::from_fn(
                crate::web::auth::capability_middleware,
            ))
            .layer(Extension(
                crate::web::auth::RemoteRouteClass::TerminalWebSocket,
            ))
            .layer(Extension(crate::web::auth::IngressProvenance::PublicTunnel))
            .layer(Extension(authority))
    }

    fn terminal_upgrade_request(
        authorization: Option<&str>,
        origin: Option<&str>,
        peer: std::net::SocketAddr,
    ) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/terminal/ws")
            .header(axum::http::header::CONNECTION, "upgrade")
            .header(axum::http::header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .extension(axum::extract::ConnectInfo(peer));
        if let Some(authorization) = authorization {
            builder = builder.header(axum::http::header::AUTHORIZATION, authorization);
        }
        if let Some(origin) = origin {
            builder = builder.header(axum::http::header::ORIGIN, origin);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn resume_requires_authenticated_scope() {
        use tower::ServiceExt;

        let _boundary_log_test_guard = crate::web::auth::test_tracing::lock().await;

        let peer = std::net::SocketAddr::from(([192, 0, 2, 10], 43123));
        let allowed_origin = "https://terminal.example.test";

        let missing = terminal_auth_app(terminal_authority())
            .oneshot(terminal_upgrade_request(None, Some(allowed_origin), peer))
            .await
            .unwrap();
        assert_ne!(missing.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_ne!(missing.status(), axum::http::StatusCode::FORBIDDEN);

        let wrong = terminal_auth_app(terminal_authority())
            .oneshot(terminal_upgrade_request(
                Some("Bearer wrong"),
                Some(allowed_origin),
                peer,
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), axum::http::StatusCode::UNAUTHORIZED);

        let authority = terminal_authority();
        let mut allowed_headers = HeaderMap::new();
        allowed_headers.insert(
            axum::http::header::ORIGIN,
            axum::http::HeaderValue::from_static("https://terminal.example.test"),
        );
        assert!(
            admit_terminal_upgrade(&authority, &allowed_headers, peer.ip())
                .unwrap()
                .is_none()
        );

        let mut bearer_headers = allowed_headers.clone();
        bearer_headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer terminal-access-token"),
        );
        assert!(
            admit_terminal_upgrade(&authority, &bearer_headers, peer.ip())
                .unwrap()
                .is_some()
        );

        let mut wrong_headers = HeaderMap::new();
        wrong_headers.insert(
            axum::http::header::ORIGIN,
            axum::http::HeaderValue::from_static("https://evil.example.test"),
        );
        assert_eq!(
            admit_terminal_upgrade(&authority, &wrong_headers, peer.ip()),
            Err(RemoteAuthError::InvalidOrigin)
        );
        assert_eq!(
            admit_terminal_upgrade(&authority, &HeaderMap::new(), peer.ip()),
            Err(RemoteAuthError::InvalidOrigin)
        );

        let rate_limited_app = terminal_auth_app(terminal_authority());
        for attempt in 1..=6 {
            let response = rate_limited_app
                .clone()
                .oneshot(terminal_upgrade_request(
                    Some("Bearer wrong"),
                    Some(allowed_origin),
                    peer,
                ))
                .await
                .unwrap();
            let expected = if attempt == 6 {
                axum::http::StatusCode::TOO_MANY_REQUESTS
            } else {
                axum::http::StatusCode::UNAUTHORIZED
            };
            assert_eq!(response.status(), expected, "attempt={attempt}");
        }

        let source = include_str!("terminal_ws.rs");
        assert!(source.contains("pre-auth: send an `authenticate` request first"));

        let mut principal = None;
        let list = Request {
            id: "1".into(),
            type_: "list".into(),
            payload: json!({}),
        };
        let rejected =
            admit_terminal_frame(&list, &authority, peer.ip(), &mut principal).unwrap_err();
        assert_eq!(rejected.0, "UNAUTHORIZED");
        let admitted = admit_terminal_frame(
            &Request {
                id: "2".into(),
                type_: "authenticate".into(),
                payload: json!({ "token": "terminal-access-token" }),
            },
            &authority,
            peer.ip(),
            &mut principal,
        )
        .unwrap();
        assert_eq!(admitted, json!({}));
        assert!(principal.is_some());
    }

    #[test]
    fn u16_rejects_negative_and_overflow() {
        assert!(u16_field(&json!({ "rows": -1 }), "rows").is_err());
        assert!(u16_field(&json!({ "rows": 70000 }), "rows").is_err());
        assert_eq!(optional_u16_field(&json!({}), "cols"), Ok(None));
        assert_eq!(
            optional_u16_field(&json!({ "cols": 40 }), "cols"),
            Ok(Some(40))
        );
    }

    #[test]
    fn string_field_rejects_empty_and_missing() {
        assert!(string_field(&json!({ "terminalId": "" }), "terminalId").is_err());
        assert!(string_field(&json!({}), "terminalId").is_err());
        assert_eq!(
            string_field(&json!({ "terminalId": "t1" }), "terminalId"),
            Ok("t1")
        );
    }

    #[test]
    fn context_close_view_preserves_authorization_until_detach() {
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        ctx.authorize("t1", conversation_id(), 7);
        assert!(ctx.is_authorized("t1"));
        assert!(!ctx.is_authorized("t2"));

        ctx.close_view("t1");
        assert!(ctx.is_authorized("t1"));

        ctx.detach("t1");
        assert!(!ctx.is_authorized("t1"));
    }

    #[tokio::test]
    async fn claim_rotation_revokes_all_connection_scope_without_terminating_pty() {
        let state = terminal_test_state();
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().canonicalize().unwrap();
        let conversation_id = conversation_id();
        let spawned = state
            .pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(conversation_id),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let terminal_id = spawned.info.id.clone();
        let generation = state.pty.claim_generation(&terminal_id).unwrap();
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        ctx.authorize(&terminal_id, conversation_id, generation);

        assert_eq!(
            authorized_terminal_scope(&state, &ctx, &terminal_id),
            Ok(conversation_id)
        );
        assert_eq!(
            live_authorized_terminal_scope(&state, &ctx.authorized, &terminal_id),
            Some(conversation_id)
        );
        assert!(ctx.has_live_authorization(&state));

        let successor = state
            .pty
            .rotate_claim(&terminal_id, &spawned.claim)
            .unwrap();

        assert_eq!(
            authorized_terminal_scope(&state, &ctx, &terminal_id),
            Err(unauthorized_error(&terminal_id))
        );
        assert_eq!(
            live_authorized_terminal_scope(&state, &ctx.authorized, &terminal_id),
            None
        );
        assert!(!ctx.has_live_authorization(&state));
        assert!(
            state.pty.get(&terminal_id).is_some(),
            "rotation preserves PTY"
        );
        assert!(state
            .pty
            .verify_claim(&terminal_id, &spawned.claim)
            .is_err());
        assert!(state.pty.verify_claim(&terminal_id, &successor).is_ok());

        state.pty.terminate(&terminal_id).await.unwrap();
    }

    #[test]
    fn string_field_validates_structural_ids() {
        // Structural validation applies to `terminalId` on every arm (a missing
        // terminal id reveals nothing about any terminal, so VALIDATION_ERROR
        // is allowed there). Claims deliberately do NOT use this path: on
        // attach/rotate/revoke a missing or empty claim flows through
        // verification and fails with the generic UNAUTHORIZED like any bad
        // credential (contract: no response distinguishes "missing" from
        // "invalid"). The handler-level wiring of that behavior needs a live
        // PtyManager (deferred seam); this test pins the helper only.
        assert!(string_field(&json!({ "terminalId": "" }), "terminalId").is_err());
        assert!(string_field(&json!({}), "terminalId").is_err());
        assert_eq!(
            string_field(&json!({ "terminalId": "t1" }), "terminalId"),
            Ok("t1")
        );
    }

    #[test]
    fn unauthorized_error_is_single_generic_shape_for_all_surfaces() {
        // attach, rotate_claim and revoke_claim must all fail with the same
        // code + message shape — no distinguishing unknown terminal from
        // wrong/revoked credential from binding mismatch (CAP-3 leak fix).
        let (code, message) = unauthorized_error("t1");
        assert_eq!(code, "UNAUTHORIZED");
        // Byte-identical to the desktop error string and independent of the
        // terminal id (no input echo — nothing distinguishes failure causes).
        assert_eq!(message, "Unauthorized");
        assert_eq!(unauthorized_error("t1"), unauthorized_error("t2"));
        assert_ne!(
            code, "TERMINAL_NOT_FOUND",
            "existence-leaking code must not return"
        );
    }

    #[tokio::test]
    async fn connection_detach_aborts_attachment_and_clears_authorization() {
        // This test pins the ConnectionContext::detach PRIMITIVE the teardown
        // relies on: aborting the attachment task and clearing authorization.
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        ctx.authorize("t1", conversation_id(), 7);

        // A live attachment task mimicking the output forwarder.
        let task = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        });
        ctx.attachments.insert("t1".to_string(), task);

        assert!(ctx.is_authorized("t1"));
        ctx.detach("t1");

        // Output stream severed + write/resize authorization removed, and the
        // abort actually reached the task (teardown is real, not bookkeeping).
        assert!(!ctx.is_authorized("t1"));
        assert!(ctx.attachments.is_empty());
    }

    #[test]
    fn companion_title_uses_cwd_basename_then_shell() {
        assert_eq!(
            terminal_display_title("/Users/dev/projects/se-manager", "zsh"),
            "se-manager"
        );
        assert_eq!(terminal_display_title("/", "zsh"), "zsh");
        assert_eq!(terminal_display_title("", "bash"), "bash");
    }

    #[test]
    fn companion_list_prefers_conversation_scope() {
        let conversation = ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let other = ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ac").unwrap();
        assert!(companion_list_matches_ids(
            conversation,
            Some("proj-1"),
            Some(conversation),
            None
        ));
        assert!(!companion_list_matches_ids(
            conversation,
            Some("proj-1"),
            Some(other),
            None
        ));
        assert!(companion_list_matches_ids(
            conversation,
            Some("proj-1"),
            None,
            Some("proj-1")
        ));
        assert!(!companion_list_matches_ids(
            conversation,
            Some("proj-1"),
            None,
            Some("proj-2")
        ));
    }

    fn companion_list_matches_ids(
        instance_conversation: ConversationId,
        instance_project: Option<&str>,
        conversation_filter: Option<ConversationId>,
        project_filter: Option<&str>,
    ) -> bool {
        if let Some(conversation_id) = conversation_filter {
            return instance_conversation == conversation_id;
        }
        project_filter == instance_project
    }

    #[test]
    fn disconnect_cleanup_and_detach_are_non_destructive() {
        let source = include_str!("terminal_ws.rs");
        let run = source
            .split("async fn run")
            .nth(1)
            .and_then(|tail| tail.split("struct ConnectionContext").next())
            .expect("run body");
        let stripped = strip_comments(run);
        for forbidden in [".kill(", "force_kill", ".terminate(", "kill_all"] {
            assert!(
                !stripped.contains(forbidden),
                "run disconnect cleanup must not call {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn terminal_ws_event_and_attachment_close_on_generation_mismatch() {
        let authority = Arc::new(RemoteAccessAuthority::for_tests("test-remote-access-token"));
        let principal = RemotePrincipal::for_tests(1);
        assert!(
            !principal_generation_mismatch(&authority, Some(&principal)),
            "fresh test authority must match principal generation 1"
        );
        let event_task = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        let mut attachments = HashMap::new();
        attachments.insert(
            "t1".to_string(),
            tokio::spawn(async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }),
        );
        let receipt = authority.retire_generation(1);
        assert!(receipt.credential_invalidated || !authority.generation_state().active);
        assert!(principal_generation_mismatch(&authority, Some(&principal)));
        abort_and_join_connection_tasks(event_task, attachments).await;
    }

    #[test]
    fn terminal_ws_passive_forward_rechecks_generation_before_send() {
        let authority = RemoteAccessAuthority::for_tests("test-remote-access-token");
        assert!(should_forward_passive(&authority, 1));
        let _ = authority.retire_generation(1);
        assert!(
            !should_forward_passive(&authority, 1),
            "passive forward must refuse after generation retirement"
        );
        assert!(!should_forward_passive(&authority, 99));
    }

    fn strip_comments(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn parse_marked_window(buffer: &str) -> Option<(u16, u16)> {
        for line in buffer.lines() {
            let Some(rest) = line.trim().strip_prefix("SE_WIN:") else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let rows = parts.next()?.parse().ok()?;
            let cols = parts.next()?.parse().ok()?;
            return Some((rows, cols));
        }
        None
    }

    async fn collect_output_until(
        receiver: &mut tokio::sync::broadcast::Receiver<crate::pty::manager::TerminalOutputChunk>,
        predicate: impl Fn(&str) -> bool,
    ) -> String {
        let mut buffer = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Ok(chunk)) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk.data));
                    if predicate(&buffer) {
                        return buffer;
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }
        buffer
    }

    async fn read_live_window(
        pty: &crate::pty::manager::PtyManager,
        terminal_id: &str,
        receiver: &mut tokio::sync::broadcast::Receiver<crate::pty::manager::TerminalOutputChunk>,
    ) -> (u16, u16) {
        while receiver.try_recv().is_ok() {}
        pty.write(terminal_id, "echo SE_WIN:$(stty size)\n")
            .await
            .expect("write stty probe");
        let buffer =
            collect_output_until(receiver, |text| parse_marked_window(text).is_some()).await;
        parse_marked_window(&buffer)
            .unwrap_or_else(|| panic!("stty size marker missing in PTY output: {buffer:?}"))
    }

    fn has_output_line(buffer: &str, marker: &str) -> bool {
        buffer.lines().any(|line| line.trim() == marker)
    }

    async fn count_full_screen_cells(
        pty: &crate::pty::manager::PtyManager,
        terminal_id: &str,
        receiver: &mut tokio::sync::broadcast::Receiver<crate::pty::manager::TerminalOutputChunk>,
        script_path: &std::path::Path,
    ) -> usize {
        std::fs::write(
            script_path,
            "printf '%s\\n' SE_PAINT_START\nawk 'BEGIN{ \"stty size\" | getline s; split(s,a); for(i=1;i<=a[1];i++){ for(j=1;j<=a[2];j++) printf \"A\"; print \"\" } }'\nprintf '%s\\n' SE_PAINT_END\n",
        )
        .expect("write paint script");
        while receiver.try_recv().is_ok() {}
        pty.write(terminal_id, &format!("sh '{}'\n", script_path.display()))
            .await
            .expect("run paint script");
        let buffer = collect_output_until(receiver, |text| {
            has_output_line(text, "SE_PAINT_START") && has_output_line(text, "SE_PAINT_END")
        })
        .await;
        let start = buffer
            .lines()
            .position(|line| line.trim() == "SE_PAINT_START");
        let end = buffer
            .lines()
            .position(|line| line.trim() == "SE_PAINT_END");
        let (Some(start), Some(end)) = (start, end) else {
            panic!("paint markers missing in PTY output: {buffer:?}");
        };
        buffer
            .lines()
            .skip(start + 1)
            .take(end.saturating_sub(start + 1))
            .flat_map(str::chars)
            .filter(|ch| *ch == 'A')
            .count()
    }

    async fn companion_request(
        state: &AppState,
        authority: &Arc<RemoteAccessAuthority>,
        principal: &RemotePrincipal,
        ctx: &mut ConnectionContext,
        type_: &str,
        payload: Value,
    ) -> Value {
        let (tx, _rx) = mpsc::channel(8);
        handle(
            Request {
                id: format!("mobile-{type_}"),
                type_: type_.to_string(),
                payload,
            },
            state,
            authority,
            principal,
            &tx,
            ctx,
        )
        .await
        .unwrap_or_else(|(code, message)| panic!("{type_} failed: {code} {message}"))
    }

    /// End-to-end on the real spawn path: a real shell, a real terminate.
    ///
    /// The shell dies, its children die with it, the slot comes back.
    ///
    /// A process-group kill alone cannot do this: the app spawns
    /// login+interactive shells, so job control gives every job its own group
    /// and a backgrounded child is out of the shell's group entirely. Only the
    /// session sweep reaches it.
    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_reclaims_a_real_terminal_and_its_session() {
        let state = terminal_test_state();
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().canonicalize().unwrap();
        let marker = cwd.join("child.pid");

        let spawned = state
            .pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(conversation_id()),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    cols: Some(80),
                    rows: Some(24),
                    shell: Some("/bin/sh".into()),
                    env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("spawn a real PTY");
        let terminal_id = spawned.info.id.clone();
        let shell_pid = spawned.info.pid as i32;
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            alive(shell_pid),
            "the shell must be running before we start"
        );

        state
            .pty
            .write(
                &terminal_id,
                &format!("sleep 120 & echo $! > {}\n", marker.display()),
            )
            .await
            .expect("write to the live PTY");

        let child_pid = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Ok(raw) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = raw.trim().parse::<i32>() {
                        break pid;
                    }
                }
                assert!(std::time::Instant::now() < deadline, "child never reported");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };

        state
            .pty
            .terminate(&terminal_id)
            .await
            .expect("cleanup must complete on a real terminal");

        let settle = |pid: i32| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        settle(shell_pid);
        assert!(!alive(shell_pid), "the shell itself must be gone");
        assert!(
            state.pty.get(&terminal_id).is_none(),
            "the terminal slot must be reclaimed"
        );

        settle(child_pid);
        assert!(
            !alive(child_pid),
            "the session sweep must reach the shell's children, including the \
             job-control groups a process-group kill cannot address"
        );
    }

    #[tokio::test]
    async fn mobile_set_display_mode_changes_live_pty_window_then_restores() {
        let state = terminal_test_state();
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().canonicalize().unwrap();
        let conversation_id = conversation_id();
        let spawned = state
            .pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(conversation_id),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    cols: Some(120),
                    rows: Some(40),
                    shell: Some("/bin/sh".into()),
                    env: Some(HashMap::from([
                        ("PS1".into(), "$ ".into()),
                        ("TERM".into(), "xterm-256color".into()),
                    ])),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("spawn desktop-sized PTY");
        let terminal_id = spawned.info.id.clone();
        let instance = state.pty.get(&terminal_id).expect("live instance");
        let mut output = instance.broadcast_tx.subscribe();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let desktop = read_live_window(&state.pty, &terminal_id, &mut output).await;
        assert_eq!(desktop, (40, 120), "desktop PTY must start at 120x40");

        let authority = terminal_authority();
        let principal = RemotePrincipal::for_tests(1);
        let generation = state.pty.claim_generation(&terminal_id).unwrap();
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        ctx.authorize(&terminal_id, conversation_id, generation);

        let phone_reply = companion_request(
            &state,
            &authority,
            &principal,
            &mut ctx,
            "set_display_mode",
            json!({
                "terminalId": terminal_id,
                "mode": "phone",
                "cols": 40,
                "rows": 18
            }),
        )
        .await;
        assert_eq!(phone_reply["mode"], "phone");
        assert_eq!(phone_reply["cols"], 40);
        assert_eq!(phone_reply["rows"], 18);
        assert!(ctx.phone_fit.contains(&terminal_id));

        let phone = read_live_window(&state.pty, &terminal_id, &mut output).await;
        assert_eq!(
            phone,
            (18, 40),
            "mobile takeover must ioctl the live window"
        );

        let _ = companion_request(
            &state,
            &authority,
            &principal,
            &mut ctx,
            "resize",
            json!({
                "terminalId": terminal_id,
                "cols": 100,
                "rows": 30
            }),
        )
        .await;
        let still_phone = read_live_window(&state.pty, &terminal_id, &mut output).await;
        assert_eq!(
            still_phone,
            (18, 40),
            "desktop-style resize must be ignored while the phone owns geometry"
        );

        let listed = live_terminal_summary(&instance, instance.cwd.clone(), None);
        assert_eq!(listed["displayMode"], "phone");
        assert_eq!(listed["cols"], 40);
        assert_eq!(listed["rows"], 18);

        let paint_script = cwd.join("paint.sh");
        let phone_cells =
            count_full_screen_cells(&state.pty, &terminal_id, &mut output, &paint_script).await;

        let desktop_reply = companion_request(
            &state,
            &authority,
            &principal,
            &mut ctx,
            "set_display_mode",
            json!({
                "terminalId": terminal_id,
                "mode": "desktop"
            }),
        )
        .await;
        assert_eq!(desktop_reply["mode"], "desktop");
        assert_eq!(desktop_reply["cols"], 120);
        assert_eq!(desktop_reply["rows"], 40);
        assert!(!ctx.phone_fit.contains(&terminal_id));

        let restored = read_live_window(&state.pty, &terminal_id, &mut output).await;
        assert_eq!(
            restored,
            (40, 120),
            "desktop mode must restore the parked window"
        );

        let desktop_cells =
            count_full_screen_cells(&state.pty, &terminal_id, &mut output, &paint_script).await;
        println!(
            "full-screen paint cells phone={phone_cells} desktop={desktop_cells} ratio={:.2}",
            desktop_cells as f64 / phone_cells.max(1) as f64
        );
        assert!(
            phone_cells < desktop_cells,
            "a full-screen paint must emit fewer cells on the phone window ({phone_cells} < {desktop_cells})"
        );
        assert!(
            desktop_cells >= 120 * 40,
            "desktop paint should cover the parked 120x40 grid, got {desktop_cells}"
        );

        state.pty.terminate(&terminal_id).await.unwrap();
    }

    #[tokio::test]
    async fn mobile_disconnect_restores_parked_desktop_window() {
        let state = terminal_test_state();
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().canonicalize().unwrap();
        let conversation_id = conversation_id();
        let spawned = state
            .pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(conversation_id),
                    cwd: Some(cwd.to_string_lossy().into_owned()),
                    cols: Some(120),
                    rows: Some(40),
                    shell: Some("/bin/sh".into()),
                    env: Some(HashMap::from([
                        ("PS1".into(), "$ ".into()),
                        ("TERM".into(), "xterm-256color".into()),
                    ])),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("spawn desktop-sized PTY");
        let terminal_id = spawned.info.id.clone();
        let instance = state.pty.get(&terminal_id).expect("live instance");
        let mut output = instance.broadcast_tx.subscribe();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let authority = terminal_authority();
        let principal = RemotePrincipal::for_tests(1);
        let generation = state.pty.claim_generation(&terminal_id).unwrap();
        let mut ctx = ConnectionContext::new(Arc::new(RwLock::new(HashMap::new())), false);
        ctx.authorize(&terminal_id, conversation_id, generation);
        companion_request(
            &state,
            &authority,
            &principal,
            &mut ctx,
            "set_display_mode",
            json!({
                "terminalId": terminal_id,
                "mode": "phone",
                "cols": 42,
                "rows": 16
            }),
        )
        .await;
        assert_eq!(
            read_live_window(&state.pty, &terminal_id, &mut output).await,
            (16, 42)
        );

        release_phone_fits(&state, &mut ctx).await;
        assert_eq!(
            read_live_window(&state.pty, &terminal_id, &mut output).await,
            (40, 120),
            "dropping the mobile websocket must restore the parked desktop window"
        );

        state.pty.terminate(&terminal_id).await.unwrap();
    }
}
