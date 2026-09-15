//! Tauri IPC for Agent Skills discovery.

use super::{list_agent_skills, read_agent_skill, AgentSkillContent, AgentSkillSummary};
use crate::skills::api_types::{
    SkillsCatalogRequest, SkillsError, SkillsInstallRequest, SkillsProjectionRequest,
    SkillsRepairRequest, SkillsStatus,
};
use crate::skills::manifest::SkillManifest;
use crate::skills::service::SkillsHubService;
use std::sync::Arc;

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
