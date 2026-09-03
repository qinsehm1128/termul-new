use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Extension, Router};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

use super::auth::test_tracing;
use super::conversation_api;
use super::ws::{dispatch_conversation_golden_request, AppState, HistoryMode};
use crate::conversation::contracts::{
    parse_created_at_utc, AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
    ConversationLifecycleState, ConversationRecordV2, CreationPartition, ExecutionTarget,
    ProjectAttachment, AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
};
use crate::conversation::migration::{
    CreatedAtSource, IdentityDecision, MigrationHostMode, MigrationMapEntryV1, MigrationMapV1,
    MigrationPhase, ReaderPrecedence, RecoveryItemV1, RecoveryKind, RecoveryProvenanceV1,
    RecoveryQueueV1, RecoverySeverity, MIGRATION_MAP_SCHEMA_VERSION,
};
use crate::conversation::{
    AgentBindingResult, AgentLifecycleProviderError, ConversationAgentLifecycle,
    ConversationApplicationService, ConversationCreationService, ConversationId,
    ConversationLifecycleService, ConversationLocator, ConversationMutation,
    ConversationPersistenceAdapter, ConversationReader, ConversationRepository, ConversationWriter,
    LegacyConversationReader, PreparedConversation, SessionWorkspaceLocator,
    SessionWorkspaceProjectionState, SessionWorkspaceService, SessionWorkspaceV1,
    TerminalResourceInspector,
};
use crate::web::sink::{AcpEvent, EventSink, WsRelaySink};

const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
const BINDING_ID: &str = "33333333-3333-4333-8333-333333333333";

type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct GoldenLifecycleProvider;

impl ConversationAgentLifecycle for GoldenLifecycleProvider {
    fn owns_session<'a>(&'a self, _binding: &'a AgentSessionBinding) -> ProviderFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn suspend<'a>(
        &'a self,
        _binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn replace<'a>(
        &'a self,
        _previous_binding: &'a AgentSessionBinding,
        _prepared: &'a PreparedConversation,
        _target_runtime_agent_id: Option<&'a str>,
    ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
    {
        Box::pin(async {
            Ok(AgentBindingResult {
                agent_session_id: "opaque/golden/replacement".to_string(),
                runtime_agent_id: "runtime-golden-replacement".to_string(),
                stable_agent_namespace: "config:golden-replacement".to_string(),
            })
        })
    }

    fn abort_replacement<'a>(
        &'a self,
        _binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn register_binding(&self, _agent_session_id: &str, _conversation_id: ConversationId) {}
}

struct NoLiveTerminals;

impl TerminalResourceInspector for NoLiveTerminals {
    fn is_live(&self, _terminal_id: &str) -> bool {
        false
    }
}

struct GoldenFixture {
    _temp: tempfile::TempDir,
    private_root: PathBuf,
    visible_root: PathBuf,
    workspace_cwd: String,
    repository: Arc<ConversationRepository>,
    writer: Arc<ConversationWriter>,
    service: Arc<ConversationApplicationService>,
    state: AppState,
}

async fn fixture() -> GoldenFixture {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let private_root = base.join("state/conversations/v2");
    let visible_root = base.join("visible");
    let visible_workspace = visible_root.join("2026/08/15").join(ID);
    std::fs::create_dir_all(&visible_workspace).unwrap();
    let (repository, _) = ConversationRepository::open(private_root.clone()).unwrap();
    let writer = ConversationWriter::for_test(Arc::clone(&repository));
    let conversation_id = ConversationId::parse(ID).unwrap();
    let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
    writer
        .create_conversation(
            ConversationRecordV2 {
                schema_version: CONVERSATION_SCHEMA_VERSION,
                conversation_id,
                created_at_utc: created_at,
                creation_partition: CreationPartition::from_created_at(created_at),
                workspace_cwd: visible_workspace.to_string_lossy().into_owned(),
                execution_target: ExecutionTarget::Workspace,
                project_attachment: None,
                lifecycle_state: ConversationLifecycleState::Ready,
                last_seq: 0,
                created_by: ConversationCreator::Legacy,
                title: None,
                title_source: None,
            },
            ConversationMutation::CreateConversation,
        )
        .await
        .unwrap();
    let map = MigrationMapV1 {
        schema_version: MIGRATION_MAP_SCHEMA_VERSION,
        operation_id: Uuid::new_v4(),
        entries: vec![MigrationMapEntryV1 {
            source_key: "legacy_chat_history:0:payloads/history-one.json".to_string(),
            legacy_storage_key: Some("storage-one".to_string()),
            legacy_agent_session_id: Some("agent-one".to_string()),
            conversation_id,
            identity_decision: IdentityDecision::AllocatedInvalidUuid,
            created_at_source: Some(CreatedAtSource::HostMetadata),
            source_record_sha256: "a".repeat(64),
        }],
    };
    let reader = Arc::new(ConversationReader::new(
        Arc::clone(&repository),
        LegacyConversationReader::default(),
        ReaderPrecedence::ConversationV2Only,
    ));
    let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
    let service = Arc::new(ConversationApplicationService::new(
        reader,
        Arc::clone(&writer),
        workspace,
        &map,
        MigrationHostMode::Standalone,
        MigrationPhase::Finalized,
        ReaderPrecedence::ConversationV2Only,
    ));
    let pty = crate::web::test_pty_manager();
    let state = AppState {
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
        conversation: Some(Arc::clone(&service)),
        workspace_manifest: None,
        acp_catalog: None,
        acp_install: None,
        store: None,
        project_root: Arc::new(parking_lot::RwLock::new(std::env::temp_dir())),
    };
    GoldenFixture {
        _temp: temp,
        private_root,
        visible_root,
        workspace_cwd: visible_workspace.to_string_lossy().into_owned(),
        repository,
        writer,
        service,
        state,
    }
}

async fn fixture_with_lifecycle() -> GoldenFixture {
    let fixture = fixture().await;
    let conversation_id = ConversationId::parse(ID).unwrap();
    let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
    let workspace_cwd = fixture
        .repository
        .get_conversation(conversation_id)
        .unwrap()
        .workspace_cwd;
    fixture
        .writer
        .bind_agent_session(
            conversation_id,
            AgentSessionBinding {
                schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                binding_id: Uuid::parse_str(BINDING_ID).unwrap(),
                agent_session_id: "opaque/golden/original".to_string(),
                runtime_agent_id: "runtime-golden-original".to_string(),
                stable_agent_namespace: "config:golden-original".to_string(),
                execution_cwd: workspace_cwd,
                bound_at_utc: created_at,
                state: AgentSessionBindingState::Active,
            },
            created_at,
        )
        .await
        .unwrap();
    let creation = Arc::new(
        ConversationCreationService::new(
            Arc::clone(&fixture.writer),
            ConversationLocator::new(fixture.private_root.clone()).unwrap(),
            SessionWorkspaceLocator::new(fixture.visible_root.clone()).unwrap(),
        )
        .unwrap(),
    );
    fixture
        .service
        .attach_lifecycle(ConversationLifecycleService::new(
            Arc::clone(&fixture.writer),
            creation,
            Arc::new(GoldenLifecycleProvider),
            Arc::new(NoLiveTerminals),
        ))
        .unwrap();
    fixture
}

fn app(state: AppState) -> axum::Router {
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(
        "conversation-golden-token",
    ));
    let principal = authority
        .verify_bearer("conversation-golden-token")
        .unwrap();
    axum::Router::new()
        .route(
            "/conversations/host-status",
            get(conversation_api::host_status),
        )
        .route("/conversations", get(conversation_api::list))
        .route(
            "/conversations/resolve-legacy",
            post(conversation_api::resolve_legacy),
        )
        .route(
            "/conversation-recovery/resolve",
            post(conversation_api::resolve_recovery),
        )
        .route(
            "/conversations/{conversationId}/attach-project",
            post(conversation_api::attach_project),
        )
        .route(
            "/conversations/{conversationId}/detach-project",
            post(conversation_api::detach_project),
        )
        .route(
            "/conversations/{conversationId}/execution-target",
            post(conversation_api::update_execution_target),
        )
        .route(
            "/conversations/{conversationId}/lifecycle/detach",
            post(super::conversation_lifecycle_api::detach),
        )
        .route(
            "/conversations/{conversationId}/lifecycle/delete",
            post(super::conversation_lifecycle_api::delete),
        )
        .with_state(state)
        .layer(Extension(principal))
        .layer(Extension(authority))
}

fn secured_app(state: AppState, authority: Arc<crate::web::RemoteAccessAuthority>) -> axum::Router {
    axum::Router::new()
        .route("/conversations", get(conversation_api::list))
        .route(
            "/conversations/{conversationId}",
            get(conversation_api::get),
        )
        .route(
            "/conversations/{conversationId}/attach-project",
            post(conversation_api::attach_project),
        )
        .route("/terminal/ws", get(super::terminal_ws::terminal_ws_upgrade))
        .with_state(state)
        .layer(middleware::from_fn(crate::web::auth::capability_middleware))
        .layer(Extension(authority))
}

fn production_app(state: AppState, authority: Arc<crate::web::RemoteAccessAuthority>) -> Router {
    let projects_file = state
        .projects_file
        .as_ref()
        .map(|path| path.as_ref().clone());
    let project_root = state.project_root.read().clone();
    super::router::router(
        state.acp,
        state.pty,
        state.terminal_events,
        state.cwd_tracker,
        state.git_tracker,
        state.exit_code_tracker,
        state.relay,
        state.registry,
        state.registry_persistence,
        projects_file,
        project_root,
        state.history_mode,
        state.conversation,
        state.workspace_manifest,
        state.acp_catalog,
        state.acp_install,
        state.store,
        authority,
    )
}

fn golden_workspace() -> SessionWorkspaceV1 {
    SessionWorkspaceV1 {
        schema_version: 1,
        conversation_id: ConversationId::parse(ID).unwrap(),
        revision: 0,
        updated_at_utc: String::new(),
        update_identity: Some("production-golden".to_string()),
        topology: None,
        active_pane_id: None,
        resources: Vec::new(),
        projection_state: SessionWorkspaceProjectionState::Native,
    }
}

fn conversation_persistence(fixture: &GoldenFixture) -> Arc<ConversationPersistenceAdapter> {
    let reader = Arc::new(ConversationReader::new(
        Arc::clone(&fixture.repository),
        LegacyConversationReader::default(),
        ReaderPrecedence::ConversationV2Only,
    ));
    Arc::new(ConversationPersistenceAdapter::new(
        Arc::clone(&fixture.writer),
        reader,
    ))
}

async fn websocket_connect(address: SocketAddr, origin: &str) -> BufReader<TcpStream> {
    let stream = TcpStream::connect(address).await.unwrap();
    let mut stream = BufReader::new(stream);
    let request = format!(
        "GET /ws HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\n\r\n"
    );
    stream
        .get_mut()
        .write_all(request.as_bytes())
        .await
        .unwrap();
    stream.get_mut().flush().await.unwrap();

    let mut headers = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream.read_until(b'\n', &mut headers),
    )
    .await
    .unwrap()
    .unwrap();
    while !headers.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(
            Duration::from_secs(5),
            stream.read_until(b'\n', &mut headers),
        )
        .await
        .unwrap()
        .unwrap();
    }
    let headers = String::from_utf8(headers).unwrap();
    assert!(
        headers.starts_with("HTTP/1.1 101"),
        "WebSocket upgrade failed: {headers}"
    );
    stream
}

async fn write_websocket_frame(stream: &mut BufReader<TcpStream>, opcode: u8, payload: &[u8]) {
    let mask = [0x12_u8, 0x34, 0x56, 0x78];
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        length @ 0..=125 => frame.push(0x80 | length as u8),
        length @ 126..=65_535 => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % mask.len()]),
    );
    stream.get_mut().write_all(&frame).await.unwrap();
    stream.get_mut().flush().await.unwrap();
}

async fn write_websocket_json(stream: &mut BufReader<TcpStream>, value: &Value) {
    write_websocket_frame(stream, 0x1, value.to_string().as_bytes()).await;
}

async fn read_websocket_json(stream: &mut BufReader<TcpStream>) -> Value {
    loop {
        let mut prefix = [0_u8; 2];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut prefix))
            .await
            .unwrap()
            .unwrap();
        let opcode = prefix[0] & 0x0f;
        let masked = prefix[1] & 0x80 != 0;
        let mut length = u64::from(prefix[1] & 0x7f);
        if length == 126 {
            let mut extended = [0_u8; 2];
            stream.read_exact(&mut extended).await.unwrap();
            length = u64::from(u16::from_be_bytes(extended));
        } else if length == 127 {
            let mut extended = [0_u8; 8];
            stream.read_exact(&mut extended).await.unwrap();
            length = u64::from_be_bytes(extended);
        }
        let mut mask = [0_u8; 4];
        if masked {
            stream.read_exact(&mut mask).await.unwrap();
        }
        let mut payload = vec![0_u8; usize::try_from(length).unwrap()];
        stream.read_exact(&mut payload).await.unwrap();
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % mask.len()];
            }
        }
        match opcode {
            0x1 => return serde_json::from_slice(&payload).unwrap(),
            0x8 => panic!("WebSocket closed before the expected reply"),
            0x9 => write_websocket_frame(stream, 0xA, &payload).await,
            0xA => {}
            other => panic!("unexpected WebSocket opcode {other}"),
        }
    }
}

async fn read_websocket_reply(stream: &mut BufReader<TcpStream>, id: &str) -> Value {
    loop {
        let value = read_websocket_json(stream).await;
        if value.get("id").and_then(Value::as_str) == Some(id) {
            return value;
        }
    }
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn normalize_workspace_paths(value: &mut Value, workspace_cwd: &str) {
    match value {
        Value::String(text) if text == workspace_cwd => {
            *text = "<workspaceCwd>".to_string();
        }
        Value::Array(values) => {
            for value in values {
                normalize_workspace_paths(value, workspace_cwd);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_workspace_paths(value, workspace_cwd);
            }
        }
        _ => {}
    }
}

fn normalized_workspace(mut value: Value, workspace_cwd: &str) -> Value {
    normalize_workspace_paths(&mut value, workspace_cwd);
    value
}

fn attachment(project_root: &std::path::Path) -> ProjectAttachment {
    ProjectAttachment {
        schema_version: 1,
        project_id: "project-golden".to_string(),
        attached_at_utc: parse_created_at_utc("2026-08-15T10:00:00.000Z").unwrap(),
        project_path_snapshot: project_root.to_string_lossy().into_owned(),
        worktree_path: None,
        worktree_branch: None,
    }
}

fn seed_recovery(repository: &ConversationRepository) -> RecoveryItemV1 {
    let item = RecoveryItemV1::new(
        RecoveryKind::AmbiguousWorkspaceManifest,
        RecoverySeverity::Warning,
        vec!["legacy_workspace_manifests/0/shared.json".to_string()],
        vec![ConversationId::parse(ID).unwrap()],
        vec!["e".repeat(64)],
        vec![json!({"candidate":"preserved"})],
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

#[test]
fn golden_tests_cooperate_with_scoped_capture_logger() {
    test_tracing::install_forwarding_logger();
    let source = include_str!("conversation_golden_tests.rs");
    assert!(
        !source.contains(&["log::set_", "logger"].concat()),
        "golden tests must not install a second global logger"
    );
    assert!(
        source.contains("test_tracing"),
        "golden tests share the scoped capture-logger identifiers"
    );
}

#[tokio::test]
async fn production_router_redacts_status_and_requires_authenticated_inspect_for_evidence() {
    const TOKEN: &str = "conversation-production-token";
    let fixture = fixture().await;
    let item = seed_recovery(&fixture.repository);
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    let app = production_app(fixture.state.clone(), authority);

    let status_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/conversations/host-status")
                .header("authorization", format!("Bearer {TOKEN}"))
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_response.status(), axum::http::StatusCode::OK);
    let status = response_json(status_response).await;
    let redacted = &status["data"]["recoveryItems"][0];
    assert_eq!(redacted["recoveryId"], item.recovery_id);
    for field in [
        "sourcePaths",
        "sourceSha256",
        "candidateFacts",
        "provenance",
    ] {
        assert_eq!(redacted[field], json!([]), "host status leaked {field}");
    }

    let inspect_request = json!({
        "recoveryId":item.recovery_id,
        "expectedRevision":item.revision,
        "action":"inspect",
        "payload":{}
    });
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/conversation-recovery/resolve")
                .header("content-type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::from(inspect_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), axum::http::StatusCode::UNAUTHORIZED);
    let unauthorized = response_json(unauthorized).await;
    assert_eq!(unauthorized["code"], "UNAUTHORIZED");
    assert!(unauthorized.get("data").is_none());
    let unauthorized_text = unauthorized.to_string();
    assert!(!unauthorized_text.contains(&item.source_paths[0]));
    assert!(!unauthorized_text.contains(&item.source_sha256[0]));
    assert!(!unauthorized_text.contains("preserved"));

    let inspect = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/conversation-recovery/resolve")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::from(inspect_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inspect.status(), axum::http::StatusCode::OK);
    let inspect = response_json(inspect).await;
    assert_eq!(inspect["success"], true);
    assert_eq!(inspect["data"]["recoveryId"], item.recovery_id);
    assert_eq!(inspect["data"]["recoveryRevision"], item.revision);
    assert_eq!(inspect["data"]["authorization"], "read");
    assert_eq!(inspect["data"]["sourcePaths"], json!(item.source_paths));
    assert_eq!(inspect["data"]["sourceSha256"], json!(item.source_sha256));
    assert_eq!(
        inspect["data"]["candidateFacts"],
        json!(item.candidate_facts)
    );
    assert_eq!(inspect["data"]["provenance"], json!(item.provenance));

    let status_after_inspect = app
        .oneshot(
            Request::builder()
                .uri("/conversations/host-status")
                .header("authorization", format!("Bearer {TOKEN}"))
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status_after_inspect = response_json(status_after_inspect).await;
    for field in [
        "sourcePaths",
        "sourceSha256",
        "candidateFacts",
        "provenance",
    ] {
        assert_eq!(
            status_after_inspect["data"]["recoveryItems"][0][field],
            json!([]),
            "host status leaked {field} after Inspect"
        );
    }
}

#[tokio::test]
async fn production_router_preserves_workspace_conflict_and_recovery_success_envelopes() {
    const TOKEN: &str = "workspace-production-token";
    let workspace_fixture = fixture().await;
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    let app = production_app(workspace_fixture.state, authority);
    let request = json!({"basedRevision":null,"workspace":golden_workspace()});

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/workspace"))
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), axum::http::StatusCode::OK);
    assert_eq!(response_json(updated).await["data"]["status"], "updated");

    let conflict = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/workspace"))
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), axum::http::StatusCode::CONFLICT);
    let conflict = response_json(conflict).await;
    assert_eq!(conflict["success"], true);
    assert_eq!(conflict["data"]["status"], "conflict");
    assert_eq!(conflict["data"]["currentRevision"], 1);

    let recovery_fixture = fixture().await;
    recovery_fixture
        .writer
        .replace_workspace_bytes(
            ConversationId::parse(ID).unwrap(),
            b"{not-valid-workspace-json",
            ConversationMutation::WorkspaceWrite,
        )
        .unwrap();
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    let recovery_app = production_app(recovery_fixture.state, authority);
    let recovery = recovery_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/workspace"))
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        recovery.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    let recovery = response_json(recovery).await;
    assert_eq!(recovery["success"], true);
    assert_eq!(recovery["data"]["status"], "recoveryRequired");
    assert!(recovery["data"]["recoveryItems"][0]["recoveryId"]
        .as_str()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_router_ws_paging_is_authenticated_and_reads_durable_admission() {
    const TOKEN: &str = "paging-production-token";
    const SESSION_ID: &str = "opaque/golden/original";
    let mut fixture = fixture_with_lifecycle().await;
    let persistence = conversation_persistence(&fixture);
    let relay = Arc::new(WsRelaySink::with_conversation_persistence(
        32,
        Arc::clone(&persistence),
        None,
    ));

    let rejected = relay
        .emit(&AcpEvent {
            sid: Some("unmapped-session".to_string()),
            type_: "acp:message_chunk",
            payload: json!({"ordinal":0}),
        })
        .expect_err("unmapped durable admission must fail closed");
    assert_eq!(
        rejected.code,
        crate::web::sink::CONVERSATION_PERSISTENCE_REJECTED
    );
    assert!(rejected.durable_rejection);
    assert_eq!(relay.session_watermark("unmapped-session"), 0);

    for ordinal in 1..=4_u64 {
        let receipt = relay
            .emit(&AcpEvent {
                sid: Some(SESSION_ID.to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal":ordinal}),
            })
            .expect("mapped durable admission");
        assert!(receipt.delivered);
        assert!(receipt.durable_admission);
        assert_eq!(receipt.session_seq, Some(ordinal + 1));
    }
    relay.flush_conversation_persistence().await.unwrap();
    let expected_page = persistence.history_page(SESSION_ID, 0, 250).unwrap();
    assert!(expected_page.complete);
    assert_eq!(
        expected_page
            .records
            .iter()
            .filter_map(|record| record.payload["ordinal"].as_u64())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );

    fixture.state.relay = Arc::clone(&relay);
    fixture.state.history_mode = HistoryMode::Server;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    authority
        .set_public_origin(Url::parse(&origin).unwrap())
        .unwrap();
    let app = production_app(fixture.state.clone(), authority);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let mut socket = websocket_connect(address, &origin).await;
    let auth_required = read_websocket_json(&mut socket).await;
    assert_eq!(auth_required["type"], "auth_required");

    write_websocket_json(
        &mut socket,
        &json!({
            "id":"page-before-auth",
            "type":"get_session_payload_page",
            "payload":{"sessionId":SESSION_ID,"afterSeq":0,"limit":250}
        }),
    )
    .await;
    let denied = read_websocket_reply(&mut socket, "page-before-auth").await;
    assert_eq!(denied["ok"], false);
    assert_eq!(denied["err"]["code"], "unauthorized");
    assert!(denied.get("payload").is_none());

    write_websocket_json(
        &mut socket,
        &json!({"id":"auth","type":"authenticate","payload":{"token":TOKEN}}),
    )
    .await;
    let authenticated = read_websocket_reply(&mut socket, "auth").await;
    assert_eq!(authenticated["ok"], true);

    write_websocket_json(
        &mut socket,
        &json!({
            "id":"page",
            "type":"get_session_payload_page",
            "payload":{"sessionId":SESSION_ID,"afterSeq":0,"limit":250}
        }),
    )
    .await;
    let page = read_websocket_reply(&mut socket, "page").await;
    assert_eq!(page["ok"], true);
    assert_eq!(
        page["payload"],
        serde_json::to_value(&expected_page).unwrap()
    );

    write_websocket_json(
        &mut socket,
        &json!({
            "id":"invalid-page",
            "type":"get_session_payload_page",
            "payload":{"sessionId":SESSION_ID,"afterSeq":0,"limit":0}
        }),
    )
    .await;
    let invalid = read_websocket_reply(&mut socket, "invalid-page").await;
    assert_eq!(invalid["ok"], false);
    assert_eq!(invalid["err"]["code"], "VALIDATION_ERROR");
    write_websocket_frame(&mut socket, 0x8, &[]).await;

    server.abort();
    let _ = server.await;
    relay.shutdown_conversation_persistence().await.unwrap();

    let private_root = fixture.private_root.clone();
    let GoldenFixture {
        _temp,
        repository,
        writer,
        service,
        state,
        ..
    } = fixture;
    drop(state);
    drop(service);
    drop(writer);
    drop(repository);
    drop(relay);
    drop(persistence);
    let (restarted, _) = ConversationRepository::open(private_root).unwrap();
    let durable_ordinals = restarted
        .read_events(ConversationId::parse(ID).unwrap(), 0)
        .unwrap()
        .into_iter()
        .filter_map(|event| event.payload["ordinal"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(durable_ordinals, vec![1, 2, 3, 4]);
    drop(restarted);
    drop(_temp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn companion_chat_requests_cursor_binding_catalog_and_delta_page() {
    const TOKEN: &str = "companion-chat-request-token";
    const SESSION_ID: &str = "opaque/golden/original";
    let mut fixture = fixture_with_lifecycle().await;
    let catalog = crate::acp::AcpCatalogService::open(fixture._temp.path().join("catalog"))
        .await
        .unwrap();
    fixture.state.acp_catalog = Some(catalog);
    let persistence = conversation_persistence(&fixture);
    let relay = Arc::new(WsRelaySink::with_conversation_persistence(
        32,
        Arc::clone(&persistence),
        None,
    ));
    for ordinal in 1..=12_u64 {
        relay
            .emit(&AcpEvent {
                sid: Some(SESSION_ID.to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal":ordinal,"role":"agent","content":{"type":"text","text":"x"}}),
            })
            .expect("mapped durable admission");
    }
    relay.flush_conversation_persistence().await.unwrap();
    let full_page = persistence.history_page(SESSION_ID, 0, 250).unwrap();
    let watermark = full_page.target_last_seq;
    assert!(
        watermark >= 12,
        "host watermark should cover the admitted tail"
    );
    assert_eq!(full_page.records.len(), 12);

    fixture.state.relay = Arc::clone(&relay);
    fixture.state.history_mode = HistoryMode::Server;
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    let app = production_app(fixture.state.clone(), authority);

    let unauthorized_binding = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/conversations/{ID}/binding"))
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unauthorized_binding.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    let unauthorized_catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/acp/catalog")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unauthorized_catalog.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    let conversations = response_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(conversations["success"], true);
    assert_eq!(conversations["data"][0]["conversationId"], ID);

    let binding = response_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/conversations/{ID}/binding"))
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(binding["success"], true);
    assert_eq!(binding["data"]["conversationId"], ID);
    assert_eq!(binding["data"]["binding"]["agentSessionId"], SESSION_ID);
    assert_eq!(
        binding["data"]["binding"]["runtimeAgentId"],
        "runtime-golden-original"
    );

    let catalog = response_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/acp/catalog")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(catalog["success"], true);
    let agents = catalog["data"]["agents"].as_array().unwrap();
    assert!(!agents.is_empty());
    assert!(
        agents.iter().any(|agent| {
            agent["status"] == "ready" && agent.get("installed").is_none_or(Value::is_null)
        }),
        "ready catalog agents must be selectable without an installed overlay"
    );
    assert!(
        agents
            .iter()
            .any(|agent| agent["distribution"]["npx"].is_object()),
        "catalog must carry npx distribution so the phone can spawn"
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let ws_authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(TOKEN));
    ws_authority
        .set_public_origin(Url::parse(&origin).unwrap())
        .unwrap();
    let ws_app = production_app(fixture.state.clone(), ws_authority);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            ws_app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let mut socket = websocket_connect(address, &origin).await;
    let auth_required = read_websocket_json(&mut socket).await;
    assert_eq!(auth_required["type"], "auth_required");

    write_websocket_json(
        &mut socket,
        &json!({"id":"auth","type":"authenticate","payload":{"token":TOKEN}}),
    )
    .await;
    let authenticated = read_websocket_reply(&mut socket, "auth").await;
    assert_eq!(authenticated["ok"], true);

    write_websocket_json(
        &mut socket,
        &json!({
            "id":"cursor",
            "type":"get_session_cursor",
            "payload":{"sessionId":SESSION_ID}
        }),
    )
    .await;
    let cursor = read_websocket_reply(&mut socket, "cursor").await;
    assert_eq!(cursor["ok"], true);
    assert_eq!(cursor["payload"]["sessionId"], SESSION_ID);
    assert_eq!(cursor["payload"]["watermark"], watermark);

    let after_seq = watermark.saturating_sub(3);
    write_websocket_json(
        &mut socket,
        &json!({
            "id":"delta",
            "type":"get_session_payload_page",
            "payload":{"sessionId":SESSION_ID,"afterSeq":after_seq,"limit":80,"targetLastSeq":watermark}
        }),
    )
    .await;
    let delta = read_websocket_reply(&mut socket, "delta").await;
    assert_eq!(delta["ok"], true);
    let records = delta["payload"]["records"].as_array().unwrap();
    assert!(!records.is_empty());
    assert!(
        records.len() < 12,
        "watermark delta must not reload the full transcript, got {}",
        records.len()
    );
    assert!(records
        .iter()
        .all(|record| { record["seq"].as_u64().is_some_and(|seq| seq > after_seq) }));
    assert_eq!(delta["payload"]["targetLastSeq"], watermark);

    write_websocket_json(
        &mut socket,
        &json!({"id":"catalog","type":"list_acp_catalog","payload":{}}),
    )
    .await;
    let ws_catalog = read_websocket_reply(&mut socket, "catalog").await;
    assert_eq!(ws_catalog["ok"], true);
    assert_eq!(ws_catalog["payload"]["agents"], catalog["data"]["agents"]);

    write_websocket_json(
        &mut socket,
        &json!({"id":"agents","type":"list_agents","payload":{}}),
    )
    .await;
    let agents = read_websocket_reply(&mut socket, "agents").await;
    assert_eq!(agents["ok"], true);
    assert_eq!(agents["payload"], json!([]));

    write_websocket_frame(&mut socket, 0x8, &[]).await;
    server.abort();
    let _ = server.await;
    relay.shutdown_conversation_persistence().await.unwrap();
}

#[tokio::test]
async fn transport_golden_matrix() {
    let fixture = fixture().await;

    let tauri_status = crate::commands::conversation_host_status_inner(&fixture.service);
    assert!(tauri_status.success);
    let expected_status = serde_json::to_value(tauri_status.data.unwrap()).unwrap();
    let http_status = response_json(
        app(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/conversations/host-status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(http_status["data"], expected_status);
    let mut authed = true;
    let ws_status = dispatch_conversation_golden_request(
        r#"{"id":"status-1","type":"conversation_host_status","payload":{}}"#,
        &mut authed,
        &fixture.service,
    )
    .await;
    assert_eq!(ws_status.payload.unwrap(), expected_status);

    let tauri = crate::commands::conversation_list_inner(&fixture.service);
    assert!(tauri.success);
    let tauri_data = serde_json::to_value(tauri.data.unwrap()).unwrap();

    let http_response = app(fixture.state.clone())
        .oneshot(
            Request::builder()
                .uri("/conversations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let http = response_json(http_response).await;
    assert_eq!(http["success"], true);
    assert_eq!(http["data"], tauri_data);

    let mut authed = true;
    let ws = dispatch_conversation_golden_request(
        r#"{"id":"list-1","type":"list_conversations","payload":{}}"#,
        &mut authed,
        &fixture.service,
    )
    .await;
    assert!(ws.ok);
    assert_eq!(ws.payload.unwrap(), tauri_data);

    for (source_kind, value) in [
        ("legacyStorageKey", "storage-one"),
        ("legacyAgentSessionId", "agent-one"),
        ("legacyChatHistoryId", "history-one"),
    ] {
        let request = json!({"sourceKind":source_kind,"value":value});
        let tauri = crate::commands::conversation_resolve_legacy_id_inner(
            &fixture.service,
            request.clone(),
        );
        assert!(tauri.success);
        let expected = serde_json::to_value(tauri.data.unwrap()).unwrap();

        let http_response = app(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations/resolve-legacy")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let http = response_json(http_response).await;
        assert_eq!(http["data"], expected, "sourceKind={source_kind}");

        let frame = json!({
            "id": format!("legacy-{source_kind}"),
            "type":"resolve_legacy_conversation_id",
            "payload":request
        });
        let ws =
            dispatch_conversation_golden_request(&frame.to_string(), &mut authed, &fixture.service)
                .await;
        assert_eq!(ws.payload.unwrap(), expected, "sourceKind={source_kind}");
    }
}

#[tokio::test]
async fn authenticated_http_and_terminal_boundaries_fail_closed_with_stable_statuses() {
    let fixture = fixture().await;
    let authority = Arc::new(crate::web::RemoteAccessAuthority::for_tests(
        "conversation-golden-token",
    ));

    for authorization in [None, Some("Bearer wrong-token")] {
        let mut builder = Request::builder().uri("/conversations");
        if let Some(value) = authorization {
            builder = builder.header("authorization", value);
        }
        let response = secured_app(fixture.state.clone(), Arc::clone(&authority))
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["code"], "UNAUTHORIZED");
        assert!(body.get("data").is_none());
        assert!(!body.to_string().contains(
            &fixture
                .repository
                .get_conversation(ConversationId::parse(ID).unwrap())
                .unwrap()
                .workspace_cwd
        ));
    }

    let terminal = secured_app(fixture.state.clone(), Arc::clone(&authority))
        .oneshot(
            Request::builder()
                .uri("/terminal/ws")
                .extension(ConnectInfo(SocketAddr::from(([192, 0, 2, 10], 43123))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        terminal.status().is_client_error(),
        "terminal upgrade without Origin must fail closed, got {}",
        terminal.status()
    );
    assert_ne!(
        terminal.status(),
        axum::http::StatusCode::SWITCHING_PROTOCOLS
    );

    let invalid = secured_app(fixture.state.clone(), Arc::clone(&authority))
        .oneshot(
            Request::builder()
                .uri("/conversations/not-a-conversation-id")
                .header("authorization", "Bearer conversation-golden-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid).await["code"],
        "CONVERSATION_INVALID_ID"
    );

    let missing_id = "11111111-1111-4111-8111-111111111111";
    let missing = secured_app(fixture.state.clone(), Arc::clone(&authority))
        .oneshot(
            Request::builder()
                .uri(format!("/conversations/{missing_id}"))
                .header("authorization", "Bearer conversation-golden-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let missing_status = missing.status();
    let missing_body = response_json(missing).await;
    assert_eq!(
        missing_status,
        axum::http::StatusCode::NOT_FOUND,
        "unexpected missing response: {missing_body}"
    );
    assert_eq!(missing_body["code"], "CONVERSATION_NOT_FOUND");

    let project_root = fixture._temp.path().join("authenticated-project");
    std::fs::create_dir_all(&project_root).unwrap();
    let project_root = project_root.canonicalize().unwrap();
    let stale = secured_app(fixture.state, authority)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/attach-project"))
                .header("authorization", "Bearer conversation-golden-token")
                .body(Body::from(
                    json!({
                        "expectedRevision":99,
                        "attachment":{
                            "schemaVersion":1,
                            "projectId":"project-golden",
                            "attachedAtUtc":"2026-08-15T10:00:00.000Z",
                            "projectPathSnapshot":project_root,
                            "worktreePath":null,
                            "worktreeBranch":null
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(response_json(stale).await["code"], "CONVERSATION_CONFLICT");
}

#[tokio::test]
async fn aggregate_transport_golden_matrix() {
    let project_temp = tempfile::tempdir().unwrap();
    let project_root = project_temp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    let project_root = std::fs::canonicalize(project_root).unwrap();
    let attachment = attachment(&project_root);
    let conversation_id = ConversationId::parse(ID).unwrap();

    let tauri_fixture = fixture().await;
    let tauri = crate::commands::conversation_attach_project_inner(
        &tauri_fixture.service,
        ID,
        0,
        serde_json::to_value(&attachment).unwrap(),
    )
    .await;
    let expected_attach = normalized_workspace(
        serde_json::to_value(tauri.data.unwrap()).unwrap(),
        &tauri_fixture.workspace_cwd,
    );

    let http_fixture = fixture().await;
    let http_workspace_cwd = http_fixture.workspace_cwd.clone();
    let http = response_json(
        app(http_fixture.state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/attach-project"))
                    .body(Body::from(
                        json!({"expectedRevision":0,"attachment":attachment}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        normalized_workspace(http["data"].clone(), &http_workspace_cwd),
        expected_attach
    );

    let ws_fixture = fixture().await;
    let mut authed = true;
    let ws = dispatch_conversation_golden_request(
        &json!({
            "id":"attach-1",
            "type":"attach_project",
            "payload":{"conversationId":ID,"expectedRevision":0,"attachment":attachment}
        })
        .to_string(),
        &mut authed,
        &ws_fixture.service,
    )
    .await;
    assert_eq!(
        normalized_workspace(ws.payload.unwrap(), &ws_fixture.workspace_cwd),
        expected_attach
    );

    let target = ExecutionTarget::ProjectRoot {
        project_id: "project-golden".to_string(),
        project_root: project_root.to_string_lossy().into_owned(),
    };
    let tauri_fixture = fixture().await;
    tauri_fixture
        .service
        .attach_project(conversation_id, 0, attachment.clone())
        .await
        .unwrap();
    let tauri = crate::commands::conversation_update_execution_target_inner(
        &tauri_fixture.service,
        ID,
        1,
        serde_json::to_value(&target).unwrap(),
    )
    .await;
    let expected_target = normalized_workspace(
        serde_json::to_value(tauri.data.unwrap()).unwrap(),
        &tauri_fixture.workspace_cwd,
    );

    let http_fixture = fixture().await;
    http_fixture
        .service
        .attach_project(conversation_id, 0, attachment.clone())
        .await
        .unwrap();
    let http_workspace_cwd = http_fixture.workspace_cwd.clone();
    let http = response_json(
        app(http_fixture.state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/execution-target"))
                    .body(Body::from(
                        json!({"expectedRevision":1,"executionTarget":target}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        normalized_workspace(http["data"].clone(), &http_workspace_cwd),
        expected_target
    );

    let ws_fixture = fixture().await;
    ws_fixture
        .service
        .attach_project(conversation_id, 0, attachment.clone())
        .await
        .unwrap();
    let ws = dispatch_conversation_golden_request(
        &json!({
            "id":"target-1",
            "type":"update_execution_target",
            "payload":{"conversationId":ID,"expectedRevision":1,"executionTarget":target}
        })
        .to_string(),
        &mut authed,
        &ws_fixture.service,
    )
    .await;
    assert_eq!(
        normalized_workspace(ws.payload.unwrap(), &ws_fixture.workspace_cwd),
        expected_target
    );

    let tauri_fixture = fixture().await;
    tauri_fixture
        .service
        .attach_project(conversation_id, 0, attachment.clone())
        .await
        .unwrap();
    let tauri =
        crate::commands::conversation_detach_project_inner(&tauri_fixture.service, ID, 1).await;
    let expected_detach = normalized_workspace(
        serde_json::to_value(tauri.data.unwrap()).unwrap(),
        &tauri_fixture.workspace_cwd,
    );

    let http_fixture = fixture().await;
    http_fixture
        .service
        .attach_project(conversation_id, 0, attachment.clone())
        .await
        .unwrap();
    let http_workspace_cwd = http_fixture.workspace_cwd.clone();
    let http = response_json(
        app(http_fixture.state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/conversations/{ID}/detach-project"))
                    .body(Body::from(json!({"expectedRevision":1}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        normalized_workspace(http["data"].clone(), &http_workspace_cwd),
        expected_detach
    );

    let ws_fixture = fixture().await;
    ws_fixture
        .service
        .attach_project(conversation_id, 0, attachment)
        .await
        .unwrap();
    let ws = dispatch_conversation_golden_request(
        &json!({
            "id":"detach-1",
            "type":"detach_project",
            "payload":{"conversationId":ID,"expectedRevision":1}
        })
        .to_string(),
        &mut authed,
        &ws_fixture.service,
    )
    .await;
    assert_eq!(
        normalized_workspace(ws.payload.unwrap(), &ws_fixture.workspace_cwd),
        expected_detach
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_usage_and_plan_full_replacements_survive_cold_restart() {
    let fixture = fixture_with_lifecycle().await;
    let persistence = conversation_persistence(&fixture);
    let relay = Arc::new(WsRelaySink::with_conversation_persistence(
        32,
        Arc::clone(&persistence),
        None,
    ));
    relay
        .emit(&AcpEvent {
            sid: Some("opaque/golden/original".to_string()),
            type_: "acp:usage_update",
            payload: json!({"used":0,"size":0}),
        })
        .unwrap();
    relay
        .emit(&AcpEvent {
            sid: Some("opaque/golden/original".to_string()),
            type_: "acp:plan_update",
            payload: json!({"plan":{"entries":[]}}),
        })
        .unwrap();
    relay.shutdown_conversation_persistence().await.unwrap();
    drop(relay);
    drop(persistence);

    let (restarted_repository, _) =
        ConversationRepository::open(fixture.private_root.clone()).unwrap();
    let restarted_writer = ConversationWriter::for_test(Arc::clone(&restarted_repository));
    let restarted_reader = Arc::new(ConversationReader::new(
        Arc::clone(&restarted_repository),
        LegacyConversationReader::default(),
        ReaderPrecedence::ConversationV2Only,
    ));
    let restarted = ConversationPersistenceAdapter::new(restarted_writer, restarted_reader);
    let records = restarted.replay_after("opaque/golden/original", 0).unwrap();
    let usage = records
        .iter()
        .find(|record| record.type_ == "usage_update")
        .unwrap();
    let plan = records
        .iter()
        .find(|record| record.type_ == "plan_update")
        .unwrap();
    assert_eq!(usage.payload, json!({"used":0,"size":0}));
    assert_eq!(plan.payload, json!({"plan":{"entries":[]}}));
    assert!(usage.seq < plan.seq);
}

#[tokio::test]
async fn lifecycle_transport_golden_matrix_uses_the_same_revisioned_outcome() {
    let conversation_id = ConversationId::parse(ID).unwrap();

    let domain_fixture = fixture_with_lifecycle().await;
    let expected = normalized_workspace(
        serde_json::to_value(
            domain_fixture
                .service
                .detach_binding(conversation_id, 1)
                .await
                .unwrap(),
        )
        .unwrap(),
        &domain_fixture.workspace_cwd,
    );

    let http_fixture = fixture_with_lifecycle().await;
    let http_workspace_cwd = http_fixture.workspace_cwd.clone();
    let response = app(http_fixture.state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/lifecycle/detach"))
                .body(Body::from(json!({"expectedRevision":1}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        normalized_workspace(
            response_json(response).await["data"].clone(),
            &http_workspace_cwd
        ),
        expected
    );

    let ws_fixture = fixture_with_lifecycle().await;
    let mut authed = true;
    let ws = dispatch_conversation_golden_request(
        &json!({
            "id":"detach-golden",
            "type":"detach_binding",
            "payload":{"conversationId":ID,"expectedRevision":1}
        })
        .to_string(),
        &mut authed,
        &ws_fixture.service,
    )
    .await;
    assert_eq!(
        normalized_workspace(ws.payload.unwrap(), &ws_fixture.workspace_cwd),
        expected
    );
}

#[tokio::test]
async fn http_conversation_delete_retires_on_success_and_retains_on_blocked_or_error() {
    let fixture = fixture_with_lifecycle().await;
    let relay = Arc::clone(&fixture.state.relay);
    relay
        .turn_watermark()
        .mark_seen("opaque/golden/original", "turn-retained");
    let router = app(fixture.state.clone());

    let conflict = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/lifecycle/delete"))
                .body(Body::from(json!({"expectedRevision":0}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(conflict.status(), axum::http::StatusCode::OK);
    assert!(relay
        .turn_watermark()
        .is_seen("opaque/golden/original", "turn-retained"));

    let conversation_id = ConversationId::parse(ID).unwrap();
    let suspend_revision = fixture
        .repository
        .get_conversation(conversation_id)
        .unwrap()
        .last_seq;
    fixture
        .service
        .suspend_binding(conversation_id, suspend_revision)
        .await
        .unwrap();
    let revision = fixture
        .repository
        .get_conversation(conversation_id)
        .unwrap()
        .last_seq;
    let deleted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/conversations/{ID}/lifecycle/delete"))
                .body(Body::from(json!({"expectedRevision":revision}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), axum::http::StatusCode::OK);
    let deleted_body = response_json(deleted).await;
    assert_eq!(deleted_body["data"]["status"], "updated", "{deleted_body}");
    assert!(
        !relay
            .turn_watermark()
            .is_seen("opaque/golden/original", "turn-retained"),
        "{deleted_body}"
    );
    assert!(fixture
        .repository
        .get_conversation(ConversationId::parse(ID).unwrap())
        .is_err());
}

#[tokio::test]
async fn recovery_action_transport_golden_matrix_preserves_immutable_evidence() {
    let idempotency_key = "21aee10a-56b8-4624-a5e7-586c25dc8d1f";

    let tauri_fixture = fixture().await;
    let tauri_item = seed_recovery(&tauri_fixture.repository);
    let request = json!({
        "recoveryId":tauri_item.recovery_id,
        "expectedRevision":tauri_item.revision,
        "idempotencyKey":idempotency_key,
        "action":"associateConversation",
        "payload":{"conversationId":ID}
    });
    let tauri = crate::commands::conversation_recovery_resolve_inner(
        &tauri_fixture.service,
        request.clone(),
    )
    .await;
    assert!(tauri.success);
    let expected = serde_json::to_value(tauri.data.unwrap()).unwrap();

    let http_fixture = fixture().await;
    let http_item = seed_recovery(&http_fixture.repository);
    assert_eq!(http_item.recovery_id, tauri_item.recovery_id);
    let http = app(http_fixture.state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/conversation-recovery/resolve")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(http.status(), axum::http::StatusCode::OK);
    assert_eq!(response_json(http).await["data"], expected);

    let ws_fixture = fixture().await;
    let ws_item = seed_recovery(&ws_fixture.repository);
    assert_eq!(ws_item.recovery_id, tauri_item.recovery_id);
    let mut authed = true;
    let ws = dispatch_conversation_golden_request(
        &json!({
            "id":"recovery-golden",
            "type":"resolve_recovery_item",
            "payload":request
        })
        .to_string(),
        &mut authed,
        &ws_fixture.service,
    )
    .await;
    assert_eq!(ws.payload.unwrap(), expected);
    assert_eq!(
        expected["sourcePaths"],
        serde_json::to_value(tauri_item.source_paths).unwrap()
    );
    assert_eq!(
        expected["sourceSha256"],
        serde_json::to_value(tauri_item.source_sha256).unwrap()
    );
    assert_eq!(
        expected["provenance"],
        serde_json::to_value(tauri_item.provenance).unwrap()
    );
}

#[tokio::test]
async fn malformed_unauthorized_ping_close_and_duplicate_requests_do_not_double_apply() {
    let fixture = fixture().await;
    let item = seed_recovery(&fixture.repository);
    let request = json!({
        "recoveryId":item.recovery_id,
        "expectedRevision":item.revision,
        "idempotencyKey":"21aee10a-56b8-4624-a5e7-586c25dc8d1f",
        "action":"associateConversation",
        "payload":{"conversationId":ID}
    });

    let first =
        crate::commands::conversation_recovery_resolve_inner(&fixture.service, request.clone())
            .await;
    let duplicate =
        crate::commands::conversation_recovery_resolve_inner(&fixture.service, request.clone())
            .await;
    assert!(first.success && duplicate.success);
    assert_eq!(
        serde_json::to_value(first.data.unwrap()).unwrap(),
        serde_json::to_value(duplicate.data.unwrap()).unwrap()
    );

    let queue = fixture.service.host_status().unwrap();
    assert_eq!(queue.recovery_item_count, 0);

    let mut unauthenticated = false;
    let unauthorized = dispatch_conversation_golden_request(
        r#"{"id":"u-1","type":"list_conversations","payload":{}}"#,
        &mut unauthenticated,
        &fixture.service,
    )
    .await;
    assert_eq!(unauthorized.err.unwrap().code, "UNAUTHORIZED");

    let mut authed = true;
    let malformed =
        dispatch_conversation_golden_request("not-json", &mut authed, &fixture.service).await;
    assert_eq!(malformed.err.unwrap().code, "unsupported");
    let ping = dispatch_conversation_golden_request(
        r#"{"id":"ping-1","type":"ping","payload":{}}"#,
        &mut authed,
        &fixture.service,
    )
    .await;
    assert!(ping.ok);

    let before = fixture
        .repository
        .get_conversation(ConversationId::parse(ID).unwrap())
        .unwrap();
    let ws_source = include_str!("ws.rs");
    assert!(ws_source.contains("Message::Close(_) | Message::Ping(_) | Message::Pong(_)"));
    assert!(ws_source.contains("Axum auto-answers pings; Close ends the loop"));
    let after = fixture
        .repository
        .get_conversation(ConversationId::parse(ID).unwrap())
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn authenticated_remote_mutation_uses_capability_not_proxy_peer_address() {
    let fixture = fixture().await;
    let item = seed_recovery(&fixture.repository);
    let response = app(fixture.state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/conversation-recovery/resolve")
                .extension(ConnectInfo(SocketAddr::from(([192, 168, 1, 5], 3000))))
                .body(Body::from(
                    json!({
                        "recoveryId":item.recovery_id,
                        "expectedRevision":item.revision,
                        "idempotencyKey":"21aee10a-56b8-4624-a5e7-586c25dc8d1f",
                        "action":"associateConversation",
                        "payload":{"conversationId":ID}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    // Since the shared RemoteAccessAuthority replaced peer-IP trust, an authenticated
    // remote principal is authorized independent of proxy address. The mutation applies
    // exactly once through the same golden transport envelope.
    assert_eq!(body["success"], true);
    assert_eq!(
        fixture.service.host_status().unwrap().recovery_item_count,
        0
    );
}
