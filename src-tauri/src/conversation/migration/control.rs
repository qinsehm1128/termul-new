//! Durable restart-required control intents for the Conversation layout migration.
//!
//! Live hosts can schedule maintenance, but they never retain the migration lock or mutate the
//! active layout. Bootstrap consumes one pending intent under its kernel lock before opening the
//! repository or admitting managers, PTYs, or routes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};

use super::journal::ApprovalReceiptV1;
use super::lock::{MigrationControlLock, MigrationControlLockGuard};
use super::{MigrationError, MigrationErrorCode, MigrationPhase, MigrationReport, Result};

pub const MIGRATION_MAINTENANCE_FILE: &str = "conversation-layout-v2-maintenance.json";
const MIGRATION_MAINTENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationMaintenanceAction {
    Rollback,
    Reapply,
    Finalize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationMaintenanceRequestV1 {
    pub action: MigrationMaintenanceAction,
    pub request_id: String,
    pub requested_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_receipt: Option<ApprovalReceiptV1>,
}

impl MigrationMaintenanceRequestV1 {
    pub fn validate(&self) -> Result<()> {
        if Uuid::parse_str(&self.request_id).is_err() {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationIdempotencyConflict,
                "validate_maintenance_request",
                "maintenance requestId must be a UUID",
            ));
        }
        match (self.action, self.approval_receipt.as_ref()) {
            (MigrationMaintenanceAction::Finalize, None) => Err(MigrationError::new(
                MigrationErrorCode::MigrationApprovalInvalid,
                "validate_maintenance_request",
                "finalization maintenance requires an approval receipt",
            )),
            (MigrationMaintenanceAction::Finalize, Some(approval))
                if approval.request_id != self.request_id =>
            {
                Err(MigrationError::new(
                    MigrationErrorCode::MigrationApprovalInvalid,
                    "validate_maintenance_request",
                    "finalization approval requestId must match the maintenance requestId",
                ))
            }
            (
                MigrationMaintenanceAction::Rollback | MigrationMaintenanceAction::Reapply,
                Some(_),
            ) => Err(MigrationError::new(
                MigrationErrorCode::MigrationApprovalInvalid,
                "validate_maintenance_request",
                "rollback and reapply maintenance must not carry an approval receipt",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationMaintenanceScheduleReceiptV1 {
    pub action: MigrationMaintenanceAction,
    pub request_id: String,
    pub requested_at_utc: DateTime<Utc>,
    pub restart_required: bool,
    pub code: String,
    pub already_scheduled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationMaintenanceCompletionReceiptV1 {
    pub request: MigrationMaintenanceRequestV1,
    pub completed_at_utc: DateTime<Utc>,
    pub phase: MigrationPhase,
    pub target_generation: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationMaintenanceStateV1 {
    schema_version: u32,
    pending: Option<MigrationMaintenanceRequestV1>,
    completed: BTreeMap<String, MigrationMaintenanceCompletionReceiptV1>,
}

impl Default for MigrationMaintenanceStateV1 {
    fn default() -> Self {
        Self {
            schema_version: MIGRATION_MAINTENANCE_SCHEMA_VERSION,
            pending: None,
            completed: BTreeMap::new(),
        }
    }
}

impl MigrationMaintenanceStateV1 {
    fn validate(&self) -> Result<()> {
        if self.schema_version != MIGRATION_MAINTENANCE_SCHEMA_VERSION {
            return Err(control_corrupt(
                "maintenance state schema version is invalid",
            ));
        }
        if let Some(request) = &self.pending {
            request.validate()?;
        }
        for (request_id, receipt) in &self.completed {
            receipt.request.validate()?;
            if request_id != &receipt.request.request_id {
                return Err(control_corrupt(
                    "completed maintenance receipt key does not match requestId",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ConversationMigrationControlService {
    migration_dir: PathBuf,
    state_path: PathBuf,
    durable_fs: DurableFileSystem,
    request_gate: Mutex<()>,
    control_lock: MigrationControlLock,
}

impl ConversationMigrationControlService {
    pub fn new(host_state_root: &Path) -> Result<Self> {
        let canonical_host_root = host_state_root.canonicalize().map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "create_control_service",
                format!("host-state root cannot be canonicalized: {error}"),
            )
        })?;
        let migration_dir = canonical_host_root.join("conversation-migrations");
        let durable_fs = DurableFileSystem::new();
        durable_fs
            .create_dir_durable(&migration_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "create_control_service",
                    error.to_string(),
                )
            })?;
        Ok(Self {
            state_path: migration_dir.join(MIGRATION_MAINTENANCE_FILE),
            migration_dir,
            durable_fs,
            request_gate: Mutex::new(()),
            control_lock: MigrationControlLock::new(&canonical_host_root)?,
        })
    }

    fn acquire_control_lock(&self) -> Result<MigrationControlLockGuard> {
        self.control_lock.acquire()
    }

    pub fn request(
        &self,
        request: MigrationMaintenanceRequestV1,
    ) -> Result<MigrationMaintenanceScheduleReceiptV1> {
        request.validate()?;
        let _control_lock = self.acquire_control_lock()?;
        let _request_gate = self
            .request_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.load_state()?;

        if let Some(existing) = state.completed.get(&request.request_id) {
            if existing.request != request {
                return Err(idempotency_conflict());
            }
            return Ok(schedule_receipt(&request, true));
        }
        if let Some(existing) = state.pending.as_ref() {
            if existing.request_id == request.request_id {
                if existing != &request {
                    return Err(idempotency_conflict());
                }
                return Ok(schedule_receipt(&request, true));
            }
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationRestartRequired,
                "schedule_maintenance",
                "another maintenance request is already pending for the next bootstrap",
            ));
        }

        state.pending = Some(request.clone());
        self.write_state(&state)?;
        log::warn!(
            "[conversation-migration] restart-required maintenance scheduled request_id={} action={:?}",
            request.request_id,
            request.action
        );
        Ok(schedule_receipt(&request, false))
    }

    pub fn pending(&self) -> Result<Option<MigrationMaintenanceRequestV1>> {
        let _control_lock = self.acquire_control_lock()?;
        let _request_gate = self
            .request_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(self.load_state()?.pending)
    }

    pub fn complete(
        &self,
        request: &MigrationMaintenanceRequestV1,
        report: &MigrationReport,
        completed_at_utc: DateTime<Utc>,
    ) -> Result<()> {
        let _control_lock = self.acquire_control_lock()?;
        let _request_gate = self
            .request_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.load_state()?;
        if let Some(existing) = state.completed.get(&request.request_id) {
            if &existing.request != request {
                return Err(idempotency_conflict());
            }
            if state.pending.as_ref() == Some(request) {
                state.pending = None;
                self.write_state(&state)?;
            }
            return Ok(());
        }
        match state.pending.as_ref() {
            Some(pending) if pending == request => {}
            Some(pending) if pending.request_id == request.request_id => {
                return Err(idempotency_conflict());
            }
            _ => {
                return Err(control_corrupt(
                    "completed maintenance request is not the durable pending request",
                ));
            }
        }
        state.completed.insert(
            request.request_id.clone(),
            MigrationMaintenanceCompletionReceiptV1 {
                request: request.clone(),
                completed_at_utc,
                phase: report.phase,
                target_generation: report.target_generation,
            },
        );
        state.pending = None;
        self.write_state(&state)?;
        log::info!(
            "[conversation-migration] maintenance consumed request_id={} action={:?} phase={:?}",
            request.request_id,
            request.action,
            report.phase
        );
        Ok(())
    }

    fn load_state(&self) -> Result<MigrationMaintenanceStateV1> {
        if !self.state_path.exists() {
            return Ok(MigrationMaintenanceStateV1::default());
        }
        let bytes = fs::read(&self.state_path).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "read_maintenance_state",
                error.to_string(),
            )
        })?;
        let state: MigrationMaintenanceStateV1 =
            serde_json::from_slice(&bytes).map_err(|error| control_corrupt(error.to_string()))?;
        state.validate()?;
        Ok(state)
    }

    fn write_state(&self, state: &MigrationMaintenanceStateV1) -> Result<()> {
        state.validate()?;
        self.durable_fs
            .create_dir_durable(&self.migration_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "write_maintenance_state",
                    error.to_string(),
                )
            })?;
        let mut bytes = serde_json::to_vec_pretty(state).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationJournalCorrupt,
                "write_maintenance_state",
                error.to_string(),
            )
        })?;
        bytes.push(b'\n');
        self.durable_fs
            .replace_bytes(&self.state_path, &bytes)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "write_maintenance_state",
                    error.to_string(),
                )
            })?;
        Ok(())
    }
}

fn schedule_receipt(
    request: &MigrationMaintenanceRequestV1,
    already_scheduled: bool,
) -> MigrationMaintenanceScheduleReceiptV1 {
    MigrationMaintenanceScheduleReceiptV1 {
        action: request.action,
        request_id: request.request_id.clone(),
        requested_at_utc: request.requested_at_utc,
        restart_required: true,
        code: MigrationErrorCode::MigrationRestartRequired
            .as_str()
            .to_string(),
        already_scheduled,
    }
}

fn idempotency_conflict() -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationIdempotencyConflict,
        "schedule_maintenance",
        "maintenance requestId was reused with a different request body",
    )
}

fn control_corrupt(detail: impl Into<String>) -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationJournalCorrupt,
        "validate_maintenance_state",
        detail,
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap()
    }

    fn request(action: MigrationMaintenanceAction) -> MigrationMaintenanceRequestV1 {
        MigrationMaintenanceRequestV1 {
            action,
            request_id: Uuid::new_v4().to_string(),
            requested_at_utc: at(),
            approval_receipt: None,
        }
    }

    #[test]
    fn request_is_immediate_restart_required_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let service = ConversationMigrationControlService::new(temp.path()).unwrap();
        let request = request(MigrationMaintenanceAction::Rollback);
        let first = service.request(request.clone()).unwrap();
        assert!(first.restart_required);
        assert!(!first.already_scheduled);
        assert_eq!(first.code, "MIGRATION_RESTART_REQUIRED");
        assert!(service.state_path.is_file());
        let durable_state: MigrationMaintenanceStateV1 =
            serde_json::from_slice(&fs::read(&service.state_path).unwrap()).unwrap();
        assert_eq!(durable_state.pending, Some(request.clone()));
        assert!(service.request(request.clone()).unwrap().already_scheduled);
        assert_eq!(service.pending().unwrap(), Some(request));
    }

    #[test]
    fn duplicate_request_id_with_changed_body_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let service = ConversationMigrationControlService::new(temp.path()).unwrap();
        let request = request(MigrationMaintenanceAction::Rollback);
        service.request(request.clone()).unwrap();
        let mut changed = request;
        changed.action = MigrationMaintenanceAction::Reapply;
        assert_eq!(
            service.request(changed).unwrap_err().code,
            MigrationErrorCode::MigrationIdempotencyConflict
        );
    }

    #[test]
    fn corrupt_intent_file_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let service = ConversationMigrationControlService::new(temp.path()).unwrap();
        fs::write(&service.state_path, b"not-json").unwrap();
        assert_eq!(
            service.pending().unwrap_err().code,
            MigrationErrorCode::MigrationJournalCorrupt
        );
    }

    #[test]
    fn finalize_requires_matching_approval_and_other_actions_reject_it() {
        let missing_approval = request(MigrationMaintenanceAction::Finalize);
        assert_eq!(
            missing_approval.validate().unwrap_err().code,
            MigrationErrorCode::MigrationApprovalInvalid
        );

        let request_id = Uuid::new_v4().to_string();
        let approval = ApprovalReceiptV1 {
            approver_id: "operator".to_string(),
            action: super::super::FINALIZATION_ACTION.to_string(),
            request_id: request_id.clone(),
            approved_at_utc: at(),
            observed_validation_sha256: "a".repeat(64),
        };
        let finalize = MigrationMaintenanceRequestV1 {
            action: MigrationMaintenanceAction::Finalize,
            request_id,
            requested_at_utc: at(),
            approval_receipt: Some(approval.clone()),
        };
        finalize.validate().unwrap();
        let rollback = MigrationMaintenanceRequestV1 {
            action: MigrationMaintenanceAction::Rollback,
            approval_receipt: Some(approval),
            ..finalize
        };
        assert_eq!(
            rollback.validate().unwrap_err().code,
            MigrationErrorCode::MigrationApprovalInvalid
        );
    }
}
