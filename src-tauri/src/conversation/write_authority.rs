//! Bootstrap-owned admission for every canonical Conversation mutation.
//!
//! Runtime callers receive only [`ConversationWriter`]. Repository mutators additionally require
//! an unforgeable [`RepositoryWritePermit`] bound to one repository instance, ConversationId, and
//! mutation kind. Legacy migration uses the separate crate-private [`MigrationWriter`], whose
//! permits are accepted only by a staging-mode repository.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::conversation::catalog::ConversationProvenanceFileV1;
use crate::conversation::contracts::{
    AgentSessionBinding, ConversationErrorCode, ConversationId, ConversationRecordV2,
    ExecutionTarget, ProjectAttachment,
};
use crate::conversation::event_log::{ConversationEventRecordV2, ConversationEventType};
use crate::conversation::migration::ReaderPrecedence;
use crate::conversation::repository::{
    ConversationMetadataUpdate, ConversationRepository, RepositoryError, Result,
};

/// Exhaustive stable classification used by admission logs and structural tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversationMutation {
    CreateConversation,
    CreationRetry,
    CreationRecovery,
    CreationFailureRecord,
    MetadataUpdate,
    AcpEventAppend,
    BindingBind,
    BindingDetach,
    BindingRebind,
    BindingSuspend,
    BindingReplace,
    ProjectAttachmentAdd,
    ProjectAttachmentDetach,
    ExecutionTargetUpdate,
    ProvenanceWrite,
    ConversationSync,
    ConversationTombstone,
    WorkspaceWrite,
    TerminalRefAdd,
    TerminalRefRemove,
    RecoveryAction,
    RecoveryQueueWrite,
    CompensationRecord,
    MigrationStageCreate,
    MigrationStageEvent,
    MigrationStageProvenance,
    MigrationStageSync,
}

impl ConversationMutation {
    pub const RUNTIME: [Self; 23] = [
        Self::CreateConversation,
        Self::CreationRetry,
        Self::CreationRecovery,
        Self::CreationFailureRecord,
        Self::MetadataUpdate,
        Self::AcpEventAppend,
        Self::BindingBind,
        Self::BindingDetach,
        Self::BindingRebind,
        Self::BindingSuspend,
        Self::BindingReplace,
        Self::ProjectAttachmentAdd,
        Self::ProjectAttachmentDetach,
        Self::ExecutionTargetUpdate,
        Self::ProvenanceWrite,
        Self::ConversationSync,
        Self::ConversationTombstone,
        Self::WorkspaceWrite,
        Self::TerminalRefAdd,
        Self::TerminalRefRemove,
        Self::RecoveryAction,
        Self::RecoveryQueueWrite,
        Self::CompensationRecord,
    ];

    #[must_use]
    pub const fn is_migration_staging(self) -> bool {
        matches!(
            self,
            Self::MigrationStageCreate
                | Self::MigrationStageEvent
                | Self::MigrationStageProvenance
                | Self::MigrationStageSync
        )
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateConversation => "create_conversation",
            Self::CreationRetry => "creation_retry",
            Self::CreationRecovery => "creation_recovery",
            Self::CreationFailureRecord => "creation_failure_record",
            Self::MetadataUpdate => "metadata_update",
            Self::AcpEventAppend => "acp_event_append",
            Self::BindingBind => "binding_bind",
            Self::BindingDetach => "binding_detach",
            Self::BindingRebind => "binding_rebind",
            Self::BindingSuspend => "binding_suspend",
            Self::BindingReplace => "binding_replace",
            Self::ProjectAttachmentAdd => "project_attachment_add",
            Self::ProjectAttachmentDetach => "project_attachment_detach",
            Self::ExecutionTargetUpdate => "execution_target_update",
            Self::ProvenanceWrite => "provenance_write",
            Self::ConversationSync => "conversation_sync",
            Self::ConversationTombstone => "conversation_tombstone",
            Self::WorkspaceWrite => "workspace_write",
            Self::TerminalRefAdd => "terminal_ref_add",
            Self::TerminalRefRemove => "terminal_ref_remove",
            Self::RecoveryAction => "recovery_action",
            Self::RecoveryQueueWrite => "recovery_queue_write",
            Self::CompensationRecord => "compensation_record",
            Self::MigrationStageCreate => "migration_stage_create",
            Self::MigrationStageEvent => "migration_stage_event",
            Self::MigrationStageProvenance => "migration_stage_provenance",
            Self::MigrationStageSync => "migration_stage_sync",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryPermitKind {
    Runtime,
    MigrationStaging,
}

/// Capability required by every repository mutation.
///
/// Fields and constructors stay private to this module. Other crate modules can carry a permit
/// returned by [`ConversationWriter::authorize`], but cannot forge or retarget it.
#[derive(Debug)]
pub(crate) struct RepositoryWritePermit {
    repository_instance_id: Uuid,
    conversation_id: ConversationId,
    mutation: ConversationMutation,
    kind: RepositoryPermitKind,
}

impl RepositoryWritePermit {
    pub(crate) fn admits(
        &self,
        repository_instance_id: Uuid,
        conversation_id: ConversationId,
        repository_is_staging: bool,
    ) -> bool {
        self.repository_instance_id == repository_instance_id
            && self.conversation_id == conversation_id
            && repository_is_staging == (self.kind == RepositoryPermitKind::MigrationStaging)
    }

    #[must_use]
    pub(crate) const fn mutation(&self) -> ConversationMutation {
        self.mutation
    }
}

/// Immutable bootstrap-owned rollback policy.
pub struct ConversationWriteAuthority {
    repository_instance_id: Uuid,
    reader_precedence: ReaderPrecedence,
    mapped_legacy_ids: HashSet<ConversationId>,
}

impl ConversationWriteAuthority {
    pub(crate) fn new(
        repository: &ConversationRepository,
        reader_precedence: ReaderPrecedence,
        mapped_legacy_ids: impl IntoIterator<Item = ConversationId>,
    ) -> Self {
        Self {
            repository_instance_id: repository.instance_id(),
            reader_precedence,
            mapped_legacy_ids: mapped_legacy_ids.into_iter().collect(),
        }
    }

    #[must_use]
    pub const fn reader_precedence(&self) -> ReaderPrecedence {
        self.reader_precedence
    }

    #[must_use]
    pub fn is_mapped_legacy(&self, conversation_id: ConversationId) -> bool {
        self.mapped_legacy_ids.contains(&conversation_id)
    }

    #[must_use]
    pub fn allows_runtime_mutation(&self, conversation_id: ConversationId) -> bool {
        self.reader_precedence != ReaderPrecedence::LegacyOnly
            && !(self.reader_precedence == ReaderPrecedence::HybridLegacyFirst
                && self.mapped_legacy_ids.contains(&conversation_id))
    }

    fn authorize(
        &self,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> Result<RepositoryWritePermit> {
        let denied = self.reader_precedence == ReaderPrecedence::LegacyOnly
            || (self.reader_precedence == ReaderPrecedence::HybridLegacyFirst
                && self.mapped_legacy_ids.contains(&conversation_id));
        if denied {
            log::warn!(
                "[conversation-write-authority] mutation={} conversation_id={} policy={:?} result=LEGACY_COMPATIBILITY_READ_ONLY",
                mutation.as_str(),
                conversation_id,
                self.reader_precedence
            );
            return Err(RepositoryError {
                code: ConversationErrorCode::LegacyCompatibilityReadOnly,
                operation: mutation.as_str(),
                conversation_id: Some(conversation_id),
                detail: "mapped legacy Conversations are read-only under the active reader policy"
                    .to_string(),
            });
        }
        log::debug!(
            "[conversation-write-authority] mutation={} conversation_id={} policy={:?} result=AUTHORIZED",
            mutation.as_str(),
            conversation_id,
            self.reader_precedence
        );
        Ok(RepositoryWritePermit {
            repository_instance_id: self.repository_instance_id,
            conversation_id,
            mutation,
            kind: RepositoryPermitKind::Runtime,
        })
    }
}

/// Sole runtime mutation facade published by bootstrap.
#[derive(Clone)]
pub struct ConversationWriter {
    repository: Arc<ConversationRepository>,
    authority: Arc<ConversationWriteAuthority>,
}

impl ConversationWriter {
    pub(crate) fn new(
        repository: Arc<ConversationRepository>,
        authority: Arc<ConversationWriteAuthority>,
    ) -> Result<Self> {
        if repository.is_staging() || repository.instance_id() != authority.repository_instance_id {
            return Err(RepositoryError {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                operation: "construct_conversation_writer",
                conversation_id: None,
                detail: "runtime writer and repository instance do not match".to_string(),
            });
        }
        Ok(Self {
            repository,
            authority,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(repository: Arc<ConversationRepository>) -> Arc<Self> {
        let authority = Arc::new(ConversationWriteAuthority::new(
            repository.as_ref(),
            ReaderPrecedence::ConversationV2Only,
            std::iter::empty(),
        ));
        Arc::new(Self::new(repository, authority).expect("test writer matches repository"))
    }

    #[must_use]
    pub fn repository(&self) -> &Arc<ConversationRepository> {
        &self.repository
    }

    #[must_use]
    pub fn authority(&self) -> &Arc<ConversationWriteAuthority> {
        &self.authority
    }

    #[must_use]
    pub fn is_writable(&self, conversation_id: ConversationId) -> bool {
        self.authority.allows_runtime_mutation(conversation_id)
    }

    pub(crate) fn authorize(
        &self,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> Result<RepositoryWritePermit> {
        if mutation.is_migration_staging() {
            log::error!(
                "[conversation-write-authority] runtime writer rejected migration-only mutation={} conversation_id={} code=CONVERSATION_RECOVERY_REQUIRED",
                mutation.as_str(),
                conversation_id
            );
            return Err(RepositoryError {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                operation: mutation.as_str(),
                conversation_id: Some(conversation_id),
                detail: "migration staging mutations require MigrationWriter".to_string(),
            });
        }
        self.authority.authorize(conversation_id, mutation)
    }

    pub(crate) async fn create_conversation(
        self: &Arc<Self>,
        record: ConversationRecordV2,
        mutation: ConversationMutation,
    ) -> Result<ConversationRecordV2> {
        let permit = self.authorize(record.conversation_id, mutation)?;
        self.repository.create_conversation(&permit, record).await
    }

    pub(crate) async fn update_metadata(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        update: ConversationMetadataUpdate,
        mutation: ConversationMutation,
    ) -> Result<ConversationRecordV2> {
        let permit = self.authorize(conversation_id, mutation)?;
        self.repository
            .update_metadata(&permit, conversation_id, update)
            .await
    }

    pub(crate) async fn append_event(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
        mutation: ConversationMutation,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, mutation)?;
        self.repository
            .append_event(&permit, conversation_id, recorded_at_utc, type_, payload)
            .await
    }

    pub(crate) async fn bind_agent_session(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        binding: AgentSessionBinding,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, ConversationMutation::BindingBind)?;
        self.repository
            .bind_agent_session(&permit, conversation_id, binding, recorded_at_utc)
            .await
    }

    // These single-mutation entry points remain part of the exhaustive authority surface and are
    // exercised directly by repository/adapter tests. Production lifecycle flows use compound
    // services that authorize once and hold the repository lifecycle lock across side effects.
    #[allow(dead_code)]
    pub(crate) async fn detach_agent_binding(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, ConversationMutation::BindingDetach)?;
        self.repository
            .detach_agent_binding(&permit, conversation_id, recorded_at_utc)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn rebind_detached_binding(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, ConversationMutation::BindingRebind)?;
        self.repository
            .rebind_detached_binding(&permit, conversation_id, recorded_at_utc)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn suspend_agent_binding(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        provider_confirmed: bool,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<Option<ConversationEventRecordV2>> {
        let permit = self.authorize(conversation_id, ConversationMutation::BindingSuspend)?;
        self.repository
            .suspend_agent_binding(
                &permit,
                conversation_id,
                provider_confirmed,
                recorded_at_utc,
            )
            .await
    }

    pub(crate) async fn replace_agent_binding(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        binding: AgentSessionBinding,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, ConversationMutation::BindingReplace)?;
        self.repository
            .replace_agent_binding(&permit, conversation_id, binding, recorded_at_utc)
            .await
    }

    pub(crate) async fn append_project_attachment(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        attachment: ProjectAttachment,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(conversation_id, ConversationMutation::ProjectAttachmentAdd)?;
        self.repository
            .append_project_attachment(&permit, conversation_id, attachment, recorded_at_utc)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn detach_project_attachment(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.authorize(
            conversation_id,
            ConversationMutation::ProjectAttachmentDetach,
        )?;
        self.repository
            .detach_project_attachment(&permit, conversation_id, recorded_at_utc)
            .await
    }

    pub(crate) async fn update_execution_target(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        expected_revision: u64,
        execution_target: ExecutionTarget,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<crate::conversation::repository::ConversationAggregateMutationRecord> {
        let permit =
            self.authorize(conversation_id, ConversationMutation::ExecutionTargetUpdate)?;
        self.repository
            .update_execution_target_cas(
                &permit,
                conversation_id,
                expected_revision,
                execution_target,
                recorded_at_utc,
            )
            .await
    }

    pub(crate) async fn attach_project(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        expected_revision: u64,
        attachment: ProjectAttachment,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<crate::conversation::repository::ConversationAggregateMutationRecord> {
        let permit = self.authorize(conversation_id, ConversationMutation::ProjectAttachmentAdd)?;
        self.repository
            .attach_project_cas(
                &permit,
                conversation_id,
                expected_revision,
                attachment,
                recorded_at_utc,
            )
            .await
    }

    pub(crate) async fn detach_project(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        expected_revision: u64,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<crate::conversation::repository::ConversationAggregateMutationRecord> {
        let permit = self.authorize(
            conversation_id,
            ConversationMutation::ProjectAttachmentDetach,
        )?;
        self.repository
            .detach_project_cas(&permit, conversation_id, expected_revision, recorded_at_utc)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn write_provenance(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        provenance: ConversationProvenanceFileV1,
    ) -> Result<()> {
        let permit = self.authorize(conversation_id, ConversationMutation::ProvenanceWrite)?;
        self.repository
            .write_provenance(&permit, conversation_id, provenance)
            .await
    }

    pub(crate) async fn sync_conversation(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> Result<()> {
        let permit = self.authorize(conversation_id, mutation)?;
        self.repository
            .sync_conversation(&permit, conversation_id)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn mark_deleted(
        self: &Arc<Self>,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        let permit =
            self.authorize(conversation_id, ConversationMutation::ConversationTombstone)?;
        self.repository.mark_deleted(&permit, conversation_id).await
    }

    pub(crate) fn replace_workspace_bytes(
        &self,
        conversation_id: ConversationId,
        bytes: &[u8],
        mutation: ConversationMutation,
    ) -> Result<()> {
        let permit = self.authorize(conversation_id, mutation)?;
        self.repository
            .replace_workspace_bytes(&permit, conversation_id, bytes)
    }

    pub(crate) fn clear_recovery_item(
        &self,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> Result<()> {
        let permit = self.authorize(conversation_id, mutation)?;
        self.repository
            .clear_recovery_item(&permit, conversation_id)
    }
}

/// Privileged writer accepted only by a staging-mode repository.
///
/// The type is crate-private and intentionally not re-exported from `conversation`; production
/// ownership audits require all construction/use sites to remain under `conversation::migration`.
pub(crate) struct MigrationWriter {
    repository: Arc<ConversationRepository>,
}

impl MigrationWriter {
    pub(crate) fn new(repository: Arc<ConversationRepository>) -> Result<Self> {
        if !repository.is_staging() {
            log::error!(
                "[conversation-write-authority] migration writer rejected result=LIVE_REPOSITORY"
            );
            return Err(RepositoryError {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                operation: "construct_migration_writer",
                conversation_id: None,
                detail: "migration writer requires a staging-mode repository".to_string(),
            });
        }
        Ok(Self { repository })
    }

    fn permit(
        &self,
        conversation_id: ConversationId,
        mutation: ConversationMutation,
    ) -> RepositoryWritePermit {
        RepositoryWritePermit {
            repository_instance_id: self.repository.instance_id(),
            conversation_id,
            mutation,
            kind: RepositoryPermitKind::MigrationStaging,
        }
    }

    #[must_use]
    pub(crate) fn repository(&self) -> &Arc<ConversationRepository> {
        &self.repository
    }

    pub(crate) async fn create_conversation(
        &self,
        record: ConversationRecordV2,
    ) -> Result<ConversationRecordV2> {
        let permit = self.permit(
            record.conversation_id,
            ConversationMutation::MigrationStageCreate,
        );
        self.repository.create_conversation(&permit, record).await
    }

    pub(crate) async fn append_event(
        &self,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
    ) -> Result<ConversationEventRecordV2> {
        let permit = self.permit(conversation_id, ConversationMutation::MigrationStageEvent);
        self.repository
            .append_event(&permit, conversation_id, recorded_at_utc, type_, payload)
            .await
    }

    pub(crate) async fn write_provenance(
        &self,
        conversation_id: ConversationId,
        provenance: ConversationProvenanceFileV1,
    ) -> Result<()> {
        let permit = self.permit(
            conversation_id,
            ConversationMutation::MigrationStageProvenance,
        );
        self.repository
            .write_provenance(&permit, conversation_id, provenance)
            .await
    }

    pub(crate) async fn sync_conversation(&self, conversation_id: ConversationId) -> Result<()> {
        let permit = self.permit(conversation_id, ConversationMutation::MigrationStageSync);
        self.repository
            .sync_conversation(&permit, conversation_id)
            .await
    }
}
