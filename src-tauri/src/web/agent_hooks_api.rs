//! Authenticated remote facade. Raw hook ingestion is never mounted here.
use super::fs_api::IpcBody;
use crate::agent_hooks::{self, install::HookInstallStatus, AgentHookEvent, AgentKind};
use axum::{extract::Query, Json};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AgentRequest {
    agent: AgentKind,
}
#[derive(Default, Deserialize)]
pub struct Cursor {
    after: Option<u64>,
}
fn result<T>(result: Result<T, String>) -> Json<IpcBody<T>> {
    Json(match result {
        Ok(value) => IpcBody::ok(value),
        Err(message) => IpcBody::err(message, "AGENT_HOOK_ERROR"),
    })
}
pub async fn status() -> Json<IpcBody<Vec<HookInstallStatus>>> {
    result(agent_hooks::commands::agent_hooks_status().await)
}
pub async fn preview(Query(body): Query<AgentRequest>) -> Json<IpcBody<String>> {
    result(agent_hooks::commands::agent_hooks_preview(body.agent).await)
}
pub async fn install(Json(body): Json<AgentRequest>) -> Json<IpcBody<HookInstallStatus>> {
    result(agent_hooks::commands::agent_hooks_install(body.agent).await)
}
pub async fn uninstall(Json(body): Json<AgentRequest>) -> Json<IpcBody<HookInstallStatus>> {
    result(agent_hooks::commands::agent_hooks_uninstall(body.agent).await)
}
pub async fn events(Query(cursor): Query<Cursor>) -> Json<IpcBody<Vec<AgentHookEvent>>> {
    Json(IpcBody::ok(agent_hooks::events(cursor.after.unwrap_or(0))))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn events_use_the_same_ipc_envelope_as_the_renderer_facade() {
        let axum::Json(body) = super::events(axum::extract::Query(super::Cursor {
            after: Some(u64::MAX),
        }))
        .await;
        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["success"], true);
        assert!(value["data"].is_array());
    }
}
