//! HTTP handlers for CLI session discovery (desktop shared-live + se-server).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tracing::{error, info};

use crate::cli_session::{
    list_cli_sessions, resolve_cli_sessions, CliSessionListArgs, CliSessionListResult,
    CliSessionResolveArgs, CliSessionResolveResult,
};
use crate::web::fs_api::IpcBody;
use crate::web::live_terminals::{live_terminals, LiveTerminalDescriptor};
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

/// One live agent process found under a live terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgentSessionEntry {
    pub terminal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub root_pid: u32,
    pub agent_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_dev: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none", rename = "hook_kind")]
    pub hook_kind: Option<crate::agent_hooks::HookKind>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "transcript_path")]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgentSessionsResult {
    pub sessions: Vec<LiveAgentSessionEntry>,
    pub scanned_at: String,
}

fn map_live_sessions(
    terminals: &[LiveTerminalDescriptor],
    detected: Vec<crate::cli_session::LiveAgentSession>,
) -> Vec<LiveAgentSessionEntry> {
    let by_pid: HashMap<u32, &LiveTerminalDescriptor> = terminals
        .iter()
        .map(|terminal| (terminal.pid, terminal))
        .collect();
    detected
        .into_iter()
        .filter_map(|session| {
            let terminal = by_pid.get(&session.root_pid)?;
            Some(LiveAgentSessionEntry {
                terminal_id: terminal.id.clone(),
                project_id: terminal.project_id.clone(),
                root_pid: session.root_pid,
                agent_id: session.agent_id.to_string(),
                session_id: session.session_id,
                self_dev: session.self_dev,
                codex_home: session.codex_home,
                cwd: session.cwd,
                source: session.source,
                hook_kind: session.hook_kind,
                transcript_path: session.transcript_path,
            })
        })
        .collect()
}

/// `GET /cli-sessions/live` → agent processes running under live terminals.
///
/// Read-only. The desktop renderer runs the same detection through
/// `detect_live_agent_sessions_cmd`; this route lets a remote client see the
/// same live evidence instead of inferring "running" from session files.
pub async fn live_get(State(state): State<AppState>) -> impl IntoResponse {
    let terminals = live_terminals(&state).await;
    let roots: Vec<u32> = terminals
        .iter()
        .map(|terminal| terminal.pid)
        .filter(|pid| *pid > 0)
        .collect();
    info!(
        target: "se_manager::web::cli_session_api",
        "operation=detect_live_agent_sessions terminals={}",
        terminals.len()
    );
    let detected = if roots.is_empty() {
        Vec::new()
    } else {
        let identities = terminals
            .iter()
            .map(|t| (t.pid, t.id.clone(), Some(t.cwd.clone())))
            .collect::<Vec<_>>();
        match spawn_blocking(move || crate::cli_session::detect_with_hooks(&identities)).await {
            Ok(sessions) => sessions,
            Err(err) => {
                error!(
                    target: "se_manager::web::cli_session_api",
                    "operation=detect_live_agent_sessions_failed error=join"
                );
                return (
                    StatusCode::OK,
                    Json(IpcBody::<LiveAgentSessionsResult>::err(
                        err.to_string(),
                        "LIVE_DETECTION_FAILED",
                    )),
                );
            }
        }
    };
    let sessions = map_live_sessions(&terminals, detected);
    (
        StatusCode::OK,
        Json(IpcBody::ok(LiveAgentSessionsResult {
            sessions,
            scanned_at: chrono::Utc::now().to_rfc3339(),
        })),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_session::LiveAgentSession;

    fn terminal(id: &str, pid: u32, project_id: Option<&str>) -> LiveTerminalDescriptor {
        LiveTerminalDescriptor {
            id: id.to_string(),
            pid,
            project_id: project_id.map(str::to_string),
            cwd: "/work/app".to_string(),
        }
    }

    fn detected(root_pid: u32, agent_id: &'static str, session_id: &str) -> LiveAgentSession {
        LiveAgentSession {
            root_pid,
            agent_id,
            session_id: session_id.to_string(),
            self_dev: Some(false),
            source: "poll",
            hook_kind: None,
            transcript_path: None,
            codex_home: None,
            cwd: Some("/work/app".to_string()),
        }
    }

    #[test]
    fn unknown_hook_qin_variant_is_not_serialized_as_default() {
        let mut session = detected(100, "qin-code", "unverified");
        session.source = "hook";
        session.self_dev = None;
        let mapped = map_live_sessions(&[terminal("terminal", 100, None)], vec![session]);
        let value = serde_json::to_value(&mapped[0]).unwrap();
        assert!(value.get("selfDev").is_none());
        assert_eq!(value["source"], "hook");
    }

    #[test]
    fn live_sessions_keep_only_processes_under_a_live_terminal() {
        let terminals = vec![
            terminal("terminal-1", 100, Some("project-a")),
            terminal("terminal-2", 200, None),
        ];
        let mapped = map_live_sessions(
            &terminals,
            vec![
                detected(100, "claude-code", "session-a"),
                detected(200, "qin-code", "session-b"),
                detected(999, "codex", "orphan"),
            ],
        );
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].terminal_id, "terminal-1");
        assert_eq!(mapped[0].project_id.as_deref(), Some("project-a"));
        assert_eq!(mapped[0].agent_id, "claude-code");
        assert_eq!(mapped[1].terminal_id, "terminal-2");
        assert_eq!(mapped[1].project_id, None);
    }

    #[test]
    fn live_session_payload_uses_camel_case_and_hides_absent_projects() {
        let terminals = vec![terminal("terminal-1", 100, None)];
        let mapped = map_live_sessions(&terminals, vec![detected(100, "pi", "s-1")]);
        let value = serde_json::to_value(&mapped[0]).expect("serialize");
        assert_eq!(value["terminalId"], "terminal-1");
        assert_eq!(value["rootPid"], 100);
        assert_eq!(value["agentId"], "pi");
        assert!(value.get("projectId").is_none());
    }
    #[tokio::test]
    async fn live_route_returns_empty_success_without_terminals() {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt;

        let pty = crate::web::test_pty_manager();
        let state = crate::web::test_state_with_terminal(
            std::sync::Arc::clone(&pty),
            crate::core::TerminalServiceHandle::in_process(std::sync::Arc::clone(&pty)),
        );
        let app = axum::Router::new()
            .route("/cli-sessions/live", get(live_get))
            .with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/cli-sessions/live")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("router response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("parse body");
        assert_eq!(value["success"], true);
        assert_eq!(value["data"]["sessions"], serde_json::json!([]));
    }
}
