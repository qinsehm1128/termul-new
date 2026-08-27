use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::scheduled_tasks::{
    ActivateTaskRequestV1, RevisionRequestV1, ScheduleSpecV1, ScheduledTaskDraftInputV1,
    ScheduledTaskService, TaskMutationContextV1, UpdateDraftRequestV1,
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

fn service(state: &AppState) -> Result<Arc<ScheduledTaskService>, IpcBody<()>> {
    state.acp.scheduled_tasks().ok_or_else(|| {
        IpcBody::err(
            "scheduled task service is unavailable",
            "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
        )
    })
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
    let service = match service(&state) {
        Ok(service) => service,
        Err(body) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(IpcBody {
                    success: false,
                    data: None,
                    error: body.error,
                    code: body.code,
                }),
            )
        }
    };
    match service.preview(&request.schedule, request.count) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskV1>>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(IpcBody::err(
                "scheduled task service is unavailable",
                "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
            )),
        );
    };
    match service.list_tasks(query.project_id.as_deref()) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(IpcBody::err(
                "scheduled task service is unavailable",
                "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
            )),
        );
    };
    match service.get_task(&task_id) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn create_draft(
    State(state): State<AppState>,
    Json(input): Json<ScheduledTaskDraftInputV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable();
    };
    match service.create_draft(input, TaskMutationContextV1::default()) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn update_draft(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<UpdateDraftRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable();
    };
    match service.update_draft(
        &task_id,
        request.expected_revision,
        request.input,
        TaskMutationContextV1::default(),
    ) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn activate(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<ActivateTaskRequestV1>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable();
    };
    match service.activate(
        &task_id,
        request.expected_revision,
        &request.expected_draft_hash,
        TaskMutationContextV1::default(),
    ) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
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
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable();
    };
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
    match result {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<RevisionRequestV1>,
) -> (StatusCode, Json<IpcBody<()>>) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(IpcBody::err(
                "scheduled task service is unavailable",
                "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
            )),
        );
    };
    match service.delete(
        &task_id,
        request.expected_revision,
        TaskMutationContextV1::default(),
    ) {
        Ok(()) => ok(()),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn run_now(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskRunV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable_run();
    };
    match service.run_now(&task_id) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn retry_run(
    State(state): State<AppState>,
    Path((task_id, run_id)): Path<(String, String)>,
) -> (
    StatusCode,
    Json<IpcBody<crate::scheduled_tasks::ScheduledTaskRunV1>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return unavailable_run();
    };
    match service.retry_run(&task_id, &run_id) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn list_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskRunV1>>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(IpcBody::err(
                "scheduled task service is unavailable",
                "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
            )),
        );
    };
    match service.list_runs(&task_id) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
}

pub async fn list_audit(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> (
    StatusCode,
    Json<IpcBody<Vec<crate::scheduled_tasks::ScheduledTaskAuditEventV1>>>,
) {
    let Some(service) = state.acp.scheduled_tasks() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(IpcBody::err(
                "scheduled task service is unavailable",
                "SCHEDULED_TASK_SERVICE_UNAVAILABLE",
            )),
        );
    };
    match service.list_audit(&task_id) {
        Ok(value) => ok(value),
        Err(error_value) => error(error_value.to_string()),
    }
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
