//! HTTP handlers for the memory index (desktop shared-live + se-server).
//!
//! Mirrors `memory_index::commands` one-for-one and reuses the same
//! [`MemoryIndexService`], per the repository's rule that web route modules
//! mirror their Tauri command domains rather than duplicating behavior.
//!
//! Every handler takes the project root explicitly. Unlike the injected MCP
//! child — which resolves it from the calling agent's own session — these are
//! called by the app's own UI on behalf of a user who has that project open, so
//! naming it is correct. The single-project fence inside the service is what
//! makes naming it safe.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::path::PathBuf;
use tokio::task::spawn_blocking;
use tracing::{error, info};

use crate::memory_index::commands::{
    MemoryIndexBuildArgs, MemoryIndexListArgs, MemoryIndexScopeArgs, MemoryIndexSearchArgs,
    MemoryIndexSessionArgs,
};
use crate::memory_index::ingest::{IngestOptions, IngestReport};
use crate::memory_index::service::{MemoryIndexStatus, MemorySearchResponse, MemorySessionDetail};
use crate::memory_index::types::IndexedSession;
use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

/// Resolve the service, or report the same failure shape every handler uses.
macro_rules! service_or_unavailable {
    ($state:expr, $body:ty) => {
        match $state.memory_index.clone() {
            Some(service) => service,
            None => {
                return (
                    StatusCode::OK,
                    Json(IpcBody::<$body>::err(
                        "memory index service unavailable".to_string(),
                        "MEMORY_INDEX_UNAVAILABLE",
                    )),
                )
            }
        }
    };
}

pub async fn build_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexBuildArgs>,
) -> impl IntoResponse {
    let service = service_or_unavailable!(state, IngestReport);
    info!(
        target: "se_manager::web::memory_index_api",
        "operation=memory_index_build full_rebuild={}",
        args.full_rebuild
    );
    let body = match spawn_blocking(move || {
        service.build(
            &PathBuf::from(args.project_root),
            &IngestOptions {
                full_rebuild: args.full_rebuild,
                index_unscoped: args.index_unscoped,
                ..IngestOptions::default()
            },
            &mut |_| {},
        )
    })
    .await
    {
        Ok(Ok(report)) => IpcBody::ok(report),
        Ok(Err(error)) => {
            error!(
                target: "se_manager::web::memory_index_api",
                "operation=memory_index_build_failed code={}",
                error.code
            );
            IpcBody::<IngestReport>::err(error.detail, error.code)
        }
        Err(error) => IpcBody::<IngestReport>::err(error.to_string(), "MEMORY_INDEX_BUILD_FAILED"),
    };
    (StatusCode::OK, Json(body))
}

pub async fn status_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexScopeArgs>,
) -> impl IntoResponse {
    let service = service_or_unavailable!(state, MemoryIndexStatus);
    let body = match spawn_blocking(move || service.status(&PathBuf::from(args.project_root))).await
    {
        Ok(Ok(status)) => IpcBody::ok(status),
        Ok(Err(error)) => IpcBody::<MemoryIndexStatus>::err(error.detail, error.code),
        Err(error) => {
            IpcBody::<MemoryIndexStatus>::err(error.to_string(), "MEMORY_INDEX_STATUS_FAILED")
        }
    };
    (StatusCode::OK, Json(body))
}

pub async fn search_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexSearchArgs>,
) -> impl IntoResponse {
    let service = service_or_unavailable!(state, MemorySearchResponse);
    info!(
        target: "se_manager::web::memory_index_api",
        "operation=memory_index_search agents={}",
        args.request.agents.len()
    );
    let body = match spawn_blocking(move || {
        service.search(&PathBuf::from(args.project_root), &args.request)
    })
    .await
    {
        Ok(Ok(response)) => IpcBody::ok(response),
        Ok(Err(error)) => IpcBody::<MemorySearchResponse>::err(error.detail, error.code),
        Err(error) => {
            IpcBody::<MemorySearchResponse>::err(error.to_string(), "MEMORY_INDEX_SEARCH_FAILED")
        }
    };
    (StatusCode::OK, Json(body))
}

pub async fn sessions_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexListArgs>,
) -> impl IntoResponse {
    let service = service_or_unavailable!(state, Vec<IndexedSession>);
    let body = match spawn_blocking(move || {
        service.list_sessions(
            &PathBuf::from(args.project_root),
            args.limit,
            args.include_unscoped,
            &args.agents,
        )
    })
    .await
    {
        Ok(Ok(sessions)) => IpcBody::ok(sessions),
        Ok(Err(error)) => IpcBody::<Vec<IndexedSession>>::err(error.detail, error.code),
        Err(error) => {
            IpcBody::<Vec<IndexedSession>>::err(error.to_string(), "MEMORY_INDEX_LIST_FAILED")
        }
    };
    (StatusCode::OK, Json(body))
}

pub async fn session_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexSessionArgs>,
) -> impl IntoResponse {
    let service = service_or_unavailable!(state, Option<MemorySessionDetail>);
    let body = match spawn_blocking(move || {
        service.get_session(
            &PathBuf::from(args.project_root),
            &args.session_key,
            args.limit,
            args.include_stale,
        )
    })
    .await
    {
        Ok(Ok(detail)) => IpcBody::ok(detail),
        Ok(Err(error)) => IpcBody::<Option<MemorySessionDetail>>::err(error.detail, error.code),
        Err(error) => IpcBody::<Option<MemorySessionDetail>>::err(
            error.to_string(),
            "MEMORY_INDEX_SESSION_FAILED",
        ),
    };
    (StatusCode::OK, Json(body))
}
