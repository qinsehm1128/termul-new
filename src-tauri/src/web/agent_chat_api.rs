//! Authenticated chat facade. Reads require view; writes require terminal.input,
//! including a second scope check immediately before a delayed queued write.
use super::{
    auth::{auth_error_response, RemoteAccessAuthority, RemoteCapability, RemotePrincipal},
    fs_api::IpcBody,
    ws::AppState,
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct Tail {
    pub after: Option<u64>,
}
#[derive(Deserialize)]
pub struct SendBody {
    pub text: String,
}
#[derive(Deserialize)]
pub struct ControlBody {
    pub action: String,
}

pub(super) fn update_allowed(
    authority: &RemoteAccessAuthority,
    principal: Option<&RemotePrincipal>,
) -> bool {
    principal.is_some_and(|principal| authority.require_scope(principal, "view").is_ok())
}

fn allowed(
    authority: &RemoteAccessAuthority,
    principal: &RemotePrincipal,
    scope: &'static str,
) -> Result<(), Response> {
    authority
        .authorize(principal, RemoteCapability::Scope(scope))
        .map_err(auth_error_response)
}
pub async fn snapshot(
    State(_state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(id): Path<String>,
    Query(query): Query<Tail>,
) -> Response {
    if let Err(response) = allowed(&authority, &principal, "view") {
        return response;
    }
    let value =
        tokio::task::spawn_blocking(move || crate::agent_chat::snapshot(&id, query.after)).await;
    match value {
        Ok(Ok(value)) => Json(IpcBody::ok(value)).into_response(),
        _ => Json(IpcBody::<()>::err(
            "Chat snapshot unavailable",
            "AGENT_CHAT_ERROR",
        ))
        .into_response(),
    }
}
pub async fn send(
    State(_state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Response {
    if let Err(response) = allowed(&authority, &principal, "terminal.input") {
        return response;
    }
    let guard: crate::agent_chat::SendGuard = Arc::new(move || {
        authority
            .authorize(&principal, RemoteCapability::Scope("terminal.input"))
            .is_ok()
    });
    let value = tokio::task::spawn_blocking(move || {
        crate::agent_chat::send_authorized(&id, body.text, Some(guard))
    })
    .await;
    match value {
        Ok(Ok(value)) => Json(IpcBody::ok(value)).into_response(),
        _ => Json(IpcBody::<()>::err(
            "Chat send not confirmed; do not retry automatically",
            "AGENT_CHAT_ERROR",
        ))
        .into_response(),
    }
}
pub async fn control(
    State(_state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(id): Path<String>,
    Json(body): Json<ControlBody>,
) -> Response {
    if let Err(response) = allowed(&authority, &principal, "terminal.input") {
        return response;
    }
    let guard: crate::agent_chat::SendGuard = Arc::new(move || {
        authority
            .authorize(&principal, RemoteCapability::Scope("terminal.input"))
            .is_ok()
    });
    let value = tokio::task::spawn_blocking(move || {
        crate::agent_chat::control_authorized(&id, &body.action, Some(guard))
    })
    .await;
    match value {
        Ok(Ok(())) => Json(IpcBody::ok(())).into_response(),
        _ => Json(IpcBody::<()>::err(
            "Chat control unavailable",
            "AGENT_CHAT_ERROR",
        ))
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::devices::PairRequest;
    use axum::{body::to_bytes, http::StatusCode};
    use serde_json::Value;

    #[tokio::test]
    async fn snapshot_and_mutation_scope_denials_are_explicit() {
        let authority = RemoteAccessAuthority::desktop_memory();
        authority.adopt_or_issue_desktop_credential(None).unwrap();
        let paired = authority
            .pair_device(PairRequest {
                code: authority.issue_pairing_code().unwrap().code,
                name: "chat fixture".into(),
                platform: "ios".into(),
            })
            .unwrap();
        authority
            .set_device_scopes(&paired.device_id, vec!["view".into()])
            .unwrap();
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        assert!(allowed(&authority, &principal, "view").is_ok());
        let denied = allowed(&authority, &principal, "terminal.input").unwrap_err();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(denied.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            serde_json::json!({"code":"FORBIDDEN","capability":"terminal.input"})
        );
        authority
            .set_device_scopes(&paired.device_id, vec!["terminal.input".into()])
            .unwrap();
        assert!(allowed(&authority, &principal, "terminal.input").is_ok());
        assert_eq!(
            allowed(&authority, &principal, "view")
                .unwrap_err()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn live_metadata_requires_current_view_and_valid_device() {
        let authority = RemoteAccessAuthority::desktop_memory();
        authority.adopt_or_issue_desktop_credential(None).unwrap();
        let paired = authority
            .pair_device(PairRequest {
                code: authority.issue_pairing_code().unwrap().code,
                name: "chat fixture".into(),
                platform: "ios".into(),
            })
            .unwrap();
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        assert!(!update_allowed(&authority, None));
        assert!(update_allowed(&authority, Some(&principal)));
        authority
            .set_device_scopes(&paired.device_id, vec!["terminal.input".into()])
            .unwrap();
        assert!(!update_allowed(&authority, Some(&principal)));
        authority
            .set_device_scopes(&paired.device_id, vec!["view".into()])
            .unwrap();
        assert!(update_allowed(&authority, Some(&principal)));
        authority.revoke_device(&paired.device_id).unwrap();
        assert!(!update_allowed(&authority, Some(&principal)));
    }

    #[test]
    fn delayed_write_guard_observes_scope_removal_and_revocation() {
        let authority = Arc::new(RemoteAccessAuthority::desktop_memory());
        authority.adopt_or_issue_desktop_credential(None).unwrap();
        let paired = authority
            .pair_device(PairRequest {
                code: authority.issue_pairing_code().unwrap().code,
                name: "chat fixture".into(),
                platform: "ios".into(),
            })
            .unwrap();
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        let guard = || {
            authority
                .authorize(&principal, RemoteCapability::Scope("terminal.input"))
                .is_ok()
        };
        assert!(guard());
        authority
            .set_device_scopes(&paired.device_id, vec!["view".into()])
            .unwrap();
        assert!(!guard());
        authority
            .set_device_scopes(&paired.device_id, vec!["terminal.input".into()])
            .unwrap();
        assert!(guard());
        authority.revoke_device(&paired.device_id).unwrap();
        assert!(!guard());
    }
}
