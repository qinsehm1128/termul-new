//! Web ACP Agent runtime — headless server + browser client support.
//!
//! This module owns the transport-neutral seams the `acp` dispatcher emits
//! through, plus the standalone Axum server (Stories 1.2–1.3) and the live WS
//! relay (Story 1.4).
//!
//! - Desktop registers a [`sink::TauriEventSink`] (`acp:*` Tauri events).
//! - Standalone `se-server` registers a live [`sink::WsRelaySink`] (Story
//!   1.4 — owns per-session event logs + seq counters + subscriber set) and
//!   calls [`serve`].
//! - Dev static serving of `dist-web/` is [`assets`] (Story 1.3); production
//!   rust-embed embedding/serving is complete.
//!
//! Auth / sandbox land in later stories. The WS relay protocol (envelope, seq,
//! event log, cursor, tiers) is [`ws`] (Story 1.4).

pub mod assets;
pub mod auth;
pub mod catalog_api;
pub mod cli_session_api;
pub mod config;
pub mod conversation_api;
pub mod conversation_lifecycle_api;
pub mod editor_workspaces_api;
pub mod fs_api;
// Rust 1.95 diagnoses three legacy callback adapters inside this pre-existing
// module. TASK-004 cannot rewrite that non-owned file, so keep the allowance
// scoped to `git_api` rather than weakening the crate-wide lint gate.
#[allow(clippy::redundant_closure)]
pub mod git_api;
pub mod install_api;
pub mod log_api;
pub mod mcp_probe_api;
pub mod mcp_servers_api;
pub mod operation_policy;
pub mod permissions;
pub mod project_registry;
pub mod projects_api;
pub mod router;
pub mod scheduled_tasks_api;
pub mod search_api;
pub mod session_workspace_api;
pub mod sink;
pub mod skills_api;
pub mod store;
pub mod terminal_ws;
pub mod upgraded_connections;
pub mod workspace_api;
pub mod worktree_api;
pub mod ws;

#[cfg(test)]
mod conversation_golden_tests;

pub use auth::{RemoteAccessAuthority, RemoteCapability, RemotePrincipal};
pub use config::ServerConfig;
pub use permissions::PermissionRendezvous;
pub use permissions::QuestionRendezvous;
pub use project_registry::{
    seed_from_file, ProjectGroupSummary, ProjectListPayload, ProjectRegistry, ProjectSummary,
    ProjectsChangedPayload,
};
pub use sink::{
    broadcast_chat_history_changed, broadcast_projects_changed, fan_out, EventSink, TauriEventSink,
    WsRelaySink,
};
pub use ws::{AppState, HistoryMode, ReliabilityTier, RuntimePolicy, SequencedEvent, WsErrorCode};

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::acp::AcpManager;
use crate::pty::PtyManager;
use crate::trackers::{CwdTracker, ExitCodeTracker, GitTracker, TerminalEventHub};
use crate::web::store::WebStore;

#[cfg(test)]
pub(crate) fn test_pty_manager() -> Arc<PtyManager> {
    let events = TerminalEventHub::standalone();
    let cwd = Arc::new(CwdTracker::new(events.clone()));
    let git = Arc::new(GitTracker::new(None, events.clone()));
    let exit = Arc::new(ExitCodeTracker::new(events.clone()));
    Arc::new(PtyManager::new(events, cwd, git, exit))
}

pub(crate) const ACP_PRODUCER_STOP_FAILED: &str = "ACP_PRODUCER_STOP_FAILED";
pub(crate) const CONVERSATION_PERSISTENCE_DRAIN_FAILED: &str =
    "CONVERSATION_PERSISTENCE_DRAIN_FAILED";
pub(crate) const CONVERSATION_CATALOG_FLUSH_FAILED: &str = "CATALOG_FLUSH_FAILED";
pub(crate) const ACP_PERSISTENCE_SHUTDOWN_FAILED: &str = "ACP_PERSISTENCE_SHUTDOWN_FAILED";
pub(crate) const PTY_CLEANUP_FAILED: &str = "PTY_CLEANUP_FAILED";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StandaloneShutdownReceipt {
    pty_shutdown: crate::pty::manager::PtyShutdownReceipt,
    connections_active: u64,
    connections_failed: u64,
    connections_timed_out: u64,
}

#[derive(Debug)]
struct StandaloneShutdownError {
    codes: Vec<&'static str>,
    receipt: StandaloneShutdownReceipt,
}

impl fmt::Display for StandaloneShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "standalone shutdown failed: {} (pty attempted={} succeeded={} failed={} in_flight={} connections active={} failed={} timed_out={})",
            self.codes.join(","),
            self.receipt.pty_shutdown.attempted,
            self.receipt.pty_shutdown.succeeded,
            self.receipt.pty_shutdown.failed,
            self.receipt.pty_shutdown.in_flight,
            self.receipt.connections_active,
            self.receipt.connections_failed,
            self.receipt.connections_timed_out
        )
    }
}

impl std::error::Error for StandaloneShutdownError {}

fn record_shutdown_failure(
    failures: &mut Vec<&'static str>,
    code: &'static str,
    phase: &'static str,
) {
    error!(
        target: "se_manager::web::shutdown",
        stable_code = code,
        shutdown_phase = phase,
        "standalone resource cleanup failed"
    );
    failures.push(code);
}

/// Stop standalone-owned producers, prove both Conversation durability barriers, and only then
/// release the remaining ACP/PTY resources. Every fallible stage is attempted in deterministic
/// order; callers receive stable codes without persistence paths or payload material.
async fn shutdown_standalone_resources_until(
    acp: &AcpManager,
    pty: &PtyManager,
    ws_relay: &WsRelaySink,
    deadline: tokio::time::Instant,
) -> Result<StandaloneShutdownReceipt, StandaloneShutdownError> {
    let mut failures = Vec::new();

    crate::host_admission::HostAdmission::global().close();
    crate::host_admission::HostAdmission::global()
        .drain_until(deadline)
        .await;
    let registry = crate::web::upgraded_connections::UpgradedConnectionRegistry::global();
    registry.stop_admission();
    let _ = registry.revoke_generations();
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let connection_receipt = registry.join_all(remaining).await;
    info!(
        target: "se_manager::web::shutdown",
        stable_code = "OK",
        shutdown_phase = "join_upgraded_connections",
        active = connection_receipt.active,
        failed = connection_receipt.failed,
        timed_out = connection_receipt.timed_out,
        cancelled = connection_receipt.cancelled,
        "upgraded connections joined under host deadline"
    );

    match tokio::time::timeout_at(deadline, acp.stop_producers()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => record_shutdown_failure(
            &mut failures,
            ACP_PRODUCER_STOP_FAILED,
            "stop_acp_producers",
        ),
    }
    if ws_relay
        .shutdown_conversation_persistence_until(deadline)
        .await
        .is_err()
    {
        record_shutdown_failure(
            &mut failures,
            CONVERSATION_PERSISTENCE_DRAIN_FAILED,
            "drain_conversation_persistence",
        );
    }
    match ws_relay.flush_catalog_until(deadline).await {
        Ok(receipt) => info!(
            target: "se_manager::web::shutdown",
            stable_code = "OK",
            shutdown_phase = "flush_conversation_catalog",
            requested_generation = receipt.requested_generation,
            flushed_generation = receipt.flushed_generation,
            write_count = receipt.write_count,
            "standalone catalog barrier completed"
        ),
        Err(_) => record_shutdown_failure(
            &mut failures,
            CONVERSATION_CATALOG_FLUSH_FAILED,
            "flush_conversation_catalog",
        ),
    }
    match tokio::time::timeout_at(deadline, acp.shutdown_persistence()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => record_shutdown_failure(
            &mut failures,
            ACP_PERSISTENCE_SHUTDOWN_FAILED,
            "shutdown_acp_persistence",
        ),
    }

    // The same absolute host deadline bounds every PTY job. All <=30 resources are scheduled
    // concurrently, and failed/in-flight jobs remain owned for later explicit retry.
    let pty_shutdown = pty.kill_all_until(deadline).await;
    info!(
        target: "se_manager::web::shutdown",
        stable_code = if pty_shutdown.clean_success() { "OK" } else { PTY_CLEANUP_FAILED },
        shutdown_phase = "cleanup_ptys",
        attempted = pty_shutdown.attempted,
        succeeded = pty_shutdown.succeeded,
        failed = pty_shutdown.failed,
        in_flight = pty_shutdown.in_flight,
        elapsed_ms = pty_shutdown.elapsed_ms,
        "standalone PTY cleanup aggregate completed"
    );
    if !pty_shutdown.clean_success() {
        record_shutdown_failure(&mut failures, PTY_CLEANUP_FAILED, "cleanup_ptys");
    }

    let receipt = StandaloneShutdownReceipt {
        pty_shutdown,
        connections_active: connection_receipt.active,
        connections_failed: connection_receipt.failed,
        connections_timed_out: connection_receipt.timed_out,
    };
    let result = if failures.is_empty() {
        Ok(receipt)
    } else {
        Err(StandaloneShutdownError {
            codes: failures,
            receipt,
        })
    };
    #[cfg(test)]
    crate::host_admission::HostAdmission::global().reopen_for_tests();
    result
}

/// Bind and serve the standalone ACP HTTP server until SIGINT/SIGTERM.
///
/// `ws_relay` is the live [`WsRelaySink`] — passed to both `AcpManager::new`
/// (as an event sink) and the router (so `/ws` can subscribe clients + replay
/// cursors). On signal: drains Axum first (graceful shutdown), stops ACP event
/// producers, drains canonical Conversation persistence and the final catalog
/// generation under one absolute deadline, then performs remaining ACP/PTY
/// cleanup. Bind failures and stable shutdown failures are returned to the
/// caller.
///
/// `registry` is the in-memory [`ProjectRegistry`] the router reads for
/// `GET /projects` + `switch_project` cwd resolution. The standalone binary
/// seeds it from the file-backed [`crate::acp::project_registry::FileProjectRegistry`]
/// at startup (VPS mode); the desktop host seeds it via `remote_sync_projects`
/// and calls [`serve_router`] directly (it never reaches this `serve`
/// wrapper).
///
/// `workspace_manifest` is the host-owned [`WorkspaceManifestService`] for
/// CAP-5 / Story 5 — atomically persists one versioned workspace manifest per
/// project. The standalone binary opens it under
/// `<service_account_state_dir>/workspace-manifests`; the desktop host opens
/// its own under `<app_data_dir>/workspace-manifests` (never shared across
/// processes — `Never`-clause). `None` degrades to fresh-only mode.
///
/// `acp_catalog` is the host-owned [`AcpCatalogService`] for CAP-6 / Story 8 —
/// resolves the trusted ACP catalog (OS/arch/runtime + per-agent status). The
/// standalone binary opens it under `<service_account_state_dir>/acp-catalog`;
/// the desktop host opens its own under `<app_data_dir>/acp-catalog`. `None`
/// degrades to `ACP_CATALOG_UNAVAILABLE`.
///
/// `acp_install` is the host-owned [`AcpInstallService`] for CAP-6 / Story 9 —
/// downloads + verifies (sha256) + extracts + atomically activates ACP agent
/// archives resolved from the catalog. The standalone binary opens it under
/// `<service_account_state_dir>/acp-registry-binaries`; the desktop host opens
/// its own under `<app_data_dir>/acp-registry-binaries`. `None` degrades to
/// `ACP_INSTALL_UNAVAILABLE`.
///
/// The standalone binary owns its agent lifetime end-to-end, so it kills agents
/// on exit. The desktop-hosted shared-live path calls [`serve_router`] directly
/// and must NOT kill the desktop's live agents — see [`serve_router`].
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    acp: Arc<AcpManager>,
    pty: Arc<PtyManager>,
    terminal_events: TerminalEventHub,
    cwd_tracker: Arc<CwdTracker>,
    git_tracker: Arc<GitTracker>,
    exit_code_tracker: Arc<ExitCodeTracker>,
    ws_relay: Arc<WsRelaySink>,
    registry: Arc<crate::web::project_registry::ProjectRegistry>,
    registry_persistence: Option<Arc<parking_lot::Mutex<crate::acp::FileProjectRegistry>>>,
    projects_file: Option<PathBuf>,
    cfg: ServerConfig,
    conversation: Arc<crate::conversation::ConversationApplicationService>,
    workspace_manifest: Option<Arc<crate::acp::WorkspaceManifestService>>,
    acp_catalog: Option<Arc<crate::acp::AcpCatalogService>>,
    acp_install: Option<Arc<crate::acp::install::AcpInstallService>>,
    authority: Arc<RemoteAccessAuthority>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_addr, handle) = serve_router(
        acp.clone(),
        pty.clone(),
        terminal_events,
        cwd_tracker,
        git_tracker,
        exit_code_tracker,
        Arc::clone(&ws_relay),
        registry,
        registry_persistence,
        projects_file,
        cfg,
        shutdown_signal_future(),
        Some(conversation),
        workspace_manifest,
        acp_catalog,
        acp_install,
        authority,
    )
    .await?;

    let serve_result = handle.await;
    let deadline = tokio::time::Instant::now() + crate::conversation::DEFAULT_DRAIN_TIMEOUT;
    shutdown_standalone_resources_until(&acp, &pty, &ws_relay, deadline).await?;

    match serve_result {
        Ok(()) => {
            info!("se-server stopped");
            Ok(())
        }
        Err(join_err) if join_err.is_cancelled() => {
            warn!("se-server serve task cancelled");
            Ok(())
        }
        Err(join_err) => Err(Box::new(join_err)),
    }
}

/// Bind the Axum router and spawn the serve loop with an external shutdown.
///
/// Binds the listener synchronously (so the caller learns the bound address
/// before serving starts), warns when `dist-web/` is missing, then spawns the
/// `axum::serve` loop on the current runtime. The returned [`JoinHandle`]
/// completes when the server has drained on shutdown or errored; the bound
/// [`SocketAddr`] is returned immediately so the host manager can build the
/// URL without waiting for the server to stop.
///
/// **Does NOT call `kill_all`** — the caller owns the agent-lifetime decision.
/// The standalone binary wraps this + adds `kill_all` in [`serve`]; the
/// desktop-hosted shared-live server (`remote/host.rs`) calls this directly so
/// toggling the server off never kills the desktop's live agents.
#[allow(clippy::too_many_arguments)]
pub async fn serve_router(
    acp: Arc<AcpManager>,
    pty: Arc<PtyManager>,
    terminal_events: TerminalEventHub,
    cwd_tracker: Arc<CwdTracker>,
    git_tracker: Arc<GitTracker>,
    exit_code_tracker: Arc<ExitCodeTracker>,
    ws_relay: Arc<WsRelaySink>,
    registry: Arc<crate::web::project_registry::ProjectRegistry>,
    registry_persistence: Option<Arc<parking_lot::Mutex<crate::acp::FileProjectRegistry>>>,
    projects_file: Option<PathBuf>,
    cfg: ServerConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
    conversation: Option<Arc<crate::conversation::ConversationApplicationService>>,
    workspace_manifest: Option<Arc<crate::acp::WorkspaceManifestService>>,
    acp_catalog: Option<Arc<crate::acp::AcpCatalogService>>,
    acp_install: Option<Arc<crate::acp::install::AcpInstallService>>,
    authority: Arc<RemoteAccessAuthority>,
) -> Result<(SocketAddr, JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>> {
    // The owning host computes ingress provenance before this shared composition is entered.
    // Capture it independently from the listener address: cloudflared connects to a loopback
    // socket, but its requests must remain PublicTunnel and never inherit LocalOperator rights.
    let host_ingress_provenance = authority.ingress_provenance();
    let bind_addr = cfg.bind_addr().ok_or_else(|| {
        format!(
            "invalid host '{}': use 127.0.0.1 (default) or 0.0.0.0 (expose)",
            cfg.host
        )
    })?;

    let listener = TcpListener::bind(bind_addr).await?;
    let addr = listener.local_addr()?;

    // Same-origin browser clients use the concrete listener Origin. Register
    // it before router construction without weakening any explicit public
    // Origin policy (desktop cloudflared adds its HTTPS Origin later).
    if addr.ip().is_loopback() {
        let local_origin = url::Url::parse(&format!("http://{addr}"))?;
        authority
            .set_public_origin(local_origin)
            .map_err(|error| format!("failed to register listener Origin: {error}"))?;
    } else if addr.ip().is_unspecified() {
        let loopback_origin = url::Url::parse(&format!("http://127.0.0.1:{}", addr.port()))?;
        authority
            .set_public_origin(loopback_origin)
            .map_err(|error| format!("failed to register listener Origin: {error}"))?;
    }
    info!(
        ingress_provenance = host_ingress_provenance.as_str(),
        "ACP web server listening on http://{}", addr
    );

    if !assets::dist_web_ready() {
        warn!(
            "dist-web/index.html not found at {:?} — run `bun run build:web` before browsing; \
             /health still works, static routes will 404",
            assets::dist_web_dir()
        );
    }

    // Advertise `Server` history mode when the relay has either canonical Conversation
    // persistence or the pre-cutover SessionPersistence compatibility provider.
    let history_mode = if ws_relay.has_persisted_history() {
        HistoryMode::Server
    } else {
        HistoryMode::LiveOnly
    };
    // Issue #613: resolve the server-side store path — explicit
    // `--store-file` wins, otherwise default under the service-account state
    // dir (same resolution posture as workspace-manifests / acp-catalog). The
    // desktop shared-live host passes `store_file: None`, so it lands on the
    // same default and gets a durable store too.
    let store = Some(Arc::new(WebStore::open(
        cfg.store_file
            .clone()
            .unwrap_or_else(|| cfg.service_account_state_dir().join("store.json")),
    )));
    // Origin registration above must not rewrite the host decision. `router` reads this exact
    // value and injects it before request middleware; ConnectInfo remains transport metadata only.
    debug_assert_eq!(authority.ingress_provenance(), host_ingress_provenance);
    let app = router::router(
        Arc::clone(&acp),
        pty,
        terminal_events,
        cwd_tracker,
        git_tracker,
        exit_code_tracker,
        Arc::clone(&ws_relay),
        Arc::clone(&registry),
        registry_persistence,
        projects_file,
        cfg.project_root.clone(),
        history_mode,
        conversation,
        workspace_manifest,
        acp_catalog,
        acp_install,
        store,
        authority,
    );

    let handle = tokio::spawn(async move {
        // Patch D: `into_make_service_with_connect_info::<SocketAddr>()` so
        // the fs WRITE routes can extract `ConnectInfo<SocketAddr>` for the
        // localhost-only guard. Read routes and `/ws` are unaffected.
        let serve_result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
        .inspect_err(|e| error!("ACP web server error: {}", e));

        match serve_result {
            Ok(()) => info!("ACP web server stopped"),
            Err(e) => error!("ACP web server stopped with error: {}", e),
        }
    });

    Ok((addr, handle))
}

/// Build the shutdown-signal future for the standalone binary path.
///
/// Waits for Ctrl-C (SIGINT) or, on Unix, SIGTERM. On signal-handler setup
/// failure, parks forever rather than completing (which would stop the server
/// immediately). The desktop-hosted path uses an `oneshot`-driven shutdown
/// instead.
async fn shutdown_signal_future() {
    match shutdown_signal().await {
        Ok(()) => info!("se-server shutting down…"),
        Err(e) => {
            warn!("shutdown signal setup failed ({e}); serving until process exit");
            // Do not complete the shutdown future — that would stop the
            // server immediately. Park until the process is killed.
            std::future::pending::<()>().await;
        }
    }
}

/// Wait for Ctrl-C (SIGINT) or, on Unix, SIGTERM.
///
/// Returns `Err` if signal handlers cannot be installed (no `expect`/`unwrap`).
async fn shutdown_signal() -> Result<(), std::io::Error> {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = ctrl_c => result?,
            _ = sigterm.recv() => {},
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        // Windows: Ctrl-C / console ctrl handler via tokio. SIGTERM is not a
        // portable Win32 signal; service-stop is out of scope for this scaffold.
        ctrl_c.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{
        AgentSessionBinding, AgentSessionBindingState, ConversationCreator, ConversationEventType,
        ConversationId, ConversationLifecycleState, ConversationMutation,
        ConversationPersistenceAdapter, ConversationRecordV2, ConversationRepository,
        ConversationWriter, CreationPartition, ExecutionTarget, LegacyConversationReader,
        ReaderPrecedence, AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    async fn conversation_relay_fixture(
        session_id: &str,
    ) -> (
        TempDir,
        Arc<ConversationRepository>,
        Arc<WsRelaySink>,
        ConversationId,
    ) {
        let temp = tempfile::tempdir().expect("temporary Conversation root");
        let root = temp
            .path()
            .canonicalize()
            .expect("canonical temporary root");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace directory");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        let (repository, report) =
            ConversationRepository::open(root.join("private")).expect("repository open");
        assert_eq!(report.valid_conversation_count, 0);
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id = ConversationId::new_v4();
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
            .expect("canonical Conversation create");
        writer
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: session_id.to_string(),
                    runtime_agent_id: "runtime-web-shutdown".to_string(),
                    stable_agent_namespace: "config:web-shutdown".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .expect("canonical agent-session binding");
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(writer, reader));
        let relay = Arc::new(WsRelaySink::with_conversation_persistence(
            32, adapter, None,
        ));
        (temp, repository, relay, conversation_id)
    }

    fn server_config(root: &std::path::Path) -> ServerConfig {
        let project_root = root.canonicalize().expect("canonical project root");
        let conversation_workspace_root = project_root.join("Termul");
        std::fs::create_dir_all(&conversation_workspace_root).expect("Conversation workspace root");
        ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            event_log_capacity: 32,
            permission_timeout_secs: 60,
            permission_reconnect_grace_secs: 15,
            project_root,
            projects_file: None,
            sessions_dir: None,
            conversation_workspace_root,
            workspace_manifests_dir: None,
            acp_catalog_dir: None,
            store_file: None,
            remote_access_token_file: None,
            allowed_origins: Vec::new(),
        }
    }

    fn install_test_agent(acp: &AcpManager, agent_id: &crate::acp::AgentId) {
        let (observed, _receiver) = std::sync::mpsc::sync_channel(1);
        acp.install_test_agent_for_new_session(agent_id.clone(), observed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn standalone_shutdown_drains_conversation_persistence() {
        let (_temp, repository, relay, conversation_id) =
            conversation_relay_fixture("standalone-drain-session").await;
        repository.reset_catalog_write_counters();
        relay
            .emit(&sink::AcpEvent {
                sid: Some("standalone-drain-session".to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal": 1}),
            })
            .expect("durable event admission");

        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![relay_sink]));
        let agent_id = crate::acp::AgentId("standalone-owned-agent".to_string());
        install_test_agent(&acp, &agent_id);
        let pty = test_pty_manager();
        let ordered = relay
            .ordered_conversation_persistence()
            .expect("ordered Conversation persistence");
        assert_eq!(
            ordered.active_worker_count(),
            crate::conversation::WRITER_SHARDS
        );

        shutdown_standalone_resources_until(
            &acp,
            &pty,
            &relay,
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("standalone shutdown succeeds");

        assert!(
            acp.stable_agent_namespace(&agent_id).is_err(),
            "producer stop removes standalone-owned agents"
        );
        assert_eq!(ordered.active_worker_count(), 0);
        assert_eq!(ordered.metrics().pending_records, 0);
        assert!(
            repository
                .read_events(conversation_id, 0)
                .expect("durable Conversation history")
                .iter()
                .any(|event| event.payload["ordinal"] == 1),
            "accepted relay event crosses the canonical durability frontier"
        );
        let catalog = repository.catalog_flush_coordinator();
        let snapshot = catalog.snapshot();
        assert!(catalog.flushed_generation() >= snapshot.generation);
        assert!(repository.catalog_write_count() >= 1);

        let source = include_str!("mod.rs");
        let start = source
            .find("async fn shutdown_standalone_resources_until(")
            .expect("standalone shutdown helper");
        let end = source[start..]
            .find("/// Bind and serve the standalone")
            .map(|offset| start + offset)
            .expect("standalone shutdown helper boundary");
        let body = &source[start..end];
        let ordered_calls = [
            "timeout_at(deadline, acp.stop_producers())",
            ".shutdown_conversation_persistence_until(deadline)",
            ".flush_catalog_until(deadline)",
            "timeout_at(deadline, acp.shutdown_persistence())",
            "pty.kill_all_until(deadline).await",
        ];
        let positions = ordered_calls.map(|needle| {
            body.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        });
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "standalone shutdown stages must retain producer/drain/catalog/cleanup order"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn standalone_catalog_flush_failed_blocks_clean_exit_under_host_deadline_and_later_mutation_responsive(
    ) {
        let (_temp, repository, relay, conversation_id) =
            conversation_relay_fixture("standalone-catalog-failure").await;
        repository.reset_catalog_write_counters();
        repository.fail_next_catalog_writes(usize::MAX);
        relay
            .emit(&sink::AcpEvent {
                sid: Some("standalone-catalog-failure".to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal": 1}),
            })
            .expect("durable event admission");
        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![relay_sink]));
        let pty = test_pty_manager();
        let pending_generation = repository.catalog_pending_generation();

        let error = shutdown_standalone_resources_until(
            &acp,
            &pty,
            &relay,
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect_err("catalog barrier failure must block clean success");
        assert!(error.codes.contains(&CONVERSATION_CATALOG_FLUSH_FAILED));
        assert!(!error.codes.contains(&CONVERSATION_PERSISTENCE_DRAIN_FAILED));
        assert_eq!(error.receipt.pty_shutdown.attempted, 0);
        assert_eq!(
            repository.catalog_pending_generation(),
            pending_generation,
            "failed final generation remains retryable"
        );
        assert_eq!(
            relay
                .ordered_conversation_persistence()
                .expect("ordered Conversation persistence")
                .active_worker_count(),
            0,
            "later cleanup still runs after a stable catalog failure"
        );

        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let mutation_started = std::time::Instant::now();
        writer
            .append_event(
                conversation_id,
                Utc::now(),
                ConversationEventType::MessageChunk,
                json!({"ordinal": 2}),
                ConversationMutation::AcpEventAppend,
            )
            .await
            .expect("later canonical mutation remains responsive");
        assert!(mutation_started.elapsed() < Duration::from_secs(1));
        assert!(repository.catalog_pending_generation() > pending_generation);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_listener_preserves_host_public_tunnel_provenance() {
        const TOKEN: &str = "public-tunnel-provenance-token";
        let temp = tempfile::tempdir().unwrap();
        let relay = Arc::new(WsRelaySink::new());
        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![relay_sink]));
        let pty = test_pty_manager();
        let authority = Arc::new(RemoteAccessAuthority::for_tests(TOKEN));
        authority.set_ingress_provenance(crate::web::auth::IngressProvenance::PublicTunnel);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let (addr, handle) = serve_router(
            acp,
            Arc::clone(&pty),
            pty.terminal_events(),
            pty.cwd_tracker(),
            pty.git_tracker(),
            pty.exit_code_tracker(),
            relay,
            Arc::new(ProjectRegistry::new()),
            None,
            None,
            server_config(temp.path()),
            async move {
                let _ = shutdown_rx.await;
            },
            None,
            None,
            None,
            None,
            Arc::clone(&authority),
        )
        .await
        .expect("public-tunnel router starts on loopback");

        let target = temp.path().join("must-not-be-created");
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/fs/mkdir"))
            .bearer_auth(TOKEN)
            .json(&json!({"path": target}))
            .send()
            .await
            .expect("loopback HTTP request completes");
        let body: serde_json::Value = response.json().await.expect("IPC body decodes");
        assert_eq!(body["success"], false);
        assert_eq!(body["code"], "FORBIDDEN");
        assert!(!target.exists());
        assert_eq!(
            authority.ingress_provenance(),
            crate::web::auth::IngressProvenance::PublicTunnel
        );

        shutdown_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("router stops")
            .expect("serve task joins");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_live_router_remains_non_owning() {
        let (temp, repository, relay, conversation_id) =
            conversation_relay_fixture("shared-live-session").await;
        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![relay_sink]));
        let agent_id = crate::acp::AgentId("desktop-live-agent".to_string());
        install_test_agent(&acp, &agent_id);
        let pty = test_pty_manager();
        let authority = Arc::new(RemoteAccessAuthority::for_tests("shared-live-test-token"));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let (_addr, handle) = serve_router(
            Arc::clone(&acp),
            Arc::clone(&pty),
            pty.terminal_events(),
            pty.cwd_tracker(),
            pty.git_tracker(),
            pty.exit_code_tracker(),
            Arc::clone(&relay),
            Arc::new(ProjectRegistry::new()),
            None,
            None,
            server_config(temp.path()),
            async move {
                let _ = shutdown_rx.await;
            },
            None,
            None,
            None,
            None,
            authority,
        )
        .await
        .expect("shared-live router starts");
        shutdown_tx.send(()).expect("signal shared-live shutdown");
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("shared-live router stops")
            .expect("shared-live serve task joins");

        assert_eq!(
            acp.stable_agent_namespace(&agent_id)
                .expect("shared-live agent remains registered"),
            Some("config:test".to_string())
        );
        let ordered = relay
            .ordered_conversation_persistence()
            .expect("ordered Conversation persistence");
        assert_eq!(
            ordered.active_worker_count(),
            crate::conversation::WRITER_SHARDS
        );
        relay
            .emit(&sink::AcpEvent {
                sid: Some("shared-live-session".to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal": 2}),
            })
            .expect("shared-live relay remains admitting events");

        let source = include_str!("mod.rs");
        let router_start = source
            .find("pub async fn serve_router(")
            .expect("serve_router exists");
        let router_end = source[router_start..]
            .find("/// Build the shutdown-signal future")
            .map(|offset| router_start + offset)
            .expect("serve_router body boundary exists");
        let router_body = &source[router_start..router_end];
        for forbidden in [
            "stop_producers",
            "shutdown_conversation_persistence",
            "flush_catalog_until",
            "shutdown_persistence",
            "kill_all",
            "pty.kill_all",
        ] {
            assert!(
                !router_body.contains(forbidden),
                "shared-live serve_router must not own {forbidden}"
            );
        }

        shutdown_standalone_resources_until(
            &acp,
            &pty,
            &relay,
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("test-owned cleanup succeeds");
        assert!(repository
            .read_events(conversation_id, 0)
            .expect("durable shared-live history")
            .iter()
            .any(|event| event.payload["ordinal"] == 2));
    }

    #[tokio::test]
    async fn shutdown_joins_upgraded_connections_under_host_deadline() {
        let registry = crate::web::upgraded_connections::UpgradedConnectionRegistry::global();
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
        let _ = registry.register(
            crate::web::upgraded_connections::UpgradedConnectionKind::Acp,
            Some(handle),
        );
        let pty = test_pty_manager();
        let acp = Arc::new(AcpManager::new(vec![]));
        let relay = Arc::new(WsRelaySink::new());
        let result = shutdown_standalone_resources_until(
            &acp,
            &pty,
            &relay,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => error.receipt,
        };
        assert_eq!(receipt.connections_active, 0);
    }

    #[tokio::test]
    async fn stop_producers_and_legacy_persistence_honor_absolute_deadline() {
        let source = include_str!("mod.rs");
        let start = source
            .find("async fn shutdown_standalone_resources_until(")
            .expect("shutdown helper exists");
        let body = &source[start..];
        assert!(body.contains("timeout_at(deadline, acp.stop_producers())"));
        assert!(body.contains("timeout_at(deadline, acp.shutdown_persistence())"));
        assert!(body.contains("join_all(remaining)"));
        let pty = test_pty_manager();
        let acp = Arc::new(AcpManager::new(vec![]));
        let relay = Arc::new(WsRelaySink::new());
        let started = tokio::time::Instant::now();
        let _ = shutdown_standalone_resources_until(
            &acp,
            &pty,
            &relay,
            started + Duration::from_millis(50),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
