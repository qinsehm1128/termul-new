//! The MCP gateway as its own long-lived process.
//!
//! `se-manager --mcp-core --config <settings>` binds the port recorded in the
//! gateway settings file and aggregates the global MCP configuration
//! (`~/<workspace dir>/mcp-servers.json`). It is independent of the window:
//! closing Se Manager leaves it serving the agents connected to it, and the
//! `se-mcp` stdio client starts it on demand when nothing listens.
//!
//! The GUI side ([`McpService`]) prepares the settings file, starts or
//! replaces the process (a different version is replaced), and talks to it
//! over the bearer-protected `/control/*` routes.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path as RoutePath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use se_mcp_bridge::{
    GatewaySettings, IdentityError, Source, GATEWAY_PROTOCOL, GATEWAY_PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use super::{
    snapshot_from_config_tolerant, AllowAllTools, AuthBootstrap, BuiltInRegistry,
    McpControlPlaneConfig, McpCore, McpCoreConfig, McpHttpGateway, McpHttpGatewayConfig,
    McpHttpGatewayError, McpSecretResolver, NamedSecret, SnapshotError, UpstreamState,
    UpstreamStatus,
};
use crate::memory_index::service::MemoryIndexService;

const LOG_TARGET: &str = "se_manager::mcp_core::service";
/// First port of the default range; debug and canary builds sit above it so
/// they never fight the installed app for a port.
const DEFAULT_PORT: u16 = 3290;
const CONFIG_POLL: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(150);
const REQUEST_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Where the global MCP configuration lives: `$SE_PROJECT_ROOT`, else the
/// home directory. `mcp-servers.json` sits in its workspace directory.
pub fn config_root() -> Option<PathBuf> {
    let raw = crate::web::config::default_project_root()?;
    crate::web::config::resolve_and_validate_project_root(&raw).ok()
}

/// The gateway settings file of this build. Debug builds keep their own file
/// so a development run never replaces the installed app's gateway.
pub fn gateway_settings_path(root: &Path, source: Source) -> PathBuf {
    root.join(crate::brand::canonical().workspace_dir)
        .join(source.default_file_name())
}
pub fn gateway_log_file(dir: &Path, source: Source) -> PathBuf {
    dir.join(match source {
        Source::Internal => "mcp-gateway.log",
        Source::Aggregator => "mcp-gateway-aggregator.log",
    })
}
pub fn default_port(source: Source) -> u16 {
    let canary =
        crate::brand::canonical().workspace_dir == crate::brand::CANARY_CANONICAL.workspace_dir;
    channel_port(source, cfg!(debug_assertions), canary)
}
fn channel_port(source: Source, debug: bool, canary: bool) -> u16 {
    let base = match source {
        Source::Internal => DEFAULT_PORT,
        Source::Aggregator => 3390,
    };
    base + u16::from(debug) + 2 * u16::from(canary)
}

fn new_token() -> String {
    format!("se-mcp-{}", uuid::Uuid::new_v4().simple())
}

/// Load the settings, creating them (default port, fresh token) when absent,
/// and record where this build's binary, profile and log are.
pub fn prepare_settings(
    path: &Path,
    source: Source,
    executable: PathBuf,
    profile_root: PathBuf,
    log_file: Option<PathBuf>,
) -> io::Result<GatewaySettings> {
    let mut settings = match GatewaySettings::load_for_source(path, source) {
        Ok(settings) => settings,
        Err(error) if error.kind() == io::ErrorKind::NotFound => GatewaySettings {
            schema_version: se_mcp_bridge::settings::SCHEMA_VERSION,
            source,
            port: default_port(source),
            token: new_token(),
            executable: None,
            profile_root: None,
            log_file: None,
        },
        Err(error) => return Err(error),
    };
    let current = (
        settings.executable.clone(),
        settings.profile_root.clone(),
        settings.log_file.clone(),
    );
    settings.executable = Some(executable);
    let migrated = settings.schema_version != se_mcp_bridge::settings::SCHEMA_VERSION;
    settings.schema_version = se_mcp_bridge::settings::SCHEMA_VERSION;
    settings.profile_root = (source == Source::Internal).then_some(profile_root);
    settings.log_file = log_file;
    let changed = current
        != (
            settings.executable.clone(),
            settings.profile_root.clone(),
            settings.log_file.clone(),
        );
    if changed || migrated || !path.exists() {
        settings.save(path)?;
    }
    Ok(settings)
}

/// Resolves secret references through the host credential backend; inline values
/// pass through. The active backend may be a private file, not an OS keychain.
/// Never logs either.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyringSecretResolver;

impl McpSecretResolver for KeyringSecretResolver {
    fn resolve(
        &self,
        _server_id: &str,
        _field: &str,
        _name: &str,
        value: &str,
    ) -> Result<String, SnapshotError> {
        Ok(value.to_owned())
    }

    fn resolve_named(
        &self,
        server_id: &str,
        field: &str,
        secret: &NamedSecret,
    ) -> Result<String, SnapshotError> {
        let failure = |message: &str| SnapshotError::SecretResolution {
            server_id: server_id.to_owned(),
            field: field.to_owned(),
            message: message.to_owned(),
        };
        if let Some(reference) = secret
            .reference
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return match crate::keyring_get(reference) {
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(failure("referenced credential is not available")),
                Err(_) => Err(failure("secure credential retrieval failed")),
            };
        }
        secret
            .value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| failure("secret has neither an inline value nor a reference"))
    }
}

/// What `/control/status` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub source: Source,
    pub protocol: String,
    pub protocol_version: u16,

    pub version: String,
    pub pid: u32,
    pub port: u16,
    pub started_at: u64,
    /// Revision of the configuration document last applied.
    pub config_revision: Option<u64>,
    /// Why the configuration document could not be read, if it could not.
    pub config_error: Option<String>,
    pub upstreams: Vec<UpstreamStatusView>,
    pub built_ins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamStatusView {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<UpstreamStatus> for UpstreamStatusView {
    fn from(status: UpstreamStatus) -> Self {
        let state = match status.state {
            UpstreamState::Disabled => "disabled",
            UpstreamState::Connecting => "connecting",
            UpstreamState::Connected => "connected",
            UpstreamState::Failed => "failed",
        };
        Self {
            id: status.id,
            name: status.name,
            state: state.to_owned(),
            error: status.error,
        }
    }
}

struct ServiceState {
    core: Arc<McpCore>,
    config_root: PathBuf,
    /// Deleting this file (uninstall, a removed profile) stops the gateway.
    settings_path: Option<PathBuf>,
    port: AtomicU16,
    started_at: u64,
    revision: RwLock<(Option<u64>, Option<String>)>,
    reload_lock: Mutex<()>,
    reload: Notify,
    shutdown: CancellationToken,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

fn read_source_config(
    canonical: PathBuf,
    legacy: PathBuf,
    source: Source,
) -> Result<McpControlPlaneConfig, &'static str> {
    let path = if canonical.exists() {
        canonical
    } else if legacy.exists() {
        legacy
    } else {
        return Ok(McpControlPlaneConfig::empty());
    };
    let bytes = std::fs::read(path).map_err(|_| "MCP_REGISTRY_READ_FAILED")?;
    if bytes.len() > crate::web::mcp_servers_api::MAX_REGISTRY_BYTES {
        return Err("MCP_REGISTRY_TOO_LARGE");
    }
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| "MCP_REGISTRY_INVALID")?;
    match &mut value {
        Value::Object(object) => {
            object.insert(
                match source {
                    Source::Internal => "upstreams",
                    Source::Aggregator => "builtIns",
                }
                .into(),
                json!([]),
            );
        }
        Value::Array(entries) if source == Source::Internal => entries.clear(),
        _ => {}
    }
    McpControlPlaneConfig::from_stored_json(&value)
        .map(|parsed| parsed.config)
        .map_err(|_| "MCP_REGISTRY_INVALID")
}

impl ServiceState {
    async fn status(&self) -> ServiceStatus {
        let (config_revision, config_error) = self.revision.read().await.clone();
        ServiceStatus {
            source: self.core.source(),
            protocol: GATEWAY_PROTOCOL.into(),
            protocol_version: GATEWAY_PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            port: self.port.load(Ordering::Relaxed),
            started_at: self.started_at,
            config_revision,
            config_error,
            upstreams: self
                .core
                .upstream_statuses()
                .await
                .into_iter()
                .map(Into::into)
                .collect(),
            built_ins: self.core.built_in_ids().await,
        }
    }

    /// Re-read the configuration and apply it. Unchanged upstreams keep
    /// their connections; one that cannot start is reported, not fatal.
    async fn reload(&self) {
        let _one_at_a_time = self.reload_lock.lock().await;
        // Resolve the brand seam before blocking work, and discard the opposite
        // projection before deserialization/credential validation.
        let canonical = crate::web::mcp_servers_api::registry_path(&self.config_root);
        let legacy = self
            .config_root
            .join(crate::brand::LEGACY.workspace_dir)
            .join(crate::web::mcp_servers_api::FILE_NAME);
        let source = self.core.source();
        let document =
            tokio::task::spawn_blocking(move || read_source_config(canonical, legacy, source))
                .await;
        let config = match document {
            Ok(Ok(config)) => config,
            Ok(Err(code)) => {
                log::warn!(target: LOG_TARGET, "source={} operation=mcp_service_reload stable_code={code}", source.as_str());
                self.revision.write().await.1 = Some(code.into());
                return;
            }
            Err(_) => return,
        };
        if self.core.source() == Source::Internal {
            match self.core.apply_built_in_config(&config.built_ins).await {
                Ok(()) => *self.revision.write().await = (Some(config.revision), None),
                Err(_) => self.revision.write().await.1 = Some("MCP_BUILTIN_APPLY_FAILED".into()),
            }
            return;
        }
        let revision = config.revision.max(1);
        let (snapshot, rejected) =
            match snapshot_from_config_tolerant(&config, revision, &KeyringSecretResolver) {
                Ok(built) => built,
                Err(error) => {
                    self.revision.write().await.1 = Some(error.to_string());
                    return;
                }
            };
        if let Err(error) = self.core.apply_snapshot(&snapshot).await {
            self.revision.write().await.1 = Some(error.to_string());
            return;
        }
        let root = self.config_root.clone();
        let descriptions = tokio::task::spawn_blocking(move || load_descriptions(&root))
            .await
            .unwrap_or_default();
        self.core.set_descriptions(descriptions).await;
        self.core
            .record_rejected(rejected.into_iter().map(|rejected| UpstreamStatus {
                id: rejected.id,
                name: rejected.name,
                state: UpstreamState::Failed,
                error: Some(rejected.error.to_string()),
            }))
            .await;
        *self.revision.write().await = (Some(config.revision), None);
        let statuses = self.core.upstream_statuses().await;
        let connected = statuses
            .iter()
            .filter(|status| status.state == UpstreamState::Connected)
            .count();
        let failed = statuses
            .iter()
            .filter(|status| status.state == UpstreamState::Failed)
            .count();
        log::info!(
            target: LOG_TARGET,
            "operation=mcp_service_reload revision={} connected={connected} failed={failed} stable_code=OK",
            config.revision
        );
    }
}

fn control_router(state: Arc<ServiceState>) -> Router {
    Router::new()
        .route("/control/status", get(control_status))
        .route("/control/reload", post(control_reload))
        .route("/control/shutdown", post(control_shutdown))
        .route("/control/servers", get(control_servers))
        .route("/control/servers/{name}/tools", get(control_tools))
        .with_state(state)
}

async fn control_status(State(state): State<Arc<ServiceState>>) -> Json<ServiceStatus> {
    Json(state.status().await)
}

async fn control_reload(State(state): State<Arc<ServiceState>>) -> Json<ServiceStatus> {
    state.reload().await;
    Json(state.status().await)
}

async fn control_shutdown(State(state): State<Arc<ServiceState>>) -> StatusCode {
    state.shutdown.cancel();
    StatusCode::ACCEPTED
}

async fn control_servers(State(state): State<Arc<ServiceState>>) -> Json<Value> {
    Json(json!({ "servers": state.core.server_summaries().await }))
}

async fn control_tools(
    State(state): State<Arc<ServiceState>>,
    RoutePath(name): RoutePath<String>,
) -> Response {
    match state.core.list_server_tools(&name).await {
        Ok(tools) => Json(json!({
            "server": name,
            "tools": tools.into_iter().map(|tool| json!({
                "name": tool.name,
                "description": tool.description.as_deref().unwrap_or_default(),
                "inputSchema": Value::Object((*tool.input_schema).clone()),
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    }
}

/// `se-manager --mcp-core --config <settings>`: serve until told to stop.
pub fn run_service(settings_path: PathBuf) -> i32 {
    let logging = crate::logging::install_core_logger().is_some();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("MCP gateway runtime initialization failed: {error}");
            return 1;
        }
    };
    match runtime.block_on(serve(settings_path)) {
        Ok(()) => 0,
        Err(ServeError::PortTaken(port)) => {
            let message = format!("port {port} is in use by another program");
            if logging {
                log::error!(target: LOG_TARGET, "operation=mcp_service_start stable_code=PORT_TAKEN port={port}");
            }
            eprintln!("MCP gateway: {message}");
            3
        }
        Err(ServeError::Other(_message)) => {
            if logging {
                log::error!(target: LOG_TARGET, "operation=mcp_service_start stable_code=FAILED");
            }
            eprintln!("MCP gateway: MCP_SERVICE_START_FAILED");
            1
        }
    }
}

enum ServeError {
    PortTaken(u16),
    Other(String),
}

async fn serve(settings_path: PathBuf) -> Result<(), ServeError> {
    let settings = GatewaySettings::load(&settings_path).map_err(|error| {
        ServeError::Other(format!(
            "cannot read gateway settings {}: {error}",
            settings_path.display()
        ))
    })?;
    let config_root = config_root()
        .ok_or_else(|| ServeError::Other("no home directory for the MCP configuration".into()))?;
    let core = Arc::new(match settings.source {
        Source::Internal => {
            let profile_root = settings
                .profile_root
                .clone()
                .or_else(|| std::env::var_os("TERMUL_CORE_PROFILE_ROOT").map(PathBuf::from));
            let mut builtins = match profile_root {
                Some(root) => {
                    BuiltInRegistry::memory_backed(Arc::new(MemoryIndexService::new(root)), None)
                }
                None => BuiltInRegistry::empty(),
            };
            let host_backend = Arc::new(
                super::host_bridge::RemoteHostBackend::new(super::host_bridge::descriptor_path(
                    &settings_path,
                ))
                .map_err(|_| ServeError::Other("cannot initialize host bridge client".into()))?,
            );
            builtins.register(Arc::new(
                super::host_capabilities::HostBackedCapability::workspace(host_backend.clone()),
            ));
            builtins.register(Arc::new(
                super::host_capabilities::HostBackedCapability::ssh(host_backend),
            ));
            McpCore::new_internal(McpCoreConfig::default(), Arc::new(AllowAllTools), builtins)
        }
        Source::Aggregator => {
            McpCore::new_aggregator(McpCoreConfig::default(), Arc::new(AllowAllTools))
        }
    });
    if settings.source == Source::Aggregator {
        core.set_credential_scope(Some(config_root.clone())).await;
    }
    let state = Arc::new(ServiceState {
        core: Arc::clone(&core),
        config_root,
        settings_path: Some(settings_path.clone()),
        port: AtomicU16::new(settings.port),
        started_at: unix_millis(),
        revision: RwLock::new((None, None)),
        reload_lock: Mutex::new(()),
        reload: Notify::new(),
        shutdown: CancellationToken::new(),
    });
    let auth = AuthBootstrap::new(1, settings.token.clone())
        .map_err(|error| ServeError::Other(error.message))?;
    let gateway = match McpHttpGateway::bind(
        Arc::clone(&core),
        McpHttpGatewayConfig {
            bind_address: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: settings.port,
            path: "/mcp".into(),
            generation: 1,
            auth,
            request_body_limit: REQUEST_BODY_LIMIT,
            control: Some(control_router(Arc::clone(&state))),
        },
    )
    .await
    {
        Ok(gateway) => gateway,
        Err(McpHttpGatewayError::Bind(error)) if error.kind() == io::ErrorKind::AddrInUse => {
            // Another gateway of ours already serves this port: nothing to do.
            if remote_status(&settings).await.is_ok() {
                return Ok(());
            }
            return Err(ServeError::PortTaken(settings.port));
        }
        Err(error) => return Err(ServeError::Other(error.to_string())),
    };
    log::info!(
        target: LOG_TARGET,
        "operation=mcp_service_start port={} pid={} version={} stable_code=OK",
        settings.port,
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );

    state.reload().await;
    let watcher = tokio::spawn(watch_config(Arc::clone(&state)));
    wait_for_stop(&state.shutdown).await;
    watcher.abort();
    gateway.shutdown().await;
    core.shutdown().await;
    log::info!(target: LOG_TARGET, "operation=mcp_service_stop stable_code=OK");
    Ok(())
}

/// An embedded gateway owns its configuration watcher and publishes no settings.
pub struct EmbeddedMcpService {
    gateway: McpHttpGateway,
    state: Arc<ServiceState>,
    watcher: tokio::task::JoinHandle<()>,
}

impl EmbeddedMcpService {
    pub fn endpoint(&self) -> &crate::mcp_core::McpEndpointDescriptor {
        self.gateway.endpoint()
    }

    pub async fn shutdown(self) {
        self.state.shutdown.cancel();
        self.watcher.abort();
        let _ = self.watcher.await;
        self.gateway.shutdown().await;
        self.state.core.shutdown().await;
    }
}

pub async fn start_embedded(
    core: Arc<McpCore>,
    config_root: PathBuf,
    mut config: McpHttpGatewayConfig,
) -> Result<EmbeddedMcpService, String> {
    if core.source() == Source::Aggregator {
        core.set_credential_scope(Some(config_root.clone())).await;
    }
    let state = Arc::new(ServiceState {
        core: Arc::clone(&core),
        config_root,
        settings_path: None,
        port: AtomicU16::new(config.port),
        started_at: unix_millis(),
        revision: RwLock::new((None, None)),
        reload_lock: Mutex::new(()),
        reload: Notify::new(),
        shutdown: CancellationToken::new(),
    });
    state.reload().await;
    config.control = Some(control_router(Arc::clone(&state)));
    let gateway = match McpHttpGateway::bind(Arc::clone(&core), config).await {
        Ok(gateway) => gateway,
        Err(_) => {
            core.shutdown().await;
            return Err("MCP_EMBEDDED_BIND_FAILED".into());
        }
    };
    state.port.store(gateway.endpoint().port, Ordering::Relaxed);
    let watcher = tokio::spawn(watch_config(Arc::clone(&state)));
    Ok(EmbeddedMcpService {
        gateway,
        state,
        watcher,
    })
}

/// Apply the configuration now, then again whenever the file changes or a
/// reload is requested.
async fn watch_config(state: Arc<ServiceState>) {
    let watched = [
        crate::web::mcp_servers_api::registry_path(&state.config_root),
        descriptions_path(&state.config_root),
    ];
    let stamp = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|path| {
                std::fs::metadata(path)
                    .ok()
                    .map(|metadata| (metadata.modified().ok(), metadata.len()))
            })
            .collect::<Vec<_>>()
    };
    let mut seen = stamp(&watched);
    state.reload().await;
    loop {
        tokio::select! {
            () = state.reload.notified() => {}
            () = tokio::time::sleep(CONFIG_POLL) => {
                if state.settings_path.as_ref().is_some_and(|path| {
                    !GatewaySettings::load_for_source(path, state.core.source()).is_ok_and(|settings| settings.port == state.port.load(Ordering::Relaxed))
                }) {
                    log::info!(
                        target: LOG_TARGET,
                        "operation=mcp_service_stop reason=settings_removed stable_code=OK"
                    );
                    state.shutdown.cancel();
                    return;
                }
                let current = stamp(&watched);
                if current == seen {
                    continue;
                }
                seen = current;
            }
        }
        state.reload().await;
    }
}

async fn wait_for_stop(shutdown: &CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => {
                shutdown.cancelled().await;
                return;
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => {}
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        () = shutdown.cancelled() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("gateway HTTP client configuration")
}

#[derive(Debug)]
pub enum RemoteError {
    /// Nothing listens on the port.
    NotRunning,
    /// Something listens but does not accept our token.
    Foreign,
    Legacy,
    Failed(String),
}

async fn verify_service(settings: &GatewaySettings) -> Result<(), RemoteError> {
    let origin = reqwest::Url::parse(&settings.base_url()).map_err(|_| RemoteError::Foreign)?;
    se_mcp_bridge::verify_gateway_identity(&http_client(), &origin, settings.source)
        .await
        .map(|_| ())
        .map_err(|error| match error {
            IdentityError::Unavailable => RemoteError::NotRunning,
            IdentityError::Rejected | IdentityError::InvalidIdentity => RemoteError::Legacy,
            _ => RemoteError::Foreign,
        })
}

async fn remote_status(settings: &GatewaySettings) -> Result<ServiceStatus, RemoteError> {
    verify_service(settings).await?;
    let response = http_client()
        .get(format!("{}/control/status", settings.base_url()))
        .bearer_auth(&settings.token)
        .send()
        .await
        .map_err(|_| RemoteError::Failed("MCP_STATUS_FAILED".into()))?;
    if !response.status().is_success() {
        return Err(RemoteError::Foreign);
    }
    let status = response
        .json::<ServiceStatus>()
        .await
        .map_err(|_| RemoteError::Foreign)?;
    if status.source != settings.source
        || status.protocol != GATEWAY_PROTOCOL
        || status.protocol_version != GATEWAY_PROTOCOL_VERSION
        || status.port != settings.port
    {
        return Err(RemoteError::Foreign);
    }
    Ok(status)
}

fn remote_message(error: RemoteError) -> String {
    match error {
        RemoteError::NotRunning => "MCP_NOT_RUNNING".into(),
        RemoteError::Legacy => "MCP_IDENTITY_REQUIRED".into(),
        RemoteError::Foreign => "MCP_GATEWAY_FOREIGN".into(),
        RemoteError::Failed(_) => "MCP_CONTROL_FAILED".into(),
    }
}

/// Only this lifecycle operation may authenticate an unmarked internal daemon.
async fn shutdown_legacy(settings: &GatewaySettings) -> Result<(), String> {
    if settings.source != Source::Internal {
        return Err("MCP_LEGACY_NOT_INTERNAL".into());
    }
    let response = http_client()
        .get(format!("{}/control/status", settings.base_url()))
        .bearer_auth(&settings.token)
        .send()
        .await
        .map_err(|_| "MCP_LEGACY_AUTH_FAILED")?;
    if !response.status().is_success() {
        return Err("MCP_LEGACY_AUTH_FAILED".into());
    }
    let value: Value = response.json().await.map_err(|_| "MCP_LEGACY_INVALID")?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let pid = value
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64);
    // A source/protocol-marked peer is never legacy, including a wrong source.
    if value.get("source").is_some()
        || value.get("protocol").is_some()
        || version.is_none()
        || pid.is_none()
        || value.get("port").and_then(Value::as_u64) != Some(u64::from(settings.port))
        || value.get("startedAt").and_then(Value::as_u64).is_none()
        || !value.get("upstreams").is_some_and(Value::is_array)
        || !value.get("builtIns").is_some_and(Value::is_array)
    {
        return Err("MCP_LEGACY_INVALID".into());
    }
    let response = http_client()
        .post(format!("{}/control/shutdown", settings.base_url()))
        .bearer_auth(&settings.token)
        .send()
        .await
        .map_err(|_| "MCP_LEGACY_STOP_FAILED")?;
    if !response.status().is_success() {
        return Err("MCP_LEGACY_STOP_FAILED".into());
    }
    Ok(())
}

/// The desktop owns one container; all source lifecycle mutations share its lock.
pub struct McpServices {
    internal: Arc<McpService>,
    aggregator: Arc<McpService>,
    mutation: Mutex<()>,
}

impl McpServices {
    pub fn new(
        config_root: PathBuf,
        executable: PathBuf,
        profile_root: PathBuf,
        log_dir: Option<PathBuf>,
    ) -> Self {
        let create = |source| {
            Arc::new(McpService::new(
                source,
                gateway_settings_path(&config_root, source),
                executable.clone(),
                profile_root.clone(),
                log_dir.as_ref().map(|dir| gateway_log_file(dir, source)),
            ))
        };
        Self {
            internal: create(Source::Internal),
            aggregator: create(Source::Aggregator),
            mutation: Mutex::new(()),
        }
    }

    pub fn get(&self, source: Source) -> Arc<McpService> {
        Arc::clone(match source {
            Source::Internal => &self.internal,
            Source::Aggregator => &self.aggregator,
        })
    }

    fn validate(&self) -> Result<(GatewaySettings, GatewaySettings), String> {
        let internal = self
            .internal
            .settings()
            .map_err(|_| "MCP_INTERNAL_SETTINGS_INVALID")?;
        let aggregator = self
            .aggregator
            .settings()
            .map_err(|_| "MCP_AGGREGATOR_SETTINGS_INVALID")?;
        if internal.port == aggregator.port {
            return Err("MCP_SOURCE_PORT_CONFLICT".into());
        }
        if internal.token == aggregator.token {
            return Err("MCP_SOURCE_TOKEN_CONFLICT".into());
        }
        Ok((internal, aggregator))
    }

    pub async fn ensure_running(&self, source: Source) -> Result<ServiceStatus, String> {
        let _mutation = self.mutation.lock().await;
        self.validate()?;
        self.get(source).ensure_running().await
    }

    pub async fn restart(&self, source: Source) -> Result<ServiceStatus, String> {
        let _mutation = self.mutation.lock().await;
        self.validate()?;
        self.get(source).restart().await
    }

    pub async fn set_port(&self, source: Source, port: u16) -> Result<ServiceStatus, String> {
        let _mutation = self.mutation.lock().await;
        let (internal, aggregator) = self.validate()?;
        let other = match source {
            Source::Internal => aggregator,
            Source::Aggregator => internal,
        };
        if port == other.port {
            return Err("MCP_SOURCE_PORT_CONFLICT".into());
        }
        self.get(source).set_port(port).await
    }

    pub async fn reload_all(&self) {
        tokio::join!(self.internal.reload(), self.aggregator.reload());
    }
}

/// The GUI's handle on one immutable-source gateway process.
pub struct McpService {
    source: Source,
    settings_path: PathBuf,
    executable: PathBuf,
    profile_root: PathBuf,
    log_file: Option<PathBuf>,
    lifecycle: Mutex<()>,
}

/// How an agent reaches the gateway: the stdio command (recommended) or the
/// HTTP endpoint with its bearer token.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub source: Source,
    pub http_ready: bool,

    pub mode: &'static str,
    pub bridge: super::clients::BridgeCommand,
    pub bridge_available: bool,
    pub url: String,
    pub token: String,
    pub port: u16,
    pub settings_path: String,
}

/// What the MCP page shows about the gateway.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub source: Source,

    pub state: &'static str,
    pub port: Option<u16>,
    pub settings_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceStatus>,
}

impl McpService {
    pub fn new(
        source: Source,
        settings_path: PathBuf,
        executable: PathBuf,
        profile_root: PathBuf,
        log_file: Option<PathBuf>,
    ) -> Self {
        Self {
            source,
            settings_path,
            executable,
            profile_root,
            log_file,
            lifecycle: Mutex::new(()),
        }
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    /// `se-mcp` ships next to the app binary.
    pub fn bridge_executable(&self) -> PathBuf {
        self.executable
            .with_file_name(format!("se-mcp{}", std::env::consts::EXE_SUFFIX))
    }

    /// The command an agent runs to reach this gateway in `mode`. Arguments
    /// are left out when they are the client's defaults.
    pub fn bridge_command(&self, mode: se_mcp_bridge::Mode) -> super::clients::BridgeCommand {
        let mut args = vec!["--source".into(), self.source.as_str().into()];
        if mode != se_mcp_bridge::Mode::Grouped {
            args.extend(["--mode".to_owned(), mode.as_str().to_owned()]);
        }
        if se_mcp_bridge::settings::default_path_for_source(self.source).as_deref()
            != Some(self.settings_path.as_path())
        {
            args.extend([
                "--config".to_owned(),
                self.settings_path.display().to_string(),
            ]);
        }
        super::clients::BridgeCommand {
            command: self.bridge_executable().display().to_string(),
            args,
        }
    }

    /// Everything the MCP page needs to show how an agent connects.
    pub async fn connection(&self, mode: se_mcp_bridge::Mode) -> Result<Connection, String> {
        let settings = self
            .settings()
            .map_err(|_| "MCP_SETTINGS_INVALID".to_owned())?;
        let http_ready = remote_status(&settings).await.is_ok();
        Ok(Connection {
            source: self.source,
            http_ready,
            mode: mode.as_str(),
            bridge: self.bridge_command(mode),
            bridge_available: self.bridge_executable().is_file(),
            url: format!("{}{}", settings.base_url(), mode.path()),
            token: settings.token,
            port: settings.port,
            settings_path: self.settings_path.display().to_string(),
        })
    }

    pub fn settings(&self) -> io::Result<GatewaySettings> {
        prepare_settings(
            &self.settings_path,
            self.source,
            self.executable.clone(),
            self.profile_root.clone(),
            self.log_file.clone(),
        )
    }

    /// Make sure this build's gateway serves the configured port: start it
    /// when absent, replace it when it runs another version.
    pub async fn ensure_running(&self) -> Result<ServiceStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        self.ensure_running_locked().await
    }

    async fn ensure_running_locked(&self) -> Result<ServiceStatus, String> {
        let settings = self.settings().map_err(|_| "MCP_SETTINGS_INVALID")?;
        match remote_status(&settings).await {
            Ok(status) if status.version == env!("CARGO_PKG_VERSION") => return Ok(status),
            Ok(_status) => {
                log::info!(
                    target: LOG_TARGET,
                    "operation=mcp_service_replace source={} stable_code=VERSION_CHANGED",
                    self.source.as_str()
                );
                self.stop_remote(&settings).await?;
            }
            Err(RemoteError::NotRunning) => {}
            Err(RemoteError::Legacy) if self.source == Source::Internal => {
                shutdown_legacy(&settings).await?;
                self.wait_stopped(&settings).await?;
            }
            Err(RemoteError::Legacy) => return Err("MCP_IDENTITY_REQUIRED".into()),
            Err(RemoteError::Foreign) => {
                return Err(format!(
                    "port {} is used by another program; choose another port",
                    settings.port
                ))
            }
            Err(RemoteError::Failed(error)) => return Err(error),
        }
        self.start(&settings).await
    }

    async fn start(&self, settings: &GatewaySettings) -> Result<ServiceStatus, String> {
        if let Some(log_file) = &self.log_file {
            if let Some(parent) = log_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        if !self.launch_in_host_app(settings) {
            se_mcp_bridge::bridge::spawn_gateway(&self.executable, &self.settings_path, settings)
                .map_err(|error| format!("could not start the MCP gateway: {error}"))?;
        }
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            match remote_status(settings).await {
                Ok(status) => return Ok(status),
                Err(RemoteError::Foreign) => {
                    return Err(format!(
                        "port {} is used by another program; choose another port",
                        settings.port
                    ))
                }
                Err(_) if tokio::time::Instant::now() < deadline => tokio::time::sleep(POLL).await,
                Err(_) => {
                    return Err(format!(
                        "the MCP gateway did not start on port {}",
                        settings.port
                    ))
                }
            }
        }
    }

    /// The gateway outlives the GUI. On macOS it runs in its own app (see
    /// `core::app_host`); as a child it would be stopped with the GUI from the
    /// Dock. Returns `false` where that is not available.
    #[cfg(target_os = "macos")]
    fn launch_in_host_app(&self, settings: &GatewaySettings) -> bool {
        let mut env: Vec<(&str, &Path)> = Vec::new();
        if let Some(profile_root) = &settings.profile_root {
            env.push(("TERMUL_CORE_PROFILE_ROOT", profile_root));
        }
        if let Some(log_file) = &settings.log_file {
            env.push(("TERMUL_CORE_LOG_FILE", log_file));
        }
        crate::core::app_host::launch(
            match self.source {
                Source::Internal => crate::core::app_host::MCP_GATEWAY,
                Source::Aggregator => crate::core::app_host::MCP_AGGREGATOR,
            },
            &self.executable,
            &[
                std::ffi::OsStr::new("--mcp-core"),
                std::ffi::OsStr::new("--config"),
                self.settings_path.as_os_str(),
            ],
            &env,
            None,
            &self.profile_root,
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn launch_in_host_app(&self, _settings: &GatewaySettings) -> bool {
        false
    }

    async fn stop_remote(&self, settings: &GatewaySettings) -> Result<(), String> {
        verify_service(settings).await.map_err(remote_message)?;
        let response = http_client()
            .post(format!("{}/control/shutdown", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await
            .map_err(|_| "MCP_SHUTDOWN_FAILED")?;
        if !response.status().is_success() {
            return Err("MCP_SHUTDOWN_REJECTED".into());
        }
        self.wait_stopped(settings).await
    }

    async fn wait_stopped(&self, settings: &GatewaySettings) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if tokio::net::TcpStream::connect(("127.0.0.1", settings.port))
                .await
                .is_err()
            {
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
        Err("MCP_STOP_TIMEOUT".into())
    }

    pub async fn view(&self) -> ServiceView {
        let settings_path = self.settings_path.display().to_string();
        let settings = match self.settings() {
            Ok(settings) => settings,
            Err(error) => {
                return ServiceView {
                    source: self.source,
                    state: "error",
                    port: None,
                    settings_path,
                    error: Some(error.to_string()),
                    status: None,
                }
            }
        };
        let (state, error, status) = match remote_status(&settings).await {
            Ok(status) => ("running", None, Some(status)),
            Err(RemoteError::NotRunning) => ("stopped", None, None),
            Err(RemoteError::Foreign | RemoteError::Legacy) => (
                "error",
                Some(format!("port {} is used by another program", settings.port)),
                None,
            ),
            Err(RemoteError::Failed(error)) => ("error", Some(error), None),
        };
        ServiceView {
            source: self.source,
            state,
            port: Some(settings.port),
            settings_path,
            error,
            status,
        }
    }

    /// Apply the configuration file now instead of on the next poll.
    pub async fn reload(&self) {
        let Ok(settings) = self.settings() else {
            return;
        };
        if verify_service(&settings).await.is_err() {
            return;
        }
        let _ = http_client()
            .post(format!("{}/control/reload", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await;
    }

    /// Move the gateway to another port: record it, stop the old process,
    /// start on the new port. Connected agents follow on their next request.
    pub async fn set_port(&self, port: u16) -> Result<ServiceStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        if port < 1024 {
            return Err("MCP_PORT_INVALID".into());
        }
        let old = self.settings().map_err(|_| "MCP_SETTINGS_INVALID")?;
        if old.port == port {
            return self.ensure_running_locked().await;
        }
        let probe =
            std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|_| "MCP_PORT_IN_USE")?;
        let running = match remote_status(&old).await {
            Ok(_) => true,
            Err(RemoteError::NotRunning) => false,
            Err(error) => return Err(remote_message(error)),
        };
        if running {
            self.stop_remote(&old).await?;
        }
        let mut next = old.clone();
        next.port = port;
        if next.save(&self.settings_path).is_err() {
            if running {
                let _ = self.start(&old).await;
            }
            return Err("MCP_SETTINGS_WRITE_FAILED".into());
        }
        drop(probe);
        match self.start(&next).await {
            Ok(status) => Ok(status),
            Err(error) => {
                // Never leave the selected source pointing at an unsuccessful move.
                let _ = self.stop_remote(&next).await;
                old.save(&self.settings_path)
                    .map_err(|_| "MCP_PORT_ROLLBACK_FAILED")?;
                if running {
                    self.start(&old)
                        .await
                        .map_err(|_| "MCP_PORT_ROLLBACK_START_FAILED")?;
                }
                Err(error)
            }
        }
    }

    pub async fn restart(&self) -> Result<ServiceStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        let settings = self.settings().map_err(|_| "MCP_SETTINGS_INVALID")?;
        match remote_status(&settings).await {
            Ok(_) => self.stop_remote(&settings).await?,
            Err(RemoteError::NotRunning) => {}
            Err(RemoteError::Legacy) if self.source == Source::Internal => {
                shutdown_legacy(&settings).await?;
                self.wait_stopped(&settings).await?;
            }
            Err(error) => return Err(remote_message(error)),
        }
        self.start(&settings).await
    }

    /// Tools of one server, from the running gateway.
    pub async fn server_tools(&self, name: &str) -> Result<Value, String> {
        let settings = self.settings().map_err(|_| "MCP_SETTINGS_INVALID")?;
        verify_service(&settings).await.map_err(remote_message)?;
        let response = http_client()
            .get(format!(
                "{}/control/servers/{name}/tools",
                settings.base_url()
            ))
            .bearer_auth(&settings.token)
            .send()
            .await
            .map_err(|_| "MCP_OPERATION_FAILED".to_owned())?;
        if !response.status().is_success() {
            return Err("MCP_CONTROL_REJECTED".into());
        }
        response.json().await.map_err(|error| error.to_string())
    }

    /// Descriptions of the routable servers, from the running gateway.
    pub async fn servers(&self) -> Result<Value, String> {
        let settings = self.settings().map_err(|_| "MCP_SETTINGS_INVALID")?;
        verify_service(&settings).await.map_err(remote_message)?;
        http_client()
            .get(format!("{}/control/servers", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await
            .map_err(|_| "MCP_OPERATION_FAILED".to_owned())?
            .json()
            .await
            .map_err(|error| error.to_string())
    }
}

/// The per-server descriptions file, if any, as config id → text.
pub fn descriptions_path(config_root: &Path) -> PathBuf {
    config_root
        .join(crate::brand::canonical().workspace_dir)
        .join("mcp-descriptions.json")
}

pub fn load_descriptions(config_root: &Path) -> BTreeMap<String, String> {
    std::fs::read(descriptions_path(config_root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn settings_are_created_once_and_keep_port_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-gateway.json");
        let first = prepare_settings(
            &path,
            Source::Internal,
            "/a/se-manager".into(),
            "/profile".into(),
            None,
        )
        .unwrap();
        assert_eq!(first.port, default_port(Source::Internal));
        assert!(first.token.starts_with("se-mcp-"));

        let mut moved = first.clone();
        moved.port = 4555;
        moved.save(&path).unwrap();
        let second = prepare_settings(
            &path,
            Source::Internal,
            "/b/se-manager".into(),
            "/profile".into(),
            None,
        )
        .unwrap();
        assert_eq!(second.port, 4555);
        assert_eq!(second.token, first.token);
        assert_eq!(second.executable, Some(PathBuf::from("/b/se-manager")));
        assert_eq!(GatewaySettings::load(&path).unwrap(), second);
    }

    #[test]
    fn control_document_is_projected_before_typed_validation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mcp-servers.json");
        let missing = root.path().join("missing.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schemaVersion": 1, "revision": 7,
                "builtIns": [{"id":"session-memory","enabled":false}],
                "upstreams": [{"not":"an upstream", "credentials":{"secretRef":123}}]
            }))
            .unwrap(),
        )
        .unwrap();
        let internal = read_source_config(path.clone(), missing.clone(), Source::Internal).unwrap();
        assert_eq!(internal.revision, 7);
        assert!(internal.upstreams.is_empty());
        assert!(read_source_config(path.clone(), missing.clone(), Source::Aggregator).is_err());
        std::fs::write(&path, serde_json::to_vec(&json!({
            "schemaVersion":1, "revision":8, "builtIns":"malformed-opposite-projection", "upstreams":[]
        })).unwrap()).unwrap();
        assert_eq!(
            read_source_config(path.clone(), missing.clone(), Source::Aggregator)
                .unwrap()
                .revision,
            8
        );
        assert!(read_source_config(path, missing, Source::Internal).is_err());
    }

    #[test]
    fn ports_are_disjoint_in_all_channels() {
        for debug in [false, true] {
            for canary in [false, true] {
                let offset = u16::from(debug) + 2 * u16::from(canary);
                assert_eq!(channel_port(Source::Internal, debug, canary), 3290 + offset);
                assert_eq!(
                    channel_port(Source::Aggregator, debug, canary),
                    3390 + offset
                );
            }
        }
    }

    #[test]
    fn schema_one_internal_token_is_retained_and_aggregator_is_independent() {
        let root = tempfile::tempdir().unwrap();
        let internal_path = gateway_settings_path(root.path(), Source::Internal);
        std::fs::create_dir_all(internal_path.parent().unwrap()).unwrap();
        std::fs::write(
            &internal_path,
            serde_json::to_vec(&json!({"schemaVersion":1,"port":43219,"token":"legacy-kept"}))
                .unwrap(),
        )
        .unwrap();
        let services = McpServices::new(
            root.path().into(),
            "/missing/se-manager".into(),
            root.path().join("profile"),
            None,
        );
        let (internal, aggregator) = services.validate().unwrap();
        assert_eq!(internal.port, 43219);
        assert_eq!(internal.token, "legacy-kept");
        assert_eq!(internal.schema_version, 2);
        assert_ne!(internal.token, aggregator.token);
        assert_eq!(aggregator.source, Source::Aggregator);
        assert_eq!(aggregator.profile_root, None);
        assert_eq!(
            GatewaySettings::load(&internal_path)
                .unwrap()
                .schema_version,
            2
        );
        assert!(!root.path().join("profile").exists());
    }

    #[tokio::test]
    async fn container_rejects_cross_source_conflicts_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let services = McpServices::new(
            root.path().into(),
            "/missing/se-manager".into(),
            root.path().join("profile"),
            None,
        );
        let (internal, aggregator) = services.validate().unwrap();
        assert_eq!(
            services
                .set_port(Source::Internal, aggregator.port)
                .await
                .unwrap_err(),
            "MCP_SOURCE_PORT_CONFLICT"
        );
        assert_eq!(services.get(Source::Internal).settings().unwrap(), internal);
        assert_eq!(
            services.get(Source::Aggregator).settings().unwrap(),
            aggregator
        );
        let mut copied = aggregator.clone();
        copied.token = internal.token.clone();
        copied
            .save(&services.get(Source::Aggregator).settings_path)
            .unwrap();
        assert_eq!(
            services.ensure_running(Source::Internal).await.unwrap_err(),
            "MCP_SOURCE_TOKEN_CONFLICT"
        );
    }

    #[tokio::test]
    async fn failed_port_move_restores_only_selected_settings() {
        let root = tempfile::tempdir().unwrap();
        let services = McpServices::new(
            root.path().into(),
            "/missing/se-manager".into(),
            root.path().join("profile"),
            None,
        );
        let (mut internal, aggregator) = services.validate().unwrap();
        let free_port = || {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        internal.port = free_port();
        internal
            .save(&services.get(Source::Internal).settings_path)
            .unwrap();
        let mut next = free_port();
        while next == internal.port || next == aggregator.port {
            next = free_port();
        }
        assert!(services.set_port(Source::Internal, next).await.is_err());
        assert_eq!(services.get(Source::Internal).settings().unwrap(), internal);
        assert_eq!(
            services.get(Source::Aggregator).settings().unwrap(),
            aggregator
        );
    }

    #[tokio::test]
    async fn wrong_identity_never_receives_control_bearer() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = Router::new()
            .route(
                "/mcp-core/identity",
                get(|| async { Json(se_mcp_bridge::GatewayIdentity::new(Source::Aggregator)) }),
            )
            .route(
                "/control/status",
                get(move || {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let service = McpService::new(
            Source::Internal,
            root.path().join("settings.json"),
            "/missing/se-manager".into(),
            root.path().join("profile"),
            None,
        );
        let mut settings = service.settings().unwrap();
        settings.port = port;
        settings.save(&service.settings_path).unwrap();
        assert!(matches!(
            remote_status(&settings).await,
            Err(RemoteError::Foreign)
        ));
        assert!(service.ensure_running().await.is_err());
        assert!(
            !service
                .connection(se_mcp_bridge::Mode::Direct)
                .await
                .unwrap()
                .http_ready
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn same_version_unmarked_internal_is_only_used_for_authenticated_shutdown() {
        use std::sync::atomic::AtomicUsize;
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&shutdowns);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = CancellationToken::new();
        let stop_request = stop.clone();
        let server_stop = stop.clone();
        let router = Router::new()
            .route("/control/status", get(move |headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer legacy-secret") {
                    return (StatusCode::UNAUTHORIZED, Json(json!({})));
                }
                (StatusCode::OK, Json(json!({"version":env!("CARGO_PKG_VERSION"),"port":port,"pid":1234,"startedAt":1,"upstreams":[],"builtIns":[]})))
            }))
            .route("/control/shutdown", post(move |headers: HeaderMap| {
                let count = Arc::clone(&count); let stop = stop_request.clone(); async move {
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer legacy-secret") { return StatusCode::UNAUTHORIZED; }
                    count.fetch_add(1, Ordering::SeqCst); stop.cancel(); StatusCode::OK
                }
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(server_stop.cancelled_owned())
                .await
                .unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let service = McpService::new(
            Source::Internal,
            root.path().join("settings.json"),
            "/missing/se-manager".into(),
            root.path().join("profile"),
            None,
        );
        let mut settings = service.settings().unwrap();
        settings.port = port;
        settings.token = "legacy-secret".into();
        settings.save(&service.settings_path).unwrap();
        assert!(matches!(
            remote_status(&settings).await,
            Err(RemoteError::Legacy)
        ));
        assert!(
            service.ensure_running().await.is_err(),
            "missing replacement executable must not adopt legacy"
        );
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }

    #[test]
    fn debug_builds_use_their_own_settings_file_and_port() {
        let path = gateway_settings_path(Path::new("/home/u"), Source::Internal);
        if cfg!(debug_assertions) {
            assert!(path.ends_with("mcp-gateway.dev.json"));
            assert_eq!(default_port(Source::Internal), DEFAULT_PORT + 1);
        } else {
            assert!(path.ends_with("mcp-gateway.json"));
        }
    }
}
