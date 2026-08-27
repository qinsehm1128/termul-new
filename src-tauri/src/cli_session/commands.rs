//! Tauri IPC for CLI session discovery.

use super::{
    list_cli_sessions, resolve_cli_sessions, CliSessionListArgs, CliSessionListResult,
    CliSessionResolveArgs, CliSessionResolveResult,
};

#[tauri::command]
pub async fn list_cli_sessions_cmd(
    args: Option<CliSessionListArgs>,
) -> Result<CliSessionListResult, String> {
    let args = args.unwrap_or_default();
    log::info!(
        target: "termul::cli_session",
        "operation=list_cli_sessions_cmd scope_paths={}",
        args.scope_paths.as_ref().map(Vec::len).unwrap_or(0)
    );
    Ok(
        tokio::task::spawn_blocking(move || list_cli_sessions(args, None))
            .await
            .map_err(|err| format!("cli session scan join failed: {err}"))?,
    )
}

#[tauri::command]
pub async fn resolve_cli_sessions_cmd(
    args: CliSessionResolveArgs,
) -> Result<CliSessionResolveResult, String> {
    log::info!(
        target: "termul::cli_session",
        "operation=resolve_cli_sessions_cmd files={}",
        args.files.len()
    );
    Ok(
        tokio::task::spawn_blocking(move || resolve_cli_sessions(args))
            .await
            .map_err(|err| format!("cli session resolve join failed: {err}"))?,
    )
}
