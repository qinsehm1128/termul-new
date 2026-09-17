//! HTTP handlers for agent skills discovery exposed to the web/remote client
//! (CAP-2: Web & Mobile 1:1 Parity).
//!
//! Mirrors the desktop `#[tauri::command] list_agent_skills_cmd` /
//! `read_agent_skill_cmd` handlers over HTTP, reusing the SAME pure-Rust
//! skill discovery logic in `crate::skills` (`list_agent_skills`,
//! `read_agent_skill`). These functions read `~/.agents/skills/` (global) +
//! `{project}/.agents/skills/` (project-local) — no `AppHandle` needed, so they
//! work as-is on the standalone server.
//!
//! Each route:
//! - wraps results in `IpcBody<T>` so the renderer facade swaps transparently
//!   with the desktop command shape.
//! - runs blocking fs calls on `tokio::task::spawn_blocking`.
//! - logs at route boundaries via `tracing` (the standalone server's logger;
//!   a no-op when no subscriber is installed on the desktop shared-live path).
//! - degrades gracefully: scan failure returns an empty list, never throws
//!   (so the slash menu stays usable on web).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;

use crate::skills::api_types::{
    SkillsCatalogRequest, SkillsError, SkillsInstallCommit, SkillsInstallRequest,
    SkillsOperationStart, SkillsOperationStatus, SkillsPreviewRequest, SkillsProjectionRequest,
    SkillsRepairRequest, SkillsStatus, ERR_CANONICAL_ROOT_UNAVAILABLE,
};
use crate::skills::scanner;
use crate::skills::service::SkillsHubService;
use crate::skills::{AgentSkillContent, AgentSkillSummary};
use crate::web::auth::IngressProvenance;
use crate::web::fs_api::IpcBody;
use crate::web::operation_policy::{self, LocalOnlyOperation};
use crate::web::ws::AppState;

/// `GET /skills?projectRoot=` query. `projectRoot` is optional: when omitted,
/// only global skills (`~/.agents/skills/`) are listed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsQuery {
    pub project_root: Option<String>,
    pub project_id: Option<String>,
}

/// `GET /skills/:name?projectRoot=` path + query. Mirrors
/// `read_agent_skill(name, project_root)`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillNamePath {
    pub name: String,
}

fn unavailable<T>(code: &str) -> (StatusCode, Json<IpcBody<T>>) {
    (
        StatusCode::OK,
        Json(IpcBody::<T>::err(
            "skills hub is unavailable".to_string(),
            code,
        )),
    )
}

fn ipc_from_skills<T>(result: Result<T, SkillsError>) -> (StatusCode, Json<IpcBody<T>>) {
    match result {
        Ok(value) => (StatusCode::OK, Json(IpcBody::ok(value))),
        Err(error) => (
            StatusCode::OK,
            Json(IpcBody::<T>::err(error.message, error.code)),
        ),
    }
}

async fn with_service<T, F>(state: AppState, work: F) -> (StatusCode, Json<IpcBody<T>>)
where
    T: Send + 'static,
    F: FnOnce(std::sync::Arc<SkillsHubService>) -> Result<T, SkillsError> + Send + 'static,
{
    let Some(service) = state.skills_hub.clone() else {
        return unavailable(ERR_CANONICAL_ROOT_UNAVAILABLE);
    };
    match tokio::task::spawn_blocking(move || work(service)).await {
        Ok(result) => ipc_from_skills(result),
        Err(error) => (
            StatusCode::OK,
            Json(IpcBody::<T>::err(
                format!("skills task failed: {error}"),
                "SKILLS_STATUS_ERROR",
            )),
        ),
    }
}

/// `GET /skills/status` — provider-aware catalog status.
pub async fn status(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Query(query): Query<SkillsQuery>,
) -> impl IntoResponse {
    let request = SkillsCatalogRequest {
        project_id: query.project_id,
        project_root: query.project_root,
    };
    with_service(state, move |service| {
        let mut status = service.status(request)?;
        status.watched_roots.clear();
        if !provenance.allows_local_operator_mutation() {
            for skill in &mut status.catalog.skills {
                for source in &mut skill.sources {
                    source.skill_md_path.clear();
                }
            }
            status.catalog.diagnostics.clear();
        }
        Ok(status)
    })
    .await
}

/// `POST /skills/sync` — rescan and persist catalog.
pub async fn sync(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsCatalogRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<SkillsStatus>::err(denial.message, denial.code)),
        );
    }
    with_service(state, move |service| service.sync(request)).await
}

/// `POST /skills/preview` — host-side source preview without canonical writes.
pub async fn preview(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsPreviewRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<SkillsOperationStart>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    with_service(state, move |service| service.start_preview_job(request)).await
}

/// `POST /skills/install-preview` — commit a validated host-side preview.
pub async fn install_preview(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsInstallCommit>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<SkillsOperationStart>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    with_service(state, move |service| service.start_install_job(request)).await
}

/// `GET /skills/operations/:job_id` — operation status polling.
pub async fn operation_status(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<SkillsOperationStatus>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    with_service(state, move |service| service.operation_status(&job_id)).await
}

/// `POST /skills/operations/cancel` — cancel a pending operation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelSkillOperationRequest {
    pub job_id: String,
}

pub async fn cancel_operation(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<CancelSkillOperationRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<()>::err(denial.message, denial.code)),
        );
    }
    with_service(state, move |service| {
        service.cancel_operation(&request.job_id)
    })
    .await
}

/// `POST /skills/install` — canonical install plus optional projection.
pub async fn install(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsInstallRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<crate::skills::manifest::SkillManifest>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    let Some(service) = state.skills_hub.as_ref() else {
        return unavailable(ERR_CANONICAL_ROOT_UNAVAILABLE);
    };
    let source = std::path::Path::new(&request.source_path);
    let allowed = source.is_absolute()
        && source.canonicalize().ok().is_some_and(|canonical| {
            scanner::path_is_within(service.root().path(), &canonical)
                || service
                    .registered_projects()
                    .iter()
                    .any(|project| scanner::path_is_within(&project.root, &canonical))
        });
    if !allowed {
        return (
            StatusCode::OK,
            Json(IpcBody::<crate::skills::manifest::SkillManifest>::err(
                "install source is outside the allowed host roots".to_string(),
                crate::skills::api_types::ERR_PROJECT_OUTSIDE_BOUNDARY,
            )),
        );
    }
    tracing::info!(scope = %request.scope, "skills install request");
    with_service(state, move |service| service.install(request)).await
}

/// `POST /skills/project` — aggregate/projection for an installed skill.
pub async fn project(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsProjectionRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<crate::skills::manifest::SkillManifest>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    with_service(state, move |service| service.project_skill(request)).await
}

/// `POST /skills/repair` — repair Termul-owned projections only.
pub async fn repair(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Json(request): Json<SkillsRepairRequest>,
) -> impl IntoResponse {
    if let Err(denial) =
        operation_policy::authorize_local_only(provenance, LocalOnlyOperation::SkillsMutation)
    {
        return (
            StatusCode::OK,
            Json(IpcBody::<crate::skills::manifest::SkillManifest>::err(
                denial.message,
                denial.code,
            )),
        );
    }
    with_service(state, move |service| service.repair(request)).await
}

/// `GET /skills?projectRoot=` — list installed agent skills. Reuses
/// `crate::skills::list_agent_skills` (same function the
/// `#[tauri::command] list_agent_skills_cmd` calls). Returns
/// `IpcBody::ok(Vec<AgentSkillSummary>)` or, on scan failure,
/// `IpcBody::ok(vec![])` (degrade — never throw, so the slash menu stays
/// usable on web).
pub async fn list(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    Query(q): Query<SkillsQuery>,
) -> impl IntoResponse {
    // Enforce `project_root` containment (web-server security boundary): a web
    // client must not probe skills under an arbitrary host path — only under
    // the server's `project_root` or any registered project root (mirrors
    // `/git/*` + `/search/*`). A non-existent projectRoot canonicalizes to Err
    // and is allowed through (no project skills scanned; only global skills —
    // harmless degrade).
    if let Some(pr) = &q.project_root {
        let Ok(canonical) = std::path::Path::new(pr).canonicalize() else {
            return (
                StatusCode::OK,
                Json(IpcBody::<Vec<AgentSkillSummary>>::err(
                    "project root does not exist".to_string(),
                    crate::skills::api_types::ERR_PROJECT_OUTSIDE_BOUNDARY,
                )),
            );
        };
        {
            // CAP-1: lock-read the live boundary (may have been rebound).
            // CAP-2: also check all registered project roots so a web client
            // that switched to a non-default project can list skills.
            // Scope in a block so the `!Send` guard drops before the
            // `spawn_blocking` `.await` (keeps the handler future `Send`).
            let outside_err = {
                let project_root = state.project_root.read();
                crate::web::git_api::ensure_within_project_boundary::<Vec<AgentSkillSummary>>(
                    &canonical,
                    &project_root,
                    &state.registry,
                )
            };
            if let Some(err) = outside_err {
                log::warn!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=REJECTED");
                return (StatusCode::OK, Json(err));
            }
        }
    }
    let project_root = q.project_root;
    let result = tokio::task::spawn_blocking(move || {
        crate::skills::list_agent_skills(project_root.as_deref())
    })
    .await
    .map_err(|e| format!("skills list task failed: {e}"));

    let body = match result {
        Ok(Ok(mut skills)) => {
            if !provenance.allows_local_operator_mutation() {
                for skill in &mut skills {
                    skill.path.clear();
                }
            }
            IpcBody::ok(skills)
        }
        Ok(Err(_e)) => {
            log::warn!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=REJECTED");
            // Degrade: return an empty list so the slash menu stays usable,
            // matching the desktop's `Promise.resolve([])` fallback contract.
            IpcBody::ok(Vec::<AgentSkillSummary>::new())
        }
        Err(_e) => {
            log::error!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=FAILED");
            IpcBody::ok(Vec::<AgentSkillSummary>::new())
        }
    };
    (StatusCode::OK, Json(body))
}

/// `GET /skills/:name?projectRoot=` — read a single skill's body. Reuses
/// `crate::skills::read_agent_skill` (same function the
/// `#[tauri::command] read_agent_skill_cmd` calls). Returns
/// `IpcBody::ok(AgentSkillContent)` or `IpcBody::err(msg, "SKILL_NOT_FOUND")`.
pub async fn read(
    State(state): State<AppState>,
    Extension(provenance): Extension<IngressProvenance>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(q): Query<SkillsQuery>,
) -> impl IntoResponse {
    // Enforce `project_root` containment (web-server security boundary) —
    // mirrors `/skills` (list). A non-existent projectRoot is allowed through.
    if let Some(pr) = &q.project_root {
        let Ok(canonical) = std::path::Path::new(pr).canonicalize() else {
            return (
                StatusCode::OK,
                Json(IpcBody::<AgentSkillContent>::err(
                    "project root does not exist".to_string(),
                    crate::skills::api_types::ERR_PROJECT_OUTSIDE_BOUNDARY,
                )),
            );
        };
        {
            // CAP-1: lock-read the live boundary (may have been rebound).
            // CAP-2: also check all registered project roots.
            // Scope in a block so the `!Send` guard drops before the
            // `spawn_blocking` `.await` (keeps the handler future `Send`).
            let outside_err = {
                let project_root = state.project_root.read();
                crate::web::git_api::ensure_within_project_boundary::<AgentSkillContent>(
                    &canonical,
                    &project_root,
                    &state.registry,
                )
            };
            if let Some(err) = outside_err {
                log::warn!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=REJECTED");
                return (StatusCode::OK, Json(err));
            }
        }
    }
    let project_root = q.project_root;
    let result = tokio::task::spawn_blocking(move || {
        crate::skills::read_agent_skill(&name, project_root.as_deref())
    })
    .await
    .map_err(|e| format!("skills read task failed: {e}"));

    let body = match result {
        Ok(Ok(mut content)) => {
            if !provenance.allows_local_operator_mutation() {
                content.path.clear();
            }
            IpcBody::ok(content)
        }
        Ok(Err(e)) => {
            log::warn!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=REJECTED");
            IpcBody::<AgentSkillContent>::err(e, "SKILL_NOT_FOUND")
        }
        Err(e) => {
            log::error!(target: "se_manager::web::skills_api", "operation=skills_api stable_code=FAILED");
            IpcBody::<AgentSkillContent>::err(
                format!("skills read task failed: {e}"),
                "SKILL_READ_ERROR",
            )
        }
    };
    (StatusCode::OK, Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpManager;
    use crate::skills::api_types::{SkillsStatus, ERR_PROJECT_NOT_REGISTERED};
    use crate::skills::service::SkillsHubContext;
    use crate::web::project_registry::ProjectRegistry;
    use crate::web::sink::WsRelaySink;
    use crate::web::test_pty_manager;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        test_state_with_skills(None)
    }

    fn test_state_with_skills(skills_hub: Option<Arc<SkillsHubService>>) -> AppState {
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
            history_mode: crate::web::ws::HistoryMode::LiveOnly,
            conversation: None,
            conversation_creation: None,
            project_root: Arc::new(parking_lot::RwLock::new(
                std::env::temp_dir()
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::temp_dir()),
            )),
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            memory_index: None,
            skills_hub,
            store: None,
        }
    }

    fn test_router(state: AppState) -> axum::Router {
        test_router_with_provenance(state, IngressProvenance::LocalOperator)
    }

    fn test_router_with_provenance(state: AppState, provenance: IngressProvenance) -> axum::Router {
        axum::Router::new()
            .route("/skills", get(list))
            .route("/skills/status", get(status))
            .route("/skills/sync", post(sync))
            .route("/skills/preview", post(preview))
            .route("/skills/install-preview", post(install_preview))
            .route("/skills/operations/{job_id}", get(operation_status))
            .route("/skills/operations/cancel", post(cancel_operation))
            .route("/skills/install", post(install))
            .route("/skills/project", post(project))
            .route("/skills/repair", post(repair))
            .route("/skills/{name}", get(read))
            .with_state(state)
            .layer(axum::Extension(provenance))
    }

    async fn get_request(state: AppState, uri: &str) -> axum::http::Response<Body> {
        test_router(state)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("router response")
    }

    async fn post_json_request(
        state: AppState,
        provenance: IngressProvenance,
        uri: &str,
        body: &str,
    ) -> axum::http::Response<Body> {
        test_router_with_provenance(state, provenance)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("router response")
    }

    async fn body_as_json<T: serde::de::DeserializeOwned>(body: Body) -> T {
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("deserialize IpcBody")
    }

    #[tokio::test]
    async fn public_preview_is_rejected_before_job_creation() {
        let resp = post_json_request(
            test_state(),
            IngressProvenance::PublicTunnel,
            "/skills/preview",
            r#"{"source":{"type":"url","url":"https://example.com/skill.md"},"scope":{"type":"global"},"mode":"installOnly","providerIds":[]}"#,
        )
        .await;
        let body: IpcBody<SkillsOperationStart> = body_as_json(resp.into_body()).await;
        assert!(!body.success);
        assert_eq!(
            body.code.as_deref(),
            Some(crate::web::operation_policy::FORBIDDEN)
        );
    }

    #[tokio::test]
    async fn list_skills_returns_array() {
        let resp = get_request(test_state(), "/skills").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<Vec<AgentSkillSummary>> = body_as_json(resp.into_body()).await;
        assert!(body.success, "skills list should succeed: {:?}", body.error);
        // May be empty on a CI host without ~/.agents/skills, but the body
        // must be a success with an array.
        let _ = body.data.expect("data array");
    }

    #[tokio::test]
    async fn list_skills_rejects_nonexistent_project_root() {
        let resp = get_request(
            test_state(),
            "/skills?projectRoot=/nonexistent/absolute/path",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<Vec<AgentSkillSummary>> = body_as_json(resp.into_body()).await;
        assert!(!body.success);
        assert_eq!(
            body.code.as_deref(),
            Some(crate::skills::api_types::ERR_PROJECT_OUTSIDE_BOUNDARY)
        );
    }

    #[tokio::test]
    async fn list_skills_rejects_project_root_outside_project_root() {
        // A projectRoot that exists but is outside the server's project_root
        // (temp_dir's parent) must be rejected with OUTSIDE_PROJECT_ROOT.
        let state = test_state();
        // CAP-1: project_root is now `Arc<RwLock<PathBuf>>` — lock-read to
        // derive the "outside" path (temp_dir's parent) for the test.
        let root = state.project_root.read().clone();
        let outside = root
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let uri = format!("/skills?projectRoot={}", outside.display());
        let resp = get_request(state, &uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<Vec<AgentSkillSummary>> = body_as_json(resp.into_body()).await;
        assert!(
            !body.success,
            "outside-project-root projectRoot must be rejected"
        );
        assert_eq!(body.code.as_deref(), Some("OUTSIDE_PROJECT_ROOT"));
    }

    #[tokio::test]
    async fn read_skill_not_found_returns_error() {
        let resp = get_request(test_state(), "/skills/nonexistent-skill-12345").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<AgentSkillContent> = body_as_json(resp.into_body()).await;
        // A nonexistent skill must return a failure body (never throw).
        assert!(
            !body.success,
            "nonexistent skill should not be found: {:?}",
            body.data
        );
        assert_eq!(body.code.as_deref(), Some("SKILL_NOT_FOUND"));
    }

    #[tokio::test]
    async fn status_without_service_returns_canonical_root_error() {
        let resp = get_request(test_state(), "/skills/status").await;
        let body: IpcBody<SkillsStatus> = body_as_json(resp.into_body()).await;
        assert!(!body.success);
        assert_eq!(body.code.as_deref(), Some(ERR_CANONICAL_ROOT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn status_rejects_unregistered_project_id() {
        let temp = tempfile::tempdir().unwrap();
        let service = Arc::new(
            SkillsHubService::new(SkillsHubContext {
                state_root: temp.path().join("state"),
                home: temp.path().join("home"),
                config_root: temp.path().join("config"),
                projects: Vec::new(),
                user_config: None,
                project_configs: Vec::new(),
            })
            .unwrap(),
        );
        let resp = get_request(
            test_state_with_skills(Some(service)),
            "/skills/status?projectId=missing",
        )
        .await;
        let body: IpcBody<SkillsStatus> = body_as_json(resp.into_body()).await;
        assert!(!body.success);
        assert_eq!(body.code.as_deref(), Some(ERR_PROJECT_NOT_REGISTERED));
    }
}
