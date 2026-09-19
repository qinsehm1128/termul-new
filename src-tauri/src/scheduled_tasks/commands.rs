use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::json;
use tauri::State;

use crate::commands::IpcResult;

use super::{
    ActivateTaskRequestV1, RevisionRequestV1, SchedulePreviewV1, ScheduleSpecV1,
    ScheduledTaskAuditEventV1, ScheduledTaskDraftInputV1, ScheduledTaskRunV1, ScheduledTaskService,
    ScheduledTaskV1, TaskMutationContextV1, UpdateDraftRequestV1,
};

/// Optional in-process scheduled-task service. `None` in ACP-Core desktop mode
/// (commands proxy over `AcpServiceHandle`).
#[derive(Clone, Default)]
pub struct HostScheduledTasks(pub Option<Arc<ScheduledTaskService>>);

fn human_context() -> TaskMutationContextV1 {
    TaskMutationContextV1::default()
}

fn ipc<T>(result: super::store::Result<T>) -> IpcResult<T> {
    match result {
        Ok(value) => IpcResult::success(value),
        Err(error) => IpcResult::error(error.to_string(), "SCHEDULED_TASK_ERROR"),
    }
}

async fn via_core<T: DeserializeOwned>(
    client: &crate::core::AcpCoreClient,
    method: &str,
    params: serde_json::Value,
) -> IpcResult<T> {
    match client.request(method, params).await {
        Ok(value) => match serde_json::from_value(value) {
            Ok(data) => IpcResult::success(data),
            Err(error) => IpcResult::error(error.to_string(), "ACP_CORE"),
        },
        Err(error) => IpcResult::error(error.command_message(), "ACP_CORE"),
    }
}

fn unavailable<T>() -> IpcResult<T> {
    IpcResult::error("scheduled task service unavailable", "SCHEDULED_TASK_ERROR")
}

#[tauri::command]
pub async fn scheduled_task_preview(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    schedule: ScheduleSpecV1,
    count: Option<usize>,
) -> Result<IpcResult<SchedulePreviewV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskPreview",
            json!({ "schedule": schedule, "count": count }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.preview(&schedule, count.unwrap_or(5))))
}

#[tauri::command]
pub async fn scheduled_task_list(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    project_id: Option<String>,
) -> Result<IpcResult<Vec<ScheduledTaskV1>>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskList",
            json!({ "projectId": project_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.list_tasks(project_id.as_deref())))
}

#[tauri::command]
pub async fn scheduled_task_get(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskGet",
            json!({ "taskId": task_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.get_task(&task_id)))
}

#[tauri::command]
pub async fn scheduled_task_draft_create(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    input: ScheduledTaskDraftInputV1,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskDraftCreate",
            json!({ "input": input }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.create_draft(input, human_context())))
}

#[tauri::command]
pub async fn scheduled_task_draft_update(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    request: UpdateDraftRequestV1,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskDraftUpdate",
            json!({ "taskId": task_id, "request": request }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.update_draft(
        &task_id,
        request.expected_revision,
        request.input,
        human_context(),
    )))
}

#[tauri::command]
pub async fn scheduled_task_activate(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    request: ActivateTaskRequestV1,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskActivate",
            json!({ "taskId": task_id, "request": request }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.activate(
        &task_id,
        request.expected_revision,
        &request.expected_draft_hash,
        human_context(),
    )))
}

#[tauri::command]
pub async fn scheduled_task_pause(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    request: RevisionRequestV1,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskPause",
            json!({ "taskId": task_id, "request": request }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.pause(
        &task_id,
        request.expected_revision,
        human_context(),
    )))
}

#[tauri::command]
pub async fn scheduled_task_resume(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    request: RevisionRequestV1,
) -> Result<IpcResult<ScheduledTaskV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskResume",
            json!({ "taskId": task_id, "request": request }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.resume(
        &task_id,
        request.expected_revision,
        human_context(),
    )))
}

#[tauri::command]
pub async fn scheduled_task_delete(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    request: RevisionRequestV1,
) -> Result<IpcResult<()>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskDelete",
            json!({ "taskId": task_id, "request": request }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.delete(
        &task_id,
        request.expected_revision,
        human_context(),
    )))
}

#[tauri::command]
pub async fn scheduled_task_run_now(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
) -> Result<IpcResult<ScheduledTaskRunV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskRunNow",
            json!({ "taskId": task_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.run_now(&task_id)))
}

#[tauri::command]
pub async fn scheduled_task_retry_run(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
    run_id: String,
) -> Result<IpcResult<ScheduledTaskRunV1>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskRetryRun",
            json!({ "taskId": task_id, "runId": run_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.retry_run(&task_id, &run_id)))
}

#[tauri::command]
pub async fn scheduled_task_list_runs(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
) -> Result<IpcResult<Vec<ScheduledTaskRunV1>>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskListRuns",
            json!({ "taskId": task_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.list_runs(&task_id)))
}

#[tauri::command]
pub async fn scheduled_task_list_audit(
    acp: State<'_, crate::core::AcpServiceHandle>,
    host: State<'_, HostScheduledTasks>,
    task_id: String,
) -> Result<IpcResult<Vec<ScheduledTaskAuditEventV1>>, String> {
    if let Some(client) = acp.core_client() {
        return Ok(via_core(
            client.as_ref(),
            "scheduledTaskListAudit",
            json!({ "taskId": task_id }),
        )
        .await);
    }
    let Some(service) = host.0.as_ref() else {
        return Ok(unavailable());
    };
    Ok(ipc(service.list_audit(&task_id)))
}
