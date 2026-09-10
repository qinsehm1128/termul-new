//! Tauri IPC for the memory index.
//!
//! Mirrors `web::memory_index_api` one-for-one and calls the same
//! [`MemoryIndexService`]; a capability that answered differently on desktop
//! than over HTTP would be a parity bug by construction.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::ingest::{IngestOptions, IngestReport};
use super::service::{
    MemoryIndexService, MemoryIndexStatus, MemorySearchRequest, MemorySearchResponse,
    MemorySessionDetail,
};
use super::types::IndexedSession;

/// Arguments shared by every command. `projectRoot` is a single path on
/// purpose: the memory index is per-project, and a list here would be the first
/// step toward serving one project's history as another's.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexScopeArgs {
    pub project_root: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexBuildArgs {
    pub project_root: String,
    #[serde(default)]
    pub full_rebuild: bool,
    #[serde(default)]
    pub index_unscoped: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexSearchArgs {
    pub project_root: String,
    #[serde(flatten)]
    pub request: MemorySearchRequest,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexListArgs {
    pub project_root: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_unscoped: bool,
    #[serde(default)]
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexSessionArgs {
    pub project_root: String,
    pub session_key: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_stale: bool,
}

/// Build or refresh a project's index.
///
/// The only mutating command, and the only one the UI is allowed to call from a
/// user action. Runs on a blocking thread: a full build walks tens of thousands
/// of files and took 154 s over this project's real corpus.
#[tauri::command]
pub async fn memory_index_build_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexBuildArgs,
) -> Result<IngestReport, String> {
    let service = Arc::clone(&service);
    log::info!(
        target: "se_manager::memory_index",
        "operation=memory_index_build full_rebuild={} index_unscoped={}",
        args.full_rebuild,
        args.index_unscoped
    );
    tokio::task::spawn_blocking(move || {
        service
            .build(
                &PathBuf::from(args.project_root),
                &IngestOptions {
                    full_rebuild: args.full_rebuild,
                    index_unscoped: args.index_unscoped,
                    ..IngestOptions::default()
                },
                &mut |_| {},
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index build join failed: {error}"))?
}

#[tauri::command]
pub async fn memory_index_status_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexScopeArgs,
) -> Result<MemoryIndexStatus, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .status(&PathBuf::from(args.project_root))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index status join failed: {error}"))?
}

#[tauri::command]
pub async fn memory_index_search_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexSearchArgs,
) -> Result<MemorySearchResponse, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .search(&PathBuf::from(args.project_root), &args.request)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index search join failed: {error}"))?
}

#[tauri::command]
pub async fn memory_index_sessions_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexListArgs,
) -> Result<Vec<IndexedSession>, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .list_sessions(
                &PathBuf::from(args.project_root),
                args.limit,
                args.include_unscoped,
                &args.agents,
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index sessions join failed: {error}"))?
}

#[tauri::command]
pub async fn memory_index_session_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexSessionArgs,
) -> Result<Option<MemorySessionDetail>, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .get_session(
                &PathBuf::from(args.project_root),
                &args.session_key,
                args.limit,
                args.include_stale,
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index session join failed: {error}"))?
}

/// The command line an external MCP client should be configured with for this
/// project.
///
/// Exists so the state root never has to be guessed: the host knows its own,
/// and hands out the exact invocation rather than documenting a platform path
/// that could drift from what the app actually uses.
#[tauri::command]
pub async fn memory_index_mcp_invocation_cmd(
    service: State<'_, Arc<MemoryIndexService>>,
    args: MemoryIndexScopeArgs,
) -> Result<Vec<String>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve executable: {error}"))?;
    Ok(super::stdio_mcp::invocation_for(
        &executable,
        &super::stdio_mcp::StdioConfig {
            project_root: PathBuf::from(args.project_root),
            state_root: service.state_root().to_path_buf(),
        },
    ))
}
