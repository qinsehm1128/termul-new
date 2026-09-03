//! Transport-neutral Conversation application service.
//!
//! Bootstrap constructs exactly one instance from the canonical repository, compatibility reader,
//! SessionWorkspace service, and durable legacy identity map. Desktop Tauri commands, Axum HTTP,
//! and authenticated WebSocket adapters share this `Arc`; adapters only decode, authorize, and map
//! the stable application envelope.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::conversation::lifecycle::{
    ConversationLifecycleAction, ConversationLifecycleError, ConversationLifecycleOutcome,
    ConversationLifecycleService,
};
use crate::conversation::migration::{
    MigrationHostMode, MigrationMapV1, MigrationPhase, ReaderPrecedence, RecoveryActionResult,
    RecoveryItemV1, RecoveryStatus, ResolveRecoveryItemRequest,
};
use crate::conversation::session_workspace::{
    SessionWorkspaceError, SessionWorkspaceLoadOutcome, SessionWorkspaceService,
    SessionWorkspaceV1, SessionWorkspaceWriteOutcome,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};
use crate::conversation::{
    AgentSessionBinding, CompatibilityError, ConversationId, ConversationReader,
    ConversationRecordV2, CreationPartition, ExecutionTarget, PrepareConversationRequest,
    ProjectAttachment,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LegacyConversationSourceKind {
    LegacyStorageKey,
    LegacyAgentSessionId,
    LegacyChatHistoryId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyConversationKey {
    pub source_kind: LegacyConversationSourceKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyConversationResolution {
    pub conversation_id: ConversationId,
    pub canonical_route: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationHostKind {
    Desktop,
    Standalone,
}

impl From<MigrationHostMode> for ConversationHostKind {
    fn from(value: MigrationHostMode) -> Self {
        match value {
            MigrationHostMode::Desktop => Self::Desktop,
            MigrationHostMode::Standalone => Self::Standalone,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationHostState {
    Ready,
    Migrating,
    Hybrid,
    Recovery,
    Error,
}

/// Wire casing for the host-status envelope only. The shared TypeScript contract
/// (`src/shared/types/conversation-api.types.ts`) requires camelCase enum values,
/// while persisted migration layout/journal files remain snake_case; this adapter
/// keeps the two representations from drifting into a decode failure.
mod host_status_wire {
    use super::{MigrationPhase, ReaderPrecedence};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize_phase<S>(value: &MigrationPhase, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match value {
            MigrationPhase::Detected => "detected",
            MigrationPhase::Quiescing => "quiescing",
            MigrationPhase::Inventoried => "inventoried",
            MigrationPhase::Staging => "staging",
            MigrationPhase::Verifying => "verifying",
            MigrationPhase::CutoverPending => "cutoverPending",
            MigrationPhase::Committed => "committed",
            MigrationPhase::ObservationWindow => "observationWindow",
            MigrationPhase::RollbackPending => "rollbackPending",
            MigrationPhase::RolledBack => "rolledBack",
            MigrationPhase::Finalized => "finalized",
        };
        wire.serialize(serializer)
    }

    pub fn deserialize_phase<'de, D>(deserializer: D) -> Result<MigrationPhase, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "detected" => MigrationPhase::Detected,
            "quiescing" => MigrationPhase::Quiescing,
            "inventoried" => MigrationPhase::Inventoried,
            "staging" => MigrationPhase::Staging,
            "verifying" => MigrationPhase::Verifying,
            "cutoverPending" | "cutover_pending" => MigrationPhase::CutoverPending,
            "committed" => MigrationPhase::Committed,
            "observationWindow" | "observation_window" => MigrationPhase::ObservationWindow,
            "rollbackPending" | "rollback_pending" => MigrationPhase::RollbackPending,
            "rolledBack" | "rolled_back" => MigrationPhase::RolledBack,
            "finalized" => MigrationPhase::Finalized,
            other => {
                return Err(serde::de::Error::unknown_variant(
                    other,
                    &[
                        "detected",
                        "quiescing",
                        "inventoried",
                        "staging",
                        "verifying",
                        "cutoverPending",
                        "committed",
                        "observationWindow",
                        "rollbackPending",
                        "rolledBack",
                        "finalized",
                    ],
                ))
            }
        })
    }

    pub fn serialize_precedence<S>(
        value: &ReaderPrecedence,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match value {
            ReaderPrecedence::LegacyOnly => "legacyOnly",
            ReaderPrecedence::ConversationV2First => "conversationV2First",
            ReaderPrecedence::HybridLegacyFirst => "hybridLegacyFirst",
            ReaderPrecedence::ConversationV2Only => "conversationV2Only",
        };
        wire.serialize(serializer)
    }

    pub fn deserialize_precedence<'de, D>(deserializer: D) -> Result<ReaderPrecedence, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "legacyOnly" | "legacy_only" => ReaderPrecedence::LegacyOnly,
            "conversationV2First" | "conversation_v2_first" => {
                ReaderPrecedence::ConversationV2First
            }
            "hybridLegacyFirst" | "hybrid_legacy_first" => ReaderPrecedence::HybridLegacyFirst,
            "conversationV2Only" | "conversation_v2_only" => ReaderPrecedence::ConversationV2Only,
            other => {
                return Err(serde::de::Error::unknown_variant(
                    other,
                    &[
                        "legacyOnly",
                        "conversationV2First",
                        "hybridLegacyFirst",
                        "conversationV2Only",
                    ],
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHostStatus {
    pub host_kind: ConversationHostKind,
    pub state: ConversationHostState,
    pub code: String,
    #[serde(
        serialize_with = "host_status_wire::serialize_phase",
        deserialize_with = "host_status_wire::deserialize_phase"
    )]
    pub migration_phase: MigrationPhase,
    #[serde(
        serialize_with = "host_status_wire::serialize_precedence",
        deserialize_with = "host_status_wire::deserialize_precedence"
    )]
    pub reader_precedence: ReaderPrecedence,
    pub recovery_item_count: usize,
    pub recovery_items: Vec<RecoveryItemV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationOpenOutcome {
    pub conversation: ConversationRecordV2,
    pub workspace: SessionWorkspaceLoadOutcome,
}

/// Current replaceable ACP binding for one Conversation. `binding` is null when
/// the Conversation exists but has no current agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationBindingSnapshot {
    pub conversation_id: ConversationId,
    pub binding: Option<AgentSessionBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationAggregateMutationAction {
    AttachProject,
    DetachProject,
    UpdateExecutionTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationIdentitySnapshot {
    pub conversation_id: ConversationId,
    pub created_at_utc: String,
    pub creation_partition: CreationPartition,
    pub workspace_cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAggregateMutationOutcome {
    pub status: String,
    pub action: ConversationAggregateMutationAction,
    pub conversation_id: ConversationId,
    pub previous_revision: u64,
    pub revision: u64,
    pub identity_before: ConversationIdentitySnapshot,
    pub identity_after: ConversationIdentitySnapshot,
    pub project_attachment: Option<ProjectAttachment>,
    pub execution_target: ExecutionTarget,
    pub conversation: ConversationRecordV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationApplicationError {
    pub code: String,
    pub operation: &'static str,
    pub conversation_id: Option<ConversationId>,
    pub detail: String,
}

impl std::fmt::Display for ConversationApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ConversationApplicationError {}

pub type Result<T> = std::result::Result<T, ConversationApplicationError>;

pub struct ConversationApplicationService {
    reader: Arc<ConversationReader>,
    writer: Arc<ConversationWriter>,
    workspace: Arc<SessionWorkspaceService>,
    legacy_index: HashMap<(LegacyConversationSourceKind, String), Vec<ConversationId>>,
    lifecycle: OnceLock<ConversationLifecycleService>,
    binding_generation: AtomicU64,
    host_kind: ConversationHostKind,
    migration_phase: MigrationPhase,
    reader_precedence: ReaderPrecedence,
}

impl ConversationApplicationService {
    #[must_use]
    pub fn new(
        reader: Arc<ConversationReader>,
        writer: Arc<ConversationWriter>,
        workspace: Arc<SessionWorkspaceService>,
        migration_map: &MigrationMapV1,
        host_mode: MigrationHostMode,
        migration_phase: MigrationPhase,
        reader_precedence: ReaderPrecedence,
    ) -> Self {
        let mut legacy_index =
            HashMap::<(LegacyConversationSourceKind, String), Vec<ConversationId>>::new();
        for entry in &migration_map.entries {
            if let Some(value) = &entry.legacy_storage_key {
                push_legacy(
                    &mut legacy_index,
                    LegacyConversationSourceKind::LegacyStorageKey,
                    value,
                    entry.conversation_id,
                );
            }
            if let Some(value) = &entry.legacy_agent_session_id {
                push_legacy(
                    &mut legacy_index,
                    LegacyConversationSourceKind::LegacyAgentSessionId,
                    value,
                    entry.conversation_id,
                );
                // Legacy chat payload metadata used the same opaque session/history id. Index it
                // independently so adapters never guess that the value is a ConversationId.
                push_legacy(
                    &mut legacy_index,
                    LegacyConversationSourceKind::LegacyChatHistoryId,
                    value,
                    entry.conversation_id,
                );
            }
            if let Some(value) = chat_history_key(&entry.source_key) {
                push_legacy(
                    &mut legacy_index,
                    LegacyConversationSourceKind::LegacyChatHistoryId,
                    value,
                    entry.conversation_id,
                );
            }
        }
        for ids in legacy_index.values_mut() {
            ids.sort_by_key(ToString::to_string);
            ids.dedup();
        }
        Self {
            reader,
            writer,
            workspace,
            legacy_index,
            lifecycle: OnceLock::new(),
            binding_generation: AtomicU64::new(0),
            host_kind: host_mode.into(),
            migration_phase,
            reader_precedence,
        }
    }

    #[must_use]
    pub fn session_workspace(&self) -> Arc<SessionWorkspaceService> {
        Arc::clone(&self.workspace)
    }

    #[must_use]
    pub fn writer(&self) -> Arc<ConversationWriter> {
        Arc::clone(&self.writer)
    }

    /// Monotonic invalidation token for repository binding indexes and bounded negative caches.
    /// Bootstrap starts at zero; every committed attach/detach/rebind/suspend/replace outcome
    /// advances it exactly once. The repository integration in the following task consumes this
    /// source without inspecting or logging opaque binding values.
    #[must_use]
    pub fn binding_generation(&self) -> u64 {
        self.binding_generation.load(Ordering::Acquire)
    }

    /// Record the initial canonical `binding_bound` outcome. Creation owns that mutation outside
    /// this service, so its caller explicitly publishes the same one-step generation advance.
    pub fn record_binding_attached(
        &self,
        conversation_id: ConversationId,
        previous_revision: u64,
        revision: u64,
    ) -> u64 {
        self.advance_binding_generation(
            "attach_binding",
            conversation_id,
            previous_revision,
            revision,
        )
    }

    pub fn attach_lifecycle(&self, lifecycle: ConversationLifecycleService) -> Result<()> {
        self.lifecycle.set(lifecycle).map_err(|_| {
            application_error(
                "CONVERSATION_SERVICE_ALREADY_ATTACHED",
                "attach_lifecycle",
                None,
                "Conversation lifecycle runtime was already attached",
            )
        })
    }

    pub fn host_status(&self) -> Result<ConversationHostStatus> {
        let started = Instant::now();
        let recovery_items = self
            .workspace
            .list_recovery_items()
            .map_err(map_workspace_error)?
            .into_iter()
            .filter(|item| item.status == RecoveryStatus::Unresolved)
            .collect::<Vec<_>>();
        let recovery_item_count = recovery_items.len();
        let state = if recovery_item_count > 0 {
            ConversationHostState::Recovery
        } else if self.reader_precedence == ReaderPrecedence::HybridLegacyFirst
            || self.migration_phase == MigrationPhase::RolledBack
        {
            ConversationHostState::Hybrid
        } else if !matches!(
            self.migration_phase,
            MigrationPhase::ObservationWindow | MigrationPhase::Finalized
        ) {
            ConversationHostState::Migrating
        } else {
            ConversationHostState::Ready
        };
        let code = match state {
            ConversationHostState::Ready => "CONVERSATION_HOST_READY",
            ConversationHostState::Migrating => "CONVERSATION_HOST_MIGRATING",
            ConversationHostState::Hybrid => "CONVERSATION_HOST_HYBRID",
            ConversationHostState::Recovery => "CONVERSATION_RECOVERY_REQUIRED",
            ConversationHostState::Error => "CONVERSATION_HOST_ERROR",
        };
        log_outcome("host_status", None, self.host_kind, code, None, started);
        Ok(ConversationHostStatus {
            host_kind: self.host_kind,
            state,
            code: code.to_string(),
            migration_phase: self.migration_phase,
            reader_precedence: self.reader_precedence,
            recovery_item_count,
            recovery_items,
        })
    }

    #[must_use]
    pub fn list_conversations(&self) -> Vec<ConversationRecordV2> {
        let started = Instant::now();
        let records = self.reader.list();
        log_outcome(
            "list_conversations",
            None,
            self.host_kind,
            "OK",
            None,
            started,
        );
        records
    }

    pub fn get_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        let started = Instant::now();
        let result = self
            .reader
            .get(conversation_id)
            .map_err(map_compatibility_error);
        let code = result
            .as_ref()
            .map_or_else(|error| error.code.as_str(), |_| "OK");
        let revision = result.as_ref().ok().map(|record| record.last_seq);
        log_outcome(
            "get_conversation",
            Some(conversation_id),
            self.host_kind,
            code,
            revision,
            started,
        );
        result
    }

    pub fn current_binding(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationBindingSnapshot> {
        let started = Instant::now();
        let result: Result<ConversationBindingSnapshot> = (|| {
            self.reader
                .get(conversation_id)
                .map_err(map_compatibility_error)?;
            let binding = self
                .writer
                .repository()
                .current_binding(conversation_id)
                .map_err(map_repository_error)?;
            Ok(ConversationBindingSnapshot {
                conversation_id,
                binding,
            })
        })();
        let code = result
            .as_ref()
            .map_or_else(|error| error.code.as_str(), |_| "OK");
        log_outcome(
            "get_conversation_binding",
            Some(conversation_id),
            self.host_kind,
            code,
            None,
            started,
        );
        result
    }

    pub async fn open_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationOpenOutcome> {
        let started = Instant::now();
        let result = async {
            let conversation = self.get_conversation(conversation_id)?;
            self.backfill_managed_skills(&conversation);
            let workspace = self
                .workspace
                .load(conversation_id)
                .await
                .map_err(map_workspace_error)?;
            Ok(ConversationOpenOutcome {
                conversation,
                workspace,
            })
        }
        .await;
        log_result(
            "open_conversation",
            Some(conversation_id),
            self.host_kind,
            None,
            started,
            &result,
        );
        result
    }

    fn backfill_managed_skills(&self, conversation: &ConversationRecordV2) {
        let binding = match self
            .writer
            .repository()
            .current_binding(conversation.conversation_id)
        {
            Ok(Some(binding)) => binding,
            Ok(None) => return,
            Err(error) => {
                log::warn!(
                    "[scheduled-task-skill] boundary=backfill_binding_lookup_failed conversation_id={} error={error}",
                    conversation.conversation_id
                );
                return;
            }
        };
        let provider_key = binding
            .stable_agent_namespace
            .strip_prefix("config:")
            .unwrap_or(binding.stable_agent_namespace.as_str());
        if let Err(error) = crate::skills::ConversationSkillProvisioner::new().provision(
            std::path::Path::new(&conversation.workspace_cwd),
            provider_key,
        ) {
            // Opening durable history must remain available even if an old
            // workspace is missing or contains an unmanaged collision.
            log::warn!(
                "[scheduled-task-skill] boundary=backfill_failed conversation_id={} error={error}",
                conversation.conversation_id
            );
        }
    }

    pub fn resolve_legacy_conversation_id(
        &self,
        key: LegacyConversationKey,
    ) -> Result<LegacyConversationResolution> {
        let started = Instant::now();
        let value = key.value.trim();
        if value.is_empty() {
            return Err(application_error(
                "VALIDATION_ERROR",
                "resolve_legacy_conversation_id",
                None,
                "legacy value must be non-empty",
            ));
        }
        let matches = self
            .legacy_index
            .get(&(key.source_kind, value.to_string()))
            .cloned()
            .unwrap_or_default();
        let result = match matches.as_slice() {
            [] => Err(application_error(
                "CONVERSATION_NOT_FOUND",
                "resolve_legacy_conversation_id",
                None,
                "legacy Conversation mapping was not found",
            )),
            [conversation_id] => Ok(LegacyConversationResolution {
                conversation_id: *conversation_id,
                canonical_route: format!("#/c/{conversation_id}"),
            }),
            _ => Err(application_error(
                "LEGACY_ID_AMBIGUOUS",
                "resolve_legacy_conversation_id",
                None,
                "legacy Conversation key maps to multiple canonical Conversations",
            )),
        };
        let code = result
            .as_ref()
            .map_or_else(|error| error.code.as_str(), |_| "OK");
        let conversation_id = result.as_ref().ok().map(|value| value.conversation_id);
        log_outcome(
            "resolve_legacy_conversation_id",
            conversation_id,
            self.host_kind,
            code,
            None,
            started,
        );
        result
    }

    pub async fn attach_project(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
        attachment: ProjectAttachment,
    ) -> Result<ConversationAggregateMutationOutcome> {
        let started = Instant::now();
        let result = self
            .writer
            .attach_project(conversation_id, expected_revision, attachment, Utc::now())
            .await
            .map_err(map_repository_error)
            .and_then(|mutation| {
                aggregate_outcome(ConversationAggregateMutationAction::AttachProject, mutation)
            });
        log_aggregate_result(
            "attach_project",
            conversation_id,
            expected_revision,
            self.host_kind,
            started,
            &result,
        );
        result
    }

    pub async fn detach_project(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationAggregateMutationOutcome> {
        let started = Instant::now();
        let result = self
            .writer
            .detach_project(conversation_id, expected_revision, Utc::now())
            .await
            .map_err(map_repository_error)
            .and_then(|mutation| {
                aggregate_outcome(ConversationAggregateMutationAction::DetachProject, mutation)
            });
        log_aggregate_result(
            "detach_project",
            conversation_id,
            expected_revision,
            self.host_kind,
            started,
            &result,
        );
        result
    }

    pub async fn update_execution_target(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
        execution_target: ExecutionTarget,
    ) -> Result<ConversationAggregateMutationOutcome> {
        let started = Instant::now();
        let result = self
            .writer
            .update_execution_target(
                conversation_id,
                expected_revision,
                execution_target,
                Utc::now(),
            )
            .await
            .map_err(map_repository_error)
            .and_then(|mutation| {
                aggregate_outcome(
                    ConversationAggregateMutationAction::UpdateExecutionTarget,
                    mutation,
                )
            });
        log_aggregate_result(
            "update_execution_target",
            conversation_id,
            expected_revision,
            self.host_kind,
            started,
            &result,
        );
        result
    }

    pub async fn get_workspace(
        &self,
        conversation_id: ConversationId,
    ) -> Result<SessionWorkspaceLoadOutcome> {
        let started = Instant::now();
        let result = self
            .workspace
            .load(conversation_id)
            .await
            .map_err(map_workspace_error);
        log_result(
            "get_workspace",
            Some(conversation_id),
            self.host_kind,
            None,
            started,
            &result,
        );
        result
    }

    pub async fn write_workspace(
        &self,
        conversation_id: ConversationId,
        based_revision: Option<u64>,
        workspace: SessionWorkspaceV1,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::WorkspaceWrite)?;
            self.workspace
                .write(conversation_id, based_revision, workspace)
                .await
                .map_err(map_workspace_error)
        }
        .await;
        log_result(
            "write_workspace",
            Some(conversation_id),
            self.host_kind,
            based_revision,
            started,
            &result,
        );
        result
    }

    pub async fn resolve_recovery_item(
        &self,
        request: ResolveRecoveryItemRequest,
    ) -> Result<RecoveryActionResult> {
        let started = Instant::now();
        let recovery_id = request.recovery_id.clone();
        let expected_revision = request.expected_revision;
        let result = self
            .workspace
            .resolve_recovery(request)
            .await
            .map_err(map_workspace_error);
        let code = result
            .as_ref()
            .map_or_else(|error| error.code.as_str(), |_| "OK");
        log::info!(
            "[conversation-application] operation=resolve_recovery_item recovery_id={} host_kind={:?} code={} revision={} duration_ms={}",
            recovery_id,
            self.host_kind,
            code,
            expected_revision,
            started.elapsed().as_millis()
        );
        result
    }

    pub async fn detach_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::BindingDetach)?;
            self.lifecycle()?
                .detach_agent_binding(conversation_id, expected_revision)
                .await
                .map_err(map_lifecycle_error)
        }
        .await;
        self.advance_binding_generation_for_outcome(
            ConversationLifecycleAction::DetachBinding,
            &result,
        );
        log_result(
            "detach_binding",
            Some(conversation_id),
            self.host_kind,
            Some(expected_revision),
            started,
            &result,
        );
        result
    }

    pub async fn rebind_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::BindingRebind)?;
            self.lifecycle()?
                .rebind_detached_binding(conversation_id, expected_revision)
                .await
                .map_err(map_lifecycle_error)
        }
        .await;
        self.advance_binding_generation_for_outcome(
            ConversationLifecycleAction::RebindDetachedBinding,
            &result,
        );
        log_result(
            "rebind_binding",
            Some(conversation_id),
            self.host_kind,
            Some(expected_revision),
            started,
            &result,
        );
        result
    }

    pub async fn suspend_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::BindingSuspend)?;
            self.lifecycle()?
                .suspend_agent_binding(conversation_id, expected_revision)
                .await
                .map_err(map_lifecycle_error)
        }
        .await;
        self.advance_binding_generation_for_outcome(
            ConversationLifecycleAction::SuspendBinding,
            &result,
        );
        log_result(
            "suspend_binding",
            Some(conversation_id),
            self.host_kind,
            Some(expected_revision),
            started,
            &result,
        );
        result
    }

    pub async fn replace_binding(
        &self,
        conversation_id: ConversationId,
        request: PrepareConversationRequest,
        expected_revision: u64,
        target_runtime_agent_id: Option<String>,
    ) -> Result<ConversationLifecycleOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::BindingReplace)?;
            self.lifecycle()?
                .replace_agent_binding(
                    conversation_id,
                    request,
                    expected_revision,
                    target_runtime_agent_id,
                )
                .await
                .map_err(map_lifecycle_error)
        }
        .await;
        self.advance_binding_generation_for_outcome(
            ConversationLifecycleAction::ReplaceBinding,
            &result,
        );
        log_result(
            "replace_binding",
            Some(conversation_id),
            self.host_kind,
            Some(expected_revision),
            started,
            &result,
        );
        result
    }

    pub async fn delete_conversation(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let started = Instant::now();
        let result = async {
            self.ensure_writable(conversation_id, ConversationMutation::ConversationTombstone)?;
            self.lifecycle()?
                .delete_conversation(conversation_id, expected_revision)
                .await
                .map_err(map_lifecycle_error)
        }
        .await;
        log_result(
            "delete_conversation",
            Some(conversation_id),
            self.host_kind,
            Some(expected_revision),
            started,
            &result,
        );
        result
    }

    /// Rename a Conversation (LocalAlias title precedence).
    pub async fn rename_conversation(
        &self,
        conversation_id: ConversationId,
        title: String,
    ) -> Result<ConversationRecordV2> {
        let started = Instant::now();
        let result = async {
            let trimmed = title.trim();
            let char_count = trimmed.chars().count();
            if trimmed.is_empty() || char_count > 120 {
                return Err(application_error(
                    "CONVERSATION_VALIDATION",
                    "rename_conversation",
                    Some(conversation_id),
                    "title must be between 1 and 120 characters",
                ));
            }
            self.ensure_writable(conversation_id, ConversationMutation::MetadataUpdate)?;
            self.writer
                .update_metadata(
                    conversation_id,
                    crate::conversation::ConversationMetadataUpdate {
                        lifecycle_state: None,
                        execution_target: None,
                        title: Some(trimmed.to_string()),
                        title_source: Some(
                            crate::conversation::ConversationTitleSource::LocalAlias,
                        ),
                    },
                    ConversationMutation::MetadataUpdate,
                )
                .await
                .map_err(map_repository_error)
        }
        .await;
        log_result(
            "rename_conversation",
            Some(conversation_id),
            self.host_kind,
            None,
            started,
            &result,
        );
        result
    }

    fn advance_binding_generation_for_outcome(
        &self,
        expected_action: ConversationLifecycleAction,
        result: &Result<ConversationLifecycleOutcome>,
    ) {
        let Ok(ConversationLifecycleOutcome::Updated {
            action,
            conversation_id,
            previous_revision,
            revision,
            ..
        }) = result
        else {
            return;
        };
        if *action != expected_action {
            log::error!(
                "[conversation-application] binding generation action mismatch conversation_id={} expected={:?} actual={:?}",
                conversation_id,
                expected_action,
                action
            );
            return;
        }
        self.advance_binding_generation(
            "binding_lifecycle",
            *conversation_id,
            *previous_revision,
            *revision,
        );
    }

    fn advance_binding_generation(
        &self,
        operation: &'static str,
        conversation_id: ConversationId,
        previous_revision: u64,
        revision: u64,
    ) -> u64 {
        if previous_revision.checked_add(1) != Some(revision) {
            log::error!(
                "[conversation-application] binding generation rejected operation={} conversation_id={} previous_revision={} revision={} code=CONVERSATION_RECOVERY_REQUIRED",
                operation,
                conversation_id,
                previous_revision,
                revision
            );
            return self.binding_generation();
        }
        match self.binding_generation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |generation| generation.checked_add(1),
        ) {
            Ok(previous_generation) => {
                let generation = previous_generation + 1;
                log::info!(
                    "[conversation-application] binding generation advanced operation={} conversation_id={} generation={} revision={}",
                    operation,
                    conversation_id,
                    generation,
                    revision
                );
                generation
            }
            Err(generation) => {
                log::error!(
                    "[conversation-application] binding generation overflow operation={} conversation_id={} generation={} code=CONVERSATION_RECOVERY_REQUIRED",
                    operation,
                    conversation_id,
                    generation
                );
                generation
            }
        }
    }

    fn lifecycle(&self) -> Result<&ConversationLifecycleService> {
        self.lifecycle.get().ok_or_else(|| {
            application_error(
                "CONVERSATION_SERVICE_UNAVAILABLE",
                "lifecycle",
                None,
                "Conversation lifecycle runtime is not attached",
            )
        })
    }

    fn ensure_writable(
        &self,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> Result<()> {
        self.writer
            .authorize(conversation_id, mutation)
            .map(|_| ())
            .map_err(map_repository_error)
    }
}

fn aggregate_outcome(
    action: ConversationAggregateMutationAction,
    mutation: crate::conversation::repository::ConversationAggregateMutationRecord,
) -> Result<ConversationAggregateMutationOutcome> {
    let identity_before = identity_snapshot(&mutation.before);
    let identity_after = identity_snapshot(&mutation.after);
    if identity_before != identity_after
        || mutation.event.seq != mutation.after.last_seq
        || mutation.before.last_seq >= mutation.after.last_seq
    {
        log::error!(
            "[conversation-application] aggregate invariant failed conversation_id={} action={:?} code=CONVERSATION_RECOVERY_REQUIRED",
            mutation.before.conversation_id,
            action
        );
        return Err(application_error(
            "CONVERSATION_RECOVERY_REQUIRED",
            "aggregate_mutation",
            Some(mutation.before.conversation_id),
            "aggregate mutation did not preserve immutable identity or advance canonical lastSeq",
        ));
    }
    Ok(ConversationAggregateMutationOutcome {
        status: "updated".to_string(),
        action,
        conversation_id: mutation.after.conversation_id,
        previous_revision: mutation.before.last_seq,
        revision: mutation.after.last_seq,
        identity_before,
        identity_after,
        project_attachment: mutation.after.project_attachment.clone(),
        execution_target: mutation.after.execution_target.clone(),
        conversation: mutation.after,
    })
}

fn identity_snapshot(record: &ConversationRecordV2) -> ConversationIdentitySnapshot {
    ConversationIdentitySnapshot {
        conversation_id: record.conversation_id,
        created_at_utc: crate::conversation::format_created_at_utc(&record.created_at_utc),
        creation_partition: record.creation_partition.clone(),
        workspace_cwd: record.workspace_cwd.clone(),
    }
}

fn log_aggregate_result(
    operation: &'static str,
    conversation_id: ConversationId,
    expected_revision: u64,
    host_kind: ConversationHostKind,
    started: Instant,
    result: &Result<ConversationAggregateMutationOutcome>,
) {
    match result {
        Ok(outcome) => log::info!(
            "[conversation-application] operation={} conversation_id={} host_kind={:?} previous_revision={} revision={} target_kind={} code=OK duration_ms={}",
            operation,
            conversation_id,
            host_kind,
            outcome.previous_revision,
            outcome.revision,
            execution_target_kind(&outcome.execution_target),
            started.elapsed().as_millis()
        ),
        Err(error) if error.code == "CONVERSATION_CONFLICT" => log::warn!(
            "[conversation-application] operation={} conversation_id={} host_kind={:?} expected_revision={} code={} duration_ms={}",
            operation,
            conversation_id,
            host_kind,
            expected_revision,
            error.code,
            started.elapsed().as_millis()
        ),
        Err(error) => log::warn!(
            "[conversation-application] operation={} conversation_id={} host_kind={:?} expected_revision={} code={} duration_ms={}",
            operation,
            conversation_id,
            host_kind,
            expected_revision,
            error.code,
            started.elapsed().as_millis()
        ),
    }
}

fn execution_target_kind(target: &ExecutionTarget) -> &'static str {
    match target {
        ExecutionTarget::Workspace => "workspace",
        ExecutionTarget::ProjectRoot { .. } => "project_root",
        ExecutionTarget::Worktree { .. } => "worktree",
    }
}

fn push_legacy(
    index: &mut HashMap<(LegacyConversationSourceKind, String), Vec<ConversationId>>,
    kind: LegacyConversationSourceKind,
    value: &str,
    conversation_id: ConversationId,
) {
    if !value.trim().is_empty() {
        index
            .entry((kind, value.to_string()))
            .or_default()
            .push(conversation_id);
    }
}

fn chat_history_key(source_key: &str) -> Option<&str> {
    let relative = source_key
        .strip_prefix("legacy_chat_history:")?
        .split_once(':')?
        .1;
    relative
        .strip_prefix("payloads/")?
        .strip_suffix(".json")
        .filter(|value| !value.is_empty())
}

fn application_error(
    code: impl Into<String>,
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    detail: impl Into<String>,
) -> ConversationApplicationError {
    ConversationApplicationError {
        code: code.into(),
        operation,
        conversation_id,
        detail: detail.into(),
    }
}

fn map_compatibility_error(source: CompatibilityError) -> ConversationApplicationError {
    let code = match source.code {
        "LEGACY_COMPATIBILITY_READ_ONLY" => "LEGACY_COMPATIBILITY_READ_ONLY",
        "CONVERSATION_NOT_FOUND" => "CONVERSATION_NOT_FOUND",
        _ => "CONVERSATION_RECOVERY_REQUIRED",
    };
    application_error(code, "read_conversation", None, source.detail)
}

fn map_repository_error(
    source: crate::conversation::repository::RepositoryError,
) -> ConversationApplicationError {
    let code = serde_json::to_value(source.code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string());
    application_error(
        code,
        source.operation,
        source.conversation_id,
        source.detail,
    )
}

fn map_workspace_error(source: SessionWorkspaceError) -> ConversationApplicationError {
    application_error(
        source.code.as_str(),
        source.operation,
        source.conversation_id,
        source.detail,
    )
}

fn map_lifecycle_error(source: ConversationLifecycleError) -> ConversationApplicationError {
    application_error(
        source.code.as_str(),
        source.operation,
        source.conversation_id,
        source.detail,
    )
}

fn log_outcome(
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    host_kind: ConversationHostKind,
    code: &str,
    revision: Option<u64>,
    started: Instant,
) {
    log::info!(
        "[conversation-application] operation={} conversation_id={} host_kind={:?} code={} revision={:?} duration_ms={}",
        operation,
        conversation_id.map_or_else(|| "none".to_string(), |value| value.to_string()),
        host_kind,
        code,
        revision,
        started.elapsed().as_millis()
    );
}

fn log_result<T>(
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    host_kind: ConversationHostKind,
    revision: Option<u64>,
    started: Instant,
    result: &Result<T>,
) {
    let code = result
        .as_ref()
        .map_or_else(|error| error.code.as_str(), |_| "OK");
    log_outcome(
        operation,
        conversation_id,
        host_kind,
        code,
        revision,
        started,
    );
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
        CreatedAtSource, IdentityDecision, MigrationMapEntryV1, MIGRATION_MAP_SCHEMA_VERSION,
    };
    use crate::conversation::{
        ConversationRepository, LegacyConversationReader, SessionWorkspaceProjectionState,
    };
    use uuid::Uuid;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    async fn fixture() -> (
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

    async fn service() -> (tempfile::TempDir, ConversationApplicationService) {
        let (temp, _repository, service) = fixture().await;
        (temp, service)
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
        RecoveryQueueV1::new(uuid::Uuid::new_v4(), vec![item.clone()])
            .persist(
                &state_root
                    .join("conversation-migrations")
                    .join("workspace-recovery-v1"),
            )
            .unwrap();
        item
    }

    #[tokio::test]
    async fn list_get_open_and_host_status_share_one_service() {
        let (_temp, service) = service().await;
        assert_eq!(service.list_conversations().len(), 1);
        let id = ConversationId::parse(ID).unwrap();
        assert_eq!(service.get_conversation(id).unwrap().conversation_id, id);
        assert!(matches!(
            service.open_conversation(id).await.unwrap().workspace,
            SessionWorkspaceLoadOutcome::Missing { .. }
        ));
        assert_eq!(
            service.host_status().unwrap().state,
            ConversationHostState::Ready
        );
        let _ = SessionWorkspaceProjectionState::Native;
    }

    #[tokio::test]
    async fn opening_an_existing_conversation_backfills_its_agent_skills_idempotently() {
        let (_temp, _repository, service) = fixture().await;
        let conversation_id = ConversationId::parse(ID).unwrap();
        let conversation = service.get_conversation(conversation_id).unwrap();
        let bound_at = parse_created_at_utc("2026-08-15T09:45:16.123Z").unwrap();
        service
            .writer()
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "old/opaque-session".to_string(),
                    runtime_agent_id: "runtime-agent".to_string(),
                    stable_agent_namespace: "config:claude-agent-acp".to_string(),
                    execution_cwd: conversation.workspace_cwd.clone(),
                    bound_at_utc: bound_at,
                    state: AgentSessionBindingState::Active,
                },
                bound_at,
            )
            .await
            .unwrap();

        service.open_conversation(conversation_id).await.unwrap();
        // The directory name is the managed skill's name, which is a brand
        // contract. Read through the same accessor the provisioner builds its
        // paths from rather than spelled here — an inline copy silently stops
        // matching the moment that contract is renamed, and this test would then
        // report "the backfill did not run" for a backfill that ran fine.
        let skill_name = crate::skills::provisioner::scheduled_task_skill_name();
        let cross_tool = std::path::Path::new(&conversation.workspace_cwd)
            .join(".agents/skills")
            .join(skill_name)
            .join("SKILL.md");
        let provider = std::path::Path::new(&conversation.workspace_cwd)
            .join(".claude/skills")
            .join(skill_name)
            .join("SKILL.md");
        let first = std::fs::read_to_string(&cross_tool).unwrap();
        assert!(provider.exists());

        service.open_conversation(conversation_id).await.unwrap();
        assert_eq!(std::fs::read_to_string(cross_tool).unwrap(), first);
    }

    #[tokio::test]
    async fn host_status_wire_matches_shared_contract_camel_case() {
        let (_temp, service) = service().await;
        let status = service.host_status().unwrap();
        let wire = serde_json::to_value(&status).unwrap();
        // The shared TypeScript parser (src/shared/types/conversation-api.types.ts)
        // requires camelCase enum values on the wire even though persisted
        // migration layout/journal files stay snake_case.
        assert!(
            matches!(
                wire["migrationPhase"].as_str(),
                Some("observationWindow") | Some("finalized")
            ),
            "migrationPhase must serialize camelCase, got {wire:?}"
        );
        assert!(
            matches!(
                wire["readerPrecedence"].as_str(),
                Some("legacyOnly")
                    | Some("conversationV2First")
                    | Some("hybridLegacyFirst")
                    | Some("conversationV2Only")
            ),
            "readerPrecedence must serialize camelCase, got {wire:?}"
        );
        // Deserialization accepts both wire and legacy persisted casings.
        for phase in ["observationWindow", "observation_window"] {
            let json = serde_json::json!({
                "hostKind": "desktop",
                "state": "ready",
                "code": "CONVERSATION_HOST_READY",
                "migrationPhase": phase,
                "readerPrecedence": "conversation_v2_first",
                "recoveryItemCount": 0,
                "recoveryItems": []
            });
            let decoded: ConversationHostStatus = serde_json::from_value(json).unwrap();
            assert_eq!(decoded.migration_phase, MigrationPhase::ObservationWindow);
            assert_eq!(
                decoded.reader_precedence,
                ReaderPrecedence::ConversationV2First
            );
        }
    }

    #[tokio::test]
    async fn legacy_resolver_accepts_exact_source_kinds_and_never_parses_values_as_ids() {
        let (_temp, service) = service().await;
        for (source_kind, value) in [
            (
                LegacyConversationSourceKind::LegacyStorageKey,
                "legacy-storage",
            ),
            (
                LegacyConversationSourceKind::LegacyAgentSessionId,
                "opaque-agent-session",
            ),
            (
                LegacyConversationSourceKind::LegacyChatHistoryId,
                "chat-history",
            ),
        ] {
            let resolution = service
                .resolve_legacy_conversation_id(LegacyConversationKey {
                    source_kind,
                    value: value.to_string(),
                })
                .unwrap();
            assert_eq!(resolution.conversation_id.to_string(), ID);
            assert_eq!(resolution.canonical_route, format!("#/c/{ID}"));
        }
        let error = service
            .resolve_legacy_conversation_id(LegacyConversationKey {
                source_kind: LegacyConversationSourceKind::LegacyStorageKey,
                value: ID.to_string(),
            })
            .unwrap_err();
        assert_eq!(error.code, "CONVERSATION_NOT_FOUND");
    }

    #[tokio::test]
    async fn tauri_command_inners_preserve_legacy_and_recovery_golden_envelopes() {
        let (_temp, repository, service) = fixture().await;
        for (source_kind, value) in [
            ("legacyStorageKey", "legacy-storage"),
            ("legacyAgentSessionId", "opaque-agent-session"),
            ("legacyChatHistoryId", "chat-history"),
        ] {
            let result = crate::commands::conversation_resolve_legacy_id_inner(
                &service,
                serde_json::json!({"sourceKind":source_kind,"value":value}),
            );
            assert!(result.success, "{source_kind}: {:?}", result.error);
            assert_eq!(result.data.unwrap().canonical_route, format!("#/c/{ID}"));
        }
        let missing = crate::commands::conversation_resolve_legacy_id_inner(
            &service,
            serde_json::json!({"sourceKind":"legacyStorageKey","value":"missing"}),
        );
        assert_eq!(missing.code.as_deref(), Some("CONVERSATION_NOT_FOUND"));

        let item = seed_recovery(&repository);
        let result = crate::commands::conversation_recovery_resolve_inner(
            &service,
            serde_json::json!({
                "recoveryId":item.recovery_id,
                "expectedRevision":item.revision,
                "action":"inspect",
                "payload":{}
            }),
        )
        .await;
        assert!(result.success, "inspect: {:?}", result.error);
        let result = result.data.unwrap();
        assert_eq!(serde_json::to_value(result.action).unwrap(), "inspect");
        assert_eq!(result.source_paths, item.source_paths);
        assert_eq!(result.source_sha256, item.source_sha256);
        assert_eq!(
            service.host_status().unwrap().state,
            ConversationHostState::Recovery
        );

        let associated = crate::commands::conversation_recovery_resolve_inner(
            &service,
            serde_json::json!({
                "recoveryId":item.recovery_id,
                "expectedRevision":item.revision,
                "idempotencyKey":"21aee10a-56b8-4624-a5e7-586c25dc8d1f",
                "action":"associateConversation",
                "payload":{"conversationId":ID}
            }),
        )
        .await;
        assert!(associated.success, "associate: {:?}", associated.error);
        assert_eq!(
            service.host_status().unwrap().state,
            ConversationHostState::Ready
        );
    }

    #[tokio::test]
    async fn binding_generation_advances_once_for_each_canonical_binding_outcome() {
        let (_temp, service) = service().await;
        let conversation_id = ConversationId::parse(ID).unwrap();
        assert_eq!(service.binding_generation(), 0);
        assert_eq!(service.record_binding_attached(conversation_id, 0, 1), 1);

        for (index, action) in [
            ConversationLifecycleAction::DetachBinding,
            ConversationLifecycleAction::RebindDetachedBinding,
            ConversationLifecycleAction::SuspendBinding,
            ConversationLifecycleAction::ReplaceBinding,
        ]
        .into_iter()
        .enumerate()
        {
            let previous_revision = index as u64 + 1;
            let result = Ok(ConversationLifecycleOutcome::Updated {
                action,
                conversation_id,
                previous_revision,
                revision: previous_revision + 1,
                workspace_cwd: "/visible/conversation".to_string(),
                lifecycle_state: ConversationLifecycleState::Ready,
                current_binding: None,
                previous_agent_session_id: None,
            });
            service.advance_binding_generation_for_outcome(action, &result);
            assert_eq!(service.binding_generation(), index as u64 + 2);
        }

        let generation = service.binding_generation();
        assert_eq!(
            service.record_binding_attached(conversation_id, 99, 101),
            generation,
            "a non-canonical revision jump must not invalidate the binding index"
        );
    }

    #[test]
    fn duplicate_legacy_mappings_are_ambiguous_and_hybrid_mutation_is_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("conversations/v2");
        let (repository, _) = ConversationRepository::open(root).unwrap();
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::HybridLegacyFirst,
        ));
        let id = ConversationId::parse(ID).unwrap();
        let other = ConversationId::new_v4();
        let entries = [id, other]
            .into_iter()
            .map(|conversation_id| MigrationMapEntryV1 {
                source_key: format!("legacy_host_session:0:{conversation_id}"),
                legacy_storage_key: Some("duplicate".to_string()),
                legacy_agent_session_id: None,
                conversation_id,
                identity_decision: IdentityDecision::AllocatedCollisionUuid,
                created_at_source: Some(CreatedAtSource::HostMetadata),
                source_record_sha256: "a".repeat(64),
            })
            .collect();
        let map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            entries,
        };
        let authority = Arc::new(crate::conversation::ConversationWriteAuthority::new(
            repository.as_ref(),
            ReaderPrecedence::HybridLegacyFirst,
            map.entries.iter().map(|entry| entry.conversation_id),
        ));
        let writer = Arc::new(ConversationWriter::new(Arc::clone(&repository), authority).unwrap());
        let service = ConversationApplicationService::new(
            reader,
            Arc::clone(&writer),
            Arc::new(SessionWorkspaceService::new(writer)),
            &map,
            MigrationHostMode::Standalone,
            MigrationPhase::RolledBack,
            ReaderPrecedence::HybridLegacyFirst,
        );
        assert_eq!(
            service
                .resolve_legacy_conversation_id(LegacyConversationKey {
                    source_kind: LegacyConversationSourceKind::LegacyStorageKey,
                    value: "duplicate".to_string(),
                })
                .unwrap_err()
                .code,
            "LEGACY_ID_AMBIGUOUS"
        );
        assert_eq!(
            service
                .ensure_writable(id, ConversationMutation::MetadataUpdate)
                .unwrap_err()
                .code,
            "LEGACY_COMPATIBILITY_READ_ONLY"
        );
    }
}
