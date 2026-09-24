//! Host-owned ACP MCP Router entry builder.
//!
//! ACP `session/new/load/resume` keep the protocol `mcpServers` field for wire
//! compatibility, but Termul generates the session set: the legacy `host_mcp`
//! stdio child (when injected) plus at most one Streamable HTTP Router entry
//! pointing at the canonical MCP Core. Caller/user-registry servers are never
//! appended. When Core endpoint/auth cannot be validated, the Router is
//! explicitly absent rather than restoring per-agent configuration.

use std::net::IpAddr;
use std::str::FromStr;

use agent_client_protocol::schema::v1::{AgentCapabilities, HttpHeader, McpServer, McpServerHttp};

use crate::mcp_core::{AuthBootstrap, McpEndpointDescriptor};

pub const MCP_ROUTER_ACP_NAME: &str = "mcp-router";
pub const MCP_ROUTER_LOG_TARGET: &str = "se_manager::acp::mcp_router";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRouterUnavailableReason {
    CoreNotWired,
    NonLoopback,
    MissingAuth,
    InvalidEndpoint,
    AgentLacksHttp,
}

impl McpRouterUnavailableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreNotWired => "core_not_wired",
            Self::NonLoopback => "non_loopback",
            Self::MissingAuth => "missing_auth",
            Self::InvalidEndpoint => "invalid_endpoint",
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
    availability: &McpRouterAvailability,
    caps: &AgentCapabilities,
) -> RouterApply {
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
            match build_router_http(endpoint, auth) {
                Ok(server) => {
                    servers.retain(|existing| !is_router_http(existing));
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
    matches!(server, McpServer::Http(http) if http.name == MCP_ROUTER_ACP_NAME)
}

#[cfg(test)]
pub fn router_http_count(servers: &[McpServer]) -> usize {
    servers
        .iter()
        .filter(|server| is_router_http(server))
        .count()
}

fn build_router_http(
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
        McpServerHttp::new(MCP_ROUTER_ACP_NAME, url).headers(headers),
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
    if !endpoint.path.starts_with('/') || endpoint.path == "/" {
        return Err(McpRouterUnavailableReason::InvalidEndpoint);
    }
    let host = match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    };
    Ok(format!("http://{host}:{}{}", endpoint.port, endpoint.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::McpServerStdio;
    use std::path::PathBuf;

    fn loopback_endpoint(port: u16) -> McpEndpointDescriptor {
        McpEndpointDescriptor {
            generation: 1,
            bind_address: "127.0.0.1".into(),
            port,
            path: "/mcp".into(),
            auth_generation: 3,
        }
    }

    fn http_caps() -> AgentCapabilities {
        let mut caps = AgentCapabilities::default();
        caps.mcp_capabilities.http = true;
        caps
    }

    #[test]
    fn available_loopback_injects_exactly_one_http_router() {
        let availability = McpRouterAvailability::from_endpoint_auth(
            loopback_endpoint(17300),
            AuthBootstrap::new(3, "router-secret").unwrap(),
        );
        let mut servers = Vec::new();
        let apply = append_router_entry(&mut servers, &availability, &http_caps());
        assert!(apply.injected);
        assert_eq!(apply.reason, None);
        assert_eq!(servers.len(), 1);
        assert_eq!(router_http_count(&servers), 1);
        let McpServer::Http(http) = &servers[0] else {
            panic!("expected HTTP router");
        };
        assert_eq!(http.name, MCP_ROUTER_ACP_NAME);
        assert_eq!(http.url, "http://127.0.0.1:17300/mcp");
        assert_eq!(http.headers.len(), 1);
        assert_eq!(http.headers[0].name, "Authorization");
        assert_eq!(http.headers[0].value, "Bearer router-secret");
    }

    #[test]
    fn host_stdio_is_preserved_beside_single_router() {
        let availability = McpRouterAvailability::from_endpoint_auth(
            loopback_endpoint(17301),
            AuthBootstrap::new(1, "router-secret").unwrap(),
        );
        let host = McpServer::Stdio(McpServerStdio::new(
            "termul".to_string(),
            PathBuf::from("/bin/echo"),
        ));
        let mut servers = vec![host];
        append_router_entry(&mut servers, &availability, &http_caps());
        assert_eq!(servers.len(), 2);
        assert_eq!(router_http_count(&servers), 1);
        assert!(matches!(&servers[0], McpServer::Stdio(stdio) if stdio.name == "termul"));
        assert!(is_router_http(&servers[1]));
    }

    #[test]
    fn second_append_still_yields_one_router_entry() {
        let availability = McpRouterAvailability::from_endpoint_auth(
            loopback_endpoint(17302),
            AuthBootstrap::new(1, "router-secret").unwrap(),
        );
        let mut servers = Vec::new();
        append_router_entry(&mut servers, &availability, &http_caps());
        append_router_entry(&mut servers, &availability, &http_caps());
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
        let apply = append_router_entry(&mut servers, &availability, &http_caps());
        assert!(!apply.injected);
        assert!(servers.is_empty());
    }

    #[test]
    fn ipv6_loopback_is_bracketed_and_accepted() {
        let endpoint = McpEndpointDescriptor {
            generation: 1,
            bind_address: "::1".into(),
            port: 17303,
            path: "/mcp".into(),
            auth_generation: 1,
        };
        let availability = McpRouterAvailability::from_endpoint_auth(
            endpoint,
            AuthBootstrap::new(1, "router-secret").unwrap(),
        );
        let mut servers = Vec::new();
        let apply = append_router_entry(&mut servers, &availability, &http_caps());
        assert!(apply.injected);
        let McpServer::Http(http) = &servers[0] else {
            panic!("expected HTTP router");
        };
        assert_eq!(http.url, "http://[::1]:17303/mcp");
    }

    #[test]
    fn agent_without_http_skips_router_and_does_not_restore_registry() {
        let availability = McpRouterAvailability::from_endpoint_auth(
            loopback_endpoint(17304),
            AuthBootstrap::new(1, "router-secret").unwrap(),
        );
        let mut servers = Vec::new();
        let apply = append_router_entry(&mut servers, &availability, &AgentCapabilities::default());
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
        let apply = append_router_entry(&mut servers, &availability, &http_caps());
        assert!(!apply.injected);
        assert_eq!(apply.reason, Some(McpRouterUnavailableReason::CoreNotWired));
        assert!(servers.is_empty());
    }

    #[test]
    fn debug_redacts_token_and_omits_path() {
        let availability = McpRouterAvailability::from_endpoint_auth(
            loopback_endpoint(17305),
            AuthBootstrap::new(9, "super-secret-token").unwrap(),
        );
        let rendered = format!("{availability:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(!rendered.contains("/mcp"));
        assert!(!rendered.contains("Authorization"));
        assert!(rendered.contains("port"));
    }
}
