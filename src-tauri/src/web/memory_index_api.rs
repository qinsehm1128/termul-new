//! HTTP handlers for the memory index (desktop shared-live + se-server).
//!
//! Mirrors `memory_index::commands` one-for-one and reuses the same
//! [`MemoryIndexService`], per the repository's rule that web route modules
//! mirror their Tauri command domains rather than duplicating behavior.
//!
//! Every handler takes the project root explicitly. Unlike the injected MCP
//! child — which resolves it from the calling agent's own session — these are
//! called by the app's own UI on behalf of a user who has that project open, so
//! naming it is correct.
//!
//! What the fence inside the service does **not** decide is whether this caller
//! may name that root at all: it enforces "exactly one project per request", not
//! "a project you are entitled to". Over HTTP the caller is not necessarily the
//! app's own UI — `termul-server` exists to be reached remotely — so the root is
//! checked against the project registry here first. See
//! [`authorized_project_root`].

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::path::{Component, Path, PathBuf};
use tokio::task::spawn_blocking;
use tracing::{error, info, warn};

use crate::memory_index::commands::{
    MemoryIndexBuildArgs, MemoryIndexListArgs, MemoryIndexScopeArgs, MemoryIndexSearchArgs,
    MemoryIndexSessionArgs,
};
use crate::memory_index::ingest::{IngestOptions, IngestReport};
use crate::memory_index::service::{MemoryIndexStatus, MemorySearchResponse, MemorySessionDetail};
use crate::memory_index::types::IndexedSession;
use crate::web::cli_session_api::allowed_scope_roots_from_registry;
use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

/// Error code returned when a caller names a project root it may not name.
pub const ERR_ROOT_NOT_REGISTERED: &str = "MEMORY_INDEX_ROOT_NOT_REGISTERED";

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

/// Reject a project root the caller has no claim to, before the service sees it.
///
/// `cli_session_api` does the same class of check for its own routes and is the
/// reason this exists — the desktop Tauri command passes no allowlist because
/// local IPC is the app itself, while the HTTP route passes one because the
/// caller may be remote. The memory index had only the first half.
///
/// One thing is deliberately **stricter** than `cli_session::scope`, whose
/// predicate also accepts a path that *contains* a registered project
/// (`root.starts_with(path)`). That leniency is harmless there: `scopePaths`
/// only choose which vendor folders get walked, and every session still carries
/// its own `cwd`. Here the named root *becomes* the fence and the index
/// namespace, so accepting an ancestor would let `{"projectRoot": "/Users/qs"}`
/// build and then read one index over every project on the machine.
///
/// Fails closed: with no registered project and no current project root there is
/// nothing a remote caller may name.
fn authorized_project_root(state: &AppState, requested: &str) -> Result<PathBuf, String> {
    let trimmed = requested.trim();
    if trimmed.is_empty() {
        return Err("projectRoot is required".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("projectRoot must be absolute and must not contain '..'".to_string());
    }

    let mut roots = allowed_scope_roots_from_registry(&state.registry);
    let current = state.project_root.read().clone();
    if !current.as_os_str().is_empty() {
        roots.push(current);
    }
    if is_registered(&path, &roots) {
        return Ok(path);
    }
    warn!(
        target: "se_manager::web::memory_index_api",
        "operation=memory_index_root_rejected registered_roots={}",
        roots.len()
    );
    Err(format!(
        "{} is not a registered project root",
        path.display()
    ))
}

/// Component-wise containment, one direction only. `Path::starts_with` compares
/// whole components, so `/repo-fork` is not inside `/repo`.
fn is_registered(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| !root.as_os_str().is_empty() && (path == root || path.starts_with(root)))
}

/// Resolve the requested root or return the shared rejection body.
macro_rules! root_or_reject {
    ($state:expr, $requested:expr, $body:ty) => {
        match authorized_project_root(&$state, &$requested) {
            Ok(root) => root,
            Err(message) => {
                return (
                    StatusCode::OK,
                    Json(IpcBody::<$body>::err(message, ERR_ROOT_NOT_REGISTERED)),
                )
            }
        }
    };
}

pub async fn build_post(
    State(state): State<AppState>,
    Json(args): Json<MemoryIndexBuildArgs>,
) -> impl IntoResponse {
    // Authorization first: an unauthorized root is rejected the same way
    // whether or not this host happens to have the service wired up.
    let project_root = root_or_reject!(state, args.project_root, IngestReport);
    let service = service_or_unavailable!(state, IngestReport);
    info!(
        target: "se_manager::web::memory_index_api",
        "operation=memory_index_build full_rebuild={}",
        args.full_rebuild
    );
    let body = match spawn_blocking(move || {
        service.build(
            &project_root,
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
    // Authorization first: an unauthorized root is rejected the same way
    // whether or not this host happens to have the service wired up.
    let project_root = root_or_reject!(state, args.project_root, MemoryIndexStatus);
    let service = service_or_unavailable!(state, MemoryIndexStatus);
    let body = match spawn_blocking(move || service.status(&project_root)).await
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
    // Authorization first: an unauthorized root is rejected the same way
    // whether or not this host happens to have the service wired up.
    let project_root = root_or_reject!(state, args.project_root, MemorySearchResponse);
    let service = service_or_unavailable!(state, MemorySearchResponse);
    info!(
        target: "se_manager::web::memory_index_api",
        "operation=memory_index_search agents={}",
        args.request.agents.len()
    );
    let body = match spawn_blocking(move || {
        service.search(&project_root, &args.request)
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
    // Authorization first: an unauthorized root is rejected the same way
    // whether or not this host happens to have the service wired up.
    let project_root = root_or_reject!(state, args.project_root, Vec<IndexedSession>);
    let service = service_or_unavailable!(state, Vec<IndexedSession>);
    let body = match spawn_blocking(move || {
        service.list_sessions(
            &project_root,
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
    // Authorization first: an unauthorized root is rejected the same way
    // whether or not this host happens to have the service wired up.
    let project_root = root_or_reject!(state, args.project_root, Option<MemorySessionDetail>);
    let service = service_or_unavailable!(state, Option<MemorySessionDetail>);
    let body = match spawn_blocking(move || {
        service.get_session(
            &project_root,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpManager;
    use crate::web::project_registry::ProjectRegistry;
    use crate::web::test_pty_manager;
    use crate::web::sink::WsRelaySink;
    use crate::web::ws::HistoryMode;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use axum::Router;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state(project_root: &Path) -> AppState {
        let pty = test_pty_manager();
        AppState {
            acp: Arc::new(AcpManager::new(vec![])),
            terminal_events: pty.terminal_events(),
            cwd_tracker: pty.cwd_tracker(),
            git_tracker: pty.git_tracker(),
            exit_code_tracker: pty.exit_code_tracker(),
            pty,
            relay: Arc::new(WsRelaySink::new()),
            registry: Arc::new(ProjectRegistry::new()),
            registry_persistence: None,
            projects_file: None,
            history_mode: HistoryMode::LiveOnly,
            conversation: None,
            conversation_creation: None,
            project_root: Arc::new(parking_lot::RwLock::new(project_root.to_path_buf())),
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            memory_index: None,
            store: None,
        }
    }

    /// The finding this check exists for: an authenticated remote caller naming
    /// `$HOME` would otherwise get one index built over every project on the
    /// machine, because the service's fence enforces "one root per request", not
    /// "a root you are entitled to".
    #[test]
    fn an_ancestor_of_a_registered_project_is_rejected() {
        let state = test_state(Path::new("/Users/qs/project/me/termul"));
        let error = authorized_project_root(&state, "/Users/qs").unwrap_err();
        assert!(
            error.contains("not a registered project root"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_current_project_root_is_accepted() {
        let state = test_state(Path::new("/Users/qs/project/me/termul"));
        assert_eq!(
            authorized_project_root(&state, "/Users/qs/project/me/termul").unwrap(),
            PathBuf::from("/Users/qs/project/me/termul")
        );
        // A directory inside it is still that project's tree.
        assert!(authorized_project_root(&state, "/Users/qs/project/me/termul/src").is_ok());
    }

    /// `Path::starts_with` compares whole components, which is the reason a
    /// sibling with a shared prefix is not admitted.
    #[test]
    fn a_sibling_sharing_a_name_prefix_is_not_inside_the_project() {
        let state = test_state(Path::new("/repo"));
        assert!(authorized_project_root(&state, "/repo-fork").is_err());
        assert!(authorized_project_root(&state, "/repo").is_ok());
    }

    #[test]
    fn traversal_and_relative_roots_are_rejected_before_any_lookup() {
        let state = test_state(Path::new("/repo"));
        for bad in ["", "   ", "relative/path", "/repo/../etc"] {
            assert!(
                authorized_project_root(&state, bad).is_err(),
                "{bad:?} should have been rejected"
            );
        }
    }

    /// With nothing registered there is nothing a remote caller may name.
    #[test]
    fn an_empty_registry_and_empty_project_root_admit_nothing() {
        let state = test_state(Path::new(""));
        assert!(authorized_project_root(&state, "/Users/qs").is_err());
        assert!(authorized_project_root(&state, "/").is_err());
    }

    /// End to end through the route, so the rejection body shape is verified and
    /// the check is proven to run before anything else in the handler.
    #[tokio::test]
    async fn the_route_rejects_an_unregistered_root_with_the_scope_error_code() {
        let state = test_state(Path::new("/Users/qs/project/me/termul"));
        let app = Router::new()
            .route("/memory-index/build", post(build_post))
            .with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/memory-index/build")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"projectRoot":"/Users/qs"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["success"], serde_json::json!(false));
        assert_eq!(
            body["code"],
            serde_json::json!(ERR_ROOT_NOT_REGISTERED),
            "body was {body}"
        );
    }
}
