use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::scheduled_tasks::{
    ActivateTaskRequestV1, RevisionRequestV1, ScheduleSpecV1, ScheduledTaskDraftInputV1,
    TaskMutationContextV1, UpdateDraftRequestV1,
};
use crate::web::fs_api::IpcBody;
use crate::web::ws::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskListQuery {
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewRequest {
    schedule: ScheduleSpecV1,
    #[serde(default = "preview_count")]
    count: usize,
}

fn preview_count() -> usize {
    5
}

async fn via_core<T: serde::de::DeserializeOwned>(
    state: &AppState,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let value = state.acp.scheduled_task_request(method, params).await?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn ok<T>(value: T) -> (StatusCode, Json<IpcBody<T>>) {
    (StatusCode::OK, Json(IpcBody::ok(value)))
}

fn error<T>(detail: impl Into<String>) -> (StatusCode, Json<IpcBody<T>>) {
    (
        StatusCode::OK,
        Json(IpcBody::err(detail, "SCHEDULED_TASK_ERROR")),
    )
}

pub async fn preview(
    State(state): State<AppState>,
    Json(request): Json<PreviewRequest>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::SchedulePreviewV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.preview(&request.schedule, request.count) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_PREVIEW,
            serde_json::json!({
                "schedule": request.schedule,
                "count": request.count,
            }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody {
            success: false,
            data: None,
            error: Some("scheduled task service is unavailable".to_string()),
            code: Some("SCHEDULED_TASK_SERVICE_UNAVAILABLE".to_string()),
        }),
    )
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskV1>>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.list_tasks(query.project_id.as_deref()) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_LIST,
            serde_json::json!({ "projectId": query.project_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

pub async fn get(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.get_task(&task_id) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_GET,
            serde_json::json!({ "taskId": task_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

pub async fn create_draft(
    State(state): State<AppState>,
    Json(input): Json<ScheduledTaskDraftInputV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.create_draft(input, TaskMutationContextV1::default()) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_DRAFT_CREATE,
            serde_json::json!({ "input": input }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable()
}

pub async fn update_draft(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<UpdateDraftRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.update_draft(
            &task_id,
            request.expected_revision,
            request.input,
            TaskMutationContextV1::default(),
        ) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_DRAFT_UPDATE,
            serde_json::json!({ "taskId": task_id, "request": request }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable()
}

pub async fn activate(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<ActivateTaskRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.activate(
            &task_id,
            request.expected_revision,
            &request.expected_draft_hash,
            TaskMutationContextV1::default(),
        ) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_ACTIVATE,
            serde_json::json!({ "taskId": task_id, "request": request }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable()
}

pub async fn pause(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<RevisionRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    mutate_status(state, task_id, request, false).await
}

pub async fn resume(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<RevisionRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    mutate_status(state, task_id, request, true).await
}

async fn mutate_status(
    state: AppState,
    task_id: String,
    request: RevisionRequestV1,
    resume: bool,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        let result = if resume {
            service.resume(
                &task_id,
                request.expected_revision,
                TaskMutationContextV1::default(),
            )
        } else {
            service.pause(
                &task_id,
                request.expected_revision,
                TaskMutationContextV1::default(),
            )
        };
        return match result {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        let method = if resume {
            crate::core::acp::METHOD_SCHEDULED_TASK_RESUME
        } else {
            crate::core::acp::METHOD_SCHEDULED_TASK_PAUSE
        };
        return match via_core(
            &state,
            method,
            serde_json::json!({ "taskId": task_id, "request": request }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable()
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<RevisionRequestV1>,
) -> (StatusCode, Json<IpcBody<()>>) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.delete(
            &task_id,
            request.expected_revision,
            TaskMutationContextV1::default(),
        ) {
            Ok(()) => ok(()),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_DELETE,
            serde_json::json!({ "taskId": task_id, "request": request }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

pub async fn run_now(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskRunV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.run_now(&task_id) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_RUN_NOW,
            serde_json::json!({ "taskId": task_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable_run()
}

pub async fn retry_run(
    State(state): State<AppState>,
    Path((task_id, run_id)): Path<(String, String)>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskRunV1>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.retry_run(&task_id, &run_id) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_RETRY_RUN,
            serde_json::json!({ "taskId": task_id, "runId": run_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    unavailable_run()
}

pub async fn list_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskRunV1>>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.list_runs(&task_id) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_LIST_RUNS,
            serde_json::json!({ "taskId": task_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

pub async fn list_audit(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskAuditEventV1>>>,
) {
    if let Some(service) = state.acp.scheduled_tasks() {
        return match service.list_audit(&task_id) {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value.to_string()),
        };
    }
    if state.acp.is_core_backed() {
        return match via_core(
            &state,
            crate::core::acp::METHOD_SCHEDULED_TASK_LIST_AUDIT,
            serde_json::json!({ "taskId": task_id }),
        )
        .await
        {
            Ok(value) => ok(value),
            Err(error_value) => error(error_value),
        };
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

fn unavailable() -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}

fn unavailable_run() -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskRunV1>>,
) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )),
    )
}
