//! Shared terminal-agent chat transports. PTY success is NOT delivery acknowledgement.
use super::{ChatSnapshot, Delivery};

#[tauri::command]
pub async fn agent_chat_snapshot(
    terminal_id: String,
    after: Option<u64>,
) -> Result<ChatSnapshot, String> {
    tokio::task::spawn_blocking(move || super::snapshot(&terminal_id, after))
        .await
        .map_err(|_| "chat worker unavailable".to_string())?
}

#[tauri::command]
pub async fn agent_chat_send(terminal_id: String, text: String) -> Result<Delivery, String> {
    tokio::task::spawn_blocking(move || super::send(&terminal_id, text))
        .await
        .map_err(|_| "chat worker unavailable".to_string())?
}

#[tauri::command]
pub async fn agent_chat_control(terminal_id: String, action: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || super::control(&terminal_id, &action))
        .await
        .map_err(|_| "chat worker unavailable".to_string())?
}
