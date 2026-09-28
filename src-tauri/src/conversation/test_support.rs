//! Fixtures shared with host-side tests that exercise this domain through
//! the host's adapters.

use std::sync::Arc;

use crate::conversation::application::ConversationApplicationService;
use crate::conversation::contracts::{
    parse_created_at_utc, ConversationCreator, ConversationId, ConversationLifecycleState,
    ConversationRecordV2, CreationPartition, ExecutionTarget, CONVERSATION_SCHEMA_VERSION,
};
use crate::conversation::migration::{
    CreatedAtSource, IdentityDecision, MigrationHostMode, MigrationMapEntryV1, MigrationMapV1,
    MigrationPhase, ReaderPrecedence, RecoveryItemV1, MIGRATION_MAP_SCHEMA_VERSION,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};
use crate::conversation::{
    ConversationReader, ConversationRepository, LegacyConversationReader, SessionWorkspaceService,
};
use uuid::Uuid;

/// The Conversation every [`application_fixture`] seeds.
pub const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

pub async fn application_fixture() -> (
    tempfile::TempDir,
    Arc<ConversationRepository>,
    ConversationApplicationService,
) {
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
    let workspace_cwd = temp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir_all(&workspace_cwd).unwrap();
    writer
        .create_conversation(
            ConversationRecordV2 {
                schema_version: CONVERSATION_SCHEMA_VERSION,
                conversation_id: id,
                created_at_utc: created_at,
                creation_partition: CreationPartition::from_created_at(created_at),
                workspace_cwd: workspace_cwd.to_string_lossy().into_owned(),
                execution_target: ExecutionTarget::Workspace,
                project_attachment: None,
                lifecycle_state: ConversationLifecycleState::Ready,
                backend: crate::conversation::ConversationBackend::Agent,
                last_seq: 0,
                created_by: ConversationCreator::Legacy,
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
    let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
    let map = MigrationMapV1 {
        schema_version: MIGRATION_MAP_SCHEMA_VERSION,
        operation_id: Uuid::new_v4(),
        entries: vec![MigrationMapEntryV1 {
            source_key: "legacy_chat_history:0:payloads/chat-history.json".to_string(),
            legacy_storage_key: Some("legacy-storage".to_string()),
            legacy_agent_session_id: Some("opaque-agent-session".to_string()),
            conversation_id: id,
            identity_decision: IdentityDecision::AllocatedInvalidUuid,
            created_at_source: Some(CreatedAtSource::HostMetadata),
            source_record_sha256: "a".repeat(64),
        }],
    };
    (
        temp,
        repository,
        ConversationApplicationService::new(
            reader,
            writer,
            workspace,
            &map,
            MigrationHostMode::Desktop,
            MigrationPhase::Finalized,
            ReaderPrecedence::ConversationV2Only,
        ),
    )
}

pub fn seed_recovery(repository: &ConversationRepository) -> RecoveryItemV1 {
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
    RecoveryQueueV1::new(uuid::Uuid::new_v4(), vec![item.clone()])
        .persist(
            &state_root
                .join("conversation-migrations")
                .join("workspace-recovery-v1"),
        )
        .unwrap();
    item
}
