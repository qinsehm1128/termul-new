//! Shared terminal-agent chat transports. PTY success is NOT delivery acknowledgement.
use super::{ChatSnapshot, Delivery};

#[tauri::command]
pub async fn agent_chat_snapshot(
    terminal_id: String,
    after: Option<u64>,
    before: Option<u64>,
) -> Result<ChatSnapshot, String> {
    tokio::task::spawn_blocking(move || super::snapshot_page(&terminal_id, after, before))
        .await
        .map_err(|_| "chat worker unavailable".to_string())?
}

#[tauri::command]
pub async fn agent_chat_send(
    terminal_id: String,
    text: String,
    force: Option<bool>,
    delivery_id: Option<String>,
) -> Result<Delivery, String> {
    tokio::task::spawn_blocking(move || {
        super::send_with_options(
            &terminal_id,
            text,
            super::SendOptions {
                force: force.unwrap_or(false),
                delivery_id,
            },
            None,
        )
    })
    .await
    .map_err(|_| "chat worker unavailable".to_string())?
}

#[tauri::command]
pub async fn agent_chat_control(terminal_id: String, action: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || super::control(&terminal_id, &action))
        .await
        .map_err(|_| "chat worker unavailable".to_string())?
}
