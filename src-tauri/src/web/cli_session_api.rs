//! HTTP handlers for CLI session discovery (desktop shared-live + termul-server).

use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tokio::task::spawn_blocking;
use tracing::{error, info};

use crate::cli_session::{
    list_cli_sessions, resolve_cli_sessions, CliSessionListArgs, CliSessionListResult,
    CliSessionResolveArgs, CliSessionResolveResult,
};
use crate::web::fs_api::IpcBody;
use crate::web::project_registry::ProjectRegistry;
use crate::web::ws::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionsQuery {
    pub limit: Option<usize>,
    pub force: Option<bool>,
}

pub fn allowed_scope_roots_from_registry(registry: &ProjectRegistry) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = registry.default_project_path() {
        if !path.trim().is_empty() {
            roots.push(PathBuf::from(path));
        }
    }
    for project in registry.snapshot().projects {
        if project.is_archived {
            continue;
        }
        if let Some(path) = project.path.filter(|value| !value.trim().is_empty()) {
            roots.push(PathBuf::from(path));
        }
    }
    roots
}

fn allowed_scope_roots(state: &AppState) -> Vec<PathBuf> {
    let mut roots = allowed_scope_roots_from_registry(&state.registry);
    let project_root = state.project_root.read().clone();
    if !project_root.as_os_str().is_empty() {
        roots.push(project_root);
    }
    roots
}

async fn run_list(state: &AppState, args: CliSessionListArgs) -> impl IntoResponse {
    let allowed = allowed_scope_roots(state);
    info!(
        target: "se_manager::web::cli_session_api",
        "operation=list_cli_sessions scope_paths={}",
        args.scope_paths.as_ref().map(Vec::len).unwrap_or(0)
    );
    let body = match spawn_blocking(move || list_cli_sessions(args, Some(&allowed))).await {
        Ok(result) => IpcBody::ok(result),
        Err(err) => {
            error!(
                target: "se_manager::web::cli_session_api",
                "operation=list_cli_sessions_failed error=join"
            );
            IpcBody::<CliSessionListResult>::err(err.to_string(), "SCAN_FAILED")
        }
    };
    (StatusCode::OK, Json(body))
}

pub async fn list_get(
    State(state): State<AppState>,
    Query(query): Query<CliSessionsQuery>,
) -> impl IntoResponse {
    run_list(
        &state,
        CliSessionListArgs {
            limit: query.limit,
            force: query.force,
            ..CliSessionListArgs::default()
        },
    )
    .await
}

pub async fn list_post(
    State(state): State<AppState>,
    Json(args): Json<CliSessionListArgs>,
) -> impl IntoResponse {
    run_list(&state, args).await
}

pub async fn resolve_post(Json(args): Json<CliSessionResolveArgs>) -> impl IntoResponse {
    info!(
        target: "se_manager::web::cli_session_api",
        "operation=resolve_cli_sessions files={}",
        args.files.len()
    );
    let body = match spawn_blocking(move || resolve_cli_sessions(args)).await {
        Ok(result) => IpcBody::ok(result),
        Err(err) => {
            error!(
                target: "se_manager::web::cli_session_api",
                "operation=resolve_cli_sessions_failed error=join"
            );
            IpcBody::<CliSessionResolveResult>::err(err.to_string(), "RESOLVE_FAILED")
        }
    };
    (StatusCode::OK, Json(body))
}
