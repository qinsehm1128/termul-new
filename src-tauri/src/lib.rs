// Module declarations
mod acp;
mod acp_binary_install;
mod acp_registry_snapshot;
mod agent_registry;
mod browser_tab_manager;
mod cli_session;
mod commands;
pub mod conversation;
mod editor_workspaces;
mod host_admission;
mod logging;
mod macos_permissions;
mod migrations;
mod path_validation;
mod pty;
mod remote;
pub mod scheduled_tasks;
mod secure_storage;
// Opt-in `termul-server` self-update subsystem. The library module itself is
// intentionally NOT feature-gated so its full test suite — including signature
// verification — runs under the spec's default `cargo test` gate. Only the
// standalone binary wiring (server_main.rs) is gated by `standalone-server`.
pub mod server_update;
mod shell_paths;
// Desktop-side channel manifest fetch for the insider/nightly updater path.
// Routes the manifest fetch through Rust (reqwest) so CSP/CORS do not block it.
mod skills;
mod ssh;
mod trackers;
mod updater_api;
pub mod web;
mod worktree;

#[cfg(target_os = "windows")]
use crate::shell_paths::git_bash_paths;
use migrations::MigrationManager;
use remote::RemoteServerState;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, RwLock};
use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[cfg(not(target_os = "linux"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

const MENU_ID_CHECK_FOR_UPDATES: &str = "check-for-updates";
const MENU_ID_RELOAD: &str = "view-reload";
const MENU_ID_TOGGLE_DEVTOOLS: &str = "view-toggle-devtools";
const MENU_ID_ZOOM_RESET: &str = "view-zoom-reset";
const MENU_ID_ZOOM_IN: &str = "view-zoom-in";
const MENU_ID_ZOOM_OUT: &str = "view-zoom-out";
const MENU_ID_TOGGLE_FULLSCREEN: &str = "view-toggle-fullscreen";
const MENU_ID_LEARN_MORE: &str = "help-learn-more";
const MENU_ID_REVEAL_LOGS: &str = "help-reveal-logs";
const MENU_ID_EXPORT_LOG_FILE: &str = "help-export-log-file";
const MENU_ID_COPY_LOG_CONTENTS: &str = "help-copy-log-contents";
const MENU_ID_EXPORT_LOG_DEFAULT: &str = "help-export-log-default";
const MENU_ID_CLOSE_TAB: &str = "window-close-tab";
const MENU_ID_SELECT_ALL: &str = "edit-select-all";
const MENU_EVENT_CLOSE_TAB: &str = "menu:close-tab";
/// Must stay equal to `MENU_EVENT_SELECT_ALL` in
/// `src/renderer/hooks/use-menu-select-all.ts` — the renderer listens for this
/// exact topic, and a rename on one side silently disables Select All.
const MENU_EVENT_SELECT_ALL: &str = "menu:select-all";
const MENU_EVENT_CHECK_FOR_UPDATES_TRIGGERED: &str = "updater:check-for-updates-triggered";

// Tray menu IDs
const TRAY_ID: &str = "termul-tray";
const TRAY_MENU_SHOW: &str = "tray-show";
const TRAY_MENU_QUIT: &str = "tray-quit";
const TRAY_QUIT_REQUESTED_EVENT: &str = "tray:quit-requested";
const LEARN_MORE_URL: &str = "https://github.com/qinsehm1128/termul-new";
const DEFAULT_ZOOM_FACTOR: f64 = 1.0;
const MIN_ZOOM_FACTOR: f64 = 0.5;
const MAX_ZOOM_FACTOR: f64 = 3.0;
const ZOOM_STEP: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeUiLanguage {
    En,
    ZhCn,
}

static NATIVE_UI_LANGUAGE: OnceLock<RwLock<NativeUiLanguage>> = OnceLock::new();

#[cfg(test)]
const NATIVE_APP_MENU_LABEL_KEYS: &[&str] = &[
    "menu.file",
    "menu.edit",
    "menu.view",
    "menu.window",
    "menu.help",
    "menu.undo",
    "menu.redo",
    "menu.cut",
    "menu.copy",
    "menu.paste",
    "menu.selectAll",
    "menu.reload",
    "menu.actualSize",
    "menu.zoomIn",
    "menu.zoomOut",
    "menu.toggleFullscreen",
    "menu.toggleDevtools",
    "menu.closeTab",
    "menu.minimize",
    "menu.maximize",
    "menu.closeWindow",
    "menu.checkUpdates",
    "menu.learnMore",
    "menu.revealLogs",
    "menu.exportLog",
    "menu.copyLogs",
    "menu.exportLogDefault",
    "menu.about",
    "menu.services",
    "menu.hide",
    "menu.hideOthers",
    "menu.showAll",
    "menu.quit",
    "tray.show",
    "tray.quit",
    "dialog.error",
    "dialog.success",
    "dialog.copied",
    "log.files",
    "log.resolvePath",
    "log.notFound",
    "log.exportSuccess",
    "log.exportFailed",
    "log.copyFailed",
    "log.copySuccess",
    "log.readFailed",
    "log.defaultDirFailed",
];

#[derive(Debug, Default, PartialEq, Eq)]
struct AppMenuSuspensionState {
    suspended: bool,
}

impl AppMenuSuspensionState {
    fn is_suspended(&self) -> bool {
        self.suspended
    }

    fn suspend(&mut self) {
        self.suspended = true;
    }

    fn restore(&mut self) {
        self.suspended = false;
    }
}

static APP_MENU_SUSPENSION_STATE: OnceLock<Mutex<AppMenuSuspensionState>> = OnceLock::new();

fn app_menu_suspension_state() -> &'static Mutex<AppMenuSuspensionState> {
    APP_MENU_SUSPENSION_STATE.get_or_init(|| Mutex::new(AppMenuSuspensionState::default()))
}

/// Serializes native UI language updates (language state + app-menu rebuild +
/// tray replacement) so concurrent `set_native_ui_language` calls cannot
/// reorder and leave stale labels installed. Distinct from the app-menu
/// suspension state, which only tracks menu rebuild suspension.
static NATIVE_UI_LANGUAGE_STATE: OnceLock<Mutex<()>> = OnceLock::new();

fn native_ui_language_state() -> &'static Mutex<()> {
    NATIVE_UI_LANGUAGE_STATE.get_or_init(|| Mutex::new(()))
}

fn native_ui_language() -> NativeUiLanguage {
    *NATIVE_UI_LANGUAGE
        .get_or_init(|| RwLock::new(NativeUiLanguage::En))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn update_native_ui_language(language: &str) {
    let language = if language.eq_ignore_ascii_case("zh-CN") {
        NativeUiLanguage::ZhCn
    } else {
        NativeUiLanguage::En
    };
    *NATIVE_UI_LANGUAGE
        .get_or_init(|| RwLock::new(NativeUiLanguage::En))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = language;
}

fn native_label_for(language: NativeUiLanguage, key: &str) -> &str {
    match (language, key) {
        (NativeUiLanguage::ZhCn, "menu.file") => "文件",
        (NativeUiLanguage::ZhCn, "menu.edit") => "编辑",
        (NativeUiLanguage::ZhCn, "menu.view") => "视图",
        (NativeUiLanguage::ZhCn, "menu.window") => "窗口",
        (NativeUiLanguage::ZhCn, "menu.help") => "帮助",
        (NativeUiLanguage::ZhCn, "menu.undo") => "撤销",
        (NativeUiLanguage::ZhCn, "menu.redo") => "重做",
        (NativeUiLanguage::ZhCn, "menu.cut") => "剪切",
        (NativeUiLanguage::ZhCn, "menu.copy") => "复制",
        (NativeUiLanguage::ZhCn, "menu.paste") => "粘贴",
        (NativeUiLanguage::ZhCn, "menu.selectAll") => "全选",
        (NativeUiLanguage::ZhCn, "menu.reload") => "重新加载",
        (NativeUiLanguage::ZhCn, "menu.actualSize") => "实际大小",
        (NativeUiLanguage::ZhCn, "menu.zoomIn") => "放大",
        (NativeUiLanguage::ZhCn, "menu.zoomOut") => "缩小",
        (NativeUiLanguage::ZhCn, "menu.toggleFullscreen") => "切换全屏",
        (NativeUiLanguage::ZhCn, "menu.toggleDevtools") => "切换开发者工具",
        (NativeUiLanguage::ZhCn, "menu.closeTab") => "关闭标签页",
        (NativeUiLanguage::ZhCn, "menu.minimize") => "最小化",
        (NativeUiLanguage::ZhCn, "menu.maximize") => "最大化",
        (NativeUiLanguage::ZhCn, "menu.closeWindow") => "关闭窗口",
        (NativeUiLanguage::ZhCn, "menu.checkUpdates") => "检查更新...",
        (NativeUiLanguage::ZhCn, "menu.learnMore") => "了解更多",
        (NativeUiLanguage::ZhCn, "menu.revealLogs") => "显示日志文件",
        (NativeUiLanguage::ZhCn, "menu.exportLog") => "导出日志文件...",
        (NativeUiLanguage::ZhCn, "menu.copyLogs") => "复制日志内容",
        (NativeUiLanguage::ZhCn, "menu.exportLogDefault") => "将日志导出到默认目录",
        (NativeUiLanguage::ZhCn, "menu.about") => "关于 Termul Manager",
        (NativeUiLanguage::ZhCn, "menu.services") => "服务",
        (NativeUiLanguage::ZhCn, "menu.hide") => "隐藏 Termul",
        (NativeUiLanguage::ZhCn, "menu.hideOthers") => "隐藏其他应用",
        (NativeUiLanguage::ZhCn, "menu.showAll") => "全部显示",
        (NativeUiLanguage::ZhCn, "menu.quit") => "退出 Termul",
        (NativeUiLanguage::ZhCn, "tray.show") => "显示 Termul",
        (NativeUiLanguage::ZhCn, "tray.quit") => "退出 Termul",
        (NativeUiLanguage::ZhCn, "dialog.error") => "错误",
        (NativeUiLanguage::ZhCn, "dialog.success") => "成功",
        (NativeUiLanguage::ZhCn, "dialog.copied") => "已复制",
        (NativeUiLanguage::ZhCn, "log.files") => "日志文件",
        (NativeUiLanguage::ZhCn, "log.resolvePath") => "无法确定日志文件路径",
        (NativeUiLanguage::ZhCn, "log.notFound") => "日志文件尚不存在",
        (NativeUiLanguage::ZhCn, "log.exportSuccess") => "日志文件已成功导出到",
        (NativeUiLanguage::ZhCn, "log.exportFailed") => "导出日志文件失败",
        (NativeUiLanguage::ZhCn, "log.copyFailed") => "复制到剪贴板失败",
        (NativeUiLanguage::ZhCn, "log.copySuccess") => "日志内容已复制到剪贴板。",
        (NativeUiLanguage::ZhCn, "log.readFailed") => "读取日志文件失败",
        (NativeUiLanguage::ZhCn, "log.defaultDirFailed") => "无法确定默认目录（下载或桌面）",
        (_, "menu.file") => "File",
        (_, "menu.edit") => "Edit",
        (_, "menu.view") => "View",
        (_, "menu.window") => "Window",
        (_, "menu.help") => "Help",
        (_, "menu.undo") => "Undo",
        (_, "menu.redo") => "Redo",
        (_, "menu.cut") => "Cut",
        (_, "menu.copy") => "Copy",
        (_, "menu.paste") => "Paste",
        (_, "menu.selectAll") => "Select All",
        (_, "menu.reload") => "Reload",
        (_, "menu.actualSize") => "Actual Size",
        (_, "menu.zoomIn") => "Zoom In",
        (_, "menu.zoomOut") => "Zoom Out",
        (_, "menu.toggleFullscreen") => "Toggle Full Screen",
        (_, "menu.toggleDevtools") => "Toggle DevTools",
        (_, "menu.closeTab") => "Close Tab",
        (_, "menu.minimize") => "Minimize",
        (_, "menu.maximize") => "Maximize",
        (_, "menu.closeWindow") => "Close Window",
        (_, "menu.checkUpdates") => "Check for Updates...",
        (_, "menu.learnMore") => "Learn More",
        (_, "menu.revealLogs") => "Reveal Log File",
        (_, "menu.exportLog") => "Export Log File...",
        (_, "menu.copyLogs") => "Copy Log Contents",
        (_, "menu.exportLogDefault") => "Export Log to Default Directory",
        (_, "menu.about") => "About Termul Manager",
        (_, "menu.services") => "Services",
        (_, "menu.hide") => "Hide Termul",
        (_, "menu.hideOthers") => "Hide Others",
        (_, "menu.showAll") => "Show All",
        (_, "menu.quit") => "Quit Termul",
        (_, "tray.show") => "Show Termul",
        (_, "tray.quit") => "Quit Termul",
        (_, "dialog.error") => "Error",
        (_, "dialog.success") => "Success",
        (_, "dialog.copied") => "Copied",
        (_, "log.files") => "Log Files",
        (_, "log.resolvePath") => "Could not resolve log file path",
        (_, "log.notFound") => "Log file does not exist yet",
        (_, "log.exportSuccess") => "Log file successfully exported to",
        (_, "log.exportFailed") => "Failed to export log file",
        (_, "log.copyFailed") => "Failed to copy to clipboard",
        (_, "log.copySuccess") => "Log contents successfully copied to clipboard.",
        (_, "log.readFailed") => "Failed to read log file",
        (_, "log.defaultDirFailed") => {
            "Could not resolve a default directory (Downloads or Desktop)"
        }
        _ => key,
    }
}

fn native_label(key: &'static str) -> &'static str {
    native_label_for(native_ui_language(), key)
}

struct ViewMenuState {
    zoom_factor: Mutex<f64>,
}

impl Default for ViewMenuState {
    fn default() -> Self {
        Self {
            zoom_factor: Mutex::new(DEFAULT_ZOOM_FACTOR),
        }
    }
}

#[cfg(target_os = "windows")]
fn resolve_executable_from_path(command: &str) -> Option<String> {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    if command.contains('\\') || command.contains('/') {
        let candidate = Path::new(command);
        return candidate.exists().then(|| command.to_string());
    }

    let path_var = env::var_os("PATH")?;
    let pathext_var =
        env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));

    let command_path = Path::new(command);
    let has_extension = command_path.extension().is_some();

    let mut extensions: Vec<OsString> = Vec::new();
    if has_extension {
        extensions.push(OsString::new());
    } else {
        extensions.push(OsString::new());
        for ext in pathext_var
            .to_string_lossy()
            .split(';')
            .filter(|s| !s.trim().is_empty())
        {
            extensions.push(OsString::from(ext.trim()));
        }
    }

    for dir in env::split_paths(&path_var) {
        for ext in &extensions {
            let candidate: PathBuf = if ext.is_empty() {
                dir.join(command)
            } else {
                dir.join(format!("{}{}", command, ext.to_string_lossy()))
            };
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }

    None
}

// Re-exports for commands
pub use acp::{
    AcpCatalogService, AcpInstallService, AcpManager, ChatHistoryStore, FileProjectRegistry,
    SessionPersistence, WorkspaceManifestService,
};
pub fn set_acp_npm_local_root(path: std::path::PathBuf) {
    acp::npm_local::set_root(path);
}
// Host-injected `plan` MCP tool: the `--internal-mcp-plan-server`
// subcommand branch in `main.rs` + `server_main.rs` reaches `host_mcp::CHILD_ARG`
// + `host_mcp::child::run()` through this re-export (the `acp` module itself is
// private). See `acp/host_mcp/mod.rs` + spec `spec-acp-host-todo-plan-tool.md`.
pub use acp::host_mcp;
pub use conversation::{
    AgentSessionBinding, ConversationErrorCode, ConversationId, ConversationLifecycleState,
    ConversationRecordV2, CreationPartition, ExecutionTarget, ProjectAttachment,
    TerminalResourceRef,
};
pub use pty::PtyManager;
pub use scheduled_tasks::ScheduledTaskStore;
pub use trackers::{CwdTracker, ExitCodeTracker, GitTracker, TerminalEventHub};
// Desktop ACP event sink: wraps the Tauri `AppHandle` so the dispatcher's
// `Vec<Arc<dyn EventSink>>` fan-out reaches the renderer as `acp:*` events
// (byte-for-byte unchanged from before Story 1.1). The headless `termul-server`
// binary (Story 1.2) will instead pass a `WsRelaySink`-backed list with no
// `AppHandle` at all.
use web::{
    PermissionRendezvous, ProjectRegistry, QuestionRendezvous, RemoteAccessAuthority,
    TauriEventSink, WsRelaySink,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ShellInfo {
    pub name: String,
    pub path: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DetectedShells {
    pub available: Vec<ShellInfo>,
    pub default: Option<ShellInfo>,
}

/// Cache for shell detection results to avoid repeated `where` command spawns
static AVAILABLE_SHELLS_CACHE: OnceLock<Vec<ShellInfo>> = OnceLock::new();
static CACHE_CALL_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[tauri::command]
fn detect_shells() -> Result<DetectedShells, String> {
    detect_shells_inner()
}

/// Reusable shell-detection entry point (same logic as the `detect_shells`
/// Tauri command, without the `#[tauri::command]` macro). The HTTP `/shells`
/// route (`web::fs_api::shells`) calls this directly so the web/remote path
/// can reach shell detection without a Tauri runtime.
pub(crate) fn detect_shells_inner() -> Result<DetectedShells, String> {
    let count = CACHE_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    log::debug!("[ShellDetect] detect_shells called (call #{})", count);

    let shells = AVAILABLE_SHELLS_CACHE.get_or_init(|| {
        log::debug!("[ShellDetect] Computing available shells (cached)");
        get_available_shells()
    });
    let default = get_default_shell_info();

    Ok(DetectedShells {
        available: shells.clone(),
        default,
    })
}

#[tauri::command]
fn get_default_shell() -> Result<ShellInfo, String> {
    get_default_shell_info().ok_or_else(|| "No default shell found".to_string())
}

#[tauri::command]
fn get_home_directory() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(env::var("USERPROFILE")
            .or_else(|_| env::var("HOME"))
            .unwrap_or_else(|_| "C:\\".to_string()))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
    }
}

/// Temporarily remove the native application menu so its keyboard accelerators
/// (e.g. `Cmd+W`, `Cmd+R`, `Cmd+C`) stop intercepting key events before they
/// reach the webview. The renderer's shortcut recorder calls this while it is
/// capturing a keybinding so the user can record any combination, including
/// ones that collide with a menu accelerator. No-op on Linux (no app menu).
#[tauri::command]
fn suspend_app_menu(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(not(target_os = "linux"))]
    {
        let mut state = app_menu_suspension_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.is_suspended() {
            app.remove_menu().map_err(|e| e.to_string())?;
            state.suspend();
            log::debug!("[native-ui] app menu suspended");
        }
    }
    #[cfg(target_os = "linux")]
    let _ = &app;
    Ok(())
}

/// Restore the native application menu removed by `suspend_app_menu`. Rebuilds
/// the menu from scratch so accelerators resume working once recording ends.
#[tauri::command]
fn restore_app_menu(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(not(target_os = "linux"))]
    {
        let mut state = app_menu_suspension_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.is_suspended() {
            let menu = build_app_menu(&app).map_err(|e| e.to_string())?;
            app.set_menu(menu).map_err(|e| e.to_string())?;
            state.restore();
            log::debug!("[native-ui] app menu restored");
        }
    }
    #[cfg(target_os = "linux")]
    let _ = &app;
    Ok(())
}

#[tauri::command]
fn set_native_ui_language(app: tauri::AppHandle, language: String) -> Result<(), String> {
    // Serialize the complete native UI update: language state + app-menu rebuild
    // + tray replacement, so a stale concurrent call cannot install older labels.
    let _guard = native_ui_language_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    update_native_ui_language(&language);

    #[cfg(not(target_os = "linux"))]
    {
        let state = app_menu_suspension_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.is_suspended() {
            log::debug!("[native-ui] deferred app menu language rebuild while suspended");
        } else {
            let menu = build_app_menu(&app).map_err(|error| {
                log::error!(
                    "[native-ui] build_app_menu failed for language={}: {error}",
                    language
                );
                error.to_string()
            })?;
            app.set_menu(menu).map_err(|error| {
                log::error!(
                    "[native-ui] app.set_menu failed for language={}: {error}",
                    language
                );
                error.to_string()
            })?;
        }
    }

    #[cfg(desktop)]
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let menu = build_tray_menu(&app).map_err(|error| {
            log::error!(
                "[native-ui] build_tray_menu failed for language={}: {error}",
                language
            );
            error.to_string()
        })?;
        tray.set_menu(Some(menu)).map_err(|error| {
            log::error!(
                "[native-ui] tray.set_menu failed for language={}: {error}",
                language
            );
            error.to_string()
        })?;
        let _ = tray.set_tooltip(Some("Termul Manager"));
    }

    log::info!("[native-ui] applied language={}", language);
    Ok(())
}

#[tauri::command]
fn reveal_log_dir_command(app: tauri::AppHandle) -> Result<(), String> {
    reveal_log_dir(&app)
}

#[tauri::command]
fn export_log_file_command(app: tauri::AppHandle) -> Result<(), String> {
    export_log_file(&app)
}

#[tauri::command]
fn copy_log_contents_command(app: tauri::AppHandle) -> Result<(), String> {
    copy_log_contents(&app)
}

#[tauri::command]
fn export_log_to_default_command(app: tauri::AppHandle) -> Result<(), String> {
    export_log_to_default(&app)
}

/// Probe the macOS privacy grants the settings panel reports on.
///
/// `active` names the ids whose probe may make macOS show a prompt; anything
/// omitted comes back as `notProbed`. Runs off the UI thread because a probe
/// spawns `codesign` and touches the network stack.
#[tauri::command]
async fn macos_permissions_report_command(
    app: tauri::AppHandle,
    active: Option<Vec<String>>,
) -> Result<macos_permissions::PermissionReport, String> {
    let bundle_id = Some(app.config().identifier.clone());
    let active = active.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        macos_permissions::collect_report(&active, bundle_id)
    })
    .await
    .map_err(|error| error.to_string())
}

/// Open the System Settings pane governing a probe id.
///
/// Takes an id, never a URL: the mapping lives in `macos_permissions` so the
/// renderer cannot hand an arbitrary URL to the system opener. Waits for the
/// launch to report back — off the UI thread — so a failure surfaces in the
/// panel instead of leaving a button that appears to do nothing.
#[tauri::command]
async fn macos_open_privacy_pane_command(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || macos_permissions::open_privacy_pane(&id))
        .await
        .map_err(|error| error.to_string())?
}

fn get_default_shell_info() -> Option<ShellInfo> {
    #[cfg(target_os = "windows")]
    {
        let comspec = env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let (name, display_name) = if comspec.to_lowercase().contains("powershell") {
            ("powershell", "PowerShell")
        } else {
            ("cmd", "Command Prompt")
        };
        Some(ShellInfo {
            name: name.to_string(),
            path: comspec,
            display_name: display_name.to_string(),
            args: None,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let shell = env::var("SHELL").ok()?;
        let name = shell.split('/').next_back().unwrap_or("sh").to_string();
        let display_name = shell_display_name(&name);
        Some(ShellInfo {
            name,
            path: shell,
            display_name,
            args: None,
        })
    }
}

fn shell_display_name(name: &str) -> String {
    match name {
        "powershell" => "PowerShell".to_string(),
        "pwsh" => "PowerShell 7".to_string(),
        "cmd" => "Command Prompt".to_string(),
        "git-bash" => "Git Bash".to_string(),
        "wsl" => "WSL".to_string(),
        "bash" => "Bash".to_string(),
        "zsh" => "Zsh".to_string(),
        "fish" => "Fish".to_string(),
        "sh" => "Shell".to_string(),
        other => other.to_string(),
    }
}

fn get_available_shells() -> Vec<ShellInfo> {
    let mut shells: Vec<ShellInfo> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // CRITICAL: Check explicit paths FIRST, then PATH entries
        // This ensures the correct shell is found when multiple versions exist
        let mut candidates = vec![
            // PowerShell 7 explicit paths (checked first)
            ("pwsh", r"C:\Program Files\PowerShell\7\pwsh.exe", None),
            ("pwsh", r"C:\Program Files\PowerShell\6\pwsh.exe", None),
            // Windows PowerShell 5 (explicit path)
            (
                "powershell",
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                None,
            ),
            // PATH-based fallbacks (checked last)
            ("pwsh", "pwsh.exe", None),
            ("powershell", "powershell.exe", None),
            ("cmd", "cmd.exe", None),
            ("wsl", "wsl.exe", None),
        ];

        // Git Bash via PATH
        candidates.push(("git-bash", "bash.exe", None));

        // Add primary paths from shared constants
        for path in git_bash_paths::PRIMARY_PATHS {
            candidates.push(("git-bash", path, None));
        }

        // Add fallback paths from shared constants
        for path in git_bash_paths::FALLBACK_PATHS {
            candidates.push(("git-bash", path, None));
        }

        for (name, path, args) in candidates {
            if is_shell_available(path) {
                // Skip duplicate names
                if !shells.iter().any(|s| s.name == name) {
                    shells.push(ShellInfo {
                        name: name.to_string(),
                        path: path.to_string(),
                        display_name: shell_display_name(name),
                        args: args.map(|a: &str| vec![a.to_string()]),
                    });
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut push_unique = |name: &str, path: &str| {
            if !is_shell_available(path) {
                return;
            }
            if shells
                .iter()
                .any(|existing| existing.name == name || existing.path == path)
            {
                return;
            }
            shells.push(ShellInfo {
                name: name.to_string(),
                path: path.to_string(),
                display_name: shell_display_name(name),
                args: None,
            });
        };

        // Prefer the login SHELL so picking "Zsh" matches Ghostty / Terminal.app.
        if let Ok(login) = env::var("SHELL") {
            if let Some(name) = Path::new(&login).file_name().and_then(|s| s.to_str()) {
                push_unique(name, &login);
            }
        }

        for prefix in crate::shell_paths::unix_shell_paths::PREFIXES {
            for name in ["zsh", "bash", "fish", "sh"] {
                push_unique(name, &format!("{prefix}/{name}"));
            }
        }
    }

    shells
}

#[cfg(target_os = "windows")]
fn is_builtin_windows_shell(shell_path: &str) -> bool {
    let normalized = shell_path.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
            | "wsl"
            | "wsl.exe"
    )
}

fn is_shell_available(shell_path: &str) -> bool {
    log::debug!("[ShellDetect] Checking availability: {}", shell_path);
    #[cfg(target_os = "windows")]
    {
        if !shell_path.contains('\\') && !shell_path.contains('/') {
            if is_builtin_windows_shell(shell_path) {
                log::debug!(
                    "[ShellDetect] Built-in Windows shell, skipping PATH resolution: {}",
                    shell_path
                );
                return true;
            }

            let resolved = resolve_executable_from_path(shell_path);
            if resolved.is_some() {
                log::debug!(
                    "[ShellDetect] Resolved from PATH without spawning cmd: {}",
                    shell_path
                );
            }
            return resolved.is_some();
        }

        Path::new(shell_path).exists()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Path::new(shell_path).exists()
    }
}

/// Register default application migrations
///
/// This function is called during app setup to register all known migrations.
/// Add new migrations here as the application schema evolves.
fn register_default_migrations(_manager: &MigrationManager) {
    // Intentionally left empty until real migrations are implemented.
}

fn get_main_webview_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<tauri::WebviewWindow<R>, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "Main webview window not found".to_string())
}

fn set_zoom_factor<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    zoom_factor: f64,
) -> Result<(), String> {
    let state = app
        .try_state::<ViewMenuState>()
        .ok_or_else(|| "View menu state is not initialized".to_string())?;
    let mut current_zoom = state
        .zoom_factor
        .lock()
        .map_err(|_| "View menu zoom state is unavailable".to_string())?;

    let clamped_zoom = zoom_factor.clamp(MIN_ZOOM_FACTOR, MAX_ZOOM_FACTOR);
    get_main_webview_window(app)?
        .set_zoom(clamped_zoom)
        .map_err(|error| error.to_string())?;
    *current_zoom = clamped_zoom;
    Ok(())
}

fn adjust_zoom_factor<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    delta: f64,
) -> Result<(), String> {
    let state = app
        .try_state::<ViewMenuState>()
        .ok_or_else(|| "View menu state is not initialized".to_string())?;
    let current_zoom = state
        .zoom_factor
        .lock()
        .map_err(|_| "View menu zoom state is unavailable".to_string())?;
    let next_zoom = (*current_zoom + delta).clamp(MIN_ZOOM_FACTOR, MAX_ZOOM_FACTOR);
    drop(current_zoom);

    set_zoom_factor(app, next_zoom)
}

fn toggle_fullscreen<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let webview_window = get_main_webview_window(app)?;
    let is_fullscreen = webview_window
        .is_fullscreen()
        .map_err(|error| error.to_string())?;
    webview_window
        .set_fullscreen(!is_fullscreen)
        .map_err(|error| error.to_string())
}

fn reload_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    get_main_webview_window(app)?
        .reload()
        .map_err(|error| error.to_string())
}

#[cfg(debug_assertions)]
fn toggle_devtools<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let webview_window = get_main_webview_window(app)?;

    if webview_window.is_devtools_open() {
        webview_window.close_devtools();
    } else {
        webview_window.open_devtools();
    }

    Ok(())
}

#[cfg(not(debug_assertions))]
fn toggle_devtools<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Result<(), String> {
    Err("DevTools are not available in this build".to_string())
}

#[cfg(target_os = "windows")]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
fn build_tray_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};

    let show_item =
        MenuItemBuilder::with_id(TRAY_MENU_SHOW, native_label("tray.show")).build(app)?;
    let quit_item =
        MenuItemBuilder::with_id(TRAY_MENU_QUIT, native_label("tray.quit")).build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;

    MenuBuilder::new(app)
        .item(&show_item)
        .item(&separator)
        .item(&quit_item)
        .build()
}

#[cfg(not(target_os = "linux"))]
fn build_app_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    #[cfg(not(target_os = "macos"))]
    let file_menu = SubmenuBuilder::new(app, native_label("menu.file"))
        .quit_with_text(native_label("menu.quit"))
        .build()?;

    // Select All is NOT the predefined item. On macOS that one carries the
    // Cmd+A key equivalent and fires `selectAll:`, and an NSMenu key equivalent
    // is consumed by AppKit before the event ever reaches the webview — so
    // CodeMirror's own `Mod-a` binding never runs. `selectAll:` then selects the
    // *DOM*, and CodeMirror keeps only the lines near the viewport in the DOM,
    // so "select all + copy" in the editor yielded just the rendered slice.
    // Routing the accelerator through the renderer lets the focused editor
    // select its whole document; `use-menu-select-all.ts` falls back to the
    // document-wide selection the predefined item used to provide.
    let select_all = MenuItemBuilder::with_id(MENU_ID_SELECT_ALL, native_label("menu.selectAll"))
        .accelerator("CmdOrCtrl+A")
        .build(app)?;

    let edit_menu = SubmenuBuilder::new(app, native_label("menu.edit"))
        .undo_with_text(native_label("menu.undo"))
        .redo_with_text(native_label("menu.redo"))
        .separator()
        .cut_with_text(native_label("menu.cut"))
        .copy_with_text(native_label("menu.copy"))
        .paste_with_text(native_label("menu.paste"))
        .item(&select_all)
        .build()?;

    let reload = MenuItemBuilder::with_id(MENU_ID_RELOAD, native_label("menu.reload"))
        .accelerator("CmdOrCtrl+R")
        .build(app)?;
    let zoom_reset =
        MenuItemBuilder::with_id(MENU_ID_ZOOM_RESET, native_label("menu.actualSize")).build(app)?;
    let zoom_in =
        MenuItemBuilder::with_id(MENU_ID_ZOOM_IN, native_label("menu.zoomIn")).build(app)?;
    let zoom_out =
        MenuItemBuilder::with_id(MENU_ID_ZOOM_OUT, native_label("menu.zoomOut")).build(app)?;
    let toggle_fullscreen = MenuItemBuilder::with_id(
        MENU_ID_TOGGLE_FULLSCREEN,
        native_label("menu.toggleFullscreen"),
    )
    .build(app)?;

    let view_menu = {
        let builder = SubmenuBuilder::new(app, native_label("menu.view")).item(&reload);

        #[cfg(debug_assertions)]
        let builder = {
            let toggle_devtools = MenuItemBuilder::with_id(
                MENU_ID_TOGGLE_DEVTOOLS,
                native_label("menu.toggleDevtools"),
            )
            .accelerator("CmdOrCtrl+Shift+I")
            .build(app)?;
            builder.item(&toggle_devtools)
        };

        builder
            .separator()
            .item(&zoom_reset)
            .item(&zoom_in)
            .item(&zoom_out)
            .separator()
            .item(&toggle_fullscreen)
            .build()?
    };

    #[cfg(target_os = "macos")]
    let window_menu = {
        let close_tab = MenuItemBuilder::with_id(MENU_ID_CLOSE_TAB, native_label("menu.closeTab"))
            .accelerator("Cmd+W")
            .build(app)?;
        SubmenuBuilder::new(app, native_label("menu.window"))
            .minimize_with_text(native_label("menu.minimize"))
            .maximize_with_text(native_label("menu.maximize"))
            .separator()
            .item(&close_tab)
            .build()?
    };

    #[cfg(not(target_os = "macos"))]
    let window_menu = SubmenuBuilder::new(app, native_label("menu.window"))
        .minimize_with_text(native_label("menu.minimize"))
        .maximize_with_text(native_label("menu.maximize"))
        .separator()
        .close_window_with_text(native_label("menu.closeWindow"))
        .build()?;

    let check_for_updates =
        MenuItemBuilder::with_id(MENU_ID_CHECK_FOR_UPDATES, native_label("menu.checkUpdates"))
            .accelerator("CmdOrCtrl+Shift+U")
            .build(app)?;
    let learn_more =
        MenuItemBuilder::with_id(MENU_ID_LEARN_MORE, native_label("menu.learnMore")).build(app)?;
    let reveal_logs =
        MenuItemBuilder::with_id(MENU_ID_REVEAL_LOGS, native_label("menu.revealLogs"))
            .build(app)?;
    let export_log_file =
        MenuItemBuilder::with_id(MENU_ID_EXPORT_LOG_FILE, native_label("menu.exportLog"))
            .build(app)?;
    let copy_log_contents =
        MenuItemBuilder::with_id(MENU_ID_COPY_LOG_CONTENTS, native_label("menu.copyLogs"))
            .build(app)?;
    let export_log_default = MenuItemBuilder::with_id(
        MENU_ID_EXPORT_LOG_DEFAULT,
        native_label("menu.exportLogDefault"),
    )
    .build(app)?;

    let help_menu = SubmenuBuilder::new(app, native_label("menu.help"))
        .item(&check_for_updates)
        .separator()
        .item(&reveal_logs)
        .item(&export_log_file)
        .item(&copy_log_contents)
        .item(&export_log_default)
        .item(&learn_more)
        .build()?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = SubmenuBuilder::new(app, app.package_info().name.clone())
            .about_with_text(native_label("menu.about"), None)
            .separator()
            .services_with_text(native_label("menu.services"))
            .separator()
            .hide_with_text(native_label("menu.hide"))
            .hide_others_with_text(native_label("menu.hideOthers"))
            .show_all_with_text(native_label("menu.showAll"))
            .separator()
            .quit_with_text(native_label("menu.quit"))
            .build()?;

        MenuBuilder::new(app)
            .item(&app_menu)
            .item(&edit_menu)
            .item(&view_menu)
            .item(&window_menu)
            .item(&help_menu)
            .build()
    }

    #[cfg(not(target_os = "macos"))]
    MenuBuilder::new(app)
        .item(&file_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()
}

fn handle_menu_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: tauri::menu::MenuEvent) {
    if event.id() == MENU_ID_CHECK_FOR_UPDATES {
        if let Err(error) = app.emit(MENU_EVENT_CHECK_FOR_UPDATES_TRIGGERED, ()) {
            log::error!("Failed to emit updater menu event: {}", error);
        }
    } else if event.id() == MENU_ID_RELOAD {
        if let Err(error) = reload_main_window(app) {
            log::error!("Failed to reload main window from menu: {}", error);
        }
    } else if event.id() == MENU_ID_TOGGLE_DEVTOOLS {
        if let Err(error) = toggle_devtools(app) {
            log::error!("Failed to toggle devtools from menu: {}", error);
        }
    } else if event.id() == MENU_ID_ZOOM_RESET {
        if let Err(error) = set_zoom_factor(app, DEFAULT_ZOOM_FACTOR) {
            log::error!("Failed to reset zoom from menu: {}", error);
        }
    } else if event.id() == MENU_ID_ZOOM_IN {
        if let Err(error) = adjust_zoom_factor(app, ZOOM_STEP) {
            log::error!("Failed to zoom in from menu: {}", error);
        }
    } else if event.id() == MENU_ID_ZOOM_OUT {
        if let Err(error) = adjust_zoom_factor(app, -ZOOM_STEP) {
            log::error!("Failed to zoom out from menu: {}", error);
        }
    } else if event.id() == MENU_ID_TOGGLE_FULLSCREEN {
        if let Err(error) = toggle_fullscreen(app) {
            log::error!("Failed to toggle fullscreen from menu: {}", error);
        }
    } else if event.id() == MENU_ID_LEARN_MORE {
        if let Err(error) = open_external_url(LEARN_MORE_URL) {
            log::error!("Failed to open Learn More link from menu: {}", error);
        }
    } else if event.id() == MENU_ID_REVEAL_LOGS {
        if let Err(error) = reveal_log_dir(app) {
            log::error!("Failed to reveal log directory from menu: {}", error);
        }
    } else if event.id() == MENU_ID_EXPORT_LOG_FILE {
        if let Err(error) = export_log_file(app) {
            log::error!("Failed to export log file from menu: {}", error);
        }
    } else if event.id() == MENU_ID_COPY_LOG_CONTENTS {
        if let Err(error) = copy_log_contents(app) {
            log::error!("Failed to copy log contents from menu: {}", error);
        }
    } else if event.id() == MENU_ID_EXPORT_LOG_DEFAULT {
        if let Err(error) = export_log_to_default(app) {
            log::error!(
                "Failed to export log to default directory from menu: {}",
                error
            );
        }
    } else if event.id() == MENU_ID_CLOSE_TAB {
        if let Err(error) = app.emit(MENU_EVENT_CLOSE_TAB, ()) {
            log::error!("Failed to emit close-tab menu event: {}", error);
        }
    } else if event.id() == MENU_ID_SELECT_ALL {
        if let Err(error) = app.emit(MENU_EVENT_SELECT_ALL, ()) {
            log::error!("Failed to emit select-all menu event: {}", error);
        }
    }
}

/// Open the OS log directory (where the rotated log file lives) in the system
/// file manager so users can locate and attach it to bug reports (issue #244).
fn reveal_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("could not resolve log directory: {}", e))?;

    // The plugin creates the directory lazily on first write; ensure it exists
    // so revealing it never fails on a fresh install that hasn't logged yet.
    if !log_dir.exists() {
        std::fs::create_dir_all(&log_dir)
            .map_err(|e| format!("could not create log directory: {}", e))?;
    }

    open_external_url(&log_dir.to_string_lossy())
}

fn show_log_action_error<R: tauri::Runtime>(app: &tauri::AppHandle<R>, message: &str) {
    app.dialog()
        .message(message)
        .title(native_label("dialog.error"))
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

fn export_log_file<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let log_path = match logging::log_file_path(app) {
        Some(path) => path,
        None => {
            let msg = native_label("log.resolvePath");
            show_log_action_error(app, msg);
            return Err(msg.to_string());
        }
    };

    if !log_path.exists() {
        let msg = native_label("log.notFound");
        show_log_action_error(app, msg);
        return Err(msg.to_string());
    }

    let app_handle = app.clone();
    app.dialog()
        .file()
        .add_filter(native_label("log.files"), &["log"])
        .set_file_name("termul.log")
        .save_file(move |file_path| {
            if let Some(tauri_plugin_dialog::FilePath::Path(dest_path)) = file_path {
                match std::fs::copy(&log_path, &dest_path) {
                    Ok(_) => {
                        app_handle
                            .dialog()
                            .message(format!(
                                "{} {}",
                                native_label("log.exportSuccess"),
                                dest_path.display()
                            ))
                            .title(native_label("dialog.success"))
                            .kind(MessageDialogKind::Info)
                            .show(|_| {});
                    }
                    Err(e) => {
                        app_handle
                            .dialog()
                            .message(format!("{}: {}", native_label("log.exportFailed"), e))
                            .title(native_label("dialog.error"))
                            .kind(MessageDialogKind::Error)
                            .show(|_| {});
                    }
                }
            }
        });

    Ok(())
}

fn copy_log_contents<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let log_path = match logging::log_file_path(app) {
        Some(path) => path,
        None => {
            let msg = native_label("log.resolvePath");
            show_log_action_error(app, msg);
            return Err(msg.to_string());
        }
    };

    if !log_path.exists() {
        let msg = native_label("log.notFound");
        show_log_action_error(app, msg);
        return Err(msg.to_string());
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        match std::fs::read_to_string(&log_path) {
            Ok(contents) => {
                if let Err(e) = app_handle.clipboard().write_text(contents) {
                    app_handle
                        .dialog()
                        .message(format!("{}: {}", native_label("log.copyFailed"), e))
                        .title(native_label("dialog.error"))
                        .kind(MessageDialogKind::Error)
                        .show(|_| {});
                } else {
                    app_handle
                        .dialog()
                        .message(native_label("log.copySuccess"))
                        .title(native_label("dialog.copied"))
                        .kind(MessageDialogKind::Info)
                        .show(|_| {});
                }
            }
            Err(e) => {
                app_handle
                    .dialog()
                    .message(format!("{}: {}", native_label("log.readFailed"), e))
                    .title(native_label("dialog.error"))
                    .kind(MessageDialogKind::Error)
                    .show(|_| {});
            }
        }
    });

    Ok(())
}

fn export_log_to_default<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let log_path = match logging::log_file_path(app) {
        Some(path) => path,
        None => {
            let msg = native_label("log.resolvePath");
            show_log_action_error(app, msg);
            return Err(msg.to_string());
        }
    };

    if !log_path.exists() {
        let msg = native_label("log.notFound");
        show_log_action_error(app, msg);
        return Err(msg.to_string());
    }

    let default_dir = match app
        .path()
        .download_dir()
        .or_else(|_| app.path().desktop_dir())
    {
        Ok(dir) => dir,
        Err(e) => {
            let msg = format!("{}: {}", native_label("log.defaultDirFailed"), e);
            show_log_action_error(app, &msg);
            return Err(msg);
        }
    };

    let dest_path = default_dir.join("termul.log");
    let app_handle = app.clone();

    tauri::async_runtime::spawn(async move {
        match std::fs::copy(&log_path, &dest_path) {
            Ok(_) => {
                app_handle
                    .dialog()
                    .message(format!(
                        "{} {}",
                        native_label("log.exportSuccess"),
                        dest_path.display()
                    ))
                    .title(native_label("dialog.success"))
                    .kind(MessageDialogKind::Info)
                    .show(|_| {});
            }
            Err(e) => {
                app_handle
                    .dialog()
                    .message(format!("{}: {}", native_label("log.exportFailed"), e))
                    .title(native_label("dialog.error"))
                    .kind(MessageDialogKind::Error)
                    .show(|_| {});
            }
        }
    });

    Ok(())
}

/// Desktop pairing starts empty. The host adopts the settings-file bearer
/// only when remote access is actually started — never on app launch.
fn provision_desktop_remote_authority() -> Result<RemoteAccessAuthority, String> {
    Ok(RemoteAccessAuthority::desktop_memory())
}

fn clear_desktop_remote_generation() {}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DesktopExitDurabilityOutcome {
    pub failures: Vec<&'static str>,
    pub conversation_drain_attempts: usize,
    pub catalog_flush_attempts: usize,
    pub pty_shutdown: Option<crate::pty::manager::PtyShutdownReceipt>,
}

impl DesktopExitDurabilityOutcome {
    #[must_use]
    pub fn clean_success(&self) -> bool {
        self.failures.is_empty()
            && self
                .pty_shutdown
                .is_none_or(|receipt| receipt.clean_success())
    }
}

/// Stop all ACP producers before awaiting the retained Conversation coordinator exactly once.
/// The caller owns later PTY/browser/legacy-store cleanup and converts any failure into a non-zero
/// bounded Desktop exit rather than reporting false clean success.
pub(crate) async fn stop_desktop_producers_and_drain(
    acp_manager: Option<&AcpManager>,
    ws_relay: Option<&WsRelaySink>,
    deadline: tokio::time::Instant,
) -> DesktopExitDurabilityOutcome {
    let mut outcome = DesktopExitDurabilityOutcome::default();
    let producer_stop_failed = match acp_manager {
        Some(acp_manager) => {
            match tokio::time::timeout_at(deadline, acp_manager.stop_producers()).await {
                Ok(Ok(())) => false,
                Ok(Err(_)) | Err(_) => true,
            }
        }
        None => true,
    };
    if producer_stop_failed {
        log::error!(
            "[desktop-exit] shutdown_phase=stop_acp_producers stable_code={} result=FAILED",
            crate::web::ACP_PRODUCER_STOP_FAILED
        );
        outcome.failures.push(crate::web::ACP_PRODUCER_STOP_FAILED);
    }

    outcome.conversation_drain_attempts = 1;
    let drain_result = match ws_relay {
        Some(ws_relay) => {
            ws_relay
                .shutdown_conversation_persistence_until(deadline)
                .await
        }
        None => Err("Conversation relay is unavailable".to_string()),
    };
    if drain_result.is_err() {
        log::error!(
            "[desktop-exit] shutdown_phase=drain_conversation_persistence stable_code={} result=FAILED",
            crate::web::CONVERSATION_PERSISTENCE_DRAIN_FAILED
        );
        outcome
            .failures
            .push(crate::web::CONVERSATION_PERSISTENCE_DRAIN_FAILED);
    } else {
        log::info!(
            "[desktop-exit] shutdown_phase=drain_conversation_persistence stable_code=OK result=PASS"
        );
    }

    outcome.catalog_flush_attempts = 1;
    let catalog_result = match ws_relay {
        Some(ws_relay) => ws_relay.flush_catalog_until(deadline).await,
        None => Err("Conversation relay is unavailable".to_string()),
    };
    match catalog_result {
        Ok(receipt) => log::info!(
            "[desktop-exit] shutdown_phase=flush_conversation_catalog stable_code=OK result=PASS requested_generation={} flushed_generation={} write_count={}",
            receipt.requested_generation,
            receipt.flushed_generation,
            receipt.write_count
        ),
        Err(_) => {
            log::error!(
                "[desktop-exit] shutdown_phase=flush_conversation_catalog stable_code={} result=FAILED",
                crate::web::CONVERSATION_CATALOG_FLUSH_FAILED
            );
            outcome
                .failures
                .push(crate::web::CONVERSATION_CATALOG_FLUSH_FAILED);
        }
    }
    outcome
}

static CLEANUP_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
// Claimed synchronously when we enter the async cleanup path and never reset.
// Prevents a second ExitRequested (e.g. an OS exit signal, or the exit(0) we
// call at the end of cleanup re-entering before the first task finishes) from
// spawning a duplicate cleanup that races kill_all()/destroy_all()/exit(0).
// CLEANUP_DONE still marks final completion so the trailing exit(0) re-entry
// returns immediately via the check above.
static CLEANUP_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Budget for the `RunEvent::Exit` reap. Deliberately tighter than the graceful
/// path's five seconds: this runs inside the platform's terminate callback,
/// which force-kills the process if it overstays, and a partial reap beats being
/// cut off mid-way with nothing killed at all.
const LAST_RESORT_PTY_CLEANUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Slice of the `RunEvent::Exit` budget reserved for winding the ACP agents
/// down, taken before the PTY reap so the two stages share one bounded budget.
///
/// Deliberately small: `stop_producers` SENDS `AcpCommand::Shutdown` on an
/// unbounded channel — which never blocks — before it joins the driver threads,
/// so every agent has already been told to stop by the time this expires. Only
/// the join half is sacrificed, and the agents keep winding down in parallel
/// with the PTY reap that follows.
const LAST_RESORT_ACP_REAP_DEADLINE: std::time::Duration = std::time::Duration::from_millis(800);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Keep the legacy importer linkable for compatibility tests and older internal callers, but
    // never invoke it after the synchronous Conversation bootstrap cutover.
    let _legacy_import_compatibility_symbol = crate::acp::import_chat_history;

    // Install the panic hook before anything can panic so Rust panics are
    // captured to the log file with a backtrace (issue #244).
    logging::install_panic_hook();
    logging::install_desktop_tracing_bridge();

    let builder = tauri::Builder::default();

    // Native menu bar:
    // - macOS: top OS menu (expected, native UX)
    // - Windows: hidden behind decorations:false (custom title bar handles it)
    // - Linux/GTK: would render as a separate widget bar inside the window,
    //   creating a double bar with the custom title bar. Skip the native menu
    //   on Linux and let the custom title bar / shortcuts cover those actions.
    #[cfg(not(target_os = "linux"))]
    let builder = builder
        .menu(build_app_menu)
        .on_menu_event(handle_menu_event);

    #[cfg(target_os = "linux")]
    let builder = builder.on_menu_event(handle_menu_event);

    // Single-instance must be the first plugin: Tauri initializes plugins in
    // registration order, so duplicate launches must be rejected before any
    // other plugin performs setup or side effects. The plugin is desktop-only.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            // Unminimize before focus so the restored window is reliably
            // foregrounded on every platform.
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));

    let mut builder = builder
        // Logging is first among the remaining plugins so the global logger is
        // installed before their setup code emits log lines.
        .plugin(logging::build_log_plugin())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init());

    // MCP Bridge in all builds
    builder = builder.plugin(tauri_plugin_mcp_bridge::init());

    let app = builder
        .setup(|app| {
            let handle = app.handle().clone();

            // Startup diagnostic banner (issue #244): version, OS/arch, build
            // channel, session id, and resolved log path on a single line.
            logging::log_startup_banner(&handle);

            // Conversation admission is the first app-managed storage/resource boundary.
            let app_data_dir = handle
                .path()
                .app_data_dir()
                .map_err(|error| format!("failed to resolve app data directory: {error}"))?;
            let conversation_workspace_base = std::env::var("TERMUL_CONVERSATION_WORKSPACE_ROOT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(std::path::PathBuf::from)
                .or_else(|| handle.path().document_dir().ok().map(|path| path.join("Termul")))
                .or_else(|| {
                    log::warn!(
                        "[conversation-bootstrap] document directory unavailable; using home directory"
                    );
                    handle.path().home_dir().ok().map(|path| path.join("Termul"))
                })
                .ok_or_else(|| {
                    "CONVERSATION_ROOT_INVALID: no document or home directory is available"
                        .to_string()
                })?;
            let conversation_bootstrap = crate::conversation::ConversationBootstrap::run(
                crate::conversation::HostConversationRoots::desktop(
                    app_data_dir.clone(),
                    conversation_workspace_base,
                ),
                crate::conversation::MigrationHostMode::Desktop,
            )
            .map_err(|error| error.to_string())?;
            log::info!(
                "[conversation-bootstrap] desktop repository ready phase={:?} precedence={:?} recovery_count={}",
                conversation_bootstrap.migration_phase,
                conversation_bootstrap.reader_precedence,
                conversation_bootstrap.recovery_item_count
            );
            app.manage(Arc::clone(&conversation_bootstrap.repository));
            app.manage(Arc::clone(&conversation_bootstrap.reader));
            app.manage(Arc::clone(&conversation_bootstrap.creation));
            app.manage(Arc::clone(&conversation_bootstrap.persistence_adapter));
            // Publish the exact bootstrap-owned ordering/shutdown authority. Relay construction
            // below resolves this same core; no second writer task set is admitted.
            app.manage(Arc::clone(&conversation_bootstrap.ordered_persistence));
            app.manage(Arc::clone(&conversation_bootstrap.workspace));
            app.manage(Arc::clone(&conversation_bootstrap.application));
            let conversation_migration_control = Arc::new(
                crate::conversation::ConversationMigrationControlService::new(&app_data_dir)
                    .map_err(|error| error.to_string())?,
            );
            app.manage(conversation_migration_control);

            // Window chrome is configured before show(). macOS overlay settings
            // live in tauri.conf.json — avoid set_decorations(true) there because
            // it resets hiddenTitle/full-size content view. Win/Linux drop native
            // frame so the HTML titlebar owns window controls.
            #[cfg(target_os = "macos")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) = window.set_title_bar_style(tauri::TitleBarStyle::Overlay) {
                        log::warn!("[macOS] Failed to set overlay title bar style: {}", e);
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) = window.set_decorations(false) {
                        log::warn!("Failed to disable native window decorations: {}", e);
                    }
                }
            }

            app.manage(ViewMenuState::default());

            // Transport-neutral terminal event fan-out: desktop events remain
            // byte-compatible while the web terminal socket subscribes to the
            // same lifecycle/metadata stream.
            let terminal_events = TerminalEventHub::tauri(handle.clone());

            let cwd_tracker = Arc::new(CwdTracker::new(terminal_events.clone()));
            app.manage(cwd_tracker.clone());

            let git_tracker = Arc::new(GitTracker::new(
                Some(handle.clone()),
                terminal_events.clone(),
            ));
            app.manage(git_tracker.clone());

            let exit_code_tracker = Arc::new(ExitCodeTracker::new(terminal_events.clone()));
            app.manage(exit_code_tracker.clone());

            let pty_manager = Arc::new(PtyManager::new(
                terminal_events,
                cwd_tracker,
                git_tracker,
                exit_code_tracker,
            ));
            app.manage(pty_manager.clone());

            // Create Browser Tab Manager
            let browser_tab_manager =
                Arc::new(browser_tab_manager::BrowserTabManager::new(handle.clone()));
            app.manage(browser_tab_manager);

            // Desktop renderer chat history lives outside tauri-plugin-store so
            // loading unrelated preferences never materializes full transcripts
            // in the WebView. The app-data path is mandatory for safe startup.
            let chat_history_root = app_data_dir.join("acp-chat-history");
            let chat_history_store = ChatHistoryStore::open_read_only(chat_history_root)
                .map_err(|error| format!("failed to open ACP chat history store: {error}"))?;
            log::info!(
                "[acp-history] store ready path={}",
                chat_history_store.root().display()
            );

            // ConversationRepository is the sole live history writer after bootstrap.
            // The legacy `acp-sessions` root was already inventoried/migrated synchronously and
            // is not opened as a live SessionPersistence store.
            app.manage(chat_history_store);
            app.manage(commands::HostHistoryStore::conversation(
                Arc::clone(&conversation_bootstrap.persistence_adapter),
                None,
            ));

            // CAP-5 / Story 5: open the host-owned workspace-manifests root
            // under `<app_data_dir>/workspace-manifests`. The desktop owns its
            // own root — NEVER shared with a standalone `termul-server` on the
            // same machine (two processes on one JSONL store would corrupt
            // both). `None` degrades to fresh-only mode (the
            // `workspace_manifest_*` commands return `Ok(None)` / idempotent
            // success; the web routes follow suit).
            let workspace_manifests_root = app_data_dir.join("workspace-manifests");
            let workspace_manifest_service =
                match tauri::async_runtime::block_on(WorkspaceManifestService::open_read_only(
                    workspace_manifests_root.clone(),
                )) {
                    Ok(service) => {
                        log::info!(
                            "[workspace-manifest] host service ready path={}",
                            service.root().display()
                        );
                        Some(service)
                    }
                    Err(error) => {
                        // Degrade, don't crash: workspace manifests become
                        // fresh-only, the app must still boot.
                        log::error!(
                            "[workspace-manifest] host service unavailable path={} error={error}",
                            workspace_manifests_root.display()
                        );
                        None
                    }
                };
            app.manage(commands::HostWorkspaceManifestStore::new(
                workspace_manifest_service.clone(),
            ));

            // CAP-6 / Story 8: open the host-owned ACP catalog root under
            // `<app_data_dir>/acp-catalog`. The desktop owns its own root —
            // NEVER shared with a standalone `termul-server` on the same
            // machine. The catalog embeds the trusted `agents.json` at build
            // time, optionally augments with the explicitly-approved CDN
            // registry snapshot, probes real runtime availability, and
            // computes the 5-state `SupportedAcpAgentStatus` per agent.
            // `None` degrades to `ACP_CATALOG_UNAVAILABLE` (the catalog
            // commands/routes return the error code; the app must still boot).
            let acp_catalog_root = handle
                .path()
                .app_data_dir()
                .map_err(|error| format!("failed to resolve app data directory: {error}"))?
                .join("acp-catalog");
            let acp_catalog_service =
                match tauri::async_runtime::block_on(
                    crate::acp::AcpCatalogService::open(acp_catalog_root.clone()),
                ) {
                    Ok(service) => {
                        log::info!(
                            "[acp-catalog] host service ready path={}",
                            service.root().display()
                        );
                        Some(service)
                    }
                    Err(error) => {
                        log::error!(
                            "[acp-catalog] host service unavailable path={} error={error}",
                            acp_catalog_root.display()
                        );
                        None
                    }
                };
            app.manage(commands::HostAcpCatalogStore::new(
                acp_catalog_service.clone(),
            ));

            // CAP-6 / Story 9: open the host-owned verified-atomic ACP install
            // root. The desktop owns its own root under
            // `<app_data_dir>/acp-registry-binaries` (NEVER shared with a
            // standalone `termul-server` on the same machine). The install
            // service downloads + verifies (sha256) + extracts + atomically
            // activates ACP agent archives resolved from the catalog, records
            // an installed-agents manifest, and exposes `install_agent(agentId)`
            // across all three transports. `None` degrades to
            // `ACP_INSTALL_UNAVAILABLE`.
            let acp_install_root = handle
                .path()
                .app_data_dir()
                .map_err(|error| format!("failed to resolve app data directory: {error}"))?
                .join("acp-registry-binaries");
            crate::acp::npm_local::set_root(
                handle
                    .path()
                    .app_data_dir()
                    .map_err(|error| format!("failed to resolve app data directory: {error}"))?
                    .join("acp-npm-packages"),
            );
            let acp_install_service =
                match acp_catalog_service.as_ref().zip(Some(acp_install_root.clone())) {
                    Some((catalog, root)) => {
                        match tauri::async_runtime::block_on(
                            crate::acp::install::AcpInstallService::open(
                                root.clone(),
                                std::sync::Arc::clone(catalog),
                            ),
                        ) {
                            Ok(service) => {
                                log::info!(
                                    "[acp-install] host service ready path={}",
                                    service.root().display()
                                );
                                Some(service)
                            }
                            Err(error) => {
                                log::error!(
                                    "[acp-install] host service unavailable path={} error={error}",
                                    root.display()
                                );
                                None
                            }
                        }
                    }
                    None => {
                        log::warn!(
                            "[acp-install] host service unavailable (catalog store is None)"
                        );
                        None
                    }
                };
            app.manage(commands::HostAcpInstallStore::new(
                acp_install_service.clone(),
            ));

            // Create ACP Manager — spawns/owns ACP agent subprocesses.
            //
            // Desktop mode fans ACP events out to TWO sinks: `TauriEventSink`
            // (the renderer's `acp:*` events, byte-for-byte unchanged) and a
            // `WsRelaySink` (the shared-live web server's per-session event log
            // + subscriber set). `fan_out` serializes once and fans N, so adding
            // the second sink does not change the `TauriEventSink` payloads.
            // With host persistence attached, the relay additionally durables
            // every session-scoped event (the same seam the standalone server
            // uses) — transport-agnostic, so desktop-origin and browser-origin
            // sessions are persisted identically.
            //
            // The shared-live web server (`remote/host.rs`) pulls both
            // `Arc<AcpManager>` and `Arc<WsRelaySink>` as Tauri state and serves
            // the desktop's live sessions to a browser/phone over the LAN.
            let mut sinks: Vec<Arc<dyn crate::web::EventSink>> =
                vec![Arc::new(TauriEventSink::new(handle.clone()))];
            let ws_relay = Arc::new(WsRelaySink::with_conversation_persistence(
                4096,
                Arc::clone(&conversation_bootstrap.persistence_adapter),
                None,
            ));
            let relay_ordered = ws_relay
                .ordered_conversation_persistence()
                .ok_or_else(|| anyhow::anyhow!("desktop relay is missing ordered persistence"))?;
            if !relay_ordered.shares_authority(&conversation_bootstrap.ordered_persistence) {
                return Err(anyhow::anyhow!(
                    "desktop relay did not retain the bootstrap ordering authority"
                )
                .into());
            }
            sinks.push(ws_relay.clone());
            let acp_manager = Arc::new(AcpManager::with_conversation_services(
                sinks,
                Arc::clone(&conversation_bootstrap.creation),
                Arc::clone(&conversation_bootstrap.persistence_adapter),
            ));
            acp_manager.set_pty_manager(&pty_manager);
            conversation_bootstrap
                .application
                .attach_lifecycle(
                    crate::conversation::ConversationLifecycleService::from_manager(
                        Arc::clone(&acp_manager),
                        Arc::clone(&pty_manager),
                    )
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            // Attach the server-side permission rendezvous so a phone can
            // respond to `acp:permission_request` over WS. The desktop renderer
            // still responds via the `acp_respond_permission` Tauri command
            // (direct `AcpManager::respond_permission`); the rendezvous's
            // at-most-one `take_permission` gate ensures whichever path responds
            // first wins.
            //
            // Capture the runtime handle explicitly (`tauri::async_runtime`)
            // rather than relying on `Handle::try_current()` — `setup` runs on
            // the main thread and is not guaranteed to be inside a tokio runtime
            // context, so capturing the handle here keeps `arm_timeout` reliable
            // when it runs later on the agent driver thread.
            let rendezvous = Arc::new(PermissionRendezvous::with_handle_and_policy(
                Arc::clone(&acp_manager),
                std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(15),
                tauri::async_runtime::handle().inner().clone(),
            ));
            ws_relay.set_rendezvous(rendezvous);
            // Attach the server-side question rendezvous so a phone attached
            // to a desktop host can answer structured questions over WS too
            // (desktop renderer answers via the `acp_answer_question` Tauri
            // command; first-response-wins across both paths).
            let question_rendezvous = Arc::new(QuestionRendezvous::with_handle(
                Arc::clone(&acp_manager),
                std::time::Duration::from_secs(60),
                tauri::async_runtime::handle().inner().clone(),
            ));
            ws_relay.set_question_rendezvous(question_rendezvous);
            let scheduled_task_root = app_data_dir.join("scheduled-tasks").join("v1");
            let scheduled_task_store = Arc::new(
                crate::scheduled_tasks::ScheduledTaskStore::open_with_legacy_root(
                    scheduled_task_root.join("catalog"),
                    Some(scheduled_task_root.join("projects")),
                )
                .map_err(|error| format!("failed to open scheduled task store: {error}"))?,
            );
            let scheduled_task_executor = Arc::new(
                crate::scheduled_tasks::AcpScheduledTaskExecutor::new(
                    Arc::clone(&acp_manager),
                    Arc::clone(&ws_relay),
                ),
            );
            let scheduled_tasks = crate::scheduled_tasks::ScheduledTaskService::new(
                scheduled_task_store,
                scheduled_task_executor,
            );
            acp_manager.set_scheduled_tasks(&scheduled_tasks);
            scheduled_tasks.start_on(tauri::async_runtime::handle().inner());
            log::info!(
                "[scheduled-task] boundary=service_started host=desktop root={}",
                scheduled_tasks.store().root().display()
            );
            app.manage(Arc::clone(&scheduled_tasks));
            app.manage(acp_manager);
            app.manage(ws_relay);

            // In-memory project registry (Epic-4 bridge) — renderer-fed via
            // `remote_sync_projects`; the source for `GET /projects` +
            // `switch_project` cwd resolution on the shared-live web server.
            // Lives only while the server runs; cleared on `remote_server_stop`.
            let project_registry = Arc::new(ProjectRegistry::new());
            app.manage(project_registry);

            // Create SSH Manager
            let ssh_manager = Arc::new(ssh::SSHManager::new(handle.clone()));
            app.manage(ssh_manager);

            // Create Migration Manager
            let migration_manager = Arc::new(MigrationManager::new(handle.clone()));
            app.manage(migration_manager.clone());

            // Pairing bearers persist in remote-tunnel/secrets.json while
            // wanted. Start adopts that generation; launch does not touch
            // the OS keyring.
            let tunnel_store = Arc::new(remote::TunnelConfigStore::new(app_data_dir.clone()));
            let remote_authority = Arc::new(provision_desktop_remote_authority()?);
            app.manage(Arc::clone(&remote_authority));

            // The shared-live host receives the exact same authority instance
            // managed above and threads it into the HTTP/ACP WebSocket router.
            let remote_state = Arc::new(
                RemoteServerState::with_desktop_authority(remote_authority)
                    .with_pairing_store(Arc::clone(&tunnel_store)),
            );
            app.manage(remote_state);
            app.manage(tunnel_store);
            app.manage(Arc::new(remote::RemoteAccessIntentStore::new(app_data_dir)));

            // Register default migrations
            register_default_migrations(migration_manager.as_ref());

            let migration_result = migration_manager.run_migrations();
            let mut migration_failures = Vec::new();

            if !migration_result.success {
                migration_failures.push(
                    migration_result
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown migration error".to_string()),
                );
            }

            if let Some(results) = migration_result.data.as_ref() {
                for result in results.iter().filter(|result| !result.success) {
                    migration_failures.push(format!(
                        "Migration {} failed: {}",
                        result.version,
                        result.error.as_deref().unwrap_or("unknown migration error")
                    ));
                }

                if migration_failures.is_empty() && !results.is_empty() {
                    log::info!(
                        "Completed {} data migration(s) during startup",
                        results.len()
                    );
                }
            }

            if !migration_failures.is_empty() {
                let failure_message = format!(
                    "Data migration startup failed:\n{}",
                    migration_failures.join("\n")
                );

                let _ = app.emit("startup-migration-failed", failure_message.clone());
                log::error!("{}", failure_message);

                return Err(anyhow::anyhow!(failure_message).into());
            }

            // ── System Tray Icon ────────────────────────────────────────────
            // Buat tray icon dengan menu klik kanan seperti Telegram.
            // Klik icon → show/focus window.
            // Close button (X) → minimize ke tray, bukan quit.
            #[cfg(desktop)]
            {
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

                let tray_menu = build_tray_menu(&handle)?;

                let _tray = TrayIconBuilder::with_id(TRAY_ID)
                    .tooltip("Termul Manager")
                    .icon(app.default_window_icon().cloned().unwrap())
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event({
                        let app_handle = handle.clone();
                        move |_tray, event| match event.id().as_ref() {
                            id if id == TRAY_MENU_SHOW => {
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                }
                            }
                            id if id == TRAY_MENU_QUIT => {
                                // Let the renderer run the existing dirty-file
                                // prompt and persistence flush before it destroys
                                // the window. Direct app.exit(0) would bypass it.
                                let _ = app_handle.emit_to(
                                    "main",
                                    TRAY_QUIT_REQUESTED_EVENT,
                                    (),
                                );
                            }
                            _ => {}
                        }
                    })
                    .on_tray_icon_event({
                        let app_handle = handle.clone();
                        move |_tray, event| {
                            if let TrayIconEvent::Click {
                                button: MouseButton::Left,
                                button_state: MouseButtonState::Up,
                                ..
                            } = event
                            {
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                }
                            }
                        }
                    })
                    .build(app)?;

            }
            // ── End Tray ────────────────────────────────────────────────────

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Shell detection commands
            detect_shells,
            get_default_shell,
            get_home_directory,
            suspend_app_menu,
            restore_app_menu,
            set_native_ui_language,
            reveal_log_dir_command,
            export_log_file_command,
            copy_log_contents_command,
            export_log_to_default_command,
            // macOS privacy (TCC) settings panel
            macos_permissions_report_command,
            macos_open_privacy_pane_command,
            // Restart-required Conversation migration maintenance
            commands::conversation_migration_control,
            // Terminal commands
            commands::terminal_spawn,
            commands::terminal_resume,
            commands::terminal_attach,
            commands::terminal_watch,
            commands::terminal_rotate_claim,
            commands::terminal_revoke_claim,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_set_display_mode,
            commands::terminal_close_view,
            commands::terminal_terminate,
            commands::terminal_kill,
            commands::terminal_get_cwd,
            commands::terminal_get_git_branch,
            commands::terminal_get_git_status,
            commands::terminal_get_exit_code,
            commands::terminal_update_orphan_detection,
            commands::terminal_add_renderer_ref,
            commands::terminal_remove_renderer_ref,
            commands::terminal_set_protected,
            commands::terminal_set_visibility,
            // Agent registry (ADR-004.6: identity/discovery, opt-in, read-only)
            commands::agent_registry_fetch,
            // Browser tab commands
            commands::browser_tab_create,
            commands::browser_tab_navigate,
            commands::browser_tab_resize,
            commands::browser_tab_show,
            commands::browser_tab_hide,
            commands::browser_tab_destroy,
            commands::browser_tab_go_back,
            commands::browser_tab_go_forward,
            commands::browser_tab_reload,
            commands::browser_tab_open_devtools,
            commands::browser_tab_inject_annotation,
            commands::browser_tab_remove_annotation_overlay,
            commands::browser_tab_inject_annotation_markers,
            commands::browser_tab_update_annotation_marker_selection,
            // Browser tab URL sync commands (called by injected JS)
            commands::browser_tab_report_url,
            commands::browser_tab_report_loaded,
            commands::browser_tab_report_region_captured,
            commands::browser_tab_report_element_captured,
            commands::browser_tab_report_title,
            commands::browser_tab_report_annotation_marker_clicked,
            // Worktree commands
            commands::worktree_list,
            commands::worktree_create,
            commands::worktree_remove,
            commands::worktree_branches,
            commands::worktree_check_dirty,
            commands::worktree_remove_all_managed,
            commands::worktree_parse_gitignore,
            commands::worktree_create_symlinks,
            commands::worktree_ensure_symlinks,
            commands::worktree_archive,
            commands::worktree_restore,
            commands::worktree_merge_preview,
            commands::worktree_merge_execute,
            // Worktree-isolated agent chat (CAP-2 base resolution + CAP-5 include carry-over)
            commands::worktree_resolve_base_branch,
            commands::worktree_copy_include_files,
            // Filesystem/search commands
            commands::search_get_rg_info,
            commands::search_content,
            commands::search_content_stream,
            commands::search_content_cancel,
            commands::search_file_names_stream,
            commands::search_file_names_cancel,
            // Attachment binary reads (brokered; fs:allow-read-file is not granted)
            commands::read_attachment_bytes,
            // SSH commands
            commands::ssh_list_profiles,
            commands::ssh_save_profile,
            commands::ssh_delete_profile,
            commands::ssh_import_config,
            commands::ssh_connect,
            commands::ssh_disconnect,
            commands::ssh_get_connections,
            commands::ssh_port_forward_start,
            commands::ssh_port_forward_stop,
            commands::sftp_list_dir,
            commands::sftp_download,
            commands::sftp_upload,
            commands::sftp_delete,
            commands::sftp_mkdir,
            commands::sftp_rename,
            // SSH askpass helper
            commands::ssh_create_askpass,
            // SFTP file operations
            commands::sftp_read_file,
            commands::sftp_write_file,
            commands::sftp_create_file,
            // Data migration commands
            commands::data_migration_get_version,
            commands::data_migration_get_history,
            commands::data_migration_run_migrations,
            commands::data_migration_get_schema_info,
            commands::data_migration_get_registered,
            commands::data_migration_rollback,
            // Git commands
            commands::git_get_status,
            commands::git_get_diff,
            commands::git_stage,
            commands::git_unstage,
            commands::git_stage_hunk,
            commands::git_unstage_hunk,
            commands::git_discard,
            commands::git_get_log,
            commands::git_commit,
            commands::git_push,
            commands::git_get_commit_context,
            commands::git_init,
            commands::git_checkout_branch,
            commands::git_create_branch,
            commands::git_stash_save,
            commands::git_stash_list,
            commands::git_stash_apply,
            commands::git_stash_pop,
            commands::git_stash_drop,
            commands::git_branch_list,
            commands::git_branch_switch,
            commands::git_branch_create,
            // Secure storage commands
            secure_storage::secure_storage_set,
            secure_storage::secure_storage_get,
            secure_storage::secure_storage_delete,
            // ACP (Agent Client Protocol) commands — ADR-003 P0
            acp::commands::acp_spawn_agent,
            acp::commands::acp_kill_agent,
            acp::commands::acp_list_agents,
            acp::commands::acp_set_permission_policy,
            acp::commands::acp_new_session,
            acp::commands::acp_load_session,
            acp::commands::acp_resume_session,
            acp::commands::acp_close_session,
            acp::commands::acp_dispose_ephemeral_session,
            acp::commands::acp_list_sessions,
            acp::commands::acp_register_discovered_session,
            acp::commands::acp_send_prompt,
            acp::commands::acp_cancel_prompt,
            acp::commands::acp_set_config_option,
            acp::commands::acp_set_mode,
            acp::commands::acp_set_model,
            acp::commands::acp_respond_permission,
            acp::commands::acp_answer_question,
            acp::commands::acp_authenticate,
            acp::commands::acp_probe_runtime,
            acp::commands::acp_set_turn_timeout,
            acp::commands::acp_set_turn_idle_timeout,
            acp::commands::acp_set_session_new_timeout,
            acp::commands::acp_set_session_reopen_timeout,
            acp::commands::acp_set_first_prompt_warmup_timeout,
            acp::commands::acp_set_prefer_local_npm_install,
            acp::commands::acp_probe_mcp_server,
            // CAP-6 / Story 8: ACP catalog (host-owned resolution).
            acp::commands::acp_list_catalog,
            acp::commands::acp_set_catalog_opt_in,
            // CAP-6 / Story 9: ACP install (host-owned verified-atomic install).
            acp::commands::acp_install_agent,
            acp_registry_snapshot::acp_fetch_registry_snapshot,
            acp_binary_install::acp_install_registry_binary,
            // Desktop updater: channel manifest fetch (CSP/CORS-free server-side
            // reqwest for the insider/nightly paths).
            updater_api::updater_fetch_channel_manifest,
            // Agent Skills (Zed-compatible SKILL.md packages)
            skills::commands::list_agent_skills_cmd,
            skills::commands::read_agent_skill_cmd,
            // Host-level AI scheduled tasks
            scheduled_tasks::commands::scheduled_task_preview,
            scheduled_tasks::commands::scheduled_task_list,
            scheduled_tasks::commands::scheduled_task_get,
            scheduled_tasks::commands::scheduled_task_draft_create,
            scheduled_tasks::commands::scheduled_task_draft_update,
            scheduled_tasks::commands::scheduled_task_activate,
            scheduled_tasks::commands::scheduled_task_pause,
            scheduled_tasks::commands::scheduled_task_resume,
            scheduled_tasks::commands::scheduled_task_delete,
            scheduled_tasks::commands::scheduled_task_run_now,
            scheduled_tasks::commands::scheduled_task_retry_run,
            scheduled_tasks::commands::scheduled_task_list_runs,
            scheduled_tasks::commands::scheduled_task_list_audit,
            cli_session::commands::list_cli_sessions_cmd,
            cli_session::commands::resolve_cli_sessions_cmd,
            // Remote server commands
            commands::remote_server_start,
            commands::remote_server_stop,
            commands::remote_server_status,
            commands::remote_access_intent_get,
            commands::remote_access_intent_set,
            commands::remote_server_rotate_credential,
            remote::tunnel::commands::tunnel_config_get,
            remote::tunnel::commands::tunnel_config_set,
            commands::remote_sync_projects,
            commands::list_editor_workspaces,
            commands::parse_code_workspace_file,
            commands::set_host_default_project,
            commands::remote_sync_chat_history,
            commands::remote_sync_mcp_registry,
            // Desktop ACP renderer-history storage
            commands::acp_history_list,
            commands::acp_history_get,
            commands::acp_history_get_page,
            commands::acp_history_save,
            commands::acp_history_delete,
            commands::acp_history_flush,
            commands::acp_history_mark_legacy_import_complete,
            commands::acp_history_list_legacy,
            commands::acp_history_get_legacy,
            // Frontend error forwarding (issue #244)
            commands::log_frontend_error,
            // Shared Conversation application service
            commands::conversation_host_status,
            commands::conversation_list,
            commands::conversation_get,
            commands::conversation_get_binding,
            commands::conversation_rename,
            commands::conversation_open,
            commands::conversation_resolve_legacy_id,
            commands::conversation_attach_project,
            commands::conversation_detach_project,
            commands::conversation_update_execution_target,
            // Per-Conversation SessionWorkspace (Conversation stage 5)
            commands::session_workspace_get,
            commands::session_workspace_write,
            commands::conversation_recovery_resolve,
            // Explicit Conversation Chat/ACP lifecycle
            commands::conversation_detach_binding,
            commands::conversation_rebind_detached_binding,
            commands::conversation_suspend_binding,
            commands::conversation_replace_binding,
            commands::conversation_delete,
            // Workspace manifest (legacy read-only compatibility)
            commands::workspace_manifest_get,
            commands::workspace_manifest_write,
            commands::workspace_manifest_delete,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // `ExitRequested` is emitted by tauri-runtime-wry in exactly two places:
        // when the LAST window is Destroyed, and when `AppHandle::exit()` is
        // called. macOS Cmd+Q takes neither path — `applicationWillTerminate`
        // drives `AppState::exit()` -> `Event::LoopDestroyed` -> `RunEvent::Exit`
        // — so the graceful handler below never ran on a normal quit and every
        // PTY was left orphaned. `Exit` cannot be prevented and the loop is
        // already gone, so this is a bounded, best-effort reap: kill the child
        // processes, skip the durable-persistence stages that need real async
        // time. When the graceful path did run, `CLEANUP_DONE` short-circuits it.
        if matches!(event, RunEvent::Exit) {
            if CLEANUP_DONE.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            // Both managers are resolved independently. The PTY manager used to
            // gate this arm with a `let ... else { return }`, which would have
            // silently skipped the ACP stage as well.
            let acp_manager = app_handle
                .try_state::<Arc<AcpManager>>()
                .map(|state| state.inner().clone());
            let pty_manager = app_handle
                .try_state::<Arc<pty::PtyManager>>()
                .map(|state| state.inner().clone());
            if acp_manager.is_none() && pty_manager.is_none() {
                return;
            }
            tauri::async_runtime::block_on(async move {
                let started = tokio::time::Instant::now();
                // ACP first: an adapter that outlives the app keeps its own
                // children alive (including the injected plan MCP server, whose
                // stdin-EOF exit never fires while the orphaned adapter holds
                // the pipe open). Telling the agents to stop early also lets
                // them wind down while the PTY reap below runs.
                if let Some(acp_manager) = acp_manager {
                    match tokio::time::timeout_at(
                        started + LAST_RESORT_ACP_REAP_DEADLINE,
                        acp_manager.stop_producers(),
                    )
                    .await
                    {
                        Ok(Ok(())) => log::info!(
                            "[desktop-exit] shutdown_phase=stop_acp_producers_on_loop_exit stable_code=OK result=PASS"
                        ),
                        Ok(Err(error)) => log::error!(
                            "[desktop-exit] shutdown_phase=stop_acp_producers_on_loop_exit stable_code={} result=FAILED error={error}",
                            crate::web::ACP_PRODUCER_STOP_FAILED
                        ),
                        // The shutdown was still delivered; only the join was
                        // cut short. Logged as a distinct outcome so a slow
                        // agent teardown is not mistaken for a failed one.
                        Err(_) => log::warn!(
                            "[desktop-exit] shutdown_phase=stop_acp_producers_on_loop_exit stable_code={} result=SIGNALLED_NOT_JOINED",
                            crate::web::ACP_PRODUCER_STOP_FAILED
                        ),
                    }
                }
                if let Some(pty_manager) = pty_manager {
                    let receipt = pty_manager
                        .kill_all_until(started + LAST_RESORT_PTY_CLEANUP_DEADLINE)
                        .await;
                    log::info!(
                        "[desktop-exit] shutdown_phase=cleanup_ptys_on_loop_exit stable_code={} result={} attempted={} succeeded={} failed={} in_flight={} elapsed_ms={}",
                        if receipt.clean_success() { "OK" } else { crate::web::PTY_CLEANUP_FAILED },
                        if receipt.clean_success() { "PASS" } else { "FAILED" },
                        receipt.attempted,
                        receipt.succeeded,
                        receipt.failed,
                        receipt.in_flight,
                        receipt.elapsed_ms
                    );
                }
            });
            return;
        }
        if let RunEvent::ExitRequested { api, .. } = event {
            if CLEANUP_DONE.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            if CLEANUP_IN_PROGRESS
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_err()
            {
                return;
            }
            crate::host_admission::HostAdmission::global().close();

            let browser_tab_manager = app_handle
                .try_state::<Arc<browser_tab_manager::BrowserTabManager>>()
                .map(|state| state.inner().clone());
            let ssh_manager = app_handle
                .try_state::<Arc<ssh::SSHManager>>()
                .map(|state| state.inner().clone());
            let remote_state = app_handle
                .try_state::<Arc<RemoteServerState>>()
                .map(|state| state.inner().clone());
            let acp_manager = app_handle
                .try_state::<Arc<AcpManager>>()
                .map(|state| state.inner().clone());
            let ws_relay = app_handle
                .try_state::<Arc<WsRelaySink>>()
                .map(|state| state.inner().clone());
            let scheduled_tasks = app_handle
                .try_state::<Arc<crate::scheduled_tasks::ScheduledTaskService>>()
                .map(|state| state.inner().clone());
            let pty_manager = app_handle
                .try_state::<Arc<PtyManager>>()
                .map(|state| state.inner().clone());
            let app_handle_clone = app_handle.clone();

            // The run callback may execute outside a Tokio reactor; Tauri owns this runtime.
            tauri::async_runtime::spawn(async move {
                let deadline = tokio::time::Instant::now()
                    + crate::conversation::DEFAULT_DRAIN_TIMEOUT;
                crate::host_admission::HostAdmission::global()
                    .drain_until(deadline)
                    .await;
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let connection_receipt =
                    crate::web::upgraded_connections::UpgradedConnectionRegistry::global()
                        .join_all(remaining)
                        .await;
                log::info!(
                    "[desktop-exit] shutdown_phase=join_upgraded_connections stable_code=OK active={} failed={} timed_out={}",
                    connection_receipt.active,
                    connection_receipt.failed,
                    connection_receipt.timed_out
                );

                // Close remote ingress before producer stop. Shared-live remains non-owning and
                // its stop path never drains Desktop-global Conversation persistence.
                if let Some(remote_state) = remote_state {
                    match tokio::time::timeout_at(deadline, remote_state.shutdown_keep_credential())
                        .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) | Err(_) => log::error!(
                            "[desktop-exit] shutdown_phase=stop_remote stable_code=REMOTE_STOP_TIMEOUT result=FAILED"
                        ),
                    }
                }

                if let Some(scheduled_tasks) = scheduled_tasks {
                    scheduled_tasks
                        .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now()))
                        .await;
                }

                let mut durability = stop_desktop_producers_and_drain(
                    acp_manager.as_deref(),
                    ws_relay.as_deref(),
                    deadline,
                )
                .await;

                if let Some(ssh_manager) = ssh_manager {
                    if tokio::time::timeout_at(deadline, ssh_manager.shutdown())
                        .await
                        .is_err()
                    {
                        log::error!(
                            "[desktop-exit] shutdown_phase=shutdown_ssh stable_code=REMOTE_STAGE_TIMEOUT result=FAILED"
                        );
                    }
                }
                if let Some(pty_manager) = pty_manager {
                    let receipt = pty_manager.kill_all_until(deadline).await;
                    log::info!(
                        "[desktop-exit] shutdown_phase=cleanup_ptys stable_code={} result={} attempted={} succeeded={} failed={} in_flight={} elapsed_ms={}",
                        if receipt.clean_success() {
                            "OK"
                        } else {
                            crate::web::PTY_CLEANUP_FAILED
                        },
                        if receipt.clean_success() { "PASS" } else { "FAILED" },
                        receipt.attempted,
                        receipt.succeeded,
                        receipt.failed,
                        receipt.in_flight,
                        receipt.elapsed_ms
                    );
                    if !receipt.clean_success() {
                        durability.failures.push(crate::web::PTY_CLEANUP_FAILED);
                    }
                    durability.pty_shutdown = Some(receipt);
                } else {
                    log::error!(
                        "[desktop-exit] shutdown_phase=cleanup_ptys stable_code={} result=FAILED attempted=0 succeeded=0 failed=0 in_flight=0 elapsed_ms=0",
                        crate::web::PTY_CLEANUP_FAILED
                    );
                    durability.failures.push(crate::web::PTY_CLEANUP_FAILED);
                }
                let mut clean_exit = durability.clean_success();

                if let Some(acp_manager) = acp_manager {
                    match tokio::time::timeout_at(deadline, acp_manager.shutdown_persistence()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) | Err(_) => {
                            log::error!(
                                "[desktop-exit] shutdown_phase=shutdown_acp_persistence stable_code={} result=FAILED",
                                crate::web::ACP_PERSISTENCE_SHUTDOWN_FAILED
                            );
                            clean_exit = false;
                        }
                    }
                }
                if let Some(browser_tab_manager) = browser_tab_manager {
                    browser_tab_manager.destroy_all();
                }
                clear_desktop_remote_generation();

                CLEANUP_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
                let exit_code = if clean_exit { 0 } else { 1 };
                log::info!(
                    "[desktop-exit] shutdown_phase=complete stable_code={} result={} exit_code={}",
                    if clean_exit { "OK" } else { "DESKTOP_EXIT_DEGRADED" },
                    if clean_exit { "PASS" } else { "FAILED" },
                    exit_code
                );
                app_handle_clone.exit(exit_code);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_remote_bootstrap_uses_memory_authority_until_start() {
        let source = include_str!("lib.rs");
        let helper_start = source
            .find("fn provision_desktop_remote_authority()")
            .expect("desktop authority helper");
        let helper_end = source[helper_start..]
            .find("fn clear_desktop_remote_generation()")
            .map(|offset| helper_start + offset)
            .expect("desktop authority helper boundary");
        let helper = &source[helper_start..helper_end];
        assert!(helper.contains("desktop_memory()"));
        assert!(!helper.contains("issue_or_load_desktop"));
        assert!(!helper.contains("keyring"));
    }

    /// `RunEvent::ExitRequested` is emitted only when the last window is
    /// Destroyed or `AppHandle::exit()` is called. A macOS Cmd+Q reaches neither:
    /// `applicationWillTerminate` -> `AppState::exit()` -> `Event::LoopDestroyed`
    /// -> `RunEvent::Exit`. Handling `ExitRequested` alone therefore orphaned
    /// every PTY on a normal quit, so the `Exit` arm is the load-bearing one.
    #[test]
    fn loop_exit_reaps_ptys_since_platform_quit_skips_exit_requested() {
        let source = include_str!("lib.rs");
        let run_start = source
            .find("app.run(|app_handle, event| {")
            .expect("run event closure");
        let run = &source[run_start..];
        let exit_arm = run
            .find("matches!(event, RunEvent::Exit)")
            .expect("RunEvent::Exit arm");
        let requested_arm = run
            .find("if let RunEvent::ExitRequested")
            .expect("RunEvent::ExitRequested arm");
        assert!(
            exit_arm < requested_arm,
            "the Exit arm must be reached on a platform-driven quit"
        );
        let arm = &run[exit_arm..requested_arm];
        assert!(
            arm.contains("kill_all_until"),
            "the Exit arm must actually reap the PTYs"
        );
        assert!(
            arm.contains("LAST_RESORT_PTY_CLEANUP_DEADLINE"),
            "the reap must be bounded — the platform force-kills a slow terminate callback"
        );
        assert!(
            arm.contains("CLEANUP_DONE"),
            "a completed graceful shutdown must short-circuit the reap"
        );
    }

    /// The same platform quit that orphaned every PTY also orphaned every ACP
    /// adapter: the `Exit` arm reaped only the PTYs. An adapter that outlives
    /// the app keeps its own children alive — including the injected plan MCP
    /// server, which waits on a stdin EOF the orphaned adapter never sends.
    #[test]
    fn loop_exit_also_reaps_acp_agents_not_just_ptys() {
        let source = include_str!("lib.rs");
        let run_start = source
            .find("app.run(|app_handle, event| {")
            .expect("run event closure");
        let run = &source[run_start..];
        let exit_arm = run
            .find("matches!(event, RunEvent::Exit)")
            .expect("RunEvent::Exit arm");
        let requested_arm = run
            .find("if let RunEvent::ExitRequested")
            .expect("RunEvent::ExitRequested arm");
        let arm = &run[exit_arm..requested_arm];
        assert!(
            arm.contains("stop_producers"),
            "the Exit arm must wind the ACP agents down, not only the PTYs"
        );
        assert!(
            arm.contains("LAST_RESORT_ACP_REAP_DEADLINE"),
            "the ACP stage must be bounded — it shares the terminate callback's budget"
        );
        let acp_stage = arm.find("stop_producers").expect("acp stage");
        let pty_stage = arm.find("kill_all_until").expect("pty stage");
        assert!(
            acp_stage < pty_stage,
            "shutdown must be signalled to the agents before the PTY reap, so they wind down in parallel with it"
        );
        assert!(
            !arm.contains("else {\n                return;\n            };"),
            "neither manager may gate the other's stage with an early return"
        );
    }

    #[test]
    fn desktop_history_page_command_is_registered() {
        let source = include_str!("lib.rs");
        let handler_start = source
            .find(".invoke_handler(tauri::generate_handler![")
            .expect("production invoke handler start");
        let handler_tail = &source[handler_start..];
        let handler_end = handler_tail
            .find("])\n        .build")
            .expect("production invoke handler end");
        let handler = &handler_tail[..handler_end];
        assert_eq!(
            handler.matches("commands::acp_history_get_page,").count(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn desktop_catalog_flush_failed_blocks_clean_exit_under_host_deadline_and_later_mutation_responsive(
    ) {
        use crate::conversation::{
            AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
            ConversationEventType, ConversationLifecycleState, ConversationMutation,
            ConversationRecordV2, ConversationWriter, CreationPartition, ExecutionTarget,
            AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
        };
        use crate::web::sink::{AcpEvent, EventSink};
        use chrono::Utc;
        use serde_json::json;
        use std::time::Duration;
        use uuid::Uuid;

        let temp = tempfile::tempdir().unwrap();
        let bootstrap = crate::conversation::ConversationBootstrap::run(
            crate::conversation::HostConversationRoots::desktop(
                temp.path().join("state"),
                temp.path().join("visible"),
            ),
            crate::conversation::MigrationHostMode::Desktop,
        )
        .unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let conversation_id = crate::conversation::ConversationId::new_v4();
        let created_at = Utc::now();
        bootstrap
            .writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: workspace.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::InitializingAgent,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        bootstrap
            .writer
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "desktop-catalog-failure".to_string(),
                    runtime_agent_id: "runtime-desktop-exit".to_string(),
                    stable_agent_namespace: "config:desktop-exit".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();

        let relay = Arc::new(WsRelaySink::with_conversation_persistence(
            32,
            Arc::clone(&bootstrap.persistence_adapter),
            None,
        ));
        assert!(relay
            .ordered_conversation_persistence()
            .unwrap()
            .shares_authority(&bootstrap.ordered_persistence));
        bootstrap.repository.reset_catalog_write_counters();
        bootstrap.repository.fail_next_catalog_writes(usize::MAX);
        relay
            .emit(&AcpEvent {
                sid: Some("desktop-catalog-failure".to_string()),
                type_: "acp:message_chunk",
                payload: json!({"ordinal": 1}),
            })
            .unwrap();
        let pending_generation = bootstrap.repository.catalog_pending_generation();
        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![relay_sink]));

        let outcome = stop_desktop_producers_and_drain(
            Some(&acp),
            Some(&relay),
            tokio::time::Instant::now() + Duration::from_millis(50),
        )
        .await;
        assert!(!outcome.clean_success());
        assert_eq!(outcome.conversation_drain_attempts, 1);
        assert_eq!(outcome.catalog_flush_attempts, 1);
        assert!(outcome
            .failures
            .contains(&crate::web::CONVERSATION_CATALOG_FLUSH_FAILED));
        assert_eq!(
            bootstrap.repository.catalog_pending_generation(),
            pending_generation,
            "failed final generation remains retryable"
        );

        let mutation_started = std::time::Instant::now();
        ConversationWriter::append_event(
            &bootstrap.writer,
            conversation_id,
            Utc::now(),
            ConversationEventType::MessageChunk,
            json!({"ordinal": 2}),
            ConversationMutation::AcpEventAppend,
        )
        .await
        .expect("later canonical mutation remains responsive");
        assert!(mutation_started.elapsed() < Duration::from_secs(1));
        assert!(bootstrap.repository.catalog_pending_generation() > pending_generation);
    }

    #[test]
    fn native_ui_labels_cover_supported_languages() {
        for key in NATIVE_APP_MENU_LABEL_KEYS {
            let english = native_label_for(NativeUiLanguage::En, key);
            let simplified_chinese = native_label_for(NativeUiLanguage::ZhCn, key);
            assert_ne!(english, *key, "missing English native menu label for {key}");
            assert_ne!(
                simplified_chinese, *key,
                "missing Simplified Chinese native menu label for {key}"
            );
            assert!(
                !english.is_empty(),
                "empty English native menu label for {key}"
            );
            assert!(
                !simplified_chinese.is_empty(),
                "empty Simplified Chinese native menu label for {key}"
            );
        }

        assert_eq!(native_label_for(NativeUiLanguage::En, "menu.copy"), "Copy");
        assert_eq!(
            native_label_for(NativeUiLanguage::ZhCn, "menu.copy"),
            "复制"
        );
        assert_eq!(
            native_label_for(NativeUiLanguage::ZhCn, "log.copySuccess"),
            "日志内容已复制到剪贴板。"
        );
    }

    /// Select All is a custom menu item, so the accelerator only works if the
    /// renderer listens for the exact topic emitted here. Nothing else fails
    /// when the two drift: the menu builds, the click is handled, the emit
    /// succeeds, and Cmd+A silently does nothing.
    #[test]
    fn select_all_menu_topic_matches_the_renderer_listener() {
        let hook_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/renderer/hooks/use-menu-select-all.ts"
        );
        let hook = std::fs::read_to_string(hook_path)
            .expect("renderer select-all hook should be readable from the repo checkout");

        assert!(
            hook.contains(&format!("'{MENU_EVENT_SELECT_ALL}'")),
            "{hook_path} does not listen for '{MENU_EVENT_SELECT_ALL}' — \
             the native Select All item would emit into the void"
        );
    }

    #[test]
    fn app_menu_suspension_defers_language_rebuilds_until_restore() {
        let mut state = AppMenuSuspensionState::default();
        assert!(!state.is_suspended());

        state.suspend();
        assert!(state.is_suspended());

        state.restore();
        assert!(!state.is_suspended());
    }

    #[test]
    fn native_ui_labels_preserve_unknown_keys() {
        assert_eq!(
            native_label_for(NativeUiLanguage::ZhCn, "unknown.key"),
            "unknown.key"
        );
    }

    #[cfg(target_os = "windows")]
    fn with_test_comspec<T>(f: impl FnOnce() -> T) -> T {
        use std::ffi::OsString;

        struct ComspecGuard(Option<OsString>);

        impl Drop for ComspecGuard {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    std::env::set_var("COMSPEC", value);
                } else {
                    std::env::remove_var("COMSPEC");
                }
            }
        }

        let _guard = ComspecGuard(std::env::var_os("COMSPEC"));
        std::env::set_var("COMSPEC", r"C:\Windows\System32\cmd.exe");
        f()
    }

    #[test]
    fn test_fallback_shell() {
        #[cfg(target_os = "windows")]
        let shell = with_test_comspec(|| get_default_shell_info().unwrap());
        #[cfg(not(target_os = "windows"))]
        let shell = get_default_shell_info().unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(shell.name, "cmd");
        #[cfg(not(target_os = "windows"))]
        assert!(shell.name == "sh" || shell.name == "bash" || shell.name == "zsh");
    }

    #[test]
    fn test_get_default_shell_returns_some() {
        let shell = get_default_shell_info();
        assert!(shell.is_some());
    }

    #[test]
    fn test_get_available_shells_not_empty() {
        let shells = get_available_shells();
        assert!(!shells.is_empty());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_available_shells_prefer_login_shell_path() {
        let Ok(login) = env::var("SHELL") else {
            return;
        };
        let Some(name) = Path::new(&login).file_name().and_then(|s| s.to_str()) else {
            return;
        };
        if !Path::new(&login).exists() {
            return;
        }
        let shells = get_available_shells();
        let listed = shells.iter().find(|shell| shell.name == name);
        assert_eq!(
            listed.map(|shell| shell.path.as_str()),
            Some(login.as_str())
        );
    }

    #[test]
    fn test_get_home_directory_command() {
        let result = get_home_directory();
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_builtin_windows_shell() {
        assert!(is_builtin_windows_shell("cmd"));
        assert!(is_builtin_windows_shell("CMD.EXE"));
        assert!(is_builtin_windows_shell("powershell"));
        assert!(is_builtin_windows_shell("pwsh"));
        assert!(is_builtin_windows_shell("wsl"));
        assert!(!is_builtin_windows_shell("bash.exe"));
        assert!(!is_builtin_windows_shell("git-bash"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_resolve_executable_from_path_nonexistent() {
        let result = resolve_executable_from_path("definitely-not-a-real-shell-xyz");
        assert!(result.is_none());
    }

    // ========== Git Bash candidate sync tests ==========

    #[cfg(target_os = "windows")]
    #[test]
    fn test_git_bash_primary_candidates_defined() {
        // Verify primary Git Bash candidates are defined (compile-time guard)
        const { assert!(!git_bash_paths::PRIMARY_PATHS.is_empty()) };

        // Verify specific well-known paths exist
        assert!(git_bash_paths::PRIMARY_PATHS
            .iter()
            .any(|p| p.contains("Program Files") && p.contains("Git\\bin")));
        assert!(git_bash_paths::PRIMARY_PATHS
            .iter()
            .any(|p| p.contains("Git\\usr\\bin")));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_git_bash_fallback_candidates_defined() {
        // Verify fallback Git Bash candidates are defined (compile-time guard)
        const { assert!(!git_bash_paths::FALLBACK_PATHS.is_empty()) };

        // All fallback paths should contain bash.exe
        for path in git_bash_paths::FALLBACK_PATHS {
            assert!(
                path.contains("bash.exe"),
                "Fallback path should contain bash.exe: {}",
                path
            );
        }
    }

    #[test]
    fn test_git_bash_shell_display_name() {
        let display_name = shell_display_name("git-bash");
        assert_eq!(display_name, "Git Bash");
    }
}
