//! Authenticated loopback-only Streamable HTTP adapter for MCP Core.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, GetPromptRequestParams, GetPromptResponse,
        ListPromptsResult, ListResourcesResult, ListToolsResult, ReadResourceRequestParams,
        ReadResourceResponse, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData, RoleServer, ServerHandler,
};
use tokio::{net::TcpListener, sync::RwLock, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{
    AuthBootstrap, McpCore, McpDomainError, McpEndpointDescriptor, McpReadiness, McpReadinessStatus,
};

const HTTP_BOUNDARY_LOG_TARGET: &str = "se_manager::mcp_core::http";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayMode {
    Entry,
    Aggregator,
}

#[derive(Debug, Clone)]
pub struct McpHttpGatewayConfig {
    pub bind_address: IpAddr,
    pub port: u16,
    pub path: String,
    pub generation: u64,
    pub auth: AuthBootstrap,
    pub request_body_limit: usize,
}

impl McpHttpGatewayConfig {
    pub fn validate(&self) -> Result<(), McpHttpGatewayError> {
        if !self.bind_address.is_loopback() {
            return Err(McpHttpGatewayError::NonLoopbackBind(self.bind_address));
        }
        if self.path != "/mcp" {
            return Err(McpHttpGatewayError::InvalidPath(self.path.clone()));
        }
        if self.request_body_limit == 0 {
            return Err(McpHttpGatewayError::InvalidRequestLimit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct AuthState {
    current: Arc<RwLock<AuthBootstrap>>,
}

impl AuthState {
    fn new(auth: AuthBootstrap) -> Self {
        Self {
            current: Arc::new(RwLock::new(auth)),
        }
    }

    async fn is_valid_token(&self, token: Option<String>) -> bool {
        let Some(token) = token else {
            return false;
        };
        let current = self.current.read().await;
        constant_time_equal(token.as_bytes(), current.bearer_token().as_bytes())
    }

    async fn rotate(&self, auth: AuthBootstrap) -> Result<(), McpHttpGatewayError> {
        let current_generation = self.current.read().await.generation;
        if auth.generation <= current_generation {
            return Err(McpHttpGatewayError::StaleAuthGeneration {
                current: current_generation,
                requested: auth.generation,
            });
        }
        *self.current.write().await = auth;
        Ok(())
    }
}

#[derive(Clone)]
struct GatewayHandler {
    core: Arc<McpCore>,
    mode: GatewayMode,
}

impl ServerHandler for GatewayHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_instructions(match self.mode {
            GatewayMode::Entry => "Se MCP entry gateway",
            GatewayMode::Aggregator => "Se MCP aggregator gateway",
        })
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let aggregate = self.core.list_tools().await.map_err(to_mcp_error)?;
        Ok(ListToolsResult::with_all_items(
            aggregate.items.into_iter().map(|item| item.tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.core
            .call_tool_with_cancel(&request.name, request.arguments, context.ct)
            .await
            .map(Into::into)
            .map_err(to_mcp_error)
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let aggregate = self.core.list_resources().await.map_err(to_mcp_error)?;
        Ok(ListResourcesResult::with_all_items(
            aggregate
                .items
                .into_iter()
                .map(|item| item.resource)
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.core
            .read_resource_by_uri(&request.uri)
            .await
            .map(Into::into)
            .map_err(to_mcp_error)
    }

    async fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let aggregate = self.core.list_prompts().await.map_err(to_mcp_error)?;
        Ok(ListPromptsResult::with_all_items(
            aggregate
                .items
                .into_iter()
                .map(|item| item.prompt)
                .collect(),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.core
            .get_prompt(&request.name, request.arguments)
            .await
            .map(Into::into)
            .map_err(to_mcp_error)
    }
}

pub struct McpHttpGateway {
    endpoint: McpEndpointDescriptor,
    core: Arc<McpCore>,
    auth: AuthState,
    readiness: Arc<RwLock<McpReadinessStatus>>,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for McpHttpGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHttpGateway")
            .field("endpoint", &self.endpoint)
            .field("readiness", &"instance-owned")
            .finish_non_exhaustive()
    }
}

impl McpHttpGateway {
    pub async fn bind(
        core: Arc<McpCore>,
        config: McpHttpGatewayConfig,
    ) -> Result<Self, McpHttpGatewayError> {
        if let Err(error) = config.validate() {
            log::warn!(
                target: HTTP_BOUNDARY_LOG_TARGET,
                "operation=mcp_http_bind stable_code={}",
                bind_failure_code(&error)
            );
            return Err(error);
        }
        let listener = TcpListener::bind(SocketAddr::new(config.bind_address, config.port))
            .await
            .map_err(|error| {
                log::error!(
                    target: HTTP_BOUNDARY_LOG_TARGET,
                    "operation=mcp_http_bind stable_code=BIND_FAILED"
                );
                McpHttpGatewayError::Bind(error)
            })?;
        let address = listener.local_addr().map_err(|error| {
            log::error!(
                target: HTTP_BOUNDARY_LOG_TARGET,
                "operation=mcp_http_bind stable_code=BIND_FAILED"
            );
            McpHttpGatewayError::Bind(error)
        })?;
        let endpoint = McpEndpointDescriptor {
            generation: config.generation,
            bind_address: address.ip().to_string(),
            port: address.port(),
            path: config.path,
            auth_generation: config.auth.generation,
        };
        endpoint.validate_loopback().map_err(|error| {
            log::warn!(
                target: HTTP_BOUNDARY_LOG_TARGET,
                "operation=mcp_http_bind stable_code=INVALID_ENDPOINT"
            );
            McpHttpGatewayError::InvalidEndpoint(error.message)
        })?;

        let auth = AuthState::new(config.auth);
        let readiness = Arc::new(RwLock::new(McpReadinessStatus {
            state: McpReadiness::Starting,
            config_revision: None,
            endpoint: Some(endpoint.clone()),
            generation: endpoint.generation,
            diagnostic: None,
        }));
        let shutdown = CancellationToken::new();
        let server_config = || {
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .with_cancellation_token(shutdown.child_token())
        };
        let entry_service = StreamableHttpService::new(
            {
                let handler = GatewayHandler {
                    core: core.clone(),
                    mode: GatewayMode::Entry,
                };
                move || Ok(handler.clone())
            },
            Arc::new(LocalSessionManager::default()),
            server_config(),
        );
        let aggregator_service = StreamableHttpService::new(
            {
                let handler = GatewayHandler {
                    core: core.clone(),
                    mode: GatewayMode::Aggregator,
                };
                move || Ok(handler.clone())
            },
            Arc::new(LocalSessionManager::default()),
            server_config(),
        );
        let request_body_limit = config.request_body_limit;
        let auth_for_middleware = auth.clone();
        let protected = Router::new()
            .nest_service("/mcp", entry_service)
            .nest_service("/mcp/aggregator", aggregator_service)
            .layer(DefaultBodyLimit::max(request_body_limit))
            .layer(middleware::from_fn(move |request, next| {
                let auth = auth_for_middleware.clone();
                async move { require_bearer(request, next, auth).await }
            }));
        let app = Router::new()
            .merge(protected)
            .route("/health", get(health))
            .route("/ready", get(ready))
            .route("/mcp-core/status", get(ready_status))
            .with_state(readiness.clone());
        let shutdown_for_server = shutdown.clone();
        let readiness_for_task = readiness.clone();
        let generation = endpoint.generation;
        let task = tokio::spawn(async move {
            {
                let mut status = readiness_for_task.write().await;
                status.state = McpReadiness::Ready;
            }
            log::info!(
                target: HTTP_BOUNDARY_LOG_TARGET,
                "operation=mcp_http_ready generation={generation} state=ready stable_code=OK"
            );
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_for_server.cancelled_owned())
                .await;
            let mut status = readiness_for_task.write().await;
            status.state = McpReadiness::Stopped;
            log::info!(
                target: HTTP_BOUNDARY_LOG_TARGET,
                "operation=mcp_http_stop generation={generation} state=stopped stable_code=OK"
            );
        });

        log::info!(
            target: HTTP_BOUNDARY_LOG_TARGET,
            "operation=mcp_http_bind generation={} port={} auth_generation={} loopback=1 stable_code=OK",
            endpoint.generation,
            endpoint.port,
            endpoint.auth_generation
        );

        Ok(Self {
            endpoint,
            core,
            auth,
            readiness,
            shutdown,
            task,
        })
    }

    pub fn endpoint(&self) -> &McpEndpointDescriptor {
        &self.endpoint
    }

    pub async fn status(&self) -> McpReadinessStatus {
        self.readiness.read().await.clone()
    }

    pub async fn rotate_auth(&self, auth: AuthBootstrap) -> Result<(), McpHttpGatewayError> {
        let generation = auth.generation;
        if let Err(error) = self.auth.rotate(auth).await {
            if let McpHttpGatewayError::StaleAuthGeneration { current, requested } = &error {
                log::warn!(
                    target: HTTP_BOUNDARY_LOG_TARGET,
                    "operation=mcp_http_auth_rotate generation={} current_auth_generation={} requested_auth_generation={} stable_code=STALE_AUTH_GENERATION",
                    self.endpoint.generation,
                    current,
                    requested
                );
            }
            return Err(error);
        }
        let mut status = self.readiness.write().await;
        if let Some(endpoint) = status.endpoint.as_mut() {
            endpoint.auth_generation = generation;
        }
        log::info!(
            target: HTTP_BOUNDARY_LOG_TARGET,
            "operation=mcp_http_auth_rotate generation={} auth_generation={} stable_code=OK",
            self.endpoint.generation,
            generation
        );
        Ok(())
    }

    pub async fn current_endpoint(&self) -> McpEndpointDescriptor {
        self.readiness
            .read()
            .await
            .endpoint
            .clone()
            .expect("gateway endpoint is present while gateway exists")
    }

    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
        self.core.shutdown().await;
    }
}

async fn require_bearer(request: Request<Body>, next: Next, auth: AuthState) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    if !auth.is_valid_token(token).await {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "unauthorized",
        )
            .into_response();
    }
    next.run(request).await
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn ready(State(status): State<Arc<RwLock<McpReadinessStatus>>>) -> Response {
    let status = status.read().await.clone();
    match status.state {
        McpReadiness::Ready | McpReadiness::Degraded => (StatusCode::OK, "ready").into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response(),
    }
}

async fn ready_status(State(status): State<Arc<RwLock<McpReadinessStatus>>>) -> Response {
    let status = status.read().await.clone();
    match status.state {
        McpReadiness::Ready | McpReadiness::Degraded => axum::Json(status).into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response(),
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn to_mcp_error(error: McpDomainError) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

fn bind_failure_code(error: &McpHttpGatewayError) -> &'static str {
    match error {
        McpHttpGatewayError::NonLoopbackBind(_) => "NON_LOOPBACK_BIND",
        McpHttpGatewayError::InvalidPath(_) => "INVALID_PATH",
        McpHttpGatewayError::InvalidRequestLimit => "INVALID_REQUEST_LIMIT",
        McpHttpGatewayError::InvalidEndpoint(_) => "INVALID_ENDPOINT",
        McpHttpGatewayError::StaleAuthGeneration { .. } => "STALE_AUTH_GENERATION",
        McpHttpGatewayError::Bind(_) => "BIND_FAILED",
    }
}

#[derive(Debug)]
pub enum McpHttpGatewayError {
    NonLoopbackBind(IpAddr),
    InvalidPath(String),
    InvalidRequestLimit,
    InvalidEndpoint(String),
    StaleAuthGeneration { current: u64, requested: u64 },
    Bind(std::io::Error),
}

impl std::fmt::Display for McpHttpGatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonLoopbackBind(address) => {
                write!(f, "MCP HTTP gateway must bind to loopback: {address}")
            }
            Self::InvalidPath(path) => write!(f, "invalid MCP HTTP path: {path}"),
            Self::InvalidRequestLimit => f.write_str("MCP HTTP request limit must be positive"),
            Self::InvalidEndpoint(message) => write!(f, "invalid MCP endpoint: {message}"),
            Self::StaleAuthGeneration { current, requested } => write!(
                f,
                "MCP auth generation {requested} is not newer than current generation {current}"
            ),
            Self::Bind(error) => write!(f, "failed to bind MCP HTTP gateway: {error}"),
        }
    }
}

impl std::error::Error for McpHttpGatewayError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    #[test]
    fn rejects_non_loopback_bind() {
        let config = McpHttpGatewayConfig {
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            path: "/mcp".into(),
            generation: 1,
            auth: AuthBootstrap::new(1, "token").unwrap(),
            request_body_limit: 1024,
        };
        assert!(matches!(
            config.validate(),
            Err(McpHttpGatewayError::NonLoopbackBind(_))
        ));
    }

    #[test]
    fn bearer_comparison_is_exact_and_length_sensitive() {
        assert!(constant_time_equal(b"secret", b"secret"));
        assert!(!constant_time_equal(b"secret", b"secret2"));
        assert!(!constant_time_equal(b"secret", b"SECRET"));
    }

    #[tokio::test]
    async fn bind_failure_logs_stable_code_without_bind_payload() {
        let _guard = crate::web::auth::test_tracing::lock().await;
        let result = McpHttpGateway::bind(
            Arc::new(McpCore::default()),
            McpHttpGatewayConfig {
                bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 0,
                path: "/mcp".into(),
                generation: 1,
                auth: AuthBootstrap::new(1, "token").unwrap(),
                request_body_limit: 1024,
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(McpHttpGatewayError::NonLoopbackBind(_))
        ));
        let output = crate::web::auth::test_tracing::messages(HTTP_BOUNDARY_LOG_TARGET).join("\n");
        assert!(output.contains("operation=mcp_http_bind"));
        assert!(output.contains("stable_code=NON_LOOPBACK_BIND"));
        assert!(!output.contains("0.0.0.0"));
    }

    #[tokio::test]
    async fn boundary_logs_cover_bind_ready_auth_rotate_and_stop_without_secrets() {
        let _guard = crate::web::auth::test_tracing::lock().await;
        let gateway = McpHttpGateway::bind(
            Arc::new(McpCore::default()),
            McpHttpGatewayConfig {
                bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                path: "/mcp".into(),
                generation: 9,
                auth: AuthBootstrap::new(1, "old-secret-token").unwrap(),
                request_body_limit: 1024,
            },
        )
        .await
        .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while gateway.status().await.state != McpReadiness::Ready {
            if tokio::time::Instant::now() >= deadline {
                panic!("gateway did not become ready");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        gateway
            .rotate_auth(AuthBootstrap::new(2, "new-secret-token").unwrap())
            .await
            .unwrap();
        assert!(matches!(
            gateway
                .rotate_auth(AuthBootstrap::new(2, "stale-secret-token").unwrap())
                .await,
            Err(McpHttpGatewayError::StaleAuthGeneration { .. })
        ));
        gateway.shutdown().await;

        let output = crate::web::auth::test_tracing::messages(HTTP_BOUNDARY_LOG_TARGET).join("\n");
        for required in [
            "operation=mcp_http_bind",
            "generation=9",
            "loopback=1",
            "operation=mcp_http_ready",
            "state=ready",
            "operation=mcp_http_auth_rotate",
            "auth_generation=2",
            "stable_code=STALE_AUTH_GENERATION",
            "operation=mcp_http_stop",
            "state=stopped",
            "stable_code=OK",
        ] {
            assert!(output.contains(required), "missing {required}: {output}");
        }
        for leaked in [
            "old-secret-token",
            "new-secret-token",
            "stale-secret-token",
            "Authorization",
            "Bearer ",
        ] {
            assert!(!output.contains(leaked), "leaked {leaked}: {output}");
        }
    }
}
