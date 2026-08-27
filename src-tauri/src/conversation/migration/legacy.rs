//! Copy-on-upgrade transformation from immutable legacy sources into ConversationRepository.
//!
//! Legacy storage keys are identity candidates, ACP session ids are opaque provenance/bindings,
//! and every non-reused identity is allocated once then durably recorded before staging begins.
//! This module never mutates a legacy root.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::inventory::{
    LegacyInventoryRootV1, LegacyInventoryV1, LegacySourceKind, LEGACY_INVENTORY_SCHEMA_VERSION,
};
use super::recovery::{
    RecoveryItemV1, RecoveryKind, RecoveryProvenanceV1, RecoveryQueueV1, RecoverySeverity,
};
use super::{MigrationError, MigrationErrorCode, Result};
use crate::conversation::catalog::{
    ConversationProvenanceFileV1, ConversationProvenanceSourceV1, PROVENANCE_SCHEMA_VERSION,
};
use crate::conversation::contracts::{
    AgentSessionBinding, AgentSessionBindingState, ConversationCreator, ConversationId,
    ConversationLifecycleState, ConversationRecordV2, CreationPartition, ExecutionTarget,
    ProjectAttachment, AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    PROJECT_ATTACHMENT_SCHEMA_VERSION,
};
use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};
use crate::conversation::event_log::{ConversationEventRecordV2, ConversationEventType};
use crate::conversation::repository::ConversationRepository;
use crate::conversation::write_authority::MigrationWriter;

pub const MIGRATION_MAP_SCHEMA_VERSION: u32 = 1;
pub const MIGRATION_MAP_FILE: &str = "migration-map-v1.json";
pub const STAGED_MANIFEST_FILE: &str = "staged-manifest-v1.json";
const STAGE_RECEIPTS_DIR: &str = "stage-receipts";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityDecision {
    ReusedUniqueStorageUuid,
    AllocatedInvalidUuid,
    AllocatedCollisionUuid,
    AllocatedChatOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedAtSource {
    HostMetadata,
    ChatPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationMapEntryV1 {
    pub source_key: String,
    pub legacy_storage_key: Option<String>,
    pub legacy_agent_session_id: Option<String>,
    pub conversation_id: ConversationId,
    pub identity_decision: IdentityDecision,
    pub created_at_source: Option<CreatedAtSource>,
    pub source_record_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationMapV1 {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub entries: Vec<MigrationMapEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageReceiptV1 {
    pub source_key: String,
    pub conversation_id: ConversationId,
    pub output_sha256: String,
    pub message_count: u64,
    pub tool_count: u64,
    pub event_count: u64,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
    pub opaque_agent_session_id: Option<String>,
    pub provenance_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagedManifestV1 {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub inventory_sha256: String,
    pub migration_map_sha256: String,
    pub recovery_queue_sha256: String,
    pub receipts: Vec<StageReceiptV1>,
    pub staged_manifest_sha256: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectWorktreeRecord {
    pub cwd: String,
    pub worktree_path: String,
    pub worktree_branch: String,
}

#[derive(Debug, Clone)]
pub struct LegacyStageConfiguration {
    pub host_state_root: PathBuf,
    pub operation_dir: PathBuf,
    pub project_worktrees: Vec<ProjectWorktreeRecord>,
}

#[derive(Debug, Clone)]
struct SourceFile {
    kind: LegacySourceKind,
    root_index: usize,
    root: PathBuf,
    relative_path: String,
    sha256: String,
}

impl SourceFile {
    fn path(&self) -> PathBuf {
        self.root.join(
            self.relative_path
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        )
    }

    fn provenance_path(&self) -> String {
        format!(
            "{}/{}/{}",
            self.kind.as_str(),
            self.root_index,
            self.relative_path
        )
    }

    fn provenance(&self) -> ConversationProvenanceSourceV1 {
        ConversationProvenanceSourceV1 {
            source_kind: self.kind.as_str().to_string(),
            relative_path: self.provenance_path(),
            sha256: self.sha256.clone(),
            preserved_read_only: true,
        }
    }

    fn recovery_provenance(&self) -> RecoveryProvenanceV1 {
        RecoveryProvenanceV1 {
            source_kind: self.kind.as_str().to_string(),
            relative_path: self.provenance_path(),
            sha256: self.sha256.clone(),
            preserved_read_only: true,
        }
    }
}

#[derive(Debug, Clone)]
struct HostSource {
    source_key: String,
    metadata: SourceFile,
    messages: Option<SourceFile>,
    tools: Option<SourceFile>,
    value: Value,
}

impl HostSource {
    fn storage_key(&self) -> Option<String> {
        string_field(&self.value, "storageKey")
    }
    fn session_id(&self) -> Option<String> {
        string_field(&self.value, "sessionId")
    }
    fn cwd(&self) -> String {
        string_field(&self.value, "cwd").unwrap_or_default()
    }
    fn project_id(&self) -> Option<String> {
        string_field(&self.value, "projectId").filter(|value| !value.is_empty())
    }
    fn stable_namespace(&self) -> Option<String> {
        string_field(&self.value, "stableAgentNamespace").filter(|value| !value.is_empty())
    }
    fn runtime_agent_id(&self) -> Option<String> {
        string_field(&self.value, "runtimeAgentId").filter(|value| !value.is_empty())
    }
    fn created_at(&self) -> Option<u64> {
        self.value.get("createdAt").and_then(Value::as_u64)
    }
    fn worktree_path(&self) -> Option<String> {
        string_field(&self.value, "worktreePath").filter(|value| !value.is_empty())
    }
    fn worktree_branch(&self) -> Option<String> {
        string_field(&self.value, "worktreeBranch").filter(|value| !value.is_empty())
    }
    fn provenance(&self) -> Vec<ConversationProvenanceSourceV1> {
        let mut records = vec![self.metadata.provenance()];
        records.extend(self.messages.iter().map(SourceFile::provenance));
        records.extend(self.tools.iter().map(SourceFile::provenance));
        records
    }
}

#[derive(Debug, Clone)]
struct ChatSource {
    source_key: String,
    payload: SourceFile,
    value: Value,
}

impl ChatSource {
    fn metadata(&self) -> Option<&serde_json::Map<String, Value>> {
        self.value.get("metadata")?.as_object()
    }
    fn session_id(&self) -> Option<String> {
        self.metadata()?.get("id")?.as_str().map(str::to_string)
    }
    fn created_at(&self) -> Option<u64> {
        self.metadata()?.get("createdAt")?.as_u64()
    }
    fn cwd(&self) -> Option<String> {
        self.metadata()?
            .get("cwd")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
    fn project_id(&self) -> Option<String> {
        self.metadata()?
            .get("projectId")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
    fn worktree_path(&self) -> Option<String> {
        self.metadata()?
            .get("worktreePath")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
    fn worktree_branch(&self) -> Option<String> {
        self.metadata()?
            .get("worktreeBranch")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
}

#[derive(Debug, Clone)]
struct PlannedConversation {
    map: MigrationMapEntryV1,
    host: Option<HostSource>,
    created_at: Option<DateTime<Utc>>,
    cwd: String,
    project_id: Option<String>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
    provenance: Vec<ConversationProvenanceSourceV1>,
    events: Vec<PlannedEvent>,
    recovery: Vec<RecoveryItemV1>,
    blocking: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct PlannedEvent {
    recorded_at_utc: DateTime<Utc>,
    type_: ConversationEventType,
    payload: Value,
}

/// Build durable identity mappings, stage repository records, provenance, and receipts, then write
/// the aggregate manifest and recovery queue. Invalid timestamps remain blocking recovery items.
pub async fn stage_legacy_conversations(
    configuration: &LegacyStageConfiguration,
    inventory: &LegacyInventoryV1,
) -> Result<StagedManifestV1> {
    if inventory.schema_version != LEGACY_INVENTORY_SCHEMA_VERSION {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "stage_legacy",
            "unsupported legacy inventory schemaVersion",
        ));
    }
    let (hosts, chats, manifests, mut parse_recovery) = load_sources(inventory)?;
    let repository_root = configuration
        .host_state_root
        .join("conversations")
        .join("v2");
    let (repository, _) =
        ConversationRepository::open_staging(repository_root).map_err(repository_error)?;
    let writer = MigrationWriter::new(repository).map_err(repository_error)?;
    let reserved_conversation_ids = writer
        .repository()
        .list_conversations()
        .into_iter()
        .map(|record| record.conversation_id)
        .collect::<HashSet<_>>();
    let existing_map = load_migration_map(&configuration.operation_dir)?;
    let (mut planned, migration_map, mut planning_recovery) = plan_conversations(
        inventory,
        hosts,
        chats,
        &configuration.project_worktrees,
        &reserved_conversation_ids,
        existing_map.as_ref(),
    )?;
    persist_migration_map(&configuration.operation_dir, &migration_map)?;
    planning_recovery.append(&mut parse_recovery);
    planning_recovery.extend(plan_manifest_recovery(&manifests, &planned));
    let mut receipts = Vec::new();
    for plan in &mut planned {
        planning_recovery.append(&mut plan.recovery);
        if plan.blocking {
            continue;
        }
        let Some(created_at) = plan.created_at else {
            continue;
        };
        let receipt = stage_one(
            &writer,
            &configuration.operation_dir,
            inventory.operation_id,
            plan,
            created_at,
        )
        .await?;
        log::info!(
            "[conversation-migration] legacy conversation staged conversation_id={} source={} message_count={} tool_count={}",
            receipt.conversation_id,
            sanitized_source(&plan.map.source_key),
            receipt.message_count,
            receipt.tool_count
        );
        receipts.push(receipt);
    }
    receipts.sort_by_key(|receipt| receipt.conversation_id.to_string());

    let recovery_queue = RecoveryQueueV1::new(inventory.operation_id, planning_recovery);
    for item in &recovery_queue.items {
        log::warn!(
            "[conversation-migration] recovery item kind={:?} recovery_id={} source_count={} candidate_count={}",
            item.kind,
            item.recovery_id,
            item.source_sha256.len(),
            item.candidate_facts.len()
        );
    }
    recovery_queue.persist(&configuration.operation_dir)?;

    let migration_map_sha256 = sha256_json(&migration_map)?;
    let recovery_queue_sha256 = sha256_json(&recovery_queue)?;
    let staged_manifest_sha256 = stage_digest(
        inventory.operation_id,
        &inventory.inventory_sha256,
        &migration_map_sha256,
        &recovery_queue_sha256,
        &receipts,
    )?;
    let manifest = StagedManifestV1 {
        schema_version: 1,
        operation_id: inventory.operation_id,
        inventory_sha256: inventory.inventory_sha256.clone(),
        migration_map_sha256,
        recovery_queue_sha256,
        receipts,
        staged_manifest_sha256,
    };
    persist_json(
        &configuration.operation_dir.join(STAGED_MANIFEST_FILE),
        &manifest,
    )?;
    log::info!(
        "[conversation-migration] legacy stage complete operation_id={} staged_conversation_count={} recovery_item_count={} staged_digest={}",
        inventory.operation_id,
        manifest.receipts.len(),
        recovery_queue.items.len(),
        digest_prefix(&manifest.staged_manifest_sha256)
    );
    Ok(manifest)
}

pub fn load_migration_map(operation_dir: &Path) -> Result<Option<MigrationMapV1>> {
    let path = operation_dir.join(MIGRATION_MAP_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read_migration_map", &path, error)),
    };
    let map: MigrationMapV1 = serde_json::from_slice(&bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_migration_map",
            error.to_string(),
        )
    })?;
    if map.schema_version != MIGRATION_MAP_SCHEMA_VERSION {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_migration_map",
            "unsupported migration map schemaVersion",
        ));
    }
    Ok(Some(map))
}

pub fn load_staged_manifest(operation_dir: &Path) -> Result<StagedManifestV1> {
    let path = operation_dir.join(STAGED_MANIFEST_FILE);
    let bytes = fs::read(&path).map_err(|error| io_error("read_staged_manifest", &path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_staged_manifest",
            error.to_string(),
        )
    })
}

async fn stage_one(
    writer: &MigrationWriter,
    operation_dir: &Path,
    operation_id: Uuid,
    plan: &PlannedConversation,
    created_at: DateTime<Utc>,
) -> Result<StageReceiptV1> {
    let receipt_path = receipt_path(operation_dir, plan.map.conversation_id);
    if let Ok(bytes) = fs::read(&receipt_path) {
        let receipt: StageReceiptV1 = serde_json::from_slice(&bytes).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "read_stage_receipt",
                error.to_string(),
            )
        })?;
        verify_receipt_matches_plan(writer.repository(), plan, &receipt)?;
        return Ok(receipt);
    }

    let project_attachment = plan
        .project_id
        .as_ref()
        .map(|project_id| ProjectAttachment {
            schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
            project_id: project_id.clone(),
            attached_at_utc: created_at,
            project_path_snapshot: plan.cwd.clone(),
            worktree_path: plan.worktree_path.clone(),
            worktree_branch: plan.worktree_branch.clone(),
        });
    let execution_target = match (
        plan.project_id.as_ref(),
        plan.worktree_path.as_ref(),
        plan.worktree_branch.as_ref(),
    ) {
        (Some(project_id), Some(path), Some(branch)) => ExecutionTarget::Worktree {
            project_id: project_id.clone(),
            worktree_path: path.clone(),
            worktree_branch: branch.clone(),
        },
        _ => ExecutionTarget::Workspace,
    };
    let record = ConversationRecordV2 {
        schema_version: CONVERSATION_SCHEMA_VERSION,
        conversation_id: plan.map.conversation_id,
        created_at_utc: created_at,
        creation_partition: CreationPartition::from_created_at(created_at),
        workspace_cwd: plan.cwd.clone(),
        execution_target,
        project_attachment,
        lifecycle_state: if plan.map.legacy_agent_session_id.is_some() {
            ConversationLifecycleState::Ready
        } else {
            ConversationLifecycleState::AgentFailed
        },
        last_seq: 0,
        created_by: ConversationCreator::Termul,
        title: None,
        title_source: None,
    };
    match writer
        .repository()
        .get_conversation(plan.map.conversation_id)
    {
        Ok(existing) => {
            if existing.created_at_utc != record.created_at_utc
                || existing.workspace_cwd != record.workspace_cwd
                || existing.project_attachment != record.project_attachment
            {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationIdempotencyConflict,
                    "stage_legacy",
                    "existing staged Conversation metadata differs from the durable mapping",
                ));
            }
        }
        Err(error)
            if error.code
                == crate::conversation::contracts::ConversationErrorCode::ConversationNotFound =>
        {
            writer
                .create_conversation(record)
                .await
                .map_err(repository_error)?;
        }
        Err(error) => return Err(repository_error(error)),
    }

    let mut all_events = Vec::new();
    if let Some(host) = &plan.host {
        if let (Some(agent_session_id), Some(runtime_agent_id), Some(stable_agent_namespace)) = (
            host.session_id(),
            host.runtime_agent_id(),
            host.stable_namespace(),
        ) {
            let binding = AgentSessionBinding {
                schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                binding_id: deterministic_binding_id(&plan.map.source_record_sha256),
                agent_session_id,
                runtime_agent_id,
                stable_agent_namespace,
                execution_cwd: if let Some(path) = &plan.worktree_path {
                    path.clone()
                } else {
                    plan.cwd.clone()
                },
                bound_at_utc: created_at,
                state: AgentSessionBindingState::Active,
            };
            all_events.push(PlannedEvent {
                recorded_at_utc: created_at,
                type_: ConversationEventType::BindingBound,
                payload: serde_json::to_value(
                    crate::conversation::event_log::BindingEventPayloadV1 { binding },
                )
                .expect("binding payload is serializable"),
            });
        }
    }
    all_events.extend(plan.events.clone());
    let existing_events = writer
        .repository()
        .read_events(plan.map.conversation_id, 0)
        .map_err(repository_error)?;
    if existing_events.len() > all_events.len()
        || existing_events
            .iter()
            .zip(all_events.iter())
            .any(|(existing, expected)| {
                existing.type_ != expected.type_
                    || existing.payload != expected.payload
                    || existing.recorded_at_utc != expected.recorded_at_utc
            })
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationIdempotencyConflict,
            "stage_legacy",
            "existing staged events are not a prefix of the deterministic migration output",
        ));
    }
    for event in all_events.iter().skip(existing_events.len()) {
        writer
            .append_event(
                plan.map.conversation_id,
                event.recorded_at_utc,
                event.type_,
                event.payload.clone(),
            )
            .await
            .map_err(repository_error)?;
    }

    let mut provenance = plan.provenance.clone();
    provenance.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    provenance.dedup();
    let provenance_file = ConversationProvenanceFileV1 {
        schema_version: PROVENANCE_SCHEMA_VERSION,
        migration_id: operation_id.to_string(),
        source_records: provenance,
    };
    writer
        .write_provenance(plan.map.conversation_id, provenance_file.clone())
        .await
        .map_err(repository_error)?;
    writer
        .sync_conversation(plan.map.conversation_id)
        .await
        .map_err(repository_error)?;

    let staged_record = writer
        .repository()
        .get_conversation(plan.map.conversation_id)
        .map_err(repository_error)?;
    let staged_events = writer
        .repository()
        .read_events(plan.map.conversation_id, 0)
        .map_err(repository_error)?;
    let summary = writer
        .repository()
        .history_summary(plan.map.conversation_id)
        .map_err(repository_error)?;
    let message_count = summary.message_count;
    let tool_count = summary.tool_count;
    let receipt = StageReceiptV1 {
        source_key: plan.map.source_key.clone(),
        conversation_id: plan.map.conversation_id,
        output_sha256: stage_output_digest(&staged_record, &staged_events, &provenance_file)?,
        message_count,
        tool_count,
        event_count: staged_events.len() as u64,
        worktree_path: plan.worktree_path.clone(),
        worktree_branch: plan.worktree_branch.clone(),
        opaque_agent_session_id: plan.map.legacy_agent_session_id.clone(),
        provenance_sha256: sha256_json(&provenance_file)?,
    };
    persist_json(&receipt_path, &receipt)?;
    Ok(receipt)
}

fn verify_receipt_matches_plan(
    repository: &ConversationRepository,
    plan: &PlannedConversation,
    receipt: &StageReceiptV1,
) -> Result<()> {
    if receipt.source_key != plan.map.source_key
        || receipt.conversation_id != plan.map.conversation_id
        || receipt.opaque_agent_session_id != plan.map.legacy_agent_session_id
        || repository
            .get_conversation(receipt.conversation_id)
            .map_err(repository_error)?
            .conversation_id
            != receipt.conversation_id
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationIdempotencyConflict,
            "reuse_stage_receipt",
            "stage receipt does not match the durable source mapping",
        ));
    }
    Ok(())
}

type LoadedSources = (
    Vec<HostSource>,
    Vec<ChatSource>,
    Vec<SourceFile>,
    Vec<RecoveryItemV1>,
);

fn load_sources(inventory: &LegacyInventoryV1) -> Result<LoadedSources> {
    let mut hosts = Vec::new();
    let mut chats = Vec::new();
    let mut manifests = Vec::new();
    let mut recovery = Vec::new();
    for (root_index, root) in inventory.roots.iter().enumerate() {
        match root.source_kind {
            LegacySourceKind::LegacyHostSessions => {
                let files = source_files(root, root_index);
                let by_path = files
                    .into_iter()
                    .map(|file| (file.relative_path.clone(), file))
                    .collect::<HashMap<_, _>>();
                for (relative, metadata) in by_path
                    .iter()
                    .filter(|(relative, _)| relative.ends_with("/metadata.json"))
                {
                    let storage_directory = relative
                        .strip_suffix("/metadata.json")
                        .expect("filtered suffix")
                        .to_string();
                    match read_json(&metadata.path()) {
                        Ok(value) => hosts.push(HostSource {
                            source_key: format!(
                                "legacy_host_session:{root_index}:{storage_directory}"
                            ),
                            metadata: metadata.clone(),
                            messages: by_path
                                .get(&format!("{storage_directory}/messages.jsonl"))
                                .cloned(),
                            tools: by_path
                                .get(&format!("{storage_directory}/tool-calls.jsonl"))
                                .cloned(),
                            value,
                        }),
                        Err(_) => recovery.push(corrupt_source_item(metadata)),
                    }
                }
            }
            LegacySourceKind::LegacyChatHistory => {
                for file in source_files(root, root_index)
                    .into_iter()
                    .filter(|file| file.relative_path.starts_with("payloads/"))
                {
                    match read_json(&file.path()).and_then(decode_chat_payload) {
                        Ok(value) => chats.push(ChatSource {
                            source_key: format!(
                                "legacy_chat_history:{root_index}:{}",
                                file.relative_path
                            ),
                            payload: file,
                            value,
                        }),
                        Err(_) => recovery.push(corrupt_source_item(&file)),
                    }
                }
            }
            LegacySourceKind::LegacyWorkspaceManifests => {
                manifests.extend(source_files(root, root_index));
            }
        }
    }
    hosts.sort_by(|left, right| left.source_key.cmp(&right.source_key));
    chats.sort_by(|left, right| left.source_key.cmp(&right.source_key));
    Ok((hosts, chats, manifests, recovery))
}

fn plan_conversations(
    inventory: &LegacyInventoryV1,
    hosts: Vec<HostSource>,
    chats: Vec<ChatSource>,
    project_worktrees: &[ProjectWorktreeRecord],
    reserved_conversation_ids: &HashSet<ConversationId>,
    existing_map: Option<&MigrationMapV1>,
) -> Result<(
    Vec<PlannedConversation>,
    MigrationMapV1,
    Vec<RecoveryItemV1>,
)> {
    let chat_by_session = chats
        .iter()
        .filter_map(|chat| chat.session_id().map(|id| (id, chat.clone())))
        .collect::<HashMap<_, _>>();
    let matched_chat_keys = hosts
        .iter()
        .filter_map(HostSource::session_id)
        .collect::<HashSet<_>>();
    let mut candidates = hosts
        .into_iter()
        .map(|host| {
            let chat = host
                .session_id()
                .and_then(|session_id| chat_by_session.get(&session_id).cloned());
            (Some(host), chat)
        })
        .collect::<Vec<_>>();
    candidates.extend(
        chats
            .into_iter()
            .filter(|chat| {
                chat.session_id()
                    .is_none_or(|session_id| !matched_chat_keys.contains(&session_id))
            })
            .map(|chat| (None, Some(chat))),
    );
    candidates.sort_by_key(|(host, chat)| {
        host.as_ref()
            .map(|value| value.source_key.clone())
            .or_else(|| chat.as_ref().map(|value| value.source_key.clone()))
            .unwrap_or_default()
    });

    let mut uuid_counts = HashMap::<Uuid, usize>::new();
    for (host, _) in &candidates {
        if let Some(uuid) = host
            .as_ref()
            .and_then(HostSource::storage_key)
            .and_then(|value| Uuid::parse_str(&value).ok())
        {
            *uuid_counts.entry(uuid).or_default() += 1;
        }
    }
    let existing_by_source = existing_map
        .map(|map| {
            map.entries
                .iter()
                .cloned()
                .map(|entry| (entry.source_key.clone(), entry))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut used_ids = HashSet::new();
    let mut map_entries = Vec::new();
    let mut planned = Vec::new();
    let mut recovery = Vec::new();

    for (host, chat) in candidates {
        let source_key = host
            .as_ref()
            .map(|value| value.source_key.clone())
            .or_else(|| chat.as_ref().map(|value| value.source_key.clone()))
            .expect("candidate has a source");
        let legacy_storage_key = host.as_ref().and_then(HostSource::storage_key);
        let source_sha256 = host
            .as_ref()
            .map(|value| value.metadata.sha256.clone())
            .or_else(|| chat.as_ref().map(|value| value.payload.sha256.clone()))
            .expect("candidate has a source digest");
        let parsed_storage = legacy_storage_key
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok());
        let default_decision = match (host.as_ref(), parsed_storage) {
            (None, _) => IdentityDecision::AllocatedChatOnly,
            (Some(_), Some(uuid))
                if uuid_counts.get(&uuid) == Some(&1)
                    && !reserved_conversation_ids.contains(
                        &ConversationId::parse(&uuid.to_string())
                            .expect("parsed UUID is a valid ConversationId"),
                    ) =>
            {
                IdentityDecision::ReusedUniqueStorageUuid
            }
            (Some(_), Some(_)) => IdentityDecision::AllocatedCollisionUuid,
            (Some(_), None) => IdentityDecision::AllocatedInvalidUuid,
        };
        let (conversation_id, identity_decision) = if let Some(existing) =
            existing_by_source.get(&source_key)
        {
            if existing.source_record_sha256 != source_sha256
                || existing.legacy_storage_key != legacy_storage_key
            {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationIdempotencyConflict,
                    "plan_legacy_identity",
                    "durable mapping source bytes or identity facts changed",
                ));
            }
            (existing.conversation_id, existing.identity_decision)
        } else if default_decision == IdentityDecision::ReusedUniqueStorageUuid {
            (
                ConversationId::parse(&parsed_storage.expect("decision requires UUID").to_string())
                    .expect("parsed UUID is a valid ConversationId"),
                default_decision,
            )
        } else {
            let allocated = loop {
                let candidate = ConversationId::new_v4();
                if !reserved_conversation_ids.contains(&candidate) && !used_ids.contains(&candidate)
                {
                    break candidate;
                }
            };
            (allocated, default_decision)
        };
        if !used_ids.insert(conversation_id) {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationIdempotencyConflict,
                "plan_legacy_identity",
                "two legacy source records map to one ConversationId",
            ));
        }
        if identity_decision == IdentityDecision::AllocatedCollisionUuid {
            recovery.push(RecoveryItemV1::new(
                RecoveryKind::IdentifierCollision,
                RecoverySeverity::Warning,
                vec![source_key.clone()],
                vec![conversation_id],
                vec![source_sha256.clone()],
                vec![json!({"legacyStorageKey":legacy_storage_key})],
                source_recovery_provenance(host.as_ref(), chat.as_ref()),
            ));
        }

        let host_created = host
            .as_ref()
            .and_then(HostSource::created_at)
            .and_then(valid_millis);
        let chat_created = chat
            .as_ref()
            .and_then(ChatSource::created_at)
            .and_then(valid_millis);
        let (created_at, created_at_source) = if let Some(value) = host_created {
            if chat_created.is_some_and(|chat_value| chat_value != value) {
                recovery.push(RecoveryItemV1::new(
                    RecoveryKind::ConflictingSessionMetadata,
                    RecoverySeverity::Warning,
                    vec![source_key.clone()],
                    vec![conversation_id],
                    vec![source_sha256.clone()],
                    vec![json!({
                        "hostCreatedAt": value.timestamp_millis(),
                        "chatCreatedAt": chat_created.map(|at| at.timestamp_millis())
                    })],
                    source_recovery_provenance(host.as_ref(), chat.as_ref()),
                ));
            }
            (Some(value), Some(CreatedAtSource::HostMetadata))
        } else if let Some(value) = chat_created {
            (Some(value), Some(CreatedAtSource::ChatPayload))
        } else {
            recovery.push(RecoveryItemV1::new(
                RecoveryKind::InvalidCreatedAt,
                RecoverySeverity::Blocking,
                vec![source_key.clone()],
                vec![conversation_id],
                vec![source_sha256.clone()],
                vec![json!({
                    "hostCreatedAt": host.as_ref().and_then(HostSource::created_at),
                    "chatCreatedAt": chat.as_ref().and_then(ChatSource::created_at)
                })],
                source_recovery_provenance(host.as_ref(), chat.as_ref()),
            ));
            (None, None)
        };
        let map = MigrationMapEntryV1 {
            source_key,
            legacy_storage_key,
            legacy_agent_session_id: host
                .as_ref()
                .and_then(HostSource::session_id)
                .or_else(|| chat.as_ref().and_then(ChatSource::session_id)),
            conversation_id,
            identity_decision,
            created_at_source,
            source_record_sha256: source_sha256,
        };

        let mut item_recovery = Vec::new();
        let cwd = host
            .as_ref()
            .map(HostSource::cwd)
            .filter(|value| !value.is_empty())
            .or_else(|| chat.as_ref().and_then(ChatSource::cwd))
            .unwrap_or_default();
        if cwd.is_empty() {
            item_recovery.push(RecoveryItemV1::new(
                RecoveryKind::CorruptSource,
                RecoverySeverity::Blocking,
                vec![map.source_key.clone()],
                vec![conversation_id],
                source_recovery_provenance(host.as_ref(), chat.as_ref())
                    .iter()
                    .map(|value| value.sha256.clone())
                    .collect(),
                vec![json!({"missingRequiredField":"cwd"})],
                source_recovery_provenance(host.as_ref(), chat.as_ref()),
            ));
        }
        let project_id = host
            .as_ref()
            .and_then(HostSource::project_id)
            .or_else(|| chat.as_ref().and_then(ChatSource::project_id));
        let (worktree_path, worktree_branch, worktree_recovery) = choose_worktree(
            conversation_id,
            &map.source_key,
            host.as_ref(),
            chat.as_ref(),
            &cwd,
            project_worktrees,
        );
        if let Some(item) = worktree_recovery {
            item_recovery.push(item);
        }
        let mut provenance = source_provenance(host.as_ref(), chat.as_ref());
        provenance.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let (events, event_materialization_failed) =
            match load_planned_events(host.as_ref(), chat.as_ref(), created_at) {
                Ok(events) => (events, false),
                Err(_) => {
                    item_recovery.push(RecoveryItemV1::new(
                        RecoveryKind::CorruptSource,
                        RecoverySeverity::Blocking,
                        vec![map.source_key.clone()],
                        vec![conversation_id],
                        source_recovery_provenance(host.as_ref(), chat.as_ref())
                            .iter()
                            .map(|value| value.sha256.clone())
                            .collect(),
                        vec![json!({"reasonCode":"eventMaterializationFailed"})],
                        source_recovery_provenance(host.as_ref(), chat.as_ref()),
                    ));
                    (Vec::new(), true)
                }
            };
        let blocking = created_at.is_none() || cwd.is_empty() || event_materialization_failed;
        map_entries.push(map.clone());
        planned.push(PlannedConversation {
            map,
            host,
            created_at,
            cwd,
            project_id,
            worktree_path,
            worktree_branch,
            provenance,
            events,
            recovery: item_recovery,
            blocking,
        });
    }
    map_entries.sort_by(|left, right| left.source_key.cmp(&right.source_key));
    let map = MigrationMapV1 {
        schema_version: MIGRATION_MAP_SCHEMA_VERSION,
        operation_id: inventory.operation_id,
        entries: map_entries,
    };
    Ok((planned, map, recovery))
}

fn choose_worktree(
    conversation_id: ConversationId,
    source_key: &str,
    host: Option<&HostSource>,
    chat: Option<&ChatSource>,
    cwd: &str,
    registry: &[ProjectWorktreeRecord],
) -> (Option<String>, Option<String>, Option<RecoveryItemV1>) {
    let host_pair = host.and_then(|value| value.worktree_path().zip(value.worktree_branch()));
    let chat_pair = chat.and_then(|value| value.worktree_path().zip(value.worktree_branch()));
    if host_pair.is_some() && chat_pair.is_some() && host_pair != chat_pair {
        let item = RecoveryItemV1::new(
            RecoveryKind::ConflictingWorktreeProvenance,
            RecoverySeverity::Warning,
            vec![source_key.to_string()],
            vec![conversation_id],
            source_recovery_provenance(host, chat)
                .iter()
                .map(|value| value.sha256.clone())
                .collect(),
            vec![
                json!({"source":"host","worktreePath":host_pair.as_ref().map(|v| &v.0),"worktreeBranch":host_pair.as_ref().map(|v| &v.1)}),
                json!({"source":"chat","worktreePath":chat_pair.as_ref().map(|v| &v.0),"worktreeBranch":chat_pair.as_ref().map(|v| &v.1)}),
            ],
            source_recovery_provenance(host, chat),
        );
        return (None, None, Some(item));
    }
    if let Some((path, branch)) = host_pair.or(chat_pair) {
        return (Some(path), Some(branch), None);
    }
    let matches = registry
        .iter()
        .filter(|record| record.cwd == cwd)
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return (
            Some(matches[0].worktree_path.clone()),
            Some(matches[0].worktree_branch.clone()),
            None,
        );
    }
    (None, None, None)
}

fn load_planned_events(
    host: Option<&HostSource>,
    chat: Option<&ChatSource>,
    created_at: Option<DateTime<Utc>>,
) -> Result<Vec<PlannedEvent>> {
    let fallback = created_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    if let Some(host) = host {
        let mut records = Vec::<LegacyEvent>::new();
        if let Some(file) = &host.messages {
            records.extend(read_legacy_jsonl(file)?);
        }
        if let Some(file) = &host.tools {
            records.extend(read_legacy_jsonl(file)?);
        }
        records.sort_by_key(|record| record.seq);
        let mut previous = 0;
        let mut events = Vec::new();
        for record in records {
            if record.seq == 0 || record.seq <= previous {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationVerificationFailed,
                    "read_legacy_events",
                    "legacy merged event sequence is invalid",
                ));
            }
            previous = record.seq;
            if let Some(type_) = map_event_type(&record.type_) {
                events.push(PlannedEvent {
                    recorded_at_utc: valid_millis(record.recorded_at).unwrap_or(fallback),
                    type_,
                    payload: record.payload,
                });
            }
        }
        return Ok(events);
    }
    let Some(chat) = chat else {
        return Ok(Vec::new());
    };
    let messages = chat
        .value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "materialize_chat_events",
                "chat payload messages field is not an array",
            )
        })?;
    let mut events = Vec::new();
    let mut open_turn: Option<(String, DateTime<Utc>, bool)> = None;
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let at = message
            .get("timestamp")
            .and_then(Value::as_u64)
            .and_then(valid_millis)
            .unwrap_or(fallback);
        if role == "user" {
            if let Some((turn_id, completed_at, responded)) = open_turn.take() {
                if responded {
                    events.push(PlannedEvent {
                        recorded_at_utc: completed_at,
                        type_: ConversationEventType::PromptComplete,
                        payload: json!({"turnId":turn_id}),
                    });
                }
            }
            let turn_id = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .strip_prefix("turn:")
                .unwrap_or_else(|| message.get("id").and_then(Value::as_str).unwrap_or(""))
                .to_string();
            events.push(PlannedEvent {
                recorded_at_utc: at,
                type_: ConversationEventType::UserPrompt,
                payload: json!({
                    "turnId":turn_id,
                    "content":message.get("blocks").cloned().unwrap_or_else(|| json!([]))
                }),
            });
            open_turn = Some((turn_id, at, false));
        } else if matches!(role, "agent" | "thought") {
            for block in message
                .get("blocks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                events.push(PlannedEvent {
                    recorded_at_utc: at,
                    type_: ConversationEventType::MessageChunk,
                    payload: json!({"role":role,"content":block}),
                });
            }
            if let Some((_, last_at, responded)) = open_turn.as_mut() {
                *last_at = at;
                *responded = true;
            }
        }
    }
    if let Some((turn_id, completed_at, responded)) = open_turn {
        if responded {
            events.push(PlannedEvent {
                recorded_at_utc: completed_at,
                type_: ConversationEventType::PromptComplete,
                payload: json!({"turnId":turn_id}),
            });
        }
    }
    Ok(events)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyEvent {
    seq: u64,
    #[serde(rename = "type")]
    type_: String,
    recorded_at: u64,
    payload: Value,
}

fn read_legacy_jsonl(file: &SourceFile) -> Result<Vec<LegacyEvent>> {
    let bytes = fs::read(file.path())
        .map_err(|error| io_error("read_legacy_jsonl", &file.path(), error))?;
    let mut records = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: LegacyEvent = serde_json::from_slice(line).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "read_legacy_jsonl",
                format!(
                    "corrupt legacy JSONL source {}: {error}",
                    file.provenance_path()
                ),
            )
        })?;
        records.push(record);
    }
    Ok(records)
}

fn map_event_type(value: &str) -> Option<ConversationEventType> {
    match value {
        "user_prompt" => Some(ConversationEventType::UserPrompt),
        "message_chunk" => Some(ConversationEventType::MessageChunk),
        "session_info_update" => Some(ConversationEventType::SessionInfoUpdate),
        "local_title_generated" => Some(ConversationEventType::LocalTitleGenerated),
        "prompt_complete" => Some(ConversationEventType::PromptComplete),
        "tool_call" => Some(ConversationEventType::ToolCall),
        "tool_call_update" => Some(ConversationEventType::ToolCallUpdate),
        _ => None,
    }
}

fn plan_manifest_recovery(
    manifests: &[SourceFile],
    planned: &[PlannedConversation],
) -> Vec<RecoveryItemV1> {
    let by_legacy_session = planned
        .iter()
        .filter_map(|plan| {
            plan.map
                .legacy_agent_session_id
                .as_ref()
                .map(|id| (id.clone(), plan.map.conversation_id))
        })
        .collect::<HashMap<_, _>>();
    let mut recovery = Vec::new();
    for manifest in manifests {
        let value = match read_json(&manifest.path()) {
            Ok(value) => value,
            Err(_) => {
                recovery.push(corrupt_source_item(manifest));
                continue;
            }
        };
        let mut referenced = Vec::new();
        collect_session_references(&value, None, &mut referenced);
        let mut conversation_ids = referenced
            .iter()
            .filter_map(|session_id| by_legacy_session.get(session_id).copied())
            .collect::<Vec<_>>();
        conversation_ids.sort_by_key(ToString::to_string);
        conversation_ids.dedup();
        if conversation_ids.len() > 1 {
            recovery.push(RecoveryItemV1::new(
                RecoveryKind::AmbiguousWorkspaceManifest,
                RecoverySeverity::Warning,
                vec![manifest.provenance_path()],
                conversation_ids,
                vec![manifest.sha256.clone()],
                vec![json!({"referencedSessionIds":referenced})],
                vec![manifest.recovery_provenance()],
            ));
        }
    }
    recovery
}

fn collect_session_references(value: &Value, key: Option<&str>, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (child_key, child) in object {
                collect_session_references(child, Some(child_key), output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_session_references(child, key, output);
            }
        }
        Value::String(value)
            if key.is_some_and(|key| {
                let lower = key.to_ascii_lowercase();
                lower.contains("session") && lower.contains("id")
            }) =>
        {
            output.push(value.clone());
            output.sort();
            output.dedup();
        }
        _ => {}
    }
}

fn source_files(root: &LegacyInventoryRootV1, root_index: usize) -> Vec<SourceFile> {
    root.files
        .iter()
        .map(|file| SourceFile {
            kind: root.source_kind,
            root_index,
            root: PathBuf::from(&root.canonical_path),
            relative_path: file.relative_path.clone(),
            sha256: file.sha256.clone(),
        })
        .collect()
}

fn source_provenance(
    host: Option<&HostSource>,
    chat: Option<&ChatSource>,
) -> Vec<ConversationProvenanceSourceV1> {
    let mut values = host.map(HostSource::provenance).unwrap_or_default();
    values.extend(chat.map(|value| value.payload.provenance()));
    values
}

fn source_recovery_provenance(
    host: Option<&HostSource>,
    chat: Option<&ChatSource>,
) -> Vec<RecoveryProvenanceV1> {
    let mut values = host
        .map(|value| {
            value
                .provenance()
                .into_iter()
                .map(|source| RecoveryProvenanceV1 {
                    source_kind: source.source_kind,
                    relative_path: source.relative_path,
                    sha256: source.sha256,
                    preserved_read_only: true,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    values.extend(chat.map(|value| value.payload.recovery_provenance()));
    values
}

fn corrupt_source_item(source: &SourceFile) -> RecoveryItemV1 {
    RecoveryItemV1::new(
        RecoveryKind::CorruptSource,
        RecoverySeverity::Blocking,
        vec![source.provenance_path()],
        Vec::new(),
        vec![source.sha256.clone()],
        Vec::new(),
        vec![source.recovery_provenance()],
    )
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).map_err(|error| io_error("read_legacy_json", path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "read_legacy_json",
            error.to_string(),
        )
    })
}

fn decode_chat_payload(value: Value) -> Result<Value> {
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "decode_chat_payload",
            "chat payload envelope schemaVersion is invalid",
        ));
    }
    value.get("payload").cloned().ok_or_else(|| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "decode_chat_payload",
            "chat payload envelope is missing payload",
        )
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn valid_millis(value: u64) -> Option<DateTime<Utc>> {
    if value == 0 || value > i64::MAX as u64 {
        return None;
    }
    Utc.timestamp_millis_opt(value as i64).single()
}

fn deterministic_binding_id(source_sha256: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"termul-legacy-binding\0");
    hasher.update(source_sha256.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn persist_migration_map(operation_dir: &Path, map: &MigrationMapV1) -> Result<()> {
    if let Some(existing) = load_migration_map(operation_dir)? {
        if existing != *map {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationIdempotencyConflict,
                "persist_migration_map",
                "durable migration map differs from the current deterministic plan",
            ));
        }
        return Ok(());
    }
    persist_json(&operation_dir.join(MIGRATION_MAP_FILE), map)
}

fn persist_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let durable_fs = DurableFileSystem::new();
    if let Some(parent) = path.parent() {
        durable_fs
            .create_dir_durable(parent, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "persist_migration_artifact",
                    error.to_string(),
                )
            })?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "persist_migration_artifact",
            error.to_string(),
        )
    })?;
    bytes.push(b'\n');
    durable_fs.replace_bytes(path, &bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationDurabilityFailed,
            "persist_migration_artifact",
            error.to_string(),
        )
    })?;
    Ok(())
}

fn receipt_path(operation_dir: &Path, conversation_id: ConversationId) -> PathBuf {
    operation_dir
        .join(STAGE_RECEIPTS_DIR)
        .join(format!("{conversation_id}.json"))
}

pub(crate) fn stage_output_digest(
    record: &ConversationRecordV2,
    events: &[ConversationEventRecordV2],
    provenance: &ConversationProvenanceFileV1,
) -> Result<String> {
    #[derive(Serialize)]
    struct Output<'a> {
        record: &'a ConversationRecordV2,
        events: &'a [ConversationEventRecordV2],
        provenance: &'a ConversationProvenanceFileV1,
    }
    sha256_json(&Output {
        record,
        events,
        provenance,
    })
}

pub(crate) fn stage_digest(
    operation_id: Uuid,
    inventory_sha256: &str,
    migration_map_sha256: &str,
    recovery_queue_sha256: &str,
    receipts: &[StageReceiptV1],
) -> Result<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DigestInput<'a> {
        schema_version: u32,
        operation_id: Uuid,
        inventory_sha256: &'a str,
        migration_map_sha256: &'a str,
        recovery_queue_sha256: &'a str,
        receipts: &'a [StageReceiptV1],
    }
    sha256_json(&DigestInput {
        schema_version: 1,
        operation_id,
        inventory_sha256,
        migration_map_sha256,
        recovery_queue_sha256,
        receipts,
    })
}

fn sha256_json(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "hash_migration_json",
            error.to_string(),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn repository_error(error: crate::conversation::repository::RepositoryError) -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        "stage_repository",
        error.to_string(),
    )
}

fn io_error(operation: &'static str, path: &Path, error: std::io::Error) -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        operation,
        format!(
            "migration source/artifact '{}' failed: {error}",
            path.display()
        ),
    )
}

fn sanitized_source(source_key: &str) -> &str {
    source_key.rsplit(':').next().unwrap_or("source")
}

fn digest_prefix(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::migration::inventory::{
        inventory_legacy_roots, LegacyRootConfiguration,
    };

    fn host_metadata(storage_key: &str, session_id: &str, worktree: (&str, &str)) -> Value {
        json!({
            "schemaVersion":1,
            "storageKey":storage_key,
            "sessionId":session_id,
            "stableAgentNamespace":"config:test",
            "runtimeAgentId":"runtime-test",
            "projectId":"project-1",
            "cwd":"/project",
            "createdAt":1_700_000_000_000_u64,
            "worktreePath":worktree.0,
            "worktreeBranch":worktree.1
        })
    }

    fn write_session(root: &Path, directory: &str, metadata: Value) {
        let path = root.join(directory);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("metadata.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let session_id = metadata["sessionId"].as_str().unwrap();
        fs::write(
            path.join("messages.jsonl"),
            format!(
                "{}\n",
                json!({"schemaVersion":1,"sessionId":session_id,"seq":1,"type":"user_prompt","recordedAt":1_700_000_001_000_u64,"payload":{"turnId":"one"}})
            ),
        )
        .unwrap();
        fs::write(path.join("tool-calls.jsonl"), b"").unwrap();
    }

    #[test]
    fn legacy_frontier_matches_native_append_for_identical_events() {
        use crate::conversation::event_log::{apply_event, ConversationFrontier};

        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let recorded_at_utc = Utc.timestamp_millis_opt(1_700_000_001_000).unwrap();
        let fixtures = [
            (
                "user_prompt",
                ConversationEventType::UserPrompt,
                json!({"content":[{"type":"text","text":"Derived"}]}),
            ),
            (
                "session_info_update",
                ConversationEventType::SessionInfoUpdate,
                json!({"title":"Agent"}),
            ),
            (
                "local_title_generated",
                ConversationEventType::LocalTitleGenerated,
                json!({"title":"Background"}),
            ),
            (
                "tool_call",
                ConversationEventType::ToolCall,
                json!({"toolCall":{"id":"one"}}),
            ),
            (
                "tool_call_update",
                ConversationEventType::ToolCallUpdate,
                json!({"update":{"id":"one"}}),
            ),
        ];
        let mut legacy = ConversationFrontier::default();
        let mut native = ConversationFrontier::default();
        for (index, (legacy_type, native_type, payload)) in fixtures.into_iter().enumerate() {
            let seq = index as u64 + 1;
            apply_event(
                &mut legacy,
                &ConversationEventRecordV2::new(
                    conversation_id,
                    seq,
                    recorded_at_utc,
                    map_event_type(legacy_type).unwrap(),
                    payload.clone(),
                ),
            )
            .unwrap();
            apply_event(
                &mut native,
                &ConversationEventRecordV2::new(
                    conversation_id,
                    seq,
                    recorded_at_utc,
                    native_type,
                    payload,
                ),
            )
            .unwrap();
        }
        assert_eq!(legacy, native);
    }

    #[tokio::test]
    async fn unique_uuid_is_reused_invalid_and_collision_are_durably_allocated() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().canonicalize().unwrap().join("host");
        let sessions = host.join("acp-sessions");
        fs::create_dir_all(&sessions).unwrap();
        let unique = "018F7A1C-1B4D-7C8A-9F01-0123456789AB";
        write_session(
            &sessions,
            "11111111-1111-4111-8111-111111111111",
            host_metadata(unique, "provider/session:abc", ("/wt/one", "feature/one")),
        );
        write_session(
            &sessions,
            "22222222-2222-4222-8222-222222222222",
            host_metadata("invalid", "opaque-two", ("/wt/two", "feature/two")),
        );
        write_session(
            &sessions,
            "33333333-3333-4333-8333-333333333333",
            host_metadata(
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "opaque-three",
                ("/wt/three", "feature/three"),
            ),
        );
        write_session(
            &sessions,
            "44444444-4444-4444-8444-444444444444",
            host_metadata(
                "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
                "opaque-four",
                ("/wt/four", "feature/four"),
            ),
        );
        let operation_dir = host.join("conversation-migrations/op");
        let inventory = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host.clone(),
                ..Default::default()
            },
            Uuid::new_v4(),
            Utc::now(),
            &operation_dir,
        )
        .unwrap();
        let config = LegacyStageConfiguration {
            host_state_root: host.clone(),
            operation_dir: operation_dir.clone(),
            project_worktrees: Vec::new(),
        };
        let staged = stage_legacy_conversations(&config, &inventory)
            .await
            .unwrap();
        assert_eq!(staged.receipts.len(), 4);
        let opaque_receipt = staged
            .receipts
            .iter()
            .find(|receipt| {
                receipt.opaque_agent_session_id.as_deref() == Some("provider/session:abc")
            })
            .unwrap();
        assert_eq!(opaque_receipt.worktree_path.as_deref(), Some("/wt/one"));
        assert_eq!(
            opaque_receipt.worktree_branch.as_deref(),
            Some("feature/one")
        );
        let map = load_migration_map(&operation_dir).unwrap().unwrap();
        let unique_entry = map
            .entries
            .iter()
            .find(|entry| entry.legacy_agent_session_id.as_deref() == Some("provider/session:abc"))
            .unwrap();
        assert_eq!(
            unique_entry.conversation_id.to_string(),
            "018f7a1c-1b4d-7c8a-9f01-0123456789ab"
        );
        assert_eq!(
            unique_entry.identity_decision,
            IdentityDecision::ReusedUniqueStorageUuid
        );
        assert!(map
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.identity_decision,
                    IdentityDecision::AllocatedInvalidUuid
                        | IdentityDecision::AllocatedCollisionUuid
                )
            })
            .all(|entry| entry.conversation_id.to_string()
                != entry
                    .legacy_storage_key
                    .clone()
                    .unwrap_or_default()
                    .to_ascii_lowercase()));
        assert!(!host
            .join("conversations/v2")
            .join("provider/session:abc")
            .exists());
        let first_map = fs::read(operation_dir.join(MIGRATION_MAP_FILE)).unwrap();
        stage_legacy_conversations(&config, &inventory)
            .await
            .unwrap();
        assert_eq!(
            fs::read(operation_dir.join(MIGRATION_MAP_FILE)).unwrap(),
            first_map
        );
    }

    #[tokio::test]
    async fn conflicting_worktree_and_ambiguous_manifest_emit_recovery_without_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().canonicalize().unwrap().join("host");
        let sessions = host.join("acp-sessions");
        let chat = host.join("acp-chat-history/payloads");
        let manifests = host.join("workspace-manifests");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&chat).unwrap();
        fs::create_dir_all(&manifests).unwrap();
        let id1 = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
        let id2 = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";
        write_session(
            &sessions,
            id1,
            host_metadata(id1, "session-one", ("/host-wt", "host-branch")),
        );
        write_session(
            &sessions,
            id2,
            host_metadata(id2, "session-two", ("/second", "second")),
        );
        fs::write(
            chat.join("aa.json"),
            serde_json::to_vec(&json!({"schemaVersion":1,"payload":{"metadata":{"id":"session-one","cwd":"/project","projectId":"project-1","createdAt":1_700_000_000_000_u64,"worktreePath":"/chat-wt","worktreeBranch":"chat-branch"},"messages":[]}})).unwrap(),
        ).unwrap();
        fs::write(
            manifests.join("shared.json"),
            serde_json::to_vec(&json!({"schemaVersion":1,"manifest":{"focusedSessionId":"session-one","sessionIds":["session-one","session-two"],"terminals":[{"terminalId":"do-not-own"}],"editors":[{"editorId":"do-not-own"}]}})).unwrap(),
        ).unwrap();
        let operation_dir = host.join("conversation-migrations/op");
        let inventory = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host.clone(),
                ..Default::default()
            },
            Uuid::new_v4(),
            Utc::now(),
            &operation_dir,
        )
        .unwrap();
        let stage_configuration = LegacyStageConfiguration {
            host_state_root: host.clone(),
            operation_dir: operation_dir.clone(),
            project_worktrees: Vec::new(),
        };
        let staged = stage_legacy_conversations(&stage_configuration, &inventory)
            .await
            .unwrap();
        let queue_path = operation_dir.join(super::super::recovery::RECOVERY_ITEMS_FILE);
        let first_queue_bytes = fs::read(&queue_path).unwrap();
        let queue: RecoveryQueueV1 = serde_json::from_slice(&first_queue_bytes).unwrap();
        let worktree = queue
            .items
            .iter()
            .find(|item| item.kind == RecoveryKind::ConflictingWorktreeProvenance)
            .unwrap();
        assert_eq!(worktree.candidate_facts.len(), 2);
        let conflicting_receipt = staged
            .receipts
            .iter()
            .find(|receipt| receipt.opaque_agent_session_id.as_deref() == Some("session-one"))
            .unwrap();
        assert_eq!(conflicting_receipt.worktree_path, None);
        assert_eq!(conflicting_receipt.worktree_branch, None);
        let manifest = queue
            .items
            .iter()
            .find(|item| item.kind == RecoveryKind::AmbiguousWorkspaceManifest)
            .unwrap();
        assert_eq!(manifest.source_sha256.len(), 1);
        assert_eq!(manifest.conversation_ids.len(), 2);
        assert!(!host.join("conversations/v2/workspace.json").exists());
        assert!(!walk_names(&host.join("conversations/v2"))
            .iter()
            .any(|name| name == "workspace.json"));
        stage_legacy_conversations(&stage_configuration, &inventory)
            .await
            .unwrap();
        assert_eq!(fs::read(queue_path).unwrap(), first_queue_bytes);
    }

    #[tokio::test]
    async fn corrupt_event_source_is_preserved_and_queued_without_partial_conversation() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().canonicalize().unwrap().join("host");
        let sessions = host.join("acp-sessions");
        let storage_key = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
        write_session(
            &sessions,
            storage_key,
            host_metadata(storage_key, "provider/session:corrupt", ("/wt", "feature")),
        );
        let corrupt_path = sessions.join(storage_key).join("messages.jsonl");
        let corrupt_bytes = b"{not valid jsonl\n";
        fs::write(&corrupt_path, corrupt_bytes).unwrap();
        let operation_dir = host.join("conversation-migrations/op");
        let inventory = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host.clone(),
                ..Default::default()
            },
            Uuid::new_v4(),
            Utc::now(),
            &operation_dir,
        )
        .unwrap();
        let staged = stage_legacy_conversations(
            &LegacyStageConfiguration {
                host_state_root: host.clone(),
                operation_dir: operation_dir.clone(),
                project_worktrees: Vec::new(),
            },
            &inventory,
        )
        .await
        .unwrap();
        assert!(staged.receipts.is_empty());
        assert_eq!(fs::read(&corrupt_path).unwrap(), corrupt_bytes);
        let queue: RecoveryQueueV1 = serde_json::from_slice(
            &fs::read(operation_dir.join(super::super::recovery::RECOVERY_ITEMS_FILE)).unwrap(),
        )
        .unwrap();
        assert!(queue.items.iter().any(|item| {
            item.kind == RecoveryKind::CorruptSource
                && item.severity == RecoverySeverity::Blocking
                && item.status == super::super::recovery::RecoveryStatus::Unresolved
        }));
        let (repository, _) = ConversationRepository::open(host.join("conversations/v2")).unwrap();
        assert!(repository.list_conversations().is_empty());
    }

    fn walk_names(root: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        stack.push(entry.path());
                    } else {
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
        }
        names
    }
}
