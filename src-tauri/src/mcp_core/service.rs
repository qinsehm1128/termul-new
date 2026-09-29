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
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path as RoutePath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use se_mcp_bridge::GatewaySettings;
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
pub fn gateway_settings_path(config_root: &Path) -> PathBuf {
    let file = if cfg!(debug_assertions) {
        "mcp-gateway.dev.json"
    } else {
        se_mcp_bridge::settings::DEFAULT_FILE_NAME
    };
    config_root
        .join(crate::brand::canonical().workspace_dir)
        .join(file)
}

pub fn default_port() -> u16 {
    let canary =
        crate::brand::canonical().workspace_dir == crate::brand::CANARY_CANONICAL.workspace_dir;
    DEFAULT_PORT + u16::from(cfg!(debug_assertions)) + 2 * u16::from(canary)
}

fn new_token() -> String {
    format!("se-mcp-{}", uuid::Uuid::new_v4().simple())
}

/// Load the settings, creating them (default port, fresh token) when absent,
/// and record where this build's binary, profile and log are.
pub fn prepare_settings(
    path: &Path,
    executable: PathBuf,
    profile_root: PathBuf,
    log_file: Option<PathBuf>,
) -> io::Result<GatewaySettings> {
    let mut settings = match GatewaySettings::load(path) {
        Ok(settings) => settings,
        Err(error) if error.kind() == io::ErrorKind::NotFound => GatewaySettings {
            schema_version: se_mcp_bridge::settings::SCHEMA_VERSION,
            port: default_port(),
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
    settings.profile_root = Some(profile_root);
    settings.log_file = log_file;
    let changed = current
        != (
            settings.executable.clone(),
            settings.profile_root.clone(),
            settings.log_file.clone(),
        );
    if changed || !path.exists() {
        settings.save(path)?;
    }
    Ok(settings)
}

/// Resolves secret references through the OS keychain; inline values pass
/// through. Never logs either.
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
    port: u16,
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

impl ServiceState {
    async fn status(&self) -> ServiceStatus {
        let (config_revision, config_error) = self.revision.read().await.clone();
        ServiceStatus {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            port: self.port,
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
        let root = self.config_root.clone();
        let document =
            tokio::task::spawn_blocking(move || crate::web::mcp_servers_api::read_document(&root))
                .await;
        let config = match document {
            Ok(Ok(config)) => config.unwrap_or_else(McpControlPlaneConfig::empty),
            Ok(Err(error)) => {
                log::warn!(
                    target: LOG_TARGET,
                    "operation=mcp_service_reload stable_code={}",
                    error.code()
                );
                self.revision.write().await.1 = Some(error.message());
                return;
            }
            Err(_) => return,
        };
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
        self.core.apply_built_in_config(&config.built_ins).await;
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
        Err(ServeError::Other(message)) => {
            if logging {
                log::error!(target: LOG_TARGET, "operation=mcp_service_start stable_code=FAILED error={message}");
            }
            eprintln!("MCP gateway: {message}");
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
    let profile_root = settings
        .profile_root
        .clone()
        .or_else(|| std::env::var_os("TERMUL_CORE_PROFILE_ROOT").map(PathBuf::from));
    let builtins = match profile_root {
        Some(root) => BuiltInRegistry::memory_backed(Arc::new(MemoryIndexService::new(root)), None),
        None => BuiltInRegistry::empty(),
    };
    let core = Arc::new(McpCore::new_with_builtins(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
        builtins,
    ));
    // OAuth grants are keyed by the directory the configuration lives in.
    core.set_credential_scope(Some(config_root.clone())).await;
    let state = Arc::new(ServiceState {
        core: Arc::clone(&core),
        config_root,
        port: settings.port,
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

    let watcher = tokio::spawn(watch_config(Arc::clone(&state)));
    wait_for_stop(&state.shutdown).await;
    watcher.abort();
    gateway.shutdown().await;
    core.shutdown().await;
    log::info!(target: LOG_TARGET, "operation=mcp_service_stop stable_code=OK");
    Ok(())
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
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

#[derive(Debug)]
pub enum RemoteError {
    /// Nothing listens on the port.
    NotRunning,
    /// Something listens but does not accept our token.
    Foreign,
    Failed(String),
}

async fn remote_status(settings: &GatewaySettings) -> Result<ServiceStatus, RemoteError> {
    let response = http_client()
        .get(format!("{}/control/status", settings.base_url()))
        .bearer_auth(&settings.token)
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                RemoteError::NotRunning
            } else {
                RemoteError::Failed(error.to_string())
            }
        })?;
    if !response.status().is_success() {
        return Err(RemoteError::Foreign);
    }
    response
        .json::<ServiceStatus>()
        .await
        .map_err(|_| RemoteError::Foreign)
}

/// The GUI's handle on the gateway process.
pub struct McpService {
    settings_path: PathBuf,
    executable: PathBuf,
    profile_root: PathBuf,
    log_file: Option<PathBuf>,
    lifecycle: Mutex<()>,
}

/// What the MCP page shows about the gateway.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
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
        settings_path: PathBuf,
        executable: PathBuf,
        profile_root: PathBuf,
        log_file: Option<PathBuf>,
    ) -> Self {
        Self {
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

    pub fn settings(&self) -> io::Result<GatewaySettings> {
        prepare_settings(
            &self.settings_path,
            self.executable.clone(),
            self.profile_root.clone(),
            self.log_file.clone(),
        )
    }

    /// Make sure this build's gateway serves the configured port: start it
    /// when absent, replace it when it runs another version.
    pub async fn ensure_running(&self) -> Result<ServiceStatus, String> {
        let _lifecycle = self.lifecycle.lock().await;
        let settings = self.settings().map_err(|error| error.to_string())?;
        match remote_status(&settings).await {
            Ok(status) if status.version == env!("CARGO_PKG_VERSION") => return Ok(status),
            Ok(status) => {
                log::info!(
                    target: LOG_TARGET,
                    "operation=mcp_service_replace running_version={} stable_code=VERSION_CHANGED",
                    status.version
                );
                self.stop_remote(&settings).await?;
            }
            Err(RemoteError::NotRunning) => {}
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
        se_mcp_bridge::bridge::spawn_gateway(&self.executable, &self.settings_path, settings)
            .map_err(|error| format!("could not start the MCP gateway: {error}"))?;
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

    async fn stop_remote(&self, settings: &GatewaySettings) -> Result<(), String> {
        let _ = http_client()
            .post(format!("{}/control/shutdown", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await;
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
        Err(format!(
            "the MCP gateway on port {} did not stop",
            settings.port
        ))
    }

    pub async fn view(&self) -> ServiceView {
        let settings_path = self.settings_path.display().to_string();
        let settings = match self.settings() {
            Ok(settings) => settings,
            Err(error) => {
                return ServiceView {
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
            Err(RemoteError::Foreign) => (
                "error",
                Some(format!("port {} is used by another program", settings.port)),
                None,
            ),
            Err(RemoteError::Failed(error)) => ("error", Some(error), None),
        };
        ServiceView {
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
        let _ = http_client()
            .post(format!("{}/control/reload", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await;
    }

    /// Move the gateway to another port: record it, stop the old process,
    /// start on the new port. Connected agents follow on their next request.
    pub async fn set_port(&self, port: u16) -> Result<ServiceStatus, String> {
        if port < 1024 {
            return Err("choose a port between 1024 and 65535".into());
        }
        let mut settings = self.settings().map_err(|error| error.to_string())?;
        if settings.port != port {
            let _lifecycle = self.lifecycle.lock().await;
            let old = settings.clone();
            settings.port = port;
            settings
                .save(&self.settings_path)
                .map_err(|error| error.to_string())?;
            if remote_status(&old).await.is_ok() {
                self.stop_remote(&old).await?;
            }
        }
        self.ensure_running().await
    }

    pub async fn restart(&self) -> Result<ServiceStatus, String> {
        {
            let _lifecycle = self.lifecycle.lock().await;
            let settings = self.settings().map_err(|error| error.to_string())?;
            if remote_status(&settings).await.is_ok() {
                self.stop_remote(&settings).await?;
            }
        }
        self.ensure_running().await
    }

    /// Tools of one server, from the running gateway.
    pub async fn server_tools(&self, name: &str) -> Result<Value, String> {
        let settings = self.settings().map_err(|error| error.to_string())?;
        let response = http_client()
            .get(format!(
                "{}/control/servers/{name}/tools",
                settings.base_url()
            ))
            .bearer_auth(&settings.token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(response.text().await.unwrap_or_default());
        }
        response.json().await.map_err(|error| error.to_string())
    }

    /// Descriptions of the routable servers, from the running gateway.
    pub async fn servers(&self) -> Result<Value, String> {
        let settings = self.settings().map_err(|error| error.to_string())?;
        http_client()
            .get(format!("{}/control/servers", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await
            .map_err(|error| error.to_string())?
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

    #[test]
    fn settings_are_created_once_and_keep_port_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-gateway.json");
        let first =
            prepare_settings(&path, "/a/se-manager".into(), "/profile".into(), None).unwrap();
        assert_eq!(first.port, default_port());
        assert!(first.token.starts_with("se-mcp-"));

        let mut moved = first.clone();
        moved.port = 4555;
        moved.save(&path).unwrap();
        let second =
            prepare_settings(&path, "/b/se-manager".into(), "/profile".into(), None).unwrap();
        assert_eq!(second.port, 4555);
        assert_eq!(second.token, first.token);
        assert_eq!(second.executable, Some(PathBuf::from("/b/se-manager")));
        assert_eq!(GatewaySettings::load(&path).unwrap(), second);
    }

    #[test]
    fn debug_builds_use_their_own_settings_file_and_port() {
        let path = gateway_settings_path(Path::new("/home/u"));
        if cfg!(debug_assertions) {
            assert!(path.ends_with("mcp-gateway.dev.json"));
            assert_eq!(default_port(), DEFAULT_PORT + 1);
        } else {
            assert!(path.ends_with("mcp-gateway.json"));
        }
    }
}
