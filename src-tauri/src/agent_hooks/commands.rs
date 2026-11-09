//! First-party desktop facade. Hook mutations remain manage-only; no plugin grant flow.
use super::{contract::*, install};
use std::path::PathBuf;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

pub(crate) fn home() -> Result<PathBuf, HookErrorDto> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| HookErrorDto {
            code: HookErrorCode::InternalError,
            message: "Home directory unavailable".into(),
        })
}
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, HookErrorDto> + Send + 'static,
) -> Result<T, HookErrorDto> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| HookErrorDto {
            code: HookErrorCode::InternalError,
            message: "Hook operation failed".into(),
        })?
}
#[tauri::command]
pub async fn agent_hooks_status_cmd() -> Result<Vec<AdapterStatusDto>, HookErrorDto> {
    let home = home()?;
    blocking(move || install::adapters_status(&home)).await
}
#[tauri::command]
pub async fn agent_hooks_reload_cmd() -> Result<Vec<AdapterStatusDto>, HookErrorDto> {
    let home = home()?;
    blocking(move || install::adapters_reload(&home)).await
}
#[tauri::command]
pub async fn agent_hooks_probe_cmd(
    adapter_id: AdapterId,
) -> Result<AdapterStatusDto, HookErrorDto> {
    let home = home()?;
    blocking(move || install::adapter_probe(&home, adapter_id.as_str())).await
}
#[tauri::command]
pub async fn agent_hooks_preview_cmd(
    request: HookPreviewRequest,
) -> Result<HookPreviewDto, HookErrorDto> {
    let home = home()?;
    blocking(move || install::preview_v3(&home, request)).await
}
#[tauri::command]
pub async fn agent_hooks_confirm_cmd(
    request: HookConfirmRequest,
) -> Result<HookMutationResult, HookErrorDto> {
    let home = home()?;
    blocking(move || install::confirm_v3(&home, request, None)).await
}
#[tauri::command]
pub async fn agent_hooks_install_cmd(
    request: HookConfirmRequest,
) -> Result<HookMutationResult, HookErrorDto> {
    let home = home()?;
    blocking(move || install::confirm_v3(&home, request, Some(HookAction::Install))).await
}
#[tauri::command]
pub async fn agent_hooks_uninstall_cmd(
    request: HookConfirmRequest,
) -> Result<HookMutationResult, HookErrorDto> {
    let home = home()?;
    blocking(move || install::confirm_v3(&home, request, Some(HookAction::Uninstall))).await
}
#[tauri::command]
pub async fn agent_hooks_open_folder_cmd(app: tauri::AppHandle) -> Result<(), HookErrorDto> {
    let root = home()?.join(".se-manager/agent-hooks/adapters");
    std::fs::create_dir_all(&root).map_err(|_| HookErrorDto {
        code: HookErrorCode::InternalError,
        message: "Cannot open adapter directory".into(),
    })?;
    app.opener()
        .open_path(root.to_string_lossy(), None::<&str>)
        .map_err(|_| HookErrorDto {
            code: HookErrorCode::InternalError,
            message: "Cannot open adapter directory".into(),
        })
}
#[tauri::command]
pub fn agent_hooks_state_cmd(
    request: HookStateSyncRequest,
    projects: State<'_, std::sync::Arc<crate::web::ProjectRegistry>>,
) -> HookStateSyncResponse {
    super::state::sync(&request, &|project| projects.find_path(project).is_some())
}
#[tauri::command]
pub async fn agent_hooks_resume_plan_cmd(
    request: ResumePlanRequest,
    terminal: State<'_, crate::core::TerminalServiceHandle>,
    projects: State<'_, std::sync::Arc<crate::web::ProjectRegistry>>,
) -> Result<ResumePlanDto, HookErrorDto> {
    let home = home()?;
    let (project, owner) = restore_ownership(
        &home,
        &request.descriptor_id,
        Some(&request.project_id),
        &projects,
    )?;
    let context = restore_context(&home, &project, &owner, &terminal);
    crate::cli_session::restore_service::resume_plan(&context, request).await
}
#[tauri::command]
pub async fn agent_hooks_resume_spawn_cmd(
    request: ResumeSpawnRequest,
    terminal: State<'_, crate::core::TerminalServiceHandle>,
    projects: State<'_, std::sync::Arc<crate::web::ProjectRegistry>>,
) -> Result<ResumeSpawnResult, HookErrorDto> {
    let home = home()?;
    let (project, owner) = restore_ownership(&home, &request.descriptor_id, None, &projects)?;
    let context = restore_context(&home, &project, &owner, &terminal);
    crate::cli_session::restore_service::resume_spawn(&context, request).await
}
/// This is a binding check, not an authentication check. HTTP callers must have
/// terminal-create authority before entering it; desktop uses the trusted host UI.
pub(crate) fn restore_ownership(
    home: &std::path::Path,
    descriptor: &str,
    expected_project: Option<&str>,
    projects: &crate::web::ProjectRegistry,
) -> Result<(String, String), HookErrorDto> {
    let (project, owner) =
        crate::cli_session::identity_store::descriptor_ownership(home, descriptor)?;
    if projects.find_path(&project).is_none()
        || expected_project.is_some_and(|expected| expected != project)
    {
        return Err(HookErrorDto {
            code: HookErrorCode::Forbidden,
            message: "Project is unavailable".into(),
        });
    }
    Ok((project, owner))
}
pub(crate) fn restore_context<'a>(
    home: &'a std::path::Path,
    project_id: &'a str,
    owner_session_id: &'a str,
    terminal: &'a crate::core::TerminalServiceHandle,
) -> crate::cli_session::restore_service::RestoreContext<'a> {
    crate::cli_session::restore_service::RestoreContext {
        home,
        host_instance_id: super::state::instance_id(),
        project_id,
        owner_session_id,
        automatic: false,
        auto_resume_enabled: false,
        terminal_service: terminal,
    }
}
