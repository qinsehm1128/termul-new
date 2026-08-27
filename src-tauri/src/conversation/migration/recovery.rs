//! Stable recovery queue and canonical cross-language recovery action state machine.
//!
//! Recovery actions can change only status, monotonic recovery revision, association decisions,
//! and (for one action) an absent workspace revision. Source paths, checksums, candidate facts,
//! and provenance are immutable and are returned from every receipt for auditability.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{MigrationError, MigrationErrorCode, Result};
use crate::conversation::contracts::ConversationId;
use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};

pub const RECOVERY_QUEUE_SCHEMA_VERSION: u32 = 1;
pub const RECOVERY_ITEMS_FILE: &str = "recovery-items-v1.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    AmbiguousWorkspaceManifest,
    IdentifierCollision,
    InvalidCreatedAt,
    CorruptSource,
    ConflictingWorktreeProvenance,
    ConflictingSessionMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoverySeverity {
    Warning,
    Blocking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryStatus {
    Unresolved,
    ResolvedAssociated,
    ResolvedStartedEmpty,
    DismissedPreserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryActionName {
    Inspect,
    AssociateConversation,
    StartEmptyWorkspace,
    DismissPreservedSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryAuthorizationClass {
    Read,
    Mutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DismissReasonCode {
    NotApplicable,
    DeferLegacyProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryProvenanceV1 {
    pub source_kind: String,
    pub relative_path: String,
    pub sha256: String,
    pub preserved_read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryItemV1 {
    pub recovery_id: String,
    pub kind: RecoveryKind,
    pub severity: RecoverySeverity,
    pub source_paths: Vec<String>,
    pub conversation_ids: Vec<ConversationId>,
    pub source_sha256: Vec<String>,
    pub candidate_facts: Vec<Value>,
    pub provenance: Vec<RecoveryProvenanceV1>,
    pub status: RecoveryStatus,
    pub suggested_actions: Vec<RecoveryActionName>,
    pub revision: u64,
    #[serde(default)]
    pub association_decisions: Vec<ConversationId>,
}

impl RecoveryItemV1 {
    #[must_use]
    pub fn new(
        kind: RecoveryKind,
        severity: RecoverySeverity,
        mut source_paths: Vec<String>,
        mut conversation_ids: Vec<ConversationId>,
        mut source_sha256: Vec<String>,
        mut candidate_facts: Vec<Value>,
        mut provenance: Vec<RecoveryProvenanceV1>,
    ) -> Self {
        source_paths.sort();
        source_paths.dedup();
        conversation_ids.sort_by_key(ToString::to_string);
        conversation_ids.dedup();
        source_sha256.sort();
        source_sha256.dedup();
        candidate_facts.sort_by_key(canonical_json);
        candidate_facts.dedup();
        provenance.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.sha256.cmp(&right.sha256))
        });
        provenance.dedup();
        let recovery_id = stable_recovery_id(kind, &source_paths, &conversation_ids);
        Self {
            recovery_id,
            kind,
            severity,
            source_paths,
            conversation_ids,
            source_sha256,
            candidate_facts,
            provenance,
            status: RecoveryStatus::Unresolved,
            suggested_actions: vec![
                RecoveryActionName::Inspect,
                RecoveryActionName::AssociateConversation,
                RecoveryActionName::StartEmptyWorkspace,
                RecoveryActionName::DismissPreservedSource,
            ],
            revision: 1,
            association_decisions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryQueueV1 {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub items: Vec<RecoveryItemV1>,
    #[serde(default)]
    pub idempotency_receipts: BTreeMap<Uuid, RecoveryActionReceiptV1>,
}

impl RecoveryQueueV1 {
    #[must_use]
    pub fn new(operation_id: Uuid, mut items: Vec<RecoveryItemV1>) -> Self {
        items.sort_by(|left, right| left.recovery_id.cmp(&right.recovery_id));
        items.dedup_by(|left, right| left.recovery_id == right.recovery_id);
        Self {
            schema_version: RECOVERY_QUEUE_SCHEMA_VERSION,
            operation_id,
            items,
            idempotency_receipts: BTreeMap::new(),
        }
    }

    pub fn resolve(
        &mut self,
        request: ResolveRecoveryItemRequest,
        workspaces: &mut HashMap<ConversationId, u64>,
    ) -> std::result::Result<RecoveryActionResult, RecoveryActionError> {
        request.validate()?;
        let item_index = self
            .items
            .iter()
            .position(|item| item.recovery_id == request.recovery_id)
            .ok_or_else(|| {
                RecoveryActionError::new(
                    RecoveryActionErrorCode::RecoveryNotFound,
                    "recovery item was not found",
                )
            })?;
        validate_candidate_scope(&self.items[item_index], &request.action)?;

        let request_digest = request.canonical_digest();
        if let Some(key) = request.idempotency_key {
            if let Some(receipt) = self.idempotency_receipts.get(&key) {
                if receipt.request_sha256 == request_digest {
                    return Ok(receipt.result.clone());
                }
                return Err(RecoveryActionError::new(
                    RecoveryActionErrorCode::MigrationIdempotencyConflict,
                    "idempotency key was reused with a different canonical request",
                ));
            }
        }
        if self.items[item_index].revision != request.expected_revision {
            return Err(RecoveryActionError::new(
                RecoveryActionErrorCode::ConversationConflict,
                "expectedRevision does not match the RecoveryItem revision",
            ));
        }

        let before_sources = immutable_source_view(&self.items[item_index]);
        let (authorization, workspace_revision, workspace_changed) = match &request.action {
            RecoveryAction::Inspect(_) => (RecoveryAuthorizationClass::Read, None, false),
            RecoveryAction::AssociateConversation(payload) => {
                let item = &mut self.items[item_index];
                item.association_decisions.push(payload.conversation_id);
                item.status = RecoveryStatus::ResolvedAssociated;
                item.revision += 1;
                (RecoveryAuthorizationClass::Mutation, None, false)
            }
            RecoveryAction::StartEmptyWorkspace(payload) => {
                if payload.expected_workspace_revision.is_some()
                    || workspaces.contains_key(&payload.conversation_id)
                {
                    return Err(RecoveryActionError::new(
                        RecoveryActionErrorCode::ConversationConflict,
                        "empty workspace CAS requires an absent target and null expectedWorkspaceRevision",
                    ));
                }
                workspaces.insert(payload.conversation_id, 1);
                let item = &mut self.items[item_index];
                item.status = RecoveryStatus::ResolvedStartedEmpty;
                item.revision += 1;
                (RecoveryAuthorizationClass::Mutation, Some(1), true)
            }
            RecoveryAction::DismissPreservedSource(_) => {
                let item = &mut self.items[item_index];
                item.status = RecoveryStatus::DismissedPreserved;
                item.revision += 1;
                (RecoveryAuthorizationClass::Mutation, None, false)
            }
        };
        let item = &self.items[item_index];
        debug_assert_eq!(before_sources, immutable_source_view(item));
        let result = RecoveryActionResult {
            recovery_id: item.recovery_id.clone(),
            action: request.action.name(),
            authorization,
            status: item.status,
            recovery_revision: item.revision,
            workspace_revision,
            workspace_changed,
            source_paths: item.source_paths.clone(),
            source_sha256: item.source_sha256.clone(),
            candidate_facts: item.candidate_facts.clone(),
            provenance: item.provenance.clone(),
        };
        if let Some(key) = request.idempotency_key {
            self.idempotency_receipts.insert(
                key,
                RecoveryActionReceiptV1 {
                    request_sha256: request_digest,
                    result: result.clone(),
                },
            );
        }
        Ok(result)
    }

    pub fn persist(&self, operation_dir: &Path) -> Result<()> {
        if self.schema_version != RECOVERY_QUEUE_SCHEMA_VERSION {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "persist_recovery_queue",
                "unsupported recovery queue schemaVersion",
            ));
        }
        let durable_fs = DurableFileSystem::new();
        durable_fs
            .create_dir_durable(operation_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "persist_recovery_queue",
                    error.to_string(),
                )
            })?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "persist_recovery_queue",
                error.to_string(),
            )
        })?;
        bytes.push(b'\n');
        durable_fs
            .replace_bytes(&operation_dir.join(RECOVERY_ITEMS_FILE), &bytes)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "persist_recovery_queue",
                    error.to_string(),
                )
            })?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectPayload {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssociateConversationPayload {
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartEmptyWorkspacePayload {
    pub conversation_id: ConversationId,
    #[serde(deserialize_with = "deserialize_required_nullable_revision")]
    pub expected_workspace_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DismissPreservedSourcePayload {
    pub reason_code: DismissReasonCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload", rename_all = "camelCase")]
pub enum RecoveryAction {
    Inspect(InspectPayload),
    AssociateConversation(AssociateConversationPayload),
    StartEmptyWorkspace(StartEmptyWorkspacePayload),
    DismissPreservedSource(DismissPreservedSourcePayload),
}

impl RecoveryAction {
    #[must_use]
    pub const fn name(&self) -> RecoveryActionName {
        match self {
            Self::Inspect(_) => RecoveryActionName::Inspect,
            Self::AssociateConversation(_) => RecoveryActionName::AssociateConversation,
            Self::StartEmptyWorkspace(_) => RecoveryActionName::StartEmptyWorkspace,
            Self::DismissPreservedSource(_) => RecoveryActionName::DismissPreservedSource,
        }
    }

    #[must_use]
    pub const fn authorization(&self) -> RecoveryAuthorizationClass {
        match self {
            Self::Inspect(_) => RecoveryAuthorizationClass::Read,
            _ => RecoveryAuthorizationClass::Mutation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveRecoveryItemRequest {
    pub recovery_id: String,
    pub expected_revision: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_canonical_uuid"
    )]
    pub idempotency_key: Option<Uuid>,
    #[serde(flatten)]
    pub action: RecoveryAction,
}

impl ResolveRecoveryItemRequest {
    fn validate(&self) -> std::result::Result<(), RecoveryActionError> {
        if self.recovery_id.trim().is_empty() || self.expected_revision == 0 {
            return Err(RecoveryActionError::new(
                RecoveryActionErrorCode::ValidationError,
                "recoveryId and a positive expectedRevision are required",
            ));
        }
        if self.action.authorization() == RecoveryAuthorizationClass::Mutation
            && self.idempotency_key.is_none()
        {
            return Err(RecoveryActionError::new(
                RecoveryActionErrorCode::ValidationError,
                "mutation recovery actions require a UUID idempotencyKey",
            ));
        }
        Ok(())
    }

    fn canonical_digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("Recovery request is serializable");
        sha256_bytes(&bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryActionResult {
    pub recovery_id: String,
    pub action: RecoveryActionName,
    pub authorization: RecoveryAuthorizationClass,
    pub status: RecoveryStatus,
    pub recovery_revision: u64,
    pub workspace_revision: Option<u64>,
    pub workspace_changed: bool,
    pub source_paths: Vec<String>,
    pub source_sha256: Vec<String>,
    pub candidate_facts: Vec<Value>,
    pub provenance: Vec<RecoveryProvenanceV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryActionReceiptV1 {
    pub request_sha256: String,
    pub result: RecoveryActionResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecoveryActionErrorCode {
    ConversationConflict,
    MigrationIdempotencyConflict,
    RecoveryNotFound,
    ValidationError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryActionError {
    pub code: RecoveryActionErrorCode,
    pub detail: String,
}

impl RecoveryActionError {
    fn new(code: RecoveryActionErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for RecoveryActionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for RecoveryActionError {}

#[must_use]
pub fn stable_recovery_id(
    kind: RecoveryKind,
    source_paths: &[String],
    conversation_ids: &[ConversationId],
) -> String {
    let mut paths = source_paths.to_vec();
    paths.sort();
    let mut ids = conversation_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    let mut hasher = Sha256::new();
    hasher.update(
        serde_json::to_value(kind)
            .expect("RecoveryKind is serializable")
            .as_str()
            .expect("RecoveryKind serializes as a string")
            .as_bytes(),
    );
    hasher.update([0]);
    hasher.update(paths.join("\0").as_bytes());
    hasher.update([0]);
    hasher.update(ids.join("\0").as_bytes());
    lower_hex(&hasher.finalize())
}

fn deserialize_required_nullable_revision<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer)
}

fn deserialize_optional_canonical_uuid<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let parsed = Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
    if parsed.to_string() != value {
        return Err(serde::de::Error::custom(
            "UUID must use canonical lowercase-hyphenated form",
        ));
    }
    Ok(Some(parsed))
}

fn validate_candidate_scope(
    item: &RecoveryItemV1,
    action: &RecoveryAction,
) -> std::result::Result<(), RecoveryActionError> {
    let target = match action {
        RecoveryAction::AssociateConversation(payload) => Some(payload.conversation_id),
        RecoveryAction::StartEmptyWorkspace(payload) => Some(payload.conversation_id),
        RecoveryAction::Inspect(_) | RecoveryAction::DismissPreservedSource(_) => None,
    };
    if let Some(target) = target {
        if !item.conversation_ids.contains(&target) {
            log::warn!(
                "[conversation-recovery] candidate rejected recovery_id={} conversation_id={} code=VALIDATION_ERROR",
                item.recovery_id,
                target
            );
            return Err(RecoveryActionError::new(
                RecoveryActionErrorCode::ValidationError,
                "recovery target is outside the RecoveryItem candidate set",
            ));
        }
    }
    Ok(())
}

fn immutable_source_view(item: &RecoveryItemV1) -> Value {
    serde_json::json!({
        "sourcePaths": item.source_paths,
        "sourceSha256": item.source_sha256,
        "candidateFacts": item.candidate_facts,
        "provenance": item.provenance,
    })
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON Value is serializable")
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    lower_hex(&hasher.finalize())
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const OTHER_ID: &str = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";
    const KEY: &str = "21aee10a-56b8-4624-a5e7-586c25dc8d1f";
    const START_KEY: &str = "d70c2b93-71bc-4df0-85a5-15bd1b7cf452";
    const DISMISS_KEY: &str = "b025313d-df5d-4254-af4f-535b47ea570f";

    fn item() -> RecoveryItemV1 {
        RecoveryItemV1::new(
            RecoveryKind::AmbiguousWorkspaceManifest,
            RecoverySeverity::Warning,
            vec!["workspace-manifests/project.json".to_string()],
            vec![ConversationId::parse(ID).unwrap()],
            vec!["a".repeat(64)],
            vec![json!({"worktreePath":"/one"})],
            vec![RecoveryProvenanceV1 {
                source_kind: "legacy_workspace_manifests".to_string(),
                relative_path: "workspace-manifests/project.json".to_string(),
                sha256: "a".repeat(64),
                preserved_read_only: true,
            }],
        )
    }

    fn request(action: RecoveryAction, expected_revision: u64) -> ResolveRecoveryItemRequest {
        ResolveRecoveryItemRequest {
            recovery_id: item().recovery_id,
            expected_revision,
            idempotency_key: match action.name() {
                RecoveryActionName::Inspect => None,
                RecoveryActionName::AssociateConversation => Some(Uuid::parse_str(KEY).unwrap()),
                RecoveryActionName::StartEmptyWorkspace => {
                    Some(Uuid::parse_str(START_KEY).unwrap())
                }
                RecoveryActionName::DismissPreservedSource => {
                    Some(Uuid::parse_str(DISMISS_KEY).unwrap())
                }
            },
            action,
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct SharedRecoveryFixture {
        request: ResolveRecoveryItemRequest,
        authorization: RecoveryAuthorizationClass,
        result: RecoveryActionResult,
    }

    fn fixture_item(recovery_id: String, revision: u64) -> RecoveryItemV1 {
        RecoveryItemV1 {
            recovery_id,
            kind: RecoveryKind::AmbiguousWorkspaceManifest,
            severity: RecoverySeverity::Warning,
            source_paths: vec!["legacy_workspace_manifests/0/project.json".to_string()],
            conversation_ids: vec![ConversationId::parse(ID).unwrap()],
            source_sha256: vec!["e".repeat(64)],
            candidate_facts: vec![json!({"candidate":"preserved"})],
            provenance: vec![RecoveryProvenanceV1 {
                source_kind: "legacy_workspace_manifests".to_string(),
                relative_path: "legacy_workspace_manifests/0/project.json".to_string(),
                sha256: "e".repeat(64),
                preserved_read_only: true,
            }],
            status: RecoveryStatus::Unresolved,
            suggested_actions: vec![
                RecoveryActionName::Inspect,
                RecoveryActionName::AssociateConversation,
                RecoveryActionName::StartEmptyWorkspace,
                RecoveryActionName::DismissPreservedSource,
            ],
            revision,
            association_decisions: Vec::new(),
        }
    }

    #[test]
    fn rust_serde_round_trips_and_executes_the_shared_canonical_fixture_set() {
        let source = include_str!("../../../../src/shared/types/conversation-recovery.types.ts");
        let start_marker = "export const RECOVERY_ACTION_FIXTURES_JSON = `";
        let end_marker = "`\n\nexport const RECOVERY_ACTION_FIXTURES";
        let start = source.find(start_marker).unwrap() + start_marker.len();
        let end = source[start..].find(end_marker).unwrap() + start;
        let fixture_json = &source[start..end];
        let fixtures: Vec<SharedRecoveryFixture> = serde_json::from_str(fixture_json).unwrap();
        assert_eq!(fixtures.len(), 4);
        for fixture in fixtures {
            let request_json = serde_json::to_value(&fixture.request).unwrap();
            let round_trip: ResolveRecoveryItemRequest =
                serde_json::from_value(request_json.clone()).unwrap();
            assert_eq!(serde_json::to_value(round_trip).unwrap(), request_json);
            assert_eq!(
                fixture.request.action.authorization(),
                fixture.authorization
            );

            let mut queue = RecoveryQueueV1::new(
                Uuid::new_v4(),
                vec![fixture_item(
                    fixture.request.recovery_id.clone(),
                    fixture.request.expected_revision,
                )],
            );
            let mut workspaces = HashMap::new();
            let result = queue
                .resolve(fixture.request, &mut workspaces)
                .expect("canonical fixture action must resolve");
            assert_eq!(result, fixture.result);
            assert_eq!(
                serde_json::from_value::<RecoveryActionResult>(
                    serde_json::to_value(&result).unwrap()
                )
                .unwrap(),
                result
            );
        }
        for invalid in [
            json!({"recovery_id":"x","expectedRevision":1,"action":"inspect","payload":{}}),
            json!({"recoveryId":"x","action":"inspect","payload":{}}),
            json!({"recoveryId":"x","expectedRevision":1,"action":"associate_conversation","payload":{"conversationId":ID},"idempotencyKey":KEY}),
            json!({"recoveryId":"x","expectedRevision":1,"action":"associateConversation","payload":{"conversationId":ID}}),
            json!({"recoveryId":"x","expectedRevision":1,"action":"inspect","payload":{},"sourcePaths":["attempted-provenance-mutation"]}),
            json!({"recoveryId":"x","expectedRevision":1,"idempotencyKey":START_KEY,"action":"startEmptyWorkspace","payload":{"conversationId":ID}}),
            json!({"recoveryId":"x","expectedRevision":1,"idempotencyKey":"21AEE10A-56B8-4624-A5E7-586C25DC8D1F","action":"dismissPreservedSource","payload":{"reasonCode":"notApplicable"}}),
            json!({"recoveryId":"x","expectedRevision":1,"idempotencyKey":null,"action":"inspect","payload":{}}),
        ] {
            if let Ok(request) = serde_json::from_value::<ResolveRecoveryItemRequest>(invalid) {
                assert!(request.validate().is_err());
            }
        }
    }

    #[test]
    fn recovery_actions_preserve_sources_and_enforce_revision_and_idempotency() {
        let source_dir = tempfile::tempdir().unwrap();
        let preserved_source = source_dir.path().join("preserved.json");
        std::fs::write(&preserved_source, b"preserved legacy bytes\n").unwrap();
        let source_bytes = std::fs::read(&preserved_source).unwrap();
        let original = item();
        let immutable = immutable_source_view(&original);
        let mut queue = RecoveryQueueV1::new(Uuid::new_v4(), vec![original]);
        let mut workspaces = HashMap::new();
        let inspect = queue
            .resolve(
                request(RecoveryAction::Inspect(InspectPayload {}), 1),
                &mut workspaces,
            )
            .unwrap();
        assert_eq!(inspect.status, RecoveryStatus::Unresolved);
        assert_eq!(inspect.recovery_revision, 1);

        let association = request(
            RecoveryAction::AssociateConversation(AssociateConversationPayload {
                conversation_id: ConversationId::parse(ID).unwrap(),
            }),
            1,
        );
        let first = queue.resolve(association.clone(), &mut workspaces).unwrap();
        assert_eq!(first.status, RecoveryStatus::ResolvedAssociated);
        assert_eq!(first.recovery_revision, 2);
        assert_eq!(immutable_source_view(&queue.items[0]), immutable);
        assert_eq!(queue.resolve(association, &mut workspaces).unwrap(), first);
        assert_eq!(queue.items[0].revision, 2);

        let stale = request(
            RecoveryAction::DismissPreservedSource(DismissPreservedSourcePayload {
                reason_code: DismissReasonCode::NotApplicable,
            }),
            1,
        );
        assert_eq!(
            queue.resolve(stale, &mut workspaces).unwrap_err().code,
            RecoveryActionErrorCode::ConversationConflict
        );
        let dismissed = queue
            .resolve(
                request(
                    RecoveryAction::DismissPreservedSource(DismissPreservedSourcePayload {
                        reason_code: DismissReasonCode::DeferLegacyProjection,
                    }),
                    2,
                ),
                &mut workspaces,
            )
            .unwrap();
        assert_eq!(dismissed.status, RecoveryStatus::DismissedPreserved);
        assert_eq!(dismissed.recovery_revision, 3);
        assert_eq!(immutable_source_view(&queue.items[0]), immutable);
        assert_eq!(std::fs::read(preserved_source).unwrap(), source_bytes);
    }

    #[test]
    fn recovery_mutation_rejects_targets_outside_candidates_without_mutation() {
        let original = item();
        let mut queue = RecoveryQueueV1::new(Uuid::new_v4(), vec![original.clone()]);
        let candidate = ConversationId::parse(ID).unwrap();
        let outside = ConversationId::parse(OTHER_ID).unwrap();

        for action in [
            RecoveryAction::AssociateConversation(AssociateConversationPayload {
                conversation_id: outside,
            }),
            RecoveryAction::StartEmptyWorkspace(StartEmptyWorkspacePayload {
                conversation_id: outside,
                expected_workspace_revision: None,
            }),
        ] {
            let mut workspaces = HashMap::from([(candidate, 7)]);
            let before_workspaces = workspaces.clone();
            let before_queue = queue.clone();
            let error = queue
                .resolve(request(action, original.revision), &mut workspaces)
                .unwrap_err();
            assert_eq!(error.code, RecoveryActionErrorCode::ValidationError);
            assert_eq!(queue, before_queue);
            assert_eq!(workspaces, before_workspaces);
        }
    }

    #[test]
    fn start_empty_workspace_is_absent_only_cas_and_never_overwrites() {
        let mut queue = RecoveryQueueV1::new(Uuid::new_v4(), vec![item()]);
        let conversation_id = ConversationId::parse(ID).unwrap();
        let mut workspaces = HashMap::new();
        let create = request(
            RecoveryAction::StartEmptyWorkspace(StartEmptyWorkspacePayload {
                conversation_id,
                expected_workspace_revision: None,
            }),
            1,
        );
        let result = queue.resolve(create.clone(), &mut workspaces).unwrap();
        assert_eq!(result.workspace_revision, Some(1));
        assert_eq!(workspaces.get(&conversation_id), Some(&1));
        assert_eq!(queue.resolve(create, &mut workspaces).unwrap(), result);

        let mut second_queue = RecoveryQueueV1::new(Uuid::new_v4(), vec![item()]);
        let mut existing = HashMap::from([(conversation_id, 7)]);
        let conflict = request(
            RecoveryAction::StartEmptyWorkspace(StartEmptyWorkspacePayload {
                conversation_id,
                expected_workspace_revision: Some(7),
            }),
            1,
        );
        assert_eq!(
            second_queue
                .resolve(conflict, &mut existing)
                .unwrap_err()
                .code,
            RecoveryActionErrorCode::ConversationConflict
        );
        assert_eq!(existing.get(&conversation_id), Some(&7));
        assert_eq!(second_queue.items[0].revision, 1);
    }
}
