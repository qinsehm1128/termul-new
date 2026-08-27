//! Host-owned AI scheduled tasks with optional project association.
//!
//! The host persists schedule definitions and a compact run ledger. Each actual
//! agent execution is represented by a separate Conversation v2 record so
//! messages, plans, tool calls and usage keep one canonical audit source.

pub mod commands;
pub mod models;
pub mod runner;
pub mod schedule;
pub mod scheduler;
pub mod store;

pub use models::{
    ActivateTaskRequestV1, CatchUpPolicy, ExecutionPolicyV1, OverlapPolicy, RevisionRequestV1,
    SchedulePreviewV1, ScheduleSpecV1, ScheduledTaskAuditActor, ScheduledTaskAuditEventV1,
    ScheduledTaskCatalogV1, ScheduledTaskDraftInputV1, ScheduledTaskRunStatus,
    ScheduledTaskRunTrigger, ScheduledTaskRunV1, ScheduledTaskStatus, ScheduledTaskV1,
    TaskMutationContextV1, UpdateDraftRequestV1, DEFAULT_CATCH_UP_WINDOW_SECONDS,
    SCHEDULED_TASK_AUDIT_SCHEMA_VERSION, SCHEDULED_TASK_RUN_SCHEMA_VERSION,
    SCHEDULED_TASK_SCHEMA_VERSION, SKILL_TEMPLATE_VERSION,
};
pub use runner::{
    AcpScheduledTaskExecutor, ScheduledTaskExecutor, TaskExecutionError, TaskExecutionOutcome,
    DEFAULT_TASK_TURN_TIMEOUT,
};
pub use schedule::{
    next_after, normalize_schedule, occurrences_after, preview_schedule, ScheduleError,
};
pub use scheduler::ScheduledTaskService;
pub use store::{new_queued_run, snapshot_hash, ScheduledTaskStore, ScheduledTaskStoreError};
