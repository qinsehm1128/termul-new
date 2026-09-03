//! Desktop-hosted shared-live ACP web server — in-process lifecycle.
//!
//! Replaces the legacy PTY bridge (`remote/server.rs`, removed). Where the old
//! server proxied live PTY I/O over a separate WebSocket, this wraps the same
//! [`crate::web`] Axum server the standalone `se-server` binary uses, so the
//! desktop's live `AcpManager` (the same agent sessions the renderer sees) is
//! shared with a browser/phone client over the LAN — the "shared-live" mode.
//!
//! ## Lifecycle
//!
//! `RemoteServerState` is a `Mutex<Option<RemoteServer>>` managed by Tauri. The
//! status-bar control drives `remote_server_start` / `_stop` (see `commands.rs`),
//! which delegate to [`RemoteServerState::start`] / [`stop`].
//!
//! ## The kill-all hazard
//!
//! The standalone `web::serve` calls `AcpManager::kill_all` after Axum drains —
//! correct for a binary that owns its agents, but catastrophic for the desktop,
//! where stopping the shared-live server must NOT kill the desktop's live agents.
//! This path therefore calls [`crate::web::serve_router`] directly (which never
//! kills agents) and drives shutdown through an `oneshot` channel.
//!
//! ## Bind model
//!
//! `Localhost` (`127.0.0.1`) is used when the phone reaches the desktop only
//! through a tunnel. `All` (`0.0.0.0`) is used when LAN publish is selected so
//! a same-Wi-Fi phone can open `http://{lan-ip}:{port}/#access_token=…`.

use std::net::SocketAddr;
use std::sync::Arc;

use serde::Serialize;
use tokio::process::Child;
use tokio::sync::oneshot;

use crate::acp::{AcpCatalogService, AcpInstallService, AcpManager, WorkspaceManifestService};
use crate::pty::PtyManager;
use crate::web::auth::{DesktopCredentialLease, IngressProvenance};
use crate::web::config::MAX_EVENT_LOG_CAPACITY;
use crate::web::sink::WsRelaySink;
use crate::web::{serve_router, ProjectRegistry, RemoteAccessAuthority, ServerConfig};

/// Which network interface(s) the in-process web server binds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteBindMode {
    /// `127.0.0.1` — localhost only (default, safest).
    Localhost,
    /// `0.0.0.0` — all interfaces (LAN / other devices on the network).
    All,
}

impl RemoteBindMode {
    /// Parse a host string into a bind mode.
    ///
    /// Accepts `localhost` / `127.0.0.1` / `loopback` → [`Localhost`], and
    /// `all` / `0.0.0.0` / `any` → [`All`]. Anything else returns `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "localhost" | "127.0.0.1" | "loopback" => Some(Self::Localhost),
            "all" | "0.0.0.0" | "any" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Localhost => "localhost",
            Self::All => "all",
        }
    }

    /// Bind host string for [`ServerConfig`] (`127.0.0.1` or `0.0.0.0`).
    pub fn host(self) -> &'static str {
        match self {
            Self::Localhost => "127.0.0.1",
            Self::All => "0.0.0.0",
        }
    }

    /// Human-readable bind target for the UI.
    pub fn display_host(self) -> &'static str {
        self.host()
    }

    /// `true` when bound to all interfaces (LAN-exposed).
    pub fn is_lan_exposed(self) -> bool {
        matches!(self, Self::All)
    }
}

/// Status of the desktop-hosted web server, returned to the frontend.
///
/// Field shape is preserved verbatim from the legacy PTY server so the renderer's
/// `RemoteStatus` type and status-bar UI stay unchanged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub running: bool,
    pub url: Option<String>,
    pub port: Option<u16>,
    /// `localhost` or `all` when running; `None` when stopped.
    pub bind_mode: Option<String>,
    /// Bind host shown in the UI (`127.0.0.1` or `0.0.0.0`).
    pub bind_host: Option<String>,
    /// Public tunnel Origin when a provider is up; never contains the bearer fragment.
    pub tunnel_url: Option<String>,
    /// Active provider id (`cloudflareQuick` / `cloudflareNamed` / `frp`).
    pub tunnel_provider: Option<String>,
    /// Credentialed scan/copy URL for the active publish mode.
    pub access_url: Option<String>,
    /// Same-Wi-Fi Origin without the bearer fragment.
    pub lan_url: Option<String>,
    /// Credentialed LAN URL (`#access_token=`). Absent when bound loopback-only.
    pub lan_access_url: Option<String>,
    /// Credentialed tunnel URL. Absent when no public Origin is attached.
    pub tunnel_access_url: Option<String>,
    /// `lan` or `tunnel` — which URL `access_url` currently represents.
    pub publish_mode: Option<String>,
}

impl RemoteStatus {
    fn stopped() -> Self {
        Self {
            running: false,
            url: None,
            port: None,
            bind_mode: None,
            bind_host: None,
            tunnel_url: None,
            tunnel_provider: None,
            access_url: None,
            lan_url: None,
            lan_access_url: None,
            tunnel_access_url: None,
            publish_mode: None,
        }
    }

    fn running(
        addr: SocketAddr,
        bind_mode: RemoteBindMode,
        tunnel_url: Option<String>,
        credential_lease: Option<&DesktopCredentialLease>,
    ) -> Self {
        let url = if addr.ip().is_unspecified() {
            None
        } else {
            Some(format!("http://{}:{}", addr.ip(), addr.port()))
        };
        let tunnel_access_url = tunnel_url.as_deref().and_then(|origin| {
            crate::remote::lan::credentialed_access_url(origin, credential_lease?.bearer())
        });
        Self {
            running: true,
            url,
            port: Some(addr.port()),
            bind_mode: Some(bind_mode.as_str().to_string()),
            bind_host: Some(bind_mode.display_host().to_string()),
            tunnel_url,
            tunnel_provider: None,
            access_url: tunnel_access_url.clone(),
            lan_url: None,
            lan_access_url: None,
            tunnel_access_url,
            publish_mode: None,
        }
    }

    fn with_provider(mut self, provider: Option<String>) -> Self {
        self.tunnel_provider = provider;
        self
    }

    fn with_lan(
        mut self,
        lan_url: Option<String>,
        credential_lease: Option<&DesktopCredentialLease>,
    ) -> Self {
        self.lan_access_url = lan_url.as_deref().and_then(|origin| {
            crate::remote::lan::credentialed_access_url(origin, credential_lease?.bearer())
        });
        self.lan_url = lan_url;
        if self.url.is_none() {
            self.url = self.lan_url.clone();
        }
        self
    }

    fn with_publish_mode(mut self, mode: crate::remote::PublishMode) -> Self {
        self.publish_mode = Some(mode.as_str().to_string());
        self.access_url = match mode {
            crate::remote::PublishMode::Lan => self.lan_access_url.clone(),
            crate::remote::PublishMode::Tunnel => self.tunnel_access_url.clone(),
        };
        self
    }
}

/// Running server handle — owns the shutdown signal, the serve task handle, and
/// the bound address.
struct RemoteServer {
    shutdown_tx: Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>,
    /// The spawned `axum::serve` task. Stored (not dropped) so `stop()` can
    /// await its graceful drain and `status()` can detect a dead task.
    serve_handle: Option<tokio::task::JoinHandle<()>>,
    addr: SocketAddr,
    bind_mode: RemoteBindMode,
    /// Public tunnel Origin (set when a provider attaches).
    tunnel_url: Option<String>,
    /// Provider id attached with the tunnel (`cloudflareQuick` / …).
    tunnel_provider: Option<String>,
    /// `true` once the cloudflared watchdog observed the child exit. Read by
    /// `status()` (sync) to drop the stale `tunnel_url` so the renderer poller
    /// clears the QR (it would otherwise offer a link that yields "This site
    /// can't be reached"). `None` when no tunnel is attached.
    tunnel_dead: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The watchdog task owning the cloudflared child: it `wait()`s for exit
    /// then flips `tunnel_dead`. Aborted on `stop()`/`Drop` so the owned child
    /// is reaped via `kill_on_drop` — the child is NOT held here directly,
    /// which keeps `status()` sync (no try_wait-across-`.await` dance).
    tunnel_watchdog: Option<tokio::task::JoinHandle<()>>,
    /// Exactly one raw desktop credential generation, owned only while this
    /// server is active. The authority stores generation metadata + digest.
    credential_lease: Option<DesktopCredentialLease>,
    authority: Arc<RemoteAccessAuthority>,
    publish_mode: crate::remote::PublishMode,
}

impl RemoteServer {
    /// `true` if the spawned serve task has exited (Ok or Err/panic). Used by
    /// `status()` to stop reporting `running` after the listener died.
    fn task_finished(&self) -> bool {
        self.serve_handle
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    fn retire_credential(
        &mut self,
        lifecycle_phase: &'static str,
    ) -> crate::web::auth::GenerationRetirementReceipt {
        let Some(lease) = self.credential_lease.take() else {
            return crate::web::auth::GenerationRetirementReceipt {
                generation: 0,
                credential_invalidated: false,
                origins_cleared: false,
                failure_state_cleared: false,
                keyring_deleted: true,
                retry_owner: false,
                stable_codes: Vec::new(),
            };
        };
        let generation = lease.generation();
        let receipt = self.authority.retire_generation(generation);
        if receipt.is_clean() {
            log::info!(
                target: "se_manager::remote::host",
                "operation=generation_retire generation={} lifecycle_phase={} stable_code=OK keyring_deleted={}",
                generation,
                lifecycle_phase,
                receipt.keyring_deleted
            );
        } else {
            if receipt.retry_owner {
                self.credential_lease = Some(lease);
            }
            log::error!(
                target: "se_manager::remote::host",
                "operation=generation_retire generation={} lifecycle_phase={} stable_code=REMOTE_CREDENTIAL_CLEANUP_FAILED keyring_deleted={}",
                generation,
                lifecycle_phase,
                receipt.keyring_deleted
            );
        }
        receipt
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.retire_credential("drop");
        // Best-effort graceful shutdown on drop (e.g. app exit). Signal the
        // serve task to drain; the JoinHandle is left to the runtime to reap
        // (awaiting it in `Drop` isn't possible — `Drop` is sync).
        if let Some(tx) = self.shutdown_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        // Abort the cloudflared watchdog so the child it owns is reaped via
        // `kill_on_drop` (the child lives inside the watchdog task; aborting
        // drops its future → drops the child). Without this, dropping the
        // JoinHandle would detach the task and the cloudflared process could
        // outlive the server on a hard exit where `stop()` never ran.
        if let Some(handle) = self.tunnel_watchdog.take() {
            handle.abort();
        }
    }
}

struct PendingCredentialLease {
    authority: Arc<RemoteAccessAuthority>,
    lease: Option<DesktopCredentialLease>,
    retire_on_drop: bool,
}

impl PendingCredentialLease {
    fn new(
        authority: Arc<RemoteAccessAuthority>,
        lease: DesktopCredentialLease,
        retire_on_drop: bool,
    ) -> Self {
        Self {
            authority,
            lease: Some(lease),
            retire_on_drop,
        }
    }

    fn into_lease(mut self) -> DesktopCredentialLease {
        self.lease.take().expect("pending credential lease")
    }
}

impl Drop for PendingCredentialLease {
    fn drop(&mut self) {
        if !self.retire_on_drop {
            return;
        }
        if let Some(lease) = self.lease.take() {
            let _ = self.authority.retire_generation(lease.generation());
        }
    }
}

/// Tauri-managed wrapper tracking the in-process web server's start/stop state.
pub struct RemoteServerState {
    inner: std::sync::Mutex<Option<RemoteServer>>,
    lifecycle: tokio::sync::Mutex<()>,
    authority: Arc<RemoteAccessAuthority>,
    pairing: Option<Arc<crate::remote::TunnelConfigStore>>,
}

impl RemoteServerState {
    pub fn with_desktop_authority(authority: Arc<RemoteAccessAuthority>) -> Self {
        Self {
            inner: std::sync::Mutex::new(None),
            lifecycle: tokio::sync::Mutex::new(()),
            authority,
            pairing: None,
        }
    }

    #[must_use]
    pub fn with_pairing_store(mut self, store: Arc<crate::remote::TunnelConfigStore>) -> Self {
        self.pairing = Some(store);
        self
    }

    pub fn new() -> Self {
        #[cfg(test)]
        {
            Self {
                inner: std::sync::Mutex::new(None),
                lifecycle: tokio::sync::Mutex::new(()),
                authority: Arc::new(RemoteAccessAuthority::for_tests("test-remote-access-token")),
                pairing: None,
            }
        }
        #[cfg(not(test))]
        Self {
            inner: std::sync::Mutex::new(None),
            lifecycle: tokio::sync::Mutex::new(()),
            authority: Arc::new(RemoteAccessAuthority::unconfigured()),
            pairing: None,
        }
    }

    fn load_pairing_token(&self) -> Option<String> {
        self.pairing
            .as_ref()
            .and_then(|store| store.pairing_token().ok().flatten())
    }

    fn persist_pairing_token(&self, token: &str) -> Result<(), String> {
        let Some(store) = self.pairing.as_ref() else {
            return Ok(());
        };
        store.set_pairing_token(Some(token))
    }

    fn clear_pairing_token(&self) -> Result<(), String> {
        let Some(store) = self.pairing.as_ref() else {
            return Ok(());
        };
        store.set_pairing_token(None)
    }

    /// Start the in-process web server sharing the desktop's live `AcpManager`.
    ///
    /// Builds a [`ServerConfig`] from the bind mode (OS-assigned port via
    /// `port: 0`), binds + spawns the serve loop via [`serve_router`] (which
    /// never kills agents), and stores the shutdown handle + serve task handle.
    /// Returns `Err` if a server is already running or the bind fails.
    ///
    /// The serve task's `JoinHandle` is stored so `stop()` can await its
    /// graceful drain and `status()` can detect a dead task. If a concurrent
    /// start wins the slot race, the loser signals its spawned task to drain
    /// before returning `Err` (no orphaned second server).
    ///
    /// `workspace_manifest` is the desktop's own `WorkspaceManifestService`
    /// (opened under `<app_data_dir>/workspace-manifests` in `lib.rs`).
    /// Threaded through to `serve_router` so the web/remote client can
    /// read/write a project's manifest through `/workspace/*`. `None`
    /// degrades to fresh-only mode.
    ///
    /// `acp_catalog` is the desktop's own `AcpCatalogService` (opened under
    /// `<app_data_dir>/acp-catalog` in `lib.rs`). Threaded through to
    /// `serve_router` so the web/remote client can resolve the catalog through
    /// `GET /acp/catalog` + WS `list_acp_catalog`. `None` degrades to
    /// `ACP_CATALOG_UNAVAILABLE`.
    ///
    /// `acp_install` is the desktop's own `AcpInstallService` (opened under
    /// `<app_data_dir>/acp-registry-binaries` in `lib.rs`). Threaded through
    /// to `serve_router` so the web/remote client can install through
    /// `POST /acp/install` + WS `install_acp_agent`. `None` degrades to
    /// `ACP_INSTALL_UNAVAILABLE`.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        acp: Arc<AcpManager>,
        pty: Arc<PtyManager>,
        ws_relay: Arc<WsRelaySink>,
        registry: Arc<ProjectRegistry>,
        bind_mode: RemoteBindMode,
        conversation: Option<Arc<crate::conversation::ConversationApplicationService>>,
        workspace_manifest: Option<Arc<WorkspaceManifestService>>,
        acp_catalog: Option<Arc<AcpCatalogService>>,
        acp_install: Option<Arc<AcpInstallService>>,
    ) -> Result<RemoteStatus, String> {
        self.start_on_port(
            acp,
            pty,
            ws_relay,
            registry,
            bind_mode,
            conversation,
            workspace_manifest,
            acp_catalog,
            acp_install,
            0,
        )
        .await
    }

    /// Start the shared-live server on an explicit loopback port.
    ///
    /// `bind_port == 0` keeps the OS-assigned ephemeral port used by Quick Tunnel
    /// and FRP. Named Cloudflare tunnels pass a stable port so remotely-managed
    /// ingress can target `http://127.0.0.1:{port}`.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_on_port(
        &self,
        acp: Arc<AcpManager>,
        pty: Arc<PtyManager>,
        ws_relay: Arc<WsRelaySink>,
        registry: Arc<ProjectRegistry>,
        _bind_mode: RemoteBindMode,
        conversation: Option<Arc<crate::conversation::ConversationApplicationService>>,
        workspace_manifest: Option<Arc<WorkspaceManifestService>>,
        acp_catalog: Option<Arc<AcpCatalogService>>,
        acp_install: Option<Arc<AcpInstallService>>,
        bind_port: u16,
    ) -> Result<RemoteStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        let bind_mode = _bind_mode;
        {
            let slot = self.inner.lock().unwrap();
            if slot.is_some() {
                return Err("Remote server is already running".to_string());
            }
        }
        let event_log_capacity = ws_relay.event_log_capacity();
        if !(1..=MAX_EVENT_LOG_CAPACITY).contains(&event_log_capacity) {
            return Err(format!(
                "remote event-log capacity must be in 1..={MAX_EVENT_LOG_CAPACITY}"
            ));
        }
        self.authority
            .set_ingress_provenance(IngressProvenance::PublicTunnel);
        let stored = self.load_pairing_token();
        let (lease, issued) = self
            .authority
            .adopt_or_issue_desktop_credential(stored.as_deref())
            .map_err(|error| format!("failed to adopt remote credential: {error}"))?;
        if issued {
            if let Err(error) = self.persist_pairing_token(lease.bearer()) {
                let _ = self.authority.retire_generation(lease.generation());
                return Err(error);
            }
        }
        let pending_credential =
            PendingCredentialLease::new(Arc::clone(&self.authority), lease, issued);

        // CAP-1 / Story 1: resolve the project-root boundary for the
        // shared-live server from the **active project** (the
        // `ProjectRegistry`'s default-project path), NOT the user home dir.
        // On a cross-drive setup (profile on C:, project on E:) the home-dir
        // fallback rejects every `/skills`, `/git/*`, and `/search/content`
        // probe for the real project with `OUTSIDE_PROJECT_ROOT`. The registry
        // carries display paths (not canonical forms), so we run the result
        // through `resolve_and_validate_project_root` so the value stored in
        // `ServerConfig::project_root` is a canonical absolute path to an
        // existing directory — exactly like the standalone `se-server`.
        //
        // Fallback chain:
        // 1. Registry default project path → canonicalize.
        // 2. `default_project_root()` ($TERMUL_PROJECT_ROOT / $HOME) → canonicalize.
        // 3. None discoverable → fatal (the server cannot enforce a boundary).
        //
        // A transient canonicalization failure on the registry default (deleted
        // between sync and start) falls through to the home fallback rather
        // than refusing to start — the operator can still re-sync the registry
        // and rebind live. A stable warning is emitted without the path/error.
        let project_root = {
            let from_registry = registry.default_project_path().and_then(|p| {
                match crate::web::config::resolve_and_validate_project_root(std::path::Path::new(
                    &p,
                )) {
                    Ok(canonical) => Some(canonical),
                    Err(_) => {
                        log::warn!(target: "se_manager::remote::host", "operation=project_root_resolve stable_code=PROJECT_ROOT_INVALID");
                        None
                    }
                }
            });
            if let Some(root) = from_registry {
                root
            } else {
                // 2. Empty registry / bad default → home fallback + warn.
                let raw_root = crate::web::config::default_project_root().ok_or_else(|| {
                    "could not determine project root for shared-live server: \
                     set $TERMUL_PROJECT_ROOT or ensure $HOME is available"
                        .to_string()
                })?;
                log::warn!(target: "se_manager::remote::host", "operation=shared_live_host stable_code=REJECTED");
                crate::web::config::resolve_and_validate_project_root(&raw_root).map_err(|e| {
                    format!(
                        "shared-live server refused to start: {e} \
                         (set $TERMUL_PROJECT_ROOT to a valid directory)"
                    )
                })?
            }
        };

        let cfg = ServerConfig {
            host: bind_mode.host().to_string(),
            // `0` = OS-assigned ephemeral port. Named Cloudflare tunnels pass a
            // stable port so remotely-managed ingress can target loopback.
            port: bind_port,
            event_log_capacity: ws_relay.event_log_capacity(),
            permission_timeout_secs: ws_relay
                .rendezvous()
                .map(|r| r.timeout().as_secs())
                .unwrap_or(60),
            permission_reconnect_grace_secs: ws_relay
                .rendezvous()
                .map(|r| r.disconnect_grace().as_secs())
                .unwrap_or(15),
            conversation_workspace_root: project_root
                .join(crate::brand::canonical().display_name),
            project_root,
            // Desktop-hosted shared-live mode queries the live desktop
            // `AcpManager` via the in-memory renderer-fed registry, NOT a
            // server-owned file (AC2 / architecture Gap #3). The
            // file-backed `acp::project_registry` is VPS-mode-only.
            projects_file: None,
            sessions_dir: None,
            // CAP-5 / Story 5: this path-override field is standalone-only.
            // The desktop shared-live host passes its already-opened
            // `WorkspaceManifestService` directly to `serve_router` (see the
            // `workspace_manifest` argument below), so no path is resolved
            // from the config here — `None` degrades nothing on this path.
            workspace_manifests_dir: None,
            acp_catalog_dir: None,
            // Issue #613: `None` → resolve `<service_account_state_dir>/store.json`
            // at serve time (the shared-live host gets a durable store too).
            store_file: None,
            remote_access_token_file: None,
            allowed_origins: Vec::new(),
        };

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let shutdown_tx = Arc::new(std::sync::Mutex::new(Some(shutdown_tx)));
        let shutdown = async move {
            let _ = shutdown_rx.await;
            log::info!(target: "se_manager::remote::host", "operation=shared_live_host stable_code=OK");
        };

        let (addr, serve_handle) = serve_router(
            acp,
            Arc::clone(&pty),
            pty.terminal_events(),
            pty.cwd_tracker(),
            pty.git_tracker(),
            pty.exit_code_tracker(),
            ws_relay,
            registry,
            None,
            None,
            cfg,
            shutdown,
            conversation,
            workspace_manifest,
            acp_catalog,
            acp_install,
            Arc::clone(&self.authority),
        )
        .await
        .map_err(|e| format!("Failed to start remote server: {}", e))?;

        let status = RemoteStatus::running(addr, bind_mode, None, None);
        log::info!(target: "se_manager::remote::host", "operation=shared_live_host stable_code=OK");

        let mut slot = self.inner.lock().unwrap();
        if slot.is_some() {
            // Lost a concurrent-start race. Signal this spawned task to drain
            // (do NOT drop `shutdown_tx` silently — that would orphan a second
            // server that `remote_server_stop` could never reach).
            if let Some(tx) = shutdown_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            // Let the serve task run down before discarding its handle.
            drop(serve_handle);
            return Err("Remote server is already running".to_string());
        }
        *slot = Some(RemoteServer {
            shutdown_tx,
            serve_handle: Some(serve_handle),
            addr,
            bind_mode,
            // Tunnel URL + watchdog are attached after
            // `cloudflared::start_quick_tunnel` resolves in
            // `remote_server_start` — keeps this method testable without a real
            // cloudflared binary (the lifecycle tests bind/stop/status here).
            tunnel_url: None,
            tunnel_provider: None,
            tunnel_dead: None,
            tunnel_watchdog: None,
            credential_lease: Some(pending_credential.into_lease()),
            authority: Arc::clone(&self.authority),
            publish_mode: crate::remote::PublishMode::Tunnel,
        });
        if bind_mode == RemoteBindMode::All {
            register_lan_origin(&self.authority, addr.port());
        }
        Ok(status)
    }

    /// Stop the in-process web server if running.
    ///
    /// Signals graceful shutdown to the serve task and awaits its drain so the
    /// standalone `serve()` "drain Axum → then kill" ordering holds on the
    /// desktop path too. Does NOT call `AcpManager::kill_all` — the desktop's
    /// live agents survive a shared-live toggle-off.
    pub async fn stop(&self) -> Result<RemoteStatus, String> {
        self.stop_inner(true).await
    }

    /// Drain the listener and tunnel without retiring the pairing generation.
    /// Used on Desktop exit so a wanted session can reuse the settings bearer.
    pub async fn shutdown_keep_credential(&self) -> Result<RemoteStatus, String> {
        self.stop_inner(false).await
    }

    async fn stop_inner(&self, retire: bool) -> Result<RemoteStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        let receipt = {
            let mut slot = self.inner.lock().unwrap();
            match slot.as_mut() {
                Some(server) if retire => server.retire_credential("stop"),
                Some(server) => {
                    let _ = server.credential_lease.take();
                    crate::web::auth::GenerationRetirementReceipt {
                        generation: 0,
                        credential_invalidated: false,
                        origins_cleared: false,
                        failure_state_cleared: false,
                        keyring_deleted: true,
                        retry_owner: false,
                        stable_codes: Vec::new(),
                    }
                }
                None => return Err("Remote server is not running".to_string()),
            }
        };
        if !receipt.is_clean() {
            log::error!(
                target: "se_manager::remote::host",
                "operation=shared_live_host lifecycle_phase=stop stable_code=REMOTE_CREDENTIAL_CLEANUP_FAILED generation={} keyring_deleted={} retry_owner={}",
                receipt.generation,
                receipt.keyring_deleted,
                receipt.retry_owner
            );
            return Err("REMOTE_CREDENTIAL_CLEANUP_FAILED".to_string());
        }
        let server = {
            let mut slot = self.inner.lock().unwrap();
            slot.take()
        };
        match server {
            Some(mut server) => {
                // Abort the cloudflared watchdog so the owned child is reaped
                // via `kill_on_drop` (the child lives inside the watchdog task;
                // aborting drops its future → drops the child). Done before
                // Axum drains so the public tunnel stops forwarding new traffic
                // before existing conns flush. An already-exited child (the
                // watchdog already completed) is a no-op.
                if let Some(handle) = server.tunnel_watchdog.take() {
                    handle.abort();
                    // Await completion of the abort so the child is reaped
                    // before we proceed (returns Err(Cancelled) — ignored).
                    let _ = handle.await;
                }
                // Signal drain, then await the serve task so Axum finishes
                // flushing before the caller proceeds (e.g. app exit →
                // `kill_all`). `Drop` would only signal; awaiting here enforces
                // ordering.
                if let Some(tx) = server.shutdown_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                if let Some(handle) = server.serve_handle.take() {
                    // `axum::serve` returns on graceful-shutdown completion; a
                    // panic surfaces as `JoinError` — log, don't propagate.
                    if let Err(join_err) = handle.await {
                        if !join_err.is_cancelled() {
                            log::warn!(target: "se_manager::remote::host", "operation=shared_live_host stable_code=REJECTED");
                        }
                    }
                }
                if retire {
                    if let Err(error) = self.clear_pairing_token() {
                        log::error!(
                            target: "se_manager::remote::host",
                            "operation=pairing_clear lifecycle_phase=stop stable_code=REMOTE_CREDENTIAL_CLEANUP_FAILED"
                        );
                        return Err(error);
                    }
                }
                Ok(RemoteStatus::stopped())
            }
            None => Err("Remote server is not running".to_string()),
        }
    }

    /// Current status of the in-process web server.
    ///
    /// Also detects a dead cloudflared child: if the watchdog flag reports the
    /// child has exited, the public trycloudflare URL no longer routes, so the
    /// stale `tunnel_url` is cleared — the renderer poller then drops the QR
    /// (it would otherwise offer a link that yields "This site can't be
    /// reached"). Stays sync (reads an `AtomicBool`) so callers need no
    /// `.await`.
    pub fn status(&self) -> RemoteStatus {
        let mut slot = self.inner.lock().unwrap();
        if slot.is_none() {
            return RemoteStatus::stopped();
        }
        // If the spawned serve task has exited (error/panic), invalidate its
        // generation before clearing the published server slot.
        if slot.as_ref().is_some_and(RemoteServer::task_finished) {
            let mut finished = slot.take().expect("finished server slot");
            finished.retire_credential("serve_finished");
            return RemoteStatus::stopped();
        }
        let server = slot.as_mut().expect("running server slot");
        // Clear the tunnel URL once the cloudflared watchdog reports the child
        // dead (the public URL no longer routes). Logged once: the flag stays
        // set but `tunnel_url` is cleared here so subsequent polls skip the arm.
        if server
            .tunnel_dead
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
            && server.tunnel_url.is_some()
        {
            log::warn!(
                target: "se_manager::remote::host",
                "operation=tunnel_watchdog lifecycle_phase=tunnel_death stable_code=TUNNEL_EXITED"
            );
            server.tunnel_url = None;
            server.tunnel_provider = None;
        }
        compose_running_status(server)
    }

    pub fn set_publish_mode(&self, mode: crate::remote::PublishMode) {
        if let Some(server) = self.inner.lock().unwrap().as_mut() {
            server.publish_mode = mode;
        }
    }

    /// Replace the active pairing bearer. Origins are re-registered because
    /// rotation clears the allowlist.
    pub fn rotate_active_credential(&self) -> Result<RemoteStatus, String> {
        {
            let mut slot = self.inner.lock().unwrap();
            let server = slot
                .as_mut()
                .ok_or_else(|| "Remote server is not running".to_string())?;
            let lease = self
                .authority
                .rotate_desktop_credential()
                .map_err(|error| format!("failed to rotate remote credential: {error}"))?;
            self.persist_pairing_token(lease.bearer())
                .map_err(|error| format!("failed to persist rotated credential: {error}"))?;
            server.credential_lease = Some(lease);
            reregister_published_origins(server);
        }
        Ok(self.status())
    }

    fn fail_active_server(&self, lifecycle_phase: &'static str) {
        let failed = {
            let mut slot = self.inner.lock().unwrap();
            if let Some(server) = slot.as_mut() {
                server.retire_credential(lifecycle_phase);
            }
            slot.take()
        };
        drop(failed);
    }

    /// Attach a started cloudflared quick-tunnel (URL + live child) to the
    /// running server so [`status`](Self::status) reports the tunnel URL and
    /// [`stop`](Self::stop) kills the child. Called by `remote_server_start`
    /// after the server binds and `cloudflared::start_quick_tunnel` resolves.
    ///
    /// If the server stopped/died between `start` and this call, the orphaned
    /// child is killed (sync `start_kill`) so no cloudflared lingers. Keeping
    /// this out of `start` lets the server-lifecycle unit tests run without a
    /// real cloudflared binary.
    #[cfg(test)]
    pub fn attach_tunnel(&self, url: String, child: Child) -> Result<(), String> {
        self.attach_tunnel_as(url, child, "cloudflareQuick")
    }

    /// Attach a started tunnel (URL + live child + provider id) to the running server.
    pub fn attach_tunnel_as(
        &self,
        url: String,
        child: Child,
        provider: &str,
    ) -> Result<(), String> {
        let mut child = Some(child);
        let parsed_origin = match url::Url::parse(&url) {
            Ok(origin) => origin,
            Err(_) => {
                if let Some(child) = child.as_mut() {
                    let _ = child.start_kill();
                }
                self.fail_active_server("attach_invalid_origin");
                return Err("cloudflared returned an invalid public Origin".to_string());
            }
        };
        {
            let slot = self.inner.lock().unwrap();
            if slot.as_ref().is_none_or(RemoteServer::task_finished) {
                if let Some(child) = child.as_mut() {
                    let _ = child.start_kill();
                }
                drop(slot);
                self.fail_active_server("attach_missing_server");
                return Err("remote server stopped before tunnel attached".to_string());
            }
        }
        if let Err(error) = self.authority.set_public_origin(parsed_origin.clone()) {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
            self.fail_active_server("attach_origin_policy");
            return Err(format!("failed to register public Origin: {error}"));
        }
        let mut public_origin = parsed_origin;
        public_origin.set_path("");
        public_origin.set_query(None);
        public_origin.set_fragment(None);
        let public_origin = public_origin.to_string().trim_end_matches('/').to_string();

        let mut slot = self.inner.lock().unwrap();
        match slot.as_mut() {
            Some(server)
                if !server.task_finished() && server.credential_lease.as_ref().is_some() =>
            {
                let child = child.take().expect("child present after take");
                let dead_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = dead_flag.clone();
                // The watchdog owns the child: `wait()` for natural exit, then
                // flag death so the next `status()` poll clears the stale URL.
                // A dead tunnel is an availability failure, not a leak — keep
                // the pairing generation so LAN (and a later tunnel retry) still
                // work. Aborted on `stop()`/`Drop`.
                let watchdog = tokio::spawn(async move {
                    let mut child = child;
                    let _ = child.wait().await;
                    flag.store(true, std::sync::atomic::Ordering::Release);
                });
                server.tunnel_url = Some(public_origin);
                server.tunnel_provider = Some(provider.to_string());
                server.tunnel_dead = Some(dead_flag);
                server.tunnel_watchdog = Some(watchdog);
                log::info!(target: "se_manager::remote::host", "operation=shared_live_host stable_code=OK");
                Ok(())
            }
            _ => {
                if let Some(child) = child.as_mut() {
                    let _ = child.start_kill();
                }
                drop(slot);
                self.fail_active_server("attach_race");
                Err("remote server stopped before tunnel attached".to_string())
            }
        }
    }
}

fn compose_running_status(server: &RemoteServer) -> RemoteStatus {
    let lan_url = if server.bind_mode == RemoteBindMode::All {
        crate::remote::lan::discover_lan_ipv4()
            .map(|ip| crate::remote::lan::lan_http_origin(ip, server.addr.port()))
    } else {
        None
    };
    RemoteStatus::running(
        server.addr,
        server.bind_mode,
        server.tunnel_url.clone(),
        server.credential_lease.as_ref(),
    )
    .with_provider(server.tunnel_provider.clone())
    .with_lan(lan_url, server.credential_lease.as_ref())
    .with_publish_mode(server.publish_mode)
}

fn register_lan_origin(authority: &RemoteAccessAuthority, port: u16) {
    if let Some(ip) = crate::remote::lan::discover_lan_ipv4() {
        if let Ok(origin) = url::Url::parse(&crate::remote::lan::lan_http_origin(ip, port)) {
            if let Err(error) = authority.set_public_origin(origin) {
                log::warn!(
                    target: "se_manager::remote::host",
                    "operation=lan_origin_register stable_code=REJECTED error_kind={}",
                    error.code()
                );
            }
        }
    }
}

fn reregister_published_origins(server: &RemoteServer) {
    let port = server.addr.port();
    if let Ok(origin) = url::Url::parse(&format!("http://127.0.0.1:{port}")) {
        let _ = server.authority.set_public_origin(origin);
    }
    if server.bind_mode == RemoteBindMode::All {
        register_lan_origin(&server.authority, port);
    }
    if let Some(tunnel) = server.tunnel_url.as_deref() {
        if let Ok(origin) = url::Url::parse(tunnel) {
            let _ = server.authority.set_public_origin(origin);
        }
    }
}

impl Default for RemoteServerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Percent-encode a filesystem path for use as a query-string value in a
    /// test URL (Windows backslashes, spaces, etc. would otherwise break the
    /// URL parse). Mirrors the `urlencoding` helper in `git_api::tests`.
    fn percent_encode_path(path: &std::path::Path) -> String {
        let mut out = String::with_capacity(path.as_os_str().len());
        for c in path.to_string_lossy().chars() {
            match c {
                ' ' => out.push_str("%20"),
                '\\' => out.push_str("%5C"),
                _ if c.is_ascii_alphanumeric()
                    || matches!(c, '-' | '_' | '.' | '~' | '/' | ':') =>
                {
                    out.push(c)
                }
                _ => {
                    for byte in c.to_string().as_bytes() {
                        out.push_str(&format!("%{:02X}", byte));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn remote_bind_mode_parse() {
        assert_eq!(
            RemoteBindMode::parse("localhost"),
            Some(RemoteBindMode::Localhost)
        );
        assert_eq!(
            RemoteBindMode::parse("127.0.0.1"),
            Some(RemoteBindMode::Localhost)
        );
        assert_eq!(RemoteBindMode::parse("all"), Some(RemoteBindMode::All));
        assert_eq!(RemoteBindMode::parse("0.0.0.0"), Some(RemoteBindMode::All));
        assert_eq!(RemoteBindMode::parse("any"), Some(RemoteBindMode::All));
        assert_eq!(RemoteBindMode::parse("bogus"), None);
    }

    #[test]
    fn remote_bind_mode_host_and_display() {
        assert_eq!(RemoteBindMode::Localhost.host(), "127.0.0.1");
        assert_eq!(RemoteBindMode::All.host(), "0.0.0.0");
        assert_eq!(RemoteBindMode::Localhost.display_host(), "127.0.0.1");
        assert_eq!(RemoteBindMode::All.display_host(), "0.0.0.0");
    }

    #[test]
    fn remote_bind_mode_is_lan_exposed() {
        assert!(!RemoteBindMode::Localhost.is_lan_exposed());
        assert!(RemoteBindMode::All.is_lan_exposed());
    }

    #[test]
    fn remote_status_stopped_is_all_none() {
        let s = RemoteStatus::stopped();
        assert!(!s.running);
        assert_eq!(s.url, None);
        assert_eq!(s.port, None);
        assert_eq!(s.bind_mode, None);
        assert_eq!(s.bind_host, None);
        assert_eq!(s.tunnel_url, None);
        assert_eq!(s.access_url, None);
        assert_eq!(s.lan_url, None);
        assert_eq!(s.lan_access_url, None);
        assert_eq!(s.tunnel_access_url, None);
        assert_eq!(s.publish_mode, None);
    }

    #[test]
    fn remote_status_running_localhost_uses_loopback_url() {
        let addr: SocketAddr = "127.0.0.1:5123".parse().unwrap();
        let s = RemoteStatus::running(addr, RemoteBindMode::Localhost, None, None);
        assert!(s.running);
        assert_eq!(s.url.as_deref(), Some("http://127.0.0.1:5123"));
        assert_eq!(s.port, Some(5123));
        assert_eq!(s.bind_mode.as_deref(), Some("localhost"));
        assert_eq!(s.bind_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(s.tunnel_url, None);
        assert_eq!(s.access_url, None);
    }

    #[test]
    fn remote_status_running_carries_tunnel_url() {
        let addr: SocketAddr = "127.0.0.1:5123".parse().unwrap();
        let s = RemoteStatus::running(
            addr,
            RemoteBindMode::Localhost,
            Some("https://foo-bar.trycloudflare.com".to_string()),
            None,
        );
        assert_eq!(
            s.tunnel_url.as_deref(),
            Some("https://foo-bar.trycloudflare.com")
        );
        assert_eq!(s.access_url, None);
    }

    #[test]
    fn remote_status_materializes_access_url_only_from_active_lease() {
        let addr: SocketAddr = "127.0.0.1:5123".parse().unwrap();
        let authority = RemoteAccessAuthority::for_tests("bootstrap-token");
        let lease = authority.rotate_desktop_credential().unwrap();
        let expected_fragment = format!("#access_token={}", lease.bearer());
        let s = RemoteStatus::running(
            addr,
            RemoteBindMode::Localhost,
            Some("https://foo-bar.trycloudflare.com".to_string()),
            Some(&lease),
        );
        assert_eq!(
            s.tunnel_url.as_deref(),
            Some("https://foo-bar.trycloudflare.com")
        );
        assert!(s
            .access_url
            .as_deref()
            .is_some_and(|url| url.ends_with(&expected_fragment)));
    }

    #[test]
    fn remote_status_running_all_has_no_url() {
        // Bound to 0.0.0.0: the host's LAN IP can't be derived from the bind
        // address, so `url` is `None` (the UI shows "use this machine's LAN
        // IP:{port}"). Don't fabricate a loopback URL the phone can't reach.
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        let s = RemoteStatus::running(addr, RemoteBindMode::All, None, None);
        assert!(s.running);
        assert_eq!(s.url, None, "0.0.0.0 must not fabricate a loopback URL");
        assert_eq!(s.port, Some(8080));
        assert_eq!(s.bind_mode.as_deref(), Some("all"));
        assert_eq!(s.bind_host.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn pending_serve_failure_guard_invalidates_unpublished_generation() {
        let authority = Arc::new(RemoteAccessAuthority::for_tests("bootstrap-token"));
        let lease = authority.rotate_desktop_credential().unwrap();
        let bearer = lease.bearer().to_string();
        let pending = PendingCredentialLease::new(Arc::clone(&authority), lease, true);
        drop(pending);
        assert_eq!(
            authority.verify_bearer(&bearer).unwrap_err(),
            crate::web::auth::RemoteAuthError::InvalidCredential
        );
    }

    #[tokio::test]
    async fn remote_server_state_stop_on_unstarted_errors() {
        // A stop on an unstarted state must error; status reports stopped.
        let state = RemoteServerState::new();
        assert!(!state.status().running);

        let err = state.stop().await;
        assert!(err.is_err(), "stop on an unstarted server must error");
        assert!(!state.status().running);
    }

    #[tokio::test]
    async fn start_reuses_settings_pairing_token_and_stop_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::remote::TunnelConfigStore::for_path(
            dir.path().join("config.json"),
        ));
        store
            .set_pairing_token(Some("test-remote-access-token"))
            .unwrap();
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let authority = Arc::new(crate::web::auth::RemoteAccessAuthority::desktop_memory());
        let state =
            RemoteServerState::with_desktop_authority(authority).with_pairing_store(store.clone());
        state
            .start(
                acp,
                pty,
                relay,
                registry,
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("settings-backed host starts");
        assert!(state.status().running);
        let (_, bearer) = active_credential(&state);
        assert_eq!(bearer, "test-remote-access-token");
        state.stop().await.expect("stop clears pairing token");
        assert_eq!(store.pairing_token().unwrap(), None);
    }

    /// A real `AcpManager` (zero sinks is legal) + a `WsRelaySink` for the
    /// shared-live host lifecycle tests. The serve task binds a real OS-assigned
    /// localhost socket — safe in tests.
    fn lifecycle_fixtures() -> (
        Arc<AcpManager>,
        Arc<PtyManager>,
        Arc<WsRelaySink>,
        Arc<ProjectRegistry>,
    ) {
        let acp = Arc::new(AcpManager::new(vec![]));
        let pty = crate::web::test_pty_manager();
        let relay = Arc::new(WsRelaySink::new());
        let registry = Arc::new(ProjectRegistry::new());
        (acp, pty, relay, registry)
    }

    fn active_credential(state: &RemoteServerState) -> (u64, String) {
        let slot = state.inner.lock().unwrap();
        let lease = slot
            .as_ref()
            .and_then(|server| server.credential_lease.as_ref())
            .expect("active credential lease");
        (lease.generation(), lease.bearer().to_string())
    }

    #[tokio::test]
    async fn remote_server_state_start_then_stop_lifecycle() {
        // The full start→status(running)→stop→status(stopped)→restart cycle
        // that T8.1 asked for and the old misnamed test never exercised.
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        assert!(!state.status().running);

        let status = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start on localhost binds an OS-assigned port");
        assert!(status.running, "start returns a running status");
        assert!(status.port.is_some(), "an OS-assigned port is reported");
        assert!(
            state.status().running,
            "status reflects running after start"
        );
        assert_eq!(state.status().port, status.port);
        let (first_generation, first_bearer) = active_credential(&state);
        assert!(state.authority.verify_bearer(&first_bearer).is_ok());

        // stop drains the serve task (the JoinHandle is awaited), invalidates
        // the active generation, and reports stopped.
        let stopped = state
            .stop()
            .await
            .expect("stop on a running server succeeds");
        assert!(!stopped.running);
        assert!(
            !state.status().running,
            "status reflects stopped after stop"
        );
        assert_eq!(
            state.authority.verify_bearer(&first_bearer).unwrap_err(),
            crate::web::auth::RemoteAuthError::InvalidCredential
        );

        // Restart works (the slot was cleared by stop) with a distinct bearer.
        let again = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("restart after stop succeeds");
        assert!(again.running);
        let (second_generation, second_bearer) = active_credential(&state);
        assert!(second_generation > first_generation);
        assert_ne!(second_bearer, first_bearer);
        assert_eq!(
            state.authority.verify_bearer(&first_bearer).unwrap_err(),
            crate::web::auth::RemoteAuthError::InvalidCredential
        );
        assert!(state.authority.verify_bearer(&second_bearer).is_ok());
        let _ = state.stop().await;
    }

    #[tokio::test]
    async fn remote_server_state_double_start_is_rejected() {
        // The lose-race guard: a second start while the first is running returns
        // Err — and (per R1) does NOT orphan a second server (its shutdown_tx is
        // signaled before returning). The first server keeps running.
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        let _first = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("first start succeeds");

        let second = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            second.is_err(),
            "a second start while running must be rejected"
        );
        assert!(state.status().running, "the first server is still running");

        let _ = state.stop().await;
    }

    #[tokio::test]
    async fn remote_server_state_rejects_oversized_event_log_capacity_before_publication() {
        let (acp, pty, _relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        let oversized = Arc::new(WsRelaySink::with_log_capacity(MAX_EVENT_LOG_CAPACITY + 1));
        let error = state
            .start(
                acp.clone(),
                pty.clone(),
                oversized,
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(error.contains("1..=16384"));
        assert!(!state.status().running);

        let maximum = Arc::new(WsRelaySink::with_log_capacity(MAX_EVENT_LOG_CAPACITY));
        let status = state
            .start(
                acp,
                pty,
                maximum,
                registry,
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("maximum bounded capacity is admitted");
        assert!(status.running);
        state.stop().await.unwrap();
    }

    #[tokio::test]
    async fn failed_tunnel_attach_invalidates_generation_before_returning() {
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        state
            .start(
                acp,
                pty,
                relay,
                registry,
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let (_, bearer) = active_credential(&state);
        let mut command = quick_exit_command();
        command.kill_on_drop(true);
        let child = command.spawn().unwrap();
        assert!(state
            .attach_tunnel("not a public origin".into(), child)
            .is_err());
        assert!(!state.status().running);
        assert_eq!(
            state.authority.verify_bearer(&bearer).unwrap_err(),
            crate::web::auth::RemoteAuthError::InvalidCredential
        );
    }

    #[tokio::test]
    async fn remote_server_state_start_stop_does_not_kill_agents() {
        // The central AC4 guarantee: toggling the shared-live server off must NOT
        // kill the desktop's live agents. `serve_router` (which host::start
        // calls) never calls `AcpManager::kill_all`; `stop` only signals the
        // oneshot. Drive the full lifecycle and assert no agent state was
        // disturbed. (AcpManager::new(vec![]) owns no agents, so there is
        // nothing to kill — this guards the path: start/stop complete without
        // touching kill_all, i.e. no panic, no error, clean drain.)
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        let _ = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start succeeds");
        // The serve task holds `Arc::clone(&acp)`; stop drains it. The desktop
        // `acp` is untouched (still usable, agents survive).
        let stopped = state.stop().await.expect("stop succeeds");
        assert!(!stopped.running);
        // `acp` is still intact — the host never called kill_all on it. (No
        // direct kill_all assertion possible without a spy; the invariant is
        // structural: serve_router does not call kill_all, host::stop does not
        // call kill_all. This test guards the path end-to-end.)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_clears_tunnel_url_but_keeps_generation_when_child_exits() {
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        let _ = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start");

        let (_, bearer) = active_credential(&state);

        let mut cmd = quick_exit_command();
        cmd.kill_on_drop(true);
        let child = cmd.spawn().expect("spawn quick-exit child");
        state
            .attach_tunnel("https://stale.trycloudflare.com".to_string(), child)
            .expect("attach");

        let mut cleared = false;
        for _ in 0..20 {
            let status = state.status();
            if status.running && status.tunnel_url.is_none() {
                cleared = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(cleared, "tunnel death must drop the public Origin");
        assert!(state.status().running, "listener stays up for LAN / retry");
        assert!(state.authority.verify_bearer(&bearer).is_ok());
        let _ = state.stop().await;
    }

    #[tokio::test]
    async fn lan_bind_honors_all_interfaces() {
        let (acp, pty, relay, registry) = lifecycle_fixtures();
        let state = RemoteServerState::new();
        let status = state
            .start(
                acp,
                pty,
                relay,
                registry,
                RemoteBindMode::All,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("LAN bind starts");
        assert!(status.running);
        assert_eq!(status.bind_mode.as_deref(), Some("all"));
        assert_eq!(status.bind_host.as_deref(), Some("0.0.0.0"));
        state.set_publish_mode(crate::remote::PublishMode::Lan);
        let published = state.status();
        assert_eq!(published.publish_mode.as_deref(), Some("lan"));
        if let Some(access) = published.access_url.as_deref() {
            assert!(access.contains("#access_token="));
        }
        let _ = state.stop().await;
    }

    /// A cross-platform command that exits 0 almost immediately, for the
    /// dead-child staleness test.
    fn quick_exit_command() -> tokio::process::Command {
        #[cfg(target_os = "windows")]
        let mut c = tokio::process::Command::new("cmd");
        #[cfg(target_os = "windows")]
        c.args(["/c", "exit", "0"]);
        #[cfg(not(target_os = "windows"))]
        let mut c = tokio::process::Command::new("true");

        c.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        c
    }

    #[test]
    fn serve_router_does_not_reference_kill_all() {
        // Structural regression guard for the story's "single most important
        // fact" (Dev Notes invariant #2 / AC4): the desktop-hosted shared-live
        // path calls `serve_router` directly, so `serve_router` must never call
        // `AcpManager::kill_all` — toggling the server off must not kill the
        // desktop's live agents. (The standalone `serve()` wrapper IS allowed to
        // call `kill_all` — it owns its agents — so only `serve_router`'s body
        // is scanned, not the whole module.) If `kill_all` is re-added to
        // `serve_router`, this test fails. The end-to-end lifecycle test above
        // guards the path at runtime; this one pins the source invariant.
        let web_mod = include_str!("../web/mod.rs");
        let body = extract_fn_body(web_mod, "serve_router")
            .expect("serve_router must be defined in web/mod.rs");
        let stripped = strip_line_comments(&body);
        assert!(
            !stripped.contains("kill_all"),
            "serve_router must not reference `kill_all` (the shared-live path must \
             not kill the desktop's agents) — re-adding it would regress AC4"
        );
    }

    /// Extract a top-level `fn`/`async fn` body by name, from its signature
    /// line up to (but not including) the next top-level `fn`/`async fn`.
    fn extract_fn_body(src: &str, fn_name: &str) -> Option<String> {
        let needle = format!("fn {fn_name}");
        let start = src.find(&needle)?;
        // Find the next top-level `fn ` after the signature (closes the body).
        let rest = &src[start + needle.len()..];
        let end = rest
            .find("\nfn ")
            .or_else(|| rest.find("\nasync fn "))
            .unwrap_or(rest.len());
        Some(src[start..start + needle.len() + end].to_string())
    }

    /// Strip `//` line comments so doc references to a token don't trip the
    /// check. Crude but sufficient — these functions hold no string literals
    /// containing `kill_all`.
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .map(|line| match line.find("//") {
                Some(idx) => &line[..idx],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn remote_server_state_default_equals_new() {
        let _ = RemoteServerState::default();
    }

    /// CAP-1 / CAP-8: the keystone cross-drive integration test.
    ///
    /// Seeds a `ProjectRegistry` with a default project whose path is provably
    /// OUTSIDE the user home tree, starts `RemoteServerState`, and asserts
    /// `GET /skills` + `POST /git/status` pass the containment check (not
    /// `OUTSIDE_PROJECT_ROOT`) over the real shared-live HTTP socket. Then
    /// switches the default to a second project and asserts the new project is
    /// accepted WITHOUT a server restart — proving the live rebind threads
    /// through.
    ///
    /// The bug this guards against is the `project_root = %USERPROFILE%` (home)
    /// binding that rejects every project outside the home tree. To reproduce
    /// that rejection, the project dirs MUST NOT be under the home dir. On
    /// Windows `std::env::temp_dir()` resolves to `%USERPROFILE%\AppData\Local\
    /// Temp` — i.e. INSIDE the home tree — so using it would let the buggy
    /// `project_root = home` code accept the project (false pass). We instead
    /// derive the project dirs from an outside-home base: `$TERMUL_TEST_OUTSIDE_
    /// HOME_BASE` when set (so CI can pin a known-writable path outside home),
    /// falling back to `home.parent()` (a sibling of home) when unset. If the
    /// base cannot be resolved or (without an override) is not writable, the
    /// test skips rather than false-pass; an explicit override that is unusable
    /// panics so a broken CI setup is loud, not silent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_live_binds_project_root_to_active_cross_drive_project() {
        let (acp, pty, relay, registry) = lifecycle_fixtures();

        // Resolve the user home dir the way the buggy `default_project_root()`
        // does, then pick a base provably OUTSIDE the home tree so the project
        // dirs reproduce the cross-drive / outside-home rejection the bug caused.
        let Some(home) = crate::web::config::default_project_root() else {
            eprintln!("skip: cannot resolve user home dir for cross-drive test");
            return;
        };
        // `$TERMUL_TEST_OUTSIDE_HOME_BASE` lets CI pin a known-writable path
        // outside home (e.g. `/var/tmp` or a separate drive on runners where
        // `home.parent()` is locked down). When unset, fall back to
        // `home.parent()`. An explicit override that is unusable is a hard
        // error — the operator asked for it, so a silent skip would mask a
        // broken setup.
        let override_set = std::env::var_os("TERMUL_TEST_OUTSIDE_HOME_BASE").is_some();
        let outside_base = match std::env::var("TERMUL_TEST_OUTSIDE_HOME_BASE") {
            Ok(raw) if !raw.trim().is_empty() => std::path::PathBuf::from(raw.trim()),
            _ => match home.parent() {
                Some(p) => p.to_path_buf(),
                None => {
                    eprintln!(
                        "skip: home dir has no parent and \
                         TERMUL_TEST_OUTSIDE_HOME_BASE is unset"
                    );
                    return;
                }
            },
        };
        // Probe the base is writable. An explicit override that is not writable
        // panics (CI must not silently skip a test the operator forced on);
        // without an override, skip silently — local dev may lack a writable
        // outside-home path.
        let probe = outside_base.join(format!(
            "se-manager-xdrive-probe-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        if std::fs::create_dir_all(&probe).is_err() {
            if override_set {
                panic!(
                    "TERMUL_TEST_OUTSIDE_HOME_BASE='{}' is not writable; CI cannot \
                     run the cross-drive test reliably — fix the override path",
                    outside_base.display()
                );
            }
            eprintln!(
                "skip: cannot write outside-home base '{}'; cross-drive layout \
                 not reproducible on this filesystem (set \
                 TERMUL_TEST_OUTSIDE_HOME_BASE to force)",
                outside_base.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&probe);

        // RAII cleanup guards — remove the dirs even if an assertion panics
        // mid-test (avoids leaking random-named dirs on the dev/CI machine).
        struct TempDirGuard(std::path::PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let dir_a = outside_base.join(format!(
            "se-manager-cross-drive-a-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let dir_b = outside_base.join(format!(
            "se-manager-cross-drive-b-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir_a).expect("create dir_a outside home");
        std::fs::create_dir_all(&dir_b).expect("create dir_b outside home");
        let _guard_a = TempDirGuard(dir_a.clone());
        let _guard_b = TempDirGuard(dir_b.clone());

        // Sanity: the project dirs are provably outside the home tree. If this
        // ever fails (unexpected home layout), the test would false-pass — skip.
        let home_canonical = home.canonicalize().unwrap_or_else(|_| home.clone());
        let dir_a_canonical = dir_a.canonicalize().unwrap_or_else(|_| dir_a.clone());
        if dir_a_canonical.starts_with(&home_canonical) {
            eprintln!(
                "skip: project dir '{}' is inside home '{}'; cross-drive layout \
                 not reproducible",
                dir_a_canonical.display(),
                home_canonical.display()
            );
            return;
        }

        // Optionally init a git repo in dir_a so /git/status can return a real
        // status (not just pass the containment check). When git is unavailable
        // the route still exercises the containment boundary and returns
        // GIT_STATUS_ERROR — never OUTSIDE_PROJECT_ROOT.
        let git_available = crate::trackers::GitTracker::run_git_command(
            std::env::temp_dir().to_str().unwrap(),
            &["--version"],
        )
        .is_some();
        if git_available {
            for args in [
                ["init", "-q"].as_slice(),
                ["config", "user.email", "t@example.com"].as_slice(),
                ["config", "user.name", "Test"].as_slice(),
                ["config", "commit.gpgsign", "false"].as_slice(),
            ] {
                let out =
                    crate::trackers::GitTracker::run_git_command(dir_a.to_str().unwrap(), args)
                        .expect("git command runs");
                assert!(
                    out.status.success(),
                    "git {:?} failed: {}",
                    args,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            std::fs::write(dir_a.join("README.md"), "hello\n").expect("write file");
        }

        // Seed the registry with both projects; dir_a is the default (the
        // "active" project the host should bind to).
        let project_a = crate::web::ProjectSummary {
            id: "p-a".to_string(),
            name: "Project A".to_string(),
            color: "blue".to_string(),
            path: Some(dir_a.to_string_lossy().into_owned()),
            is_archived: false,
            is_default: true,
        };
        let project_b = crate::web::ProjectSummary {
            id: "p-b".to_string(),
            name: "Project B".to_string(),
            color: "green".to_string(),
            path: Some(dir_b.to_string_lossy().into_owned()),
            is_archived: false,
            is_default: false,
        };
        registry.set(vec![project_a, project_b], Some("p-a".to_string()));

        // Start the shared-live server. CAP-1: `start` now derives project_root
        // from the registry default (dir_a), NOT the user home dir.
        let state = RemoteServerState::new();
        let status = state
            .start(
                acp.clone(),
                pty.clone(),
                relay.clone(),
                registry.clone(),
                RemoteBindMode::Localhost,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start with a cross-drive project must succeed");
        assert!(status.running, "server should be running");
        let url = status
            .url
            .expect("localhost bind produces a loopback URL")
            .clone();
        let (_, bearer) = active_credential(&state);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build reqwest client");

        // GET /skills?projectRoot=dir_a — must succeed (not OUTSIDE_PROJECT_ROOT).
        // The route canonicalizes dir_a and checks it against project_root
        // (which is now dir_a's canonical form, not the home dir). Build the
        // URL with percent-encoding so Windows backslash paths parse correctly.
        let skills_url_a = format!("{url}/skills?projectRoot={}", percent_encode_path(&dir_a));
        let resp = client
            .get(&skills_url_a)
            .bearer_auth(&bearer)
            .send()
            .await
            .expect("GET /skills");
        let body: serde_json::Value = resp.json().await.expect("parse /skills body");
        // The containment claim is "not OUTSIDE_PROJECT_ROOT" — do NOT also
        // assert success==true, since /skills success depends on the global
        // skills scan (~/.agents/skills) which may be unreadable/empty in CI
        // for reasons unrelated to the boundary fix. The cross-drive fix is
        // proven by the absence of the OUTSIDE_PROJECT_ROOT rejection.
        assert!(
            body.get("code").and_then(|v| v.as_str()) != Some("OUTSIDE_PROJECT_ROOT"),
            "/skills must not reject the active project (cross-drive fix), got: {body}"
        );

        // POST /git/status { cwd: dir_a } — must not be OUTSIDE_PROJECT_ROOT.
        // When git is available + repo initialized, returns success with a
        // status list; otherwise GIT_STATUS_ERROR (the containment check still
        // passed).
        let resp = client
            .post(format!("{url}/git/status"))
            .bearer_auth(&bearer)
            .json(&serde_json::json!({ "cwd": dir_a.to_string_lossy() }))
            .send()
            .await
            .expect("POST /git/status");
        let body: serde_json::Value = resp.json().await.expect("parse /git/status body");
        assert!(
            body.get("code").and_then(|v| v.as_str()) != Some("OUTSIDE_PROJECT_ROOT"),
            "/git/status must not reject the active project (cross-drive fix), got: {body}"
        );
        if git_available {
            assert!(
                body.get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                "/git/status should succeed for a git repo, got: {body}"
            );
        }

        // ---- Switch the default to project B (dir_b) WITHOUT a restart ----
        // CAP-1: the registry's set_default_project triggers rebind_project_root,
        // which recomputes project_root from the new default (dir_b) and writes
        // the canonical path to the AppState.project_root handle in place.
        assert!(
            registry.set_default_project("p-b"),
            "set_default_project must succeed for a switchable project"
        );

        // GET /skills?projectRoot=dir_b — must succeed with the new boundary.
        let skills_url_b = format!("{url}/skills?projectRoot={}", percent_encode_path(&dir_b));
        let resp = client
            .get(&skills_url_b)
            .bearer_auth(&bearer)
            .send()
            .await
            .expect("GET /skills after switch");
        let body: serde_json::Value = resp.json().await.expect("parse /skills body");
        // Containment claim only (see above) — do not couple to skills-scan success.
        assert!(
            body.get("code").and_then(|v| v.as_str()) != Some("OUTSIDE_PROJECT_ROOT"),
            "/skills must not reject the new active project after switch, got: {body}"
        );

        // The old project (dir_a) is now OUTSIDE the new project_root (dir_b),
        // so /skills?projectRoot=dir_a should be rejected. This proves the
        // rebound boundary actually moved (not just widened to cover both).
        let resp = client
            .get(&skills_url_a)
            .bearer_auth(&bearer)
            .send()
            .await
            .expect("GET /skills old project after switch");
        let body: serde_json::Value = resp.json().await.expect("parse /skills body");
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("OUTSIDE_PROJECT_ROOT"),
            "the old project must be rejected after the boundary moved to dir_b, got: {body}"
        );

        let _ = state.stop().await;
        // dir_a / dir_b are removed by the TempDirGuard RAII guards on drop,
        // even if an assertion above panicked.
    }
}
