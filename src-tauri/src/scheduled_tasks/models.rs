use serde::{Deserialize, Serialize};

use crate::conversation::ExecutionTarget;

pub const SCHEDULED_TASK_SCHEMA_VERSION: u32 = 1;
pub const SCHEDULED_TASK_RUN_SCHEMA_VERSION: u32 = 1;
pub const SCHEDULED_TASK_AUDIT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_CATCH_UP_WINDOW_SECONDS: u64 = 24 * 60 * 60;
pub const SKILL_TEMPLATE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ScheduleSpecV1 {
    Cron {
        expression: String,
        timezone: String,
    },
    Interval {
        every_seconds: u64,
        anchor_at: String,
    },
    At {
        at: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduledTaskStatus {
    Draft,
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OverlapPolicy {
    Skip,
    BufferOne,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CatchUpPolicy {
    Skip,
    LatestOnce,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionPolicyV1 {
    pub overlap: OverlapPolicy,
    pub catch_up: CatchUpPolicy,
    pub catch_up_window_seconds: u64,
}

impl Default for ExecutionPolicyV1 {
    fn default() -> Self {
        Self {
            overlap: OverlapPolicy::BufferOne,
            catch_up: CatchUpPolicy::LatestOnce,
            catch_up_window_seconds: DEFAULT_CATCH_UP_WINDOW_SECONDS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskV1 {
    pub schema_version: u32,
    pub task_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub name: String,
    pub description: String,
    pub status: ScheduledTaskStatus,
    pub schedule: ScheduleSpecV1,
    pub execution_policy: ExecutionPolicyV1,
    pub prompt: String,
    pub agent_config_id: String,
    pub execution_target: ExecutionTarget,
    pub execution_cwd: String,
    pub workspace_cwd: String,
    pub source_conversation_id: Option<String>,
    pub permissions: Vec<String>,
    pub skill_template_version: u32,
    pub revision: u64,
    pub draft_hash: String,
    pub created_at: String,
    pub updated_at: String,
    pub next_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskDraftInputV1 {
    #[serde(default)]
    pub project_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub schedule: ScheduleSpecV1,
    #[serde(default)]
    pub execution_policy: ExecutionPolicyV1,
    pub prompt: String,
    pub agent_config_id: String,
    pub execution_target: ExecutionTarget,
    pub execution_cwd: String,
    pub workspace_cwd: String,
    #[serde(default)]
    pub source_conversation_id: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchedulePreviewV1 {
    pub normalized: ScheduleSpecV1,
    pub next_run_times: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduledTaskRunTrigger {
    Scheduled,
    Manual,
    CatchUp,
    Retry,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduledTaskRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskRunV1 {
    pub schema_version: u32,
    pub run_id: String,
    pub task_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub trigger: ScheduledTaskRunTrigger,
    pub status: ScheduledTaskRunStatus,
    pub occurrence_key: String,
    pub scheduled_for: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub conversation_id: Option<String>,
    pub task_snapshot_hash: String,
    pub retry_of_run_id: Option<String>,
    pub summary: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScheduledTaskAuditActor {
    Human,
    Agent,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskAuditEventV1 {
    pub schema_version: u32,
    pub event_id: String,
    pub task_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub action: String,
    pub actor: ScheduledTaskAuditActor,
    pub source_conversation_id: Option<String>,
    pub source_tool_call_id: Option<String>,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledTaskCatalogV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub tasks: Vec<ScheduledTaskV1>,
}

impl Default for ScheduledTaskCatalogV1 {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_TASK_SCHEMA_VERSION,
            revision: 0,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionRequestV1 {
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivateTaskRequestV1 {
    pub expected_revision: u64,
    pub expected_draft_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDraftRequestV1 {
    pub expected_revision: u64,
    pub input: ScheduledTaskDraftInputV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskMutationContextV1 {
    pub actor: ScheduledTaskAuditActor,
    #[serde(default)]
    pub source_conversation_id: Option<String>,
    #[serde(default)]
    pub source_tool_call_id: Option<String>,
}

impl Default for TaskMutationContextV1 {
    fn default() -> Self {
        Self {
            actor: ScheduledTaskAuditActor::Human,
            source_conversation_id: None,
            source_tool_call_id: None,
        }
    }
}
