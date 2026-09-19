//! Tauri IPC for the memory index.
//!
//! Mirrors `web::memory_index_api` one-for-one and calls the same
//! [`MemoryIndexService`]; a capability that answered differently on desktop
//! than over HTTP would be a parity bug by construction.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use super::ingest::{IngestOptions, IngestProgress, IngestReport};
use super::service::{
    MemoryIndexService, MemoryIndexStatus, MemorySearchRequest, MemorySearchResponse,
    MemorySessionDetail,
};
use super::types::IndexedSession;

/// Optional in-process memory-index service. `None` in ACP-Core desktop mode
/// (commands proxy over `AcpServiceHandle`).
#[derive(Clone, Default)]
pub struct HostMemoryIndex(pub Option<Arc<crate::memory_index::service::MemoryIndexService>>);

async fn via_core<T: DeserializeOwned>(
    client: &crate::core::AcpCoreClient,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let value = client
        .request(method, params)
        .await
        .map_err(|error| error.command_message())?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn require_service(host: &HostMemoryIndex) -> Result<Arc<MemoryIndexService>, String> {
    host.0
        .clone()
        .ok_or_else(|| "memory index service unavailable".to_string())
}

/// Progress event name.
///
/// The `acp:` prefix is the app's existing event-bus convention, not a claim
/// that this is an ACP event: `AcpTransport::onEvent` is the one subscription
/// path the renderer has that works on both surfaces, and it maps `acp:x` to the
/// WebSocket type `x`. Inventing a second bus for one event would mean the
/// browser client silently gets no progress at all.
pub const MEMORY_INDEX_PROGRESS_EVENT: &str = "acp:memory_index_progress";

/// Shortest gap between two progress events.
///
/// The walk reports once per file — 3139 of them on the measured corpus, and an
/// incremental pass skips through unchanged files far faster than a human eye
/// or a React render can follow. Throttling here rather than in `ingest` keeps
/// the walk's own reporting exact for tests while bounding what crosses the IPC
/// boundary. The last event of a build is always sent (see `throttled`).
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(150);

/// Wrap a progress sink so it fires at most once per [`PROGRESS_MIN_INTERVAL`],
/// plus always on the first and last file of each vendor.
///
/// "Last file" is knowable without lookahead because [`IngestProgress`] carries
/// `files_seen` and `files_total`: the final event of a vendor is the one where
/// they are equal, and that is the event a progress bar must not miss or it
/// sticks at 97%.
pub(crate) fn throttled(mut sink: impl FnMut(&IngestProgress)) -> impl FnMut(IngestProgress) {
    let mut last_sent: Option<Instant> = None;
    move |progress: IngestProgress| {
        let boundary = progress.files_seen <= 1 || progress.files_seen >= progress.files_total;
        let due = last_sent.is_none_or(|at| at.elapsed() >= PROGRESS_MIN_INTERVAL);
        if boundary || due {
            last_sent = Some(Instant::now());
            sink(&progress);
        }
    }
}

/// Arguments shared by every command. `projectRoot` is a single path on
/// purpose: the memory index is per-project, and a list here would be the first
/// step toward serving one project's history as another's.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexScopeArgs {
    pub project_root: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexBuildArgs {
    pub project_root: String,
    #[serde(default)]
    pub full_rebuild: bool,
    #[serde(default)]
    pub index_unscoped: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexSearchArgs {
    pub project_root: String,
    #[serde(flatten)]
    pub request: MemorySearchRequest,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexSessionArgs {
    pub project_root: String,
    pub session_key: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_stale: bool,
    /// Mirrors the same field on the list and search requests: a session the
    /// listing surfaced must be openable on the same terms.
    #[serde(default)]
    pub include_unscoped: bool,
}

/// Build or refresh a project's index.
///
/// The only mutating command, and the only one the UI is allowed to call from a
/// user action. Runs on a blocking thread: a full build walks tens of thousands
/// of files and took 154 s over this project's real corpus.
#[tauri::command]
pub async fn memory_index_build_cmd(
    app: AppHandle,
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexBuildArgs,
) -> Result<IngestReport, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memoryBuild",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
    log::info!(
        target: "se_manager::memory_index",
        "operation=memory_index_build full_rebuild={} index_unscoped={}",
        args.full_rebuild,
        args.index_unscoped
    );
    tokio::task::spawn_blocking(move || {
        let mut emit = throttled(move |progress: &IngestProgress| {
            let _ = app.emit(MEMORY_INDEX_PROGRESS_EVENT, progress);
        });
        service
            .build(
                &PathBuf::from(args.project_root),
                &IngestOptions {
                    full_rebuild: args.full_rebuild,
                    index_unscoped: args.index_unscoped,
                    ..IngestOptions::default()
                },
                &mut emit,
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("memory index build join failed: {error}"))?
}

/// Ask a running build to stop. `false` means nothing was running.
#[tauri::command]
pub async fn memory_index_cancel_cmd(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexScopeArgs,
) -> Result<bool, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memoryCancel",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    require_service(host.inner())?
        .cancel_build(&PathBuf::from(args.project_root))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn memory_index_status_cmd(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexScopeArgs,
) -> Result<MemoryIndexStatus, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memoryStatus",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
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
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexSearchArgs,
) -> Result<MemorySearchResponse, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memorySearch",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
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
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexListArgs,
) -> Result<Vec<IndexedSession>, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memorySessions",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
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
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexSessionArgs,
) -> Result<Option<MemorySessionDetail>, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memorySession",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
    tokio::task::spawn_blocking(move || {
        service
            .get_session(
                &PathBuf::from(args.project_root),
                &args.session_key,
                args.limit,
                args.include_stale,
                args.include_unscoped,
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
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
    args: MemoryIndexScopeArgs,
) -> Result<Vec<String>, String> {
    if let Some(client) = acp.core_client() {
        return via_core(
            client.as_ref(),
            "memoryMcpInvocation",
            serde_json::to_value(&args).map_err(|error| error.to_string())?,
        )
        .await;
    }
    let service = require_service(host.inner())?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve executable: {error}"))?;
    Ok(super::stdio_mcp::invocation_for(
        &executable,
        &super::stdio_mcp::StdioConfig {
            project_root: Some(PathBuf::from(args.project_root)),
            state_root: service.state_root().to_path_buf(),
        },
    ))
}

/// The universal memory MCP invocation: one server that lists every indexed
/// project and lets the client pick per query. No project argument — the
/// project selection happens inside the server, not in the client config.
#[tauri::command]
pub async fn memory_index_universal_mcp_invocation_cmd(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostMemoryIndex>,
) -> Result<Vec<String>, String> {
    if let Some(client) = acp.core_client() {
        return via_core(client.as_ref(), "memoryUniversalMcpInvocation", json!({})).await;
    }
    let service = require_service(host.inner())?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve executable: {error}"))?;
    Ok(super::stdio_mcp::universal_invocation_for(
        &executable,
        service.state_root(),
    ))
}
