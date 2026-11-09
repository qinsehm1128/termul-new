use super::{
    install::{self, HookInstallStatus},
    AgentHookEvent, AgentKind,
};

#[tauri::command]
pub async fn agent_hooks_status() -> Result<Vec<HookInstallStatus>, String> {
    tokio::task::spawn_blocking(install::statuses)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn agent_hooks_preview(agent: AgentKind) -> Result<String, String> {
    tokio::task::spawn_blocking(move || install::preview(agent))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn agent_hooks_install(agent: AgentKind) -> Result<HookInstallStatus, String> {
    tokio::task::spawn_blocking(move || install::install(agent))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn agent_hooks_uninstall(agent: AgentKind) -> Result<HookInstallStatus, String> {
    tokio::task::spawn_blocking(move || install::uninstall(agent))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn agent_hook_events(after: Option<u64>) -> Vec<AgentHookEvent> {
    super::events(after.unwrap_or(0))
}
