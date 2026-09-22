//! ACP-local durable journal for in-flight cross-Core lifecycle operations.
//!
//! This store is operational metadata only. Canonical Conversation records, the
//! event log, and SessionWorkspace remain the business authorities; this journal
//! never becomes a second Conversation persistence root and never issues
//! destructive cleanup on its own. Unknown or corrupt records are diagnosed and
//! fail closed.
//!
//! Layout under the existing profile/state root:
//! `{profile_root}/acp-lifecycle/v1/operations/{operation-id}.json`

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::contracts::ConversationId;
use crate::conversation::durable_fs::{
    DirectoryPermissions, DurableFileSystem, DurableFsError, DurableWriteOutcome,
};

pub const LIFECYCLE_OPERATION_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub const CAPTURED_TERMINAL_SCOPE_SCHEMA_VERSION: u32 = 1;
pub const DURABLE_TERMINAL_SPAWN_INTENT_SCHEMA_VERSION: u32 = 1;
pub const LIFECYCLE_JOURNAL_ROOT_DIR: &str = "acp-lifecycle";
pub const LIFECYCLE_JOURNAL_VERSION_DIR: &str = "v1";
pub const LIFECYCLE_JOURNAL_OPERATIONS_DIR: &str = "operations";
pub const MAX_LIFECYCLE_JOURNAL_RECORD_BYTES: usize = 64 * 1024;
pub const MAX_LIFECYCLE_JOURNAL_OPERATIONS: usize = 10_000;

/// Dedicated UUID v5 namespace for deterministic lifecycle operation IDs.
/// This is not a Conversation ID and must not be reused as business identity.
pub const LIFECYCLE_OPERATION_ID_NAMESPACE: Uuid =
    uuid::uuid!("3c8f0d2a-6b91-5a47-8e2c-19d4f7a0b6c1");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleJournalErrorCode {
    LifecycleJournalCorrupt,
    LifecycleJournalSchemaInvalid,
    LifecycleJournalUnknownRecord,
    LifecycleJournalDurabilityFailed,
    LifecycleJournalIoFailed,
    LifecycleJournalBoundExceeded,
}

impl LifecycleJournalErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LifecycleJournalCorrupt => "LIFECYCLE_JOURNAL_CORRUPT",
            Self::LifecycleJournalSchemaInvalid => "LIFECYCLE_JOURNAL_SCHEMA_INVALID",
            Self::LifecycleJournalUnknownRecord => "LIFECYCLE_JOURNAL_UNKNOWN_RECORD",
            Self::LifecycleJournalDurabilityFailed => "LIFECYCLE_JOURNAL_DURABILITY_FAILED",
            Self::LifecycleJournalIoFailed => "LIFECYCLE_JOURNAL_IO_FAILED",
            Self::LifecycleJournalBoundExceeded => "LIFECYCLE_JOURNAL_BOUND_EXCEEDED",
        }
    }
}

#[derive(Debug)]
pub struct LifecycleJournalError {
    pub code: LifecycleJournalErrorCode,
    pub operation: &'static str,
    pub conversation_id: Option<ConversationId>,
    pub operation_id: Option<Uuid>,
    pub detail: String,
}

impl LifecycleJournalError {
    fn new(
        code: LifecycleJournalErrorCode,
        operation: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation,
            conversation_id: None,
            operation_id: None,
            detail: detail.into(),
        }
    }

    #[must_use]
    fn with_ids(
        mut self,
        operation_id: Option<Uuid>,
        conversation_id: Option<ConversationId>,
    ) -> Self {
        self.operation_id = operation_id;
        self.conversation_id = conversation_id;
        self
    }
}

impl fmt::Display for LifecycleJournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for LifecycleJournalError {}

pub type Result<T> = std::result::Result<T, LifecycleJournalError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleOperationKind {
    DeleteConversation,
    RecreateTerminal,
}

impl LifecycleOperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeleteConversation => "deleteConversation",
            Self::RecreateTerminal => "recreateTerminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOperationPhase {
    CaptureScope,
    Observe,
    TerminatePending,
    TerminationConfirmed,
    PurgePending,
    RecreatePending,
    Recreated,
    Completed,
}

impl LifecycleOperationPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CaptureScope => "capture_scope",
            Self::Observe => "observe",
            Self::TerminatePending => "terminate_pending",
            Self::TerminationConfirmed => "termination_confirmed",
            Self::PurgePending => "purge_pending",
            Self::RecreatePending => "recreate_pending",
            Self::Recreated => "recreated",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOperationStatus {
    InFlight,
    NeedsRecovery,
    Blocked,
    Completed,
    FailedClosed,
}

impl LifecycleOperationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::NeedsRecovery => "needs_recovery",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::FailedClosed => "failed_closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedTerminalScopeV1 {
    pub schema_version: u32,
    pub terminal_ids: Vec<String>,
}

/// Host-owned cwd selector. Raw filesystem cwd is never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DurableTerminalCwdSource {
    Workspace,
    ExecutionTarget,
}

/// Host default shell only. Raw program paths and argv are never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalProgramPolicyV1 {
    HostDefaultShell,
}

/// Inherited host environment only. Raw env maps are never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalEnvironmentPolicyV1 {
    HostInherited,
}

/// Conversation-scoped interactive terminal. SSH/project kinds are not recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalKindPolicyV1 {
    ConversationInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalRecoveryState {
    Active,
    Lost,
    Recreating,
    Recreated,
}

impl TerminalRecoveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Lost => "lost",
            Self::Recreating => "recreating",
            Self::Recreated => "recreated",
        }
    }
}

/// Policy-safe spawn intent. Claims, environment values, and raw cwd never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableTerminalSpawnIntentV1 {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub cwd_source: DurableTerminalCwdSource,
    pub cols: u16,
    pub rows: u16,
    pub program_policy: TerminalProgramPolicyV1,
    pub env_policy: TerminalEnvironmentPolicyV1,
    pub kind: TerminalKindPolicyV1,
}

impl DurableTerminalSpawnIntentV1 {
    pub fn validate(&self, operation: &'static str) -> Result<()> {
        if self.schema_version != DURABLE_TERMINAL_SPAWN_INTENT_SCHEMA_VERSION {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid,
                operation,
                format!(
                    "unsupported spawn intent schema version {}",
                    self.schema_version
                ),
            )
            .with_ids(None, Some(self.conversation_id)));
        }
        if self.cols == 0 || self.rows == 0 {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                operation,
                "spawn intent dimensions must be greater than zero",
            )
            .with_ids(None, Some(self.conversation_id)));
        }
        if self
            .project_id
            .as_ref()
            .is_some_and(|project_id| project_id.trim().is_empty())
        {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                operation,
                "spawn intent project id must not be blank",
            )
            .with_ids(None, Some(self.conversation_id)));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleOperationRecordV1 {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub kind: LifecycleOperationKind,
    pub conversation_id: ConversationId,
    pub expected_revision: u64,
    pub captured_terminal_scope: CapturedTerminalScopeV1,
    pub phase: LifecycleOperationPhase,
    pub status: LifecycleOperationStatus,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_intent: Option<DurableTerminalSpawnIntentV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_recovery_state: Option<TerminalRecoveryState>,
}

impl LifecycleOperationRecordV1 {
    pub fn start(
        kind: LifecycleOperationKind,
        conversation_id: ConversationId,
        expected_revision: u64,
        captured_terminal_ids: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let record = Self {
            schema_version: LIFECYCLE_OPERATION_JOURNAL_SCHEMA_VERSION,
            operation_id: deterministic_operation_id(kind, conversation_id),
            kind,
            conversation_id,
            expected_revision,
            captured_terminal_scope: CapturedTerminalScopeV1 {
                schema_version: CAPTURED_TERMINAL_SCOPE_SCHEMA_VERSION,
                terminal_ids: normalize_terminal_ids(captured_terminal_ids),
            },
            phase: LifecycleOperationPhase::CaptureScope,
            status: LifecycleOperationStatus::InFlight,
            attempts: 1,
            last_error_code: None,
            created_at_utc: now,
            updated_at_utc: now,
            spawn_intent: None,
            replacement_terminal_id: None,
            terminal_recovery_state: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn start_recreate(
        conversation_id: ConversationId,
        expected_revision: u64,
        terminal_id: String,
        spawn_intent: DurableTerminalSpawnIntentV1,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        spawn_intent.validate("start_recreate")?;
        if spawn_intent.conversation_id != conversation_id {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "start_recreate",
                "spawn intent conversation does not match the journaled Conversation",
            )
            .with_ids(None, Some(conversation_id)));
        }
        let record = Self {
            schema_version: LIFECYCLE_OPERATION_JOURNAL_SCHEMA_VERSION,
            operation_id: deterministic_recreate_operation_id(conversation_id, &terminal_id),
            kind: LifecycleOperationKind::RecreateTerminal,
            conversation_id,
            expected_revision,
            captured_terminal_scope: CapturedTerminalScopeV1 {
                schema_version: CAPTURED_TERMINAL_SCOPE_SCHEMA_VERSION,
                terminal_ids: normalize_terminal_ids(vec![terminal_id]),
            },
            phase: LifecycleOperationPhase::CaptureScope,
            status: LifecycleOperationStatus::InFlight,
            attempts: 1,
            last_error_code: None,
            created_at_utc: now,
            updated_at_utc: now,
            spawn_intent: Some(spawn_intent),
            replacement_terminal_id: None,
            terminal_recovery_state: Some(TerminalRecoveryState::Active),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn advance(
        &mut self,
        phase: LifecycleOperationPhase,
        status: LifecycleOperationStatus,
        last_error_code: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.phase = phase;
        self.status = status;
        self.last_error_code = last_error_code;
        self.updated_at_utc = now;
        self.validate()
    }

    pub fn bump_attempt(&mut self, now: DateTime<Utc>) {
        self.attempts = self.attempts.saturating_add(1);
        self.updated_at_utc = now;
    }

    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        self.status != LifecycleOperationStatus::Completed
            && self.phase != LifecycleOperationPhase::Completed
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LIFECYCLE_OPERATION_JOURNAL_SCHEMA_VERSION {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid,
                "validate",
                format!("unsupported journal schema version {}", self.schema_version),
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if self.attempts == 0 {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "attempts must be at least 1",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if self.updated_at_utc < self.created_at_utc {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "updatedAtUtc precedes createdAtUtc",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if self.captured_terminal_scope.schema_version != CAPTURED_TERMINAL_SCOPE_SCHEMA_VERSION {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid,
                "validate",
                "unsupported captured terminal scope schema version",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if self
            .captured_terminal_scope
            .terminal_ids
            .iter()
            .any(|terminal_id| terminal_id.trim().is_empty())
        {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "captured terminal scope contains a blank terminal id",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        let unique = self
            .captured_terminal_scope
            .terminal_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if unique.len() != self.captured_terminal_scope.terminal_ids.len() {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "captured terminal scope contains duplicate terminal ids",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if let Some(intent) = &self.spawn_intent {
            intent.validate("validate")?;
            if intent.conversation_id != self.conversation_id {
                return Err(LifecycleJournalError::new(
                    LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                    "validate",
                    "spawn intent conversation does not match the journaled Conversation",
                )
                .with_ids(Some(self.operation_id), Some(self.conversation_id)));
            }
        }
        if let Some(replacement_terminal_id) = &self.replacement_terminal_id {
            if replacement_terminal_id.trim().is_empty() {
                return Err(LifecycleJournalError::new(
                    LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                    "validate",
                    "replacement terminal id must not be blank",
                )
                .with_ids(Some(self.operation_id), Some(self.conversation_id)));
            }
        }
        if self.kind == LifecycleOperationKind::DeleteConversation
            && (self.spawn_intent.is_some()
                || self.replacement_terminal_id.is_some()
                || self.terminal_recovery_state.is_some())
        {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "delete operations cannot carry terminal recreate intent",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if let Some(state) = self.terminal_recovery_state {
            if !recovery_state_allowed_for_phase(self.phase, state) {
                return Err(LifecycleJournalError::new(
                    LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                    "validate",
                    format!(
                        "recovery state {} is not valid for phase {}",
                        state.as_str(),
                        self.phase.as_str()
                    ),
                )
                .with_ids(Some(self.operation_id), Some(self.conversation_id)));
            }
        }
        if let Some(code) = &self.last_error_code {
            if code.trim().is_empty()
                || code
                    .chars()
                    .any(|ch| !(ch.is_ascii_uppercase() || ch == '_'))
            {
                return Err(LifecycleJournalError::new(
                    LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                    "validate",
                    "last error code must be a stable SCREAMING_SNAKE_CASE token",
                )
                .with_ids(Some(self.operation_id), Some(self.conversation_id)));
            }
        }
        if !phase_allowed_for_kind(self.kind, self.phase) {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                format!(
                    "phase {} is not valid for kind {}",
                    self.phase.as_str(),
                    self.kind.as_str()
                ),
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        let completed_phase = self.phase == LifecycleOperationPhase::Completed;
        let completed_status = self.status == LifecycleOperationStatus::Completed;
        if completed_phase != completed_status {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "completed phase and completed status must be set together",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        if completed_status && self.last_error_code.is_some() {
            return Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalCorrupt,
                "validate",
                "completed operations cannot retain a last error code",
            )
            .with_ids(Some(self.operation_id), Some(self.conversation_id)));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleJournalDiagnostic {
    pub code: LifecycleJournalErrorCode,
    pub operation_id: Option<Uuid>,
    pub file_name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleJournalStartupScan {
    pub incomplete: Vec<LifecycleOperationRecordV1>,
    pub diagnostics: Vec<LifecycleJournalDiagnostic>,
}

impl LifecycleJournalStartupScan {
    #[must_use]
    pub fn has_corrupt_records(&self) -> bool {
        self.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                LifecycleJournalErrorCode::LifecycleJournalCorrupt
                    | LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid
                    | LifecycleJournalErrorCode::LifecycleJournalUnknownRecord
            )
        })
    }
}

#[derive(Debug)]
pub struct LifecycleOperationJournal {
    operations_dir: PathBuf,
    durable_fs: DurableFileSystem,
    lock: Mutex<()>,
}

impl LifecycleOperationJournal {
    /// Open or create the ACP-local journal under the existing profile/state root.
    pub fn open(profile_root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_fs(profile_root, DurableFileSystem::new())
    }

    pub fn open_with_fs(
        profile_root: impl AsRef<Path>,
        durable_fs: DurableFileSystem,
    ) -> Result<Self> {
        let profile_root = profile_root.as_ref();
        let canonical =
            fs::canonicalize(profile_root).unwrap_or_else(|_| profile_root.to_path_buf());
        let operations_dir = operations_dir_for(&canonical);
        durable_fs
            .create_dir_durable(&operations_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| map_durable_error("open", error))?;
        Ok(Self {
            operations_dir,
            durable_fs,
            lock: Mutex::new(()),
        })
    }

    #[must_use]
    pub fn operations_dir(&self) -> &Path {
        &self.operations_dir
    }

    pub fn persist(&self, record: &LifecycleOperationRecordV1) -> Result<DurableWriteOutcome> {
        record.validate()?;
        let bytes = encode_record(record)?;
        let _guard = lock_journal(&self.lock, "persist")?;
        let path = self.record_path(record.operation_id);
        self.durable_fs
            .replace_bytes(&path, &bytes)
            .map_err(|error| {
                map_durable_error("persist", error)
                    .with_ids(Some(record.operation_id), Some(record.conversation_id))
            })
    }

    pub fn load(&self, operation_id: Uuid) -> Result<Option<LifecycleOperationRecordV1>> {
        let _guard = lock_journal(&self.lock, "load")?;
        self.load_unlocked(operation_id)
    }

    pub fn list_incomplete(&self) -> Result<Vec<LifecycleOperationRecordV1>> {
        Ok(self.scan_startup()?.incomplete)
    }

    /// Startup recovery hook: enumerate unfinished operations and diagnose
    /// unknown/corrupt records without deleting files or guessing cleanup.
    pub fn scan_startup(&self) -> Result<LifecycleJournalStartupScan> {
        let _guard = lock_journal(&self.lock, "scan_startup")?;
        let mut incomplete = Vec::new();
        let mut diagnostics = Vec::new();
        let entries = match fs::read_dir(&self.operations_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LifecycleJournalStartupScan {
                    incomplete,
                    diagnostics,
                });
            }
            Err(error) => {
                return Err(io_error("scan_startup", error));
            }
        };

        let mut seen = 0usize;
        for entry in entries {
            let entry = entry.map_err(|error| io_error("scan_startup", error))?;
            seen += 1;
            if seen > MAX_LIFECYCLE_JOURNAL_OPERATIONS {
                return Err(LifecycleJournalError::new(
                    LifecycleJournalErrorCode::LifecycleJournalBoundExceeded,
                    "scan_startup",
                    format!(
                        "operation journal exceeded MAX_LIFECYCLE_JOURNAL_OPERATIONS={MAX_LIFECYCLE_JOURNAL_OPERATIONS}"
                    ),
                ));
            }
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                diagnostics.push(diagnostic(
                    LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                    None,
                    file_name.to_string_lossy().into_owned(),
                    "journal entry name is not valid UTF-8",
                ));
                continue;
            };
            if is_owned_temp_name(name) {
                continue;
            }
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(diagnostic(
                        LifecycleJournalErrorCode::LifecycleJournalIoFailed,
                        None,
                        name.to_string(),
                        format!("journal entry metadata could not be read: {error}"),
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                diagnostics.push(diagnostic(
                    LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                    None,
                    name.to_string(),
                    "journal entry is a symlink and was not followed",
                ));
                continue;
            }
            if !metadata.is_file() {
                diagnostics.push(diagnostic(
                    LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                    None,
                    name.to_string(),
                    "journal entry is not a regular file",
                ));
                continue;
            }
            let Some(file_operation_id) = parse_operation_filename(name) else {
                diagnostics.push(diagnostic(
                    LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                    None,
                    name.to_string(),
                    "journal file name is not a canonical operation id",
                ));
                continue;
            };
            match decode_record_file(&path, file_operation_id) {
                Ok(record) => {
                    if record.is_incomplete() {
                        incomplete.push(record);
                    }
                }
                Err(error) => {
                    log::error!(
                        "[acp-lifecycle-journal] operation=scan_startup stable_code={} operation_id={} detail={}",
                        error.code.as_str(),
                        file_operation_id,
                        error.detail
                    );
                    diagnostics.push(diagnostic(
                        error.code,
                        Some(file_operation_id),
                        name.to_string(),
                        error.detail,
                    ));
                }
            }
        }

        incomplete.sort_by(|left, right| {
            left.updated_at_utc
                .cmp(&right.updated_at_utc)
                .then_with(|| left.operation_id.cmp(&right.operation_id))
        });
        diagnostics.sort_by(|left, right| {
            left.file_name
                .cmp(&right.file_name)
                .then_with(|| left.code.as_str().cmp(right.code.as_str()))
        });
        Ok(LifecycleJournalStartupScan {
            incomplete,
            diagnostics,
        })
    }

    fn load_unlocked(&self, operation_id: Uuid) -> Result<Option<LifecycleOperationRecordV1>> {
        let path = self.record_path(operation_id);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                "load",
                "journal record path is a symlink and was not followed",
            )
            .with_ids(Some(operation_id), None)),
            Ok(metadata) if metadata.is_file() => decode_record_file(&path, operation_id).map(Some),
            Ok(_) => Err(LifecycleJournalError::new(
                LifecycleJournalErrorCode::LifecycleJournalUnknownRecord,
                "load",
                "journal record path is not a regular file",
            )
            .with_ids(Some(operation_id), None)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("load", error).with_ids(Some(operation_id), None)),
        }
    }

    fn record_path(&self, operation_id: Uuid) -> PathBuf {
        self.operations_dir.join(operation_file_name(operation_id))
    }
}

#[must_use]
pub fn lifecycle_journal_root_for(profile_root: &Path) -> PathBuf {
    profile_root
        .join(LIFECYCLE_JOURNAL_ROOT_DIR)
        .join(LIFECYCLE_JOURNAL_VERSION_DIR)
}

#[must_use]
pub fn operations_dir_for(profile_root: &Path) -> PathBuf {
    lifecycle_journal_root_for(profile_root).join(LIFECYCLE_JOURNAL_OPERATIONS_DIR)
}

#[must_use]
pub fn deterministic_operation_id(
    kind: LifecycleOperationKind,
    conversation_id: ConversationId,
) -> Uuid {
    let name = format!("{}:{conversation_id}", kind.as_str());
    Uuid::new_v5(&LIFECYCLE_OPERATION_ID_NAMESPACE, name.as_bytes())
}

#[must_use]
pub fn deterministic_recreate_operation_id(
    conversation_id: ConversationId,
    terminal_id: &str,
) -> Uuid {
    let name = format!(
        "{}:{conversation_id}:{terminal_id}",
        LifecycleOperationKind::RecreateTerminal.as_str()
    );
    Uuid::new_v5(&LIFECYCLE_OPERATION_ID_NAMESPACE, name.as_bytes())
}

fn phase_allowed_for_kind(kind: LifecycleOperationKind, phase: LifecycleOperationPhase) -> bool {
    match kind {
        LifecycleOperationKind::DeleteConversation => matches!(
            phase,
            LifecycleOperationPhase::CaptureScope
                | LifecycleOperationPhase::Observe
                | LifecycleOperationPhase::TerminatePending
                | LifecycleOperationPhase::TerminationConfirmed
                | LifecycleOperationPhase::PurgePending
                | LifecycleOperationPhase::Completed
        ),
        LifecycleOperationKind::RecreateTerminal => matches!(
            phase,
            LifecycleOperationPhase::CaptureScope
                | LifecycleOperationPhase::Observe
                | LifecycleOperationPhase::RecreatePending
                | LifecycleOperationPhase::Recreated
                | LifecycleOperationPhase::Completed
        ),
    }
}

fn recovery_state_allowed_for_phase(
    phase: LifecycleOperationPhase,
    state: TerminalRecoveryState,
) -> bool {
    matches!(
        (phase, state),
        (
            LifecycleOperationPhase::CaptureScope,
            TerminalRecoveryState::Active
        ) | (
            LifecycleOperationPhase::Observe,
            TerminalRecoveryState::Active
        ) | (
            LifecycleOperationPhase::Observe,
            TerminalRecoveryState::Lost
        ) | (
            LifecycleOperationPhase::RecreatePending,
            TerminalRecoveryState::Recreating
        ) | (
            LifecycleOperationPhase::Recreated,
            TerminalRecoveryState::Recreated
        ) | (
            LifecycleOperationPhase::Completed,
            TerminalRecoveryState::Active
        ) | (
            LifecycleOperationPhase::Completed,
            TerminalRecoveryState::Recreated
        )
    )
}

fn normalize_terminal_ids(mut terminal_ids: Vec<String>) -> Vec<String> {
    terminal_ids.sort();
    terminal_ids.dedup();
    terminal_ids
}

fn operation_file_name(operation_id: Uuid) -> String {
    format!("{operation_id}.json")
}

fn parse_operation_filename(name: &str) -> Option<Uuid> {
    let stem = name.strip_suffix(".json")?;
    let parsed = Uuid::parse_str(stem).ok()?;
    let canonical = parsed.hyphenated().to_string().to_ascii_lowercase();
    (stem == canonical).then_some(parsed)
}

fn is_owned_temp_name(name: &str) -> bool {
    name.ends_with(".tmp")
}

fn encode_record(record: &LifecycleOperationRecordV1) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| {
        LifecycleJournalError::new(
            LifecycleJournalErrorCode::LifecycleJournalCorrupt,
            "persist",
            format!("journal record could not be encoded: {error}"),
        )
        .with_ids(Some(record.operation_id), Some(record.conversation_id))
    })?;
    if bytes.len() > MAX_LIFECYCLE_JOURNAL_RECORD_BYTES {
        return Err(LifecycleJournalError::new(
            LifecycleJournalErrorCode::LifecycleJournalCorrupt,
            "persist",
            format!(
                "encoded journal record exceeds MAX_LIFECYCLE_JOURNAL_RECORD_BYTES={MAX_LIFECYCLE_JOURNAL_RECORD_BYTES}"
            ),
        )
        .with_ids(Some(record.operation_id), Some(record.conversation_id)));
    }
    Ok(bytes)
}

fn decode_record_file(
    path: &Path,
    expected_operation_id: Uuid,
) -> Result<LifecycleOperationRecordV1> {
    let bytes = fs::read(path)
        .map_err(|error| io_error("load", error).with_ids(Some(expected_operation_id), None))?;
    if bytes.is_empty() || bytes.len() > MAX_LIFECYCLE_JOURNAL_RECORD_BYTES {
        return Err(LifecycleJournalError::new(
            LifecycleJournalErrorCode::LifecycleJournalCorrupt,
            "load",
            "journal record is empty or exceeds the bounded record size",
        )
        .with_ids(Some(expected_operation_id), None));
    }
    let record = match serde_json::from_slice::<LifecycleOperationRecordV1>(&bytes) {
        Ok(record) => record,
        Err(_) => {
            return Err(LifecycleJournalError::new(
                diagnose_invalid_bytes(&bytes),
                "load",
                "journal record failed schema validation",
            )
            .with_ids(Some(expected_operation_id), None));
        }
    };
    if record.operation_id != expected_operation_id {
        return Err(LifecycleJournalError::new(
            LifecycleJournalErrorCode::LifecycleJournalCorrupt,
            "load",
            "journal file name does not match the encoded operation id",
        )
        .with_ids(Some(expected_operation_id), Some(record.conversation_id)));
    }
    record.validate()?;
    Ok(record)
}

fn diagnose_invalid_bytes(bytes: &[u8]) -> LifecycleJournalErrorCode {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return LifecycleJournalErrorCode::LifecycleJournalCorrupt;
    };
    match value.get("schemaVersion").and_then(|value| value.as_u64()) {
        Some(version) if version != u64::from(LIFECYCLE_OPERATION_JOURNAL_SCHEMA_VERSION) => {
            LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid
        }
        _ => LifecycleJournalErrorCode::LifecycleJournalCorrupt,
    }
}

fn diagnostic(
    code: LifecycleJournalErrorCode,
    operation_id: Option<Uuid>,
    file_name: String,
    detail: impl Into<String>,
) -> LifecycleJournalDiagnostic {
    LifecycleJournalDiagnostic {
        code,
        operation_id,
        file_name,
        detail: detail.into(),
    }
}

fn lock_journal<'a>(
    lock: &'a Mutex<()>,
    operation: &'static str,
) -> Result<std::sync::MutexGuard<'a, ()>> {
    lock.lock().map_err(|_| {
        LifecycleJournalError::new(
            LifecycleJournalErrorCode::LifecycleJournalIoFailed,
            operation,
            "lifecycle journal lock is poisoned",
        )
    })
}

fn map_durable_error(operation: &'static str, error: DurableFsError) -> LifecycleJournalError {
    let code = match &error {
        DurableFsError::InvalidPath { .. } | DurableFsError::SymlinkComponent { .. } => {
            LifecycleJournalErrorCode::LifecycleJournalUnknownRecord
        }
        DurableFsError::InvalidJsonLine { .. } => {
            LifecycleJournalErrorCode::LifecycleJournalCorrupt
        }
        DurableFsError::Io { .. } => LifecycleJournalErrorCode::LifecycleJournalDurabilityFailed,
    };
    LifecycleJournalError::new(code, operation, error.to_string())
}

fn io_error(operation: &'static str, error: io::Error) -> LifecycleJournalError {
    LifecycleJournalError::new(
        LifecycleJournalErrorCode::LifecycleJournalIoFailed,
        operation,
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::durable_fs::{CrashInjector, CrashPoint, NamespaceState};
    use crate::conversation::HostConversationRoots;
    use std::sync::Arc;

    const CONVERSATION_ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    struct InterruptAt(CrashPoint);

    impl CrashInjector for InterruptAt {
        fn should_interrupt(&self, point: CrashPoint) -> bool {
            point == self.0
        }
    }

    fn fixed_time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn later_time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T06:00:05Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn conversation_id() -> ConversationId {
        ConversationId::parse(CONVERSATION_ID).unwrap()
    }

    fn temp_profile() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().canonicalize().unwrap();
        (temp, profile)
    }

    fn start_delete(terminals: &[&str]) -> LifecycleOperationRecordV1 {
        LifecycleOperationRecordV1::start(
            LifecycleOperationKind::DeleteConversation,
            conversation_id(),
            7,
            terminals.iter().map(|id| (*id).to_string()).collect(),
            fixed_time(),
        )
        .unwrap()
    }

    #[test]
    fn journal_root_stays_outside_conversation_repository() {
        let roots = HostConversationRoots::standalone(
            PathBuf::from("/tmp/termul-profile"),
            PathBuf::from("/tmp/termul-workspace"),
            None,
            None,
        );
        assert_eq!(
            roots.lifecycle_journal_root(),
            PathBuf::from("/tmp/termul-profile/acp-lifecycle/v1")
        );
        assert_eq!(
            roots.private_conversation_root(),
            PathBuf::from("/tmp/termul-profile/conversations/v2")
        );
        assert_ne!(
            roots.lifecycle_journal_root(),
            roots.private_conversation_root()
        );
    }

    #[test]
    fn serialization_roundtrip_preserves_phase_and_scope() {
        let mut record = start_delete(&["term-b", "term-a", "term-a"]);
        record
            .advance(
                LifecycleOperationPhase::TerminatePending,
                LifecycleOperationStatus::NeedsRecovery,
                Some("TERMINAL_UNAVAILABLE".to_string()),
                later_time(),
            )
            .unwrap();
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["schemaVersion"], 1);
        assert_eq!(encoded["kind"], "deleteConversation");
        assert_eq!(encoded["phase"], "terminate_pending");
        assert_eq!(encoded["status"], "needs_recovery");
        assert_eq!(encoded["expectedRevision"], 7);
        assert_eq!(
            encoded["capturedTerminalScope"]["terminalIds"],
            serde_json::json!(["term-a", "term-b"])
        );
        let decoded: LifecycleOperationRecordV1 = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(
            decoded.operation_id,
            deterministic_operation_id(
                LifecycleOperationKind::DeleteConversation,
                conversation_id()
            )
        );
    }

    #[test]
    fn operation_ids_are_deterministic_for_kind_and_conversation() {
        let first = deterministic_operation_id(
            LifecycleOperationKind::DeleteConversation,
            conversation_id(),
        );
        let second = deterministic_operation_id(
            LifecycleOperationKind::DeleteConversation,
            conversation_id(),
        );
        let recreate =
            deterministic_operation_id(LifecycleOperationKind::RecreateTerminal, conversation_id());
        assert_eq!(first, second);
        assert_ne!(first, recreate);
        assert_eq!(first.get_version(), Some(uuid::Version::Sha1));
    }

    #[test]
    fn persist_survives_reopen_and_incomplete_filter() {
        let (_temp, profile) = temp_profile();
        let journal = LifecycleOperationJournal::open(&profile).unwrap();
        let mut in_flight = start_delete(&["term-1"]);
        in_flight
            .advance(
                LifecycleOperationPhase::Observe,
                LifecycleOperationStatus::InFlight,
                None,
                later_time(),
            )
            .unwrap();
        journal.persist(&in_flight).unwrap();

        let mut completed = LifecycleOperationRecordV1::start(
            LifecycleOperationKind::RecreateTerminal,
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ac").unwrap(),
            1,
            Vec::new(),
            fixed_time(),
        )
        .unwrap();
        completed
            .advance(
                LifecycleOperationPhase::Completed,
                LifecycleOperationStatus::Completed,
                None,
                later_time(),
            )
            .unwrap();
        journal.persist(&completed).unwrap();

        let reopened = LifecycleOperationJournal::open(&profile).unwrap();
        let loaded = reopened.load(in_flight.operation_id).unwrap().unwrap();
        assert_eq!(loaded.phase, LifecycleOperationPhase::Observe);
        assert_eq!(loaded.expected_revision, 7);
        assert_eq!(loaded.captured_terminal_scope.terminal_ids, ["term-1"]);
        let incomplete = reopened.list_incomplete().unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].operation_id, in_flight.operation_id);
        assert!(reopened.load(completed.operation_id).unwrap().is_some());
    }

    #[test]
    fn crash_before_temp_sync_keeps_previous_record() {
        let (_temp, profile) = temp_profile();
        let journal = LifecycleOperationJournal::open(&profile).unwrap();
        let original = start_delete(&["term-1"]);
        journal.persist(&original).unwrap();

        let interrupted = LifecycleOperationJournal::open_with_fs(
            &profile,
            DurableFileSystem::with_crash_injector(Arc::new(InterruptAt(
                CrashPoint::BeforeTempSync,
            ))),
        )
        .unwrap();
        let mut updated = original.clone();
        updated
            .advance(
                LifecycleOperationPhase::PurgePending,
                LifecycleOperationStatus::InFlight,
                None,
                later_time(),
            )
            .unwrap();
        let outcome = interrupted.persist(&updated).unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::OldComplete);
        assert_eq!(outcome.crash_point, CrashPoint::BeforeTempSync);

        let loaded = journal.load(original.operation_id).unwrap().unwrap();
        assert_eq!(loaded.phase, LifecycleOperationPhase::CaptureScope);
        assert_eq!(loaded.updated_at_utc, original.updated_at_utc);
    }

    #[test]
    fn crash_after_replace_keeps_new_record() {
        let (_temp, profile) = temp_profile();
        let journal = LifecycleOperationJournal::open(&profile).unwrap();
        let original = start_delete(&["term-1"]);
        journal.persist(&original).unwrap();

        let interrupted = LifecycleOperationJournal::open_with_fs(
            &profile,
            DurableFileSystem::with_crash_injector(Arc::new(InterruptAt(CrashPoint::AfterReplace))),
        )
        .unwrap();
        let mut updated = original.clone();
        updated
            .advance(
                LifecycleOperationPhase::TerminationConfirmed,
                LifecycleOperationStatus::InFlight,
                None,
                later_time(),
            )
            .unwrap();
        let outcome = interrupted.persist(&updated).unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterReplace);

        let loaded = journal.load(original.operation_id).unwrap().unwrap();
        assert_eq!(loaded.phase, LifecycleOperationPhase::TerminationConfirmed);
    }

    #[test]
    fn invalid_schema_and_corrupt_records_fail_closed_without_deletion() {
        let (_temp, profile) = temp_profile();
        let journal = LifecycleOperationJournal::open(&profile).unwrap();
        let valid = start_delete(&["term-1"]);
        journal.persist(&valid).unwrap();

        let schema_id = Uuid::new_v4();
        let schema_name = operation_file_name(schema_id);
        fs::write(
            journal.operations_dir().join(&schema_name),
            br#"{"schemaVersion":99,"operationId":"00000000-0000-0000-0000-000000000000"}"#,
        )
        .unwrap();
        let corrupt_id = Uuid::new_v4();
        let corrupt_name = operation_file_name(corrupt_id);
        let corrupt_path = journal.operations_dir().join(&corrupt_name);
        fs::write(&corrupt_path, b"{not-json").unwrap();
        fs::write(
            journal.operations_dir().join("not-an-operation.json"),
            b"{}",
        )
        .unwrap();
        fs::write(
            journal.operations_dir().join(format!(
                "{}.pid.temp.tmp",
                operation_file_name(Uuid::new_v4())
            )),
            b"owned temp leftover",
        )
        .unwrap();

        let before = fs::read(&corrupt_path).unwrap();
        let scan = journal.scan_startup().unwrap();
        assert_eq!(scan.incomplete.len(), 1);
        assert_eq!(scan.incomplete[0].operation_id, valid.operation_id);
        assert!(scan.has_corrupt_records());
        assert!(scan.diagnostics.iter().any(|diagnostic| {
            diagnostic.file_name == schema_name
                && diagnostic.code == LifecycleJournalErrorCode::LifecycleJournalSchemaInvalid
        }));
        assert!(scan.diagnostics.iter().any(|diagnostic| {
            diagnostic.file_name == corrupt_name
                && diagnostic.code == LifecycleJournalErrorCode::LifecycleJournalCorrupt
        }));
        assert!(scan.diagnostics.iter().any(|diagnostic| {
            diagnostic.file_name == "not-an-operation.json"
                && diagnostic.code == LifecycleJournalErrorCode::LifecycleJournalUnknownRecord
        }));
        assert_eq!(fs::read(&corrupt_path).unwrap(), before);
        assert!(journal.operations_dir().join(&schema_name).exists());
        assert!(journal
            .operations_dir()
            .join("not-an-operation.json")
            .exists());
        assert!(journal.load(schema_id).is_err());
        assert!(journal.load(corrupt_id).is_err());
    }

    #[test]
    fn unknown_fields_and_illegal_phase_are_rejected() {
        let mut value = serde_json::to_value(start_delete(&["term-1"])).unwrap();
        value["unexpectedField"] = serde_json::json!(true);
        assert!(serde_json::from_value::<LifecycleOperationRecordV1>(value).is_err());

        let mut record = start_delete(&["term-1"]);
        let error = record
            .advance(
                LifecycleOperationPhase::RecreatePending,
                LifecycleOperationStatus::InFlight,
                None,
                later_time(),
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            LifecycleJournalErrorCode::LifecycleJournalCorrupt
        );
    }

    fn sample_intent() -> DurableTerminalSpawnIntentV1 {
        DurableTerminalSpawnIntentV1 {
            schema_version: DURABLE_TERMINAL_SPAWN_INTENT_SCHEMA_VERSION,
            conversation_id: conversation_id(),
            project_id: Some("project-1".to_string()),
            cwd_source: DurableTerminalCwdSource::Workspace,
            cols: 80,
            rows: 24,
            program_policy: TerminalProgramPolicyV1::HostDefaultShell,
            env_policy: TerminalEnvironmentPolicyV1::HostInherited,
            kind: TerminalKindPolicyV1::ConversationInteractive,
        }
    }

    #[test]
    fn recreate_intent_roundtrip_excludes_secrets_and_raw_cwd() {
        let record = LifecycleOperationRecordV1::start_recreate(
            conversation_id(),
            3,
            "term-old".to_string(),
            sample_intent(),
            fixed_time(),
        )
        .unwrap();
        assert_eq!(
            record.operation_id,
            deterministic_recreate_operation_id(conversation_id(), "term-old")
        );
        assert_ne!(
            record.operation_id,
            deterministic_operation_id(LifecycleOperationKind::RecreateTerminal, conversation_id())
        );
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["kind"], "recreateTerminal");
        assert_eq!(encoded["terminalRecoveryState"], "active");
        assert_eq!(encoded["spawnIntent"]["programPolicy"], "hostDefaultShell");
        assert_eq!(encoded["spawnIntent"]["envPolicy"], "hostInherited");
        assert_eq!(encoded["spawnIntent"]["cwdSource"], "workspace");
        assert!(encoded.get("claim").is_none());
        assert!(encoded["spawnIntent"].get("env").is_none());
        assert!(encoded["spawnIntent"].get("cwd").is_none());
        assert!(encoded["spawnIntent"].get("args").is_none());
        let decoded: LifecycleOperationRecordV1 = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn recreate_intent_rejects_raw_env_cwd_and_claims() {
        let mut value = serde_json::to_value(sample_intent()).unwrap();
        value["env"] = serde_json::json!({"TOKEN": "secret"});
        assert!(serde_json::from_value::<DurableTerminalSpawnIntentV1>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("env");
        value["cwd"] = serde_json::json!("/tmp/secret-path");
        assert!(serde_json::from_value::<DurableTerminalSpawnIntentV1>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("cwd");
        value["claim"] = serde_json::json!("claim-secret");
        assert!(serde_json::from_value::<DurableTerminalSpawnIntentV1>(value).is_err());
    }

    #[test]
    fn recreate_intent_survives_reopen() {
        let (_temp, profile) = temp_profile();
        let journal = LifecycleOperationJournal::open(&profile).unwrap();
        let record = LifecycleOperationRecordV1::start_recreate(
            conversation_id(),
            4,
            "term-old".to_string(),
            sample_intent(),
            fixed_time(),
        )
        .unwrap();
        journal.persist(&record).unwrap();
        let reopened = LifecycleOperationJournal::open(&profile).unwrap();
        let loaded = reopened.load(record.operation_id).unwrap().unwrap();
        assert_eq!(loaded.spawn_intent, Some(sample_intent()));
        assert_eq!(
            loaded.terminal_recovery_state,
            Some(TerminalRecoveryState::Active)
        );
        assert_eq!(loaded.captured_terminal_scope.terminal_ids, ["term-old"]);
    }
}
