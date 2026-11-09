//! Tauri IPC for CLI session discovery.

use super::{
    list_cli_sessions, resolve_cli_sessions, CliSessionListArgs, CliSessionListResult,
    CliSessionResolveArgs, CliSessionResolveResult, LiveAgentSession,
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
    terminal: tauri::State<'_, crate::core::TerminalServiceHandle>,
) -> Result<Vec<LiveAgentSession>, String> {
    let identities = super::live::terminal_hook_identities(&terminal)
        .await
        .into_iter()
        .filter(|t| root_pids.contains(&t.0))
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || super::live::detect_with_hooks(&identities))
        .await
        .map_err(|err| format!("live agent session detection join failed: {err}"))
}
