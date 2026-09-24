//! Desktop-owned MCP Core runtime holder.
//!
//! The desktop does not start MCP Core by default. A desktop gateway requires
//! an explicit bearer-token and port owner. When enabled, this holder owns one
//! authenticated, loopback-only in-process gateway, publishes its endpoint to
//! ACP, applies the canonical project snapshot through the secure-storage
//! boundary, and reports disabled/failed state without guessing configuration.
//!
//! Standalone `se-server` keeps its existing composition in `server_main.rs`.
//! This module is intentionally not part of the Terminal/ACP Core process-role
//! supervisor and never owns or terminates PTY/ACP children.

use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use super::{
    AllowAllTools, AuthBootstrap, BuiltInRegistry, McpControlPlaneConfig, McpCore, McpCoreConfig,
    McpCoreProcessConfig, McpDiagnostic, McpEndpointDescriptor, McpHttpGateway,
    McpHttpGatewayConfig, McpReadiness, McpSecretResolver, McpSnapshotController,
    SnapshotApplyReceipt, SnapshotError,
};
use crate::mcp_core::NamedSecret;
use crate::memory_index::service::MemoryIndexService;

/// Desktop opt-in switch.  The default is disabled so an installation never
/// binds a listener or invents an auth owner during normal startup.
pub const MCP_CORE_ENABLED_ENV: &str = "TERMUL_MCP_CORE_ENABLED";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopMcpCoreAvailability {
    Disabled,
    Available,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopMcpCoreStatus {
    pub availability: DesktopMcpCoreAvailability,
    pub endpoint: Option<McpEndpointDescriptor>,
    pub diagnostic: Option<McpDiagnostic>,
    /// The revision of the last-known-good snapshot applied to the live Core.
    /// A failed or stale replacement never clears this value.
    pub snapshot_revision: Option<u64>,
    pub snapshot_diagnostic: Option<McpDiagnostic>,
}

impl DesktopMcpCoreStatus {
    fn disabled(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            availability: DesktopMcpCoreAvailability::Disabled,
            endpoint: None,
            diagnostic: Some(McpDiagnostic::redacted(code, message, false)),
            snapshot_revision: None,
            snapshot_diagnostic: Some(McpDiagnostic::redacted(
                "MCP_SNAPSHOT_NOT_APPLIED",
                "canonical project snapshot is not applied while the desktop MCP Core is absent",
                false,
            )),
        }
    }

    fn failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            availability: DesktopMcpCoreAvailability::Failed,
            endpoint: None,
            diagnostic: Some(McpDiagnostic::redacted(code, message, true)),
            snapshot_revision: None,
            snapshot_diagnostic: Some(McpDiagnostic::redacted(
                "MCP_SNAPSHOT_NOT_APPLIED",
                "canonical project snapshot will apply when the desktop project registry is synchronized",
                true,
            )),
        }
    }
}

/// Owns the desktop gateway task without owning any ACP/PTY subprocess.
///
/// The endpoint/auth pair is retained privately for the one ACP application
/// call made by desktop setup.  Status intentionally never includes the bearer
/// token.
pub struct DesktopMcpCoreRuntime {
    gateway: Mutex<Option<McpHttpGateway>>,
    router: RwLock<Option<(McpEndpointDescriptor, AuthBootstrap)>>,
    snapshot: Option<Arc<McpSnapshotController>>,
    project_scope: Mutex<Option<std::path::PathBuf>>,
    project_apply_lock: Mutex<()>,
    status: RwLock<DesktopMcpCoreStatus>,
}

impl std::fmt::Debug for DesktopMcpCoreRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopMcpCoreRuntime")
            .field("gateway", &"instance-owned")
            .field("router", &"private-endpoint-auth")
            .field("snapshot", &"last-known-good")
            .field("status", &"async-owned")
            .finish_non_exhaustive()
    }
}

impl DesktopMcpCoreRuntime {
    /// Construct the explicit disabled state used when opt-in is absent or
    /// when another desktop composition owns ACP and cannot accept a router
    /// endpoint through the current protocol.
    pub fn disabled(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            gateway: Mutex::new(None),
            router: RwLock::new(None),
            snapshot: None,
            project_scope: Mutex::new(None),
            project_apply_lock: Mutex::new(()),
            status: RwLock::new(DesktopMcpCoreStatus::disabled(code, message)),
        }
    }

    /// Read the desktop opt-in and existing `TERMUL_MCP_CORE_*` auth/bind
    /// configuration.  `TERMUL_MCP_CORE_ENABLED` must be truthy;
    /// `TERMUL_MCP_CORE_PORT` and `TERMUL_MCP_CORE_AUTH_TOKEN` are required;
    /// bind defaults to loopback and request limit/auth generation retain the
    /// process-role defaults. Invalid or incomplete opt-in configuration
    /// degrades to a typed failed state; it never aborts desktop startup.
    pub async fn start_from_env(memory_index: Arc<MemoryIndexService>) -> Self {
        if !env_truthy(std::env::var(MCP_CORE_ENABLED_ENV).ok().as_deref()) {
            log::info!(
                target: "se_manager::mcp_core::desktop",
                "operation=mcp_desktop_runtime state=disabled stable_code=OPT_IN_REQUIRED"
            );
            return Self::disabled(
                "MCP_CORE_DISABLED",
                "desktop MCP Core is disabled; set TERMUL_MCP_CORE_ENABLED=1 with loopback auth configuration to opt in",
            );
        }

        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                return Self::failed_from_error(
                    "MCP_CORE_EXECUTABLE_UNAVAILABLE",
                    format!(
                        "cannot resolve desktop executable for MCP Core configuration: {error}"
                    ),
                )
            }
        };
        let process = match McpCoreProcessConfig::from_env(executable) {
            Ok(process) => process,
            Err(error) => {
                return Self::failed_from_error("MCP_CORE_CONFIG_INVALID", error.to_string());
            }
        };
        let config = McpHttpGatewayConfig {
            bind_address: process.bind_address,
            port: process.port,
            path: "/mcp".into(),
            // Keep endpoint and auth generations coherent.  ACP uses this
            // descriptor as the generation-fenced router contract.
            generation: process.auth.generation,
            auth: process.auth,
            request_body_limit: process.request_body_limit,
        };
        Self::start_with_config(memory_index, config).await
    }

    /// Start an authenticated loopback gateway with an already validated
    /// config.  This seam keeps lifecycle tests deterministic (port `0` is
    /// allowed by `McpHttpGateway`) without mutating process environment.
    pub async fn start_with_config(
        memory_index: Arc<MemoryIndexService>,
        config: McpHttpGatewayConfig,
    ) -> Self {
        let auth = config.auth.clone();
        let core = Arc::new(McpCore::new_with_builtins(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
            BuiltInRegistry::memory_backed(memory_index, None),
        ));
        match McpHttpGateway::bind(Arc::clone(&core), config).await {
            Ok(gateway) => {
                if let Err(error) = wait_until_ready(&gateway).await {
                    gateway.shutdown().await;
                    return Self::failed_from_error("MCP_CORE_NOT_READY", error);
                }
                let endpoint = gateway.endpoint().clone();
                log::info!(
                    target: "se_manager::mcp_core::desktop",
                    "operation=mcp_desktop_runtime state=available generation={} port={} auth_generation={} loopback=1 stable_code=READY",
                    endpoint.generation,
                    endpoint.port,
                    auth.generation
                );
                let snapshot = Arc::new(McpSnapshotController::new(Arc::clone(&core)));
                Self {
                    gateway: Mutex::new(Some(gateway)),
                    router: RwLock::new(Some((endpoint.clone(), auth))),
                    snapshot: Some(snapshot),
                    project_scope: Mutex::new(None),
                    project_apply_lock: Mutex::new(()),
                    status: RwLock::new(DesktopMcpCoreStatus {
                        availability: DesktopMcpCoreAvailability::Available,
                        endpoint: Some(endpoint),
                        diagnostic: None,
                        snapshot_revision: None,
                        snapshot_diagnostic: Some(McpDiagnostic::redacted(
                            "MCP_SNAPSHOT_DEFERRED",
                            "canonical project snapshot will apply when the desktop project registry is synchronized",
                            true,
                        )),
                    }),
                }
            }
            Err(error) => Self::failed_from_error("MCP_CORE_BIND_FAILED", error.to_string()),
        }
    }

    fn failed_from_error(code: &'static str, message: String) -> Self {
        log::error!(
            target: "se_manager::mcp_core::desktop",
            "operation=mcp_desktop_runtime state=failed stable_code={code}"
        );
        Self::from_status(DesktopMcpCoreStatus::failed(code, message))
    }

    fn from_status(status: DesktopMcpCoreStatus) -> Self {
        Self {
            gateway: Mutex::new(None),
            router: RwLock::new(None),
            snapshot: None,
            project_scope: Mutex::new(None),
            project_apply_lock: Mutex::new(()),
            status: RwLock::new(status),
        }
    }

    /// Endpoint/auth for `AcpManager::apply_mcp_router_endpoint`.  The token
    /// is available only to this internal setup seam and is never serialized.
    pub async fn endpoint_auth(&self) -> Option<(McpEndpointDescriptor, AuthBootstrap)> {
        self.router.read().await.clone()
    }

    pub async fn status(&self) -> DesktopMcpCoreStatus {
        self.status.read().await.clone()
    }

    /// Apply a canonical project document through the desktop-owned snapshot
    /// controller. Parsing and secret resolution happen before the controller
    /// touches the live Core, so invalid/stale/secret failures retain the
    /// controller's last-known-good snapshot.
    pub async fn apply_document(
        &self,
        document: &Value,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        match McpControlPlaneConfig::from_stored_json(document) {
            Ok(parsed) => self.apply_config(&parsed.config).await,
            Err(error) => {
                let error = SnapshotError::InvalidRegistry(error.to_string());
                self.record_snapshot_error(&error).await;
                Err(error)
            }
        }
    }

    /// Apply one normalized canonical config. The desktop resolver returns
    /// inline values directly and sends only explicit `ref` tokens to the
    /// secure-storage owner; it never logs either token or resolved value.
    pub async fn apply_config(
        &self,
        config: &McpControlPlaneConfig,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        let Some(controller) = self.snapshot.as_ref() else {
            let error =
                SnapshotError::Unavailable("desktop MCP Core snapshot controller is absent".into());
            self.record_snapshot_error(&error).await;
            return Err(error);
        };
        let result = controller
            .apply_config(config, &DesktopMcpSecretResolver)
            .await;
        match &result {
            Ok(receipt) => {
                let mut status = self.status.write().await;
                status.snapshot_revision = Some(receipt.revision);
                status.snapshot_diagnostic = None;
            }
            Err(error) => self.record_snapshot_error(error).await,
        }
        result
    }

    /// Record a redacted snapshot diagnostic without changing the accepted
    /// revision. Callers use this for canonical read/project-root failures.
    pub async fn record_snapshot_error(&self, error: &SnapshotError) {
        self.record_snapshot_diagnostic(snapshot_diagnostic(error))
            .await;
    }

    /// Record a caller-owned, already-redacted diagnostic without changing the
    /// accepted revision. This is used for canonical document read failures,
    /// before there is a `SnapshotError` from the snapshot adapter.
    pub async fn record_snapshot_diagnostic(&self, diagnostic: McpDiagnostic) {
        let accepted = match self.snapshot.as_ref() {
            Some(controller) => controller.accepted_revision().await,
            None => 0,
        };
        let mut status = self.status.write().await;
        if accepted > 0 {
            status.snapshot_revision = Some(accepted);
        }
        status.snapshot_diagnostic = Some(diagnostic);
    }

    /// Load and apply the canonical project MCP document for the current
    /// ProjectRegistry default. The durable document is the only source used
    /// here; no user registry or renderer pass-through is consulted.
    pub async fn refresh_from_project_registry(
        &self,
        project_registry: &crate::web::ProjectRegistry,
    ) -> Result<(), String> {
        let loaded = crate::commands::load_mcp_registry_from_project_file(project_registry).await;
        let code = loaded
            .code
            .clone()
            .unwrap_or_else(|| "MCP_SNAPSHOT_READ_FAILED".into());
        let Some(document) = loaded.data else {
            let diagnostic = match code.as_str() {
                "MCP_REGISTRY_NOT_FOUND" => McpDiagnostic::redacted(
                    "MCP_SNAPSHOT_NOT_FOUND",
                    "canonical MCP document is not present for the active project",
                    false,
                ),
                _ => McpDiagnostic::redacted(
                    "MCP_SNAPSHOT_READ_FAILED",
                    "canonical MCP document could not be loaded",
                    true,
                ),
            };
            self.record_snapshot_diagnostic(diagnostic).await;
            return Err(code);
        };
        self.apply_document_for_project_registry(&document, project_registry)
            .await
            .map(|_| ())
            .map_err(|error| snapshot_error_code(&error).to_owned())
    }

    /// Apply a canonical write against the current project scope. Commands use
    /// this after durable `mcp_put`/`remote_sync` writes so persistence remains
    /// authoritative even when runtime secret resolution or upstream startup
    /// fails.
    pub async fn apply_document_for_project_registry(
        &self,
        document: &Value,
        project_registry: &crate::web::ProjectRegistry,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        let project_root = match crate::commands::active_mcp_project_root(project_registry) {
            Ok(root) => root,
            Err(error) => {
                let error = SnapshotError::Unavailable(
                    error
                        .code
                        .unwrap_or_else(|| "NO_ACTIVE_PROJECT_ROOT".into()),
                );
                self.record_snapshot_error(&error).await;
                return Err(error);
            }
        };
        let parsed = match McpControlPlaneConfig::from_stored_json(document) {
            Ok(parsed) => parsed,
            Err(error) => {
                let error = SnapshotError::InvalidRegistry(error.to_string());
                self.record_snapshot_error(&error).await;
                return Err(error);
            }
        };
        self.apply_config_for_project(parsed.config, project_root)
            .await
    }

    async fn apply_config_for_project(
        &self,
        mut config: McpControlPlaneConfig,
        project_root: std::path::PathBuf,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        let _project_apply = self.project_apply_lock.lock().await;
        let Some(controller) = self.snapshot.as_ref() else {
            return self.apply_config(&config).await;
        };
        let scope_changed = self.project_scope.lock().await.as_ref() != Some(&project_root);
        if scope_changed {
            // Persisted revisions are project-scoped. Reset only the revision
            // fence when changing projects; the prior last-good snapshot stays
            // live until this replacement applies successfully.
            controller.reset_revision_for_project_scope().await;
        }
        if config.revision == 0 {
            config.revision = 1;
        }
        let result = self.apply_config(&config).await;
        if result.is_ok() {
            *self.project_scope.lock().await = Some(project_root);
        }
        result
    }

    /// Stop only the gateway owned by this holder.  Repeated calls are safe and
    /// disabled/failed holders remain explicitly absent.
    pub async fn shutdown(&self) -> Result<(), String> {
        let gateway = self.gateway.lock().await.take();
        if let Some(gateway) = gateway {
            gateway.shutdown().await;
            *self.router.write().await = None;
            let mut status = self.status.write().await;
            status.availability = DesktopMcpCoreAvailability::Stopped;
            status.endpoint = None;
            status.diagnostic = None;
            log::info!(
                target: "se_manager::mcp_core::desktop",
                "operation=mcp_desktop_runtime state=stopped stable_code=OK"
            );
        }
        Ok(())
    }
}

async fn wait_until_ready(gateway: &McpHttpGateway) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match gateway.status().await.state {
            McpReadiness::Ready | McpReadiness::Degraded => return Ok(()),
            McpReadiness::Stopped | McpReadiness::Failed => {
                return Err("MCP Core gateway stopped before becoming ready".into())
            }
            McpReadiness::Starting | McpReadiness::Stopping => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("MCP Core gateway readiness timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Secure resolver for desktop canonical MCP configuration.
///
/// `NamedSecret::reference` is a key owned by the existing secure-storage
/// boundary. Inline values are compatibility data in the project document and
/// are returned as-is; they are never looked up as keychain keys. The resolver
/// deliberately contains no logging so neither class of secret can enter logs.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DesktopMcpSecretResolver;

impl McpSecretResolver for DesktopMcpSecretResolver {
    fn resolve(
        &self,
        _server_id: &str,
        _field: &str,
        _name: &str,
        value: &str,
    ) -> Result<String, SnapshotError> {
        // Compatibility fallback for callers that only have a classified token.
        // Desktop canonical application uses `resolve_named` below.
        Ok(value.to_owned())
    }

    fn resolve_named(
        &self,
        server_id: &str,
        field: &str,
        secret: &NamedSecret,
    ) -> Result<String, SnapshotError> {
        if let Some(reference) = secret
            .reference
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return match crate::keyring_get(reference) {
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(SnapshotError::SecretResolution {
                    server_id: server_id.to_owned(),
                    field: field.to_owned(),
                    message: "referenced credential is not available".into(),
                }),
                Err(_) => Err(SnapshotError::SecretResolution {
                    server_id: server_id.to_owned(),
                    field: field.to_owned(),
                    message: "secure credential retrieval failed".into(),
                }),
            };
        }
        secret
            .value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| SnapshotError::SecretResolution {
                server_id: server_id.to_owned(),
                field: field.to_owned(),
                message: "secret has neither an inline value nor a reference".into(),
            })
    }
}

fn snapshot_error_code(error: &SnapshotError) -> &'static str {
    match error {
        SnapshotError::InvalidRegistry(_) | SnapshotError::InvalidSnapshot(_) => {
            "MCP_SNAPSHOT_INVALID"
        }
        SnapshotError::SecretResolution { .. } => "MCP_SNAPSHOT_SECRET_FAILED",
        SnapshotError::StaleRevision { .. } => "MCP_SNAPSHOT_STALE",
        SnapshotError::ApplyFailed(_) => "MCP_SNAPSHOT_APPLY_FAILED",
        SnapshotError::Unavailable(_) => "MCP_SNAPSHOT_NOT_APPLIED",
    }
}

fn snapshot_diagnostic(error: &SnapshotError) -> McpDiagnostic {
    match error {
        SnapshotError::InvalidRegistry(_) => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_INVALID",
            "canonical MCP document is invalid",
            false,
        ),
        SnapshotError::SecretResolution { .. } => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_SECRET_FAILED",
            "MCP secret resolution failed",
            true,
        ),
        SnapshotError::InvalidSnapshot(_) => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_INVALID",
            "MCP snapshot validation failed",
            false,
        ),
        SnapshotError::StaleRevision {
            accepted,
            requested,
        } => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_STALE",
            format!("MCP snapshot revision {requested} is not newer than {accepted}"),
            false,
        ),
        SnapshotError::ApplyFailed(_) => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_APPLY_FAILED",
            "MCP Core rejected the snapshot; the last-known-good snapshot is retained",
            true,
        ),
        SnapshotError::Unavailable(_) => McpDiagnostic::redacted(
            "MCP_SNAPSHOT_NOT_APPLIED",
            "desktop MCP Core snapshot controller is unavailable",
            false,
        ),
    }
}

fn env_truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{self, CredentialBackend, CredentialError};
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct TestCredentialBackend {
        values: StdMutex<BTreeMap<String, String>>,
        reads: StdMutex<Vec<String>>,
    }

    impl CredentialBackend for TestCredentialBackend {
        fn get(&self, _service: &str, key: &str) -> Result<Option<String>, CredentialError> {
            self.reads.lock().unwrap().push(key.to_owned());
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, _service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, _service: &str, key: &str) -> Result<(), CredentialError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn gateway_config(token: &str) -> McpHttpGatewayConfig {
        McpHttpGatewayConfig {
            bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            path: "/mcp".into(),
            generation: 7,
            auth: AuthBootstrap::new(7, token).unwrap(),
            request_body_limit: 64 * 1024,
        }
    }

    #[test]
    fn opt_in_parser_requires_an_explicit_truthy_value() {
        assert!(!env_truthy(None));
        assert!(!env_truthy(Some("0")));
        assert!(!env_truthy(Some("false")));
        assert!(env_truthy(Some("1")));
        assert!(env_truthy(Some(" TRUE ")));
        assert!(env_truthy(Some("on")));
    }

    #[tokio::test]
    async fn disabled_holder_is_explicitly_absent() {
        let holder = DesktopMcpCoreRuntime::disabled("MCP_CORE_DISABLED", "opt-in required");
        let status = holder.status().await;
        assert_eq!(status.availability, DesktopMcpCoreAvailability::Disabled);
        assert!(status.endpoint.is_none());
        assert_eq!(
            status.diagnostic.as_ref().unwrap().code,
            "MCP_CORE_DISABLED"
        );
        assert!(status.snapshot_revision.is_none());
        assert_eq!(
            status.snapshot_diagnostic.as_ref().unwrap().code,
            "MCP_SNAPSHOT_NOT_APPLIED"
        );
        assert!(holder.endpoint_auth().await.is_none());
        holder.shutdown().await.unwrap();
        assert_eq!(
            holder.status().await.availability,
            DesktopMcpCoreAvailability::Disabled
        );
    }

    #[tokio::test]
    async fn invalid_bind_is_explicitly_failed_without_router_contract() {
        let state = tempdir().unwrap();
        let memory = Arc::new(MemoryIndexService::new(state.path().to_path_buf()));
        let holder = DesktopMcpCoreRuntime::start_with_config(
            memory,
            McpHttpGatewayConfig {
                bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                ..gateway_config("desktop-token")
            },
        )
        .await;
        let status = holder.status().await;
        assert_eq!(status.availability, DesktopMcpCoreAvailability::Failed);
        assert!(status.endpoint.is_none());
        assert_eq!(
            status.diagnostic.as_ref().unwrap().code,
            "MCP_CORE_BIND_FAILED"
        );
        assert!(holder.endpoint_auth().await.is_none());
        holder.shutdown().await.unwrap();
    }

    #[test]
    fn resolver_distinguishes_inline_values_from_keyring_references() {
        let backend = Arc::new(TestCredentialBackend::default());
        backend
            .values
            .lock()
            .unwrap()
            .insert("mcp/remote/token".into(), "keyring-secret".into());
        let _guard = credentials::override_backend(backend.clone());
        let resolver = DesktopMcpSecretResolver;

        let inline = NamedSecret::inline("Authorization", "inline-secret");
        assert_eq!(
            resolver
                .resolve_named("remote", "headers", &inline)
                .unwrap(),
            "inline-secret"
        );
        assert!(backend.reads.lock().unwrap().is_empty());

        let reference = NamedSecret::by_ref("Authorization", "mcp/remote/token");
        assert_eq!(
            resolver
                .resolve_named("remote", "headers", &reference)
                .unwrap(),
            "keyring-secret"
        );
        assert_eq!(
            backend.reads.lock().unwrap().as_slice(),
            &["mcp/remote/token".to_string()]
        );
    }

    #[tokio::test]
    async fn startup_runtime_applies_a_canonical_document() {
        let state = tempdir().unwrap();
        let memory = Arc::new(MemoryIndexService::new(state.path().to_path_buf()));
        let holder =
            DesktopMcpCoreRuntime::start_with_config(memory, gateway_config("desktop-token")).await;
        let document = serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": []
        });
        let receipt = holder.apply_document(&document).await.unwrap();
        assert_eq!(receipt.revision, 1);
        let status = holder.status().await;
        assert_eq!(status.snapshot_revision, Some(1));
        assert!(status.snapshot_diagnostic.is_none());
        holder.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_stale_and_secret_failures_retain_last_good_status() {
        let state = tempdir().unwrap();
        let memory = Arc::new(MemoryIndexService::new(state.path().to_path_buf()));
        let holder =
            DesktopMcpCoreRuntime::start_with_config(memory, gateway_config("desktop-token")).await;
        let first = serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": []
        });
        holder.apply_document(&first).await.unwrap();

        let invalid = serde_json::json!({
            "schemaVersion": 1,
            "revision": 2,
            "upstreams": [{"id": "bad", "type": "http", "url": "not-a-url"}]
        });
        assert!(holder.apply_document(&invalid).await.is_err());
        let status = holder.status().await;
        assert_eq!(status.snapshot_revision, Some(1));
        assert_eq!(
            status.snapshot_diagnostic.as_ref().unwrap().code,
            "MCP_SNAPSHOT_INVALID"
        );

        assert!(holder.apply_document(&first).await.is_err());
        let status = holder.status().await;
        assert_eq!(status.snapshot_revision, Some(1));
        assert_eq!(
            status.snapshot_diagnostic.as_ref().unwrap().code,
            "MCP_SNAPSHOT_STALE"
        );

        let secret_failure = serde_json::json!({
            "schemaVersion": 1,
            "revision": 2,
            "upstreams": [{
                "id": "remote",
                "type": "http",
                "url": "https://example.test/mcp",
                "headers": [{"name": "Authorization", "ref": "missing/mcp/token"}]
            }]
        });
        assert!(holder.apply_document(&secret_failure).await.is_err());
        let status = holder.status().await;
        assert_eq!(status.snapshot_revision, Some(1));
        assert_eq!(
            status.snapshot_diagnostic.as_ref().unwrap().code,
            "MCP_SNAPSHOT_SECRET_FAILED"
        );
        let redacted = serde_json::to_string(&status).unwrap();
        assert!(!redacted.contains("missing/mcp/token"));
        holder.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn configured_gateway_has_router_contract_and_shuts_down_cleanly() {
        let state = tempdir().unwrap();
        let memory = Arc::new(MemoryIndexService::new(state.path().to_path_buf()));
        let holder =
            DesktopMcpCoreRuntime::start_with_config(memory, gateway_config("desktop-token")).await;
        let status = holder.status().await;
        assert_eq!(status.availability, DesktopMcpCoreAvailability::Available);
        let (endpoint, auth) = holder.endpoint_auth().await.expect("router contract");
        assert_eq!(endpoint.port, status.endpoint.as_ref().unwrap().port);
        assert_eq!(endpoint.generation, auth.generation);
        assert_eq!(auth.bearer_token(), "desktop-token");
        assert!(status.snapshot_revision.is_none());
        assert_eq!(
            status.snapshot_diagnostic.as_ref().unwrap().code,
            "MCP_SNAPSHOT_DEFERRED"
        );
        assert_eq!(
            holder.status().await.availability,
            DesktopMcpCoreAvailability::Available
        );

        let url = format!("http://127.0.0.1:{}/health", endpoint.port);
        holder.shutdown().await.unwrap();
        assert_eq!(
            holder.status().await.availability,
            DesktopMcpCoreAvailability::Stopped
        );
        assert!(reqwest::get(url).await.is_err());
    }
}
