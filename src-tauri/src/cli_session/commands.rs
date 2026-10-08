//! Tauri IPC for CLI session discovery.

use super::{
    detect_live_agent_sessions, list_cli_sessions, resolve_cli_sessions, CliSessionListArgs,
    CliSessionListResult, CliSessionResolveArgs, CliSessionResolveResult, LiveAgentSession,
};

#[tauri::command]
pub async fn list_cli_sessions_cmd(
    args: Option<CliSessionListArgs>,
) -> Result<CliSessionListResult, String> {
    let args = args.unwrap_or_default();
    log::info!(
        target: "se_manager::cli_session",
        "operation=list_cli_sessions_cmd scope_paths={}",
        args.scope_paths.as_ref().map(Vec::len).unwrap_or(0)
    );
    tokio::task::spawn_blocking(move || list_cli_sessions(args, None))
        .await
        .map_err(|err| format!("cli session scan join failed: {err}"))
}

#[tauri::command]
pub async fn resolve_cli_sessions_cmd(
    args: CliSessionResolveArgs,
) -> Result<CliSessionResolveResult, String> {
    log::info!(
        target: "se_manager::cli_session",
        "operation=resolve_cli_sessions_cmd files={}",
        args.files.len()
    );
    tokio::task::spawn_blocking(move || resolve_cli_sessions(args))
        .await
        .map_err(|err| format!("cli session resolve join failed: {err}"))
}

/// Agent sessions running under the given terminal shell pids. Desktop only:
/// it reads this machine's process table.
#[tauri::command]
pub async fn detect_live_agent_sessions_cmd(
    root_pids: Vec<u32>,
) -> Result<Vec<LiveAgentSession>, String> {
    tokio::task::spawn_blocking(move || detect_live_agent_sessions(&root_pids))
        .await
        .map_err(|err| format!("live agent session detection join failed: {err}"))
}
