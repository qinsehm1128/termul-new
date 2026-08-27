//! HTTP handler for frontend error forwarding (CAP-2: Web & Mobile 1:1 Parity).
//!
//! Mirrors the desktop `#[tauri::command] log_frontend_error` handler over
//! HTTP. The web client has no Tauri runtime to invoke the command; this route
//! lets the renderer forward errors to the server's log file so they survive a
//! closed DevTools console (same motivation as the desktop path, issue #244).
//!
//! - **Loopback-only:** like fs write routes, refused from non-loopback peers
//!   (`check_local_only`) so a LAN client cannot spam the server's log.
//! - **Sanitization:** reuses `crate::sanitize_log_field` (log-injection
//!   defense: newlines/CR/tab escaped, control chars stripped, truncated to
//!   `MAX_FRONTEND_FIELD_LEN`) so a crafted error message cannot forge
//!   authoritative-looking log lines.
//! - **Best-effort:** logging failure is swallowed — a failure to log must not
//!   cascade into another error (matching `log-api.ts` swallow semantics).
//! - **tracing:** the standalone server uses `tracing::error!`/`warn!` (NOT
//!   the `log` facade the desktop command uses) — the `web` module is the
//!   standalone boundary.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;

use crate::web::auth::IngressProvenance;
use crate::web::fs_api::{check_local_only, IpcBody};
use crate::web::ws::AppState;

/// `POST /log/frontend-error` body. Mirrors the desktop
/// `log_frontend_error` command parameters (camelCase for JSON parity).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendErrorRequest {
    pub level: Option<String>,
    pub message: String,
    pub source: Option<String>,
    pub stack: Option<String>,
    pub component_stack: Option<String>,
}

/// `POST /log/frontend-error` — forward a renderer error to the server log.
/// Loopback-only (refused from non-loopback peers). Returns `IpcBody::ok(())`
/// on success; logging failures are swallowed (best-effort, no loop).
pub async fn frontend_error(
    State(_state): State<AppState>,
    axum::Extension(provenance): axum::Extension<IngressProvenance>,
    Json(req): Json<FrontendErrorRequest>,
) -> impl IntoResponse {
    if let Some(forbidden) = check_local_only::<()>(provenance) {
        return (StatusCode::OK, Json(forbidden));
    }

    let level = if req.level.as_deref() == Some("warn") {
        "warn"
    } else {
        "error"
    };
    let supplied_fields = 1_u8
        + u8::from(req.source.is_some())
        + u8::from(req.stack.is_some())
        + u8::from(req.component_stack.is_some());
    let message_bytes = req.message.len();
    log::warn!(
        target: "termul::web::log_api",
        "operation=frontend_error level={} supplied_fields={} message_bytes={} stable_code=FRONTEND_ERROR_REPORTED",
        level,
        supplied_fields,
        message_bytes
    );

    (StatusCode::OK, Json(IpcBody::<()>::ok(())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpManager;
    use crate::web::project_registry::ProjectRegistry;
    use crate::web::sink::WsRelaySink;
    use crate::web::test_pty_manager;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use axum::routing::post;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> AppState {
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
            project_root: Arc::new(parking_lot::RwLock::new(
                std::env::temp_dir()
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::temp_dir()),
            )),
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            store: None,
        }
    }

    fn test_router(state: AppState) -> axum::Router {
        axum::Router::new()
            .route("/log/frontend-error", post(frontend_error))
            .with_state(state)
    }

    async fn post_json(
        state: AppState,
        body: &serde_json::Value,
        peer: SocketAddr,
    ) -> axum::http::Response<Body> {
        let bytes = serde_json::to_vec(body).expect("serialize body");
        test_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/log/frontend-error")
                    .header("content-type", "application/json")
                    .extension(ConnectInfo(peer))
                    .extension(if peer.ip().is_loopback() {
                        IngressProvenance::LocalOperator
                    } else {
                        IngressProvenance::PublicTunnel
                    })
                    .body(Body::from(bytes))
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
    async fn frontend_error_succeeds_on_loopback() {
        let req = serde_json::json!({
            "level": "error",
            "message": "test error",
            "source": "test",
        });
        let peer = SocketAddr::from(([127, 0, 0, 1], 54321));
        let resp = post_json(test_state(), &req, peer).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<()> = body_as_json(resp.into_body()).await;
        assert!(body.success, "log should succeed: {:?}", body.error);
    }

    #[tokio::test]
    async fn frontend_error_refused_from_non_loopback() {
        let req = serde_json::json!({
            "message": "test",
        });
        let peer = SocketAddr::from(([192, 168, 1, 50], 40000));
        let resp = post_json(test_state(), &req, peer).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<()> = body_as_json(resp.into_body()).await;
        assert!(!body.success, "non-loopback must be refused");
        assert_eq!(body.code.as_deref(), Some("FORBIDDEN"));
    }

    #[tokio::test]
    async fn frontend_error_warn_level_succeeds() {
        let req = serde_json::json!({
            "level": "warn",
            "message": "warning test",
        });
        let peer = SocketAddr::from(([127, 0, 0, 1], 54321));
        let resp = post_json(test_state(), &req, peer).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: IpcBody<()> = body_as_json(resp.into_body()).await;
        assert!(body.success);
    }
}
