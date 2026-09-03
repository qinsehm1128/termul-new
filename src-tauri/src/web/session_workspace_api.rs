//! HTTP facade for revisioned per-Conversation SessionWorkspace.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::conversation::{
    ConversationApplicationService, ConversationId, SessionWorkspaceLoadOutcome,
    SessionWorkspaceV1, SessionWorkspaceWriteOutcome,
};
use crate::web::auth::{status_for_code, RemoteAccessAuthority, RemoteCapability, RemotePrincipal};
use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteRequest {
    pub based_revision: Option<u64>,
    pub workspace: SessionWorkspaceV1,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum SessionWorkspaceLoadWire {
    #[serde(rename_all = "camelCase")]
    Missing {
        conversation_id: ConversationId,
    },
    Loaded {
        workspace: Box<SessionWorkspaceV1>,
    },
    #[serde(rename_all = "camelCase")]
    RecoveryRequired {
        conversation_id: ConversationId,
        recovery_items: Vec<crate::conversation::migration::RecoveryItemV1>,
    },
}

impl From<SessionWorkspaceLoadOutcome> for SessionWorkspaceLoadWire {
    fn from(outcome: SessionWorkspaceLoadOutcome) -> Self {
        match outcome {
            SessionWorkspaceLoadOutcome::Missing { conversation_id } => {
                Self::Missing { conversation_id }
            }
            SessionWorkspaceLoadOutcome::Loaded { workspace } => Self::Loaded { workspace },
            SessionWorkspaceLoadOutcome::RecoveryRequired {
                conversation_id,
                recovery_items,
            } => Self::RecoveryRequired {
                conversation_id,
                recovery_items,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum SessionWorkspaceWriteWire {
    #[serde(rename_all = "camelCase")]
    Updated {
        revision: u64,
        updated_at_utc: String,
    },
    #[serde(rename_all = "camelCase")]
    Conflict {
        current_revision: u64,
        current_updated_at_utc: String,
        current_update_identity: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RecoveryRequired {
        recovery_items: Vec<crate::conversation::migration::RecoveryItemV1>,
    },
}

impl From<SessionWorkspaceWriteOutcome> for SessionWorkspaceWriteWire {
    fn from(outcome: SessionWorkspaceWriteOutcome) -> Self {
        match outcome {
            SessionWorkspaceWriteOutcome::Updated {
                revision,
                updated_at_utc,
            } => Self::Updated {
                revision,
                updated_at_utc,
            },
            SessionWorkspaceWriteOutcome::Conflict {
                current_revision,
                current_updated_at_utc,
                current_update_identity,
            } => Self::Conflict {
                current_revision,
                current_updated_at_utc,
                current_update_identity,
            },
            SessionWorkspaceWriteOutcome::RecoveryRequired { recovery_items } => {
                Self::RecoveryRequired { recovery_items }
            }
        }
    }
}

fn service(
    state: &AppState,
) -> Result<std::sync::Arc<ConversationApplicationService>, (&'static str, String)> {
    state.conversation.clone().ok_or_else(|| {
        (
            "SESSION_WORKSPACE_UNAVAILABLE",
            "bootstrap-published Conversation application service is unavailable".to_string(),
        )
    })
}

pub async fn get(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) =
        require::<SessionWorkspaceLoadWire>(&authority, &principal, RemoteCapability::Read)
    {
        return response;
    }
    let conversation_id = match ConversationId::parse_path_component(&conversation_id) {
        Ok(value) => value,
        Err(error) => {
            return (
                status_for_code("CONVERSATION_INVALID_ID"),
                Json(IpcBody::<SessionWorkspaceLoadWire>::err(
                    error.to_string(),
                    "CONVERSATION_INVALID_ID",
                )),
            )
        }
    };
    let service = match service(&state) {
        Ok(service) => service,
        Err((code, detail)) => {
            return (
                status_for_code(code),
                Json(IpcBody::<SessionWorkspaceLoadWire>::err(detail, code)),
            )
        }
    };
    match service.get_workspace(conversation_id).await {
        Ok(outcome) => {
            let status = if matches!(
                outcome,
                SessionWorkspaceLoadOutcome::RecoveryRequired { .. }
            ) {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(IpcBody::ok(SessionWorkspaceLoadWire::from(outcome))),
            )
        }
        Err(error) => {
            warn!(
                target: "se_manager::web::session_workspace_api",
                conversation_id = %conversation_id,
                code = %error.code,
                "workspace get failed"
            );
            (
                status_for_code(&error.code),
                Json(IpcBody::<SessionWorkspaceLoadWire>::err(
                    error.detail,
                    error.code,
                )),
            )
        }
    }
}

pub async fn write(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(response) =
        require::<SessionWorkspaceWriteWire>(&authority, &principal, RemoteCapability::Mutate)
    {
        return response;
    }
    let conversation_id = match ConversationId::parse_path_component(&conversation_id) {
        Ok(value) => value,
        Err(error) => {
            return (
                status_for_code("CONVERSATION_INVALID_ID"),
                Json(IpcBody::<SessionWorkspaceWriteWire>::err(
                    error.to_string(),
                    "CONVERSATION_INVALID_ID",
                )),
            )
        }
    };
    let request: WriteRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                status_for_code("VALIDATION_ERROR"),
                Json(IpcBody::<SessionWorkspaceWriteWire>::err(
                    format!("payload validation failed: {error}"),
                    "VALIDATION_ERROR",
                )),
            )
        }
    };
    let service = match service(&state) {
        Ok(service) => service,
        Err((code, detail)) => {
            return (
                status_for_code(code),
                Json(IpcBody::<SessionWorkspaceWriteWire>::err(detail, code)),
            )
        }
    };
    match service
        .write_workspace(conversation_id, request.based_revision, request.workspace)
        .await
    {
        Ok(outcome) => {
            let status = if matches!(outcome, SessionWorkspaceWriteOutcome::Conflict { .. }) {
                StatusCode::CONFLICT
            } else if matches!(
                outcome,
                SessionWorkspaceWriteOutcome::RecoveryRequired { .. }
            ) {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(IpcBody::ok(SessionWorkspaceWriteWire::from(outcome))),
            )
        }
        Err(error) => {
            warn!(
                target: "se_manager::web::session_workspace_api",
                conversation_id = %conversation_id,
                code = %error.code,
                "workspace write failed"
            );
            (
                status_for_code(&error.code),
                Json(IpcBody::<SessionWorkspaceWriteWire>::err(
                    error.detail,
                    error.code,
                )),
            )
        }
    }
}

fn require<T>(
    authority: &RemoteAccessAuthority,
    principal: &RemotePrincipal,
    capability: RemoteCapability,
) -> Result<(), (StatusCode, Json<IpcBody<T>>)> {
    authority.authorize(principal, capability).map_err(|error| {
        (
            error.status(),
            Json(IpcBody::err(error.to_string(), error.code())),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::WorkspaceManifestService;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
        ConversationRecordV2, CreationPartition, ExecutionTarget, CONVERSATION_SCHEMA_VERSION,
    };
    use crate::conversation::migration::{
        MigrationHostMode, MigrationMapV1, MigrationPhase, ReaderPrecedence,
        MIGRATION_MAP_SCHEMA_VERSION,
    };
    use crate::conversation::{
        ConversationMutation, ConversationReader, ConversationRepository, ConversationWriter,
        LegacyConversationReader, SessionWorkspaceProjectionState, SessionWorkspaceService,
    };
    use crate::web::ws::HistoryMode;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use axum::routing::get;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    async fn state() -> (tempfile::TempDir, Arc<ConversationRepository>, AppState) {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().canonicalize().unwrap().join("state");
        let (repository, _) =
            ConversationRepository::open(state_root.join("conversations/v2")).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let created_at_utc = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: ConversationId::parse(ID).unwrap(),
                    created_at_utc,
                    creation_partition: CreationPartition::from_created_at(created_at_utc),
                    workspace_cwd: "/visible/session".to_string(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let legacy =
            WorkspaceManifestService::open_read_only(state_root.join("workspace-manifests"))
                .await
                .unwrap();
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let migration_map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            entries: Vec::new(),
        };
        let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
        let conversation = Arc::new(ConversationApplicationService::new(
            reader,
            writer,
            workspace,
            &migration_map,
            MigrationHostMode::Standalone,
            MigrationPhase::Finalized,
            ReaderPrecedence::ConversationV2Only,
        ));
        let pty = crate::web::test_pty_manager();
        (
            temp,
            repository,
            AppState {
                acp: Arc::new(crate::acp::AcpManager::new(vec![])),
                terminal_events: pty.terminal_events(),
                cwd_tracker: pty.cwd_tracker(),
                git_tracker: pty.git_tracker(),
                exit_code_tracker: pty.exit_code_tracker(),
                pty,
                relay: Arc::new(crate::web::sink::WsRelaySink::new()),
                registry: Arc::new(crate::web::project_registry::ProjectRegistry::new()),
                registry_persistence: None,
                projects_file: None,
                history_mode: HistoryMode::LiveOnly,
                conversation: Some(conversation),
                project_root: Arc::new(parking_lot::RwLock::new(std::env::temp_dir())),
                workspace_manifest: Some(legacy),
                acp_catalog: None,
                acp_install: None,
                store: None,
            },
        )
    }

    fn router(state: AppState) -> axum::Router {
        let authority = Arc::new(RemoteAccessAuthority::for_tests("workspace-api-token"));
        let principal = authority.verify_bearer("workspace-api-token").unwrap();
        axum::Router::new()
            .route(
                "/conversations/{conversationId}/workspace",
                get(super::get).post(write),
            )
            .with_state(state)
            .layer(Extension(principal))
            .layer(Extension(authority))
    }

    fn workspace() -> SessionWorkspaceV1 {
        SessionWorkspaceV1 {
            schema_version: 1,
            conversation_id: ConversationId::parse(ID).unwrap(),
            revision: 0,
            updated_at_utc: String::new(),
            update_identity: Some("web-test".to_string()),
            topology: None,
            active_pane_id: None,
            resources: Vec::new(),
            projection_state: SessionWorkspaceProjectionState::Native,
        }
    }

    async fn body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn loopback() -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000)))
    }

    #[tokio::test]
    async fn workspace_get_write_conflict_and_forbidden_contract() {
        let (_temp, _repository, state) = state().await;
        let app = router(state);
        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/conversations/{ID}/workspace"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let missing: IpcBody<SessionWorkspaceLoadWire> = body(get_response).await;
        assert!(
            matches!(missing.data, Some(SessionWorkspaceLoadWire::Missing { .. })),
            "unexpected get body: {missing:?}"
        );

        let request = serde_json::to_vec(&serde_json::json!({
            "basedRevision":null,
            "workspace":workspace()
        }))
        .unwrap();
        let write_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/workspace"))
                    .header("content-type", "application/json")
                    .extension(loopback())
                    .body(Body::from(request.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let updated: IpcBody<SessionWorkspaceWriteWire> = body(write_response).await;
        assert!(matches!(
            updated.data,
            Some(SessionWorkspaceWriteWire::Updated { revision: 1, .. })
        ));

        let conflict_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/workspace"))
                    .header("content-type", "application/json")
                    .extension(loopback())
                    .body(Body::from(request.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict_response.status(), StatusCode::CONFLICT);
        let conflict: IpcBody<SessionWorkspaceWriteWire> = body(conflict_response).await;
        assert!(matches!(
            conflict.data,
            Some(SessionWorkspaceWriteWire::Conflict {
                current_revision: 1,
                ..
            })
        ));

        let forbidden_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/workspace"))
                    .header("content-type", "application/json")
                    .extension(ConnectInfo(SocketAddr::from(([192, 168, 1, 5], 3000))))
                    .body(Body::from(request))
                    .unwrap(),
            )
            .await
            .unwrap();
        let proxied: IpcBody<SessionWorkspaceWriteWire> = body(forbidden_response).await;
        assert!(
            proxied.success,
            "authenticated proxy requests must not rely on peer IP"
        );
    }
}
