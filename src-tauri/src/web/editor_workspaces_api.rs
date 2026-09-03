//! HTTP handlers for host editor-workspace discovery.

use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::web::projects_api::IpcBody;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseCodeWorkspaceRequest {
    pub path: String,
}

pub async fn list() -> impl IntoResponse {
    Json(IpcBody::ok(
        crate::editor_workspaces::discover_editor_workspaces(),
    ))
}

pub async fn parse(Json(body): Json<ParseCodeWorkspaceRequest>) -> impl IntoResponse {
    match crate::editor_workspaces::parse_code_workspace_file(std::path::Path::new(&body.path)) {
        Ok(list) => {
            tracing::info!(
                target = "se_manager::editor_workspaces",
                operation = "parse_workspace",
                count = list.candidates.len(),
                stable_code = "OK",
                "parsed code-workspace file"
            );
            Json(IpcBody::ok(list))
        }
        Err(error) => Json(IpcBody::err(error, "WORKSPACE_PARSE_FAILED")),
    }
}
