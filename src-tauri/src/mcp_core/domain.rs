//! Instance-owned MCP upstream connections and aggregation.
//!
//! This module deliberately stops at a library/service boundary. The Se host
//! owns the project control-plane document, snapshots, and secrets; the Core
//! owns only live rmcp clients, routing, limits, and connection cleanup.
//! `McpConfigSnapshot` is a runtime projection of that document, not a second
//! persisted configuration.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use futures::{stream, StreamExt};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult, Prompt,
        ReadResourceRequestParams, ReadResourceResult, Resource, Tool,
    },
    service::{RoleClient, RunningService, ServiceExt},
    transport::{
        child_process::TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
};
use tokio::{
    process::Command,
    sync::{Mutex, RwLock},
};
use tokio_util::sync::CancellationToken;

use super::builtins::{BuiltInRegistry, BuiltInRoute};
use super::{McpBuiltInConfig, McpConfigSnapshot, McpUpstreamServer, McpUpstreamTransport};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

pub type ClientService = RunningService<RoleClient, ()>;

#[derive(Debug, Clone)]
pub struct McpCoreConfig {
    pub max_concurrency: usize,
    pub max_response_bytes: usize,
    pub operation_timeout: Duration,
}

impl Default for McpCoreConfig {
    fn default() -> Self {
        Self {
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
        }
    }
}

impl McpCoreConfig {
    fn normalized(&self) -> Self {
        Self {
            max_concurrency: self.max_concurrency.max(1),
            max_response_bytes: self.max_response_bytes.max(1),
            operation_timeout: self.operation_timeout.max(Duration::from_millis(1)),
        }
    }
}

pub trait ToolPermission: Send + Sync {
    fn allows_tool(&self, server_id: &str, tool_name: &str) -> bool;
}

#[derive(Debug, Default)]
pub struct AllowAllTools;

impl ToolPermission for AllowAllTools {
    fn allows_tool(&self, _server_id: &str, _tool_name: &str) -> bool {
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct DenyListedTools {
    denied: BTreeSet<(String, String)>,
}

impl DenyListedTools {
    pub fn new(denied: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            denied: denied.into_iter().collect(),
        }
    }
}

impl ToolPermission for DenyListedTools {
    fn allows_tool(&self, server_id: &str, tool_name: &str) -> bool {
        !self
            .denied
            .contains(&(server_id.to_string(), tool_name.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    ListTools,
    CallTool,
    ListResources,
    ReadResource,
    ListPrompts,
    GetPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamFailure {
    pub server_id: String,
    pub operation: Operation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate<T> {
    pub items: Vec<T>,
    pub failures: Vec<UpstreamFailure>,
}

impl<T> Aggregate<T> {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            failures: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AggregatedTool {
    pub server_id: String,
    pub tool: Tool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AggregatedResource {
    pub server_id: String,
    pub resource: Resource,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AggregatedPrompt {
    pub server_id: String,
    pub prompt: Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamKind {
    Stdio,
    StreamableHttp,
}

struct UpstreamConnection {
    id: String,
    kind: UpstreamKind,
    client: Mutex<ClientService>,
}

impl std::fmt::Debug for UpstreamConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamConnection")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl UpstreamConnection {
    async fn close(self: Arc<Self>) {
        let mut client = self.client.lock().await;
        let _ = client.close().await;
    }
}

enum CallTarget {
    BuiltIn {
        server_id: String,
        name: String,
        provider: Arc<dyn super::builtins::BuiltInCapability>,
    },
    Upstream {
        connection: Arc<UpstreamConnection>,
        name: String,
    },
}

#[derive(Clone)]
pub struct McpCore {
    config: McpCoreConfig,
    permission: Arc<dyn ToolPermission>,
    upstreams: Arc<RwLock<BTreeMap<String, Arc<UpstreamConnection>>>>,
    builtins: Arc<RwLock<BuiltInRegistry>>,
}

impl std::fmt::Debug for McpCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCore")
            .field("config", &self.config)
            .field("upstreams", &"instance-owned")
            .field("builtins", &"instance-owned")
            .finish()
    }
}

impl Default for McpCore {
    fn default() -> Self {
        Self::new(McpCoreConfig::default(), Arc::new(AllowAllTools))
    }
}

impl McpCore {
    pub fn new(config: McpCoreConfig, permission: Arc<dyn ToolPermission>) -> Self {
        Self::new_with_builtins(config, permission, BuiltInRegistry::empty())
    }

    pub fn new_with_builtins(
        config: McpCoreConfig,
        permission: Arc<dyn ToolPermission>,
        builtins: BuiltInRegistry,
    ) -> Self {
        Self {
            config: config.normalized(),
            permission,
            upstreams: Arc::new(RwLock::new(BTreeMap::new())),
            builtins: Arc::new(RwLock::new(builtins)),
        }
    }

    pub async fn replace_builtins(&self, builtins: BuiltInRegistry) {
        *self.builtins.write().await = builtins;
    }

    pub async fn apply_built_in_config(&self, configs: &[McpBuiltInConfig]) {
        self.builtins.write().await.apply_config(configs);
    }

    pub async fn built_in_ids(&self) -> Vec<String> {
        self.builtins
            .read()
            .await
            .enabled_ids()
            .into_iter()
            .collect()
    }

    pub async fn connect_and_add(&self, server: McpUpstreamServer) -> Result<(), McpDomainError> {
        if !server.enabled {
            return Ok(());
        }
        let connection = Arc::new(connect_server(&server).await?);
        let old = self.upstreams.write().await.insert(server.id, connection);
        if let Some(old) = old {
            old.close().await;
        }
        Ok(())
    }

    pub async fn apply_snapshot(&self, snapshot: &McpConfigSnapshot) -> Result<(), McpDomainError> {
        snapshot
            .validate()
            .map_err(|error| McpDomainError::InvalidConfiguration(error.message))?;

        // Build every replacement connection before touching the live map. A
        // failed reload therefore leaves the previous last-known-good runtime
        // intact instead of partially replacing it.
        let mut replacement = BTreeMap::new();
        for server in snapshot.servers.iter().filter(|server| server.enabled) {
            match connect_server(server).await {
                Ok(connection) => {
                    replacement.insert(server.id.clone(), Arc::new(connection));
                }
                Err(error) => {
                    for (_, connection) in replacement {
                        connection.close().await;
                    }
                    return Err(error);
                }
            }
        }

        let old = {
            let mut current = self.upstreams.write().await;
            std::mem::replace(&mut *current, replacement)
        };
        for (_, connection) in old {
            connection.close().await;
        }
        Ok(())
    }

    pub async fn remove(&self, server_id: &str) -> bool {
        let removed = self.upstreams.write().await.remove(server_id);
        if let Some(connection) = removed {
            connection.close().await;
            true
        } else {
            false
        }
    }

    pub async fn upstream_ids(&self) -> Vec<String> {
        self.upstreams.read().await.keys().cloned().collect()
    }

    pub async fn list_tools(&self) -> Result<Aggregate<AggregatedTool>, McpDomainError> {
        let (mut items, colliding_ids) = {
            let registry = self.builtins.read().await;
            (
                registry.list_tools(&*self.permission),
                registry.enabled_ids(),
            )
        };
        let connections = self
            .connections()
            .await
            .into_iter()
            .filter(|connection| !colliding_ids.contains(&connection.id))
            .collect();
        let mut aggregate = self
            .fanout(connections, Operation::ListTools, |connection| async move {
                let client = connection.client.lock().await;
                let tools = client.list_all_tools().await.map_err(|_| ())?;
                Ok(tools
                    .into_iter()
                    .filter(|tool| {
                        self.permission
                            .allows_tool(&connection.id, tool.name.as_ref())
                    })
                    .map(|mut tool| {
                        let original = tool.name.to_string();
                        tool.name = format_tool_name(&connection.id, &original).into();
                        AggregatedTool {
                            server_id: connection.id.clone(),
                            tool,
                        }
                    })
                    .collect::<Vec<_>>())
            })
            .await;
        items.append(&mut aggregate.items);
        aggregate.items = items;
        self.enforce_size(&aggregate.items)?;
        Ok(aggregate)
    }

    pub async fn call_tool(
        &self,
        exposed_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        self.call_tool_with_cancel(exposed_name, arguments, CancellationToken::new())
            .await
    }

    pub async fn call_tool_with_cancel(
        &self,
        exposed_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        match self.route_call(exposed_name).await? {
            CallTarget::BuiltIn {
                server_id,
                name,
                provider,
            } => {
                if !self.permission.allows_tool(&server_id, &name)
                    || !self.builtins.read().await.tool_permitted(&server_id, &name)
                {
                    return Err(McpDomainError::PermissionDenied { server_id, name });
                }
                let request = provider.call_tool(&name, arguments, cancellation.clone());
                tokio::pin!(request);
                let result = tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Err(McpDomainError::Cancelled {
                            server_id: server_id.clone(),
                            operation: Operation::CallTool,
                        });
                    }
                    result = tokio::time::timeout(self.config.operation_timeout, &mut request) => {
                        result.map_err(|_| McpDomainError::Timeout {
                            server_id: server_id.clone(),
                            operation: Operation::CallTool,
                        })??
                    }
                };
                self.enforce_size(&result)?;
                Ok(result)
            }
            CallTarget::Upstream { connection, name } => {
                if !self.permission.allows_tool(&connection.id, &name) {
                    return Err(McpDomainError::PermissionDenied {
                        server_id: connection.id.clone(),
                        name,
                    });
                }
                let mut params = CallToolRequestParams::new(name);
                if let Some(arguments) = arguments {
                    params = params.with_arguments(arguments);
                }
                let client = connection.client.lock().await;
                let request = client.call_tool(params);
                tokio::pin!(request);
                let result = tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Err(McpDomainError::Cancelled {
                            server_id: connection.id.clone(),
                            operation: Operation::CallTool,
                        });
                    }
                    result = tokio::time::timeout(self.config.operation_timeout, &mut request) => {
                        result
                            .map_err(|_| McpDomainError::Timeout {
                                server_id: connection.id.clone(),
                                operation: Operation::CallTool,
                            })?
                            .map_err(|_| McpDomainError::UpstreamUnavailable {
                                server_id: connection.id.clone(),
                                operation: Operation::CallTool,
                            })?
                    }
                };
                self.enforce_size(&result)?;
                Ok(result)
            }
        }
    }

    pub async fn list_resources(&self) -> Result<Aggregate<AggregatedResource>, McpDomainError> {
        let connections = self.connections().await;
        let aggregate = self
            .fanout(
                connections,
                Operation::ListResources,
                |connection| async move {
                    let client = connection.client.lock().await;
                    client
                        .list_all_resources()
                        .await
                        .map(|resources| {
                            resources
                                .into_iter()
                                .map(|resource| AggregatedResource {
                                    server_id: connection.id.clone(),
                                    resource,
                                })
                                .collect()
                        })
                        .map_err(|_| ())
                },
            )
            .await;
        self.enforce_size(&aggregate.items)?;
        Ok(aggregate)
    }

    pub async fn read_resource_by_uri(
        &self,
        uri: &str,
    ) -> Result<ReadResourceResult, McpDomainError> {
        let aggregate = self.list_resources().await?;
        let server_id = aggregate
            .items
            .iter()
            .find(|item| item.resource.uri == uri)
            .map(|item| item.server_id.clone())
            .ok_or_else(|| McpDomainError::UpstreamNotFound(uri.to_owned()))?;
        self.read_resource(&server_id, uri.to_owned()).await
    }

    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: impl Into<String>,
    ) -> Result<ReadResourceResult, McpDomainError> {
        let connection = self.connection(server_id).await?;
        let client = connection.client.lock().await;
        let result = tokio::time::timeout(
            self.config.operation_timeout,
            client.read_resource(ReadResourceRequestParams::new(uri)),
        )
        .await
        .map_err(|_| McpDomainError::Timeout {
            server_id: server_id.to_string(),
            operation: Operation::ReadResource,
        })?
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: server_id.to_string(),
            operation: Operation::ReadResource,
        })?;
        self.enforce_size(&result)?;
        Ok(result)
    }

    pub async fn list_prompts(&self) -> Result<Aggregate<AggregatedPrompt>, McpDomainError> {
        let connections = self.connections().await;
        let aggregate = self
            .fanout(
                connections,
                Operation::ListPrompts,
                |connection| async move {
                    let client = connection.client.lock().await;
                    client
                        .list_all_prompts()
                        .await
                        .map(|prompts| {
                            prompts
                                .into_iter()
                                .map(|mut prompt| {
                                    let original = prompt.name.clone();
                                    prompt.name = format_tool_name(&connection.id, &original);
                                    AggregatedPrompt {
                                        server_id: connection.id.clone(),
                                        prompt,
                                    }
                                })
                                .collect()
                        })
                        .map_err(|_| ())
                },
            )
            .await;
        self.enforce_size(&aggregate.items)?;
        Ok(aggregate)
    }

    pub async fn get_prompt(
        &self,
        exposed_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<GetPromptResult, McpDomainError> {
        let (connection, original_name) = self.route_upstream_name(exposed_name).await?;
        let mut params = GetPromptRequestParams::new(original_name);
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        let client = connection.client.lock().await;
        let result = tokio::time::timeout(self.config.operation_timeout, client.get_prompt(params))
            .await
            .map_err(|_| McpDomainError::Timeout {
                server_id: connection.id.clone(),
                operation: Operation::GetPrompt,
            })?
            .map_err(|_| McpDomainError::UpstreamUnavailable {
                server_id: connection.id.clone(),
                operation: Operation::GetPrompt,
            })?;
        self.enforce_size(&result)?;
        Ok(result)
    }

    pub async fn shutdown(&self) {
        let connections = std::mem::take(&mut *self.upstreams.write().await);
        for (_, connection) in connections {
            connection.close().await;
        }
    }

    async fn connections(&self) -> Vec<Arc<UpstreamConnection>> {
        self.upstreams.read().await.values().cloned().collect()
    }

    async fn connection(&self, server_id: &str) -> Result<Arc<UpstreamConnection>, McpDomainError> {
        self.upstreams
            .read()
            .await
            .get(server_id)
            .cloned()
            .ok_or_else(|| McpDomainError::UpstreamNotFound(server_id.to_string()))
    }

    async fn route_upstream_name(
        &self,
        exposed_name: &str,
    ) -> Result<(Arc<UpstreamConnection>, String), McpDomainError> {
        self.match_upstream(exposed_name)
            .await
            .map(|(_, connection, name)| (connection, name))
            .ok_or_else(|| McpDomainError::ToolNotFound(exposed_name.to_string()))
    }

    async fn match_upstream(
        &self,
        exposed_name: &str,
    ) -> Option<(usize, Arc<UpstreamConnection>, String)> {
        self.connections()
            .await
            .into_iter()
            .filter_map(|connection| {
                let prefix = format!("{}_", connection.id);
                exposed_name
                    .strip_prefix(&prefix)
                    .filter(|name| !name.is_empty())
                    .map(|name| (prefix.len(), connection, name.to_string()))
            })
            .max_by_key(|(prefix_len, _, _)| *prefix_len)
    }

    async fn route_call(&self, exposed_name: &str) -> Result<CallTarget, McpDomainError> {
        let builtin = self.builtins.read().await.route(exposed_name);
        let upstream = self.match_upstream(exposed_name).await;
        match (builtin, upstream) {
            (None, None) => Err(McpDomainError::ToolNotFound(exposed_name.to_string())),
            (Some(route), None) => Ok(CallTarget::from_builtin(route)),
            (None, Some((_, connection, name))) => Ok(CallTarget::Upstream { connection, name }),
            (Some(route), Some((upstream_len, connection, name))) => {
                if route.prefix_len >= upstream_len {
                    Ok(CallTarget::from_builtin(route))
                } else {
                    Ok(CallTarget::Upstream { connection, name })
                }
            }
        }
    }

    async fn fanout<T, F, Fut>(
        &self,
        connections: Vec<Arc<UpstreamConnection>>,
        operation: Operation,
        call: F,
    ) -> Aggregate<T>
    where
        T: Send,
        F: Fn(Arc<UpstreamConnection>) -> Fut + Copy + Send + Sync,
        Fut: std::future::Future<Output = Result<Vec<T>, ()>> + Send,
    {
        let results = stream::iter(connections)
            .map(|connection| {
                let server_id = connection.id.clone();
                async move {
                    let result =
                        tokio::time::timeout(self.config.operation_timeout, call(connection)).await;
                    (server_id, result)
                }
            })
            .buffer_unordered(self.config.max_concurrency)
            .collect::<Vec<_>>()
            .await;

        let mut aggregate = Aggregate::empty();
        for (server_id, result) in results {
            match result {
                Ok(Ok(items)) => aggregate.items.extend(items),
                Ok(Err(())) | Err(_) => aggregate.failures.push(UpstreamFailure {
                    server_id,
                    operation,
                }),
            }
        }
        aggregate
    }

    fn enforce_size<T: serde::Serialize>(&self, value: &T) -> Result<(), McpDomainError> {
        let size = serde_json::to_vec(value)
            .map_err(|_| McpDomainError::Serialization)?
            .len();
        if size > self.config.max_response_bytes {
            return Err(McpDomainError::ResponseTooLarge {
                limit: self.config.max_response_bytes,
                actual: size,
            });
        }
        Ok(())
    }
}

async fn connect_server(server: &McpUpstreamServer) -> Result<UpstreamConnection, McpDomainError> {
    let (kind, client) = match &server.transport {
        McpUpstreamTransport::Stdio { command, args, env } => {
            let mut command_line = Command::new(command);
            command_line.args(args).kill_on_drop(true);
            command_line.stderr(Stdio::null());
            for (name, value) in env {
                command_line.env(name, value.expose());
            }
            let (transport, _) = TokioChildProcess::builder(command_line)
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?;
            (
                UpstreamKind::Stdio,
                ().serve(transport)
                    .await
                    .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?,
            )
        }
        McpUpstreamTransport::StreamableHttp { url, headers } => {
            let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
            let mut custom = HashMap::new();
            for (name, value) in headers {
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?;
                let value = reqwest::header::HeaderValue::from_str(value.expose())
                    .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?;
                custom.insert(name, value);
            }
            config = config.custom_headers(custom);
            (
                UpstreamKind::StreamableHttp,
                ().serve(StreamableHttpClientTransport::from_config(config))
                    .await
                    .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?,
            )
        }
    };
    Ok(UpstreamConnection {
        id: server.id.clone(),
        kind,
        client: Mutex::new(client),
    })
}

pub(crate) fn format_tool_name(server_id: &str, name: &str) -> String {
    format!("{server_id}_{name}")
}

impl CallTarget {
    fn from_builtin(route: BuiltInRoute) -> Self {
        Self::BuiltIn {
            server_id: route.server_id,
            name: route.name,
            provider: route.provider,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpDomainError {
    InvalidConfiguration(String),
    ConnectFailed(String),
    UpstreamNotFound(String),
    ToolNotFound(String),
    InvalidArguments {
        server_id: String,
        name: String,
    },
    PermissionDenied {
        server_id: String,
        name: String,
    },
    Timeout {
        server_id: String,
        operation: Operation,
    },
    UpstreamUnavailable {
        server_id: String,
        operation: Operation,
    },
    Cancelled {
        server_id: String,
        operation: Operation,
    },
    ResponseTooLarge {
        limit: usize,
        actual: usize,
    },
    Serialization,
}

impl std::fmt::Display for McpDomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(f, "invalid MCP configuration: {message}")
            }
            Self::ConnectFailed(server_id) => write!(f, "failed to connect upstream {server_id}"),
            Self::UpstreamNotFound(server_id) => write!(f, "upstream {server_id} not found"),
            Self::ToolNotFound(name) => write!(f, "tool or prompt {name} not found"),
            Self::InvalidArguments { server_id, name } => {
                write!(f, "invalid arguments for {server_id}:{name}")
            }
            Self::PermissionDenied { server_id, name } => {
                write!(f, "permission denied for {server_id}:{name}")
            }
            Self::Timeout {
                server_id,
                operation,
            } => {
                write!(f, "upstream {server_id} timed out during {operation:?}")
            }
            Self::UpstreamUnavailable {
                server_id,
                operation,
            } => {
                write!(f, "upstream {server_id} failed during {operation:?}")
            }
            Self::Cancelled {
                server_id,
                operation,
            } => {
                write!(
                    f,
                    "upstream {server_id} call cancelled during {operation:?}"
                )
            }
            Self::ResponseTooLarge { limit, actual } => {
                write!(f, "MCP response is {actual} bytes; limit is {limit}")
            }
            Self::Serialization => f.write_str("failed to serialize MCP response"),
        }
    }
}

impl std::error::Error for McpDomainError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_names_use_server_prefix_and_longest_match() {
        assert_eq!(format_tool_name("local", "search"), "local_search");
        assert_eq!(
            format_tool_name("local_tools", "search"),
            "local_tools_search"
        );
    }

    #[test]
    fn deny_list_is_instance_owned_and_exact() {
        let permissions = DenyListedTools::new([(String::from("a"), String::from("blocked"))]);
        assert!(!permissions.allows_tool("a", "blocked"));
        assert!(permissions.allows_tool("a", "other"));
        assert!(permissions.allows_tool("b", "blocked"));
    }

    #[test]
    fn config_normalizes_zero_limits() {
        let config = McpCoreConfig {
            max_concurrency: 0,
            max_response_bytes: 0,
            operation_timeout: Duration::ZERO,
        }
        .normalized();
        assert_eq!(config.max_concurrency, 1);
        assert_eq!(config.max_response_bytes, 1);
        assert!(config.operation_timeout > Duration::ZERO);
    }
}
