//! Private loopback bridge from the independent MCP gateway to its Se host.
//! The bridge token is distinct from the public MCP bearer. The descriptor is
//! host-private and refreshed on host restart; it never appears in MCP output.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::host_capabilities::{
    HostCapabilityBackend, HostCapabilityError, HOST_REQUEST_LIMIT, HOST_RESPONSE_LIMIT,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostBridgeEndpoint {
    pub port: u16,
    token: String,
}

impl std::fmt::Debug for HostBridgeEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBridgeEndpoint")
            .field("port", &self.port)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

pub fn descriptor_path(gateway_settings: &Path) -> PathBuf {
    gateway_settings.with_extension("host.json")
}

impl HostBridgeEndpoint {
    pub fn publish(&self, path: &Path) -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing descriptor parent"))?;
        std::fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".mcp-host-{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            std::fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    fn load(path: &Path) -> Result<Self, HostCapabilityError> {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| HostCapabilityError::unavailable())?;
        if !metadata.file_type().is_file() || metadata.len() > 4096 {
            return Err(HostCapabilityError::unavailable());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(HostCapabilityError::unavailable());
            }
        }
        let bytes = std::fs::read(path).map_err(|_| HostCapabilityError::unavailable())?;
        let endpoint: Self =
            serde_json::from_slice(&bytes).map_err(|_| HostCapabilityError::unavailable())?;
        if endpoint.port == 0 || endpoint.token.len() < 32 {
            return Err(HostCapabilityError::unavailable());
        }
        Ok(endpoint)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallRequest {
    request_id: String,
    provider: String,
    tool: String,
    arguments: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum CallResponse {
    Ok { value: Value },
    Error { error: HostCapabilityError },
}

#[derive(Clone)]
struct BridgeState {
    token: String,
    backend: Arc<dyn HostCapabilityBackend>,
    requests: Arc<parking_lot::Mutex<HashMap<String, BridgeRequest>>>,
}

struct BridgeRequest {
    token: CancellationToken,
    expires: Instant,
    started: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelRequest {
    request_id: String,
}

fn authorize(state: &BridgeState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if supplied.is_some_and(|v| token_matches(&state.token, v)) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn request_token(
    state: &BridgeState,
    id: &str,
    start: bool,
) -> Result<CancellationToken, StatusCode> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut requests = state.requests.lock();
    requests.retain(|_, request| request.expires > Instant::now());
    if !requests.contains_key(id) && requests.len() >= 1024 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let request = requests.entry(id.into()).or_insert_with(|| BridgeRequest {
        token: CancellationToken::new(),
        expires: Instant::now() + Duration::from_secs(135),
        started: false,
    });
    if start {
        if request.started {
            return Err(StatusCode::CONFLICT);
        }
        request.started = true;
    }
    Ok(request.token.clone())
}

async fn handle_cancel(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(request): Json<CancelRequest>,
) -> Result<StatusCode, StatusCode> {
    authorize(&state, &headers)?;
    // A bounded tombstone also covers cancel racing ahead of call registration.
    request_token(&state, &request.request_id, false)?.cancel();
    Ok(StatusCode::NO_CONTENT)
}

struct RequestGuard {
    state: BridgeState,
    id: String,
    token: CancellationToken,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.token.cancel();
        self.state.requests.lock().remove(&self.id);
    }
}

pub struct HostBridge {
    pub endpoint: HostBridgeEndpoint,
    stop: CancellationToken,
}

impl HostBridge {
    pub async fn bind(backend: Arc<dyn HostCapabilityBackend>) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = HostBridgeEndpoint {
            port: listener.local_addr()?.port(),
            token: uuid::Uuid::new_v4().simple().to_string(),
        };
        let state = BridgeState {
            token: endpoint.token.clone(),
            backend,
            requests: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        };
        let router = Router::new()
            .route("/v1/call", post(handle_call))
            .route("/v1/cancel", post(handle_cancel))
            .layer(DefaultBodyLimit::max(HOST_REQUEST_LIMIT))
            .with_state(state);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        tokio::spawn(async move {
            if axum::serve(listener, router)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
                .is_err()
            {
                log::error!(target: "se_manager::mcp_core::host", "operation=host_bridge_serve stable_code=HOST_BRIDGE_FAILED");
            }
        });
        log::info!(target: "se_manager::mcp_core::host", "operation=host_bridge_bind stable_code=READY");
        Ok(Self { endpoint, stop })
    }
}

impl Drop for HostBridge {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn token_matches(expected: &str, supplied: &str) -> bool {
    if expected.len() != supplied.len() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0u8, |different, (a, b)| different | (a ^ b))
        == 0
}

async fn handle_call(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(call): Json<CallRequest>,
) -> Result<Json<CallResponse>, StatusCode> {
    authorize(&state, &headers)?;
    if !matches!(call.provider.as_str(), "workspace" | "ssh") {
        return Err(StatusCode::NOT_FOUND);
    }
    let cancellation = request_token(&state, &call.request_id, true)?;
    let _cancel_on_drop = RequestGuard {
        state: state.clone(),
        id: call.request_id,
        token: cancellation.clone(),
    };
    if cancellation.is_cancelled() {
        return Err(StatusCode::CONFLICT);
    }
    let result = tokio::time::timeout(
        Duration::from_secs(125),
        state
            .backend
            .call(&call.provider, &call.tool, call.arguments, cancellation),
    )
    .await;
    let response = match result {
        Ok(Ok(value)) => CallResponse::Ok { value },
        Ok(Err(error)) => CallResponse::Error { error },
        Err(_) => CallResponse::Error {
            error: HostCapabilityError::new(
                "HOST_TIMEOUT",
                "Host operation timed out; remote termination is not confirmed",
            ),
        },
    };
    let size = serde_json::to_vec(&response)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .len();
    if size > HOST_RESPONSE_LIMIT {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(Json(response))
}

/// Reload discovery on every call so an existing gateway can reconnect after
/// the desktop restarts. This reads only the bridge descriptor, not credentials.
pub struct RemoteHostBackend {
    descriptor: PathBuf,
    client: reqwest::Client,
}

struct RemoteCancelGuard {
    client: reqwest::Client,
    endpoint: HostBridgeEndpoint,
    id: Option<String>,
}
impl Drop for RemoteCancelGuard {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        runtime.spawn(async move {
            // Best effort delivery is not a claim of remote termination.
            let _ = client
                .post(format!("http://127.0.0.1:{}/v1/cancel", endpoint.port))
                .bearer_auth(endpoint.token)
                .timeout(Duration::from_secs(2))
                .json(&CancelRequest { request_id: id })
                .send()
                .await;
        });
    }
}

impl RemoteHostBackend {
    pub fn new(descriptor: PathBuf) -> Result<Self, HostCapabilityError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(130))
            .build()
            .map_err(|_| HostCapabilityError::unavailable())?;
        Ok(Self { descriptor, client })
    }
}

#[async_trait]
impl HostCapabilityBackend for RemoteHostBackend {
    async fn call(
        &self,
        provider: &str,
        tool: &str,
        arguments: Map<String, Value>,
        cancellation: CancellationToken,
    ) -> Result<Value, HostCapabilityError> {
        let endpoint = HostBridgeEndpoint::load(&self.descriptor)?;
        let request = CallRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            provider: provider.into(),
            tool: tool.into(),
            arguments,
        };
        if serde_json::to_vec(&request)
            .map_err(|_| HostCapabilityError::new("INVALID_ARGUMENTS", "Invalid request"))?
            .len()
            > HOST_REQUEST_LIMIT
        {
            return Err(HostCapabilityError::new(
                "REQUEST_TOO_LARGE",
                "Host request exceeds its limit",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(HostCapabilityError::new(
                "CANCELLED",
                "Request cancelled before dispatch",
            ));
        }
        let mut cancel_guard = RemoteCancelGuard {
            client: self.client.clone(),
            endpoint: endpoint.clone(),
            id: Some(request.request_id.clone()),
        };
        let operation = async {
            let response = self
                .client
                .post(format!("http://127.0.0.1:{}/v1/call", endpoint.port))
                .bearer_auth(&endpoint.token)
                .json(&request)
                .send()
                .await
                .map_err(|_| HostCapabilityError::unavailable())?;
            if !response.status().is_success() {
                return Err(HostCapabilityError::unavailable());
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| HostCapabilityError::unavailable())?;
                if chunk.len() > HOST_RESPONSE_LIMIT.saturating_sub(bytes.len()) {
                    return Err(HostCapabilityError::new(
                        "RESPONSE_TOO_LARGE",
                        "Host response exceeds its limit",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let response =
                serde_json::from_slice(&bytes).map_err(|_| HostCapabilityError::unavailable())?;
            cancel_guard.id = None;
            match response {
                CallResponse::Ok { value } => Ok(value),
                CallResponse::Error { error } => Err(error),
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(HostCapabilityError::new("CANCELLED", "Host operation cancelled; remote termination is not confirmed")),
            result = operation => result,
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(AtomicUsize);

    struct CancellableFixture {
        entered: tokio::sync::Notify,
        cancelled: Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl HostCapabilityBackend for CancellableFixture {
        async fn call(
            &self,
            _: &str,
            _: &str,
            _: Map<String, Value>,
            cancellation: CancellationToken,
        ) -> Result<Value, HostCapabilityError> {
            // A disconnected HTTP handler can be dropped before this future
            // polls again. Observe its actual token outside the dropped future,
            // as the blocking SSH worker does, rather than requiring a response.
            let observed = cancellation.clone();
            let notified = self.cancelled.clone();
            tokio::spawn(async move {
                observed.cancelled().await;
                notified.notify_one();
            });
            self.entered.notify_one();
            cancellation.cancelled().await;
            Err(HostCapabilityError::new("CANCELLED", "cancelled"))
        }
    }

    #[tokio::test]
    async fn gateway_cancellation_and_future_drop_signal_host_backend() {
        for abort in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let fixture = Arc::new(CancellableFixture {
                entered: tokio::sync::Notify::new(),
                cancelled: Arc::new(tokio::sync::Notify::new()),
            });
            let bridge = HostBridge::bind(fixture.clone()).await.unwrap();
            let path = temp.path().join("host.json");
            bridge.endpoint.publish(&path).unwrap();
            let cancellation = CancellationToken::new();
            let task_cancel = cancellation.clone();
            let task = tokio::spawn(async move {
                RemoteHostBackend::new(path)
                    .unwrap()
                    .call("ssh", "execute", Map::new(), task_cancel)
                    .await
            });
            tokio::time::timeout(Duration::from_secs(3), fixture.entered.notified())
                .await
                .unwrap();
            if abort {
                task.abort();
            } else {
                cancellation.cancel();
            }
            tokio::time::timeout(Duration::from_secs(3), fixture.cancelled.notified())
                .await
                .unwrap();
            if !abort {
                assert_eq!(task.await.unwrap().unwrap_err().code, "CANCELLED");
            }
        }
    }

    #[tokio::test]
    async fn authenticated_cancel_before_dispatch_prevents_target_io() {
        let fixture = Arc::new(Fixture(AtomicUsize::new(0)));
        let bridge = HostBridge::bind(fixture.clone()).await.unwrap();
        let client = reqwest::Client::new();
        let id = uuid::Uuid::new_v4().to_string();
        let url = format!("http://127.0.0.1:{}", bridge.endpoint.port);
        let denied = client
            .post(format!("{url}/v1/cancel"))
            .json(&CancelRequest {
                request_id: id.clone(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let cancelled = client
            .post(format!("{url}/v1/cancel"))
            .bearer_auth(&bridge.endpoint.token)
            .json(&CancelRequest {
                request_id: id.clone(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::NO_CONTENT);
        let response = client
            .post(format!("{url}/v1/call"))
            .bearer_auth(&bridge.endpoint.token)
            .json(&CallRequest {
                request_id: id,
                provider: "ssh".into(),
                tool: "execute".into(),
                arguments: Map::new(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(fixture.0.load(Ordering::SeqCst), 0);
    }

    #[async_trait]
    impl HostCapabilityBackend for Fixture {
        async fn call(
            &self,
            provider: &str,
            tool: &str,
            _arguments: Map<String, Value>,
            _cancellation: CancellationToken,
        ) -> Result<Value, HostCapabilityError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if tool == "denied" {
                return Err(HostCapabilityError::new(
                    "PERMISSION_DENIED",
                    "Operation is not granted",
                ));
            }
            Ok(serde_json::json!({"provider":provider,"tool":tool}))
        }
    }

    #[tokio::test]
    async fn bridge_requires_its_private_token_before_invoking_host() {
        let backend = Arc::new(Fixture(AtomicUsize::new(0)));
        let bridge = HostBridge::bind(backend.clone()).await.unwrap();
        let response = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/v1/call", bridge.endpoint.port))
            .bearer_auth("not-the-host-token")
            .json(&serde_json::json!({"request_id":uuid::Uuid::new_v4().to_string(),"provider":"ssh","tool":"execute","arguments":{}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
        assert!(!format!("{:?}", bridge.endpoint).contains(&bridge.endpoint.token));
    }

    #[tokio::test]
    async fn existing_gateway_reloads_endpoint_when_host_restarts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gateway.host.json");
        let first = Arc::new(Fixture(AtomicUsize::new(0)));
        let bridge = HostBridge::bind(first.clone()).await.unwrap();
        bridge.endpoint.publish(&path).unwrap();
        let client = RemoteHostBackend::new(path.clone()).unwrap();
        let result = client
            .call(
                "workspace",
                "project_list",
                Map::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result["tool"], "project_list");
        assert_eq!(first.0.load(Ordering::SeqCst), 1);
        let second = Arc::new(Fixture(AtomicUsize::new(0)));
        let replacement = HostBridge::bind(second.clone()).await.unwrap();
        replacement.endpoint.publish(&path).unwrap();
        drop(bridge);
        client
            .call(
                "workspace",
                "group_list",
                Map::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(second.0.load(Ordering::SeqCst), 1);
        assert_eq!(first.0.load(Ordering::SeqCst), 1);
        let error = client
            .call("ssh", "denied", Map::new(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, "PERMISSION_DENIED");
    }

    #[tokio::test]
    async fn missing_host_is_explicitly_unavailable_not_an_empty_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let client = RemoteHostBackend::new(temp.path().join("absent.json")).unwrap();
        let error = client
            .call(
                "workspace",
                "project_list",
                Map::new(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "HOST_UNAVAILABLE");
        assert!(!error.message.contains(temp.path().to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_is_private_and_rejects_symlinks_and_world_readable_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("host.json");
        let endpoint = HostBridgeEndpoint {
            port: 12345,
            token: "x".repeat(32),
        };
        endpoint.publish(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(HostBridgeEndpoint::load(&path).is_ok());
        let link = temp.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(HostBridgeEndpoint::load(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(HostBridgeEndpoint::load(&path).is_err());
    }
}
