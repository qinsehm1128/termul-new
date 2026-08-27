//! Generation-stamped reader layout pointer and restart reconciliation rules.
//!
//! The descriptor is the only cutover authority. Reconciliation is deliberately non-destructive:
//! commit restores the verified v2 target, while rollback selects hybrid legacy-first reads and
//! leaves both namespaces intact.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::journal::{MigrationJournalV1, MigrationPhase};
use super::{MigrationError, MigrationErrorCode, Result};

pub const LAYOUT_DESCRIPTOR_SCHEMA_VERSION: u32 = 1;
pub const LAYOUT_DESCRIPTOR_FILE: &str = "conversation-layout.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveLayout {
    LegacyV1,
    ConversationV2,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderPrecedence {
    LegacyOnly,
    ConversationV2First,
    HybridLegacyFirst,
    ConversationV2Only,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLayoutDescriptorV1 {
    pub schema_version: u32,
    pub generation: Uuid,
    pub active_layout: ActiveLayout,
    pub reader_precedence: ReaderPrecedence,
    pub v2_root: String,
    pub migration_operation_id: Option<Uuid>,
    pub updated_at_utc: DateTime<Utc>,
}

impl ConversationLayoutDescriptorV1 {
    #[must_use]
    pub fn legacy(host_root: &Path, now_utc: DateTime<Utc>) -> Self {
        Self {
            schema_version: LAYOUT_DESCRIPTOR_SCHEMA_VERSION,
            generation: Uuid::new_v4(),
            active_layout: ActiveLayout::LegacyV1,
            reader_precedence: ReaderPrecedence::LegacyOnly,
            v2_root: host_root
                .join("conversations")
                .join("v2")
                .to_string_lossy()
                .into_owned(),
            migration_operation_id: None,
            updated_at_utc: now_utc,
        }
    }

    #[must_use]
    pub fn target(journal: &MigrationJournalV1, host_root: &Path, now_utc: DateTime<Utc>) -> Self {
        Self {
            schema_version: LAYOUT_DESCRIPTOR_SCHEMA_VERSION,
            generation: journal.target_generation,
            active_layout: ActiveLayout::ConversationV2,
            reader_precedence: ReaderPrecedence::ConversationV2First,
            v2_root: host_root
                .join("conversations")
                .join("v2")
                .to_string_lossy()
                .into_owned(),
            migration_operation_id: Some(journal.operation_id),
            updated_at_utc: now_utc,
        }
    }

    #[must_use]
    pub fn hybrid(journal: &MigrationJournalV1, host_root: &Path, now_utc: DateTime<Utc>) -> Self {
        Self {
            schema_version: LAYOUT_DESCRIPTOR_SCHEMA_VERSION,
            generation: journal.target_generation,
            active_layout: ActiveLayout::Hybrid,
            reader_precedence: ReaderPrecedence::HybridLegacyFirst,
            v2_root: host_root
                .join("conversations")
                .join("v2")
                .to_string_lossy()
                .into_owned(),
            migration_operation_id: Some(journal.operation_id),
            updated_at_utc: now_utc,
        }
    }

    #[must_use]
    pub fn finalized(
        journal: &MigrationJournalV1,
        host_root: &Path,
        now_utc: DateTime<Utc>,
    ) -> Self {
        Self {
            reader_precedence: ReaderPrecedence::ConversationV2Only,
            ..Self::target(journal, host_root, now_utc)
        }
    }

    pub fn validate_for_host(&self, host_root: &Path) -> Result<()> {
        let expected_v2_root = host_root.join("conversations").join("v2");
        let valid_pair = matches!(
            (self.active_layout, self.reader_precedence),
            (ActiveLayout::LegacyV1, ReaderPrecedence::LegacyOnly)
                | (
                    ActiveLayout::ConversationV2,
                    ReaderPrecedence::ConversationV2First
                )
                | (
                    ActiveLayout::ConversationV2,
                    ReaderPrecedence::ConversationV2Only
                )
                | (ActiveLayout::Hybrid, ReaderPrecedence::HybridLegacyFirst)
        );
        if self.schema_version != LAYOUT_DESCRIPTOR_SCHEMA_VERSION
            || Path::new(&self.v2_root) != expected_v2_root
            || !valid_pair
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationLayoutCorrupt,
                "validate_layout",
                "layout schema, v2 root, or reader policy is invalid",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn is_target_for(&self, journal: &MigrationJournalV1) -> bool {
        self.generation == journal.target_generation
            && self.migration_operation_id == Some(journal.operation_id)
            && self.active_layout == ActiveLayout::ConversationV2
            && matches!(
                self.reader_precedence,
                ReaderPrecedence::ConversationV2First | ReaderPrecedence::ConversationV2Only
            )
    }

    #[must_use]
    pub fn is_cutover_target_for(&self, journal: &MigrationJournalV1) -> bool {
        self.is_target_for(journal)
            && self.reader_precedence == ReaderPrecedence::ConversationV2First
    }

    #[must_use]
    pub fn is_hybrid_for(&self, journal: &MigrationJournalV1) -> bool {
        self.generation == journal.target_generation
            && self.migration_operation_id == Some(journal.operation_id)
            && self.active_layout == ActiveLayout::Hybrid
            && self.reader_precedence == ReaderPrecedence::HybridLegacyFirst
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutoverRecovery {
    NoAction,
    WriteTargetThenCommit,
    CompleteCommit,
    RestoreCommittedTarget,
    WriteHybridThenRollback,
    CompleteRollback,
    RestoreFinalizedTarget,
}

#[must_use]
pub fn recover_cutover(
    journal: &MigrationJournalV1,
    descriptor: &ConversationLayoutDescriptorV1,
) -> CutoverRecovery {
    match journal.phase {
        MigrationPhase::CutoverPending if descriptor.is_cutover_target_for(journal) => {
            CutoverRecovery::CompleteCommit
        }
        MigrationPhase::CutoverPending => CutoverRecovery::WriteTargetThenCommit,
        MigrationPhase::Committed | MigrationPhase::ObservationWindow
            if !descriptor.is_cutover_target_for(journal) =>
        {
            CutoverRecovery::RestoreCommittedTarget
        }
        MigrationPhase::RollbackPending if descriptor.is_hybrid_for(journal) => {
            CutoverRecovery::CompleteRollback
        }
        MigrationPhase::RollbackPending => CutoverRecovery::WriteHybridThenRollback,
        MigrationPhase::Finalized
            if descriptor.reader_precedence != ReaderPrecedence::ConversationV2Only
                || !descriptor.is_target_for(journal) =>
        {
            CutoverRecovery::RestoreFinalizedTarget
        }
        _ => CutoverRecovery::NoAction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::migration::journal::MigrationJournalV1;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-15T10:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn journal(phase: MigrationPhase) -> MigrationJournalV1 {
        let mut journal = MigrationJournalV1::new("a".repeat(64), now());
        journal.phase = phase;
        journal
    }

    #[test]
    fn cutover_pending_retries_old_pointer_and_completes_target_pointer() {
        let root = Path::new("/host");
        let pending = journal(MigrationPhase::CutoverPending);
        let old = ConversationLayoutDescriptorV1::legacy(root, now());
        assert_eq!(
            recover_cutover(&pending, &old),
            CutoverRecovery::WriteTargetThenCommit
        );
        let target = ConversationLayoutDescriptorV1::target(&pending, root, now());
        assert_eq!(
            recover_cutover(&pending, &target),
            CutoverRecovery::CompleteCommit
        );
    }

    #[test]
    fn committed_is_commit_wins_and_rollback_pending_is_hybrid_wins() {
        let root = Path::new("/host");
        let committed = journal(MigrationPhase::Committed);
        let old = ConversationLayoutDescriptorV1::legacy(root, now());
        assert_eq!(
            recover_cutover(&committed, &old),
            CutoverRecovery::RestoreCommittedTarget
        );

        let rollback = journal(MigrationPhase::RollbackPending);
        let hybrid = ConversationLayoutDescriptorV1::hybrid(&rollback, root, now());
        assert_eq!(
            recover_cutover(&rollback, &hybrid),
            CutoverRecovery::CompleteRollback
        );
        assert_eq!(
            recover_cutover(&rollback, &old),
            CutoverRecovery::WriteHybridThenRollback
        );
    }

    #[test]
    fn reader_policy_serializes_to_exact_wire_values() {
        assert_eq!(
            serde_json::to_value(ReaderPrecedence::HybridLegacyFirst).unwrap(),
            "hybrid_legacy_first"
        );
        assert_eq!(
            serde_json::to_value(ReaderPrecedence::ConversationV2Only).unwrap(),
            "conversation_v2_only"
        );
    }
}
