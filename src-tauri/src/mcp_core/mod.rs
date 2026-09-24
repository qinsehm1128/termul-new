//! Runtime-neutral contracts for Se's independent HTTP-first MCP Core.
//!
//! Se owns persistence and secret resolution. These types describe the
//! versioned messages exchanged with the Core; they do not perform I/O or
//! mutate configuration.

use std::{collections::BTreeMap, fmt, net::IpAddr, str::FromStr};

use serde::{Deserialize, Serialize};

pub mod builtins;
pub mod config;
pub mod desktop;
pub mod domain;
pub mod http;
pub mod process;
pub mod snapshot;
pub use builtins::{BuiltInCapability, BuiltInRegistry};
pub use config::{
    default_built_ins, prepare_write, ConfigError, ConfigSource, McpBuiltInConfig,
    McpBuiltInStatus, McpCapabilityPolicy, McpControlPlaneConfig, McpControlPlaneStatus,
    McpPersistedTransport, McpRoutingConfig, McpUpstreamConfig, McpUpstreamStatus,
    NameCollisionPolicy, NamedSecret, ParsedControlPlane, BUILTIN_PROJECT_SCOPE,
    BUILTIN_SESSION_MEMORY, MCP_CONTROL_PLANE_SCHEMA_VERSION,
};
pub use desktop::{
    DesktopMcpCoreAvailability, DesktopMcpCoreRuntime, DesktopMcpCoreStatus, MCP_CORE_ENABLED_ENV,
};
pub use domain::{
    Aggregate, AggregatedPrompt, AggregatedResource, AggregatedTool, AllowAllTools,
    DenyListedTools, McpCore, McpCoreConfig, McpDomainError, Operation, ToolPermission,
    UpstreamFailure, UpstreamKind,
};
pub use http::{GatewayMode, McpHttpGateway, McpHttpGatewayConfig, McpHttpGatewayError};
pub use process::{
    run_mcp_core_process, McpCoreProcessConfig, McpCoreSupervisor, ProcessError,
    MCP_CORE_PROBE_MISSES,
};
pub use snapshot::{
    build_snapshot, snapshot_from_config, InlineSecretResolver, McpSecretResolver,
    McpSnapshotController, SnapshotApplyReceipt, SnapshotError,
};

pub const MCP_CORE_CONTRACT_VERSION: u16 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigSnapshot {
    pub contract_version: u16,
    pub revision: u64,
    pub servers: Vec<McpUpstreamServer>,
}

impl McpConfigSnapshot {
    pub fn validate(&self) -> Result<(), McpCoreError> {
        if self.contract_version != MCP_CORE_CONTRACT_VERSION {
            return Err(McpCoreError::invalid_snapshot(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.revision == 0 {
            return Err(McpCoreError::invalid_snapshot(
                "snapshot revision must be greater than zero",
            ));
        }

        let mut ids = std::collections::BTreeSet::new();
        for server in &self.servers {
            if server.id.trim().is_empty() {
                return Err(McpCoreError::invalid_snapshot(
                    "upstream server id must not be empty",
                ));
            }
            if !ids.insert(&server.id) {
                return Err(McpCoreError::invalid_snapshot(format!(
                    "duplicate upstream server id {}",
                    server.id
                )));
            }
            server.transport.validate(&server.id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpUpstreamServer {
    pub id: String,
    pub enabled: bool,
    pub transport: McpUpstreamTransport,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum McpUpstreamTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, RedactedSecret>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, RedactedSecret>,
    },
}

impl fmt::Debug for McpUpstreamTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &RedactedMap(env))
                .finish(),
            Self::StreamableHttp { url, headers } => f
                .debug_struct("StreamableHttp")
                .field("url", url)
                .field("headers", &RedactedMap(headers))
                .finish(),
        }
    }
}

impl McpUpstreamTransport {
    fn validate(&self, server_id: &str) -> Result<(), McpCoreError> {
        match self {
            Self::Stdio { command, .. } if command.trim().is_empty() => {
                Err(McpCoreError::invalid_snapshot(format!(
                    "stdio command for upstream {server_id} must not be empty"
                )))
            }
            Self::Stdio { .. } => Ok(()),
            Self::StreamableHttp { url, .. } => {
                let parsed = url::Url::parse(url).map_err(|error| {
                    McpCoreError::invalid_snapshot(format!(
                        "invalid HTTP URL for upstream {server_id}: {error}"
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                    return Err(McpCoreError::invalid_snapshot(format!(
                        "HTTP URL for upstream {server_id} must have an http(s) scheme and host"
                    )));
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedSecret(String);

impl RedactedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

struct RedactedMap<'a>(&'a BTreeMap<String, RedactedSecret>);

impl fmt::Debug for RedactedMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for key in self.0.keys() {
            map.entry(key, &"<redacted>");
        }
        map.finish()
    }
}

impl fmt::Debug for McpConfigSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpConfigSnapshot")
            .field("contract_version", &self.contract_version)
            .field("revision", &self.revision)
            .field("servers", &self.servers)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthBootstrap {
    pub generation: u64,
    pub scheme: AuthScheme,
    pub bearer_token: RedactedSecret,
}

impl AuthBootstrap {
    pub fn new(generation: u64, bearer_token: impl Into<String>) -> Result<Self, McpCoreError> {
        let token = RedactedSecret::new(bearer_token);
        if token.expose().is_empty() {
            return Err(McpCoreError::invalid_auth("bearer token must not be empty"));
        }
        Ok(Self {
            generation,
            scheme: AuthScheme::Bearer,
            bearer_token: token,
        })
    }

    pub fn bearer_token(&self) -> &str {
        self.bearer_token.expose()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthScheme {
    Bearer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEndpointDescriptor {
    pub generation: u64,
    pub bind_address: String,
    pub port: u16,
    pub path: String,
    pub auth_generation: u64,
}

impl McpEndpointDescriptor {
    pub fn validate_loopback(&self) -> Result<(), McpCoreError> {
        let address = IpAddr::from_str(&self.bind_address).map_err(|_| {
            McpCoreError::invalid_endpoint("bind address must be a numeric loopback address")
        })?;
        if !address.is_loopback() {
            return Err(McpCoreError::invalid_endpoint(
                "MCP Core endpoint must bind to loopback",
            ));
        }
        if self.port == 0 {
            return Err(McpCoreError::invalid_endpoint(
                "MCP Core endpoint port must be assigned",
            ));
        }
        if !self.path.starts_with('/') || self.path == "/" {
            return Err(McpCoreError::invalid_endpoint(
                "MCP Core endpoint path must be a non-root absolute path",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpReadiness {
    Starting,
    Ready,
    Degraded,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReadinessStatus {
    pub state: McpReadiness,
    pub config_revision: Option<u64>,
    pub endpoint: Option<McpEndpointDescriptor>,
    pub generation: u64,
    pub diagnostic: Option<McpDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiagnostic {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl McpDiagnostic {
    pub fn redacted(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum McpControlMessage {
    ApplySnapshot { snapshot: McpConfigSnapshot },
    BootstrapAuth { auth: AuthBootstrap },
    GetStatus,
    Shutdown { reason: ShutdownReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShutdownReason {
    Requested,
    Replaced,
    FatalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum McpControlReply {
    SnapshotAccepted { revision: u64 },
    SnapshotRejected { revision: u64, error: McpCoreError },
    AuthAccepted { generation: u64 },
    Status { status: McpReadinessStatus },
    ShutdownAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCoreError {
    pub code: McpErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl McpCoreError {
    pub fn invalid_snapshot(message: impl Into<String>) -> Self {
        Self {
            code: McpErrorCode::InvalidSnapshot,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn invalid_auth(message: impl Into<String>) -> Self {
        Self {
            code: McpErrorCode::InvalidAuth,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn invalid_endpoint(message: impl Into<String>) -> Self {
        Self {
            code: McpErrorCode::InvalidEndpoint,
            message: message.into(),
            retryable: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpErrorCode {
    InvalidSnapshot,
    InvalidAuth,
    InvalidEndpoint,
    RevisionConflict,
    NotReady,
    UpstreamUnavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRevisions {
    last_accepted: Option<u64>,
}

impl SnapshotRevisions {
    pub fn new() -> Self {
        Self {
            last_accepted: None,
        }
    }

    pub fn last_accepted(&self) -> Option<u64> {
        self.last_accepted
    }

    pub fn accept(&mut self, revision: u64) -> Result<(), McpCoreError> {
        if revision == 0 {
            return Err(McpCoreError::invalid_snapshot(
                "snapshot revision must be greater than zero",
            ));
        }
        if self.last_accepted.is_some_and(|last| revision <= last) {
            return Err(McpCoreError {
                code: McpErrorCode::RevisionConflict,
                message: format!(
                    "snapshot revision {revision} is not newer than last accepted revision {}",
                    self.last_accepted.unwrap()
                ),
                retryable: false,
            });
        }
        self.last_accepted = Some(revision);
        Ok(())
    }
}

impl Default for SnapshotRevisions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(revision: u64) -> McpConfigSnapshot {
        McpConfigSnapshot {
            contract_version: MCP_CORE_CONTRACT_VERSION,
            revision,
            servers: vec![McpUpstreamServer {
                id: "local-tools".into(),
                enabled: true,
                transport: McpUpstreamTransport::Stdio {
                    command: "node".into(),
                    args: vec!["server.js".into()],
                    env: BTreeMap::from([("API_KEY".into(), RedactedSecret::new("secret"))]),
                },
            }],
        }
    }

    #[test]
    fn snapshot_serialization_is_stable() {
        let json = serde_json::to_string(&snapshot(7)).unwrap();
        assert_eq!(
            json,
            r#"{"contractVersion":1,"revision":7,"servers":[{"id":"local-tools","enabled":true,"transport":{"kind":"stdio","command":"node","args":["server.js"],"env":{"API_KEY":"secret"}}}]}"#
        );
    }

    #[test]
    fn snapshot_validation_rejects_invalid_revision_and_duplicates() {
        let mut invalid = snapshot(0);
        assert_eq!(
            invalid.validate().unwrap_err().code,
            McpErrorCode::InvalidSnapshot
        );

        invalid.revision = 1;
        invalid.servers.push(invalid.servers[0].clone());
        assert!(invalid
            .validate()
            .unwrap_err()
            .message
            .contains("duplicate"));
    }

    #[test]
    fn snapshot_validation_rejects_non_http_upstream_and_empty_stdio_command() {
        let mut invalid = snapshot(1);
        invalid.servers[0].transport = McpUpstreamTransport::Stdio {
            command: "  ".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        assert!(invalid.validate().is_err());

        invalid.servers[0].transport = McpUpstreamTransport::StreamableHttp {
            url: "file:///tmp/server".into(),
            headers: BTreeMap::new(),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn revisions_are_strictly_monotonic() {
        let mut revisions = SnapshotRevisions::new();
        assert!(revisions.accept(4).is_ok());
        assert!(revisions.accept(4).is_err());
        assert!(revisions.accept(3).is_err());
        assert!(revisions.accept(5).is_ok());
        assert_eq!(revisions.last_accepted(), Some(5));
    }

    #[test]
    fn endpoint_validation_is_loopback_only() {
        let valid = McpEndpointDescriptor {
            generation: 2,
            bind_address: "127.0.0.1".into(),
            port: 43123,
            path: "/mcp".into(),
            auth_generation: 3,
        };
        assert!(valid.validate_loopback().is_ok());

        let mut invalid = valid.clone();
        invalid.bind_address = "0.0.0.0".into();
        assert!(invalid.validate_loopback().is_err());
    }

    #[test]
    fn debug_output_redacts_secret_values() {
        let auth = AuthBootstrap::new(1, "bearer-secret").unwrap();
        let snapshot = snapshot(1);
        let auth_debug = format!("{auth:?}");
        let snapshot_debug = format!("{snapshot:?}");
        assert!(!auth_debug.contains("bearer-secret"));
        assert!(!snapshot_debug.contains("secret"));
        assert!(auth_debug.contains("<redacted>"));
        assert!(snapshot_debug.contains("<redacted>"));
    }

    #[test]
    fn control_message_serialization_is_tagged_and_secret_safe_in_debug() {
        let message = McpControlMessage::BootstrapAuth {
            auth: AuthBootstrap::new(9, "token").unwrap(),
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"kind":"bootstrapAuth","auth":{"generation":9,"scheme":"bearer","bearerToken":"token"}}"#
        );
        let debug = format!("{message:?}");
        assert!(!debug.contains("\"token\""));
        assert!(debug.contains("<redacted>"));
    }
}
