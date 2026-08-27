//! HTTP adapters for canonical Conversation binding and delete lifecycle operations.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use tracing::warn;

use crate::conversation::{
    ConversationApplicationService, ConversationId, ConversationLifecycleOutcome,
    PrepareConversationRequest,
};
use crate::web::auth::{status_for_code, RemoteAccessAuthority, RemoteCapability, RemotePrincipal};
use crate::web::fs_api::IpcBody;
use crate::web::sink::AcpEvent;
use crate::web::ws::AppState;
use crate::web::EventSink;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevisionRequest {
    expected_revision: u64,
    #[serde(default)]
    remove_workspace: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceRequest {
    expected_revision: u64,
    request: PrepareConversationRequest,
}

pub async fn detach(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    mutate_revision(
        state,
        authority,
        principal,
        conversation_id,
        body,
        Mutation::Detach,
    )
    .await
}

pub async fn rebind(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    mutate_revision(
        state,
        authority,
        principal,
        conversation_id,
        body,
        Mutation::Rebind,
    )
    .await
}

pub async fn suspend(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    mutate_revision(
        state,
        authority,
        principal,
        conversation_id,
        body,
        Mutation::Suspend,
    )
    .await
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    mutate_revision(
        state,
        authority,
        principal,
        conversation_id,
        body,
        Mutation::Delete,
    )
    .await
}

pub async fn replace(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Path(conversation_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Some(response) = mutation_denial(&authority, &principal) {
        return response;
    }
    let conversation_id = match parse_id(&conversation_id) {
        Ok(value) => value,
        Err((code, detail)) => return failure(code, detail),
    };
    let request: ReplaceRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => return validation(error.to_string()),
    };
    let service = match application(&state) {
        Ok(service) => service,
        Err((code, detail)) => return failure(code, detail),
    };
    respond(
        &state,
        conversation_id,
        None,
        service
            .replace_binding(conversation_id, request.request, request.expected_revision)
            .await,
    )
    .await
}

#[derive(Clone, Copy)]
enum Mutation {
    Detach,
    Rebind,
    Suspend,
    Delete,
}

async fn mutate_revision(
    state: AppState,
    authority: Arc<RemoteAccessAuthority>,
    principal: RemotePrincipal,
    conversation_id: String,
    body: Bytes,
    mutation: Mutation,
) -> (StatusCode, Json<IpcBody<ConversationLifecycleOutcome>>) {
    if let Some(response) = mutation_denial(&authority, &principal) {
        return response;
    }
    let conversation_id = match parse_id(&conversation_id) {
        Ok(value) => value,
        Err((code, detail)) => return failure(code, detail),
    };
    let request: RevisionRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => return validation(error.to_string()),
    };
    let service = match application(&state) {
        Ok(service) => service,
        Err((code, detail)) => return failure(code, detail),
    };
    let workspace_cwd = if matches!(mutation, Mutation::Delete) {
        service
            .get_conversation(conversation_id)
            .ok()
            .map(|record| record.workspace_cwd)
    } else {
        None
    };
    let current_session_id = if matches!(mutation, Mutation::Delete) {
        match service
            .writer()
            .repository()
            .current_binding(conversation_id)
        {
            Ok(binding) => binding.map(|binding| binding.agent_session_id),
            Err(_) => {
                return failure(
                    "CONVERSATION_RECOVERY_REQUIRED".to_string(),
                    "failed to resolve Conversation binding before delete".to_string(),
                )
            }
        }
    } else {
        None
    };
    let result = match mutation {
        Mutation::Detach => {
            service
                .detach_binding(conversation_id, request.expected_revision)
                .await
        }
        Mutation::Rebind => {
            service
                .rebind_binding(conversation_id, request.expected_revision)
                .await
        }
        Mutation::Suspend => {
            service
                .suspend_binding(conversation_id, request.expected_revision)
                .await
        }
        Mutation::Delete => {
            service
                .delete_conversation(conversation_id, request.expected_revision)
                .await
        }
    };
    if matches!(mutation, Mutation::Delete) && request.remove_workspace && result.is_ok() {
        if let Some(path) = workspace_cwd.filter(|path| !path.trim().is_empty()) {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                log::warn!(
                    "[conversation-delete] workspace removal failed conversation_id={} path={} error={error}",
                    conversation_id,
                    path
                );
            }
        }
    }
    respond(&state, conversation_id, current_session_id, result).await
}

fn application(
    state: &AppState,
) -> Result<std::sync::Arc<ConversationApplicationService>, (String, String)> {
    state.conversation.clone().ok_or_else(|| {
        (
            "CONVERSATION_SERVICE_UNAVAILABLE".to_string(),
            "bootstrap-published Conversation application service is unavailable".to_string(),
        )
    })
}

async fn respond(
    state: &AppState,
    conversation_id: ConversationId,
    current_session_id: Option<String>,
    result: crate::conversation::application::Result<ConversationLifecycleOutcome>,
) -> (StatusCode, Json<IpcBody<ConversationLifecycleOutcome>>) {
    match result {
        Ok(outcome) => {
            if matches!(
                outcome,
                ConversationLifecycleOutcome::Updated {
                    action: crate::conversation::ConversationLifecycleAction::DeleteConversation,
                    ..
                }
            ) {
                if let Some(session_id) = current_session_id {
                    if let Err(code) = state.relay.retire_session(&session_id).await {
                        warn!(
                            target: "termul::web::conversation_lifecycle_api",
                            conversation_id = %conversation_id,
                            code,
                            "Conversation delete committed but auxiliary retirement failed"
                        );
                        return failure(
                            "CONVERSATION_RETIREMENT_FAILED".to_string(),
                            "Conversation auxiliary retirement failed".to_string(),
                        );
                    }
                }
            }
            if let Err(error) = state.relay.emit(&AcpEvent {
                sid: None,
                type_: "conversation_lifecycle",
                payload: serde_json::to_value(&outcome)
                    .expect("Conversation lifecycle outcome serializes"),
            }) {
                warn!(
                    target: "termul::web::conversation_lifecycle_api",
                    conversation_id = %conversation_id,
                    code = error.code,
                    "conversation lifecycle committed but live delivery degraded"
                );
                return failure(
                    error.code.to_string(),
                    "conversation lifecycle event delivery degraded".to_string(),
                );
            }
            (StatusCode::OK, Json(IpcBody::ok(outcome)))
        }
        Err(error) => {
            warn!(
                target: "termul::web::conversation_lifecycle_api",
                conversation_id = %conversation_id,
                code = %error.code,
                operation = error.operation,
                "conversation lifecycle mutation failed"
            );
            failure(error.code, error.detail)
        }
    }
}

fn parse_id(value: &str) -> Result<ConversationId, (String, String)> {
    ConversationId::parse_path_component(value)
        .map_err(|error| ("CONVERSATION_INVALID_ID".to_string(), error.to_string()))
}

fn validation(detail: String) -> (StatusCode, Json<IpcBody<ConversationLifecycleOutcome>>) {
    failure(
        "VALIDATION_ERROR".to_string(),
        format!("payload validation failed: {detail}"),
    )
}

fn mutation_denial(
    authority: &RemoteAccessAuthority,
    principal: &RemotePrincipal,
) -> Option<(StatusCode, Json<IpcBody<ConversationLifecycleOutcome>>)> {
    authority
        .authorize(principal, RemoteCapability::Mutate)
        .err()
        .map(|error| failure(error.code().to_string(), error.to_string()))
}

fn failure(
    code: String,
    detail: String,
) -> (StatusCode, Json<IpcBody<ConversationLifecycleOutcome>>) {
    (
        status_for_code(&code),
        Json(IpcBody::<ConversationLifecycleOutcome>::err(detail, code)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::{AcpManager, AgentId};
    use crate::conversation::contracts::{
        parse_created_at_utc, AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
        ConversationLifecycleState, ConversationRecordV2, CreationPartition, ExecutionTarget,
        AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    };
    use crate::conversation::migration::{
        MigrationHostMode, MigrationMapV1, MigrationPhase, ReaderPrecedence,
        MIGRATION_MAP_SCHEMA_VERSION,
    };
    use crate::conversation::{
        ConversationApplicationService, ConversationCreationService, ConversationLifecycleService,
        ConversationLocator, ConversationMutation, ConversationPersistenceAdapter,
        ConversationReader, ConversationRepository, ConversationWriter, SessionWorkspaceLocator,
        SessionWorkspaceService,
    };
    use crate::web::ws::HistoryMode;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use axum::routing::post;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tower::ServiceExt;
    use uuid::Uuid;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    async fn state() -> (
        tempfile::TempDir,
        AppState,
        u64,
        Arc<ConversationRepository>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let private = base.join("state/conversations/v2");
        let visible = base.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse(ID).unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        let workspace = visible.join("sessions/2026/08/15").join(ID);
        std::fs::create_dir_all(&workspace).unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: workspace.to_string_lossy().into_owned(),
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
        writer
            .bind_agent_session(
                id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "opaque/http".to_string(),
                    runtime_agent_id: "agent-http".to_string(),
                    stable_agent_namespace: "config:http".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: chrono::Utc::now(),
                    state: AgentSessionBindingState::Active,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        let creation = Arc::new(
            ConversationCreationService::new(
                Arc::clone(&writer),
                ConversationLocator::new(private).unwrap(),
                SessionWorkspaceLocator::new(visible).unwrap(),
            )
            .unwrap(),
        );
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            Default::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let persistence = Arc::new(ConversationPersistenceAdapter::new(
            Arc::clone(&writer),
            Arc::clone(&reader),
        ));
        let acp = Arc::new(AcpManager::with_conversation_services(
            vec![],
            creation,
            persistence,
        ));
        let (observed_tx, _observed_rx) = std::sync::mpsc::sync_channel(1);
        acp.install_test_agent_for_new_session_with_close_result(
            AgentId("agent-http".to_string()),
            observed_tx,
            Err("provider close leaked SUPER_SECRET=do-not-return".to_string()),
        );
        let pty = crate::web::test_pty_manager();
        acp.set_pty_manager(&pty);
        let migration_map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            entries: Vec::new(),
        };
        let workspace_service = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
        let conversation = Arc::new(ConversationApplicationService::new(
            reader,
            writer,
            workspace_service,
            &migration_map,
            MigrationHostMode::Standalone,
            MigrationPhase::Finalized,
            ReaderPrecedence::ConversationV2Only,
        ));
        conversation
            .attach_lifecycle(
                ConversationLifecycleService::from_manager(Arc::clone(&acp), Arc::clone(&pty))
                    .unwrap(),
            )
            .unwrap();
        let revision = repository.get_conversation(id).unwrap().last_seq;
        let state = AppState {
            acp,
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
            workspace_manifest: None,
            acp_catalog: None,
            acp_install: None,
            store: None,
        };
        (temp, state, revision, repository)
    }

    fn router(state: AppState) -> axum::Router {
        let authority = Arc::new(RemoteAccessAuthority::for_tests("lifecycle-api-token"));
        let principal = authority.verify_bearer("lifecycle-api-token").unwrap();
        axum::Router::new()
            .route(
                "/conversations/{conversationId}/lifecycle/detach",
                post(detach),
            )
            .route(
                "/conversations/{conversationId}/lifecycle/delete",
                post(delete),
            )
            .route(
                "/conversations/{conversationId}/lifecycle/replace",
                post(replace),
            )
            .with_state(state)
            .layer(Extension(principal))
            .layer(Extension(authority))
    }

    async fn body(response: axum::response::Response) -> IpcBody<ConversationLifecycleOutcome> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn detach_and_stale_last_seq_use_stable_camel_case_contract() {
        let (_temp, state, revision, _repository) = state().await;
        let app = router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/lifecycle/detach"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::from(format!("{{\"expectedRevision\":{revision}}}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let detached = body(response).await;
        let value = serde_json::to_value(detached.data.unwrap()).unwrap();
        assert_eq!(value["action"], "detachBinding");
        assert_eq!(value["currentBinding"]["state"], "detached");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/lifecycle/delete"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::from(format!("{{\"expectedRevision\":{revision}}}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body(response).await.code.as_deref(),
            Some("CONVERSATION_CONFLICT")
        );
    }

    #[tokio::test]
    async fn replacement_double_failure_returns_compensation_receipt_and_recovery_state() {
        let (_temp, state, revision, repository) = state().await;
        repository.fail_next_agent_binding_appends(1);
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/lifecycle/replace"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::from(format!(
                        "{{\"expectedRevision\":{revision},\"request\":{{\"schemaVersion\":1,\"conversationId\":\"{ID}\",\"projectAttachment\":null,\"executionTarget\":{{\"kind\":\"workspace\"}}}}}}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let failed = body(response).await;
        assert_eq!(failed.code.as_deref(), Some("ACP_COMPENSATION_FAILED"));
        let detail = failed.error.unwrap();
        let failure: crate::conversation::AgentCompensationFailure =
            serde_json::from_str(&detail).unwrap();
        assert_eq!(failure.primary_code, "CONVERSATION_DURABILITY_FAILED");
        assert_eq!(
            failure.provider_close_code.as_deref(),
            Some("ACP_CLOSE_FAILED")
        );
        assert!(failure.recovery_id.is_some());
        assert!(!detail.contains("SUPER_SECRET"));
        assert!(!detail.contains("opaque/fake-session"));

        let id = ConversationId::parse(ID).unwrap();
        let record = repository.get_conversation(id).unwrap();
        assert_eq!(
            record.lifecycle_state,
            ConversationLifecycleState::RecoveryRequired
        );
        let current = repository.current_binding(id).unwrap().unwrap();
        assert_eq!(current.agent_session_id, "opaque/http");
        assert_eq!(current.state, AgentSessionBindingState::Active);
    }
}
