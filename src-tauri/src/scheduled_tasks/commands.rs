use std::sync::Arc;

use tauri::State;

use crate::commands::IpcResult;

use super::{
    ActivateTaskRequestV1, RevisionRequestV1, SchedulePreviewV1, ScheduleSpecV1,
    ScheduledTaskAuditEventV1, ScheduledTaskDraftInputV1, ScheduledTaskRunV1, ScheduledTaskService,
    ScheduledTaskV1, TaskMutationContextV1, UpdateDraftRequestV1,
};

fn human_context() -> TaskMutationContextV1 {
    TaskMutationContextV1::default()
}

fn ipc<T>(result: super::store::Result<T>) -> IpcResult<T> {
    match result {
        Ok(value) => IpcResult::success(value),
        Err(error) => IpcResult::error(error.to_string(), "SCHEDULED_TASK_ERROR"),
    }
}

#[tauri::command]
pub fn scheduled_task_preview(
    service: State<'_, Arc<ScheduledTaskService>>,
    schedule: ScheduleSpecV1,
    count: Option<usize>,
) -> IpcResult<SchedulePreviewV1> {
    ipc(service.preview(&schedule, count.unwrap_or(5)))
}

#[tauri::command]
pub fn scheduled_task_list(
    service: State<'_, Arc<ScheduledTaskService>>,
    project_id: Option<String>,
) -> IpcResult<Vec<ScheduledTaskV1>> {
    ipc(service.list_tasks(project_id.as_deref()))
}

#[tauri::command]
pub fn scheduled_task_get(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.get_task(&task_id))
}

#[tauri::command]
pub fn scheduled_task_draft_create(
    service: State<'_, Arc<ScheduledTaskService>>,
    input: ScheduledTaskDraftInputV1,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.create_draft(input, human_context()))
}

#[tauri::command]
pub fn scheduled_task_draft_update(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    request: UpdateDraftRequestV1,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.update_draft(
        &task_id,
        request.expected_revision,
        request.input,
        human_context(),
    ))
}

#[tauri::command]
pub fn scheduled_task_activate(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    request: ActivateTaskRequestV1,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.activate(
        &task_id,
        request.expected_revision,
        &request.expected_draft_hash,
        human_context(),
    ))
}

#[tauri::command]
pub fn scheduled_task_pause(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    request: RevisionRequestV1,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.pause(&task_id, request.expected_revision, human_context()))
}

#[tauri::command]
pub fn scheduled_task_resume(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    request: RevisionRequestV1,
) -> IpcResult<ScheduledTaskV1> {
    ipc(service.resume(&task_id, request.expected_revision, human_context()))
}

#[tauri::command]
pub fn scheduled_task_delete(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    request: RevisionRequestV1,
) -> IpcResult<()> {
    ipc(service.delete(&task_id, request.expected_revision, human_context()))
}

#[tauri::command]
pub fn scheduled_task_run_now(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
) -> IpcResult<ScheduledTaskRunV1> {
    ipc(service.inner().run_now(&task_id))
}

#[tauri::command]
pub fn scheduled_task_retry_run(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
    run_id: String,
) -> IpcResult<ScheduledTaskRunV1> {
    ipc(service.inner().retry_run(&task_id, &run_id))
}

#[tauri::command]
pub fn scheduled_task_list_runs(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
) -> IpcResult<Vec<ScheduledTaskRunV1>> {
    ipc(service.list_runs(&task_id))
}

#[tauri::command]
pub fn scheduled_task_list_audit(
    service: State<'_, Arc<ScheduledTaskService>>,
    task_id: String,
) -> IpcResult<Vec<ScheduledTaskAuditEventV1>> {
    ipc(service.list_audit(&task_id))
}
