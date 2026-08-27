//! Thin HTTP adapters for the shared Conversation application service.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use crate::conversation::migration::{RecoveryActionResult, RecoveryAuthorizationClass};
use crate::conversation::{
    ConversationAggregateMutationOutcome, ConversationApplicationService, ConversationId,
    ExecutionTarget, LegacyConversationKey, LegacyConversationResolution, ProjectAttachment,
};
use crate::web::auth::{status_for_code, RemoteAccessAuthority, RemoteCapability, RemotePrincipal};
use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

fn service(state: &AppState) -> Result<Arc<ConversationApplicationService>, (String, String)> {
    state.conversation.clone().ok_or_else(|| {
        (
            "CONVERSATION_SERVICE_UNAVAILABLE".to_string(),
            "bootstrap-published Conversation application service is unavailable".to_string(),
        )
    })
}

pub async fn host_status(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
) -> impl IntoResponse {
    let result = require(&authority, &principal, RemoteCapability::Read).and_then(|()| {
        service(&state).and_then(|service| {
            service
                .host_status()
                .map(redact_host_status)
                .map_err(|error| (error.code, error.detail))
        })
    });
    respond(result)
}

pub async fn list(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
) -> impl IntoResponse {
    let result = require(&authority, &principal, RemoteCapability::Read)
        .and_then(|()| service(&state).map(|service| service.list_conversations()));
    respond(result)
}

pub async fn get(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
) -> impl IntoResponse {
    let result = require(&authority, &principal, RemoteCapability::Read)
        .and_then(|()| parse_id(&conversation_id))
        .and_then(|conversation_id| {
            service(&state).and_then(|service| {
                service
                    .get_conversation(conversation_id)
                    .map_err(|error| (error.code, error.detail))
            })
        });
    respond(result)
}

pub async fn current_binding(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
) -> impl IntoResponse {
    let result = require(&authority, &principal, RemoteCapability::Read)
        .and_then(|()| parse_id(&conversation_id))
        .and_then(|conversation_id| {
            service(&state).and_then(|service| {
                service
                    .current_binding(conversation_id)
                    .map_err(|error| (error.code, error.detail))
            })
        });
    respond(result)
}

pub async fn open(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
) -> impl IntoResponse {
    let result = match require(&authority, &principal, RemoteCapability::Read)
        .and_then(|()| parse_id(&conversation_id))
    {
        Ok(conversation_id) => match service(&state) {
            Ok(service) => service
                .open_conversation(conversation_id)
                .await
                .map_err(|error| (error.code, error.detail)),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    respond(result)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameRequest {
    title: String,
}

pub async fn rename(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Json(request): Json<RenameRequest>,
) -> impl IntoResponse {
    let result = match require(&authority, &principal, RemoteCapability::Mutate)
        .and_then(|()| parse_id(&conversation_id))
    {
        Ok(conversation_id) => match service(&state) {
            Ok(service) => service
                .rename_conversation(conversation_id, request.title)
                .await
                .map_err(|error| (error.code, error.detail)),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    respond(result)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachProjectRequest {
    expected_revision: u64,
    attachment: ProjectAttachment,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DetachProjectRequest {
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateExecutionTargetRequest {
    expected_revision: u64,
    execution_target: ExecutionTarget,
}

pub async fn attach_project(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(error) = require(&authority, &principal, RemoteCapability::Mutate) {
        return respond::<ConversationAggregateMutationOutcome>(Err(error));
    }
    let conversation_id = match parse_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return respond::<ConversationAggregateMutationOutcome>(Err(error)),
    };
    let request: AttachProjectRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return respond::<ConversationAggregateMutationOutcome>(Err((
                "VALIDATION_ERROR".to_string(),
                format!("payload validation failed: {error}"),
            )))
        }
    };
    let result = match service(&state) {
        Ok(service) => service
            .attach_project(
                conversation_id,
                request.expected_revision,
                request.attachment,
            )
            .await
            .map_err(|error| (error.code, error.detail)),
        Err(error) => Err(error),
    };
    respond(result)
}

pub async fn detach_project(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(error) = require(&authority, &principal, RemoteCapability::Mutate) {
        return respond::<ConversationAggregateMutationOutcome>(Err(error));
    }
    let conversation_id = match parse_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return respond::<ConversationAggregateMutationOutcome>(Err(error)),
    };
    let request: DetachProjectRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return respond::<ConversationAggregateMutationOutcome>(Err((
                "VALIDATION_ERROR".to_string(),
                format!("payload validation failed: {error}"),
            )))
        }
    };
    let result = match service(&state) {
        Ok(service) => service
            .detach_project(conversation_id, request.expected_revision)
            .await
            .map_err(|error| (error.code, error.detail)),
        Err(error) => Err(error),
    };
    respond(result)
}

pub async fn update_execution_target(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(error) = require(&authority, &principal, RemoteCapability::Mutate) {
        return respond::<ConversationAggregateMutationOutcome>(Err(error));
    }
    let conversation_id = match parse_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return respond::<ConversationAggregateMutationOutcome>(Err(error)),
    };
    let request: UpdateExecutionTargetRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return respond::<ConversationAggregateMutationOutcome>(Err((
                "VALIDATION_ERROR".to_string(),
                format!("payload validation failed: {error}"),
            )))
        }
    };
    let result = match service(&state) {
        Ok(service) => service
            .update_execution_target(
                conversation_id,
                request.expected_revision,
                request.execution_target,
            )
            .await
            .map_err(|error| (error.code, error.detail)),
        Err(error) => Err(error),
    };
    respond(result)
}

pub async fn resolve_legacy(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(error) = require(&authority, &principal, RemoteCapability::Read) {
        return respond::<LegacyConversationResolution>(Err(error));
    }
    let request: LegacyConversationKey = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return respond::<LegacyConversationResolution>(Err((
                "VALIDATION_ERROR".to_string(),
                format!("payload validation failed: {error}"),
            )))
        }
    };
    let result = service(&state).and_then(|service| {
        service
            .resolve_legacy_conversation_id(request)
            .map_err(|error| (error.code, error.detail))
    });
    respond(result)
}

pub async fn resolve_recovery(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    body: Bytes,
) -> impl IntoResponse {
    let request: crate::conversation::migration::ResolveRecoveryItemRequest =
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => {
                return respond::<RecoveryActionResult>(Err((
                    "VALIDATION_ERROR".to_string(),
                    format!("payload validation failed: {error}"),
                )))
            }
        };
    let capability = if request.action.authorization() == RecoveryAuthorizationClass::Mutation {
        RemoteCapability::Mutate
    } else {
        RemoteCapability::RecoveryInspect
    };
    if let Err(error) = require(&authority, &principal, capability) {
        return respond::<RecoveryActionResult>(Err(error));
    }
    let result = match service(&state) {
        Ok(service) => service
            .resolve_recovery_item(request)
            .await
            .map_err(|error| (error.code, error.detail)),
        Err(error) => Err(error),
    };
    respond(result)
}

fn require(
    authority: &RemoteAccessAuthority,
    principal: &RemotePrincipal,
    capability: RemoteCapability,
) -> Result<(), (String, String)> {
    authority
        .authorize(principal, capability)
        .map_err(|error| (error.code().to_string(), error.to_string()))
}

fn redact_host_status(
    mut status: crate::conversation::application::ConversationHostStatus,
) -> crate::conversation::application::ConversationHostStatus {
    for item in &mut status.recovery_items {
        item.source_paths.clear();
        item.source_sha256.clear();
        item.candidate_facts.clear();
        item.provenance.clear();
    }
    status
}

fn parse_id(value: &str) -> Result<ConversationId, (String, String)> {
    ConversationId::parse_path_component(value)
        .map_err(|error| ("CONVERSATION_INVALID_ID".to_string(), error.to_string()))
}

fn respond<T: Serialize>(result: Result<T, (String, String)>) -> (StatusCode, Json<IpcBody<T>>) {
    match result {
        Ok(value) => (StatusCode::OK, Json(IpcBody::ok(value))),
        Err((code, detail)) => (status_for_code(&code), Json(IpcBody::err(detail, code))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
        ConversationLifecycleState, ConversationRecordV2, CreationPartition, ExecutionTarget,
        AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    };
    use crate::conversation::migration::{
        CreatedAtSource, IdentityDecision, MigrationHostMode, MigrationMapEntryV1, MigrationMapV1,
        MigrationPhase, ReaderPrecedence, RecoveryItemV1, MIGRATION_MAP_SCHEMA_VERSION,
    };
    use crate::conversation::{
        ConversationMutation, ConversationReader, ConversationRepository, ConversationWriter,
        LegacyConversationReader, SessionWorkspaceService,
    };
    use crate::web::ws::HistoryMode;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use axum::routing::{get, post};
    use std::net::SocketAddr;
    use tower::ServiceExt;
    use uuid::Uuid;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    async fn state_with_repository() -> (tempfile::TempDir, Arc<ConversationRepository>, AppState) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("state/conversations/v2");
        let (repository, _) = ConversationRepository::open(root).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse(ID).unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: "/visible/conversation".to_string(),
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
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            entries: vec![MigrationMapEntryV1 {
                source_key: "legacy_chat_history:0:payloads/history-one.json".to_string(),
                legacy_storage_key: Some("storage-one".to_string()),
                legacy_agent_session_id: Some("agent-session-one".to_string()),
                conversation_id: id,
                identity_decision: IdentityDecision::AllocatedInvalidUuid,
                created_at_source: Some(CreatedAtSource::HostMetadata),
                source_record_sha256: "a".repeat(64),
            }],
        };
        let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
        let conversation = Arc::new(ConversationApplicationService::new(
            reader,
            writer,
            workspace,
            &map,
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
                registry: Arc::new(crate::web::ProjectRegistry::new()),
                registry_persistence: None,
                projects_file: None,
                history_mode: HistoryMode::LiveOnly,
                conversation: Some(conversation),
                workspace_manifest: None,
                acp_catalog: None,
                acp_install: None,
                store: None,
                project_root: Arc::new(parking_lot::RwLock::new(std::env::temp_dir())),
            },
        )
    }

    async fn state() -> (tempfile::TempDir, AppState) {
        let (temp, _repository, state) = state_with_repository().await;
        (temp, state)
    }

    async fn state_with_bound_session() -> (tempfile::TempDir, AppState) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("state/conversations/v2");
        let (repository, _) = ConversationRepository::open(root).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse(ID).unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: "/visible/conversation".to_string(),
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
                    agent_session_id: "opaque/phone-binding".to_string(),
                    runtime_agent_id: "claude-acp".to_string(),
                    stable_agent_namespace: "config:acp-registry:claude-acp".to_string(),
                    execution_cwd: "/visible/conversation".to_string(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            entries: vec![],
        };
        let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
        let conversation = Arc::new(ConversationApplicationService::new(
            reader,
            writer,
            workspace,
            &map,
            MigrationHostMode::Standalone,
            MigrationPhase::Finalized,
            ReaderPrecedence::ConversationV2Only,
        ));
        let pty = crate::web::test_pty_manager();
        (
            temp,
            AppState {
                acp: Arc::new(crate::acp::AcpManager::new(vec![])),
                terminal_events: pty.terminal_events(),
                cwd_tracker: pty.cwd_tracker(),
                git_tracker: pty.git_tracker(),
                exit_code_tracker: pty.exit_code_tracker(),
                pty,
                relay: Arc::new(crate::web::sink::WsRelaySink::new()),
                registry: Arc::new(crate::web::ProjectRegistry::new()),
                registry_persistence: None,
                projects_file: None,
                history_mode: HistoryMode::LiveOnly,
                conversation: Some(conversation),
                workspace_manifest: None,
                acp_catalog: None,
                acp_install: None,
                store: None,
                project_root: Arc::new(parking_lot::RwLock::new(std::env::temp_dir())),
            },
        )
    }

    fn app(state: AppState) -> axum::Router {
        let authority = Arc::new(RemoteAccessAuthority::for_tests("conversation-api-token"));
        let principal = authority.verify_bearer("conversation-api-token").unwrap();
        axum::Router::new()
            .route("/conversations/host-status", get(host_status))
            .route("/conversations", get(list))
            .route("/conversations/resolve-legacy", post(resolve_legacy))
            .route("/conversations/{conversationId}", get(super::get))
            .route(
                "/conversations/{conversationId}/binding",
                get(super::current_binding),
            )
            .route("/conversations/{conversationId}/open", post(open))
            .route("/conversation-recovery/resolve", post(resolve_recovery))
            .with_state(state)
            .layer(Extension(principal))
            .layer(Extension(authority))
    }

    async fn json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn list_get_open_status_and_exact_legacy_route_share_camel_case_envelopes() {
        let (_temp, state) = state().await;
        let app = app(state);
        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list_body: IpcBody<Vec<ConversationRecordV2>> = json(list_response).await;
        assert!(list_body.success);
        assert_eq!(list_body.data.unwrap()[0].conversation_id.to_string(), ID);

        let binding_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/conversations/{ID}/binding"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let binding_body: IpcBody<crate::conversation::ConversationBindingSnapshot> =
            json(binding_response).await;
        assert!(binding_body.success);
        let snapshot = binding_body.data.unwrap();
        assert_eq!(snapshot.conversation_id.to_string(), ID);
        assert!(snapshot.binding.is_none());

        for (source_kind, value) in [
            ("legacyStorageKey", "storage-one"),
            ("legacyAgentSessionId", "agent-session-one"),
            ("legacyChatHistoryId", "history-one"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/conversations/resolve-legacy")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&serde_json::json!({
                                "sourceKind": source_kind,
                                "value": value
                            }))
                            .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let body: IpcBody<LegacyConversationResolution> = json(response).await;
            assert!(body.success);
            assert_eq!(body.data.unwrap().canonical_route, format!("#/c/{ID}"));
        }
    }

    #[tokio::test]
    async fn current_binding_returns_active_agent_session() {
        let (_temp, state) = state_with_bound_session().await;
        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/conversations/{ID}/binding"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: IpcBody<crate::conversation::ConversationBindingSnapshot> = json(response).await;
        assert!(body.success);
        let snapshot = body.data.unwrap();
        assert_eq!(snapshot.conversation_id.to_string(), ID);
        let binding = snapshot.binding.expect("active binding");
        assert_eq!(binding.agent_session_id, "opaque/phone-binding");
        assert_eq!(binding.runtime_agent_id, "claude-acp");
    }

    fn seed_recovery(repository: &ConversationRepository) -> RecoveryItemV1 {
        use crate::conversation::migration::{
            RecoveryKind, RecoveryProvenanceV1, RecoveryQueueV1, RecoverySeverity,
        };
        let item = RecoveryItemV1::new(
            RecoveryKind::AmbiguousWorkspaceManifest,
            RecoverySeverity::Warning,
            vec!["legacy_workspace_manifests/0/shared.json".to_string()],
            vec![ConversationId::parse(ID).unwrap()],
            vec!["e".repeat(64)],
            vec![serde_json::json!({"candidate":"preserved"})],
            vec![RecoveryProvenanceV1 {
                source_kind: "legacy_workspace_manifests".to_string(),
                relative_path: "legacy_workspace_manifests/0/shared.json".to_string(),
                sha256: "e".repeat(64),
                preserved_read_only: true,
            }],
        );
        let state_root = repository
            .root()
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap();
        RecoveryQueueV1::new(Uuid::new_v4(), vec![item.clone()])
            .persist(
                &state_root
                    .join("conversation-migrations")
                    .join("workspace-recovery-v1"),
            )
            .unwrap();
        item
    }

    #[tokio::test]
    async fn recovery_actions_and_remote_forbidden_use_exact_shared_contract() {
        let cases = [
            ("inspect", serde_json::json!({}), None),
            (
                "associateConversation",
                serde_json::json!({"conversationId":ID}),
                Some("21aee10a-56b8-4624-a5e7-586c25dc8d1f"),
            ),
            (
                "startEmptyWorkspace",
                serde_json::json!({"conversationId":ID,"expectedWorkspaceRevision":null}),
                Some("d70c2b93-71bc-4df0-85a5-15bd1b7cf452"),
            ),
            (
                "dismissPreservedSource",
                serde_json::json!({"reasonCode":"deferLegacyProjection"}),
                Some("b025313d-df5d-4254-af4f-535b47ea570f"),
            ),
        ];
        for (action, payload, idempotency_key) in cases {
            let (_temp, repository, state) = state_with_repository().await;
            let item = seed_recovery(&repository);
            let mut request = serde_json::json!({
                "recoveryId":item.recovery_id,
                "expectedRevision":item.revision,
                "action":action,
                "payload":payload
            });
            if let Some(key) = idempotency_key {
                request
                    .as_object_mut()
                    .unwrap()
                    .insert("idempotencyKey".to_string(), serde_json::json!(key));
            }
            let response = app(state)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/conversation-recovery/resolve")
                        .header("content-type", "application/json")
                        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                        .body(Body::from(serde_json::to_vec(&request).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let body: IpcBody<RecoveryActionResult> = json(response).await;
            assert!(body.success, "{action}: {:?}", body.error);
            let result = body.data.unwrap();
            assert_eq!(serde_json::to_value(result.action).unwrap(), action);
            assert_eq!(result.source_paths, item.source_paths);
            assert_eq!(result.source_sha256, item.source_sha256);
            assert_eq!(result.candidate_facts, item.candidate_facts);
            assert_eq!(result.provenance, item.provenance);
        }

        let (_temp, repository, state) = state_with_repository().await;
        let item = seed_recovery(&repository);
        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversation-recovery/resolve")
                    .extension(ConnectInfo(SocketAddr::from(([192, 168, 1, 5], 3000))))
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "recoveryId":item.recovery_id,
                            "expectedRevision":item.revision,
                            "idempotencyKey":"21aee10a-56b8-4624-a5e7-586c25dc8d1f",
                            "action":"associateConversation",
                            "payload":{"conversationId":ID}
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: IpcBody<RecoveryActionResult> = json(response).await;
        assert!(
            body.success,
            "authenticated proxy requests must not rely on peer IP"
        );
    }

    #[tokio::test]
    async fn legacy_not_found_and_payload_validation_keep_stable_codes() {
        let (_temp, state) = state().await;
        let app = app(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations/resolve-legacy")
                    .body(Body::from(
                        r#"{"sourceKind":"legacyStorageKey","value":"missing"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: IpcBody<LegacyConversationResolution> = json(response).await;
        assert_eq!(body.code.as_deref(), Some("CONVERSATION_NOT_FOUND"));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations/resolve-legacy")
                    .body(Body::from(
                        r#"{"sourceKind":"legacy_storage_key","value":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: IpcBody<LegacyConversationResolution> = json(response).await;
        assert_eq!(body.code.as_deref(), Some("VALIDATION_ERROR"));
    }
}
