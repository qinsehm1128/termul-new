//! `/quick-terminals` routes: the same service and payloads as the desktop
//! `quick_terminal_*` commands.
//!
//! A remote caller never names a directory. A project target is re-resolved
//! from the host's project registry, and worktree targets stay desktop-only,
//! matching the narrowed remote terminal spawn intent.

use axum::{extract::State, http::StatusCode, Json};
use serde_json::Value;

use crate::commands::IpcResult;
use crate::quick_terminal::{
    CloseQuickTerminalTerminal, CreateQuickTerminal, CreateQuickTerminalTerminal,
    OpenQuickTerminal, QuickTerminalIdParams, QuickTerminalOpened, QuickTerminalRecord,
    RenameQuickTerminal, METHOD_CLOSE, METHOD_CLOSE_TERMINAL, METHOD_CREATE,
    METHOD_CREATE_TERMINAL, METHOD_DELETE, METHOD_LIST, METHOD_OPEN, METHOD_RENAME,
};
use crate::quick_terminal_commands::request;
use crate::web::ws::AppState;
use se_quick_terminal::QuickTerminalTarget;

type Reply<T> = (StatusCode, Json<IpcResult<T>>);

const REMOTE_TARGET_REJECTED: &str = "QUICK_TERMINAL_REMOTE_TARGET_REJECTED";

fn reply<T>(result: IpcResult<T>) -> Reply<T> {
    (StatusCode::OK, Json(result))
}

pub async fn list(State(state): State<AppState>) -> Reply<Vec<QuickTerminalRecord>> {
    reply(request(&state.terminal, METHOD_LIST, &Value::Null).await)
}

pub async fn create(
    State(state): State<AppState>,
    Json(mut payload): Json<CreateQuickTerminal>,
) -> Reply<QuickTerminalRecord> {
    match &payload.target {
        QuickTerminalTarget::Workspace => {}
        QuickTerminalTarget::ProjectRoot { project_id, .. } => {
            let Some(project_root) = state.registry.find_path(project_id) else {
                return reply(IpcResult::error(
                    "project is not registered on this host",
                    REMOTE_TARGET_REJECTED,
                ));
            };
            payload.target = QuickTerminalTarget::ProjectRoot {
                project_id: project_id.clone(),
                project_root,
            };
        }
        QuickTerminalTarget::Worktree { .. } => {
            return reply(IpcResult::error(
                "worktree quick terminals can only be created on the desktop",
                REMOTE_TARGET_REJECTED,
            ));
        }
    }
    reply(request(&state.terminal, METHOD_CREATE, &payload).await)
}

pub async fn open(
    State(state): State<AppState>,
    Json(payload): Json<OpenQuickTerminal>,
) -> Reply<QuickTerminalOpened> {
    reply(request(&state.terminal, METHOD_OPEN, &payload).await)
}

pub async fn rename(
    State(state): State<AppState>,
    Json(payload): Json<RenameQuickTerminal>,
) -> Reply<QuickTerminalRecord> {
    reply(request(&state.terminal, METHOD_RENAME, &payload).await)
}

pub async fn delete(
    State(state): State<AppState>,
    Json(payload): Json<QuickTerminalIdParams>,
) -> Reply<()> {
    reply(request(&state.terminal, METHOD_DELETE, &payload).await)
}

pub async fn close(
    State(state): State<AppState>,
    Json(payload): Json<QuickTerminalIdParams>,
) -> Reply<QuickTerminalRecord> {
    reply(request(&state.terminal, METHOD_CLOSE, &payload).await)
}

pub async fn create_terminal(
    State(state): State<AppState>,
    Json(payload): Json<CreateQuickTerminalTerminal>,
) -> Reply<QuickTerminalOpened> {
    reply(request(&state.terminal, METHOD_CREATE_TERMINAL, &payload).await)
}

pub async fn close_terminal(
    State(state): State<AppState>,
    Json(payload): Json<CloseQuickTerminalTerminal>,
) -> Reply<QuickTerminalRecord> {
    reply(request(&state.terminal, METHOD_CLOSE_TERMINAL, &payload).await)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::quick_terminal::QuickTerminalId;
    use crate::web::project_registry::{ProjectRegistry, ProjectSummary};
    use crate::web::ws::HistoryMode;
    use std::sync::Arc;

    struct Roots {
        _profile: tempfile::TempDir,
        _workspace: tempfile::TempDir,
        project: tempfile::TempDir,
    }

    fn state() -> (Roots, AppState) {
        let roots = Roots {
            _profile: tempfile::tempdir().unwrap(),
            _workspace: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        };
        let relay = Arc::new(crate::web::sink::WsRelaySink::new());
        let pty = crate::web::test_pty_manager();
        let registry = Arc::new(ProjectRegistry::new());
        registry.set(
            vec![ProjectSummary {
                id: "p1".to_string(),
                name: "p1".to_string(),
                color: "blue".to_string(),
                path: Some(roots.project.path().to_string_lossy().into_owned()),
                is_archived: false,
                is_default: true,
                live_terminal_count: 0,
                terminal_screen_engine: Default::default(),
            }],
            Some("p1".to_string()),
        );
        // Durable directory creation refuses symlinked components (macOS `/var`).
        let terminal = crate::quick_terminal::with_local_service(
            crate::core::TerminalServiceHandle::in_process(Arc::clone(&pty)),
            &roots._profile.path().canonicalize().unwrap(),
            &roots._workspace.path().canonicalize().unwrap(),
        );
        let state = AppState {
            acp: crate::core::AcpWebHostHandle::in_process(
                Arc::new(crate::acp::AcpManager::new(vec![])),
                Arc::clone(&relay),
            ),
            terminal,
            terminal_events: pty.terminal_events(),
            cwd_tracker: pty.cwd_tracker(),
            git_tracker: pty.git_tracker(),
            exit_code_tracker: pty.exit_code_tracker(),
            pty,
            relay,
            registry,
            registry_persistence: None,
            projects_file: None,
            history_mode: HistoryMode::LiveOnly,
            conversation: None,
            conversation_creation: None,
            project_root: Arc::new(parking_lot::RwLock::new(std::env::temp_dir())),
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            memory_index: None,
            skills_hub: None,
            store: None,
        };
        (roots, state)
    }

    #[test]
    fn terminal_tab_bodies_do_not_accept_host_spawn_fields() {
        let id = QuickTerminalId::new_v4();
        assert!(
            serde_json::from_value::<CreateQuickTerminalTerminal>(serde_json::json!({
                "id": id, "cols": 80, "rows": 24
            }))
            .is_ok()
        );
        for field in ["terminalId", "cwd", "shell", "env"] {
            let mut payload = serde_json::json!({"id": id, "cols": 80, "rows": 24});
            payload[field] = serde_json::json!("untrusted");
            assert!(
                serde_json::from_value::<CreateQuickTerminalTerminal>(payload).is_err(),
                "{field}"
            );
        }
        assert!(
            serde_json::from_value::<CloseQuickTerminalTerminal>(serde_json::json!({
                "id": id, "terminalId": "tab"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<CloseQuickTerminalTerminal>(serde_json::json!({
                "id": id, "terminalId": "tab", "cwd": "/tmp"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn terminal_tab_routes_preserve_host_errors() {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::post;
        use tower::ServiceExt;

        let (_roots, state) = state();
        let app = axum::Router::new()
            .route("/quick-terminals/open", post(open))
            .route("/quick-terminals/create-terminal", post(create_terminal))
            .route("/quick-terminals/close-terminal", post(close_terminal))
            .with_state(state);
        let id = QuickTerminalId::new_v4();
        for (path, payload) in [
            (
                "/quick-terminals/open".to_string(),
                serde_json::json!({"id": id, "cols": 80, "rows": 24, "terminalId": "missing"}),
            ),
            (
                "/quick-terminals/create-terminal".to_string(),
                serde_json::json!({"id": id, "cols": 80, "rows": 24}),
            ),
            (
                "/quick-terminals/close-terminal".to_string(),
                serde_json::json!({"id": id, "terminalId": "missing"}),
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&path)
                        .header("content-type", "application/json")
                        .body(Body::from(payload.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], "QUICK_TERMINAL_NOT_FOUND", "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn a_remote_project_target_uses_the_host_registry_path() {
        let (roots, state) = state();
        let (_, Json(created)) = create(
            State(state),
            Json(CreateQuickTerminal {
                target: QuickTerminalTarget::ProjectRoot {
                    project_id: "p1".to_string(),
                    project_root: "/etc".to_string(),
                },
                title: None,
            }),
        )
        .await;

        let record = created.data.expect("created");
        assert_eq!(
            std::path::Path::new(&record.cwd),
            roots.project.path().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn remote_callers_cannot_pick_worktrees_or_unknown_projects() {
        let (_roots, state) = state();
        for target in [
            QuickTerminalTarget::Worktree {
                project_id: "p1".to_string(),
                worktree_path: "/tmp".to_string(),
                worktree_branch: "main".to_string(),
            },
            QuickTerminalTarget::ProjectRoot {
                project_id: "unknown".to_string(),
                project_root: "/tmp".to_string(),
            },
        ] {
            let (_, Json(result)) = create(
                State(state.clone()),
                Json(CreateQuickTerminal {
                    target,
                    title: None,
                }),
            )
            .await;
            assert_eq!(result.code.as_deref(), Some(REMOTE_TARGET_REJECTED));
        }
        let (_, Json(listed)) = list(State(state)).await;
        assert_eq!(listed.data.map(|records| records.len()), Some(0));
    }
}
