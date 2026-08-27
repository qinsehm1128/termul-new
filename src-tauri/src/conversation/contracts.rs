//! Runtime-neutral Conversation identity, lifecycle, and resource-reference contracts.
//!
//! Termul allocates [`ConversationId`] before ACP session creation. The ACP session id remains an
//! opaque external binding and is never accepted as Conversation identity or a path component.

use std::fmt;
use std::io::{self, Write};

use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

pub use crate::conversation::usage_plan::{
    PlanBodyV1, PlanEntryV1, PlanUpdateV1, UsageCostV1, UsagePlanSchemaError, UsageUpdateV1,
};

pub const CONVERSATION_SCHEMA_VERSION: u32 = 2;
pub const PROJECT_ATTACHMENT_SCHEMA_VERSION: u32 = 1;
pub const AGENT_SESSION_BINDING_SCHEMA_VERSION: u32 = 1;
pub const TERMINAL_RESOURCE_REF_SCHEMA_VERSION: u32 = 1;
pub const CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION: u32 = 1;
pub const CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION: u32 = 1;
pub const MIN_CONVERSATION_HISTORY_PAGE_LIMIT: usize = 1;
pub const MAX_CONVERSATION_HISTORY_PAGE_LIMIT: usize = 1_000;
/// Maximum encoded size of one canonical Conversation JSONL record, excluding its newline.
pub const MAX_CONVERSATION_RECORD_BYTES: usize = 256 * 1024;
/// Maximum encoded size of one renderer-facing Conversation history page.
pub const MAX_CONVERSATION_HISTORY_PAGE_BYTES: usize = 4 * 1024 * 1024;

const MACOS_EINVAL: i32 = 22;
const MACOS_ENOTSUP: i32 = 45;

/// Termul-owned Conversation UUID, allocated before any ACP `session/new` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConversationId(Uuid);

impl ConversationId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse any UUID spelling accepted by `uuid`; display and serde always emit its canonical form.
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(value).map(Self)
    }

    /// Parse a canonical path component, rejecting aliases such as uppercase or simple UUID forms.
    pub fn parse_path_component(value: &str) -> Result<Self, ConversationIdPathError> {
        let parsed = Self::parse(value).map_err(ConversationIdPathError::InvalidUuid)?;
        let canonical = parsed.to_string();
        if value == canonical {
            Ok(parsed)
        } else {
            Err(ConversationIdPathError::NonCanonical { canonical })
        }
    }

    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let canonical = self.0.hyphenated().to_string().to_ascii_lowercase();
        formatter.write_str(&canonical)
    }
}

impl Serialize for ConversationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Path-component validation error with a migration-safe canonical replacement.
#[derive(Debug)]
pub enum ConversationIdPathError {
    InvalidUuid(uuid::Error),
    NonCanonical { canonical: String },
}

impl ConversationIdPathError {
    #[must_use]
    pub fn canonical_replacement(&self) -> Option<&str> {
        match self {
            Self::InvalidUuid(_) => None,
            Self::NonCanonical { canonical } => Some(canonical),
        }
    }
}

impl fmt::Display for ConversationIdPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUuid(error) => write!(formatter, "invalid ConversationId: {error}"),
            Self::NonCanonical { canonical } => write!(
                formatter,
                "non-canonical ConversationId path component; use {canonical}"
            ),
        }
    }
}

impl std::error::Error for ConversationIdPathError {}

/// RFC3339-millisecond UTC timestamp validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatedAtUtcError {
    Invalid(String),
    NonCanonical { canonical: String },
}

impl fmt::Display for CreatedAtUtcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid UTC timestamp: {message}"),
            Self::NonCanonical { canonical } => write!(
                formatter,
                "timestamp must use RFC3339 milliseconds with trailing Z; use {canonical}"
            ),
        }
    }
}

impl std::error::Error for CreatedAtUtcError {}

/// Emit exactly `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[must_use]
pub fn format_created_at_utc(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Parse only the canonical RFC3339-millisecond UTC form used by Conversation contracts.
pub fn parse_created_at_utc(value: &str) -> Result<DateTime<Utc>, CreatedAtUtcError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| CreatedAtUtcError::Invalid(error.to_string()))?
        .with_timezone(&Utc);
    let canonical = format_created_at_utc(&parsed);
    if value == canonical {
        Ok(parsed)
    } else {
        Err(CreatedAtUtcError::NonCanonical { canonical })
    }
}

fn serialize_utc_millis<S>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format_created_at_utc(value))
}

fn deserialize_utc_millis<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse_created_at_utc(&value).map_err(serde::de::Error::custom)
}

/// Immutable UTC calendar partition derived only from `createdAtUtc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreationPartition {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub path: String,
}

impl CreationPartition {
    #[must_use]
    pub fn from_created_at(created_at: DateTime<Utc>) -> Self {
        Self::from_date(created_at.date_naive())
    }

    /// Construct only a calendar-valid UTC partition.
    #[must_use]
    pub fn try_new(year: i32, month: u32, day: u32) -> Option<Self> {
        NaiveDate::from_ymd_opt(year, month, day).map(Self::from_date)
    }

    fn from_date(date: NaiveDate) -> Self {
        let year = date.year();
        let month = date.month();
        let day = date.day();
        Self {
            year,
            month,
            day,
            path: format!("{year:04}/{month:02}/{day:02}"),
        }
    }
}

/// Explicit execution choice; no variant changes the independent Conversation workspace cwd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionTarget {
    Workspace,
    ProjectRoot {
        #[serde(rename = "projectId")]
        project_id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
    },
    Worktree {
        #[serde(rename = "projectId")]
        project_id: String,
        #[serde(rename = "worktreePath")]
        worktree_path: String,
        #[serde(rename = "worktreeBranch")]
        worktree_branch: String,
    },
}

/// Optional project attribution/context. It does not own or mutate `workspaceCwd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAttachment {
    pub schema_version: u32,
    pub project_id: String,
    #[serde(
        serialize_with = "serialize_utc_millis",
        deserialize_with = "deserialize_utc_millis"
    )]
    pub attached_at_utc: DateTime<Utc>,
    pub project_path_snapshot: String,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionBindingState {
    Active,
    Detached,
    Suspended,
    Replaced,
}

/// Replaceable binding to an opaque external ACP session id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSessionBinding {
    pub schema_version: u32,
    pub binding_id: Uuid,
    /// External ACP value: never UUID-validated and never used as a filesystem component.
    pub agent_session_id: String,
    pub runtime_agent_id: String,
    pub stable_agent_namespace: String,
    /// Resolved explicit execution target, independent from the Conversation workspace cwd.
    pub execution_cwd: String,
    #[serde(
        serialize_with = "serialize_utc_millis",
        deserialize_with = "deserialize_utc_millis"
    )]
    pub bound_at_utc: DateTime<Utc>,
    pub state: AgentSessionBindingState,
}

/// Non-owning reference to a `PtyManager`-owned terminal resource.
///
/// Raw terminal claims, environment values, credentials, and terminal output are never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResourceRef {
    pub schema_version: u32,
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLifecycleState {
    AllocatingWorkspace,
    InitializingAgent,
    Ready,
    AgentFailed,
    RecoveryRequired,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCreator {
    Termul,
}

/// Stable application error codes shared by Rust and TypeScript transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConversationErrorCode {
    ConversationInvalidId,
    ConversationInvalidCreatedAt,
    ConversationUnsupportedSchema,
    ConversationNotFound,
    ConversationCorrupt,
    ConversationPathEscape,
    ConversationSymlinkComponent,
    ConversationDurabilityFailed,
    ConversationCreateFailed,
    ConversationBindFailed,
    ConversationConflict,
    ConversationBindingNotFound,
    ConversationBindingNotActive,
    ConversationBindingNotDetached,
    ConversationBindingNotAddressable,
    ConversationLiveResources,
    ConversationRecoveryRequired,
    ConversationDurabilityUnsupported,
    LegacyCompatibilityReadOnly,
    ConversationRecordTooLarge,
    ConversationPageTooLarge,
    ValidationError,
}

impl ConversationErrorCode {
    /// Classify a macOS `F_FULLFSYNC` failure without silently weakening Immediate durability.
    #[must_use]
    pub fn from_macos_full_sync_error(error: &std::io::Error) -> Self {
        match error.raw_os_error() {
            Some(MACOS_EINVAL | MACOS_ENOTSUP) => Self::ConversationDurabilityUnsupported,
            _ => Self::ConversationDurabilityFailed,
        }
    }
}

/// Canonical provenance for a Conversation history title.
///
/// Precedence is `LocalAlias > BackgroundGenerated > AgentSupplied > DerivedFirstMessage`;
/// absence represents the untitled floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTitleSource {
    BackgroundGenerated,
    AgentSupplied,
    DerivedFirstMessage,
    LocalAlias,
}

/// Runtime-neutral ACP history record carried inside bounded Conversation pages. The canonical
/// repository adapter maps v2 Conversation events into this stable v1 renderer-facing dialect;
/// opaque session ids remain values only and are never used as filesystem components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHistoryRecordV1 {
    pub schema_version: u32,
    pub session_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub type_: String,
    pub recorded_at: u64,
    pub payload: Value,
}

/// One immutable target-bounded history page shared by Tauri and WebSocket transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHistoryPageV1 {
    pub schema_version: u32,
    pub records: Vec<ConversationHistoryRecordV1>,
    pub next_cursor: u64,
    pub complete: bool,
    pub target_last_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationHistoryPageValidationError {
    UnsupportedPageSchema,
    UnsupportedRecordSchema,
    InvalidLimit,
    TooManyRecords,
    EmptySessionId,
    SessionMismatch,
    CursorAheadOfTarget,
    CursorRegression,
    CursorBeyondTarget,
    TargetChanged,
    CompletionMismatch,
    RecordSequenceConflict,
    RecordTooLarge,
    PageTooLarge,
}

impl ConversationHistoryPageValidationError {
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::RecordTooLarge => "CONVERSATION_RECORD_TOO_LARGE",
            Self::PageTooLarge => "CONVERSATION_PAGE_TOO_LARGE",
            _ => "VALIDATION_ERROR",
        }
    }
}

impl fmt::Display for ConversationHistoryPageValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            Self::UnsupportedPageSchema => "history page schemaVersion is unsupported",
            Self::UnsupportedRecordSchema => "history record schemaVersion is unsupported",
            Self::InvalidLimit => "history page limit must be an integer between 1 and 1000",
            Self::TooManyRecords => "history page contains more records than requested",
            Self::EmptySessionId => "history page sessionId must be non-empty",
            Self::SessionMismatch => "history page belongs to another session",
            Self::CursorAheadOfTarget => "history page cursor is ahead of targetLastSeq",
            Self::CursorRegression => "history page nextCursor did not advance",
            Self::CursorBeyondTarget => "history page nextCursor exceeds targetLastSeq",
            Self::TargetChanged => "history page targetLastSeq changed during traversal",
            Self::CompletionMismatch => "history page complete flag disagrees with nextCursor",
            Self::RecordSequenceConflict => "history page records are not strictly ordered",
            Self::RecordTooLarge => "history record exceeds the 256 KiB encoded limit",
            Self::PageTooLarge => "history page exceeds the 4 MiB encoded limit",
        };
        formatter.write_str(detail)
    }
}

impl std::error::Error for ConversationHistoryPageValidationError {}

impl ConversationHistoryPageV1 {
    pub fn validate(
        &self,
        expected_session_id: &str,
        after_seq: u64,
        limit: usize,
        expected_target_last_seq: Option<u64>,
    ) -> Result<(), ConversationHistoryPageValidationError> {
        use ConversationHistoryPageValidationError as ValidationError;

        if self.schema_version != CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedPageSchema);
        }
        if !(MIN_CONVERSATION_HISTORY_PAGE_LIMIT..=MAX_CONVERSATION_HISTORY_PAGE_LIMIT)
            .contains(&limit)
        {
            return Err(ValidationError::InvalidLimit);
        }
        if self.records.len() > limit {
            return Err(ValidationError::TooManyRecords);
        }
        if expected_session_id.trim().is_empty() {
            return Err(ValidationError::EmptySessionId);
        }
        if after_seq > self.target_last_seq {
            return Err(ValidationError::CursorAheadOfTarget);
        }
        if expected_target_last_seq.is_some_and(|target| target != self.target_last_seq) {
            return Err(ValidationError::TargetChanged);
        }
        if self.next_cursor < after_seq
            || (self.next_cursor == after_seq && after_seq < self.target_last_seq)
        {
            return Err(ValidationError::CursorRegression);
        }
        if self.next_cursor > self.target_last_seq {
            return Err(ValidationError::CursorBeyondTarget);
        }
        if self.complete != (self.next_cursor == self.target_last_seq) {
            return Err(ValidationError::CompletionMismatch);
        }

        let mut previous_seq = after_seq;
        for record in &self.records {
            if record.schema_version != CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION {
                return Err(ValidationError::UnsupportedRecordSchema);
            }
            if record.session_id != expected_session_id {
                return Err(ValidationError::SessionMismatch);
            }
            if record.seq <= previous_seq || record.seq > self.next_cursor {
                return Err(ValidationError::RecordSequenceConflict);
            }
            if encoded_json_len_bounded(record, MAX_CONVERSATION_RECORD_BYTES).is_none() {
                return Err(ValidationError::RecordTooLarge);
            }
            previous_seq = record.seq;
        }
        if encoded_json_len_bounded(self, MAX_CONVERSATION_HISTORY_PAGE_BYTES).is_none() {
            return Err(ValidationError::PageTooLarge);
        }
        Ok(())
    }
}

struct BoundedJsonCounter {
    bytes: usize,
    limit: usize,
}

impl Write for BoundedJsonCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self.bytes.checked_add(buffer.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "encoded JSON length overflow")
        })?;
        if next > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded JSON exceeds configured limit",
            ));
        }
        self.bytes = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Count JSON bytes without allocating the encoded value. `None` means serialization failed or
/// crossed `limit`; callers use the same helper before page/cache allocation and wire emission.
pub(crate) fn encoded_json_len_bounded<T: Serialize>(value: &T, limit: usize) -> Option<usize> {
    let mut counter = BoundedJsonCounter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .ok()
        .map(|()| counter.bytes)
}

/// Canonical event-derived history summary exposed to persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHistorySummaryV1 {
    pub conversation_id: ConversationId,
    pub title: Option<String>,
    pub title_source: Option<ConversationTitleSource>,
    #[serde(
        serialize_with = "serialize_utc_millis",
        deserialize_with = "deserialize_utc_millis"
    )]
    pub last_activity_at_utc: DateTime<Utc>,
    pub message_count: u64,
    pub tool_count: u64,
}

/// Canonical Conversation metadata. Binding and terminal-resource history are stored separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecordV2 {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    #[serde(
        serialize_with = "serialize_utc_millis",
        deserialize_with = "deserialize_utc_millis"
    )]
    pub created_at_utc: DateTime<Utc>,
    pub creation_partition: CreationPartition,
    pub workspace_cwd: String,
    pub execution_target: ExecutionTarget,
    pub project_attachment: Option<ProjectAttachment>,
    pub lifecycle_state: ConversationLifecycleState,
    pub last_seq: u64,
    pub created_by: ConversationCreator,
    pub title: Option<String>,
    pub title_source: Option<ConversationTitleSource>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CANONICAL_ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const UPPERCASE_ID: &str = "018F7A1C-1B4D-7C8A-9F01-0123456789AB";

    #[test]
    fn uppercase_uuid_reports_canonical_replacement_and_is_rejected_as_path_component() {
        let parsed = ConversationId::parse(UPPERCASE_ID).expect("uppercase UUID parses");
        assert_eq!(parsed.to_string(), CANONICAL_ID);
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(CANONICAL_ID));

        let error = ConversationId::parse_path_component(UPPERCASE_ID)
            .expect_err("uppercase UUID is not a canonical path component");
        assert_eq!(error.canonical_replacement(), Some(CANONICAL_ID));
        assert_eq!(
            ConversationId::parse_path_component(CANONICAL_ID).unwrap(),
            parsed
        );
    }

    #[test]
    fn utc_partition_handles_midnight_leap_day_and_invalid_dates() {
        let midnight = parse_created_at_utc("2026-01-01T00:00:00.000Z").unwrap();
        assert_eq!(
            CreationPartition::from_created_at(midnight).path,
            "2026/01/01"
        );

        let leap_day = parse_created_at_utc("2028-02-29T23:59:59.999Z").unwrap();
        assert_eq!(
            CreationPartition::from_created_at(leap_day).path,
            "2028/02/29"
        );
        assert_eq!(
            CreationPartition::try_new(2028, 2, 29).unwrap().path,
            "2028/02/29"
        );
        assert!(CreationPartition::try_new(2027, 2, 29).is_none());
        assert!(parse_created_at_utc("2027-02-29T00:00:00.000Z").is_err());
        assert!(parse_created_at_utc("2026-01-01T00:00:00Z").is_err());
        assert!(parse_created_at_utc("2025-12-31T19:00:00.000-05:00").is_err());
    }

    #[test]
    fn serde_fixture_matches_typescript_contract_and_keeps_opaque_binding_separate() {
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        let record = ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::parse(CANONICAL_ID).unwrap(),
            created_at_utc: created_at,
            creation_partition: CreationPartition::from_created_at(created_at),
            workspace_cwd: format!("/user-visible-root/sessions/2026/08/15/{CANONICAL_ID}"),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::AllocatingWorkspace,
            last_seq: 0,
            created_by: ConversationCreator::Termul,
            title: None,
            title_source: None,
        };
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({
                "schemaVersion": 2,
                "conversationId": CANONICAL_ID,
                "createdAtUtc": "2026-08-15T09:45:15.123Z",
                "creationPartition": {
                    "year": 2026,
                    "month": 8,
                    "day": 15,
                    "path": "2026/08/15"
                },
                "workspaceCwd": format!(
                    "/user-visible-root/sessions/2026/08/15/{CANONICAL_ID}"
                ),
                "executionTarget": { "kind": "workspace" },
                "projectAttachment": null,
                "lifecycleState": "allocating_workspace",
                "lastSeq": 0,
                "createdBy": "termul"
            })
        );

        let binding = AgentSessionBinding {
            schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
            binding_id: Uuid::parse_str("b2832b54-2ca4-4db4-93fd-f93bf6793114").unwrap(),
            agent_session_id: "agent/session:not-a-uuid?generation=2".to_string(),
            runtime_agent_id: "agent-runtime-id".to_string(),
            stable_agent_namespace: "config:configured-agent-id".to_string(),
            execution_cwd: record.workspace_cwd.clone(),
            bound_at_utc: parse_created_at_utc("2026-08-15T09:45:16.000Z").unwrap(),
            state: AgentSessionBindingState::Active,
        };
        let binding_value = serde_json::to_value(&binding).unwrap();
        assert_eq!(
            binding_value["agentSessionId"],
            "agent/session:not-a-uuid?generation=2"
        );
        assert_ne!(
            binding_value["agentSessionId"],
            record.conversation_id.to_string()
        );
    }

    #[test]
    fn tagged_execution_targets_use_exact_wire_values() {
        let targets = [
            ExecutionTarget::Workspace,
            ExecutionTarget::ProjectRoot {
                project_id: "project-opaque-id".to_string(),
                project_root: "/projects/example".to_string(),
            },
            ExecutionTarget::Worktree {
                project_id: "project-opaque-id".to_string(),
                worktree_path: "/projects/example-worktree".to_string(),
                worktree_branch: "chat/example".to_string(),
            },
        ];
        assert_eq!(
            targets
                .iter()
                .map(|target| serde_json::to_value(target).unwrap())
                .collect::<Vec<_>>(),
            vec![
                json!({ "kind": "workspace" }),
                json!({
                    "kind": "project_root",
                    "projectId": "project-opaque-id",
                    "projectRoot": "/projects/example"
                }),
                json!({
                    "kind": "worktree",
                    "projectId": "project-opaque-id",
                    "worktreePath": "/projects/example-worktree",
                    "worktreeBranch": "chat/example"
                })
            ]
        );
    }

    #[test]
    fn macos_full_sync_unsupported_errors_fail_closed_with_stable_code() {
        for raw_os_error in [MACOS_ENOTSUP, MACOS_EINVAL] {
            let error = std::io::Error::from_raw_os_error(raw_os_error);
            assert_eq!(
                ConversationErrorCode::from_macos_full_sync_error(&error),
                ConversationErrorCode::ConversationDurabilityUnsupported
            );
        }

        let other_error = std::io::Error::from_raw_os_error(5);
        assert_eq!(
            ConversationErrorCode::from_macos_full_sync_error(&other_error),
            ConversationErrorCode::ConversationDurabilityFailed
        );
        assert_eq!(
            serde_json::to_value(ConversationErrorCode::ConversationDurabilityUnsupported).unwrap(),
            json!("CONVERSATION_DURABILITY_UNSUPPORTED")
        );
    }

    #[test]
    fn conversation_history_page_v1_has_exact_camel_case_wire_contract() {
        let page = ConversationHistoryPageV1 {
            schema_version: CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION,
            records: vec![ConversationHistoryRecordV1 {
                schema_version: CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                session_id: "opaque/session".to_string(),
                seq: 18,
                type_: "message_chunk".to_string(),
                recorded_at: 1_766_000_000_018,
                payload: json!({"role":"agent","content":{"type":"text","text":"ok"}}),
            }],
            next_cursor: 18,
            complete: false,
            target_last_seq: 42,
        };
        page.validate("opaque/session", 17, 250, None).unwrap();
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            json!({
                "schemaVersion": 1,
                "records": [{
                    "schemaVersion": 1,
                    "sessionId": "opaque/session",
                    "seq": 18,
                    "type": "message_chunk",
                    "recordedAt": 1_766_000_000_018_u64,
                    "payload": {"role":"agent","content":{"type":"text","text":"ok"}}
                }],
                "nextCursor": 18,
                "complete": false,
                "targetLastSeq": 42
            })
        );
    }

    #[test]
    fn conversation_history_page_validation_rejects_identity_cursor_limit_and_target_drift() {
        let mut page = ConversationHistoryPageV1 {
            schema_version: CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION,
            records: vec![ConversationHistoryRecordV1 {
                schema_version: CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                session_id: "session-a".to_string(),
                seq: 18,
                type_: "user_prompt".to_string(),
                recorded_at: 18,
                payload: json!({}),
            }],
            next_cursor: 18,
            complete: false,
            target_last_seq: 20,
        };
        for invalid_limit in [0, MAX_CONVERSATION_HISTORY_PAGE_LIMIT + 1] {
            let error = page
                .validate("session-a", 17, invalid_limit, None)
                .unwrap_err();
            assert_eq!(error.stable_code(), "VALIDATION_ERROR");
            assert_eq!(error, ConversationHistoryPageValidationError::InvalidLimit);
        }
        assert_eq!(
            page.validate("session-b", 17, 250, None).unwrap_err(),
            ConversationHistoryPageValidationError::SessionMismatch
        );
        assert_eq!(
            page.validate("session-a", 18, 250, None).unwrap_err(),
            ConversationHistoryPageValidationError::CursorRegression
        );
        assert_eq!(
            page.validate("session-a", 17, 250, Some(21)).unwrap_err(),
            ConversationHistoryPageValidationError::TargetChanged
        );
        page.complete = true;
        assert_eq!(
            page.validate("session-a", 17, 250, None).unwrap_err(),
            ConversationHistoryPageValidationError::CompletionMismatch
        );
    }

    #[test]
    fn terminal_reference_is_non_owning_and_rejects_raw_claims() {
        let terminal = TerminalResourceRef {
            schema_version: TERMINAL_RESOURCE_REF_SCHEMA_VERSION,
            terminal_id: "terminal-1".to_string(),
            project_id: None,
        };
        assert_eq!(
            serde_json::to_value(terminal).unwrap(),
            json!({ "schemaVersion": 1, "terminalId": "terminal-1" })
        );
        assert!(serde_json::from_value::<TerminalResourceRef>(json!({
            "schemaVersion": 1,
            "terminalId": "terminal-1",
            "claim": "raw-secret-claim"
        }))
        .is_err());
    }
}
