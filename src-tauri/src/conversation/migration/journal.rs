//! Durable authority for the Conversation layout migration state machine.
//!
//! Phase changes and step receipts are persisted together. Callers must durably sync phase
//! artifacts before invoking [`advance_phase`]; an interrupted handler resumes in the same phase.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{MigrationError, MigrationErrorCode, Result};

pub const MIGRATION_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub const STEP_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const MIGRATION_ID: &str = "conversation-layout-v2";
pub const FINALIZATION_ACTION: &str = "finalizeConversationV2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceReceiptState {
    Started,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceRequestReceiptV1 {
    pub action: String,
    pub request_sha256: String,
    pub state: MaintenanceReceiptState,
    pub started_at_utc: DateTime<Utc>,
    pub completed_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhase {
    Detected,
    Quiescing,
    Inventoried,
    Staging,
    Verifying,
    CutoverPending,
    Committed,
    ObservationWindow,
    RollbackPending,
    RolledBack,
    Finalized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepReceiptV1 {
    pub schema_version: u32,
    pub completed_at_utc: DateTime<Utc>,
    pub output_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapObservationReceiptV1 {
    pub bootstrap_run_id: String,
    pub admitted_at_utc: DateTime<Utc>,
    pub validation_sha256: String,
    #[serde(default)]
    pub control_request_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationEvidenceV1 {
    pub successful_bootstrap_count: u32,
    pub first_observed_at_utc: Option<DateTime<Utc>>,
    pub last_observed_at_utc: Option<DateTime<Utc>>,
    pub validation_sha256: String,
    pub bootstrap_receipts: Vec<BootstrapObservationReceiptV1>,
}

impl ObservationEvidenceV1 {
    #[must_use]
    pub fn empty(validation_sha256: String) -> Self {
        Self {
            successful_bootstrap_count: 0,
            first_observed_at_utc: None,
            last_observed_at_utc: None,
            validation_sha256,
            bootstrap_receipts: Vec::new(),
        }
    }

    pub fn record(&mut self, receipt: BootstrapObservationReceiptV1) -> Result<bool> {
        validate_uuid(
            &receipt.bootstrap_run_id,
            MigrationErrorCode::MigrationObservationInvalid,
        )?;
        validate_sha256(&receipt.validation_sha256)?;
        let distinct_control_ids = receipt.control_request_ids.iter().collect::<HashSet<_>>();
        if distinct_control_ids.len() != receipt.control_request_ids.len()
            || receipt
                .control_request_ids
                .iter()
                .any(|request_id| Uuid::parse_str(request_id).is_err())
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationObservationInvalid,
                "record_observation",
                "bootstrap receipt control request IDs must be distinct UUIDs",
            ));
        }
        if receipt.validation_sha256 != self.validation_sha256 {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationObservationInvalid,
                "record_observation",
                "bootstrap receipt validation digest does not match the current verified layout",
            ));
        }
        if let Some(existing) = self
            .bootstrap_receipts
            .iter()
            .find(|existing| existing.bootstrap_run_id == receipt.bootstrap_run_id)
        {
            if existing == &receipt {
                return Ok(false);
            }
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationIdempotencyConflict,
                "record_observation",
                "bootstrapRunId was reused with different observation output",
            ));
        }
        self.bootstrap_receipts.push(receipt);
        self.bootstrap_receipts.sort_by(|left, right| {
            left.admitted_at_utc
                .cmp(&right.admitted_at_utc)
                .then_with(|| left.bootstrap_run_id.cmp(&right.bootstrap_run_id))
        });
        self.successful_bootstrap_count = self.bootstrap_receipts.len() as u32;
        self.first_observed_at_utc = self
            .bootstrap_receipts
            .first()
            .map(|value| value.admitted_at_utc);
        self.last_observed_at_utc = self
            .bootstrap_receipts
            .last()
            .map(|value| value.admitted_at_utc);
        Ok(true)
    }

    pub fn validate_ready(&self) -> Result<()> {
        let distinct = self
            .bootstrap_receipts
            .iter()
            .map(|receipt| receipt.bootstrap_run_id.as_str())
            .collect::<HashSet<_>>();
        let all_current = self.bootstrap_receipts.iter().all(|receipt| {
            receipt.validation_sha256 == self.validation_sha256
                && valid_sha256(&receipt.validation_sha256)
        });
        let elapsed = self
            .first_observed_at_utc
            .zip(self.last_observed_at_utc)
            .is_some_and(|(first, last)| last >= first + Duration::hours(1));
        if self.successful_bootstrap_count < 2
            || distinct.len() < 2
            || self.successful_bootstrap_count as usize != self.bootstrap_receipts.len()
            || !elapsed
            || !all_current
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationFinalizationNotReady,
                "validate_observation",
                "observation requires two distinct successful bootstraps over at least one hour with the current validation digest",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalReceiptV1 {
    pub approver_id: String,
    pub action: String,
    pub request_id: String,
    pub approved_at_utc: DateTime<Utc>,
    pub observed_validation_sha256: String,
}

impl ApprovalReceiptV1 {
    pub fn validate(&self, evidence: &ObservationEvidenceV1) -> Result<()> {
        if self.approver_id.trim().is_empty()
            || self.action != FINALIZATION_ACTION
            || Uuid::parse_str(&self.request_id).is_err()
            || !valid_sha256(&self.observed_validation_sha256)
            || self.observed_validation_sha256 != evidence.validation_sha256
            || evidence
                .last_observed_at_utc
                .is_none_or(|last| self.approved_at_utc < last)
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationApprovalInvalid,
                "validate_approval",
                "approval identity, request, action, digest, or timestamp is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationJournalV1 {
    pub schema_version: u32,
    pub migration_id: String,
    pub operation_id: Uuid,
    pub operation_key: String,
    pub phase: MigrationPhase,
    pub source_layout: String,
    pub target_layout: String,
    pub target_generation: Uuid,
    pub inventory_sha256: Option<String>,
    pub staged_manifest_sha256: Option<String>,
    pub completed_steps: BTreeMap<String, StepReceiptV1>,
    pub attempt: u32,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
    pub last_error: Option<String>,
    pub observation_evidence: Option<ObservationEvidenceV1>,
    pub approval_receipt: Option<ApprovalReceiptV1>,
    #[serde(default)]
    pub maintenance_request_receipts: BTreeMap<String, MaintenanceRequestReceiptV1>,
}

impl MigrationJournalV1 {
    #[must_use]
    pub fn new(operation_key: String, now_utc: DateTime<Utc>) -> Self {
        Self {
            schema_version: MIGRATION_JOURNAL_SCHEMA_VERSION,
            migration_id: MIGRATION_ID.to_string(),
            operation_id: Uuid::new_v4(),
            operation_key,
            phase: MigrationPhase::Detected,
            source_layout: "legacy_v1".to_string(),
            target_layout: "conversation_v2".to_string(),
            target_generation: Uuid::new_v4(),
            inventory_sha256: None,
            staged_manifest_sha256: None,
            completed_steps: BTreeMap::new(),
            attempt: 1,
            created_at_utc: now_utc,
            updated_at_utc: now_utc,
            last_error: None,
            observation_evidence: None,
            approval_receipt: None,
            maintenance_request_receipts: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != MIGRATION_JOURNAL_SCHEMA_VERSION
            || self.migration_id != MIGRATION_ID
            || !valid_sha256(&self.operation_key)
            || self.source_layout != "legacy_v1"
            || self.target_layout != "conversation_v2"
            || self.attempt == 0
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "validate_journal",
                "journal schema, identity, operation key, layout, or attempt is invalid",
            ));
        }
        for (key, receipt) in &self.completed_steps {
            if key.trim().is_empty()
                || receipt.schema_version != STEP_RECEIPT_SCHEMA_VERSION
                || !valid_sha256(&receipt.output_sha256)
            {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationJournalCorrupt,
                    "validate_journal",
                    "journal contains an invalid step receipt",
                ));
            }
        }
        if let Some(value) = &self.inventory_sha256 {
            validate_sha256(value)?;
        }
        if let Some(value) = &self.staged_manifest_sha256 {
            validate_sha256(value)?;
        }
        for (request_id, receipt) in &self.maintenance_request_receipts {
            if Uuid::parse_str(request_id).is_err()
                || receipt.action.trim().is_empty()
                || !valid_sha256(&receipt.request_sha256)
                || (receipt.state == MaintenanceReceiptState::Completed
                    && receipt.completed_at_utc.is_none())
                || (receipt.state == MaintenanceReceiptState::Started
                    && receipt.completed_at_utc.is_some())
            {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationJournalCorrupt,
                    "validate_journal",
                    "journal contains an invalid maintenance request receipt",
                ));
            }
        }
        Ok(())
    }

    pub fn record_step(
        &mut self,
        key: String,
        output_sha256: String,
        completed_at_utc: DateTime<Utc>,
    ) -> Result<bool> {
        if key.trim().is_empty() {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "record_step",
                "step key is blank",
            ));
        }
        validate_sha256(&output_sha256)?;
        if let Some(existing) = self.completed_steps.get(&key) {
            if existing.output_sha256 == output_sha256 {
                return Ok(false);
            }
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationIdempotencyConflict,
                "record_step",
                "completed step key has a different output digest",
            ));
        }
        self.completed_steps.insert(
            key,
            StepReceiptV1 {
                schema_version: STEP_RECEIPT_SCHEMA_VERSION,
                completed_at_utc,
                output_sha256,
            },
        );
        self.updated_at_utc = completed_at_utc;
        self.last_error = None;
        Ok(true)
    }

    #[must_use]
    pub fn receipt_with_prefix(&self, prefix: &str) -> Option<(&String, &StepReceiptV1)> {
        self.completed_steps
            .iter()
            .find(|(key, _)| key.starts_with(prefix))
    }

    pub fn record_failure(&mut self, error: &MigrationError, now_utc: DateTime<Utc>) {
        self.attempt = self.attempt.saturating_add(1);
        self.updated_at_utc = now_utc;
        self.last_error = Some(format!("{}: {}", error.code.as_str(), error.detail));
    }
}

pub fn advance_phase(
    journal: &mut MigrationJournalV1,
    next: MigrationPhase,
    now_utc: DateTime<Utc>,
    has_explicit_approval: bool,
) -> Result<()> {
    let allowed = matches!(
        (journal.phase, next),
        (MigrationPhase::Detected, MigrationPhase::Quiescing)
            | (MigrationPhase::Quiescing, MigrationPhase::Inventoried)
            | (MigrationPhase::Inventoried, MigrationPhase::Staging)
            | (MigrationPhase::Staging, MigrationPhase::Verifying)
            | (MigrationPhase::Verifying, MigrationPhase::CutoverPending)
            | (MigrationPhase::CutoverPending, MigrationPhase::Committed)
            | (MigrationPhase::Committed, MigrationPhase::ObservationWindow)
            | (MigrationPhase::Committed, MigrationPhase::RollbackPending)
            | (
                MigrationPhase::ObservationWindow,
                MigrationPhase::RollbackPending
            )
            | (MigrationPhase::RollbackPending, MigrationPhase::RolledBack)
            | (MigrationPhase::RolledBack, MigrationPhase::Verifying)
    ) || (journal.phase == MigrationPhase::ObservationWindow
        && next == MigrationPhase::Finalized
        && has_explicit_approval);
    if !allowed {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationIllegalTransition,
            "advance_phase",
            format!(
                "illegal migration transition {:?} -> {:?}",
                journal.phase, next
            ),
        ));
    }
    journal.phase = next;
    journal.updated_at_utc = now_utc;
    journal.last_error = None;
    Ok(())
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn validate_sha256(value: &str) -> Result<()> {
    if valid_sha256(value) {
        Ok(())
    } else {
        Err(MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "validate_sha256",
            "digest must be 64 lowercase hexadecimal characters",
        ))
    }
}

fn validate_uuid(value: &str, code: MigrationErrorCode) -> Result<()> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| MigrationError::new(code, "validate_uuid", "identifier must be a UUID"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn transition_table_accepts_only_declared_edges_and_requires_finalize_approval() {
        use MigrationPhase::*;
        let phases = [
            Detected,
            Quiescing,
            Inventoried,
            Staging,
            Verifying,
            CutoverPending,
            Committed,
            ObservationWindow,
            RollbackPending,
            RolledBack,
            Finalized,
        ];
        let allowed = [
            (Detected, Quiescing),
            (Quiescing, Inventoried),
            (Inventoried, Staging),
            (Staging, Verifying),
            (Verifying, CutoverPending),
            (CutoverPending, Committed),
            (Committed, ObservationWindow),
            (Committed, RollbackPending),
            (ObservationWindow, RollbackPending),
            (RollbackPending, RolledBack),
            (RolledBack, Verifying),
        ];
        for current in phases {
            for next in phases {
                let result = advance_phase(&mut journal(current), next, now(), false);
                assert_eq!(
                    result.is_ok(),
                    allowed.contains(&(current, next)),
                    "unexpected transition result for {current:?} -> {next:?}"
                );
            }
        }
        assert!(advance_phase(&mut journal(ObservationWindow), Finalized, now(), false).is_err());
        assert!(advance_phase(&mut journal(ObservationWindow), Finalized, now(), true).is_ok());
    }

    #[test]
    fn receipts_are_idempotent_and_digest_collisions_fail_closed() {
        let mut journal = journal(MigrationPhase::Staging);
        let key = format!("{}:stage:item:source", journal.operation_key);
        assert!(journal
            .record_step(key.clone(), "b".repeat(64), now())
            .unwrap());
        assert!(!journal
            .record_step(key.clone(), "b".repeat(64), now())
            .unwrap());
        let error = journal.record_step(key, "c".repeat(64), now()).unwrap_err();
        assert_eq!(error.code, MigrationErrorCode::MigrationIdempotencyConflict);
    }
}
