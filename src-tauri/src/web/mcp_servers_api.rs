use std::path::{Path, PathBuf};

use axum::{extract::State, Json};
use serde_json::Value;
use tokio::fs;

use crate::acp::atomic_file;
use crate::mcp_core::{prepare_write, ConfigError, McpControlPlaneConfig, McpControlPlaneStatus};

use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

pub(crate) const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
pub(crate) const FILE_NAME: &str = "mcp-servers.json";
pub(crate) const CONTROL_PLANE_LOG_TARGET: &str = "se_manager::mcp_core::control_plane";

/// Resolve `{project_root}/{workspace dir}/mcp-servers.json` — the path this
/// app *writes*.
///
/// Shared by the web `PUT /mcp-servers` handler and the desktop
/// `remote_sync_mcp_registry` Tauri command so both surfaces write the same
/// canonical control-plane document the web route reads.
///
/// The brand seam is read here, on the request thread. Both handlers below call
/// this before any `spawn_blocking` and move the resolved path into the
/// closure, never the seam (FORBID-07).
pub(crate) fn registry_path(project_root: &Path) -> PathBuf {
    project_root
        .join(crate::brand::canonical().workspace_dir)
        .join(FILE_NAME)
}

/// Resolve the registry to *read*: the current workspace directory, falling
/// back to the one a pre-rename build wrote.
///
/// M-08 is the root the app does not own — these directories are scattered
/// through every repository the user has ever opened, the app holds no
/// inventory of them, and moving one would dirty a git working tree and
/// invalidate the worktree paths already recorded in conversation history. So
/// the legacy file is read in place and never rewritten (FORBID-04): after the
/// rename a registry edit lands in the new directory and the old file is left
/// exactly as it was.
fn registry_read_path(project_root: &Path) -> PathBuf {
    let canonical = registry_path(project_root);
    if canonical.exists() {
        return canonical;
    }
    let legacy = project_root
        .join(crate::brand::LEGACY.workspace_dir)
        .join(FILE_NAME);
    if legacy.exists() {
        return legacy;
    }
    canonical
}

#[derive(Debug)]
pub(crate) enum RegistryDocumentError {
    Invalid(String),
    TooLarge,
    RevisionConflict { accepted: u64, requested: u64 },
    Io(String),
}

impl RegistryDocumentError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "MCP_REGISTRY_INVALID",
            Self::TooLarge => "MCP_REGISTRY_TOO_LARGE",
            Self::RevisionConflict { .. } => "MCP_REGISTRY_REVISION_CONFLICT",
            Self::Io(_) => "MCP_REGISTRY_WRITE_ERROR",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::Invalid(message) => message.clone(),
            Self::TooLarge => "MCP registry exceeds the 1 MiB limit".into(),
            Self::RevisionConflict {
                accepted,
                requested,
            } => format!("MCP config revision {requested} is not newer than {accepted}"),
            Self::Io(_) => "Failed to persist MCP registry".into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            Self::Io(detail) => Some(detail),
            _ => None,
        }
    }
}

impl From<ConfigError> for RegistryDocumentError {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::RevisionConflict {
                accepted,
                requested,
            } => Self::RevisionConflict {
                accepted,
                requested,
            },
            other => Self::Invalid(other.to_string()),
        }
    }
}

/// Load the on-disk document without rewriting it.
///
/// Missing files are `Ok(None)`. Legacy arrays and canonical objects are both
/// accepted and returned as the in-memory canonical config.
pub(crate) fn read_document(
    project_root: &Path,
) -> Result<Option<McpControlPlaneConfig>, RegistryDocumentError> {
    let path = registry_read_path(project_root);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.len() > MAX_REGISTRY_BYTES => Err(RegistryDocumentError::TooLarge),
        Ok(bytes) => parse_document_bytes(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RegistryDocumentError::Io(error.to_string())),
    }
}

fn parse_document_bytes(bytes: &[u8]) -> Result<McpControlPlaneConfig, RegistryDocumentError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| RegistryDocumentError::Invalid("MCP registry file is malformed".into()))?;
    McpControlPlaneConfig::from_stored_json(&value)
        .map(|parsed| parsed.config)
        .map_err(RegistryDocumentError::from)
}

/// Canonicalize `incoming` against the existing document and atomically write
/// it to the canonical project path. Never writes the legacy `.termul` file.
pub(crate) fn write_document(
    project_root: &Path,
    incoming: &Value,
) -> Result<McpControlPlaneConfig, RegistryDocumentError> {
    let existing = read_document(project_root)?;
    let config = prepare_write(incoming, existing.as_ref())?;
    let value = config
        .to_canonical_json()
        .map_err(RegistryDocumentError::from)?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| RegistryDocumentError::Invalid("MCP registry is not serializable".into()))?;
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err(RegistryDocumentError::TooLarge);
    }
    let path = registry_path(project_root);
    atomic_file::replace(&path, &bytes)
        .map_err(|error| RegistryDocumentError::Io(error.to_string()))?;
    log_control_plane_put(&config, "OK");
    Ok(config)
}

fn log_control_plane_put(config: &McpControlPlaneConfig, stable_code: &str) {
    let server_ids = config
        .upstreams
        .iter()
        .map(|upstream| upstream.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let status = config
        .upstreams
        .iter()
        .map(|upstream| {
            if upstream.enabled {
                "enabled"
            } else {
                "disabled"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    log::info!(
        target: CONTROL_PLANE_LOG_TARGET,
        "operation=mcp_put_config revision={} server_ids={} status={} upstreams={} stable_code={}",
        config.revision,
        server_ids,
        status,
        config.upstreams.len(),
        stable_code
    );
}

fn log_control_plane_put_failure(stable_code: &str) {
    log::error!(
        target: CONTROL_PLANE_LOG_TARGET,
        "operation=mcp_put_config stable_code={stable_code}"
    );
}

pub async fn get(State(state): State<AppState>) -> Json<IpcBody<Value>> {
    // CAP-1: lock-read the live project_root so the MCP registry file
    // (<workspace dir>/mcp-servers.json) follows the active project on a switch.
    let project_root = state.project_root.read().clone();
    let path = registry_read_path(&project_root);
    match fs::read(&path).await {
        Ok(bytes) if bytes.len() > MAX_REGISTRY_BYTES => Json(IpcBody::err(
            "MCP registry exceeds the 1 MiB limit",
            "MCP_REGISTRY_TOO_LARGE",
        )),
        Ok(bytes) => match parse_document_bytes(&bytes) {
            Ok(config) => match config.to_canonical_json() {
                Ok(value) => {
                    tracing::info!(
                        entries = config.upstreams.len(),
                        revision = config.revision,
                        "loaded MCP registry"
                    );
                    Json(IpcBody::ok(value))
                }
                Err(error) => Json(IpcBody::err(error.to_string(), "MCP_REGISTRY_INVALID")),
            },
            Err(error) => {
                tracing::warn!(error = %error.message(), "MCP registry file is malformed");
                Json(IpcBody::err(error.message(), error.code()))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match McpControlPlaneConfig::empty().to_canonical_json() {
                Ok(value) => Json(IpcBody::ok(value)),
                Err(error) => Json(IpcBody::err(error.to_string(), "MCP_REGISTRY_INVALID")),
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to read MCP registry");
            Json(IpcBody::err(
                "Failed to read MCP registry",
                "MCP_REGISTRY_READ_ERROR",
            ))
        }
    }
}

pub async fn put(State(state): State<AppState>, Json(value): Json<Value>) -> Json<IpcBody<()>> {
    match persist(&state, value).await {
        Ok(_) => Json(IpcBody::ok(())),
        Err(error) => persist_error(error),
    }
}

/// Canonical control-plane write: same persistence as `PUT /mcp-servers`, but
/// returns the written document so clients can adopt the new revision.
pub async fn put_config(
    State(state): State<AppState>,
    Json(value): Json<Value>,
) -> Json<IpcBody<Value>> {
    match persist(&state, value).await {
        Ok(config) => match config.to_canonical_json() {
            Ok(document) => Json(IpcBody::ok(document)),
            Err(error) => Json(IpcBody::err(error.to_string(), "MCP_REGISTRY_INVALID")),
        },
        Err(error) => persist_error(error),
    }
}

/// Redacted operator status for the canonical project document.
pub async fn status(State(state): State<AppState>) -> Json<IpcBody<McpControlPlaneStatus>> {
    let project_root = state.project_root.read().clone();
    match tokio::task::spawn_blocking(move || read_document(&project_root)).await {
        Ok(Ok(Some(config))) => Json(IpcBody::ok(config.to_status())),
        Ok(Ok(None)) => Json(IpcBody::ok(McpControlPlaneConfig::empty().to_status())),
        Ok(Err(error)) => {
            tracing::warn!(error = %error.message(), "MCP status read failed");
            Json(IpcBody::err(error.message(), error.code()))
        }
        Err(error) => {
            tracing::error!(error = %error, "MCP status read task failed");
            Json(IpcBody::err(
                "Failed to read MCP registry",
                "MCP_REGISTRY_READ_ERROR",
            ))
        }
    }
}

async fn persist(
    state: &AppState,
    value: Value,
) -> Result<McpControlPlaneConfig, RegistryDocumentError> {
    // CAP-1: lock-read the live project_root (follows the active project).
    let project_root = state.project_root.read().clone();
    let write_root = project_root.clone();
    let write_result =
        tokio::task::spawn_blocking(move || write_document(&write_root, &value)).await;
    match write_result {
        Ok(Ok(config)) => Ok(config),
        Ok(Err(error)) => {
            log_control_plane_put_failure(error.code());
            Err(error)
        }
        Err(_) => {
            log_control_plane_put_failure("MCP_REGISTRY_WRITE_ERROR");
            Err(RegistryDocumentError::Io(
                "MCP registry write task failed".into(),
            ))
        }
    }
}

fn persist_error<T>(error: RegistryDocumentError) -> Json<IpcBody<T>> {
    Json(IpcBody::err(error.message(), error.code()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpManager;
    use crate::web::project_registry::ProjectRegistry;
    use crate::web::sink::WsRelaySink;
    use crate::web::test_pty_manager;
    use crate::web::ws::HistoryMode;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app(dir: PathBuf) -> axum::Router {
        let pty = test_pty_manager();
        let relay = Arc::new(WsRelaySink::new());
        let state = AppState {
            acp: crate::core::AcpWebHostHandle::in_process(
                Arc::new(AcpManager::new(vec![])),
                Arc::clone(&relay),
            ),
            terminal: crate::core::TerminalServiceHandle::in_process(Arc::clone(&pty)),
            terminal_events: pty.terminal_events(),
            cwd_tracker: pty.cwd_tracker(),
            git_tracker: pty.git_tracker(),
            exit_code_tracker: pty.exit_code_tracker(),
            pty,
            relay,
            registry: Arc::new(ProjectRegistry::new()),
            registry_persistence: None,
            projects_file: None,
            history_mode: HistoryMode::LiveOnly,
            conversation: None,
            conversation_creation: None,
            project_root: Arc::new(parking_lot::RwLock::new(dir)),
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            memory_index: None,
            skills_hub: None,
            store: None,
        };
        axum::Router::new()
            .route("/mcp-servers", get(super::get).put(super::put))
            .route("/mcp/config", get(super::get).put(super::put_config))
            .route("/mcp/status", get(super::status))
            .with_state(state)
    }

    #[tokio::test]
    async fn put_then_get_round_trips_registry() {
        let dir = std::env::temp_dir().join(format!("se-manager-mcp-api-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir).await;
        let app = test_app(dir.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp-servers")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"[{"id":"one","type":"stdio","name":"fs","command":"npx","enabled":true}]"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mcp-servers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["success"], true);
        assert_eq!(value["data"]["schemaVersion"], 1);
        assert_eq!(value["data"]["revision"], 1);
        assert_eq!(value["data"]["upstreams"][0]["name"], "fs");
        assert_eq!(value["data"]["upstreams"][0]["command"], "npx");
        let _ = fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn put_canonical_object_round_trips_and_rejects_stale_revision() {
        let dir = std::env::temp_dir().join(format!(
            "se-manager-mcp-api-canonical-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir).await;
        let app = test_app(dir.clone());
        let first_body = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": [{
                "id": "one",
                "type": "stdio",
                "name": "fs",
                "command": "npx",
                "enabled": true
            }]
        }))
        .unwrap();
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp-servers")
                    .header("content-type", "application/json")
                    .body(Body::from(first_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let stale_payload = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": []
        }))
        .unwrap();
        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp-servers")
                    .header("content-type", "application/json")
                    .body(Body::from(stale_payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        let stale_body = to_bytes(stale.into_body(), usize::MAX).await.unwrap();
        let stale_value: Value = serde_json::from_slice(&stale_body).unwrap();
        assert_eq!(stale_value["success"], false);
        assert_eq!(stale_value["code"], "MCP_REGISTRY_REVISION_CONFLICT");

        let next_payload = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "revision": 2,
            "upstreams": [{
                "id": "two",
                "type": "stdio",
                "name": "other",
                "command": "node"
            }]
        }))
        .unwrap();
        let next = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp-servers")
                    .header("content-type", "application/json")
                    .body(Body::from(next_payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        let next_body = to_bytes(next.into_body(), usize::MAX).await.unwrap();
        let next_value: Value = serde_json::from_slice(&next_body).unwrap();
        assert_eq!(next_value["success"], true);
        let _ = fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn put_config_returns_document_and_status_is_redacted() {
        let dir = std::env::temp_dir().join(format!(
            "se-manager-mcp-api-control-plane-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir).await;
        let app = test_app(dir.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": [{
                "id": "secret",
                "type": "stdio",
                "name": "Files",
                "command": "npx",
                "env": [{"name": "TOKEN", "value": "super-secret"}],
                "enabled": true
            }]
        }))
        .unwrap();
        let put = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp/config")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
        let put_body = to_bytes(put.into_body(), usize::MAX).await.unwrap();
        let put_value: Value = serde_json::from_slice(&put_body).unwrap();
        assert_eq!(put_value["success"], true);
        assert_eq!(put_value["data"]["revision"], 1);
        assert_eq!(put_value["data"]["upstreams"][0]["name"], "Files");

        let status = app
            .oneshot(
                Request::builder()
                    .uri("/mcp/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status_body = to_bytes(status.into_body(), usize::MAX).await.unwrap();
        let status_value: Value = serde_json::from_slice(&status_body).unwrap();
        assert_eq!(status_value["success"], true);
        assert_eq!(status_value["data"]["revision"], 1);
        assert_eq!(status_value["data"]["upstreams"][0]["type"], "stdio");
        assert_eq!(status_value["data"]["upstreams"][0]["name"], "Files");
        let encoded = status_body;
        assert!(!String::from_utf8_lossy(&encoded).contains("super-secret"));
        let _ = fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn put_config_boundary_log_omits_secrets_headers_env_args_and_paths() {
        let _guard = crate::web::auth::test_tracing::lock().await;
        let dir = std::env::temp_dir().join(format!(
            "se-manager-mcp-api-boundary-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir).await;
        let app = test_app(dir.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "revision": 1,
            "upstreams": [{
                "id": "files",
                "type": "stdio",
                "name": "Files",
                "command": "/tmp/secret-bin",
                "args": ["--token"],
                "env": [{"name": "TOKEN", "value": "super-secret"}],
                "enabled": true
            }, {
                "id": "remote",
                "type": "http",
                "name": "Remote",
                "url": "https://example.test/mcp",
                "headers": [{"name": "Authorization", "value": "Bearer leaked-header"}],
                "enabled": false
            }]
        }))
        .unwrap();
        let put = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp/config")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);

        let output = crate::web::auth::test_tracing::messages(CONTROL_PLANE_LOG_TARGET).join("\n");
        let path = registry_path(&dir).display().to_string();
        for required in [
            "operation=mcp_put_config",
            "revision=1",
            "server_ids=files,remote",
            "status=enabled,disabled",
            "stable_code=OK",
        ] {
            assert!(output.contains(required), "missing {required}: {output}");
        }
        for leaked in [
            "super-secret",
            "/tmp/secret-bin",
            "--token",
            "TOKEN",
            "leaked-header",
            "Authorization",
            "https://example.test/mcp",
            path.as_str(),
        ] {
            assert!(!output.contains(leaked), "leaked {leaked}: {output}");
        }
        let _ = fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn rejects_non_array_payload() {
        let dir =
            std::env::temp_dir().join(format!("se-manager-mcp-api-invalid-{}", std::process::id()));
        let app = test_app(dir.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/mcp-servers")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["success"], false);
        assert_eq!(value["code"], "MCP_REGISTRY_INVALID");
        let _ = fs::remove_dir_all(dir).await;
    }
}
