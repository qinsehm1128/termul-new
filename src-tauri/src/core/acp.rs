//! ACP Core process boundary.
//!
//! This module owns the FULL ACP stack in a process independent from the GUI:
//! `AcpManager`, the Conversation durable writer (bootstrap-owned ordered
//! persistence), `WsRelaySink` durable admission, and the permission/question
//! rendezvous. The GUI becomes a client (`AcpCoreClient`) that proxies
//! commands and mirrors `acp:*` events verbatim; the standalone server keeps
//! its existing in-process composition.
//!
//! Invariants:
//! - Conversation durable writes happen ONLY here once the desktop adopts
//!   this Core (single writer). The GUI must not open the repository.
//! - Events cross the boundary as `{seq, type, sid, data}` envelopes; the
//!   GUI re-emits `type` + `data` under the original `acp:*` name so the
//!   renderer contract is byte-identical.
//! - No GUI-runtime dependency in this module (structural test below).

use super::ipc::{
    prepare_runtime_dir, read_frame, read_json_frame, remove_stale_socket, validate_hello,
    write_json_frame, CoreEndpoint, CoreError, CoreErrorPayload, CoreEvent, CoreHello, CoreRequest,
    CoreResponse, CoreRole, CURRENT_PROTOCOL_VERSION,
};
use crate::acp::config::{AgentConfig, AgentId, PermissionPolicy, SessionId};
use crate::acp::manager::{
    AcpManager, NewSessionOutcome, SessionCreationContext, SessionReopenOutcome,
};
use crate::acp::session_persistence::SessionRegistration;
use crate::conversation::{
    ConversationApplicationService, ConversationBackend, ConversationBootstrap,
    ConversationCreationService, ConversationId, ConversationLifecycleAction,
    ConversationLifecycleOutcome, ConversationPersistenceAdapter, ExecutionTarget,
    HostConversationRoots, MigrationHostMode, PrepareConversationRequest, ProjectAttachment,
    SessionWorkspaceService,
};
use crate::memory_index::commands::{
    throttled, MemoryIndexBuildArgs, MemoryIndexListArgs, MemoryIndexScopeArgs,
    MemoryIndexSearchArgs, MemoryIndexSessionArgs, MEMORY_INDEX_PROGRESS_EVENT,
};
use crate::memory_index::ingest::IngestOptions;
use crate::memory_index::service::MemoryIndexService;
use crate::scheduled_tasks::{
    ActivateTaskRequestV1, RevisionRequestV1, ScheduleSpecV1, ScheduledTaskDraftInputV1,
    ScheduledTaskService, TaskMutationContextV1, UpdateDraftRequestV1,
};
use crate::web::permissions::{PermissionRendezvous, QuestionRendezvous};
use crate::web::sink::{
    AcpEvent, EventDeliveryReceipt, EventSink, EventSinkError, EventSinkPriority, WsRelaySink,
};
use agent_client_protocol::schema::v1::{
    ContentBlock, ListSessionsResponse, McpServer, SessionConfigOption, StopReason, TextContent,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, watch};

pub const METHOD_HEALTH: &str = "health";
pub const METHOD_LIST_AGENTS: &str = "listAgents";
pub const METHOD_SPAWN_AGENT: &str = "spawnAgent";
pub const METHOD_KILL_AGENT: &str = "killAgent";
pub const METHOD_SET_PERMISSION_POLICY: &str = "setPermissionPolicy";
pub const METHOD_NEW_SESSION: &str = "newSession";
pub const METHOD_LOAD_SESSION: &str = "loadSession";
pub const METHOD_RESUME_SESSION: &str = "resumeSession";
pub const METHOD_CLOSE_SESSION: &str = "closeSession";
pub const METHOD_DISPOSE_EPHEMERAL_SESSION: &str = "disposeEphemeralSession";
pub const METHOD_IS_EPHEMERAL_SESSION: &str = "isEphemeralSession";
pub const METHOD_LIST_SESSIONS: &str = "listSessions";
pub const METHOD_REGISTER_DISCOVERED_SESSION: &str = "registerDiscoveredSession";
pub const METHOD_CONVERSATION_ID_FOR_SESSION: &str = "conversationIdForSession";
pub const METHOD_SEND_PROMPT: &str = "sendPrompt";
pub const METHOD_CANCEL_PROMPT: &str = "cancelPrompt";
pub const METHOD_OWNS_SESSION: &str = "ownsSession";
pub const METHOD_IS_TURN_ACTIVE: &str = "isTurnActive";
pub const METHOD_WAIT_TURN_IDLE: &str = "waitTurnIdle";
pub const METHOD_RESPOND_PERMISSION: &str = "respondPermission";
pub const METHOD_ANSWER_QUESTION: &str = "answerQuestion";
pub const METHOD_PERMISSION_INFO: &str = "permissionInfo";
pub const METHOD_QUESTION_INFO: &str = "questionInfo";
pub const METHOD_AUTHENTICATE: &str = "authenticate";
pub const METHOD_SET_MODE: &str = "setMode";
pub const METHOD_SET_MODEL: &str = "setModel";
pub const METHOD_SET_CONFIG_OPTION: &str = "setConfigOption";
pub const METHOD_COMPOSER_CONTROLS: &str = "composerControls";
pub const METHOD_HISTORY_LIST: &str = "historyList";
pub const METHOD_HISTORY_GET: &str = "historyGet";
pub const METHOD_HISTORY_GET_PAGE: &str = "historyGetPage";
pub const METHOD_HISTORY_CURSOR: &str = "historyCursor";
pub const METHOD_HISTORY_OPEN: &str = "historyOpen";
pub const METHOD_WORKSPACE_ENSURE_TERMINAL_REF_WRITABLE: &str =
    "workspaceEnsureTerminalRefWritable";
pub const METHOD_WORKSPACE_ADD_TERMINAL_REF: &str = "workspaceAddTerminalRef";
pub const METHOD_WORKSPACE_REMOVE_TERMINAL_REF_AFTER_TERMINATION: &str =
    "workspaceRemoveTerminalRefAfterTermination";
pub const METHOD_SUBSCRIBE_EVENTS: &str = "subscribeEvents";
pub const METHOD_STOP_PRODUCERS: &str = "stopProducers";
pub const METHOD_SHUTDOWN_PERSISTENCE: &str = "shutdownPersistence";
pub const METHOD_SHUTDOWN: &str = "shutdown";
pub const METHOD_RETIRE_SESSION: &str = "retireSession";
pub const METHOD_FLUSH_CATALOG: &str = "flushCatalog";
pub const METHOD_REGISTER_CONVERSATION_BINDING: &str = "registerConversationBinding";
pub const METHOD_STABLE_AGENT_NAMESPACE: &str = "stableAgentNamespace";
pub const METHOD_LIST_RUNNING_NAMESPACES: &str = "listRunningNamespaces";

pub const METHOD_CONVERSATION_HOST_STATUS: &str = "conversationHostStatus";
pub const METHOD_CONVERSATION_LIST: &str = "conversationList";
pub const METHOD_CONVERSATION_OPEN: &str = "conversationOpen";
pub const METHOD_CONVERSATION_RENAME: &str = "conversationRename";
pub const METHOD_CONVERSATION_PREPARE_TERMINAL: &str = "conversationPrepareTerminal";
pub const METHOD_CONVERSATION_PROVISION_TERMINAL: &str = "conversationProvisionTerminal";
pub const METHOD_CONVERSATION_RECOVERY_RESOLVE: &str = "conversationRecoveryResolve";
pub const METHOD_CONVERSATION_ATTACH_PROJECT: &str = "conversationAttachProject";
pub const METHOD_CONVERSATION_DETACH_PROJECT: &str = "conversationDetachProject";
pub const METHOD_CONVERSATION_UPDATE_EXECUTION_TARGET: &str = "conversationUpdateExecutionTarget";
pub const METHOD_CONVERSATION_DETACH_BINDING: &str = "conversationDetachBinding";
pub const METHOD_CONVERSATION_REBIND_BINDING: &str = "conversationRebindBinding";
pub const METHOD_CONVERSATION_SUSPEND_BINDING: &str = "conversationSuspendBinding";
pub const METHOD_CONVERSATION_REPLACE_BINDING: &str = "conversationReplaceBinding";
pub const METHOD_CONVERSATION_DELETE: &str = "conversationDelete";
pub const METHOD_CONVERSATION_GET: &str = "conversationGet";
pub const METHOD_CONVERSATION_GET_BINDING: &str = "conversationGetBinding";
pub const METHOD_CONVERSATION_RESOLVE_LEGACY_ID: &str = "conversationResolveLegacyId";
pub const METHOD_CONVERSATION_GET_WORKSPACE: &str = "conversationGetWorkspace";
pub const METHOD_CONVERSATION_WRITE_WORKSPACE: &str = "conversationWriteWorkspace";

pub const METHOD_SCHEDULED_TASK_PREVIEW: &str = "scheduledTaskPreview";
pub const METHOD_SCHEDULED_TASK_LIST: &str = "scheduledTaskList";
pub const METHOD_SCHEDULED_TASK_GET: &str = "scheduledTaskGet";
pub const METHOD_SCHEDULED_TASK_DRAFT_CREATE: &str = "scheduledTaskDraftCreate";
pub const METHOD_SCHEDULED_TASK_DRAFT_UPDATE: &str = "scheduledTaskDraftUpdate";
pub const METHOD_SCHEDULED_TASK_ACTIVATE: &str = "scheduledTaskActivate";
pub const METHOD_SCHEDULED_TASK_PAUSE: &str = "scheduledTaskPause";
pub const METHOD_SCHEDULED_TASK_RESUME: &str = "scheduledTaskResume";
pub const METHOD_SCHEDULED_TASK_DELETE: &str = "scheduledTaskDelete";
pub const METHOD_SCHEDULED_TASK_RUN_NOW: &str = "scheduledTaskRunNow";
pub const METHOD_SCHEDULED_TASK_RETRY_RUN: &str = "scheduledTaskRetryRun";
pub const METHOD_SCHEDULED_TASK_LIST_RUNS: &str = "scheduledTaskListRuns";
pub const METHOD_SCHEDULED_TASK_LIST_AUDIT: &str = "scheduledTaskListAudit";

pub const METHOD_MEMORY_BUILD: &str = "memoryBuild";
pub const METHOD_MEMORY_CANCEL: &str = "memoryCancel";
pub const METHOD_MEMORY_STATUS: &str = "memoryStatus";
pub const METHOD_MEMORY_SEARCH: &str = "memorySearch";
pub const METHOD_MEMORY_SESSIONS: &str = "memorySessions";
pub const METHOD_MEMORY_SESSION: &str = "memorySession";
pub const METHOD_MEMORY_MCP_INVOCATION: &str = "memoryMcpInvocation";
pub const METHOD_MEMORY_UNIVERSAL_MCP_INVOCATION: &str = "memoryUniversalMcpInvocation";

pub const ACP_EVENT_TOPIC: &str = "acp.event";
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
/// `sendPrompt` resolves when the whole agent turn completes; turns routinely
/// run minutes, so it gets its own budget instead of the control-plane one.
const LONG_RPC_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Event envelope crossing the Core boundary. `data` is the verbatim
/// renderer-facing payload; `seq` is a monotonic informational watermark.
/// Gap detection is not implemented on the GUI client — `connection_changed`
/// is the resync signal. Per-session durable replay stays owned by the relay
/// inside the Core.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCoreEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    pub data: Value,
}

/// Shared event fan-out state: the sink appends, connections broadcast.
#[derive(Default)]
struct SharedEvents {
    tx: Mutex<Option<broadcast::Sender<CoreEvent>>>,
    seq: AtomicU64,
}

impl SharedEvents {
    fn sender(&self) -> Option<broadcast::Sender<CoreEvent>> {
        self.tx.lock().clone()
    }

    fn emit_acp(&self, type_: &str, sid: Option<String>, data: Value) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let envelope = AcpCoreEvent {
            seq,
            type_: type_.to_string(),
            sid,
            data,
        };
        let frame = CoreEvent {
            topic: ACP_EVENT_TOPIC.to_string(),
            payload: serde_json::to_value(&envelope).unwrap_or(Value::Null),
        };
        if let Some(tx) = self.sender() {
            // Lagged/empty receiver sets are expected: the GUI reconnects and
            // resyncs from the watermark, and durable replay lives in the relay.
            let _ = tx.send(frame);
        }
    }
}

/// Live-delivery sink that carries every admitted ACP event over Core IPC.
/// Durable admission (relay) runs first via `EventSinkPriority`; this sink
/// must never reject — a GUI being down is not an event failure.
struct CoreIpcEventSink {
    shared: Arc<SharedEvents>,
}

impl CoreIpcEventSink {
    fn install(shared: &Arc<SharedEvents>) -> Self {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        *shared.tx.lock() = Some(tx);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl EventSink for CoreIpcEventSink {
    fn priority(&self) -> EventSinkPriority {
        EventSinkPriority::LiveDelivery
    }

    fn emit(&self, event: &AcpEvent) -> Result<EventDeliveryReceipt, EventSinkError> {
        self.shared
            .emit_acp(event.type_, event.sid.clone(), event.payload.clone());
        Ok(EventDeliveryReceipt::delivered(None, false))
    }
}

struct AcpCoreState {
    manager: Arc<AcpManager>,
    relay: Arc<WsRelaySink>,
    workspace: Arc<SessionWorkspaceService>,
    creation: Arc<ConversationCreationService>,
    persistence: Arc<ConversationPersistenceAdapter>,
    application: Arc<ConversationApplicationService>,
    scheduled_tasks: Arc<ScheduledTaskService>,
    memory: Arc<MemoryIndexService>,
    events: Arc<SharedEvents>,
    shutdown: watch::Sender<bool>,
}

fn invalid(detail: impl Into<String>) -> CoreError {
    CoreError::InvalidRequest(detail.into())
}

fn response(id: u64, result: Result<Value, CoreError>) -> CoreResponse {
    match result {
        Ok(result) => CoreResponse {
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => CoreResponse {
            id,
            result: None,
            error: Some(CoreErrorPayload {
                code: error.code().to_string(),
                message: match error {
                    CoreError::InvalidRequest(detail) if !detail.is_empty() => detail,
                    other => other.client_message().to_string(),
                },
            }),
        },
    }
}

/// Resolve the Conversation workspace base the same way the desktop does:
/// explicit env override, else `<home>/Documents/<brand>`, else `<home>/<brand>`.
/// The GUI launcher passes its own computed root via env so both processes
/// always agree; the fallback keeps the Core runnable standalone in tests.
fn workspace_base_from_env() -> PathBuf {
    if let Some(root) = std::env::var_os("TERMUL_CORE_WORKSPACE_ROOT") {
        let root = PathBuf::from(root);
        if root.as_os_str().is_empty() {
            // fall through to the derived default
        } else {
            return root;
        }
    }
    #[cfg(unix)]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    #[cfg(not(any(unix, windows)))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let brand = crate::brand::canonical().display_name.to_string();
    match home {
        Some(home) => home.join("Documents").join(&brand),
        None => std::env::temp_dir().join(brand),
    }
}

pub async fn run_acp_core(profile_root: PathBuf) -> Result<(), CoreError> {
    let workspace_base = workspace_base_from_env();
    run_acp_core_with_roots(profile_root, workspace_base).await
}

/// Compose and serve the ACP Core against explicit roots. Split from
/// [`run_acp_core`] so integration tests can inject temp directories.
pub async fn run_acp_core_with_roots(
    state_root: PathBuf,
    workspace_base: PathBuf,
) -> Result<(), CoreError> {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let endpoint = CoreEndpoint::for_profile(&state_root, CoreRole::AcpCore);
        prepare_runtime_dir(&endpoint)?;
        let _ = remove_stale_socket(&endpoint);
        let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
        let listener = UnixListener::bind(path).map_err(CoreError::from)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(CoreError::from)?;

        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let state = Arc::new(
            compose_acp_core(state_root, workspace_base, shutdown).inspect_err(|error| {
                log::error!(
                    target: "se_manager::core",
                    "operation=acp_core_compose stable_code={} detail={}",
                    error.code(),
                    match error {
                        CoreError::InvalidRequest(detail) => detail.as_str(),
                        _ => error.client_message(),
                    }
                );
            })?,
        );
        log::info!(
            target: "se_manager::core",
            "operation=core_listen role=acp-core stable_code=READY"
        );

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        shutdown_acp_core(&state).await?;
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(CoreError::from)?;
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(error) = handle_connection(stream, state).await {
                            log::debug!(target: "se_manager::core", "operation=acp_connection stable_code={}", error.code());
                        }
                    });
                }
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (state_root, workspace_base);
        Err(CoreError::UnsupportedPlatform)
    }
}

/// Build the full ACP stack on the Conversation bootstrap. Mirrors the desktop
/// composition in `lib.rs` (relay durability authority, rendezvous) minus the
/// renderer sink, which the IPC event stream replaces.
fn compose_acp_core(
    state_root: PathBuf,
    workspace_base: PathBuf,
    shutdown: watch::Sender<bool>,
) -> Result<AcpCoreState, CoreError> {
    let bootstrap = ConversationBootstrap::run(
        HostConversationRoots::desktop(state_root.clone(), workspace_base),
        MigrationHostMode::Desktop,
    )
    .map_err(|error| invalid(format!("conversation bootstrap failed: {error}")))?;
    log::info!(
        target: "se_manager::core",
        "operation=acp_core_bootstrap stable_code=READY phase={:?} recovery_count={}",
        bootstrap.migration_phase,
        bootstrap.recovery_item_count
    );

    let shared = Arc::new(SharedEvents::default());
    let ipc_sink = CoreIpcEventSink::install(&shared);

    let relay = Arc::new(WsRelaySink::with_conversation_persistence(
        4096,
        Arc::clone(&bootstrap.persistence_adapter),
        None,
    ));
    let relay_ordered = relay
        .ordered_conversation_persistence()
        .ok_or_else(|| invalid("acp core relay is missing ordered persistence"))?;
    if !relay_ordered.shares_authority(&bootstrap.ordered_persistence) {
        return Err(invalid(
            "acp core relay did not retain the bootstrap ordering authority",
        ));
    }

    let manager = Arc::new(AcpManager::with_conversation_services(
        vec![Arc::new(ipc_sink), Arc::clone(&relay) as Arc<dyn EventSink>],
        Arc::clone(&bootstrap.creation),
        Arc::clone(&bootstrap.persistence_adapter),
    ));
    // The Core has no in-process PTY manager. The detached runtime keeps the
    // terminal seams honest (nothing is live from the Core's viewpoint) until
    // a Terminal-Core-linking runtime replaces it.
    manager.set_terminal_service(super::handles::TerminalServiceHandle::from_runtime(
        Arc::new(super::handles::DetachedTerminalRuntime),
    ));

    let runtime_handle = tokio::runtime::Handle::current();
    let rendezvous = Arc::new(PermissionRendezvous::with_handle_and_policy(
        Arc::clone(&manager),
        Duration::from_secs(60),
        Duration::from_secs(15),
        runtime_handle.clone(),
    ));
    relay.set_rendezvous(rendezvous);
    let question_rendezvous = Arc::new(QuestionRendezvous::with_handle(
        Arc::clone(&manager),
        Duration::from_secs(60),
        runtime_handle.clone(),
    ));
    relay.set_question_rendezvous(question_rendezvous);

    bootstrap
        .application
        .attach_lifecycle(
            crate::conversation::ConversationLifecycleService::from_terminal(
                Arc::clone(&manager),
                super::handles::TerminalServiceHandle::from_runtime(Arc::new(
                    super::handles::DetachedTerminalRuntime,
                )),
            )
            .map_err(|error| invalid(error.to_string()))?,
        )
        .map_err(|error| invalid(error.to_string()))?;

    // DurableFileSystem rejects symlink path components (`/var` -> `/private/var` on
    // macOS). Canonicalize the existing state root before joining so tempfile-backed
    // tests and real profile dirs both open the store.
    let scheduled_state_root =
        std::fs::canonicalize(&state_root).unwrap_or_else(|_| state_root.clone());
    let scheduled_task_root = scheduled_state_root.join("scheduled-tasks").join("v1");
    let scheduled_task_store = Arc::new(
        crate::scheduled_tasks::ScheduledTaskStore::open_with_legacy_root(
            scheduled_task_root.join("catalog"),
            Some(scheduled_task_root.join("projects")),
        )
        .map_err(|error| invalid(format!("failed to open scheduled task store: {error}")))?,
    );
    let scheduled_tasks = ScheduledTaskService::new(
        scheduled_task_store,
        Arc::new(crate::scheduled_tasks::AcpScheduledTaskExecutor::new(
            Arc::clone(&manager),
            Arc::clone(&relay),
        )),
    );
    manager.set_scheduled_tasks(&scheduled_tasks);
    scheduled_tasks.start_on(&runtime_handle);

    let memory = Arc::new(MemoryIndexService::new(state_root));
    manager.set_memory_index(&memory);

    Ok(AcpCoreState {
        manager,
        relay,
        workspace: Arc::clone(&bootstrap.workspace),
        creation: Arc::clone(&bootstrap.creation),
        persistence: Arc::clone(&bootstrap.persistence_adapter),
        application: Arc::clone(&bootstrap.application),
        scheduled_tasks,
        memory,
        events: shared,
        shutdown,
    })
}

async fn shutdown_acp_core(state: &AcpCoreState) -> Result<(), CoreError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    state.manager.stop_producers().await.map_err(invalid)?;
    let flush = state
        .relay
        .flush_catalog_until(deadline)
        .await
        .map_err(|code| invalid(format!("acp core catalog flush failed: {code}")))?;
    log::info!(
        target: "se_manager::core",
        "operation=acp_core_shutdown stable_code=OK flushed_generation={} write_count={}",
        flush.flushed_generation,
        flush.write_count
    );
    state
        .manager
        .shutdown_persistence()
        .await
        .map_err(invalid)?;
    Ok(())
}

#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<AcpCoreState>,
) -> Result<(), CoreError> {
    let mut stream = stream;
    let hello: CoreHello = read_json_frame(&mut stream).await?;
    let ack = validate_hello(&hello, CoreRole::AcpCore)?;
    write_json_frame(&mut stream, &ack).await?;

    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    // Event forwarder: every admitted event reaches every connected client.
    // Frame-granular interleaving with responses is safe — the reader loop
    // writes responses through the same mutex.
    if let Some(events) = state.events.sender() {
        let mut events = events.subscribe();
        let writer = Arc::clone(&writer);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(frame) => {
                        let mut writer = writer.lock().await;
                        if write_json_frame(&mut *writer, &frame).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    loop {
        let payload = match read_frame(&mut reader).await {
            Ok(payload) => payload,
            Err(CoreError::Io(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        let request: CoreRequest = serde_json::from_slice(&payload).map_err(|error| {
            CoreError::InvalidFrame(format!("invalid ACP core request: {error}"))
        })?;
        let shutdown = request.method == METHOD_SHUTDOWN;
        let reply = response(request.id, dispatch(&state, &request).await);
        {
            let mut writer = writer.lock().await;
            write_json_frame(&mut *writer, &reply).await?;
        }
        if shutdown {
            let _ = state.shutdown.send(true);
            return Ok(());
        }
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(request: &CoreRequest) -> Result<T, CoreError> {
    serde_json::from_value(request.params.clone())
        .map_err(|error| invalid(format!("invalid params for {}: {error}", request.method)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentIdParams {
    agent_id: AgentId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionParams {
    agent_id: AgentId,
    session_id: SessionId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSessionParams {
    agent_id: AgentId,
    cwd: String,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    ephemeral: Option<bool>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    worktree_path: Option<String>,
    #[serde(default)]
    worktree_branch: Option<String>,
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    project_attachment: Option<ProjectAttachment>,
    #[serde(default)]
    execution_target: Option<ExecutionTarget>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReopenSessionParams {
    agent_id: AgentId,
    session_id: SessionId,
    cwd: String,
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionsParams {
    agent_id: AgentId,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterDiscoveredParams {
    session_id: String,
    agent_id: AgentId,
    cwd: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    updated_at: Option<u64>,
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendPromptParams {
    agent_id: AgentId,
    session_id: SessionId,
    #[serde(default)]
    content: Option<Vec<ContentBlock>>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondPermissionParams {
    agent_id: AgentId,
    request_id: String,
    #[serde(default)]
    option_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionInfoParams {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuestionInfoParams {
    question_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerQuestionParams {
    agent_id: AgentId,
    question_id: String,
    #[serde(default)]
    values: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateParams {
    agent_id: AgentId,
    method_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModeParams {
    agent_id: AgentId,
    session_id: SessionId,
    mode_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModelParams {
    agent_id: AgentId,
    session_id: SessionId,
    model_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetConfigOptionParams {
    agent_id: AgentId,
    session_id: SessionId,
    config_id: String,
    value_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryPageParams {
    session_id: String,
    after_seq: u64,
    limit: usize,
    #[serde(default)]
    target_last_seq: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceConversationParams {
    conversation_id: String,
    #[serde(default)]
    writable: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceTerminalRefParams {
    conversation_id: String,
    terminal_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionPolicyParams {
    agent_id: AgentId,
    policy: PermissionPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationIdOnlyParams {
    conversation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationRenameParams {
    conversation_id: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationRequestParams {
    request: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationProvisionTerminalParams {
    conversation_id: String,
    terminal_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationRevisionParams {
    conversation_id: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationAttachProjectParams {
    conversation_id: String,
    expected_revision: u64,
    attachment: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationUpdateExecutionTargetParams {
    conversation_id: String,
    expected_revision: u64,
    execution_target: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationReplaceBindingParams {
    conversation_id: String,
    expected_revision: u64,
    request: Value,
    #[serde(default)]
    target_runtime_agent_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationDeleteParams {
    conversation_id: String,
    expected_revision: u64,
    #[serde(default)]
    remove_workspace: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskPreviewParams {
    schedule: ScheduleSpecV1,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskListParams {
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskIdParams {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskDraftCreateParams {
    input: ScheduledTaskDraftInputV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskDraftUpdateParams {
    task_id: String,
    request: UpdateDraftRequestV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskActivateParams {
    task_id: String,
    request: ActivateTaskRequestV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskRevisionParams {
    task_id: String,
    request: RevisionRequestV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskRetryRunParams {
    task_id: String,
    run_id: String,
}

fn parse_conversation_id(raw: &str) -> Result<ConversationId, CoreError> {
    ConversationId::parse(raw).map_err(|error| invalid(error.to_string()))
}

fn parse_conversation_id_component(raw: &str) -> Result<ConversationId, CoreError> {
    ConversationId::parse_path_component(raw).map_err(|error| invalid(error.to_string()))
}

fn require_core_host_admission() -> Result<(), CoreError> {
    crate::host_admission::HostAdmission::global()
        .check()
        .map_err(|_| invalid("host is shutting down"))
}

fn conversation_application_err(
    error: crate::conversation::ConversationApplicationError,
) -> CoreError {
    log::warn!(
        "[acp-core] operation={} conversation_id={} code={}",
        error.operation,
        error
            .conversation_id
            .map_or_else(|| "none".to_string(), |value| value.to_string()),
        error.code
    );
    invalid(format!("{}:{}", error.code, error.detail))
}

fn conversation_creation_err(error: crate::conversation::ConversationCreationError) -> CoreError {
    log::warn!(
        "[acp-core] operation={} conversation_id={} code={:?}",
        error.operation,
        error
            .conversation_id
            .map_or_else(|| "none".to_string(), |value| value.to_string()),
        error.code
    );
    invalid(format!("{:?}:{}", error.code, error.detail))
}

fn to_json<T: Serialize>(value: T) -> Result<Value, CoreError> {
    serde_json::to_value(value).map_err(|error| invalid(error.to_string()))
}

fn payload_from_value<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CoreError> {
    serde_json::from_value(value)
        .map_err(|error| invalid(format!("payload validation failed: {error}")))
}

fn scheduled_task_human_context() -> TaskMutationContextV1 {
    TaskMutationContextV1::default()
}

fn emit_memory_progress(
    events: &SharedEvents,
    relay: &WsRelaySink,
    progress: &crate::memory_index::ingest::IngestProgress,
) {
    let Ok(payload) = serde_json::to_value(progress) else {
        return;
    };
    events.emit_acp(MEMORY_INDEX_PROGRESS_EVENT, None, payload.clone());
    let event = AcpEvent {
        sid: None,
        type_: MEMORY_INDEX_PROGRESS_EVENT,
        payload,
    };
    let _ = EventSink::emit(relay, &event);
}

async fn retire_deleted_binding_if_updated(
    relay: &WsRelaySink,
    current_session_id: Option<&str>,
    outcome: &ConversationLifecycleOutcome,
) -> Result<(), String> {
    if matches!(
        outcome,
        ConversationLifecycleOutcome::Updated {
            action: ConversationLifecycleAction::DeleteConversation,
            ..
        }
    ) {
        if let Some(session_id) = current_session_id {
            relay.retire_session(session_id).await?;
        }
    }
    Ok(())
}

async fn dispatch(state: &AcpCoreState, request: &CoreRequest) -> Result<Value, CoreError> {
    let manager = &state.manager;
    match request.method.as_str() {
        METHOD_HEALTH => Ok(json!({
            "role": "acp-core",
            "status": "ready",
            "eventWatermark": state.events.seq.load(Ordering::Relaxed),
        })),
        METHOD_LIST_AGENTS => {
            serde_json::to_value(manager.list_agents()).map_err(|error| invalid(error.to_string()))
        }
        METHOD_SPAWN_AGENT => {
            let config: AgentConfig = parse_params(request)?;
            let outcome = manager.spawn(config).await.map_err(invalid)?;
            serde_json::to_value(outcome).map_err(|error| invalid(error.to_string()))
        }
        METHOD_KILL_AGENT => {
            let params: AgentIdParams = parse_params(request)?;
            manager.kill(&params.agent_id).await.map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SET_PERMISSION_POLICY => {
            let params: PermissionPolicyParams = parse_params(request)?;
            manager
                .set_permission_policy(&params.agent_id, params.policy)
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_NEW_SESSION => {
            let params: NewSessionParams = parse_params(request)?;
            let conversation_id = params
                .conversation_id
                .as_deref()
                .map(parse_conversation_id)
                .transpose()?;
            let result = manager
                .new_session_with_context(
                    &params.agent_id,
                    params.cwd,
                    params.mcp_servers.unwrap_or_default(),
                    SessionCreationContext {
                        project_id: params.project_id.filter(|id| !id.trim().is_empty()),
                        ephemeral: params.ephemeral.unwrap_or(false),
                        conversation_id,
                        project_attachment: params.project_attachment,
                        execution_target: params.execution_target,
                        worktree_path: params.worktree_path.filter(|p| !p.trim().is_empty()),
                        worktree_branch: params.worktree_branch.filter(|b| !b.trim().is_empty()),
                    },
                )
                .await;
            let outcome: NewSessionOutcome = result.map_err(invalid)?;
            serde_json::to_value(outcome).map_err(|error| invalid(error.to_string()))
        }
        METHOD_LOAD_SESSION | METHOD_RESUME_SESSION => {
            let params: ReopenSessionParams = parse_params(request)?;
            // Register the durable route before contacting the agent: the
            // reopen can synchronously emit session updates that must resolve
            // canonical persistence during the in-flight request.
            if let Some(raw) = params.conversation_id.as_deref() {
                match ConversationId::parse(raw) {
                    Ok(conversation_id) => {
                        manager.register_conversation_binding(&params.session_id.0, conversation_id)
                    }
                    Err(_) => {
                        log::warn!(
                            "[acp-core] binding skipped: invalid conversationId in {}",
                            request.method
                        );
                    }
                }
            }
            let outcome: SessionReopenOutcome = if request.method == METHOD_LOAD_SESSION {
                manager
                    .load_session(
                        &params.agent_id,
                        params.session_id,
                        params.cwd,
                        Vec::new(),
                        params.mcp_servers.unwrap_or_default(),
                    )
                    .await
            } else {
                manager
                    .resume_session(
                        &params.agent_id,
                        params.session_id,
                        params.cwd,
                        Vec::new(),
                        params.mcp_servers.unwrap_or_default(),
                    )
                    .await
            }
            .map_err(invalid)?;
            serde_json::to_value(outcome).map_err(|error| invalid(error.to_string()))
        }
        METHOD_CLOSE_SESSION => {
            let params: SessionParams = parse_params(request)?;
            let retirement_id = params.session_id.0.clone();
            let result = if let Some(conversation_id) =
                manager.conversation_id_for_current_session(&params.session_id.0)
            {
                let service = crate::conversation::ConversationLifecycleService::from_terminal(
                    Arc::clone(manager),
                    super::handles::TerminalServiceHandle::from_runtime(Arc::new(
                        super::handles::DetachedTerminalRuntime,
                    )),
                )
                .map_err(|error| invalid(error.to_string()))?;
                let expected_revision = state
                    .creation
                    .repository()
                    .get_conversation(conversation_id)
                    .map_err(|error| invalid(error.to_string()))?
                    .last_seq;
                service
                    .suspend_agent_binding(conversation_id, expected_revision)
                    .await
                    .map(|_| ())
                    .map_err(|error| invalid(error.to_string()))
            } else {
                manager
                    .close_session(&params.agent_id, params.session_id)
                    .await
                    .map_err(invalid)
            };
            result?;
            state
                .relay
                .retire_session(&retirement_id)
                .await
                .map_err(|code| invalid(format!("CONVERSATION_RETIREMENT_FAILED:{code}")))?;
            Ok(Value::Null)
        }
        METHOD_DISPOSE_EPHEMERAL_SESSION => {
            let params: SessionParams = parse_params(request)?;
            let retirement_id = params.session_id.0.clone();
            manager
                .dispose_ephemeral_session(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            state
                .relay
                .retire_session(&retirement_id)
                .await
                .map_err(|code| invalid(format!("CONVERSATION_RETIREMENT_FAILED:{code}")))?;
            Ok(Value::Null)
        }
        METHOD_IS_EPHEMERAL_SESSION => {
            let params: SessionParams = parse_params(request)?;
            let ephemeral = manager
                .is_ephemeral_session(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            Ok(json!({ "ephemeral": ephemeral }))
        }
        METHOD_LIST_SESSIONS => {
            let params: ListSessionsParams = parse_params(request)?;
            let response: ListSessionsResponse = manager
                .list_sessions(&params.agent_id, params.cwd, params.cursor)
                .await
                .map_err(invalid)?;
            serde_json::to_value(response).map_err(|error| invalid(error.to_string()))
        }
        METHOD_REGISTER_DISCOVERED_SESSION => {
            let params: RegisterDiscoveredParams = parse_params(request)?;
            if params.session_id.trim().is_empty() || params.cwd.trim().is_empty() {
                return Err(invalid("session id and cwd are required"));
            }
            let persistence = manager
                .persistence()
                .ok_or_else(|| invalid("session persistence unavailable"))?;
            let metadata = persistence
                .register_discovered_session(
                    SessionRegistration {
                        session_id: params.session_id,
                        stable_agent_namespace: manager
                            .stable_agent_namespace(&params.agent_id)
                            .map_err(invalid)?,
                        runtime_agent_id: Some(params.agent_id.0),
                        project_id: params.project_id,
                        cwd: params.cwd.into(),
                        ..Default::default()
                    },
                    params.title,
                    params.updated_at,
                )
                .await
                .map_err(|error| invalid(error.to_string()))?;
            serde_json::to_value(&metadata).map_err(|error| invalid(error.to_string()))
        }
        METHOD_CONVERSATION_ID_FOR_SESSION => {
            let params: SessionIdParams = parse_params(request)?;
            Ok(json!({
                "conversationId": manager.conversation_id_for_current_session(&params.session_id),
            }))
        }
        METHOD_SEND_PROMPT => {
            let params: SendPromptParams = parse_params(request)?;
            let blocks = match (params.content, params.text) {
                (Some(blocks), _) if !blocks.is_empty() => blocks,
                (_, Some(text)) => vec![ContentBlock::Text(TextContent::new(text))],
                (Some(_), None) => return Err(invalid("prompt content must not be empty")),
                (None, None) => return Err(invalid("send_prompt requires either content or text")),
            };
            let agent_id = manager
                .ensure_session_on_live_agent(&params.agent_id, &params.session_id)
                .await
                .map_err(invalid)?;
            let ephemeral = manager
                .is_ephemeral_session(&agent_id, params.session_id.clone())
                .await
                .map_err(invalid)?;
            if !ephemeral {
                let bound = manager
                    .conversation_id_for_current_session(&params.session_id.0)
                    .is_some();
                if bound {
                    if let Err(error) = crate::acp::commands::persist_accepted_prompt(
                        &state.relay,
                        &agent_id,
                        &params.session_id,
                        &blocks,
                    )
                    .await
                    {
                        log::warn!(
                            "[acp-core] failed to persist accepted prompt for session {} (agent {}): {error}",
                            params.session_id.0,
                            agent_id.0
                        );
                        return Err(invalid(format!(
                            "failed to persist accepted prompt: {error}"
                        )));
                    }
                } else {
                    log::warn!(
                        "[acp-core] accepted prompt not persisted: session {} has no Conversation binding (agent {})",
                        params.session_id.0,
                        agent_id.0
                    );
                }
            }
            let stop_reason: StopReason = manager
                .send_prompt(&agent_id, params.session_id, blocks, params.turn_id)
                .await
                .map_err(invalid)?;
            serde_json::to_value(stop_reason).map_err(|error| invalid(error.to_string()))
        }
        METHOD_CANCEL_PROMPT => {
            let params: SessionParams = parse_params(request)?;
            manager
                .cancel_prompt(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_OWNS_SESSION => {
            let params: SessionParams = parse_params(request)?;
            let owns = manager
                .owns_session(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            Ok(json!({ "owns": owns }))
        }
        METHOD_IS_TURN_ACTIVE => {
            let params: SessionParams = parse_params(request)?;
            let active = manager
                .is_turn_active(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            Ok(json!({ "active": active }))
        }
        METHOD_WAIT_TURN_IDLE => {
            let params: SessionParams = parse_params(request)?;
            manager
                .wait_turn_idle(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_RESPOND_PERMISSION => {
            let params: RespondPermissionParams = parse_params(request)?;
            manager
                .respond_permission(&params.agent_id, params.request_id, params.option_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_ANSWER_QUESTION => {
            let params: AnswerQuestionParams = parse_params(request)?;
            manager
                .answer_question(&params.agent_id, params.question_id, params.values)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_PERMISSION_INFO => {
            let params: PermissionInfoParams = parse_params(request)?;
            let Some(rdz) = state.relay.rendezvous() else {
                return Err(invalid("unknown permission request"));
            };
            let Some(agent_id) = rdz.agent_for_request(&params.request_id) else {
                return Err(invalid(format!(
                    "unknown permission request: {}",
                    params.request_id
                )));
            };
            let Some(session_id) = rdz.session_for_request(&params.request_id) else {
                return Err(invalid(format!(
                    "unknown permission request: {}",
                    params.request_id
                )));
            };
            Ok(json!({ "agentId": agent_id, "sessionId": session_id }))
        }
        METHOD_QUESTION_INFO => {
            let params: QuestionInfoParams = parse_params(request)?;
            let Some(rdz) = state.relay.question_rendezvous() else {
                return Err(invalid("unknown question request"));
            };
            let Some(agent_id) = rdz.agent_for_question(&params.question_id) else {
                return Err(invalid(format!(
                    "unknown question request: {}",
                    params.question_id
                )));
            };
            let Some(session_id) = rdz.session_for_question(&params.question_id) else {
                return Err(invalid(format!(
                    "unknown question request: {}",
                    params.question_id
                )));
            };
            Ok(json!({ "agentId": agent_id, "sessionId": session_id }))
        }
        METHOD_AUTHENTICATE => {
            let params: AuthenticateParams = parse_params(request)?;
            manager
                .authenticate(&params.agent_id, params.method_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SET_MODE => {
            let params: SetModeParams = parse_params(request)?;
            manager
                .set_mode(&params.agent_id, params.session_id, params.mode_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SET_MODEL => {
            let params: SetModelParams = parse_params(request)?;
            manager
                .set_model(&params.agent_id, params.session_id, params.model_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SET_CONFIG_OPTION => {
            let params: SetConfigOptionParams = parse_params(request)?;
            let options: Vec<SessionConfigOption> = manager
                .set_config_option(
                    &params.agent_id,
                    params.session_id,
                    params.config_id,
                    params.value_id,
                )
                .await
                .map_err(invalid)?;
            serde_json::to_value(options).map_err(|error| invalid(error.to_string()))
        }
        METHOD_COMPOSER_CONTROLS => {
            let params: SessionParams = parse_params(request)?;
            let controls = manager
                .composer_controls(&params.agent_id, params.session_id)
                .await
                .map_err(invalid)?;
            serde_json::to_value(controls).map_err(|error| invalid(error.to_string()))
        }
        METHOD_HISTORY_LIST => serde_json::to_value(state.persistence.list_sessions())
            .map_err(|error| invalid(error.to_string())),
        METHOD_HISTORY_GET => {
            let params: SessionIdParams = parse_params(request)?;
            match crate::commands::materialize_acp_history_with_ceiling(
                &state.persistence,
                &params.session_id,
            ) {
                Ok((metadata, records)) => {
                    let payload = crate::acp::session_payload::materialize_session_payload(
                        &metadata, &records,
                    );
                    serde_json::to_value(&payload).map_err(|error| invalid(error.to_string()))
                }
                Err(error) if error.code == "CONVERSATION_NOT_FOUND" => Ok(Value::Null),
                Err(error) => Err(invalid(format!(
                    "failed to read Conversation history: {}:{}",
                    error.code, error.detail
                ))),
            }
        }
        METHOD_HISTORY_GET_PAGE => {
            let params: HistoryPageParams = parse_params(request)?;
            let page = state
                .persistence
                .history_page_blocking(
                    params.session_id,
                    params.after_seq,
                    params.limit,
                    params.target_last_seq,
                )
                .await
                .map_err(|error| {
                    invalid(format!(
                        "failed to read Conversation history page: {}:{}",
                        error.code, error.detail
                    ))
                })?;
            serde_json::to_value(&page).map_err(|error| invalid(error.to_string()))
        }
        METHOD_HISTORY_CURSOR => {
            let params: SessionIdParams = parse_params(request)?;
            let watermark = state.persistence.last_seq(&params.session_id).unwrap_or(0);
            Ok(json!({ "sessionId": params.session_id, "watermark": watermark }))
        }
        METHOD_HISTORY_OPEN => {
            let params: SessionIdParams = parse_params(request)?;
            match state.persistence.last_seq(&params.session_id) {
                Ok(_) => Ok(json!({ "sessionId": params.session_id })),
                Err(error) if error.code == "CONVERSATION_NOT_FOUND" => {
                    Err(invalid("session payload not found"))
                }
                Err(error) => Err(invalid(format!(
                    "failed to open persisted session: {}: {}",
                    error.code, error.detail
                ))),
            }
        }
        METHOD_WORKSPACE_ENSURE_TERMINAL_REF_WRITABLE => {
            let params: WorkspaceConversationParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            state
                .workspace
                .ensure_terminal_ref_writable(conversation_id, params.writable.unwrap_or(true))
                .map_err(|error| invalid(format!("{}:{}", error.code.as_str(), error.detail)))?;
            Ok(Value::Null)
        }
        METHOD_WORKSPACE_ADD_TERMINAL_REF => {
            let params: WorkspaceTerminalRefParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            state
                .workspace
                .add_terminal_ref(conversation_id, &params.terminal_id)
                .await
                .map_err(|error| invalid(format!("{}:{}", error.code.as_str(), error.detail)))?;
            Ok(Value::Null)
        }
        METHOD_WORKSPACE_REMOVE_TERMINAL_REF_AFTER_TERMINATION => {
            let params: WorkspaceTerminalRefParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            state
                .workspace
                .remove_terminal_ref_after_termination(conversation_id, &params.terminal_id)
                .await
                .map_err(|error| invalid(format!("{}:{}", error.code.as_str(), error.detail)))?;
            Ok(Value::Null)
        }
        METHOD_SUBSCRIBE_EVENTS => Ok(json!({
            "watermark": state.events.seq.load(Ordering::Relaxed),
        })),
        METHOD_STOP_PRODUCERS => {
            state.manager.stop_producers().await.map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SHUTDOWN_PERSISTENCE => {
            state
                .manager
                .shutdown_persistence()
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SHUTDOWN => Ok(Value::Null),
        METHOD_CONVERSATION_HOST_STATUS => {
            let outcome = state
                .application
                .host_status()
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_LIST => to_json(state.application.list_conversations()),
        METHOD_CONVERSATION_OPEN => {
            require_core_host_admission()?;
            let params: ConversationIdOnlyParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let outcome = state
                .application
                .open_conversation(conversation_id)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_RENAME => {
            let params: ConversationRenameParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let record = state
                .application
                .rename_conversation(conversation_id, params.title)
                .await
                .map_err(conversation_application_err)?;
            to_json(record)
        }
        METHOD_CONVERSATION_PREPARE_TERMINAL => {
            let params: ConversationRequestParams = parse_params(request)?;
            let mut prepared_request: PrepareConversationRequest =
                payload_from_value(params.request)?;
            // The caller names the folder; the backend is this command's identity and is
            // never taken from the payload. A request that could ask for `agent` here
            // would be a second, unaudited way to create an agent Conversation.
            prepared_request.backend = ConversationBackend::Terminal;
            let prepared = state
                .creation
                .prepare_conversation(prepared_request)
                .await
                .map_err(conversation_creation_err)?;
            to_json(prepared)
        }
        METHOD_CONVERSATION_PROVISION_TERMINAL => {
            let params: ConversationProvisionTerminalParams = parse_params(request)?;
            let conversation_id = ConversationId::parse(&params.conversation_id)
                .map_err(|error| invalid(format!("conversationId is not a UUID: {error}")))?;
            state
                .creation
                .provision_terminal(conversation_id, &params.terminal_id)
                .await
                .map_err(conversation_creation_err)?;
            to_json(())
        }
        METHOD_CONVERSATION_RECOVERY_RESOLVE => {
            let params: ConversationRequestParams = parse_params(request)?;
            let recovery_request: crate::conversation::migration::ResolveRecoveryItemRequest =
                payload_from_value(params.request)?;
            let outcome = state
                .application
                .resolve_recovery_item(recovery_request)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_ATTACH_PROJECT => {
            require_core_host_admission()?;
            let params: ConversationAttachProjectParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let attachment: ProjectAttachment = payload_from_value(params.attachment)?;
            let outcome = state
                .application
                .attach_project(conversation_id, params.expected_revision, attachment)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_DETACH_PROJECT => {
            require_core_host_admission()?;
            let params: ConversationRevisionParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let outcome = state
                .application
                .detach_project(conversation_id, params.expected_revision)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_UPDATE_EXECUTION_TARGET => {
            require_core_host_admission()?;
            let params: ConversationUpdateExecutionTargetParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let execution_target: ExecutionTarget = payload_from_value(params.execution_target)?;
            let outcome = state
                .application
                .update_execution_target(
                    conversation_id,
                    params.expected_revision,
                    execution_target,
                )
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_DETACH_BINDING => {
            let params: ConversationRevisionParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let outcome = state
                .application
                .detach_binding(conversation_id, params.expected_revision)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_REBIND_BINDING => {
            let params: ConversationRevisionParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let outcome = state
                .application
                .rebind_binding(conversation_id, params.expected_revision)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_SUSPEND_BINDING => {
            let params: ConversationRevisionParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let outcome = state
                .application
                .suspend_binding(conversation_id, params.expected_revision)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_REPLACE_BINDING => {
            let params: ConversationReplaceBindingParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let replace_request: PrepareConversationRequest = payload_from_value(params.request)?;
            let outcome = state
                .application
                .replace_binding(
                    conversation_id,
                    replace_request,
                    params.expected_revision,
                    params.target_runtime_agent_id,
                )
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_DELETE => {
            require_core_host_admission()?;
            let params: ConversationDeleteParams = parse_params(request)?;
            let conversation_id = parse_conversation_id_component(&params.conversation_id)?;
            let workspace_cwd = state
                .application
                .get_conversation(conversation_id)
                .ok()
                .map(|record| record.workspace_cwd);
            let current_session_id = match state
                .application
                .writer()
                .repository()
                .current_binding(conversation_id)
            {
                Ok(binding) => binding.map(|binding| binding.agent_session_id),
                Err(_) => {
                    return Err(invalid(
                        "CONVERSATION_RECOVERY_REQUIRED:failed to resolve Conversation binding before delete",
                    ))
                }
            };
            let outcome = state
                .application
                .delete_conversation(conversation_id, params.expected_revision)
                .await
                .map_err(conversation_application_err)?;
            if let Err(code) = retire_deleted_binding_if_updated(
                &state.relay,
                current_session_id.as_deref(),
                &outcome,
            )
            .await
            {
                log::error!(
                    "[conversation-retirement] operation=acp_core_delete code={} conversation_id={}",
                    code,
                    conversation_id
                );
                return Err(invalid(
                    "CONVERSATION_RETIREMENT_FAILED:Conversation auxiliary retirement failed",
                ));
            }
            if params.remove_workspace == Some(true) {
                if let Some(path) = workspace_cwd.filter(|path| !path.trim().is_empty()) {
                    // User-confirmed recursive removal of the Conversation workspace
                    // directory; best-effort so a locked file cannot veto the delete.
                    if let Err(error) = std::fs::remove_dir_all(&path) {
                        log::warn!(
                            "[conversation-delete] workspace removal failed conversation_id={} path={} error={error}",
                            params.conversation_id,
                            path
                        );
                    } else {
                        log::info!(
                            "[conversation-delete] workspace removed conversation_id={} path={}",
                            params.conversation_id,
                            path
                        );
                    }
                }
            }
            to_json(outcome)
        }
        METHOD_CONVERSATION_GET => {
            let params: ConversationIdParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            let record = state
                .application
                .get_conversation(conversation_id)
                .map_err(conversation_application_err)?;
            to_json(record)
        }
        METHOD_CONVERSATION_GET_BINDING => {
            let params: ConversationIdParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            let binding = state
                .application
                .current_binding(conversation_id)
                .map_err(conversation_application_err)?;
            to_json(binding)
        }
        METHOD_CONVERSATION_RESOLVE_LEGACY_ID => {
            let key: crate::conversation::LegacyConversationKey = parse_params(request)?;
            let resolution = state
                .application
                .resolve_legacy_conversation_id(key)
                .map_err(conversation_application_err)?;
            to_json(resolution)
        }
        METHOD_CONVERSATION_GET_WORKSPACE => {
            let params: ConversationIdParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            let outcome = state
                .application
                .get_workspace(conversation_id)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_CONVERSATION_WRITE_WORKSPACE => {
            require_core_host_admission()?;
            let params: ConversationWriteWorkspaceParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            let workspace = serde_json::from_value(params.workspace)
                .map_err(|error| invalid(format!("payload validation failed: {error}")))?;
            let outcome = state
                .application
                .write_workspace(conversation_id, params.based_revision, workspace)
                .await
                .map_err(conversation_application_err)?;
            to_json(outcome)
        }
        METHOD_SCHEDULED_TASK_PREVIEW => {
            let params: ScheduledTaskPreviewParams = parse_params(request)?;
            let preview = state
                .scheduled_tasks
                .preview(&params.schedule, params.count.unwrap_or(5))
                .map_err(|error| invalid(error.to_string()))?;
            to_json(preview)
        }
        METHOD_SCHEDULED_TASK_LIST => {
            let params: ScheduledTaskListParams = if request.params.is_null() {
                ScheduledTaskListParams::default()
            } else {
                parse_params(request)?
            };
            let tasks = state
                .scheduled_tasks
                .list_tasks(params.project_id.as_deref())
                .map_err(|error| invalid(error.to_string()))?;
            to_json(tasks)
        }
        METHOD_SCHEDULED_TASK_GET => {
            let params: ScheduledTaskIdParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .get_task(&params.task_id)
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_DRAFT_CREATE => {
            let params: ScheduledTaskDraftCreateParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .create_draft(params.input, scheduled_task_human_context())
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_DRAFT_UPDATE => {
            let params: ScheduledTaskDraftUpdateParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .update_draft(
                    &params.task_id,
                    params.request.expected_revision,
                    params.request.input,
                    scheduled_task_human_context(),
                )
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_ACTIVATE => {
            let params: ScheduledTaskActivateParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .activate(
                    &params.task_id,
                    params.request.expected_revision,
                    &params.request.expected_draft_hash,
                    scheduled_task_human_context(),
                )
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_PAUSE => {
            let params: ScheduledTaskRevisionParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .pause(
                    &params.task_id,
                    params.request.expected_revision,
                    scheduled_task_human_context(),
                )
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_RESUME => {
            let params: ScheduledTaskRevisionParams = parse_params(request)?;
            let task = state
                .scheduled_tasks
                .resume(
                    &params.task_id,
                    params.request.expected_revision,
                    scheduled_task_human_context(),
                )
                .map_err(|error| invalid(error.to_string()))?;
            to_json(task)
        }
        METHOD_SCHEDULED_TASK_DELETE => {
            let params: ScheduledTaskRevisionParams = parse_params(request)?;
            state
                .scheduled_tasks
                .delete(
                    &params.task_id,
                    params.request.expected_revision,
                    scheduled_task_human_context(),
                )
                .map_err(|error| invalid(error.to_string()))?;
            to_json(())
        }
        METHOD_SCHEDULED_TASK_RUN_NOW => {
            let params: ScheduledTaskIdParams = parse_params(request)?;
            let run = state
                .scheduled_tasks
                .run_now(&params.task_id)
                .map_err(|error| invalid(error.to_string()))?;
            to_json(run)
        }
        METHOD_SCHEDULED_TASK_RETRY_RUN => {
            let params: ScheduledTaskRetryRunParams = parse_params(request)?;
            let run = state
                .scheduled_tasks
                .retry_run(&params.task_id, &params.run_id)
                .map_err(|error| invalid(error.to_string()))?;
            to_json(run)
        }
        METHOD_SCHEDULED_TASK_LIST_RUNS => {
            let params: ScheduledTaskIdParams = parse_params(request)?;
            let runs = state
                .scheduled_tasks
                .list_runs(&params.task_id)
                .map_err(|error| invalid(error.to_string()))?;
            to_json(runs)
        }
        METHOD_SCHEDULED_TASK_LIST_AUDIT => {
            let params: ScheduledTaskIdParams = parse_params(request)?;
            let events = state
                .scheduled_tasks
                .list_audit(&params.task_id)
                .map_err(|error| invalid(error.to_string()))?;
            to_json(events)
        }
        METHOD_MEMORY_BUILD => {
            let params: MemoryIndexBuildArgs = parse_params(request)?;
            let service = Arc::clone(&state.memory);
            let events = Arc::clone(&state.events);
            let relay = Arc::clone(&state.relay);
            log::info!(
                target: "se_manager::memory_index",
                "operation=memory_index_build full_rebuild={} index_unscoped={}",
                params.full_rebuild,
                params.index_unscoped
            );
            let report = tokio::task::spawn_blocking(move || {
                let mut emit = throttled(
                    move |progress: &crate::memory_index::ingest::IngestProgress| {
                        emit_memory_progress(&events, &relay, progress);
                    },
                );
                service
                    .build(
                        &PathBuf::from(params.project_root),
                        &IngestOptions {
                            full_rebuild: params.full_rebuild,
                            index_unscoped: params.index_unscoped,
                            ..IngestOptions::default()
                        },
                        &mut emit,
                    )
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| invalid(format!("memory index build join failed: {error}")))?
            .map_err(invalid)?;
            to_json(report)
        }
        METHOD_MEMORY_CANCEL => {
            let params: MemoryIndexScopeArgs = parse_params(request)?;
            let cancelled = state
                .memory
                .cancel_build(&PathBuf::from(params.project_root))
                .map_err(|error| invalid(error.to_string()))?;
            to_json(cancelled)
        }
        METHOD_MEMORY_STATUS => {
            let params: MemoryIndexScopeArgs = parse_params(request)?;
            let service = Arc::clone(&state.memory);
            let status = tokio::task::spawn_blocking(move || {
                service
                    .status(&PathBuf::from(params.project_root))
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| invalid(format!("memory index status join failed: {error}")))?
            .map_err(invalid)?;
            to_json(status)
        }
        METHOD_MEMORY_SEARCH => {
            let params: MemoryIndexSearchArgs = parse_params(request)?;
            let service = Arc::clone(&state.memory);
            let response = tokio::task::spawn_blocking(move || {
                service
                    .search(&PathBuf::from(params.project_root), &params.request)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| invalid(format!("memory index search join failed: {error}")))?
            .map_err(invalid)?;
            to_json(response)
        }
        METHOD_MEMORY_SESSIONS => {
            let params: MemoryIndexListArgs = parse_params(request)?;
            let service = Arc::clone(&state.memory);
            let sessions = tokio::task::spawn_blocking(move || {
                service
                    .list_sessions(
                        &PathBuf::from(params.project_root),
                        params.limit,
                        params.include_unscoped,
                        &params.agents,
                    )
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| invalid(format!("memory index sessions join failed: {error}")))?
            .map_err(invalid)?;
            to_json(sessions)
        }
        METHOD_MEMORY_SESSION => {
            let params: MemoryIndexSessionArgs = parse_params(request)?;
            let service = Arc::clone(&state.memory);
            let session = tokio::task::spawn_blocking(move || {
                service
                    .get_session(
                        &PathBuf::from(params.project_root),
                        &params.session_key,
                        params.limit,
                        params.include_stale,
                        params.include_unscoped,
                    )
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| invalid(format!("memory index session join failed: {error}")))?
            .map_err(invalid)?;
            to_json(session)
        }
        METHOD_MEMORY_MCP_INVOCATION => {
            let params: MemoryIndexScopeArgs = parse_params(request)?;
            let executable = std::env::current_exe()
                .map_err(|error| invalid(format!("could not resolve executable: {error}")))?;
            let invocation = crate::memory_index::stdio_mcp::invocation_for(
                &executable,
                &crate::memory_index::stdio_mcp::StdioConfig {
                    project_root: Some(PathBuf::from(params.project_root)),
                    state_root: state.memory.state_root().to_path_buf(),
                },
            );
            to_json(invocation)
        }
        METHOD_MEMORY_UNIVERSAL_MCP_INVOCATION => {
            let executable = std::env::current_exe()
                .map_err(|error| invalid(format!("could not resolve executable: {error}")))?;
            let invocation = crate::memory_index::stdio_mcp::universal_invocation_for(
                &executable,
                state.memory.state_root(),
            );
            to_json(invocation)
        }
        METHOD_RETIRE_SESSION => {
            let params: SessionIdParams = parse_params(request)?;
            state
                .relay
                .retire_session(&params.session_id)
                .await
                .map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_FLUSH_CATALOG => {
            let deadline = tokio::time::Instant::now() + crate::conversation::DEFAULT_DRAIN_TIMEOUT;
            state
                .relay
                .flush_catalog_until(deadline)
                .await
                .map_err(|_| invalid("flushCatalog failed"))?;
            Ok(Value::Null)
        }
        METHOD_REGISTER_CONVERSATION_BINDING => {
            let params: RegisterBindingParams = parse_params(request)?;
            let conversation_id = parse_conversation_id(&params.conversation_id)?;
            manager.register_conversation_binding(&params.session_id, conversation_id);
            Ok(Value::Null)
        }
        METHOD_STABLE_AGENT_NAMESPACE => {
            let params: AgentIdParams = parse_params(request)?;
            let namespace = manager
                .stable_agent_namespace(&params.agent_id)
                .map_err(invalid)?;
            Ok(json!({ "namespace": namespace }))
        }
        METHOD_LIST_RUNNING_NAMESPACES => serde_json::to_value(manager.list_running_namespaces())
            .map_err(|error| invalid(error.to_string())),
        other => Err(invalid(format!(
            "acp method '{other}' is not exported over core IPC"
        ))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdParams {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBindingParams {
    session_id: String,
    conversation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationIdParams {
    conversation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationWriteWorkspaceParams {
    conversation_id: String,
    #[serde(default)]
    based_revision: Option<u64>,
    workspace: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

struct ClientInner {
    writer: tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<CoreResponse>>>,
    events: broadcast::Sender<AcpCoreEvent>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reconnect: tokio::sync::Mutex<()>,
}

/// Client for the ACP Core process. Mirrors `TerminalCoreClient`: one framed
/// connection, request/response by id, and a broadcast event stream carrying
/// verbatim `acp:*` envelopes for the GUI-side mirror.
#[derive(Clone)]
pub struct AcpCoreClient {
    inner: Arc<ClientInner>,
}

impl AcpCoreClient {
    #[cfg(unix)]
    pub async fn connect(endpoint: &CoreEndpoint) -> Result<Self, CoreError> {
        let (reader, writer) = open_acp_stream(endpoint).await?;
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let inner = Arc::new(ClientInner {
            writer: tokio::sync::Mutex::new(writer),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            events,
            reader: Mutex::new(None),
            reconnect: tokio::sync::Mutex::new(()),
        });
        spawn_acp_read_loop(&inner, reader);
        Ok(Self { inner })
    }

    #[cfg(not(unix))]
    pub async fn connect(_endpoint: &CoreEndpoint) -> Result<Self, CoreError> {
        Err(CoreError::UnsupportedPlatform)
    }

    /// Re-handshake against `endpoint` and swap the live stream. Pending RPCs
    /// and the event broadcast stay on the same inner Arc so GUI mirror tasks
    /// keep receiving after a Core restart.
    #[cfg(unix)]
    pub async fn reconnect(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        let _guard = self.inner.reconnect.lock().await;
        let (reader, writer) = open_acp_stream(endpoint).await?;
        {
            let mut writer_guard = self.inner.writer.lock().await;
            if let Some(handle) = self.inner.reader.lock().take() {
                handle.abort();
            }
            fail_stale_pending(&self.inner);
            *writer_guard = writer;
        }
        spawn_acp_read_loop(&self.inner, reader);
        Ok(())
    }

    #[cfg(not(unix))]
    pub async fn reconnect(&self, _endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        Err(CoreError::UnsupportedPlatform)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AcpCoreEvent> {
        self.inner.events.subscribe()
    }

    /// Raw request/response escape hatch used by the service-handle runtime.
    /// Resolves the full `CoreResponse` (result or error payload) so callers
    /// can map transport errors themselves.
    pub async fn raw_request(&self, request: CoreRequest) -> Result<CoreResponse, CoreError> {
        let method = request.method.clone();
        let id = request.id;
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(id, tx);
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
            let _ = (&request, id, method);
            self.inner.pending.lock().remove(&id);
            return Err(CoreError::UnsupportedPlatform);
        }
        let timeout = if method == METHOD_SEND_PROMPT
            || method == METHOD_WAIT_TURN_IDLE
            || method == METHOD_MEMORY_BUILD
        {
            LONG_RPC_TIMEOUT
        } else {
            RPC_TIMEOUT
        };
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| CoreError::Io(format!("acp core RPC '{method}' timed out")))?
            .map_err(|_| CoreError::Io("acp core RPC cancelled".into()))
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, CoreError> {
        let timeout = if method == METHOD_SEND_PROMPT
            || method == METHOD_WAIT_TURN_IDLE
            || method == METHOD_MEMORY_BUILD
        {
            LONG_RPC_TIMEOUT
        } else {
            RPC_TIMEOUT
        };
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(id, tx);
        let request = CoreRequest {
            id,
            method: method.to_string(),
            params,
        };
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
            let _ = timeout;
            return Err(CoreError::UnsupportedPlatform);
        }
        let response = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| CoreError::Io(format!("acp core RPC '{method}' timed out")))?
            .map_err(|_| CoreError::Io("acp core RPC cancelled".into()))?;
        if let Some(error) = response.error {
            if error.code == CoreError::Unauthorized.code() {
                return Err(CoreError::Unauthorized);
            }
            return Err(CoreError::InvalidRequest(error.message));
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, CoreError> {
        self.rpc(method, params).await
    }

    pub async fn health(&self) -> Result<Value, CoreError> {
        self.rpc(METHOD_HEALTH, Value::Null).await
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentId>, CoreError> {
        let value = self.rpc(METHOD_LIST_AGENTS, Value::Null).await?;
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
    }

    pub async fn spawn_agent(&self, config: AgentConfig) -> Result<Value, CoreError> {
        let params = serde_json::to_value(&config).map_err(|error| invalid(error.to_string()))?;
        self.rpc(METHOD_SPAWN_AGENT, params).await
    }

    pub async fn kill_agent(&self, agent_id: &AgentId) -> Result<(), CoreError> {
        self.rpc(METHOD_KILL_AGENT, json!({ "agentId": agent_id }))
            .await
            .map(|_| ())
    }

    pub async fn event_watermark(&self) -> Result<u64, CoreError> {
        let value = self.rpc(METHOD_SUBSCRIBE_EVENTS, Value::Null).await?;
        Ok(value
            .get("watermark")
            .and_then(Value::as_u64)
            .unwrap_or_default())
    }

    pub async fn shutdown(&self) -> Result<(), CoreError> {
        self.rpc(METHOD_SHUTDOWN, Value::Null).await.map(|_| ())
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        if let Some(handle) = self.reader.lock().take() {
            handle.abort();
        }
    }
}

#[cfg(unix)]
async fn open_acp_stream(
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
        role: CoreRole::AcpCore,
        protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
        client_name: "termul-gui".to_string(),
    };
    write_json_frame(&mut stream, &hello).await?;
    let ack: super::ipc::CoreHelloAck = read_json_frame(&mut stream).await?;
    if ack.role != CoreRole::AcpCore || ack.protocol_version != CURRENT_PROTOCOL_VERSION {
        return Err(CoreError::InvalidHandshake(
            "core returned an incompatible handshake".into(),
        ));
    }
    Ok(stream.into_split())
}

#[cfg(unix)]
fn spawn_acp_read_loop(inner: &Arc<ClientInner>, reader: tokio::net::unix::OwnedReadHalf) {
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
        if let Ok(response) = serde_json::from_slice::<CoreResponse>(&payload) {
            if let Some(tx) = inner.pending.lock().remove(&response.id) {
                let _ = tx.send(response);
            }
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<CoreEvent>(&payload) {
            if event.topic == ACP_EVENT_TOPIC {
                if let Ok(envelope) = serde_json::from_value::<AcpCoreEvent>(event.payload) {
                    let _ = inner.events.send(envelope);
                }
            }
        }
    }
    fail_stale_pending(&inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_core_module_does_not_depend_on_the_gui_runtime() {
        let source = include_str!("acp.rs");
        let forbidden = ["tau", "ri::"].concat();
        assert!(
            !source.contains(&forbidden),
            "ACP Core must stay independent of the GUI runtime"
        );
    }

    #[test]
    fn core_exports_the_full_control_surface() {
        assert_eq!(METHOD_SEND_PROMPT, "sendPrompt");
        assert_eq!(METHOD_HISTORY_GET_PAGE, "historyGetPage");
        assert_eq!(METHOD_HISTORY_CURSOR, "historyCursor");
        assert_eq!(METHOD_HISTORY_OPEN, "historyOpen");
        assert_eq!(METHOD_PERMISSION_INFO, "permissionInfo");
        assert_eq!(METHOD_QUESTION_INFO, "questionInfo");
        assert_eq!(METHOD_WORKSPACE_ADD_TERMINAL_REF, "workspaceAddTerminalRef");
        assert_eq!(METHOD_SUBSCRIBE_EVENTS, "subscribeEvents");
        assert_eq!(METHOD_CONVERSATION_HOST_STATUS, "conversationHostStatus");
        assert_eq!(METHOD_CONVERSATION_LIST, "conversationList");
        assert_eq!(METHOD_CONVERSATION_OPEN, "conversationOpen");
        assert_eq!(METHOD_CONVERSATION_RENAME, "conversationRename");
        assert_eq!(
            METHOD_CONVERSATION_PREPARE_TERMINAL,
            "conversationPrepareTerminal"
        );
        assert_eq!(
            METHOD_CONVERSATION_PROVISION_TERMINAL,
            "conversationProvisionTerminal"
        );
        assert_eq!(
            METHOD_CONVERSATION_RECOVERY_RESOLVE,
            "conversationRecoveryResolve"
        );
        assert_eq!(
            METHOD_CONVERSATION_ATTACH_PROJECT,
            "conversationAttachProject"
        );
        assert_eq!(
            METHOD_CONVERSATION_DETACH_PROJECT,
            "conversationDetachProject"
        );
        assert_eq!(
            METHOD_CONVERSATION_UPDATE_EXECUTION_TARGET,
            "conversationUpdateExecutionTarget"
        );
        assert_eq!(
            METHOD_CONVERSATION_DETACH_BINDING,
            "conversationDetachBinding"
        );
        assert_eq!(
            METHOD_CONVERSATION_REBIND_BINDING,
            "conversationRebindBinding"
        );
        assert_eq!(
            METHOD_CONVERSATION_SUSPEND_BINDING,
            "conversationSuspendBinding"
        );
        assert_eq!(
            METHOD_CONVERSATION_REPLACE_BINDING,
            "conversationReplaceBinding"
        );
        assert_eq!(METHOD_CONVERSATION_DELETE, "conversationDelete");
        assert_eq!(METHOD_SCHEDULED_TASK_PREVIEW, "scheduledTaskPreview");
        assert_eq!(METHOD_SCHEDULED_TASK_LIST, "scheduledTaskList");
        assert_eq!(METHOD_SCHEDULED_TASK_GET, "scheduledTaskGet");
        assert_eq!(
            METHOD_SCHEDULED_TASK_DRAFT_CREATE,
            "scheduledTaskDraftCreate"
        );
        assert_eq!(
            METHOD_SCHEDULED_TASK_DRAFT_UPDATE,
            "scheduledTaskDraftUpdate"
        );
        assert_eq!(METHOD_SCHEDULED_TASK_ACTIVATE, "scheduledTaskActivate");
        assert_eq!(METHOD_SCHEDULED_TASK_PAUSE, "scheduledTaskPause");
        assert_eq!(METHOD_SCHEDULED_TASK_RESUME, "scheduledTaskResume");
        assert_eq!(METHOD_SCHEDULED_TASK_DELETE, "scheduledTaskDelete");
        assert_eq!(METHOD_SCHEDULED_TASK_RUN_NOW, "scheduledTaskRunNow");
        assert_eq!(METHOD_SCHEDULED_TASK_RETRY_RUN, "scheduledTaskRetryRun");
        assert_eq!(METHOD_SCHEDULED_TASK_LIST_RUNS, "scheduledTaskListRuns");
        assert_eq!(METHOD_SCHEDULED_TASK_LIST_AUDIT, "scheduledTaskListAudit");
        assert_eq!(METHOD_MEMORY_BUILD, "memoryBuild");
        assert_eq!(METHOD_MEMORY_CANCEL, "memoryCancel");
        assert_eq!(METHOD_MEMORY_STATUS, "memoryStatus");
        assert_eq!(METHOD_MEMORY_SEARCH, "memorySearch");
        assert_eq!(METHOD_MEMORY_SESSIONS, "memorySessions");
        assert_eq!(METHOD_MEMORY_SESSION, "memorySession");
        assert_eq!(METHOD_MEMORY_MCP_INVOCATION, "memoryMcpInvocation");
        assert_eq!(
            METHOD_MEMORY_UNIVERSAL_MCP_INVOCATION,
            "memoryUniversalMcpInvocation"
        );
    }

    #[tokio::test]
    async fn ipc_event_sink_envelopes_events_with_a_monotonic_watermark() {
        let shared = Arc::new(SharedEvents::default());
        let sink = CoreIpcEventSink::install(&shared);
        let mut receiver = shared.sender().expect("sender installed").subscribe();

        sink.emit(&AcpEvent {
            sid: Some("session-1".into()),
            type_: "acp:message_chunk",
            payload: json!({"chunk": "hi"}),
        })
        .expect("ipc sink never rejects");
        sink.emit(&AcpEvent {
            sid: None,
            type_: "acp:agent_spawned",
            payload: json!({"agentId": "a1"}),
        })
        .expect("ipc sink never rejects");

        let first = receiver.recv().await.expect("first event");
        let envelope: AcpCoreEvent =
            serde_json::from_value(first.payload).expect("envelope decodes");
        assert_eq!(envelope.seq, 1);
        assert_eq!(envelope.type_, "acp:message_chunk");
        assert_eq!(envelope.sid.as_deref(), Some("session-1"));
        assert_eq!(envelope.data, json!({"chunk": "hi"}));

        let second = receiver.recv().await.expect("second event");
        let envelope: AcpCoreEvent =
            serde_json::from_value(second.payload).expect("envelope decodes");
        assert_eq!(envelope.seq, 2);
        assert_eq!(envelope.type_, "acp:agent_spawned");
        assert_eq!(shared.seq.load(Ordering::Relaxed), 2);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acp_core_serves_the_control_plane_and_events_over_ipc() {
        let profile = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);
        let state_root = profile.path().to_path_buf();
        let workspace_root = workspace.path().to_path_buf();
        let server =
            tokio::spawn(async move { run_acp_core_with_roots(state_root, workspace_root).await });

        let client = wait_for_client(&endpoint).await;
        let health = client.health().await.expect("health");
        assert_eq!(health["role"], "acp-core");
        assert_eq!(health["status"], "ready");

        assert!(client.list_agents().await.expect("listAgents").is_empty());

        let watermark = client.event_watermark().await.expect("watermark");
        assert_eq!(watermark, 0);

        let error = client
            .request("definitelyNotAMethod", Value::Null)
            .await
            .expect_err("unknown method is rejected");
        assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST");
        assert!(
            error.client_message().contains("definitelyNotAMethod")
                || matches!(error, CoreError::InvalidRequest(detail) if detail.contains("definitelyNotAMethod"))
        );

        let history = client
            .request(METHOD_HISTORY_LIST, Value::Null)
            .await
            .expect("historyList");
        assert_eq!(history.as_array().map(Vec::len), Some(0));

        client.shutdown().await.expect("shutdown");
        let _ = tokio::time::timeout(Duration::from_secs(10), server).await;
        workspace.close().unwrap();
        profile.close().unwrap();
    }

    #[cfg(unix)]
    async fn wait_for_client(endpoint: &CoreEndpoint) -> AcpCoreClient {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match AcpCoreClient::connect(endpoint).await {
                Ok(client) => return client,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => panic!("acp core did not become ready: {error}"),
            }
        }
    }

    #[cfg(unix)]
    async fn wait_for_reconnect(client: &AcpCoreClient, endpoint: &CoreEndpoint) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match client.reconnect(endpoint).await {
                Ok(()) => return,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => panic!("acp core reconnect failed: {error}"),
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acp_core_client_reconnects_to_a_new_server_on_the_same_endpoint() {
        let profile = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);
        let state_root = profile.path().to_path_buf();
        let workspace_root = workspace.path().to_path_buf();
        let server =
            tokio::spawn(async move { run_acp_core_with_roots(state_root, workspace_root).await });

        let client = wait_for_client(&endpoint).await;
        client.health().await.expect("health before reconnect");
        let _watermark = client.event_watermark().await.expect("watermark");

        client.shutdown().await.expect("shutdown first server");
        let _ = tokio::time::timeout(Duration::from_secs(10), server).await;

        let state_root = profile.path().to_path_buf();
        let workspace_root = workspace.path().to_path_buf();
        let server =
            tokio::spawn(async move { run_acp_core_with_roots(state_root, workspace_root).await });

        wait_for_reconnect(&client, &endpoint).await;
        let health = client.health().await.expect("health after reconnect");
        assert_eq!(health["role"], "acp-core");
        assert_eq!(health["status"], "ready");
        let _watermark = client
            .event_watermark()
            .await
            .expect("watermark after reconnect");

        client.shutdown().await.expect("shutdown second server");
        let _ = tokio::time::timeout(Duration::from_secs(10), server).await;
        workspace.close().unwrap();
        profile.close().unwrap();
    }
}
