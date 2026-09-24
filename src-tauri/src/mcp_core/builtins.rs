//! Termul-owned MCP capabilities aggregated beside external upstreams.
//!
//! Built-ins are not a second persisted registry. Enablement and per-tool
//! policy live on the project control-plane document; this module only holds
//! live providers. Session/project memory always re-derives
//! [`crate::memory_index::scope::ProjectFence`] on the calling thread of each
//! tool invocation and never logs arguments, queries, or paths.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use rmcp::model::{CallToolResult, ContentBlock, Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::domain::{format_tool_name, AggregatedTool, McpDomainError, Operation, ToolPermission};
use super::{McpBuiltInConfig, McpCapabilityPolicy, BUILTIN_PROJECT_SCOPE, BUILTIN_SESSION_MEMORY};
use crate::memory_index::{
    paths::{INDEX_DIR_NAME, INDEX_FILE_NAME},
    scope::ProjectFence,
    service::{MemoryIndexService, MemorySearchRequest},
    store::MemoryStore,
    MemoryIndexError, ERR_OUT_OF_SCOPE, ERR_PROJECT_ROOT_INVALID,
};

pub const TOOL_MEMORY_SEARCH: &str = "memory_search";
pub const TOOL_MEMORY_SESSION_LIST: &str = "memory_session_list";
pub const TOOL_MEMORY_SESSION_GET: &str = "memory_session_get";
pub const TOOL_MEMORY_SESSION_MESSAGES: &str = "memory_session_messages";
pub const TOOL_PROJECT_BOUNDARY: &str = "project_boundary";
pub const TOOL_MEMORY_PROJECTS: &str = "memory_projects";

const DEFAULT_WINDOW_BEFORE: usize = 10;
const DEFAULT_WINDOW_AFTER: usize = 10;
const DEFAULT_WINDOW_MAX_CHARS: usize = 20_000;
const KEY_ALLOWED: &str = "abcdefghijklmnopqrstuvwxyz0123456789-";

/// Live built-in MCP provider. Implementations must not log arguments.
#[async_trait]
pub trait BuiltInCapability: Send + Sync {
    fn id(&self) -> &str;
    fn tools(&self) -> Vec<Tool>;
    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError>;
}

struct RegisteredBuiltIn {
    provider: Arc<dyn BuiltInCapability>,
    enabled: bool,
    policy: McpCapabilityPolicy,
}

/// Instance-owned set of Termul built-in providers.
pub struct BuiltInRegistry {
    entries: BTreeMap<String, RegisteredBuiltIn>,
}

impl std::fmt::Debug for BuiltInRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltInRegistry")
            .field("ids", &self.entries.keys().collect::<Vec<_>>())
            .field("enabled", &self.enabled_ids())
            .finish()
    }
}

impl Default for BuiltInRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

impl BuiltInRegistry {
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Session-memory and project-scope providers backed by one host-private index.
    ///
    /// `bound_project_root` pins tools to a single project (ACP session). Open
    /// plane callers select only an indexed `project` key per call; raw paths
    /// are never accepted from an unbound client.
    pub fn memory_backed(
        memory: Arc<MemoryIndexService>,
        bound_project_root: Option<PathBuf>,
    ) -> Self {
        let scope = MemoryScope {
            memory,
            bound_project_root,
        };
        let mut registry = Self::empty();
        registry.register(Arc::new(SessionMemoryProvider {
            scope: scope.clone(),
        }));
        registry.register(Arc::new(ProjectScopeProvider { scope }));
        registry
    }

    pub fn register(&mut self, provider: Arc<dyn BuiltInCapability>) -> &mut Self {
        let id = provider.id().to_string();
        self.entries.insert(
            id,
            RegisteredBuiltIn {
                provider,
                enabled: true,
                policy: McpCapabilityPolicy::default(),
            },
        );
        self
    }

    pub fn apply_config(&mut self, configs: &[McpBuiltInConfig]) {
        for config in configs {
            if let Some(entry) = self.entries.get_mut(&config.id) {
                entry.enabled = config.enabled;
                entry.policy = config.policy.clone();
            }
        }
    }

    pub fn enabled_ids(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn list_tools(&self, permission: &dyn ToolPermission) -> Vec<AggregatedTool> {
        let mut items = Vec::new();
        for (id, entry) in &self.entries {
            if !entry.enabled {
                continue;
            }
            for mut tool in entry.provider.tools() {
                let original = tool.name.to_string();
                if !policy_allows(&entry.policy, &original) {
                    continue;
                }
                if !permission.allows_tool(id, &original) {
                    continue;
                }
                tool.name = format_tool_name(id, &original).into();
                items.push(AggregatedTool {
                    server_id: id.clone(),
                    tool,
                });
            }
        }
        items
    }

    pub fn tool_permitted(&self, server_id: &str, tool_name: &str) -> bool {
        self.entries
            .get(server_id)
            .is_some_and(|entry| entry.enabled && policy_allows(&entry.policy, tool_name))
    }

    pub fn route(&self, exposed_name: &str) -> Option<BuiltInRoute> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .filter_map(|(id, entry)| {
                let prefix = format!("{id}_");
                exposed_name
                    .strip_prefix(&prefix)
                    .filter(|name| !name.is_empty())
                    .map(|name| BuiltInRoute {
                        prefix_len: prefix.len(),
                        server_id: id.clone(),
                        name: name.to_string(),
                        provider: Arc::clone(&entry.provider),
                    })
            })
            .max_by_key(|route| route.prefix_len)
    }
}

/// Match produced by [`BuiltInRegistry::route`].
pub struct BuiltInRoute {
    pub prefix_len: usize,
    pub server_id: String,
    pub name: String,
    pub provider: Arc<dyn BuiltInCapability>,
}

fn policy_allows(policy: &McpCapabilityPolicy, tool_name: &str) -> bool {
    if policy.deny_tools.iter().any(|denied| denied == tool_name) {
        return false;
    }
    match &policy.allow_tools {
        Some(allow) => allow.iter().any(|name| name == tool_name),
        None => true,
    }
}

#[derive(Clone)]
struct MemoryScope {
    memory: Arc<MemoryIndexService>,
    bound_project_root: Option<PathBuf>,
}

enum ScopeResolveError {
    Missing,
    Denied,
}

impl MemoryScope {
    fn resolve(
        &self,
        project_root: Option<&str>,
        project_key: Option<&str>,
    ) -> Result<PathBuf, ScopeResolveError> {
        let requested = self.requested_root(project_root, project_key)?;
        match (&self.bound_project_root, requested) {
            (Some(bound), None) => fence_root(bound),
            (Some(bound), Some(requested)) => {
                let bound_root = fence_root(bound)?;
                let requested_root = fence_root(&requested)?;
                let bound_key = ProjectFence::single(&bound_root)
                    .map_err(|_| ScopeResolveError::Denied)?
                    .namespace_key();
                let requested_key = ProjectFence::single(&requested_root)
                    .map_err(|_| ScopeResolveError::Denied)?
                    .namespace_key();
                if bound_key != requested_key {
                    return Err(ScopeResolveError::Denied);
                }
                Ok(bound_root)
            }
            (None, Some(requested)) => fence_root(&requested),
            (None, None) => Err(ScopeResolveError::Missing),
        }
    }

    fn requested_root(
        &self,
        project_root: Option<&str>,
        project_key: Option<&str>,
    ) -> Result<Option<PathBuf>, ScopeResolveError> {
        let path = project_root
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let key = project_key.map(str::trim).filter(|value| !value.is_empty());
        match (path, key) {
            (None, None) => Ok(None),
            (Some(path), None) => Ok(Some(path)),
            (None, Some(key)) => Ok(Some(self.root_for_key(key)?)),
            (Some(path), Some(key)) => {
                let from_key = self.root_for_key(key)?;
                let path_fence =
                    ProjectFence::single(&path).map_err(|_| ScopeResolveError::Denied)?;
                let key_fence =
                    ProjectFence::single(&from_key).map_err(|_| ScopeResolveError::Denied)?;
                if path_fence.namespace_key() != key_fence.namespace_key() {
                    return Err(ScopeResolveError::Denied);
                }
                Ok(Some(from_key))
            }
        }
    }

    fn root_for_key(&self, key: &str) -> Result<PathBuf, ScopeResolveError> {
        if !is_namespace_key(key) {
            return Err(ScopeResolveError::Denied);
        }
        let database = self
            .memory
            .state_root()
            .join(INDEX_DIR_NAME)
            .join(key)
            .join(INDEX_FILE_NAME);
        let stored =
            MemoryStore::stored_project_root(&database).ok_or(ScopeResolveError::Denied)?;
        Ok(PathBuf::from(stored))
    }
}

fn fence_root(path: &Path) -> Result<PathBuf, ScopeResolveError> {
    let fence = ProjectFence::single(path).map_err(|error| map_fence_error(&error))?;
    Ok(fence.project().canonical().to_path_buf())
}

fn map_fence_error(error: &MemoryIndexError) -> ScopeResolveError {
    match error.code {
        ERR_OUT_OF_SCOPE | ERR_PROJECT_ROOT_INVALID => ScopeResolveError::Denied,
        _ => ScopeResolveError::Denied,
    }
}

fn scope_error(server_id: &str, name: &str, error: ScopeResolveError) -> McpDomainError {
    match error {
        ScopeResolveError::Missing => McpDomainError::InvalidArguments {
            server_id: server_id.to_string(),
            name: name.to_string(),
        },
        ScopeResolveError::Denied => McpDomainError::PermissionDenied {
            server_id: server_id.to_string(),
            name: name.to_string(),
        },
    }
}

fn is_namespace_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|character| KEY_ALLOWED.contains(character))
}

fn read_only_tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
    Tool::new(
        name,
        description,
        Arc::new(schema.as_object().cloned().unwrap_or_default()),
    )
    .with_annotations(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, McpDomainError> {
    let text = serde_json::to_string_pretty(value).map_err(|_| McpDomainError::Serialization)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

fn run_blocking<T, F>(server_id: &str, name: &str, work: F) -> Result<T, McpDomainError>
where
    T: Send,
    F: FnOnce() -> Result<T, MemoryIndexError> + Send,
{
    work().map_err(|error| match error.code {
        ERR_OUT_OF_SCOPE | ERR_PROJECT_ROOT_INVALID => McpDomainError::PermissionDenied {
            server_id: server_id.to_string(),
            name: name.to_string(),
        },
        _ => McpDomainError::UpstreamUnavailable {
            server_id: server_id.to_string(),
            operation: Operation::CallTool,
        },
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    server_id: &str,
    name: &str,
    arguments: Option<Map<String, Value>>,
) -> Result<T, McpDomainError> {
    let value = match arguments {
        Some(arguments) => Value::Object(arguments),
        None => json!({}),
    };
    serde_json::from_value(value).map_err(|_| McpDomainError::InvalidArguments {
        server_id: server_id.to_string(),
        name: name.to_string(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeArgs {
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    agents: Vec<String>,
    #[serde(default)]
    include_unscoped: bool,
    #[serde(default)]
    include_stale: bool,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionListArgs {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    agents: Vec<String>,
    #[serde(default)]
    include_unscoped: bool,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionGetArgs {
    session_key: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    include_stale: bool,
    #[serde(default)]
    include_unscoped: bool,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMessagesArgs {
    session_key: String,
    #[serde(default)]
    anchor_ordinal: Option<u32>,
    #[serde(default)]
    before: Option<usize>,
    #[serde(default)]
    after: Option<usize>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

struct SessionMemoryProvider {
    scope: MemoryScope,
}

struct ProjectScopeProvider {
    scope: MemoryScope,
}

#[async_trait]
impl BuiltInCapability for SessionMemoryProvider {
    fn id(&self) -> &str {
        BUILTIN_SESSION_MEMORY
    }

    fn tools(&self) -> Vec<Tool> {
        let project_fields = json!({
            "projectRoot": { "type": "string" },
            "project": { "type": "string" }
        });
        vec![
            read_only_tool(
                TOOL_MEMORY_SEARCH,
                "Search this project's cross-agent conversation memory. Read-only. The project is the bound session project, or projectRoot/project per call.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer" },
                        "agents": { "type": "array", "items": { "type": "string" } },
                        "includeUnscoped": { "type": "boolean" },
                        "includeStale": { "type": "boolean" },
                        "projectRoot": project_fields["projectRoot"].clone(),
                        "project": project_fields["project"].clone()
                    }
                }),
            ),
            read_only_tool(
                TOOL_MEMORY_SESSION_LIST,
                "List this project's indexed agent sessions, newest first by first-message time. Read-only.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "limit": { "type": "integer" },
                        "agents": { "type": "array", "items": { "type": "string" } },
                        "includeUnscoped": { "type": "boolean" },
                        "projectRoot": { "type": "string" },
                        "project": { "type": "string" }
                    }
                }),
            ),
            read_only_tool(
                TOOL_MEMORY_SESSION_GET,
                "Read one indexed session's messages in transcript order. Read-only.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sessionKey"],
                    "properties": {
                        "sessionKey": { "type": "string" },
                        "limit": { "type": "integer" },
                        "includeStale": { "type": "boolean" },
                        "includeUnscoped": { "type": "boolean" },
                        "projectRoot": { "type": "string" },
                        "project": { "type": "string" }
                    }
                }),
            ),
            read_only_tool(
                TOOL_MEMORY_SESSION_MESSAGES,
                "Read one page of session messages around an anchor ordinal. Read-only.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sessionKey"],
                    "properties": {
                        "sessionKey": { "type": "string" },
                        "anchorOrdinal": { "type": "integer" },
                        "before": { "type": "integer" },
                        "after": { "type": "integer" },
                        "maxChars": { "type": "integer" },
                        "projectRoot": { "type": "string" },
                        "project": { "type": "string" }
                    }
                }),
            ),
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        _cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        match name {
            TOOL_MEMORY_SEARCH => self.search(arguments).await,
            TOOL_MEMORY_SESSION_LIST => self.list_sessions(arguments).await,
            TOOL_MEMORY_SESSION_GET => self.get_session(arguments).await,
            TOOL_MEMORY_SESSION_MESSAGES => self.session_messages(arguments).await,
            _ => Err(McpDomainError::ToolNotFound(format_tool_name(
                BUILTIN_SESSION_MEMORY,
                name,
            ))),
        }
    }
}

impl SessionMemoryProvider {
    async fn search(
        &self,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        let args: SearchArgs = parse_args(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SEARCH, arguments)?;
        if args.query.trim().is_empty() {
            return Err(McpDomainError::InvalidArguments {
                server_id: BUILTIN_SESSION_MEMORY.into(),
                name: TOOL_MEMORY_SEARCH.into(),
            });
        }
        let root = self
            .scope
            .resolve(args.project_root.as_deref(), args.project.as_deref())
            .map_err(|error| scope_error(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SEARCH, error))?;
        let memory = Arc::clone(&self.scope.memory);
        let request = MemorySearchRequest {
            query: args.query,
            limit: args.limit,
            agents: args.agents,
            include_unscoped: args.include_unscoped,
            include_stale: args.include_stale,
        };
        let response = tokio::task::spawn_blocking(move || {
            run_blocking(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SEARCH, || {
                memory.search(&root, &request)
            })
        })
        .await
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: BUILTIN_SESSION_MEMORY.into(),
            operation: Operation::CallTool,
        })??;
        json_result(&response)
    }

    async fn list_sessions(
        &self,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        let args: SessionListArgs =
            parse_args(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_LIST, arguments)?;
        let root = self
            .scope
            .resolve(args.project_root.as_deref(), args.project.as_deref())
            .map_err(|error| {
                scope_error(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_LIST, error)
            })?;
        let memory = Arc::clone(&self.scope.memory);
        let response = tokio::task::spawn_blocking(move || {
            run_blocking(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_LIST, || {
                memory.list_sessions(&root, args.limit, args.include_unscoped, &args.agents)
            })
        })
        .await
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: BUILTIN_SESSION_MEMORY.into(),
            operation: Operation::CallTool,
        })??;
        json_result(&response)
    }

    async fn get_session(
        &self,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        let args: SessionGetArgs =
            parse_args(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_GET, arguments)?;
        if args.session_key.trim().is_empty() {
            return Err(McpDomainError::InvalidArguments {
                server_id: BUILTIN_SESSION_MEMORY.into(),
                name: TOOL_MEMORY_SESSION_GET.into(),
            });
        }
        let root = self
            .scope
            .resolve(args.project_root.as_deref(), args.project.as_deref())
            .map_err(|error| scope_error(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_GET, error))?;
        let memory = Arc::clone(&self.scope.memory);
        let session_key = args.session_key;
        let response = tokio::task::spawn_blocking(move || {
            run_blocking(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_GET, || {
                memory.get_session(
                    &root,
                    &session_key,
                    args.limit,
                    args.include_stale,
                    args.include_unscoped,
                )
            })
        })
        .await
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: BUILTIN_SESSION_MEMORY.into(),
            operation: Operation::CallTool,
        })??;
        json_result(&response)
    }

    async fn session_messages(
        &self,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        let args: SessionMessagesArgs = parse_args(
            BUILTIN_SESSION_MEMORY,
            TOOL_MEMORY_SESSION_MESSAGES,
            arguments,
        )?;
        if args.session_key.trim().is_empty() {
            return Err(McpDomainError::InvalidArguments {
                server_id: BUILTIN_SESSION_MEMORY.into(),
                name: TOOL_MEMORY_SESSION_MESSAGES.into(),
            });
        }
        let root = self
            .scope
            .resolve(args.project_root.as_deref(), args.project.as_deref())
            .map_err(|error| {
                scope_error(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_MESSAGES, error)
            })?;
        let memory = Arc::clone(&self.scope.memory);
        let session_key = args.session_key;
        let response = tokio::task::spawn_blocking(move || {
            run_blocking(BUILTIN_SESSION_MEMORY, TOOL_MEMORY_SESSION_MESSAGES, || {
                memory.session_window(
                    &root,
                    &session_key,
                    args.anchor_ordinal,
                    args.before.unwrap_or(DEFAULT_WINDOW_BEFORE),
                    args.after.unwrap_or(DEFAULT_WINDOW_AFTER),
                    args.max_chars.unwrap_or(DEFAULT_WINDOW_MAX_CHARS),
                )
            })
        })
        .await
        .map_err(|_| McpDomainError::UpstreamUnavailable {
            server_id: BUILTIN_SESSION_MEMORY.into(),
            operation: Operation::CallTool,
        })??;
        json_result(&response)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectBoundary {
    project_key: String,
    project_label: String,
    bound: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectListing {
    key: String,
    label: String,
}

#[async_trait]
impl BuiltInCapability for ProjectScopeProvider {
    fn id(&self) -> &str {
        BUILTIN_PROJECT_SCOPE
    }

    fn tools(&self) -> Vec<Tool> {
        vec![
            read_only_tool(
                TOOL_PROJECT_BOUNDARY,
                "Expose the authorized project boundary for this MCP caller. Re-derives the host-private fence per call.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "projectRoot": { "type": "string" },
                        "project": { "type": "string" }
                    }
                }),
            ),
            read_only_tool(
                TOOL_MEMORY_PROJECTS,
                "List every project whose cross-agent conversation memory this server can query. Each entry's key is the project selector the memory tools take.",
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {}
                }),
            ),
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        _cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        match name {
            TOOL_PROJECT_BOUNDARY => self.boundary(arguments),
            TOOL_MEMORY_PROJECTS => self.list_projects(),
            _ => Err(McpDomainError::ToolNotFound(format_tool_name(
                BUILTIN_PROJECT_SCOPE,
                name,
            ))),
        }
    }
}

impl ProjectScopeProvider {
    fn boundary(
        &self,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, McpDomainError> {
        let args: ScopeArgs = parse_args(BUILTIN_PROJECT_SCOPE, TOOL_PROJECT_BOUNDARY, arguments)?;
        let root = self
            .scope
            .resolve(args.project_root.as_deref(), args.project.as_deref())
            .map_err(|error| scope_error(BUILTIN_PROJECT_SCOPE, TOOL_PROJECT_BOUNDARY, error))?;
        let fence = ProjectFence::single(&root).map_err(|_| McpDomainError::PermissionDenied {
            server_id: BUILTIN_PROJECT_SCOPE.into(),
            name: TOOL_PROJECT_BOUNDARY.into(),
        })?;
        json_result(&ProjectBoundary {
            project_key: fence.namespace_key(),
            project_label: fence.display_label(),
            bound: self.scope.bound_project_root.is_some(),
        })
    }

    fn list_projects(&self) -> Result<CallToolResult, McpDomainError> {
        let dir = self.scope.memory.state_root().join(INDEX_DIR_NAME);
        let mut listings = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(key) = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                else {
                    continue;
                };
                if !is_namespace_key(&key) {
                    continue;
                }
                let database = dir.join(&key).join(INDEX_FILE_NAME);
                if !database.is_file() {
                    continue;
                }
                let label = key
                    .rsplit_once('-')
                    .map(|(label, _)| label.to_string())
                    .unwrap_or_else(|| key.clone());
                listings.push(ProjectListing { key, label });
            }
        }
        listings.sort_by(|left, right| left.key.cmp(&right.key));
        json_result(&listings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_core::{AllowAllTools, DenyListedTools, McpCore, McpCoreConfig};
    use crate::memory_index::types::{
        FileIdentity, IndexedSession, LineageDepth, NormalizedMessage, NormalizedRole,
        SessionScope, SourcePointer, TimestampConfidence, SCHEMA_VERSION,
    };
    use std::time::Duration;

    struct Harness {
        _temp: tempfile::TempDir,
        memory: Arc<MemoryIndexService>,
        project_root: PathBuf,
        other_root: PathBuf,
        transcript: PathBuf,
    }

    fn harness() -> Harness {
        let temp = tempfile::tempdir().unwrap();
        let project_root = temp.path().join("project");
        let other_root = temp.path().join("other");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&other_root).unwrap();
        let project_root = project_root.canonicalize().unwrap();
        let other_root = other_root.canonicalize().unwrap();
        let state_root = temp.path().join("state");
        std::fs::create_dir_all(&state_root).unwrap();
        let memory = Arc::new(MemoryIndexService::new(state_root.canonicalize().unwrap()));
        let transcript = temp.path().join("chat.jsonl");
        std::fs::write(&transcript, b"{\"role\":\"user\",\"text\":\"alpha\"}\n").unwrap();
        Harness {
            memory,
            project_root,
            other_root,
            transcript,
            _temp: temp,
        }
    }

    fn seed(harness: &Harness, project_root: &Path, text: &str) -> String {
        let fence = ProjectFence::single(project_root).unwrap();
        let location = fence.index_location(harness.memory.state_root()).unwrap();
        location.ensure_dir().unwrap();
        let mut store =
            MemoryStore::open(&location.database_path, &location.namespace_key).unwrap();
        store
            .write_project_root(fence.project().canonical())
            .unwrap();
        let identity = FileIdentity::read(&harness.transcript).unwrap();
        let bytes = std::fs::read(&harness.transcript).unwrap();
        let pointer =
            SourcePointer::for_record(&identity, harness.transcript.to_str().unwrap(), 0, &bytes);
        let key = format!("claude-code:{}:{text}", project_root.display());
        let session = IndexedSession {
            schema_version: SCHEMA_VERSION,
            session_key: key.clone(),
            vendor: "claude-code".into(),
            vendor_session_id: "sess-1".into(),
            root_session_key: key.clone(),
            lineage_depth: LineageDepth::ROOT,
            project_key: location.namespace_key.clone(),
            scope: SessionScope::Scoped,
            cwd: Some(project_root.to_string_lossy().into_owned()),
            title: Some("seeded".into()),
            first_message_at_utc: Some("2026-09-01T00:00:00.000Z".into()),
            first_message_at_ms: Some(1_788_220_800_000),
            last_activity_at_utc: None,
            last_activity_at_ms: None,
            timestamp_confidence: TimestampConfidence::Native,
            message_count: 1,
            tool_count: 0,
            file_path: harness.transcript.to_string_lossy().into_owned(),
            source: pointer.clone(),
        };
        let message = NormalizedMessage {
            schema_version: SCHEMA_VERSION,
            message_key: format!("{key}#0"),
            session_key: key.clone(),
            root_session_key: key.clone(),
            lineage_depth: LineageDepth::ROOT,
            ordinal: 0,
            role: NormalizedRole::User,
            timestamp_utc: Some("2026-09-01T00:00:00.000Z".into()),
            timestamp_ms: Some(1_788_220_800_000),
            timestamp_confidence: TimestampConfidence::Native,
            text: text.to_string(),
            tool_name: None,
            tool_call_id: None,
            source: pointer,
        };
        store.replace_session(&session, &[message], &[], 0).unwrap();
        key
    }

    fn args(value: Value) -> Option<Map<String, Value>> {
        value.as_object().cloned()
    }

    struct SlowBuiltIn;

    #[async_trait]
    impl BuiltInCapability for SlowBuiltIn {
        fn id(&self) -> &str {
            "slow-built-in"
        }

        fn tools(&self) -> Vec<Tool> {
            vec![read_only_tool(
                "wait",
                "Wait",
                json!({"type": "object", "properties": {}}),
            )]
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Option<Map<String, Value>>,
            _cancellation: CancellationToken,
        ) -> Result<CallToolResult, McpDomainError> {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(CallToolResult::success(vec![ContentBlock::text("done")]))
        }
    }

    struct LargeBuiltIn;

    #[async_trait]
    impl BuiltInCapability for LargeBuiltIn {
        fn id(&self) -> &str {
            "large-built-in"
        }

        fn tools(&self) -> Vec<Tool> {
            vec![read_only_tool(
                "dump",
                "Dump",
                json!({"type": "object", "properties": {}}),
            )]
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Option<Map<String, Value>>,
            _cancellation: CancellationToken,
        ) -> Result<CallToolResult, McpDomainError> {
            Ok(CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(4096),
            )]))
        }
    }

    #[tokio::test]
    async fn memory_backed_registry_lists_prefixed_tools() {
        let harness = harness();
        let registry = BuiltInRegistry::memory_backed(Arc::clone(&harness.memory), None);
        let tools = registry.list_tools(&AllowAllTools);
        let names: Vec<_> = tools
            .iter()
            .map(|item| item.tool.name.to_string())
            .collect();
        assert!(names.contains(&format!("{BUILTIN_SESSION_MEMORY}_{TOOL_MEMORY_SEARCH}")));
        assert!(names.contains(&format!("{BUILTIN_PROJECT_SCOPE}_{TOOL_PROJECT_BOUNDARY}")));
        assert!(names.contains(&format!("{BUILTIN_PROJECT_SCOPE}_{TOOL_MEMORY_PROJECTS}")));
        assert!(!format!("{registry:?}").contains(harness.memory.state_root().to_str().unwrap()));
    }

    #[tokio::test]
    async fn apply_config_disables_and_denies_individual_tools() {
        let harness = harness();
        let mut registry = BuiltInRegistry::memory_backed(Arc::clone(&harness.memory), None);
        registry.apply_config(&[McpBuiltInConfig {
            id: BUILTIN_SESSION_MEMORY.into(),
            enabled: false,
            policy: McpCapabilityPolicy::default(),
        }]);
        let tools = registry.list_tools(&AllowAllTools);
        assert!(!tools
            .iter()
            .any(|item| item.server_id == BUILTIN_SESSION_MEMORY));
        assert!(tools
            .iter()
            .any(|item| item.server_id == BUILTIN_PROJECT_SCOPE));

        registry.apply_config(&[
            McpBuiltInConfig {
                id: BUILTIN_SESSION_MEMORY.into(),
                enabled: true,
                policy: McpCapabilityPolicy {
                    allow_tools: None,
                    deny_tools: vec![TOOL_MEMORY_SEARCH.into()],
                },
            },
            McpBuiltInConfig {
                id: BUILTIN_PROJECT_SCOPE.into(),
                enabled: true,
                policy: McpCapabilityPolicy::default(),
            },
        ]);
        let tools = registry.list_tools(&AllowAllTools);
        assert!(
            !tools
                .iter()
                .any(|item| item.tool.name
                    == format!("{BUILTIN_SESSION_MEMORY}_{TOOL_MEMORY_SEARCH}"))
        );
        assert!(tools
            .iter()
            .any(|item| item.tool.name
                == format!("{BUILTIN_SESSION_MEMORY}_{TOOL_MEMORY_SESSION_LIST}")));
    }

    #[tokio::test]
    async fn search_rederives_scope_and_rejects_cross_project_requests() {
        let harness = harness();
        seed(&harness, &harness.project_root, "the login redirect loops");
        seed(&harness, &harness.other_root, "unrelated other project");
        let core = McpCore::new_with_builtins(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
            BuiltInRegistry::memory_backed(Arc::clone(&harness.memory), None),
        );
        let exposed = format!("{BUILTIN_SESSION_MEMORY}_{TOOL_MEMORY_SEARCH}");
        let result = core
            .call_tool(
                &exposed,
                args(json!({
                    "query": "login",
                    "projectRoot": harness.project_root
                })),
            )
            .await
            .unwrap();
        let text = result.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("login redirect"));
        assert!(!text.contains("unrelated other project"));

        let other = core
            .call_tool(
                &exposed,
                args(json!({
                    "query": "unrelated",
                    "projectRoot": harness.other_root,
                })),
            )
            .await
            .unwrap();
        let other_text = other.content[0].as_text().unwrap().text.clone();
        assert!(other_text.contains("unrelated other project"));
        assert!(!other_text.contains("login redirect"));

        let missing = core
            .call_tool(&exposed, args(json!({ "query": "login" })))
            .await;
        assert!(matches!(
            missing,
            Err(McpDomainError::InvalidArguments { .. })
        ));
        assert!(!format!("{missing:?}").contains("login"));
    }

    #[tokio::test]
    async fn bound_provider_ignores_other_project_and_serves_without_selector() {
        let harness = harness();
        seed(&harness, &harness.project_root, "bound session memory");
        seed(&harness, &harness.other_root, "should stay hidden");
        let core = McpCore::new_with_builtins(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
            BuiltInRegistry::memory_backed(
                Arc::clone(&harness.memory),
                Some(harness.project_root.clone()),
            ),
        );
        let exposed = format!("{BUILTIN_SESSION_MEMORY}_{TOOL_MEMORY_SEARCH}");
        let result = core
            .call_tool(&exposed, args(json!({ "query": "bound" })))
            .await
            .unwrap();
        assert!(result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("bound session memory"));

        let crossed = core
            .call_tool(
                &exposed,
                args(json!({
                    "query": "hidden",
                    "projectRoot": harness.other_root
                })),
            )
            .await;
        assert!(matches!(
            crossed,
            Err(McpDomainError::PermissionDenied { .. })
        ));
        assert!(!format!("{crossed:?}").contains("should stay hidden"));
        assert!(!format!("{crossed:?}").contains(harness.other_root.to_str().unwrap()));
    }

    #[tokio::test]
    async fn project_boundary_and_listing_use_rederived_fence() {
        let harness = harness();
        seed(&harness, &harness.project_root, "listed");
        let core = McpCore::new_with_builtins(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
            BuiltInRegistry::memory_backed(Arc::clone(&harness.memory), None),
        );
        let boundary = core
            .call_tool(
                &format!("{BUILTIN_PROJECT_SCOPE}_{TOOL_PROJECT_BOUNDARY}"),
                args(json!({ "projectRoot": harness.project_root })),
            )
            .await
            .unwrap();
        let text = boundary.content[0].as_text().unwrap().text.clone();
        let fence = ProjectFence::single(&harness.project_root).unwrap();
        assert!(text.contains(&fence.namespace_key()));
        assert!(text.contains("\"bound\": false"));

        let listing = core
            .call_tool(
                &format!("{BUILTIN_PROJECT_SCOPE}_{TOOL_MEMORY_PROJECTS}"),
                None,
            )
            .await
            .unwrap();
        let listed = listing.content[0].as_text().unwrap().text.clone();
        assert!(listed.contains(&fence.namespace_key()));
        assert!(!listed.contains(harness.project_root.to_str().unwrap()));
    }

    #[tokio::test]
    async fn permission_timeout_cancellation_and_size_apply_to_builtins() {
        let mut registry = BuiltInRegistry::empty();
        registry.register(Arc::new(SlowBuiltIn));
        registry.register(Arc::new(LargeBuiltIn));
        let permission = Arc::new(DenyListedTools::new([(
            "slow-built-in".into(),
            "wait".into(),
        )]));
        let core = McpCore::new(
            McpCoreConfig {
                operation_timeout: Duration::from_millis(40),
                max_response_bytes: 256,
                ..Default::default()
            },
            permission,
        );
        core.replace_builtins(registry).await;

        let tools = core.list_tools().await.unwrap();
        assert!(!tools
            .items
            .iter()
            .any(|item| item.tool.name == "slow-built-in_wait"));
        assert!(tools
            .items
            .iter()
            .any(|item| item.tool.name == "large-built-in_dump"));
        assert!(matches!(
            core.call_tool("slow-built-in_wait", None).await,
            Err(McpDomainError::PermissionDenied { .. })
        ));

        let allowed = McpCore::new(
            McpCoreConfig {
                operation_timeout: Duration::from_millis(40),
                max_response_bytes: 256,
                ..Default::default()
            },
            Arc::new(AllowAllTools),
        );
        let mut registry = BuiltInRegistry::empty();
        registry.register(Arc::new(SlowBuiltIn));
        registry.register(Arc::new(LargeBuiltIn));
        allowed.replace_builtins(registry).await;
        assert!(matches!(
            allowed.call_tool("slow-built-in_wait", None).await,
            Err(McpDomainError::Timeout {
                operation: Operation::CallTool,
                ..
            })
        ));
        assert!(matches!(
            allowed.call_tool("large-built-in_dump", None).await,
            Err(McpDomainError::ResponseTooLarge { .. })
        ));

        let cancellable = McpCore::default();
        let mut registry = BuiltInRegistry::empty();
        registry.register(Arc::new(SlowBuiltIn));
        cancellable.replace_builtins(registry).await;
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        assert!(matches!(
            cancellable
                .call_tool_with_cancel("slow-built-in_wait", None, cancellation)
                .await,
            Err(McpDomainError::Cancelled {
                operation: Operation::CallTool,
                ..
            })
        ));
        task.await.unwrap();
    }
}
