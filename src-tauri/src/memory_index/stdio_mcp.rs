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
//! ## Authorization
//!
//! The project is fixed at spawn time from `--project` and is not a parameter of
//! any tool. A client cannot ask this process about a different project, and a
//! second project needs a second process. Beyond that the boundary is the
//! process itself: the server runs as whoever launched it, over their own files,
//! which is the same model every stdio MCP server uses.
//!
//! `--state-root` is required rather than inferred. The desktop names its state
//! root through Tauri's `app_data_dir()`; re-deriving that platform path here
//! would create a second source of truth that can silently drift from the one
//! the running app uses, and a memory server reading a *different* directory
//! than the app writes is worse than one that refuses to start. The desktop
//! hands out the exact ready-to-paste invocation instead.

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::service::serve_server;
use rmcp::{tool, tool_router};

use super::service::{MemoryIndexService, MemorySearchRequest};
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
    pub project_root: PathBuf,
    pub state_root: PathBuf,
}

/// Parse `--memory-mcp-server --project <path> --state-root <path>`.
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
                project_root = Some(take_value(args, &mut index, PROJECT_ARG)?);
            }
            STATE_ROOT_ARG => {
                state_root = Some(take_value(args, &mut index, STATE_ROOT_ARG)?);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}"));
            }
            other => return Err(format!("unexpected argument {other}")),
        }
        index += 1;
    }
    let project_root = project_root.ok_or_else(|| format!("missing {PROJECT_ARG} <path>"))?;
    let state_root = state_root.ok_or_else(|| format!("missing {STATE_ROOT_ARG} <path>"))?;
    Ok(StdioConfig {
        project_root: PathBuf::from(project_root),
        state_root: PathBuf::from(state_root),
    })
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// The exact command line an external MCP client should be configured with.
///
/// Produced by the host, which knows its own state root, so the user never has
/// to guess it.
#[must_use]
pub fn invocation_for(executable: &Path, config: &StdioConfig) -> Vec<String> {
    vec![
        executable.to_string_lossy().into_owned(),
        MEMORY_MCP_ARG.to_string(),
        PROJECT_ARG.to_string(),
        config.project_root.to_string_lossy().into_owned(),
        STATE_ROOT_ARG.to_string(),
        config.state_root.to_string_lossy().into_owned(),
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
                "[memory-mcp] usage: {MEMORY_MCP_ARG} {PROJECT_ARG} <project dir> \
                 {STATE_ROOT_ARG} <host state dir>"
            );
            return 1;
        }
    };
    // Fail at startup, not on the first query: a client that was configured with
    // a bad path should find out when it starts the server.
    if let Err(error) = super::scope::ProjectFence::single(&config.project_root) {
        eprintln!("[memory-mcp] {error}");
        return 1;
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
    let service = MemoryMcpServer {
        service: MemoryIndexService::new(config.state_root),
        project_root: config.project_root,
    };
    let running = serve_server(service, (stdin, stdout))
        .await
        .map_err(|error| format!("mcp server initialize failed: {error}"))?;
    running
        .waiting()
        .await
        .map_err(|error| format!("mcp server ended with error: {error}"))?;
    Ok(())
}

/// Read-only by construction: there is no build/refresh tool here.
///
/// Refreshing the index walks tens of thousands of files and can take minutes;
/// it belongs to the explicit right-click action in the app, not to whatever
/// happens to be connected over stdio.
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
            )
        })
        .await
    }
}

/// Run a blocking store query off the reactor and render the answer as JSON.
///
/// SQLite reads are blocking; running them on the current-thread runtime would
/// stall the stdio transport for the duration of a query.
async fn run_blocking<T, F>(work: F) -> String
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce() -> super::MemoryIndexResult<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|error| format!("could not encode result: {error}")),
        Ok(Err(error)) => format!("{error}"),
        Err(error) => format!("memory index query panicked: {error}"),
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
        assert_eq!(config.project_root, PathBuf::from("/repo"));
        assert_eq!(config.state_root, PathBuf::from("/state"));
    }

    /// Both paths are required. Inferring either one is what would let this
    /// server read a different directory than the app writes.
    #[test]
    fn both_paths_are_required() {
        let missing_state = parse_args(&args(&[MEMORY_MCP_ARG, PROJECT_ARG, "/repo"])).unwrap_err();
        assert!(missing_state.contains(STATE_ROOT_ARG), "{missing_state}");
        let missing_project =
            parse_args(&args(&[MEMORY_MCP_ARG, STATE_ROOT_ARG, "/state"])).unwrap_err();
        assert!(missing_project.contains(PROJECT_ARG), "{missing_project}");
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
            project_root: PathBuf::from("/Users/qs/project/me/termul"),
            state_root: PathBuf::from("/Users/qs/Library/Application Support/com.se-manager.app"),
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
    #[test]
    fn this_server_exposes_no_write_or_build_tool() {
        let source = include_str!("stdio_mcp.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let tool_count = code.matches("#[tool(").count();
        assert_eq!(tool_count, 3, "expected exactly three read-only tools");
        for forbidden in ["service.build(", "build_index", "IngestOptions"] {
            assert!(
                !code.contains(forbidden),
                "the external MCP surface must stay read-only, found {forbidden}"
            );
        }
    }
}
