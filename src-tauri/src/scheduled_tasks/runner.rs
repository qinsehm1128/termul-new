use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;

use crate::acp::{AcpManager, SessionCreationContext};
use crate::conversation::{ProjectAttachment, PROJECT_ATTACHMENT_SCHEMA_VERSION};
use crate::web::WsRelaySink;

use super::models::{ScheduledTaskRunStatus, ScheduledTaskRunV1, ScheduledTaskV1};
use super::store::ScheduledTaskStore;

pub const DEFAULT_TASK_TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
pub struct TaskExecutionOutcome {
    pub conversation_id: Option<String>,
    pub summary: Option<String>,
    pub usage: Option<serde_json::Value>,
}

#[async_trait]
pub trait ScheduledTaskExecutor: Send + Sync {
    async fn execute(
        &self,
        task: ScheduledTaskV1,
        run: ScheduledTaskRunV1,
    ) -> Result<TaskExecutionOutcome, TaskExecutionError>;
}

#[derive(Debug, Clone)]
pub struct TaskExecutionError {
    pub code: &'static str,
    pub detail: String,
    pub conversation_id: Option<String>,
}

impl TaskExecutionError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: sanitize_error(detail.into()),
            conversation_id: None,
        }
    }

    fn with_conversation_id(mut self, conversation_id: Option<String>) -> Self {
        self.conversation_id = conversation_id;
        self
    }
}

impl std::fmt::Display for TaskExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for TaskExecutionError {}

pub struct AcpScheduledTaskExecutor {
    manager: Arc<AcpManager>,
    relay: Arc<WsRelaySink>,
    turn_timeout: Duration,
}

impl AcpScheduledTaskExecutor {
    #[must_use]
    pub fn new(manager: Arc<AcpManager>, relay: Arc<WsRelaySink>) -> Self {
        Self {
            manager,
            relay,
            turn_timeout: DEFAULT_TASK_TURN_TIMEOUT,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_turn_timeout(mut self, turn_timeout: Duration) -> Self {
        self.turn_timeout = turn_timeout;
        self
    }
}

#[async_trait]
impl ScheduledTaskExecutor for AcpScheduledTaskExecutor {
    async fn execute(
        &self,
        task: ScheduledTaskV1,
        run: ScheduledTaskRunV1,
    ) -> Result<TaskExecutionOutcome, TaskExecutionError> {
        let agent_id = self
            .manager
            .find_agent_by_config_id(&task.agent_config_id)
            .ok_or_else(|| {
                TaskExecutionError::new(
                    "AGENT_UNAVAILABLE",
                    "the selected ACP is not currently connected",
                )
            })?;

        let project_attachment = task.project_id.clone().map(|project_id| ProjectAttachment {
            schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
            project_id,
            attached_at_utc: Utc::now(),
            project_path_snapshot: task.execution_cwd.clone(),
            worktree_path: match &task.execution_target {
                crate::conversation::ExecutionTarget::Worktree { worktree_path, .. } => {
                    Some(worktree_path.clone())
                }
                _ => None,
            },
            worktree_branch: match &task.execution_target {
                crate::conversation::ExecutionTarget::Worktree {
                    worktree_branch, ..
                } => Some(worktree_branch.clone()),
                _ => None,
            },
        });
        let session = self
            .manager
            .new_session_with_context(
                &agent_id,
                task.execution_cwd.clone(),
                Vec::new(),
                SessionCreationContext {
                    project_id: task.project_id.clone(),
                    ephemeral: false,
                    conversation_id: None,
                    project_attachment,
                    execution_target: Some(task.execution_target.clone()),
                    worktree_path: None,
                    worktree_branch: None,
                },
            )
            .await
            .map_err(|error| TaskExecutionError::new("ACP_SESSION_CREATE_FAILED", error))?;

        let conversation_id = session.conversation_id.map(|value| value.to_string());
        let blocks = vec![ContentBlock::Text(TextContent::new(task.prompt.clone()))];
        self.relay
            .persist_user_prompt(
                session.session_id.0.as_str(),
                json!({
                    "agentId": agent_id,
                    "sessionId": session.session_id,
                    "turnId": run.run_id,
                    "content": blocks,
                    "origin": "scheduled-task",
                    "scheduledTaskId": task.task_id,
                }),
            )
            .await
            .map_err(|error| {
                TaskExecutionError::new("PROMPT_PERSIST_FAILED", error)
                    .with_conversation_id(conversation_id.clone())
            })?;

        let stop_reason = tokio::time::timeout(
            self.turn_timeout,
            self.manager
                .send_prompt(&agent_id, session.session_id, blocks, Some(run.run_id)),
        )
        .await
        .map_err(|_| {
            TaskExecutionError::new("TASK_TURN_TIMEOUT", "the scheduled ACP turn timed out")
                .with_conversation_id(conversation_id.clone())
        })?
        .map_err(|error| {
            TaskExecutionError::new("ACP_PROMPT_FAILED", error)
                .with_conversation_id(conversation_id.clone())
        })?;

        Ok(TaskExecutionOutcome {
            conversation_id,
            summary: Some(format!("ACP turn completed: {stop_reason:?}")),
            usage: None,
        })
    }
}

pub async fn execute_and_record(
    store: Arc<ScheduledTaskStore>,
    executor: Arc<dyn ScheduledTaskExecutor>,
    task: ScheduledTaskV1,
    mut run: ScheduledTaskRunV1,
) -> ScheduledTaskRunV1 {
    run.status = ScheduledTaskRunStatus::Running;
    run.started_at = Some(Utc::now().to_rfc3339());
    if let Err(error) = store.append_run(&run) {
        log::error!(
            "[scheduled-task] boundary=run_start_persist task_id={} run_id={} code=RUN_LEDGER_WRITE_FAILED error={error}",
            task.task_id,
            run.run_id
        );
        run.status = ScheduledTaskRunStatus::Failed;
        run.finished_at = Some(Utc::now().to_rfc3339());
        run.error_code = Some("RUN_LEDGER_WRITE_FAILED".to_string());
        run.error_detail = Some("failed to persist run start".to_string());
        return run;
    }
    log::info!(
        "[scheduled-task] boundary=run_started task_id={} run_id={} trigger={:?}",
        task.task_id,
        run.run_id,
        run.trigger
    );

    match executor.execute(task.clone(), run.clone()).await {
        Ok(outcome) => {
            run.status = ScheduledTaskRunStatus::Succeeded;
            run.conversation_id = outcome.conversation_id;
            run.summary = outcome.summary;
            run.usage = outcome.usage;
            log::info!(
                "[scheduled-task] boundary=run_completed task_id={} run_id={} status=succeeded",
                task.task_id,
                run.run_id
            );
        }
        Err(error) => {
            run.status = ScheduledTaskRunStatus::Failed;
            run.conversation_id = error.conversation_id;
            run.error_code = Some(error.code.to_string());
            run.error_detail = Some(error.detail);
            log::warn!(
                "[scheduled-task] boundary=run_completed task_id={} run_id={} status=failed code={}",
                task.task_id,
                run.run_id,
                error.code
            );
        }
    }
    run.finished_at = Some(Utc::now().to_rfc3339());
    if let Err(error) = store.append_run(&run) {
        log::error!(
            "[scheduled-task] boundary=run_finish_persist task_id={} run_id={} code=RUN_LEDGER_WRITE_FAILED error={error}",
            task.task_id,
            run.run_id
        );
    }
    run
}

fn sanitize_error(value: String) -> String {
    let first_line = value
        .lines()
        .next()
        .unwrap_or("scheduled task execution failed");
    first_line.chars().take(512).collect()
}
