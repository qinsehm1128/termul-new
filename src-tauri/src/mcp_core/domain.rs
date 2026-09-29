//! Instance-owned MCP upstream connections and aggregation.
//!
//! This module deliberately stops at a library/service boundary. The Se host
//! owns the project control-plane document, snapshots, and secrets; the Core
//! owns only live rmcp clients, routing, limits, and connection cleanup.
//! `McpConfigSnapshot` is a runtime projection of that document, not a second
//! persisted configuration.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    hash::{Hash, Hasher},
    path::Path,
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
    service::{Peer, RoleClient, RunningService, ServiceExt},
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
use super::facade::{McpFacadeCatalog, McpFacadeFailure, McpFacadeFailureCode, McpFacadeTool};
use super::{McpBuiltInConfig, McpConfigSnapshot, McpUpstreamServer, McpUpstreamTransport};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_CONCURRENCY: usize = 8;

pub type ClientService = RunningService<RoleClient, ()>;

#[derive(Debug, Clone)]
pub struct McpCoreConfig {
    pub max_concurrency: usize,
    pub max_response_bytes: usize,
    /// Listing and reading: fan-out steps that should fail fast.
    pub operation_timeout: Duration,
    /// Tool calls: real work (crawls, builds, queries) that may run long.
    pub call_timeout: Duration,
    /// Starting one upstream (`npx` may download a package first).
    pub connect_timeout: Duration,
}

impl Default for McpCoreConfig {
    fn default() -> Self {
        Self {
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }
}

impl McpCoreConfig {
    fn normalized(&self) -> Self {
        let floor = Duration::from_millis(1);
        Self {
            max_concurrency: self.max_concurrency.max(1),
            max_response_bytes: self.max_response_bytes.max(1),
            operation_timeout: self.operation_timeout.max(floor),
            call_timeout: self.call_timeout.max(floor),
            connect_timeout: self.connect_timeout.max(floor),
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

/// One routable server as agents see it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSummary {
    pub name: String,
    pub description: String,
    pub built_in: bool,
}

const ADVERTISED_DESCRIPTION_MAX_CHARS: usize = 240;

/// What the upstream says about itself in `initialize`, trimmed to one short
/// paragraph so it fits in a tool description.
fn advertised_description(peer: &Peer<RoleClient>) -> String {
    let Some(info) = peer.peer_info() else {
        return String::new();
    };
    let text = info
        .server_info
        .as_ref()
        .and_then(|server| server.description.as_deref())
        .filter(|text| !text.trim().is_empty())
        .or(info.instructions.as_deref())
        .unwrap_or_default();
    let paragraph = text.trim().split("\n\n").next().unwrap_or_default();
    let flat = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= ADVERTISED_DESCRIPTION_MAX_CHARS {
        flat
    } else {
        let cut = flat
            .chars()
            .take(ADVERTISED_DESCRIPTION_MAX_CHARS)
            .collect::<String>();
        format!("{}…", cut.trim_end())
    }
}

/// Live state of one configured upstream, keyed by its configuration id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamStatus {
    pub id: String,
    pub name: String,
    pub state: UpstreamState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpstreamState {
    Disabled,
    Connecting,
    Connected,
    Failed,
}

struct UpstreamConnection {
    /// Route name agents use; the key of `McpCore::upstreams`.
    id: String,
    server: McpUpstreamServer,
    kind: UpstreamKind,
    /// Requests go through the peer so one slow call never queues the next.
    peer: Peer<RoleClient>,
    /// Held only to close the connection.
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
    statuses: Arc<RwLock<Vec<UpstreamStatus>>>,
    /// Curated "what is this server for" text, keyed by config or built-in id.
    descriptions: Arc<RwLock<BTreeMap<String, String>>>,
    builtins: Arc<RwLock<BuiltInRegistry>>,
    credential_scope: Arc<RwLock<Option<std::path::PathBuf>>>,
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
            statuses: Arc::new(RwLock::new(Vec::new())),
            descriptions: Arc::new(RwLock::new(BTreeMap::new())),
            builtins: Arc::new(RwLock::new(builtins)),
            credential_scope: Arc::new(RwLock::new(None)),
        }
    }

    /// Bind OAuth credential reads to one canonical project root.
    ///
    /// The binding is replaced atomically before a project-scoped snapshot is
    /// applied. A core with no binding cannot open an OAuth upstream.
    pub async fn set_credential_scope(&self, project_root: Option<std::path::PathBuf>) {
        *self.credential_scope.write().await = project_root;
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
        let scope = self.credential_scope.read().await.clone();
        let connection = Arc::new(
            connect_server(&server, scope.as_deref())
                .await
                .map_err(|_| McpDomainError::ConnectFailed(server.id.clone()))?,
        );
        let old = self
            .upstreams
            .write()
            .await
            .insert(connection.id.clone(), connection);
        if let Some(old) = old {
            old.close().await;
        }
        Ok(())
    }

    pub async fn apply_snapshot(&self, snapshot: &McpConfigSnapshot) -> Result<(), McpDomainError> {
        snapshot
            .validate()
            .map_err(|error| McpDomainError::InvalidConfiguration(error.message))?;

        // Unchanged upstreams keep their live connection; new or changed ones
        // connect side by side. One that fails is reported in its status and
        // left out, so it never takes the healthy ones down with it.
        let scope = self.credential_scope.read().await.clone();
        let live = self.upstreams.read().await.clone();
        let mut next = BTreeMap::new();
        let mut statuses = Vec::with_capacity(snapshot.servers.len());
        let mut pending = Vec::new();
        for server in &snapshot.servers {
            let status = |state| UpstreamStatus {
                id: server.id.clone(),
                name: server.route_name().to_owned(),
                state,
                error: None,
            };
            if !server.enabled {
                statuses.push(status(UpstreamState::Disabled));
            } else if let Some(connection) = live
                .get(server.route_name())
                .filter(|connection| connection.server == *server)
            {
                next.insert(connection.id.clone(), Arc::clone(connection));
                statuses.push(status(UpstreamState::Connected));
            } else {
                statuses.push(status(UpstreamState::Connecting));
                pending.push(server.clone());
            }
        }
        *self.statuses.write().await = statuses.clone();

        let timeout = self.config.connect_timeout;
        let results = stream::iter(pending)
            .map(|server| {
                let scope = scope.clone();
                async move {
                    let result =
                        tokio::time::timeout(timeout, connect_server(&server, scope.as_deref()))
                            .await
                            .unwrap_or_else(|_| Err("timed out while starting".to_owned()));
                    (server.id, result)
                }
            })
            .buffer_unordered(CONNECT_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for (id, result) in results {
            let Some(status) = statuses.iter_mut().find(|status| status.id == id) else {
                continue;
            };
            match result {
                Ok(connection) => {
                    status.state = UpstreamState::Connected;
                    next.insert(connection.id.clone(), Arc::new(connection));
                }
                Err(message) => {
                    status.state = UpstreamState::Failed;
                    status.error = Some(message);
                }
            }
        }

        let old = std::mem::replace(&mut *self.upstreams.write().await, next.clone());
        *self.statuses.write().await = statuses;
        for (name, connection) in old {
            let kept = next
                .get(&name)
                .is_some_and(|current| Arc::ptr_eq(current, &connection));
            if !kept {
                connection.close().await;
            }
        }
        Ok(())
    }

    /// Replace the curated descriptions (config id or built-in id → text).
    pub async fn set_descriptions(&self, descriptions: BTreeMap<String, String>) {
        *self.descriptions.write().await = descriptions;
    }

    /// Every server an agent can route to right now: enabled built-ins, then
    /// connected upstreams. Needs no upstream round trip.
    pub async fn server_summaries(&self) -> Vec<ServerSummary> {
        let descriptions = self.descriptions.read().await.clone();
        let mut summaries = self.builtins.read().await.summaries();
        for summary in &mut summaries {
            if let Some(text) = descriptions.get(&summary.name) {
                summary.description = text.clone();
            }
        }
        let taken = summaries
            .iter()
            .map(|summary| summary.name.clone())
            .collect::<BTreeSet<_>>();
        for connection in self.connections().await {
            if taken.contains(&connection.id) {
                continue;
            }
            let description = descriptions
                .get(&connection.server.id)
                .cloned()
                .unwrap_or_else(|| advertised_description(&connection.peer));
            summaries.push(ServerSummary {
                name: connection.id.clone(),
                description,
                built_in: false,
            });
        }
        summaries
    }

    /// Tools of one server under their own (unprefixed) names. Only that
    /// server is asked.
    pub async fn list_server_tools(&self, name: &str) -> Result<Vec<Tool>, McpDomainError> {
        let builtins = self.builtins.read().await;
        if builtins.enabled_ids().contains(name) {
            let prefix = format!("{name}_");
            return Ok(builtins
                .list_tools(&*self.permission)
                .into_iter()
                .filter(|item| item.server_id == name)
                .map(|mut item| {
                    if let Some(original) = item.tool.name.strip_prefix(&prefix) {
                        item.tool.name = original.to_owned().into();
                    }
                    item.tool
                })
                .collect());
        }
        drop(builtins);
        let connection = self.connection(name).await?;
        let tools = tokio::time::timeout(
            self.config.operation_timeout,
            connection.peer.list_all_tools(),
        )
        .await
        .map_err(|_| McpDomainError::Timeout {
            server_id: name.to_owned(),
            operation: Operation::ListTools,
        })?
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: name.to_owned(),
            operation: Operation::ListTools,
        })?;
        let tools = tools
            .into_iter()
            .filter(|tool| {
                self.permission.allows_tool(name, tool.name.as_ref())
                    && connection.server.policy.allows(tool.name.as_ref())
            })
            .collect::<Vec<_>>();
        self.enforce_size(&tools)?;
        Ok(tools)
    }

    /// State of one routable server by name, built-ins included.
    pub async fn server_status(&self, name: &str) -> Option<UpstreamStatus> {
        if self.builtins.read().await.enabled_ids().contains(name) {
            return Some(UpstreamStatus {
                id: name.to_owned(),
                name: name.to_owned(),
                state: UpstreamState::Connected,
                error: None,
            });
        }
        let recorded = self
            .statuses
            .read()
            .await
            .iter()
            .find(|status| status.name == name)
            .cloned();
        match recorded {
            Some(status) => Some(status),
            // Added directly, outside a snapshot.
            None => self
                .connection(name)
                .await
                .ok()
                .map(|connection| UpstreamStatus {
                    id: connection.server.id.clone(),
                    name: name.to_owned(),
                    state: UpstreamState::Connected,
                    error: None,
                }),
        }
    }

    /// Per-upstream state from the last applied snapshot, in config order.
    pub async fn upstream_statuses(&self) -> Vec<UpstreamStatus> {
        self.statuses.read().await.clone()
    }

    /// Record upstreams a tolerant snapshot left out, so they show up as
    /// failed instead of silently missing.
    pub async fn record_rejected(&self, rejected: impl IntoIterator<Item = UpstreamStatus>) {
        self.statuses.write().await.extend(rejected);
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
                let tools = connection.peer.list_all_tools().await.map_err(|_| ())?;
                Ok(tools
                    .into_iter()
                    .filter(|tool| {
                        self.permission
                            .allows_tool(&connection.id, tool.name.as_ref())
                            && connection.server.policy.allows(tool.name.as_ref())
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

    pub async fn list_facade_catalogs(&self) -> Result<Vec<McpFacadeCatalog>, McpDomainError> {
        let aggregate = self.list_tools().await?;
        let mut grouped: BTreeMap<String, McpFacadeCatalog> = BTreeMap::new();
        for server_id in self
            .built_in_ids()
            .await
            .into_iter()
            .chain(self.upstream_ids().await)
        {
            grouped
                .entry(server_id.clone())
                .or_insert_with(|| McpFacadeCatalog {
                    server_id,
                    catalog_revision: 1,
                    description: String::new(),
                    when_to_use: Vec::new(),
                    avoid_when: Vec::new(),
                    tools: Vec::new(),
                    failures: Vec::new(),
                });
        }
        for item in aggregate.items {
            let server_id = item.server_id.clone();
            let catalog = grouped
                .entry(server_id.clone())
                .or_insert_with(|| McpFacadeCatalog {
                    server_id,
                    catalog_revision: 1,
                    description: String::new(),
                    when_to_use: Vec::new(),
                    avoid_when: Vec::new(),
                    tools: Vec::new(),
                    failures: Vec::new(),
                });
            let annotations = item.tool.annotations.as_ref();
            let read_only = annotations
                .and_then(|value| value.read_only_hint)
                .unwrap_or(false);
            let destructive = annotations
                .and_then(|value| value.destructive_hint)
                .unwrap_or(!read_only);
            catalog.tools.push(McpFacadeTool {
                name: item
                    .tool
                    .name
                    .strip_prefix(&format!("{}_", catalog.server_id))
                    .unwrap_or(item.tool.name.as_ref())
                    .to_string(),
                description: item
                    .tool
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_string(),
                when_to_use: String::new(),
                avoid_when: String::new(),
                input_schema: Some(serde_json::Value::Object((*item.tool.input_schema).clone())),
                read_only,
                destructive,
                confirmation_required: destructive,
                allowed: true,
            });
        }
        for failure in aggregate.failures {
            let catalog =
                grouped
                    .entry(failure.server_id.clone())
                    .or_insert_with(|| McpFacadeCatalog {
                        server_id: failure.server_id.clone(),
                        catalog_revision: 1,
                        description: String::new(),
                        when_to_use: Vec::new(),
                        avoid_when: Vec::new(),
                        tools: Vec::new(),
                        failures: Vec::new(),
                    });
            catalog.failures.push(McpFacadeFailure {
                server_id: failure.server_id,
                code: match failure.operation {
                    Operation::ListTools => McpFacadeFailureCode::UpstreamUnavailable,
                    Operation::CallTool => McpFacadeFailureCode::UpstreamUnavailable,
                    _ => McpFacadeFailureCode::SchemaInvalid,
                },
                message: "MCP upstream did not provide a complete tool catalog".into(),
            });
        }
        for catalog in grouped.values_mut() {
            catalog.catalog_revision = facade_catalog_revision(catalog);
        }
        Ok(grouped.into_values().collect())
    }

    pub async fn call_facade_tool_with_cancel(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        catalog_revision: Option<u64>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        // A facade call is direct when no revision is pinned. A pinned call
        // validates the current catalog so stale clients fail before dispatch.
        if let Some(requested_revision) = catalog_revision {
            let catalogs = self.list_facade_catalogs().await?;
            let catalog = catalogs
                .iter()
                .find(|catalog| catalog.server_id == server_id)
                .ok_or_else(|| McpDomainError::UpstreamNotFound(server_id.to_string()))?;
            super::facade::require_current_catalog_revision(
                Some(requested_revision),
                catalog.catalog_revision,
            )
            .map_err(|_| McpDomainError::InvalidConfiguration("stale MCP catalog".into()))?;
            if !catalog
                .tools
                .iter()
                .any(|tool| tool.name == tool_name && tool.allowed)
            {
                return Err(McpDomainError::PermissionDenied {
                    server_id: server_id.to_string(),
                    name: tool_name.to_string(),
                });
            }
        }
        self.call_tool_with_cancel(
            &format_tool_name(server_id, tool_name),
            arguments,
            cancellation,
        )
        .await
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
        let target = self.route_call(exposed_name).await?;
        self.dispatch(target, arguments, cancellation).await
    }

    /// Call `tool` on the server routed as `server`, without going through
    /// prefix matching of an exposed name.
    pub async fn call_server_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        let builtin = self
            .builtins
            .read()
            .await
            .route(&format_tool_name(server, tool))
            .filter(|route| route.server_id == server);
        let target = match builtin {
            Some(route) => CallTarget::from_builtin(route),
            None => CallTarget::Upstream {
                connection: self.connection(server).await?,
                name: tool.to_owned(),
            },
        };
        self.dispatch(target, arguments, cancellation).await
    }

    async fn dispatch(
        &self,
        target: CallTarget,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        match target {
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
                    result = tokio::time::timeout(self.config.call_timeout, &mut request) => {
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
                if !self.permission.allows_tool(&connection.id, &name)
                    || !connection.server.policy.allows(&name)
                {
                    return Err(McpDomainError::PermissionDenied {
                        server_id: connection.id.clone(),
                        name,
                    });
                }
                let mut params = CallToolRequestParams::new(name);
                if let Some(arguments) = arguments {
                    params = params.with_arguments(arguments);
                }
                let request = connection.peer.call_tool(params);
                tokio::pin!(request);
                let result = tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Err(McpDomainError::Cancelled {
                            server_id: connection.id.clone(),
                            operation: Operation::CallTool,
                        });
                    }
                    result = tokio::time::timeout(self.config.call_timeout, &mut request) => {
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
                    connection
                        .peer
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
        let result = tokio::time::timeout(
            self.config.operation_timeout,
            connection
                .peer
                .read_resource(ReadResourceRequestParams::new(uri)),
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
                    connection
                        .peer
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
        let result = tokio::time::timeout(
            self.config.operation_timeout,
            connection.peer.get_prompt(params),
        )
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

#[cfg(not(target_os = "windows"))]
fn resolve_stdio_command_in_path(command: &str, path: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    if command.contains('/') {
        return Some(command.to_string());
    }
    for directory in path.split(':').filter(|segment| !segment.is_empty()) {
        let candidate = std::path::Path::new(directory).join(command);
        if let Ok(metadata) = std::fs::metadata(&candidate) {
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

async fn connect_server(
    server: &McpUpstreamServer,
    credential_scope: Option<&Path>,
) -> Result<UpstreamConnection, String> {
    let (kind, client) = match &server.transport {
        McpUpstreamTransport::Stdio { command, args, env } => {
            let mut env_map = HashMap::new();
            for (name, value) in env {
                env_map.insert(name.clone(), value.expose().to_owned());
            }
            crate::pty::env_refresh::apply_fresh_path(&mut env_map);

            let resolved = crate::pty::manager::resolve_spawn_program(command).ok();
            let mut program = resolved
                .as_ref()
                .map(|value| value.program.clone())
                .unwrap_or_else(|| command.clone());
            let mut command_args = resolved.map(|value| value.prepend_args).unwrap_or_default();
            command_args.extend(args.iter().cloned());
            #[cfg(not(target_os = "windows"))]
            if let Some(path) = env_map.get("PATH") {
                if let Some(path_program) = resolve_stdio_command_in_path(&program, path) {
                    program = path_program;
                }
            }

            let mut command_line = Command::new(program);
            command_line.args(command_args).kill_on_drop(true);
            command_line.stderr(Stdio::null());
            for (name, value) in env_map {
                command_line.env(name, value);
            }
            let (transport, _) = TokioChildProcess::builder(command_line)
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("could not start `{command}`: {error}"))?;
            (
                UpstreamKind::Stdio,
                ().serve(transport)
                    .await
                    .map_err(|error| format!("MCP handshake failed: {error}"))?,
            )
        }
        McpUpstreamTransport::StreamableHttp {
            url,
            headers,
            oauth,
        } => {
            if oauth
                .as_ref()
                .is_some_and(|value| value.auth_mode == crate::mcp_core::oauth::McpAuthMode::OAuth)
            {
                let header_values = headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose().to_owned()))
                    .collect::<BTreeMap<_, _>>();
                let Some(project_root) = credential_scope else {
                    return Err("OAuth credentials are not available to this runtime".into());
                };
                let transport = crate::mcp_core::oauth::oauth_transport(
                    url.as_str(),
                    project_root,
                    &server.id,
                    oauth.as_ref().expect("oauth config"),
                    &header_values,
                )
                .await
                .map_err(|_| "OAuth authorization is missing or expired".to_owned())?;
                (
                    UpstreamKind::StreamableHttp,
                    ().serve(transport)
                        .await
                        .map_err(|error| format!("MCP handshake failed: {error}"))?,
                )
            } else {
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
                // Hosted MCP services may intentionally omit Mcp-Session-Id and
                // serve stateless HTTP requests. The router client accepts this;
                // the Core aggregator must do the same for the same upstream.
                config.allow_stateless = true;
                let mut custom = HashMap::new();
                for (name, value) in headers {
                    let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| format!("header name `{name}` is invalid"))?;
                    let value = reqwest::header::HeaderValue::from_str(value.expose())
                        .map_err(|_| format!("header `{name}` has an invalid value"))?;
                    custom.insert(name, value);
                }
                config = config.custom_headers(custom);
                (
                    UpstreamKind::StreamableHttp,
                    ().serve(StreamableHttpClientTransport::from_config(config))
                        .await
                        .map_err(|error| format!("MCP handshake failed: {error}"))?,
                )
            }
        }
    };
    Ok(UpstreamConnection {
        id: server.route_name().to_owned(),
        server: server.clone(),
        kind,
        peer: client.peer().clone(),
        client: Mutex::new(client),
    })
}

pub(crate) fn format_tool_name(server_id: &str, name: &str) -> String {
    format!("{server_id}_{name}")
}

fn facade_catalog_revision(catalog: &McpFacadeCatalog) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    catalog.server_id.hash(&mut hasher);
    for tool in &catalog.tools {
        tool.name.hash(&mut hasher);
        tool.description.hash(&mut hasher);
        tool.read_only.hash(&mut hasher);
        tool.destructive.hash(&mut hasher);
        tool.confirmation_required.hash(&mut hasher);
    }
    let revision = hasher.finish() & ((1_u64 << 53) - 1);
    revision.max(1)
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
            call_timeout: Duration::ZERO,
            connect_timeout: Duration::ZERO,
        }
        .normalized();
        assert_eq!(config.max_concurrency, 1);
        assert_eq!(config.max_response_bytes, 1);
        assert!(config.operation_timeout > Duration::ZERO);
        assert!(config.call_timeout > Duration::ZERO);
        assert!(config.connect_timeout > Duration::ZERO);
    }
}
