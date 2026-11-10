//! Shared terminal-agent chat transports. PTY success is NOT delivery acknowledgement.
use super::{ChatError, ChatSnapshot, Delivery};

/// Worker join failure is a real fault, never an expected absence state.
fn worker_unavailable() -> ChatError {
    ChatError::Failed("chat worker unavailable".into())
}

#[tauri::command]
pub async fn agent_chat_snapshot(
    terminal_id: String,
    after: Option<u64>,
    before: Option<u64>,
) -> Result<ChatSnapshot, String> {
    tokio::task::spawn_blocking(move || super::snapshot_page(&terminal_id, after, before))
        .await
        .unwrap_or_else(|_| Err(worker_unavailable()))
        .map_err(|error| {
            error.log_detail();
            error.code().to_string()
        })
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
    .unwrap_or_else(|_| Err(worker_unavailable()))
    .map_err(|error| {
            error.log_detail();
            error.code().to_string()
        })
}

#[tauri::command]
pub async fn agent_chat_control(terminal_id: String, action: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || super::control(&terminal_id, &action))
        .await
        .unwrap_or_else(|_| Err(worker_unavailable()))
        .map_err(|error| {
            error.log_detail();
            error.code().to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tauri_transport_returns_the_same_stable_codes_as_the_web_facade() {
        for (error, code) in [
            (ChatError::NoHook, "AGENT_CHAT_NO_HOOK"),
            (ChatError::UnsupportedAgent, "UNSUPPORTED_AGENT_CHAT"),
            (ChatError::Failed("worker detail".into()), "AGENT_CHAT_ERROR"),
        ] {
            assert_eq!(error.code(), code);
            assert_eq!(error.code().to_string(), code);
        }
        assert_eq!(worker_unavailable().code(), "AGENT_CHAT_ERROR");
        assert!(!worker_unavailable().is_expected());
    }
}
