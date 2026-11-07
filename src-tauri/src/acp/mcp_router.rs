//! Host-owned ACP MCP Router entry builder.
//!
//! ACP `session/new/load/resume` keep the protocol `mcpServers` field for wire
//! compatibility, but Termul generates the session set: the legacy `host_mcp`
//! stdio child (when injected) plus at most one Streamable HTTP Router entry
//! per isolated MCP source. Caller/user-registry servers are never
//! appended. When a gateway identity/endpoint/auth cannot be validated, its Router is
//! explicitly absent rather than restoring per-agent configuration.

use std::{net::IpAddr, str::FromStr, time::Duration};

use se_mcp_bridge::Source;

use agent_client_protocol::schema::v1::{AgentCapabilities, HttpHeader, McpServer, McpServerHttp};

use crate::mcp_core::{AuthBootstrap, McpEndpointDescriptor};

pub const MCP_ROUTER_ACP_NAME: &str = "mcp-router";
pub const MCP_AGGREGATOR_ROUTER_ACP_NAME: &str = "mcp-router-aggregator";
pub const MCP_ROUTER_LOG_TARGET: &str = "se_manager::acp::mcp_router";

pub const fn router_name(source: Source) -> &'static str {
    match source {
        Source::Internal => MCP_ROUTER_ACP_NAME,
        Source::Aggregator => MCP_AGGREGATOR_ROUTER_ACP_NAME,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRouterUnavailableReason {
    CoreNotWired,
    NonLoopback,
    MissingAuth,
    InvalidEndpoint,
    IdentityRejected,
    AgentLacksHttp,
}

impl McpRouterUnavailableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreNotWired => "core_not_wired",
            Self::NonLoopback => "non_loopback",
            Self::MissingAuth => "missing_auth",
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::IdentityRejected => "identity_rejected",
            Self::AgentLacksHttp => "agent_lacks_http",
        }
    }
}

#[derive(Clone)]
pub enum McpRouterAvailability {
    Available {
        endpoint: McpEndpointDescriptor,
        auth: AuthBootstrap,
    },
    Unavailable {
        reason: McpRouterUnavailableReason,
    },
}

impl std::fmt::Debug for McpRouterAvailability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Available { endpoint, auth } => f
                .debug_struct("Available")
                .field("generation", &endpoint.generation)
                .field("port", &endpoint.port)
                .field("auth_generation", &auth.generation)
                .finish_non_exhaustive(),
            Self::Unavailable { reason } => f
                .debug_struct("Unavailable")
                .field("reason", &reason.as_str())
                .finish(),
        }
    }
}

impl Default for McpRouterAvailability {
    fn default() -> Self {
        Self::Unavailable {
            reason: McpRouterUnavailableReason::CoreNotWired,
        }
    }
}

impl McpRouterAvailability {
    pub fn from_endpoint_auth(endpoint: McpEndpointDescriptor, auth: AuthBootstrap) -> Self {
        if auth.bearer_token().is_empty() {
            return Self::Unavailable {
                reason: McpRouterUnavailableReason::MissingAuth,
            };
        }
        match loopback_http_url(&endpoint) {
            Ok(_) => Self::Available { endpoint, auth },
            Err(reason) => Self::Unavailable { reason },
        }
    }

    /// Revalidate the live source before exposing any bearer token to an agent.
    pub async fn verify_identity(self, source: Source) -> Self {
        let Self::Available { ref endpoint, .. } = self else {
            return self;
        };
        let origin = loopback_http_url(endpoint)
            .ok()
            .and_then(|url| reqwest::Url::parse(&url).ok())
            .map(|mut url| {
                url.set_path("/");
                url
            });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(2))
            .build();
        if let (Some(origin), Ok(client)) = (origin, client) {
            if se_mcp_bridge::verify_gateway_identity(&client, &origin, source)
                .await
                .is_ok()
            {
                return self;
            }
        }
        Self::Unavailable {
            reason: McpRouterUnavailableReason::IdentityRejected,
        }
    }

    #[cfg(test)]
    pub fn reason(&self) -> Option<McpRouterUnavailableReason> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable { reason } => Some(*reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterApply {
    pub injected: bool,
    pub reason: Option<McpRouterUnavailableReason>,
}

pub fn append_router_entry(
    servers: &mut Vec<McpServer>,
    source: Source,
    availability: &McpRouterAvailability,
    caps: &AgentCapabilities,
) -> RouterApply {
    servers.retain(
        |existing| !matches!(existing, McpServer::Http(http) if http.name == router_name(source)),
    );
    match availability {
        McpRouterAvailability::Unavailable { reason } => RouterApply {
            injected: false,
            reason: Some(*reason),
        },
        McpRouterAvailability::Available { endpoint, auth } => {
            if !caps.mcp_capabilities.http {
                return RouterApply {
                    injected: false,
                    reason: Some(McpRouterUnavailableReason::AgentLacksHttp),
                };
            }
            match build_router_http(source, endpoint, auth) {
                Ok(server) => {
                    servers.push(server);
                    RouterApply {
                        injected: true,
                        reason: None,
                    }
                }
                Err(reason) => RouterApply {
                    injected: false,
                    reason: Some(reason),
                },
            }
        }
    }
}

pub fn is_router_http(server: &McpServer) -> bool {
    matches!(server, McpServer::Http(http) if http.name == MCP_ROUTER_ACP_NAME || http.name == MCP_AGGREGATOR_ROUTER_ACP_NAME)
}

#[cfg(test)]
pub fn router_http_count(servers: &[McpServer]) -> usize {
    servers
        .iter()
        .filter(|server| is_router_http(server))
        .count()
}

fn build_router_http(
    source: Source,
    endpoint: &McpEndpointDescriptor,
    auth: &AuthBootstrap,
) -> Result<McpServer, McpRouterUnavailableReason> {
    if auth.bearer_token().is_empty() {
        return Err(McpRouterUnavailableReason::MissingAuth);
    }
    let url = loopback_http_url(endpoint)?;
    let headers = vec![HttpHeader::new(
        "Authorization",
        format!("Bearer {}", auth.bearer_token()),
    )];
    Ok(McpServer::Http(
        McpServerHttp::new(router_name(source), url).headers(headers),
    ))
}

fn loopback_http_url(
    endpoint: &McpEndpointDescriptor,
) -> Result<String, McpRouterUnavailableReason> {
    let address = IpAddr::from_str(&endpoint.bind_address)
        .map_err(|_| McpRouterUnavailableReason::InvalidEndpoint)?;
    if !address.is_loopback() {
        return Err(McpRouterUnavailableReason::NonLoopback);
    }
    if endpoint.port == 0 {
        return Err(McpRouterUnavailableReason::InvalidEndpoint);
    }
    if !matches!(
        endpoint.path.as_str(),
        "/mcp" | "/mcp/entry" | "/mcp/aggregator" | "/mcp/direct"
    ) {
        return Err(McpRouterUnavailableReason::InvalidEndpoint);
    }
    let host = match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    };
    Ok(format!("http://{host}:{}{}", endpoint.port, endpoint.path))
}

/// Local-only identity listener used by ACP fixture tests; never starts a service.
#[cfg(test)]
pub(super) struct TestGateway {
    pub endpoint: McpEndpointDescriptor,
    pub auth: AuthBootstrap,
    pub requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub auth_seen: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(test)]
impl TestGateway {
    pub async fn start(source: Source) -> Self {
        Self::responding(
            axum::http::StatusCode::OK,
            serde_json::json!({
                "protocol": "se-mcp-gateway",
                "protocolVersion": 1,
                "source": source.as_str(),
            }),
        )
        .await
    }

    pub async fn responding(status: axum::http::StatusCode, body: serde_json::Value) -> Self {
        use std::sync::{atomic::Ordering, Arc};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let auth_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let request_count = requests.clone();
        let has_auth = auth_seen.clone();
        let app = axum::Router::new().route(
            "/mcp-core/identity",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let body = body.clone();
                request_count.fetch_add(1, Ordering::SeqCst);
                if headers.contains_key("authorization") {
                    has_auth.store(true, Ordering::SeqCst);
                }
                async move {
                    let mut response = (status, axum::Json(body)).into_response();
                    // If a redirect is followed, the second request hits this listener.
                    if status.is_redirection() {
                        response.headers_mut().insert(
                            "location",
                            axum::http::HeaderValue::from_static("/mcp-core/identity"),
                        );
                    }
                    response
                }
            }),
        );
        use axum::response::IntoResponse;
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint: McpEndpointDescriptor {
                generation: 1,
                bind_address: "127.0.0.1".into(),
                port,
                path: "/mcp".into(),
                auth_generation: 1,
            },
            auth: AuthBootstrap::new(1, format!("fixture-token-{port}")).unwrap(),
            requests,
            auth_seen,
            task,
        }
    }

    pub async fn availability(&self, expected: Source) -> McpRouterAvailability {
        McpRouterAvailability::from_endpoint_auth(self.endpoint.clone(), self.auth.clone())
            .verify_identity(expected)
            .await
    }
}

#[cfg(test)]
impl Drop for TestGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::McpServerStdio;
    use std::path::PathBuf;

    fn http_caps() -> AgentCapabilities {
        let mut caps = AgentCapabilities::default();
        caps.mcp_capabilities.http = true;
        caps
    }

    #[tokio::test]
    async fn available_loopback_injects_exactly_one_http_router() {
        let fixture = TestGateway::start(Source::Internal).await;
        let availability = fixture.availability(Source::Internal).await;
        let mut servers = Vec::new();
        let apply =
            append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        assert!(apply.injected);
        assert_eq!(apply.reason, None);
        assert_eq!(servers.len(), 1);
        assert_eq!(router_http_count(&servers), 1);
        let McpServer::Http(http) = &servers[0] else {
            panic!("expected HTTP router");
        };
        assert_eq!(http.name, MCP_ROUTER_ACP_NAME);
        assert_eq!(
            http.url,
            format!("http://127.0.0.1:{}/mcp", fixture.endpoint.port)
        );
        assert_eq!(http.headers.len(), 1);
        assert_eq!(http.headers[0].name, "Authorization");
        assert_eq!(
            http.headers[0].value,
            format!("Bearer {}", fixture.auth.bearer_token())
        );
    }

    #[tokio::test]
    async fn host_stdio_is_preserved_beside_single_router() {
        let fixture = TestGateway::start(Source::Internal).await;
        let availability = fixture.availability(Source::Internal).await;
        let host = McpServer::Stdio(McpServerStdio::new(
            "termul".to_string(),
            PathBuf::from("/bin/echo"),
        ));
        let mut servers = vec![host];
        append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        assert_eq!(servers.len(), 2);
        assert_eq!(router_http_count(&servers), 1);
        assert!(matches!(&servers[0], McpServer::Stdio(stdio) if stdio.name == "termul"));
        assert!(is_router_http(&servers[1]));
    }

    #[tokio::test]
    async fn second_append_still_yields_one_router_entry() {
        let fixture = TestGateway::start(Source::Internal).await;
        let availability = fixture.availability(Source::Internal).await;
        let mut servers = Vec::new();
        append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        assert_eq!(router_http_count(&servers), 1);
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn non_loopback_endpoint_is_explicitly_unavailable() {
        let endpoint = McpEndpointDescriptor {
            generation: 1,
            bind_address: "8.8.8.8".into(),
            port: 17300,
            path: "/mcp".into(),
            auth_generation: 1,
        };
        let availability = McpRouterAvailability::from_endpoint_auth(
            endpoint,
            AuthBootstrap::new(1, "router-secret").unwrap(),
        );
        assert_eq!(
            availability.reason(),
            Some(McpRouterUnavailableReason::NonLoopback)
        );
        let mut servers = Vec::new();
        let apply =
            append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        assert!(!apply.injected);
        assert!(servers.is_empty());
    }

    #[test]
    fn ipv6_loopback_url_is_bracketed() {
        let endpoint = McpEndpointDescriptor {
            generation: 1,
            bind_address: "::1".into(),
            port: 17303,
            path: "/mcp".into(),
            auth_generation: 1,
        };
        assert_eq!(
            loopback_http_url(&endpoint).unwrap(),
            "http://[::1]:17303/mcp"
        );
    }

    #[tokio::test]
    async fn agent_without_http_skips_router_and_does_not_restore_registry() {
        let fixture = TestGateway::start(Source::Internal).await;
        let availability = fixture.availability(Source::Internal).await;
        let mut servers = Vec::new();
        let apply = append_router_entry(
            &mut servers,
            Source::Internal,
            &availability,
            &AgentCapabilities::default(),
        );
        assert!(!apply.injected);
        assert_eq!(
            apply.reason,
            Some(McpRouterUnavailableReason::AgentLacksHttp)
        );
        assert!(servers.is_empty());
    }

    #[test]
    fn unwired_core_is_explicit_absence() {
        let availability = McpRouterAvailability::default();
        assert_eq!(
            availability.reason(),
            Some(McpRouterUnavailableReason::CoreNotWired)
        );
        let mut servers = Vec::new();
        let apply =
            append_router_entry(&mut servers, Source::Internal, &availability, &http_caps());
        assert!(!apply.injected);
        assert_eq!(apply.reason, Some(McpRouterUnavailableReason::CoreNotWired));
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn sources_deduplicate_independently_and_failure_only_removes_own_router() {
        let internal = TestGateway::start(Source::Internal).await;
        let aggregator = TestGateway::start(Source::Aggregator).await;
        let mut servers = Vec::new();
        for _ in 0..2 {
            for (source, fixture) in [
                (Source::Internal, &internal),
                (Source::Aggregator, &aggregator),
            ] {
                let availability = fixture.availability(source).await;
                assert!(
                    append_router_entry(&mut servers, source, &availability, &http_caps()).injected
                );
            }
        }
        assert_eq!(router_http_count(&servers), 2);
        for source in [Source::Internal, Source::Aggregator] {
            assert_eq!(servers.iter().filter(|server| matches!(server, McpServer::Http(http) if http.name == router_name(source))).count(), 1);
        }
        append_router_entry(
            &mut servers,
            Source::Internal,
            &McpRouterAvailability::default(),
            &http_caps(),
        );
        assert_eq!(router_http_count(&servers), 1);
        assert!(
            matches!(&servers[0], McpServer::Http(http) if http.name == router_name(Source::Aggregator))
        );
        assert!(!internal.auth_seen.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!aggregator
            .auth_seen
            .load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn identity_rejects_wrong_source_missing_marker_version_and_redirect_without_auth() {
        use axum::http::StatusCode;
        use std::sync::atomic::Ordering;
        let valid =
            serde_json::to_value(se_mcp_bridge::GatewayIdentity::new(Source::Internal)).unwrap();
        let mut wrong_version = valid.clone();
        wrong_version["protocolVersion"] = serde_json::json!(99);
        let mut wrong_protocol = valid.clone();
        wrong_protocol["protocol"] = serde_json::json!("unknown-gateway");
        for (status, body) in [
            (StatusCode::OK, serde_json::json!({})),
            (StatusCode::OK, wrong_version),
            (StatusCode::OK, wrong_protocol),
            (StatusCode::TEMPORARY_REDIRECT, valid),
            (
                StatusCode::OK,
                serde_json::to_value(se_mcp_bridge::GatewayIdentity::new(Source::Aggregator))
                    .unwrap(),
            ),
        ] {
            let fixture = TestGateway::responding(status, body).await;
            assert_eq!(
                fixture.availability(Source::Internal).await.reason(),
                Some(McpRouterUnavailableReason::IdentityRejected)
            );
            assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
            assert!(!fixture.auth_seen.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn invalid_router_paths_cannot_carry_query_fragment_or_userinfo() {
        for path in [
            "/",
            "/mcp?token=bad",
            "/mcp#fragment",
            "//attacker/mcp",
            "/other",
        ] {
            let endpoint = McpEndpointDescriptor {
                generation: 1,
                bind_address: "127.0.0.1".into(),
                port: 1,
                path: path.into(),
                auth_generation: 1,
            };
            assert_eq!(
                loopback_http_url(&endpoint),
                Err(McpRouterUnavailableReason::InvalidEndpoint)
            );
        }
    }

    #[tokio::test]
    async fn debug_redacts_token_and_omits_path() {
        let fixture = TestGateway::start(Source::Internal).await;
        let availability = fixture.availability(Source::Internal).await;
        let rendered = format!("{availability:?}");
        assert!(!rendered.contains(fixture.auth.bearer_token()));
        assert!(!rendered.contains("/mcp"));
        assert!(!rendered.contains("Authorization"));
        assert!(rendered.contains("port"));
    }
}
