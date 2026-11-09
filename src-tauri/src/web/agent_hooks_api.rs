//! Authenticated v3 facade. Raw hook ingestion is never mounted here.
use super::{fs_api::IpcBody, ws::AppState};
use crate::agent_hooks::{commands, contract::*, state};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterRequest {
    adapter_id: AdapterId,
}
fn result<T: Serialize>(result: Result<T, HookErrorDto>) -> Response {
    match result {
        Ok(value) => Json(IpcBody::ok(value)).into_response(),
        Err(error) => {
            let status = match error.code {
                HookErrorCode::ValidationError => StatusCode::BAD_REQUEST,
                HookErrorCode::Forbidden => StatusCode::FORBIDDEN,
                HookErrorCode::Conflict
                | HookErrorCode::IdentityStale
                | HookErrorCode::PlanExpired => StatusCode::CONFLICT,
                HookErrorCode::UnsupportedPlatform | HookErrorCode::UnsupportedAgentChat => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                HookErrorCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let code = serde_json::to_value(error.code)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "INTERNAL_ERROR".into());
            (status, Json(IpcBody::<T>::err(error.message, &code))).into_response()
        }
    }
}
pub async fn status() -> Response {
    result(commands::agent_hooks_status_cmd().await)
}
pub async fn reload() -> Response {
    result(commands::agent_hooks_reload_cmd().await)
}
pub async fn probe(Json(body): Json<AdapterRequest>) -> Response {
    result(commands::agent_hooks_probe_cmd(body.adapter_id).await)
}
pub async fn preview(Json(request): Json<HookPreviewRequest>) -> Response {
    result(commands::agent_hooks_preview_cmd(request).await)
}
pub async fn confirm(Json(request): Json<HookConfirmRequest>) -> Response {
    result(commands::agent_hooks_confirm_cmd(request).await)
}
pub async fn install(Json(request): Json<HookConfirmRequest>) -> Response {
    result(commands::agent_hooks_install_cmd(request).await)
}
pub async fn uninstall(Json(request): Json<HookConfirmRequest>) -> Response {
    result(commands::agent_hooks_uninstall_cmd(request).await)
}
pub async fn sync(
    State(app): State<AppState>,
    Query(request): Query<HookStateSyncRequest>,
) -> Response {
    Json(IpcBody::ok(state::sync(&request, &|project| {
        app.registry.find_path(project).is_some()
    })))
    .into_response()
}
pub async fn resume_plan(
    State(app): State<AppState>,
    Json(request): Json<ResumePlanRequest>,
) -> Response {
    let home = match commands::home() {
        Ok(home) => home,
        Err(error) => return result::<ResumePlanDto>(Err(error)),
    };
    let (project, owner) = match commands::restore_ownership(
        &home,
        &request.descriptor_id,
        Some(&request.project_id),
        &app.registry,
    ) {
        Ok(value) => value,
        Err(error) => return result::<ResumePlanDto>(Err(error)),
    };
    let context = commands::restore_context(&home, &project, &owner, &app.terminal);
    result(crate::cli_session::restore_service::resume_plan(&context, request).await)
}
pub async fn resume_spawn(
    State(app): State<AppState>,
    Json(request): Json<ResumeSpawnRequest>,
) -> Response {
    let home = match commands::home() {
        Ok(home) => home,
        Err(error) => return result::<ResumeSpawnResult>(Err(error)),
    };
    let (project, owner) =
        match commands::restore_ownership(&home, &request.descriptor_id, None, &app.registry) {
            Ok(value) => value,
            Err(error) => return result::<ResumeSpawnResult>(Err(error)),
        };
    let context = commands::restore_context(&home, &project, &owner, &app.terminal);
    result(crate::cli_session::restore_service::resume_spawn(&context, request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_error_codes_map_to_http_status() {
        for (code, status) in [
            (HookErrorCode::Forbidden, 403),
            (HookErrorCode::Conflict, 409),
            (HookErrorCode::UnsupportedPlatform, 422),
            (HookErrorCode::ValidationError, 400),
        ] {
            let response = result::<()>(Err(HookErrorDto {
                code,
                message: "Operation rejected".into(),
            }));
            assert_eq!(response.status().as_u16(), status);
        }
    }
}
