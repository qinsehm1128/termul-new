//! The standalone, read-only MCP server for **external** clients.
//!
//! Before this existed, Termul exposed no MCP surface to anyone outside itself.
//! The one rmcp server in the repository is the `host_mcp` child, which the
//! agent spawns and which forwards every call to a parent TCP listener using a
//! port and token from its own environment. An external client — Claude
//! Desktop, another editor, a script — has no parent to reach and no token to
//! present, so that server is unreachable by construction. This subcommand is
//! therefore not an optimization; it is the only route by which the memory index
//! is usable outside the app.
//!
//! ## Modes
//!
//! **Universal (no `--project`)** — the intended shape. The server enumerates
//! every project with an index under the state root (`memory_projects`) and
//! each read tool takes a `project` selector (the namespace key returned by
//! that listing). The client decides which project to ask about.
//!
//! **Legacy (`--project <path>`)** — the v0.9.0 shape, kept so client configs
//! exported by the previous release keep working: the three read tools are
//! pinned to that one project and cannot see any other.
//!
//! ## Authorization
//!
//! The process boundary is the outer fence in both modes: the server runs as
//! whoever launched it, over their own files, which is the same model every
//! stdio MCP server uses. In universal mode the per-tool fence is rebuilt from
//! the project root recorded in the index database itself (written at build
//! time), never from client input, and an index that predates that recording
//! is reported as needing a rebuild rather than being served blind.
//!
//! `--state-root` is required rather than inferred. The desktop names its state
//! root through Tauri's `app_data_dir()`; re-deriving that platform path here
//! would create a second source of truth that can silently drift from the one
//! the running app uses, and a memory server reading a *different* directory
//! than the app writes is worse than one that refuses to start. The desktop
//! hands out the exact ready-to-paste invocation instead.

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars;
use rmcp::service::serve_server;
use rmcp::{tool, tool_router};

use super::service::{MemoryIndexService, MemorySearchRequest};
use super::store::MemoryStore;
use crate::acp::host_mcp::{MemorySearchInput, MemorySessionGetInput, MemorySessionListInput};

/// The subcommand flag. Named for what it serves, and public so the desktop can
/// print the invocation it expects a client to use.
pub const MEMORY_MCP_ARG: &str = "--memory-mcp-server";
pub const PROJECT_ARG: &str = "--project";
pub const STATE_ROOT_ARG: &str = "--state-root";

/// True when this process was launched as the standalone memory MCP server.
///
/// Matches the flag at any argv position, mirroring
/// [`crate::acp::host_mcp::is_child_invocation`] so the two entrypoints cannot
/// drift in how they are detected.
#[must_use]
pub fn is_invocation() -> bool {
    std::env::args().skip(1).any(|arg| arg == MEMORY_MCP_ARG)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioConfig {
    /// `None` = universal mode (project chosen per tool call);
    /// `Some` = legacy single-project mode kept for v0.9.0 client configs.
    pub project_root: Option<PathBuf>,
    pub state_root: PathBuf,
}

/// Parse `--memory-mcp-server [--project <path>] --state-root <path>`.
///
/// Returns an error string rather than an enum so `run()` can print it verbatim
/// and exit non-zero, matching how the plan child reports a bad environment.
pub fn parse_args(args: &[String]) -> Result<StdioConfig, String> {
    let mut project_root = None;
    let mut state_root = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            MEMORY_MCP_ARG => {}
            PROJECT_ARG => {
                project_root = Some(PathBuf::from(take_value(args, &mut index, PROJECT_ARG)?));
            }
            STATE_ROOT_ARG => {
                state_root = Some(PathBuf::from(take_value(args, &mut index, STATE_ROOT_ARG)?));
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}"));
            }
            other => return Err(format!("unexpected argument {other}")),
        }
        index += 1;
    }
    let state_root = state_root.ok_or_else(|| format!("missing {STATE_ROOT_ARG} <path>"))?;
    Ok(StdioConfig {
        project_root,
        state_root,
    })
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// The exact command line a legacy single-project client should be configured
/// with. Kept for compatibility with v0.9.0-exported configs.
#[must_use]
pub fn invocation_for(executable: &Path, config: &StdioConfig) -> Vec<String> {
    let mut invocation = vec![
        executable.to_string_lossy().into_owned(),
        MEMORY_MCP_ARG.to_string(),
    ];
    if let Some(project_root) = &config.project_root {
        invocation.push(PROJECT_ARG.to_string());
        invocation.push(project_root.to_string_lossy().into_owned());
    }
    invocation.push(STATE_ROOT_ARG.to_string());
    invocation.push(config.state_root.to_string_lossy().into_owned());
    invocation
}

/// The universal invocation: one server, every indexed project, chosen by the
/// client per query. This is what the Settings export card copies.
#[must_use]
pub fn universal_invocation_for(executable: &Path, state_root: &Path) -> Vec<String> {
    vec![
        executable.to_string_lossy().into_owned(),
        MEMORY_MCP_ARG.to_string(),
        STATE_ROOT_ARG.to_string(),
        state_root.to_string_lossy().into_owned(),
    ]
}

/// Subcommand entrypoint. Returns a process exit code.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse_args(&args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("[memory-mcp] {message}");
            eprintln!(
                "[memory-mcp] usage: {MEMORY_MCP_ARG} [--project <project dir>] \
                 {STATE_ROOT_ARG} <host state dir>"
            );
            return 1;
        }
    };
    // Fail at startup, not on the first query: a client that was configured with
    // a bad path should find out when it starts the server. Only the legacy
    // single-project mode has a spawn-time fence to verify; universal mode
    // validates each requested project against its index on every call.
    if let Some(project_root) = &config.project_root {
        if let Err(error) = super::scope::ProjectFence::single(project_root) {
            eprintln!("[memory-mcp] {error}");
            return 1;
        }
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("[memory-mcp] failed to start runtime: {error}");
            return 1;
        }
    };
    match runtime.block_on(serve(config)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[memory-mcp] {error}");
            1
        }
    }
}

async fn serve(config: StdioConfig) -> Result<(), String> {
    let (stdin, stdout) = rmcp::transport::io::stdio();
    let service = MemoryIndexService::new(config.state_root.clone());
    // Each branch runs to completion inside its own scope: the two server
    // types are distinct concrete types and must not be unified.
    match &config.project_root {
        Some(project_root) => {
            let server = MemoryMcpServer {
                service,
                project_root: project_root.clone(),
            };
            let running = serve_server(server, (stdin, stdout))
                .await
                .map_err(|error| format!("mcp server initialize failed: {error}"))?;
            running
                .waiting()
                .await
                .map_err(|error| format!("mcp server ended with error: {error}"))?;
        }
        None => {
            let server = UniversalMemoryMcpServer {
                service,
                state_root: config.state_root.clone(),
            };
            let running = serve_server(server, (stdin, stdout))
                .await
                .map_err(|error| format!("mcp server initialize failed: {error}"))?;
            running
                .waiting()
                .await
                .map_err(|error| format!("mcp server ended with error: {error}"))?;
        }
    }
    Ok(())
}

/// Read-only by construction: there is no build/refresh tool here.
///
/// Refreshing the index walks tens of thousands of files and can take minutes;
/// it belongs to the explicit right-click action in the app, not to whatever
/// happens to be connected over stdio.
/// Page-shape defaults for `memory_session_messages` (legacy server tool).
pub const DEFAULT_WINDOW_BEFORE: usize = 10;
pub const DEFAULT_WINDOW_AFTER: usize = 10;
pub const DEFAULT_WINDOW_MAX_CHARS: usize = 20_000;

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessagesInput {
    /// Session key from `memory_search` or `memory_session_list`.
    pub session_key: String,
    /// The ordinal to read around — a search hit's `ordinal`. Absent = start
    /// of the session.
    pub anchor_ordinal: Option<u32>,
    /// Context messages above the anchor. Defaults to 10.
    #[serde(default)]
    pub before: Option<usize>,
    /// Context messages below the anchor (or page size without an anchor).
    /// Defaults to 10.
    #[serde(default)]
    pub after: Option<usize>,
    /// Hard budget on the page's text. Defaults to 20,000 characters.
    #[serde(default)]
    pub max_chars: Option<usize>,
}

struct MemoryMcpServer {
    service: MemoryIndexService,
    project_root: PathBuf,
}

#[tool_router(server_handler)]
impl MemoryMcpServer {
    #[tool(
        name = "memory_search",
        description = "Search this project's cross-agent conversation memory — every past Claude Code, Codex and pi session for this project, normalized into one shape. Use it before re-deriving something the project has already worked through. Read-only. The project is fixed by how this server was launched and cannot be chosen."
    )]
    async fn memory_search(&self, Parameters(input): Parameters<MemorySearchInput>) -> String {
        let service = self.service.clone();
        let project_root = self.project_root.clone();
        run_blocking(move || {
            service.search(
                &project_root,
                &MemorySearchRequest {
                    query: input.query,
                    limit: input.limit,
                    agents: input.agents,
                    include_unscoped: input.include_unscoped,
                    include_stale: input.include_stale,
                },
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_list",
        description = "List this project's indexed agent sessions, newest first by the time of their FIRST message (not file modification time). Read-only."
    )]
    async fn memory_session_list(
        &self,
        Parameters(input): Parameters<MemorySessionListInput>,
    ) -> String {
        let service = self.service.clone();
        let project_root = self.project_root.clone();
        run_blocking(move || {
            service.list_sessions(
                &project_root,
                input.limit,
                input.include_unscoped,
                &input.agents,
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_get",
        description = "Read one indexed session's messages in transcript order, given a sessionKey from memory_search or memory_session_list. Read-only."
    )]
    async fn memory_session_get(
        &self,
        Parameters(input): Parameters<MemorySessionGetInput>,
    ) -> String {
        let service = self.service.clone();
        let project_root = self.project_root.clone();
        run_blocking(move || {
            service.get_session(
                &project_root,
                &input.session_key,
                input.limit,
                input.include_stale,
                input.include_unscoped,
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_messages",
        description = "Read one page of this project's session messages around an anchor ordinal (a search hit's `ordinal`), with `before`/`after` context and a character budget. Returns cursors (`first_ordinal`/`last_ordinal`/`has_older`/`has_newer`) for paging. Read-only."
    )]
    async fn memory_session_messages(
        &self,
        Parameters(input): Parameters<SessionMessagesInput>,
    ) -> String {
        let service = self.service.clone();
        let project_root = self.project_root.clone();
        run_blocking(move || {
            service.session_window(
                &project_root,
                &input.session_key,
                input.anchor_ordinal,
                input.before.unwrap_or(DEFAULT_WINDOW_BEFORE),
                input.after.unwrap_or(DEFAULT_WINDOW_AFTER),
                input.max_chars.unwrap_or(DEFAULT_WINDOW_MAX_CHARS),
            )
        })
        .await
    }
}

/// Universal-mode server: one process serves every indexed project, and each
/// read tool names its project by the namespace key returned from
/// `memory_projects`. The fence is rebuilt per call from the project root
/// recorded in that project's index database — never from client input.
struct UniversalMemoryMcpServer {
    service: MemoryIndexService,
    state_root: PathBuf,
}

/// One entry of the `memory_projects` listing.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryProjectListing {
    /// Namespace key the read tools take as their `project` selector.
    key: String,
    /// Human-readable project label (folder name embedded in the key).
    label: String,
    /// Canonical project root, when the index has recorded one. `null` means
    /// the index predates universal mode — rebuild it in the app to serve it.
    project_root: Option<String>,
}

const KEY_ALLOWED: &str = "abcdefghijklmnopqrstuvwxyz0123456789-";

/// A namespace key is machine-generated (`label-digest`); requiring this
/// alphabet makes it impossible for a caller to traverse out of the
/// memory-index directory through the `project` selector.
fn is_namespace_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| KEY_ALLOWED.contains(c))
}

fn index_database_path(state_root: &Path, key: &str) -> PathBuf {
    state_root.join("memory-index").join(key).join("index.sqlite3")
}

#[tool_router(server_handler)]
impl UniversalMemoryMcpServer {
    /// Every project with a readable index under this state root, newest
    /// information first not guaranteed — the listing is alphabetical by key.
    fn list_projects(&self) -> Vec<MemoryProjectListing> {
        let dir = self.state_root.join("memory-index");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut listings: Vec<MemoryProjectListing> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter_map(|path| {
                let key = path.file_name()?.to_string_lossy().into_owned();
                if !is_namespace_key(&key) {
                    return None;
                }
                let database = index_database_path(&self.state_root, &key);
                if !database.is_file() {
                    return None;
                }
                let label = key
                    .rsplit_once('-')
                    .map(|(label, _)| label.to_string())
                    .unwrap_or_else(|| key.clone());
                Some(MemoryProjectListing {
                    project_root: MemoryStore::stored_project_root(&database),
                    key,
                    label,
                })
            })
            .collect();
        listings.sort_by(|a, b| a.key.cmp(&b.key));
        listings
    }

    /// Resolve a client-supplied project selector to the canonical project
    /// root recorded in that index. Legacy indexes without the record cannot
    /// be served — the caller is told to rebuild rather than being served
    /// without a verifiable fence.
    fn resolve_project(&self, key: &str) -> Result<PathBuf, String> {
        if !is_namespace_key(key) {
            return Err(format!("{TOOL_ERROR_MARKER} VALIDATION_ERROR: unknown project {key}"));
        }
        let database = index_database_path(&self.state_root, key);
        let root = MemoryStore::stored_project_root(&database).ok_or_else(|| {
            format!(
                "{TOOL_ERROR_MARKER} MEMORY_INDEX_REBUILD_REQUIRED: \
                 this index predates universal mode; rebuild it in the Se app"
            )
        })?;
        Ok(PathBuf::from(root))
    }

    #[tool(
        name = "memory_projects",
        description = "List every project whose cross-agent conversation memory this server can query. Each entry's `key` is the `project` selector the other memory tools take; `projectRoot` is the folder it covers. Call this first."
    )]
    async fn memory_projects(&self) -> String {
        let listings = self.list_projects();
        serde_json::to_string_pretty(&listings)
            .unwrap_or_else(|error| tool_error("MEMORY_INDEX_ENCODE_FAILED", &error.to_string()))
    }

    #[tool(
        name = "memory_search",
        description = "Search one project's cross-agent conversation memory — every past Claude Code, Codex and pi session for it, normalized into one shape. Use it before re-deriving something the project has already worked through. Read-only. `project` is a key from `memory_projects`."
    )]
    async fn memory_search(
        &self,
        Parameters(input): Parameters<UniversalSearchInput>,
    ) -> String {
        let Ok(project_root) = self.resolve_project(&input.project) else {
            return self.resolve_error(&input.project);
        };
        let service = self.service.clone();
        run_blocking(move || {
            service.search(
                &project_root,
                &MemorySearchRequest {
                    query: input.query,
                    limit: input.limit,
                    agents: input.agents,
                    include_unscoped: input.include_unscoped,
                    include_stale: input.include_stale,
                },
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_list",
        description = "List one project's indexed agent sessions, newest first by the time of their FIRST message (not file modification time). Read-only. `project` is a key from `memory_projects`."
    )]
    async fn memory_session_list(
        &self,
        Parameters(input): Parameters<UniversalSessionListInput>,
    ) -> String {
        let Ok(project_root) = self.resolve_project(&input.project) else {
            return self.resolve_error(&input.project);
        };
        let service = self.service.clone();
        run_blocking(move || {
            service.list_sessions(
                &project_root,
                input.limit,
                input.include_unscoped,
                &input.agents,
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_get",
        description = "Read one indexed session's messages in transcript order, given a sessionKey from memory_search or memory_session_list. Read-only. `project` is a key from `memory_projects`."
    )]
    async fn memory_session_get(
        &self,
        Parameters(input): Parameters<UniversalSessionGetInput>,
    ) -> String {
        let Ok(project_root) = self.resolve_project(&input.project) else {
            return self.resolve_error(&input.project);
        };
        let service = self.service.clone();
        run_blocking(move || {
            service.get_session(
                &project_root,
                &input.session_key,
                input.limit,
                input.include_stale,
                input.include_unscoped,
            )
        })
        .await
    }

    #[tool(
        name = "memory_session_messages",
        description = "Read one page of one project's session messages around an anchor ordinal (a search hit's `ordinal`), with `before`/`after` context and a character budget. Returns cursors (`first_ordinal`/`last_ordinal`/`has_older`/`has_newer`) for paging. Read-only. `project` is a key from `memory_projects`."
    )]
    async fn memory_session_messages(
        &self,
        Parameters(input): Parameters<UniversalSessionMessagesInput>,
    ) -> String {
        let project_root = match self.resolve_project(&input.project) {
            Ok(root) => root,
            Err(message) => return message,
        };
        let service = self.service.clone();
        run_blocking(move || {
            service.session_window(
                &project_root,
                &input.session_key,
                input.anchor_ordinal,
                input.before.unwrap_or(DEFAULT_WINDOW_BEFORE),
                input.after.unwrap_or(DEFAULT_WINDOW_AFTER),
                input.max_chars.unwrap_or(DEFAULT_WINDOW_MAX_CHARS),
            )
        })
        .await
    }

    fn resolve_error(&self, key: &str) -> String {
        tool_error(
            "MEMORY_INDEX_UNKNOWN_PROJECT",
            &format!("unknown project {key}; call memory_projects first"),
        )
    }
}

/// Tool inputs for the universal server. These are stdio_mcp-local on purpose:
/// they carry a `project` selector the single-project inputs never had, and
/// the shared host_mcp inputs must not grow one (its frame has no project).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UniversalSearchInput {
    /// Namespace key from `memory_projects`.
    pub project: String,
    /// Words to look for. Treated as literal terms with implicit AND.
    pub query: String,
    /// Maximum hits to return. Defaults to 20, capped at 200.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to particular agents (`claude-code`, `codex`, `pi`).
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub include_unscoped: bool,
    #[serde(default)]
    pub include_stale: bool,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UniversalSessionMessagesInput {
    /// Namespace key from `memory_projects`.
    pub project: String,
    /// Session key from `memory_search` or `memory_session_list`.
    pub session_key: String,
    /// The ordinal to read around — a search hit's `ordinal`. Absent = start
    /// of the session.
    pub anchor_ordinal: Option<u32>,
    /// Context messages above the anchor. Defaults to 10.
    #[serde(default)]
    pub before: Option<usize>,
    /// Context messages below the anchor (or page size without an anchor).
    /// Defaults to 10.
    #[serde(default)]
    pub after: Option<usize>,
    /// Hard budget on the page's text. Defaults to 20,000 characters.
    #[serde(default)]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UniversalSessionListInput {
    /// Namespace key from `memory_projects`.
    pub project: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub include_unscoped: bool,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UniversalSessionGetInput {
    /// Namespace key from `memory_projects`.
    pub project: String,
    /// Session key from `memory_search` or `memory_session_list`.
    pub session_key: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_stale: bool,
    #[serde(default)]
    pub include_unscoped: bool,
}

/// SQLite reads are blocking; running them on the current-thread runtime would
/// stall the stdio transport for the duration of a query.
///
/// Every tool here returns a plain string, so a failure has to be legible *as* a
/// failure inside that string. It previously came back as bare text, which an
/// agent reading the result had no way to tell apart from an answer — a scope
/// rejection or a missing index looked exactly like "here is what the project
/// remembers". The marker below is the distinguishing token, and the shape is
/// identical for all three failure kinds so a caller only has to recognise one.
async fn run_blocking<T, F>(work: F) -> String
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce() -> super::MemoryIndexResult<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|error| tool_error("MEMORY_INDEX_ENCODE_FAILED", &error.to_string())),
        Ok(Err(error)) => tool_error(error.code, &error.detail),
        Err(error) => tool_error("MEMORY_INDEX_QUERY_PANICKED", &error.to_string()),
    }
}

/// Marker every failed tool result starts with.
pub const TOOL_ERROR_MARKER: &str = "MEMORY_INDEX_ERROR";

fn tool_error(code: &str, detail: &str) -> String {
    format!("{TOOL_ERROR_MARKER} {code}: {detail}")
}

#[cfg(test)]
mod error_shape_tests {
    use super::*;
    use crate::memory_index::{MemoryIndexError, MemoryIndexResult, ERR_OUT_OF_SCOPE};

    /// A failed tool call has to be legible as a failure. These tools return a
    /// plain string, so an out-of-scope rejection or a missing index used to
    /// arrive looking exactly like an answer — an external agent would read
    /// "MEMORY_INDEX_OUT_OF_SCOPE: ..." as what the project remembers.
    #[tokio::test]
    async fn a_failed_query_is_distinguishable_from_an_answer() {
        let failed = run_blocking(|| -> MemoryIndexResult<Vec<String>> {
            Err(MemoryIndexError::new(ERR_OUT_OF_SCOPE, "somewhere else"))
        })
        .await;
        assert!(
            failed.starts_with(TOOL_ERROR_MARKER),
            "an error did not announce itself: {failed:?}"
        );
        assert!(failed.contains(ERR_OUT_OF_SCOPE));

        let answered = run_blocking(|| Ok(vec!["alpha".to_string()])).await;
        assert!(
            !answered.starts_with(TOOL_ERROR_MARKER),
            "a successful result was marked as an error: {answered:?}"
        );
        assert!(answered.contains("alpha"));
    }

    /// An empty result is an answer, not an error — "this project remembers
    /// nothing about that" has to stay distinguishable from "the query failed".
    #[tokio::test]
    async fn an_empty_result_is_not_reported_as_an_error() {
        let empty = run_blocking(|| Ok(Vec::<String>::new())).await;
        assert!(!empty.starts_with(TOOL_ERROR_MARKER), "{empty:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn a_complete_invocation_parses() {
        let config = parse_args(&args(&[
            MEMORY_MCP_ARG,
            PROJECT_ARG,
            "/repo",
            STATE_ROOT_ARG,
            "/state",
        ]))
        .unwrap();
        assert_eq!(config.project_root, Some(PathBuf::from("/repo")));
        assert_eq!(config.state_root, PathBuf::from("/state"));
    }

    /// `--project` is now optional (universal mode); `--state-root` never is.
    #[test]
    fn state_root_is_required_project_is_not() {
        let missing_state = parse_args(&args(&[MEMORY_MCP_ARG, PROJECT_ARG, "/repo"])).unwrap_err();
        assert!(missing_state.contains(STATE_ROOT_ARG), "{missing_state}");

        let universal =
            parse_args(&args(&[MEMORY_MCP_ARG, STATE_ROOT_ARG, "/state"])).unwrap();
        assert_eq!(universal.project_root, None);
    }

    #[test]
    fn a_flag_without_a_value_is_an_error_not_a_silent_default() {
        for incomplete in [
            args(&[MEMORY_MCP_ARG, PROJECT_ARG]),
            args(&[MEMORY_MCP_ARG, PROJECT_ARG, "   ", STATE_ROOT_ARG, "/state"]),
        ] {
            assert!(
                parse_args(&incomplete).is_err(),
                "{incomplete:?} must not parse"
            );
        }
    }

    #[test]
    fn unknown_options_are_refused() {
        let error = parse_args(&args(&[
            MEMORY_MCP_ARG,
            PROJECT_ARG,
            "/repo",
            STATE_ROOT_ARG,
            "/state",
            "--allow-everything",
        ]))
        .unwrap_err();
        assert!(error.contains("--allow-everything"), "{error}");
    }

    #[test]
    fn the_printed_invocation_round_trips_through_the_parser() {
        let config = StdioConfig {
            project_root: Some(PathBuf::from("/Users/qs/project/me/termul")),
            state_root: PathBuf::from(
                "/Users/qs/Library/Application Support/com.se-manager.app",
            ),
        };
        let invocation = invocation_for(Path::new("/Applications/Se.app/se-manager"), &config);
        assert_eq!(invocation[0], "/Applications/Se.app/se-manager");
        // Everything after the executable is what the process itself will see.
        assert_eq!(parse_args(&invocation[1..]).unwrap(), config);
    }

    /// Both binaries must branch on the flag before they build anything.
    ///
    /// A source-level guard because the failure it prevents is a future edit,
    /// not a runtime state: an external client's server process that first
    /// opens a window, or a store, is not a stdio MCP server. Mirrors the
    /// existing `bootstrap_precedes_all_mutable_store_opens` guard.
    #[test]
    fn both_entrypoints_branch_before_any_app_setup() {
        let desktop = include_str!("../main.rs");
        let branch = desktop
            .find("memory_index::stdio_mcp::is_invocation")
            .expect("the desktop binary must branch on the memory MCP flag");
        let run = desktop
            .find("se_manager_lib::run()")
            .expect("desktop run() call");
        assert!(
            branch < run,
            "the memory MCP branch must precede the Tauri app launch"
        );

        let standalone = include_str!("../server_main.rs");
        let branch = standalone
            .find("memory_index::stdio_mcp::is_invocation")
            .expect("the standalone binary must branch on the memory MCP flag");
        for later in ["ConversationBootstrap::run", "match serve("] {
            let position = standalone
                .find(later)
                .unwrap_or_else(|| panic!("standalone should still contain {later}"));
            assert!(
                branch < position,
                "the memory MCP branch must precede {later}"
            );
        }
    }

    /// The one thing this surface must never grow. A refresh walks tens of
    /// thousands of files; it belongs to the explicit action in the app.
    /// Legacy server: 3 read tools + paged-messages = 4. Universal server:
    /// `memory_projects` + those 4 = 5. Nine total, all read-only.
    #[test]
    fn this_server_exposes_no_write_or_build_tool() {
        let source = include_str!("stdio_mcp.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let tool_count = code.matches("#[tool(").count();
        assert_eq!(tool_count, 9, "expected nine read-only tools");
        for forbidden in ["service.build(", "build_index", "IngestOptions"] {
            assert!(
                !code.contains(forbidden),
                "the external MCP surface must stay read-only, found {forbidden}"
            );
        }
    }
}
