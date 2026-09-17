//! Tauri IPC for Agent Skills discovery.

use super::{list_agent_skills, read_agent_skill, AgentSkillContent, AgentSkillSummary};
use crate::skills::api_types::{
    SkillsCatalogRequest, SkillsError, SkillsInstallCommit, SkillsInstallRequest,
    SkillsOperationStart, SkillsOperationStatus, SkillsPreviewRequest, SkillsProjectionRequest,
    SkillsRepairRequest, SkillsStatus,
};
use crate::skills::manifest::SkillManifest;
use crate::skills::service::SkillsHubService;
use std::sync::Arc;
use tauri::Emitter;

#[tauri::command]
pub async fn skills_status_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    project_id: Option<String>,
    project_root: Option<String>,
) -> Result<SkillsStatus, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .status(SkillsCatalogRequest {
                project_id,
                project_root,
            })
            .map_err(SkillsError::into_string)
    })
    .await
    .map_err(|error| format!("skills status task failed: {error}"))?
}

#[tauri::command]
pub async fn skills_refresh_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    project_id: Option<String>,
    project_root: Option<String>,
) -> Result<SkillsStatus, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .sync(SkillsCatalogRequest {
                project_id,
                project_root,
            })
            .map_err(SkillsError::into_string)
    })
    .await
    .map_err(|error| format!("skills refresh task failed: {error}"))?
}

#[tauri::command]
pub async fn skills_sync_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    project_id: Option<String>,
    project_root: Option<String>,
) -> Result<SkillsStatus, String> {
    skills_refresh_cmd(service, project_id, project_root).await
}

#[tauri::command]
pub async fn skills_preview_cmd(
    app: tauri::AppHandle,
    service: tauri::State<'_, Arc<SkillsHubService>>,
    request: SkillsPreviewRequest,
) -> Result<SkillsOperationStart, String> {
    let start = service
        .start_preview_job(request)
        .map_err(SkillsError::into_string)?;
    watch_operation_progress(app, Arc::clone(&service), start.job_id.clone());
    Ok(start)
}

#[tauri::command]
pub async fn skills_operation_status_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    job_id: String,
) -> Result<SkillsOperationStatus, String> {
    service
        .operation_status(&job_id)
        .map_err(SkillsError::into_string)
}

#[tauri::command]
pub async fn skills_cancel_operation_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    job_id: String,
) -> Result<(), String> {
    service
        .cancel_operation(&job_id)
        .map_err(SkillsError::into_string)
}

#[tauri::command]
pub async fn skills_install_preview_cmd(
    app: tauri::AppHandle,
    service: tauri::State<'_, Arc<SkillsHubService>>,
    request: SkillsInstallCommit,
) -> Result<SkillsOperationStart, String> {
    let start = service
        .start_install_job(request)
        .map_err(SkillsError::into_string)?;
    watch_operation_progress(app, Arc::clone(&service), start.job_id.clone());
    Ok(start)
}

fn watch_operation_progress(app: tauri::AppHandle, service: Arc<SkillsHubService>, job_id: String) {
    tokio::spawn(async move {
        for _ in 0..1800 {
            let Ok(status) = service.operation_status(&job_id) else {
                return;
            };
            let terminal = matches!(
                status.phase.as_str(),
                "completed" | "preview_ready" | "failed" | "cancelled"
            );
            let _ = app.emit("skills_operation_progress", &status);
            if terminal {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

#[tauri::command]
pub async fn skills_install_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    request: SkillsInstallRequest,
) -> Result<SkillManifest, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || service.install(request).map_err(SkillsError::into_string))
        .await
        .map_err(|error| format!("skills install task failed: {error}"))?
}

#[tauri::command]
pub async fn skills_project_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    request: SkillsProjectionRequest,
) -> Result<SkillManifest, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || {
        service
            .project_skill(request)
            .map_err(SkillsError::into_string)
    })
    .await
    .map_err(|error| format!("skills project task failed: {error}"))?
}

#[tauri::command]
pub async fn skills_repair_cmd(
    service: tauri::State<'_, Arc<SkillsHubService>>,
    request: SkillsRepairRequest,
) -> Result<SkillManifest, String> {
    let service = Arc::clone(&service);
    tokio::task::spawn_blocking(move || service.repair(request).map_err(SkillsError::into_string))
        .await
        .map_err(|error| format!("skills repair task failed: {error}"))?
}

#[tauri::command]
pub async fn list_agent_skills_cmd(
    project_root: Option<String>,
) -> Result<Vec<AgentSkillSummary>, String> {
    list_agent_skills(project_root.as_deref())
}

#[tauri::command]
pub async fn read_agent_skill_cmd(
    name: String,
    project_root: Option<String>,
) -> Result<AgentSkillContent, String> {
    read_agent_skill(&name, project_root.as_deref())
}
