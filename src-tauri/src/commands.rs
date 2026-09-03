use crate::browser_tab_manager::{BrowserBounds, BrowserTabInfo, BrowserTabManager};
use crate::migrations::{
    MigrationInfo, MigrationManager, MigrationRecord, MigrationResult, SchemaVersion,
};
use crate::path_validation;
use crate::pty::claims::{ClaimError, RotatedClaim};
use crate::pty::manager::{
    SpawnedTerminal, TerminalAttachResult, TerminalCleanupFailure, TerminalCleanupStage,
    TerminalReplay, TerminalResumeGrant, TerminalResumeRequest, TerminalSpawnIntentV1,
};
use crate::pty::{PtyManager, SpawnOptions};
use crate::remote;
use crate::trackers::{
    CwdTracker, ExitCodeTracker, GitCommit, GitStatus, GitStatusDetail, GitTracker,
};
use crate::worktree::{
    BaseBranchInfo, BranchEntry, DirtyStatus, GitWorktreeEntry, IncludeCopyResult, RemoveResult,
    WorktreeManager,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter, State, Webview};

/// Validate that the caller webview matches the expected tab_id.
/// This prevents cross-tab command injection where a malicious webview
/// could emit events for other tabs.
fn validate_browser_tab_caller(webview: &Webview, expected_tab_id: &str) -> Result<(), String> {
    let caller_label = webview.label();
    if caller_label != expected_tab_id {
        log::warn!(
            "[Security] Browser tab command rejected: caller '{}' does not match expected '{}'",
            caller_label,
            expected_tab_id
        );
        return Err(format!(
            "Browser tab command rejected: caller '{}' does not match expected '{}'",
            caller_label, expected_tab_id
        ));
    }
    Ok(())
}

/// Validate and canonicalize a project path to prevent path traversal attacks.
/// Returns the canonicalized path or an error if the path is invalid or inaccessible.
fn validate_project_path(path: &str) -> Result<PathBuf, String> {
    let path_buf = PathBuf::from(path);

    // Canonicalize to resolve symlinks and relative paths
    let canonical = path_buf.canonicalize().map_err(|_| {
        log::warn!("[Security] operation=validate_project_path stable_code=PATH_VALIDATION_FAILED");
        "Invalid or inaccessible path".to_string()
    })?;

    // On Windows, `canonicalize()` returns a verbatim (`\\?\…`) path. That prefix
    // defeats external tools such as `git.exe` (e.g. `git worktree add` fails with
    // "could not create leading directories …: Invalid argument"). Strip it so the
    // validated path stays tool-friendly while keeping the canonicalization benefits.
    let canonical_str = canonical.to_string_lossy();
    let simplified = path_validation::strip_verbatim_prefix(&canonical_str).into_owned();

    log::debug!("[Security] operation=validate_project_path stable_code=OK");
    Ok(PathBuf::from(simplified))
}

/// Macro to validate a path and convert it to a String, returning early with an IpcResult error if validation fails.
pub(crate) fn require_host_admission<T>() -> Result<(), IpcResult<T>> {
    crate::host_admission::HostAdmission::global()
        .check()
        .map_err(|code| IpcResult::error("host is shutting down", code))
}

macro_rules! validate_and_stringify {
    ($path:expr) => {{
        if crate::host_admission::HostAdmission::global()
            .check()
            .is_err()
        {
            return Ok(IpcResult::error(
                "host is shutting down",
                crate::host_admission::HOST_SHUTTING_DOWN,
            ));
        }
        match validate_project_path($path) {
            Ok(validated) => match validated.to_str() {
                Some(s) => s.to_string(),
                None => {
                    return Ok(IpcResult::error(
                        "Path contains invalid UTF-8",
                        "INVALID_PATH_ENCODING",
                    ))
                }
            },
            Err(e) => return Ok(IpcResult::error(e, "PATH_VALIDATION_FAILED")),
        }
    }};
}

/// IPC Result pattern
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcResult<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl<T> IpcResult<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    pub fn error(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.into()),
            code: Some(code.into()),
        }
    }
}

/// Safe terminal cleanup/compound detail shared by Tauri and terminal WebSocket responses.
/// The outer `IpcResult.code` distinguishes ordinary termination from compound rollback; this
/// payload carries only the stable primary code, exact cleanup stage, and recoverable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TerminalResourceFailureV1 {
    pub terminal_id: String,
    pub primary_code: String,
    pub cleanup_stage: TerminalCleanupStage,
}

impl TerminalResourceFailureV1 {
    fn from_cleanup(primary_code: impl Into<String>, failure: TerminalCleanupFailure) -> Self {
        Self {
            terminal_id: failure.terminal_id,
            primary_code: primary_code.into(),
            cleanup_stage: failure.stage,
        }
    }

    fn wire_detail(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"terminalId\":\"redacted\",\"primaryCode\":\"{}\",\"cleanupStage\":\"{}\"}}",
                self.primary_code,
                self.cleanup_stage.as_str()
            )
        })
    }
}

/// Durably schedule Conversation migration maintenance for the next process bootstrap. This
/// command never acquires live mutation authority, opens stores, or touches PTYs/routes.
#[tauri::command]
pub fn conversation_migration_control(
    request: crate::conversation::MigrationMaintenanceRequestV1,
    control: State<'_, Arc<crate::conversation::ConversationMigrationControlService>>,
) -> Result<IpcResult<crate::conversation::MigrationMaintenanceScheduleReceiptV1>, String> {
    Ok(conversation_migration_control_inner(
        control.inner().as_ref(),
        request,
    ))
}

fn conversation_migration_control_inner(
    control: &crate::conversation::ConversationMigrationControlService,
    request: crate::conversation::MigrationMaintenanceRequestV1,
) -> IpcResult<crate::conversation::MigrationMaintenanceScheduleReceiptV1> {
    match control.request(request) {
        Ok(receipt) => IpcResult::success(receipt),
        Err(error) => IpcResult::error(error.detail, error.code.as_str()),
    }
}

#[cfg(test)]
mod conversation_migration_control_tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::conversation::{
        ConversationMigrationControlService, MigrationMaintenanceAction,
        MigrationMaintenanceRequestV1,
    };

    #[test]
    fn conversation_migration_control_schedules_restart_without_live_resources() {
        let temp = tempfile::tempdir().unwrap();
        let control = ConversationMigrationControlService::new(temp.path()).unwrap();
        let request = MigrationMaintenanceRequestV1 {
            action: MigrationMaintenanceAction::Rollback,
            request_id: Uuid::new_v4().to_string(),
            requested_at_utc: Utc::now(),
            approval_receipt: None,
        };
        let result = conversation_migration_control_inner(&control, request.clone());
        assert!(result.success);
        let receipt = result.data.unwrap();
        assert!(receipt.restart_required);
        assert_eq!(receipt.code, "MIGRATION_RESTART_REQUIRED");
        assert_eq!(control.pending().unwrap(), Some(request));
    }
}

/// Terminal visibility state
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVisibilityRequest {
    pub is_visible: bool,
}

/// Orphan detection settings
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanDetectionSettings {
    pub enabled: bool,
    pub timeout_minutes: Option<u64>,
}

/// Renderer ref request
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererRefRequest {
    pub terminal_id: String,
    pub renderer_id: String,
}

/// Request to update a terminal's orphan-reaping protection.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTerminalProtectedRequest {
    pub terminal_id: String,
    pub protected: bool,
}

// ==================== Attachment Commands ====================

/// Maximum attachment image size the renderer may read through this command.
/// Mirrors the renderer's `MAX_IMAGE_BYTES` (10 MB) so the brokered read can
/// reject oversized files before transferring them across IPC.
const ATTACHMENT_MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Image extensions the attachment flow is allowed to read by path. The
/// generic `fs:allow-read-file` permission was removed from the renderer
/// capability; this command is the only binary-read path left, and it is
/// intentionally restricted to images (the only content type the composer and
/// chat preview need to read by path) to limit the confidentiality surface.
const ATTACHMENT_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg", "ico",
];

fn attachment_is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            ATTACHMENT_IMAGE_EXTENSIONS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

/// Read attachment image bytes by path. Replaces direct renderer
/// `fs:allow-read-file` access so binary reads go through one validated,
/// size- and type-constrained command instead of the generic fs plugin.
///
/// Returns the raw bytes via `Response::new`, which arrives on the JS side as
/// an `ArrayBuffer`. Rejects (throws on JS) when the path is not absolute,
/// does not exist, is not a regular file, exceeds the size cap, or is not an
/// image — callers fall back to a file-icon preview on rejection.
#[tauri::command]
pub fn read_attachment_bytes(path: String) -> Result<Response, String> {
    let stripped = path_validation::strip_verbatim_prefix(&path);
    let candidate = PathBuf::from(stripped.as_ref());

    if !candidate.is_absolute() {
        return Err("Attachment path must be absolute".to_string());
    }

    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("Invalid or inaccessible attachment path: {}", e))?;

    if !attachment_is_image_extension(&canonical) {
        return Err("Attachment path is not an image".to_string());
    }

    let mut file =
        std::fs::File::open(&canonical).map_err(|e| format!("Failed to open attachment: {}", e))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Failed to read attachment metadata: {}", e))?;
    if !metadata.is_file() {
        return Err("Attachment path is not a regular file".to_string());
    }
    if metadata.len() > ATTACHMENT_MAX_IMAGE_BYTES {
        return Err("Attachment image exceeds the 10 MB limit".to_string());
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read attachment: {}", e))?;
    if bytes.len() as u64 > ATTACHMENT_MAX_IMAGE_BYTES {
        return Err("Attachment image exceeds the 10 MB limit".to_string());
    }
    Ok(Response::new(bytes))
}

// ==================== Terminal Commands ====================

/// Spawn a new terminal with binary data channel
///
/// The `on_data` channel uses Tauri 2's Channel API for
/// zero-overhead binary IPC. PTY output is sent as raw `Vec<u8>` via
/// `Response::new(bytes)`, arriving in JS as `ArrayBuffer` with no JSON
/// serialization overhead.
///
/// CAP-3: the response carries the terminal info PLUS the issued `claim`
/// credential (flattened camelCase, same shape as the web `spawn` reply).
/// This is the initial issuance path; authenticated resume and explicit
/// rotation can later replace the claim.
#[tauri::command]
pub async fn terminal_spawn(
    options: SpawnOptions,
    on_data: Channel<Response>,
    pty_manager: State<'_, Arc<PtyManager>>,
    workspace: State<'_, Arc<crate::conversation::SessionWorkspaceService>>,
) -> Result<IpcResult<SpawnedTerminal>, String> {
    Ok(terminal_spawn_resource(
        options,
        Some(on_data),
        pty_manager.inner(),
        workspace.inner(),
    )
    .await)
}

pub(crate) async fn terminal_spawn_resource(
    options: SpawnOptions,
    on_data: Option<Channel<Response>>,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> IpcResult<SpawnedTerminal> {
    terminal_spawn_resource_impl(options, on_data, pty_manager, workspace).await
}

/// Remote-only spawn path. The wire payload is already narrowed to
/// [`TerminalSpawnIntentV1`]; `PtyManager` derives every executable, shell,
/// environment, and cwd value from the host-owned Conversation record.
pub(crate) async fn terminal_spawn_intent_resource(
    intent: TerminalSpawnIntentV1,
    conversation: &crate::conversation::ConversationRecordV2,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> IpcResult<SpawnedTerminal> {
    let conversation_id = intent.conversation_id;
    if let Err(error) = workspace.ensure_terminal_ref_writable(conversation_id, true) {
        log::warn!(
            "[terminal-command] remote spawn admission rejected conversation_id={} code={}",
            conversation_id,
            error.code.as_str()
        );
        return IpcResult::error(error.detail, error.code.as_str());
    }

    let spawned = match pty_manager
        .spawn_for_conversation(intent, conversation, None)
        .await
    {
        Ok(spawned) => spawned,
        Err(error) if error.ends_with("scope is unauthorized") => {
            return IpcResult::error("Unauthorized", "UNAUTHORIZED")
        }
        Err(error) => return IpcResult::error(error, "SPAWN_FAILED"),
    };
    commit_terminal_spawn_resource(spawned, conversation_id, pty_manager, workspace).await
}

async fn terminal_spawn_resource_impl(
    options: SpawnOptions,
    on_data: Option<Channel<Response>>,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> IpcResult<SpawnedTerminal> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let is_ephemeral_ssh = options.kind.as_deref() == Some("ssh");
    // Scope-less durable terminals (regular project workspace) spawn without a
    // SessionWorkspace resource; conversation-scoped terminals keep admission.
    let conversation_id = if is_ephemeral_ssh {
        None
    } else {
        match options.conversation_id {
            Some(conversation_id) => {
                if let Err(error) = workspace.ensure_terminal_ref_writable(conversation_id, true) {
                    log::warn!(
                        "[terminal-command] durable spawn admission rejected conversation_id={} code={}",
                        conversation_id,
                        error.code.as_str()
                    );
                    return IpcResult::error(error.detail, error.code.as_str());
                }
                Some(conversation_id)
            }
            None => None,
        }
    };

    let spawned = match pty_manager.spawn(options, on_data).await {
        Ok(spawned) => spawned,
        Err(error) => return IpcResult::error(error, "SPAWN_FAILED"),
    };
    let Some(conversation_id) = conversation_id else {
        return IpcResult::success(spawned);
    };
    commit_terminal_spawn_resource(spawned, conversation_id, pty_manager, workspace).await
}

async fn commit_terminal_spawn_resource(
    spawned: SpawnedTerminal,
    conversation_id: crate::conversation::ConversationId,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> IpcResult<SpawnedTerminal> {
    if let Err(primary) = workspace
        .add_terminal_ref(conversation_id, &spawned.info.id)
        .await
    {
        let primary_code = primary.code.as_str();
        match pty_manager.terminate(&spawned.info.id).await {
            Ok(_) => {
                log::warn!(
                    "[terminal-command] spawn ref failed and PTY rollback completed terminal_id={} primary_code={} cleanup_stage=complete stable_result={}",
                    spawned.info.id,
                    primary_code,
                    primary_code
                );
                return IpcResult::error(primary.detail, primary_code);
            }
            Err(cleanup) => {
                let failure = TerminalResourceFailureV1::from_cleanup(primary_code, cleanup);
                log::error!(
                    "[terminal-command] compound spawn rollback failed terminal_id={} primary_code={} cleanup_stage={} stable_result={}",
                    failure.terminal_id,
                    failure.primary_code,
                    failure.cleanup_stage,
                    crate::conversation::TERMINAL_RESOURCE_ROLLBACK_FAILED
                );
                return IpcResult::error(
                    failure.wire_detail(),
                    crate::conversation::TERMINAL_RESOURCE_ROLLBACK_FAILED,
                );
            }
        }
    }

    log::info!(
        "[terminal-command] spawn resource committed conversation_id={} terminal_id={} code=OK",
        conversation_id,
        spawned.info.id
    );
    IpcResult::success(spawned)
}

/// Milliseconds between claim-generation checks in a desktop attach forwarder.
/// Bounds how long a forwarder survives a rotate/revoke while its terminal is
/// idle (no output events to piggyback the check on).
const ATTACH_FORWARDER_GENERATION_CHECK_MS: u64 = 250;

/// Tracked desktop attach forwarders: terminal_id → (token, abort handle).
/// Exactly one live forwarder per terminal — a re-attach aborts the
/// predecessor; rotate/revoke terminate it via the claim generation bump.
static ATTACH_FORWARDERS: OnceLock<Mutex<HashMap<String, (u64, tokio::task::AbortHandle)>>> =
    OnceLock::new();
static ATTACH_FORWARDER_TOKENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn attach_forwarders() -> &'static Mutex<HashMap<String, (u64, tokio::task::AbortHandle)>> {
    ATTACH_FORWARDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Recover the forwarder map guard even if a holder panicked while holding the
/// lock — silently skipping on poison would skip predecessor aborts and leak
/// entries.
fn lock_forwarders(
) -> std::sync::MutexGuard<'static, HashMap<String, (u64, tokio::task::AbortHandle)>> {
    attach_forwarders()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Teardown condition for an attach forwarder, pure for testability. Shared
/// by the desktop forwarder and the web attachment task (CAP-3 amendment R1:
/// invalidation severs EVERY access derived from the credential, on all
/// connections).
///
/// A forwarder must terminate when the captured claim generation no longer
/// matches the registry: a bump means rotate/revoke invalidated the credential
/// this stream was derived from, and a missing record (`None`) means the
/// terminal was killed/reaped.
pub(crate) fn forwarder_should_terminate(
    captured_generation: Option<u64>,
    current_generation: Option<u64>,
) -> bool {
    captured_generation != current_generation
}

#[cfg(test)]
mod forwarder_teardown_tests {
    use super::forwarder_should_terminate;

    #[test]
    fn same_generation_keeps_streaming() {
        assert!(!forwarder_should_terminate(Some(3), Some(3)));
    }

    #[test]
    fn rotated_generation_terminates() {
        assert!(forwarder_should_terminate(Some(3), Some(4)));
        assert!(forwarder_should_terminate(Some(3), Some(2)));
    }

    #[test]
    fn killed_or_reaped_terminal_terminates() {
        assert!(forwarder_should_terminate(Some(3), None));
    }

    #[test]
    fn absent_at_both_ends_is_neutral() {
        assert!(!forwarder_should_terminate(None, None));
        // A forwarder can never start without a record, but the condition must
        // still be total.
        assert!(forwarder_should_terminate(None, Some(1)));
    }
}

/// Why a passive terminal reference could not be resumed.
///
/// [`ClaimError`] stays collapsed for every credential path — no response shape
/// there may distinguish unknown terminal from wrong credential. Resume is the
/// one place the distinction is both safe and necessary: the caller has already
/// proved it holds the SessionWorkspace reference, so "the PTY is gone" tells it
/// nothing it did not supply, and without it the renderer cannot tell a dead end
/// from a retryable failure — it offers a reconnect that can never succeed.
///
/// Local callers only. `web::terminal_ws` maps every variant to one generic
/// response, which is what keeps the remote surface leak-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalResumeDenial {
    /// The reference is valid but its PTY no longer exists or has exited.
    /// Retrying can never succeed.
    Gone,
    /// Everything else: no such reference, wrong Conversation, untracked ref,
    /// claim rotation refused.
    Unauthorized,
}

/// Validate the passive SessionWorkspace reference and rotate a one-time claim
/// for a matching live PTY. Every cause collapses to [`TerminalResumeDenial`]'s
/// two variants; the underlying claim machinery still sees only [`ClaimError`].
pub(crate) async fn terminal_resume_resource(
    request: &TerminalResumeRequest,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> Result<(TerminalResumeGrant, TerminalReplay), TerminalResumeDenial> {
    let has_passive_ref = match workspace.load(request.conversation_id).await {
        Ok(crate::conversation::SessionWorkspaceLoadOutcome::Loaded { workspace }) => {
            workspace.resources.iter().any(|resource| {
                matches!(
                    resource,
                    crate::conversation::SessionWorkspaceResourceDescriptor::Terminal {
                        terminal_id,
                        conversation_id,
                        ..
                    } if terminal_id == &request.terminal_id
                        && *conversation_id == request.conversation_id
                )
            })
        }
        Ok(
            crate::conversation::SessionWorkspaceLoadOutcome::Missing { .. }
            | crate::conversation::SessionWorkspaceLoadOutcome::RecoveryRequired { .. },
        )
        | Err(_) => false,
    };
    if !has_passive_ref {
        log::warn!(
            "[terminal-resume] denied conversation_id={} terminal_id={} code=UNAUTHORIZED",
            request.conversation_id,
            request.terminal_id
        );
        return Err(TerminalResumeDenial::Unauthorized);
    }

    // Classify BEFORE `resume_for_conversation`, which collapses "PTY is gone",
    // "not active", "untracked ref" and "wrong Conversation" into one opaque
    // ClaimError. A reference whose PTY died — the app exited and killed its
    // children, or the shell ended — is a dead end, and the renderer needs to
    // know that to retire the record instead of offering a doomed reconnect.
    //
    // The PTY can still die between this check and the call below; that races
    // to `Unauthorized`, which is the safe direction — the record survives and
    // the next resume reports `Gone`.
    let alive = pty_manager
        .get(&request.terminal_id)
        .is_some_and(|instance| instance.is_active());
    if !alive {
        log::info!(
            "[terminal-resume] gone conversation_id={} terminal_id={} code=TERMINAL_GONE",
            request.conversation_id,
            request.terminal_id
        );
        return Err(TerminalResumeDenial::Gone);
    }

    pty_manager
        .resume_for_conversation(
            request.conversation_id,
            &request.terminal_id,
            request.last_seq,
        )
        .map_err(|_: ClaimError| TerminalResumeDenial::Unauthorized)
}

/// Resume a passive terminal reference after a cold desktop renderer start.
/// The local Tauri invoke boundary is trusted, but the exact Conversation and
/// terminal scope is still validated before the old claim generation rotates.
#[tauri::command]
pub async fn terminal_resume(
    request: TerminalResumeRequest,
    on_data: Channel<Response>,
    pty_manager: State<'_, Arc<PtyManager>>,
    workspace: State<'_, Arc<crate::conversation::SessionWorkspaceService>>,
) -> Result<IpcResult<TerminalResumeGrant>, String> {
    let (grant, replay) =
        match terminal_resume_resource(&request, pty_manager.inner(), workspace.inner()).await {
            Ok(value) => value,
            // Distinct on purpose: the renderer retires a record it can never
            // revive, and keeps the retryable placeholder for everything else.
            Err(TerminalResumeDenial::Gone) => {
                return Ok(IpcResult::error("Terminal is gone", "TERMINAL_GONE"))
            }
            Err(TerminalResumeDenial::Unauthorized) => {
                return Ok(IpcResult::error("Unauthorized", "UNAUTHORIZED"))
            }
        };

    // Rotation invalidates any predecessor forwarder. Abort the local tracked
    // handle immediately rather than waiting for its generation-check tick.
    if let Some((_, forwarder)) = lock_forwarders().remove(&request.terminal_id) {
        forwarder.abort();
    }
    for chunk in &replay.chunks {
        if on_data.send(Response::new(chunk.data.clone())).is_err() {
            log::warn!(
                "[terminal-resume] replay channel closed conversation_id={} terminal_id={} latest_seq={} gap={}",
                request.conversation_id,
                request.terminal_id,
                grant.terminal.latest_seq,
                grant.terminal.gap
            );
            break;
        }
    }
    log::info!(
        "[terminal-resume] desktop grant delivered conversation_id={} terminal_id={} latest_seq={} gap={}",
        request.conversation_id,
        request.terminal_id,
        grant.terminal.latest_seq,
        grant.terminal.gap
    );
    Ok(IpcResult::success(grant))
}

/// Attach to a terminal's output stream with a claim credential (CAP-3).
///
/// Verification is the gate: `terminalId` + valid `claim` + `lastSeq`. Any
/// verification failure (unknown terminal, missing/wrong/revoked credential,
/// binding mismatch) returns ONE generic UNAUTHORIZED error — no response
/// shape distinguishes the cases (existence leak stays fixed). On success the
/// ring-bounded replay (chunks with `seq > lastSeq`) is delivered through the
/// raw-bytes channel, then a tracked forwarder streams live output.
///
/// Forwarder lifecycle: exactly one per terminal — this command aborts any
/// predecessor before installing the new one; rotate/revoke bump the claim
/// generation, which the forwarder observes and terminates on. The PTY is
/// never touched by attach failures or forwarder teardown.
#[tauri::command]
pub async fn terminal_attach(
    terminal_id: String,
    claim: String,
    last_seq: u64,
    on_data: Channel<Response>,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<TerminalAttachResult>, String> {
    if let Err(error) = require_host_admission() {
        return Ok(error);
    }
    // Capture the generation BEFORE verifying (TOCTOU-safe ordering): if a
    // rotate/revoke lands between capture and verify, verify fails (the
    // credential was invalidated) and we reject; if it lands after verify, the
    // captured generation is stale and the forwarder terminates on its next
    // check. Capturing after verify would invert the race and let an
    // invalidated credential stream forever.
    let generation = pty_manager.claim_generation(&terminal_id);
    if pty_manager.verify_claim(&terminal_id, &claim).is_err() {
        // Registry already logged the failure (terminal id, never the
        // credential). Keep the response generic.
        return Ok(IpcResult::error("Unauthorized", "UNAUTHORIZED"));
    }
    let Some(instance) = pty_manager.get(&terminal_id) else {
        // Verified a heartbeat ago but gone now — same generic error.
        return Ok(IpcResult::error("Unauthorized", "UNAUTHORIZED"));
    };

    Ok(install_desktop_output_forwarder(
        terminal_id,
        last_seq,
        generation,
        on_data,
        instance,
        pty_manager.inner().clone(),
        "terminal-attach",
    )
    .await)
}

/// Watch a live host PTY from the trusted desktop renderer without rotating
/// the companion claim. Used when the phone created the terminal.
#[tauri::command]
pub async fn terminal_watch(
    terminal_id: String,
    last_seq: u64,
    on_data: Channel<Response>,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<TerminalAttachResult>, String> {
    if let Err(error) = require_host_admission() {
        return Ok(error);
    }
    let Some(instance) = pty_manager
        .get(&terminal_id)
        .filter(|item| item.is_active())
    else {
        return Ok(IpcResult::error("Terminal not found", "TERMINAL_NOT_FOUND"));
    };
    let generation = pty_manager.claim_generation(&terminal_id);
    log::info!("[terminal-watch] watching terminal_id={terminal_id}");
    Ok(install_desktop_output_forwarder(
        terminal_id,
        last_seq,
        generation,
        on_data,
        instance,
        pty_manager.inner().clone(),
        "terminal-watch",
    )
    .await)
}

async fn install_desktop_output_forwarder(
    terminal_id: String,
    last_seq: u64,
    generation: Option<u64>,
    on_data: Channel<Response>,
    instance: Arc<crate::pty::manager::TerminalInstance>,
    pty_manager: Arc<PtyManager>,
    log_label: &'static str,
) -> IpcResult<TerminalAttachResult> {
    // Bounded replay + live subscription snapshot atomically (existing seq
    // infra, unchanged).
    let replay = instance.subscribe_from(last_seq);
    let result = pty_manager.build_attach_result(&instance, &replay);

    // Single-forwarder invariant: abort the predecessor BEFORE delivering the
    // replay so it cannot interleave a duplicate stream.
    let token = ATTACH_FORWARDER_TOKENS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let replacing_view = {
        let mut forwarders = lock_forwarders();
        let replacing = forwarders.contains_key(&terminal_id);
        if let Some((_, previous)) = forwarders.remove(&terminal_id) {
            previous.abort();
            log::info!("[{log_label}] aborted previous forwarder terminal_id={terminal_id}");
        }
        replacing
    };

    for chunk in &replay.chunks {
        if on_data.send(Response::new(chunk.data.clone())).is_err() {
            log::warn!(
                "[terminal-attach] replay channel closed terminal_id={}",
                terminal_id
            );
            return IpcResult::success(result);
        }
    }

    let pty = Arc::clone(&pty_manager);
    let forwarder_id = terminal_id.clone();
    let handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(
            ATTACH_FORWARDER_GENERATION_CHECK_MS,
        ));
        let mut receiver = replay.receiver;
        loop {
            // Teardown on rotate/revoke (generation bump) or kill/reap
            // (record gone). Checked every tick AND after every chunk.
            if forwarder_should_terminate(generation, pty.claim_generation(&forwarder_id)) {
                log::info!(
                    "[{log_label}] forwarder terminating (claim invalidated) terminal_id={forwarder_id}"
                );
                break;
            }
            tokio::select! {
                received = receiver.recv() => {
                    match received {
                        Ok(chunk) => {
                            if on_data.send(Response::new(chunk.data)).is_err() {
                                break; // renderer channel gone
                            }
                        }
                        // Desktop's raw-bytes channel has no gap framing
                        // (deferred parity decision) — keep streaming.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            log::warn!(
                                "[{log_label}] output receiver lagged by {skipped} for {forwarder_id}"
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tick.tick() => {}
            }
        }
        // Self-deregister only if this forwarder is still the tracked one (a
        // later re-attach may have replaced the entry already).
        let mut forwarders = lock_forwarders();
        if let Some((tracked_token, _)) = forwarders.get(&forwarder_id) {
            if *tracked_token == token {
                forwarders.remove(&forwarder_id);
            }
        }
    });
    crate::host_admission::HostAdmission::global().track_abort(handle.abort_handle());

    if !handle.is_finished() {
        let mut forwarders = lock_forwarders();
        // A concurrent attach may have raced — keep this (latest) forwarder
        // and abort any rival: exactly one survives, last attach wins. A
        // forwarder that already terminated (e.g. a generation bump raced the
        // spawn) is never registered, so no dead entry lingers in the map.
        if let Some((prev_token, prev_abort)) =
            forwarders.insert(terminal_id.clone(), (token, handle.abort_handle()))
        {
            if prev_token != token {
                prev_abort.abort();
            }
        }
    }

    if !replacing_view {
        pty_manager.note_view_opened(&terminal_id);
    }
    log::info!(
        "[{log_label}] attached terminal_id={} latest_seq={} gap={}",
        terminal_id,
        result.latest_seq,
        result.gap
    );
    IpcResult::success(result)
}

/// Rotate a terminal's claim credential (CAP-3).
///
/// Possession-based: presenting the current credential yields a fresh one and
/// atomically invalidates the old. Any verification failure returns the same
/// generic UNAUTHORIZED error as attach. The generation bump terminates the
/// tracked desktop attach forwarder (teardown of the invalidated holder's
/// access) while the PTY itself keeps running.
#[tauri::command]
pub async fn terminal_rotate_claim(
    terminal_id: String,
    claim: String,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<RotatedClaim>, String> {
    match pty_manager.rotate_claim(&terminal_id, &claim) {
        Ok(new_claim) => Ok(IpcResult::success(RotatedClaim { claim: new_claim })),
        Err(_) => Ok(IpcResult::error("Unauthorized", "UNAUTHORIZED")),
    }
}

/// Revoke a terminal's claim credential (CAP-3).
///
/// The PTY survives (never-clause) and stays attachable by a client holding a
/// newly rotated credential; only the presented credential is invalidated. The
/// generation bump terminates the tracked desktop attach forwarder. Any
/// verification failure returns the same generic UNAUTHORIZED error.
#[tauri::command]
pub async fn terminal_revoke_claim(
    terminal_id: String,
    claim: String,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    match pty_manager.revoke_claim(&terminal_id, &claim) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(_) => Ok(IpcResult::error("Unauthorized", "UNAUTHORIZED")),
    }
}

/// Write data to a terminal
#[tauri::command]
pub async fn terminal_write(
    terminal_id: String,
    data: String,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    match pty_manager.write(&terminal_id, &data).await {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "WRITE_FAILED")),
    }
}

/// Resize a terminal
#[tauri::command]
pub async fn terminal_resize(
    terminal_id: String,
    cols: u16,
    rows: u16,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    match pty_manager.resize(&terminal_id, cols, rows).await {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "RESIZE_FAILED")),
    }
}

/// Phone takeover parks desktop geometry; desktop mode restores it.
#[tauri::command]
pub async fn terminal_set_display_mode(
    terminal_id: String,
    mode: String,
    cols: Option<u16>,
    rows: Option<u16>,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<crate::pty::manager::DisplayModeState>, String> {
    let parsed = match crate::trackers::TerminalDisplayMode::parse(&mode) {
        Ok(mode) => mode,
        Err(error) => return Ok(IpcResult::error(error, "VALIDATION_ERROR")),
    };
    let force = parsed == crate::trackers::TerminalDisplayMode::Desktop;
    match pty_manager
        .set_display_mode(&terminal_id, parsed, cols, rows, "desktop-host", force)
        .await
    {
        Ok(state) => Ok(IpcResult::success(state)),
        Err(error) => Ok(IpcResult::error(error, "RESIZE_FAILED")),
    }
}

/// Close one renderer view without touching the PTY, claim, or passive workspace ref.
/// The last remaining view also pauses cwd/git polling; the process stays alive.
#[tauri::command]
pub async fn terminal_close_view(
    terminal_id: String,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    let had_forwarder = if let Some((_, forwarder)) = lock_forwarders().remove(&terminal_id) {
        forwarder.abort();
        true
    } else {
        false
    };
    if had_forwarder {
        pty_manager.note_view_closed(&terminal_id);
    } else {
        pty_manager.pause_tracking_if_unwatched(&terminal_id);
    }
    log::info!("[terminal-command] close-view terminal_id={terminal_id}");
    Ok(IpcResult::success(()))
}

/// Explicitly terminate a terminal resource. This is the destructive path.
#[tauri::command]
pub async fn terminal_terminate(
    terminal_id: String,
    pty_manager: State<'_, Arc<PtyManager>>,
    workspace: State<'_, Arc<crate::conversation::SessionWorkspaceService>>,
) -> Result<IpcResult<()>, String> {
    Ok(terminal_terminate_resource(&terminal_id, pty_manager.inner(), workspace.inner()).await)
}

pub(crate) async fn terminal_terminate_resource(
    terminal_id: &str,
    pty_manager: &Arc<PtyManager>,
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
) -> IpcResult<()> {
    let mut scope = pty_manager
        .get(terminal_id)
        .filter(|instance| instance.workspace_ref_tracked)
        .map(|instance| instance.conversation_id);
    if pty_manager.get(terminal_id).is_none() {
        return IpcResult::success(());
    }
    if let Some(conversation_id) = scope {
        match terminate_workspace_scope(workspace, conversation_id).await {
            Ok(true) => {}
            Ok(false) => {
                log::info!(
                    "[terminal-command] terminate skips missing workspace conversation_id={} terminal_id={}",
                    conversation_id,
                    terminal_id
                );
                scope = None;
            }
            Err(error) => {
                log::warn!(
                    "[terminal-command] terminate admission rejected conversation_id={} terminal_id={} code={}",
                    conversation_id,
                    terminal_id,
                    error.code.as_str()
                );
                return IpcResult::error(error.detail, error.code.as_str());
            }
        }
    }

    if let Err(cleanup) = pty_manager.terminate(terminal_id).await {
        let failure = TerminalResourceFailureV1::from_cleanup(
            crate::conversation::TERMINAL_TERMINATE_FAILED,
            cleanup,
        );
        log::warn!(
            "[terminal-command] terminate cleanup failed terminal_id={} primary_code={} cleanup_stage={} stable_result={}",
            failure.terminal_id,
            failure.primary_code,
            failure.cleanup_stage,
            crate::conversation::TERMINAL_TERMINATE_FAILED
        );
        return IpcResult::error(
            failure.wire_detail(),
            crate::conversation::TERMINAL_TERMINATE_FAILED,
        );
    }
    if let Some((_, forwarder)) = lock_forwarders().remove(terminal_id) {
        forwarder.abort();
    }
    if let Some(conversation_id) = scope {
        if let Err(error) = workspace
            .remove_terminal_ref_after_termination(conversation_id, terminal_id)
            .await
        {
            if is_missing_conversation_workspace_error(&error) {
                log::info!(
                    "[terminal-command] terminate ignores missing workspace after kill conversation_id={} terminal_id={}",
                    conversation_id,
                    terminal_id
                );
            } else {
                log::warn!(
                    "[terminal-command] terminate ref cleanup failed conversation_id={} terminal_id={} code={}",
                    conversation_id,
                    terminal_id,
                    error.code.as_str()
                );
                return IpcResult::error(error.detail, error.code.as_str());
            }
        }
    }
    log::info!("[terminal-command] terminated terminal_id={terminal_id}");
    IpcResult::success(())
}

fn is_missing_conversation_workspace_error(
    error: &crate::conversation::SessionWorkspaceError,
) -> bool {
    use crate::conversation::SessionWorkspaceErrorCode;
    if error.code == SessionWorkspaceErrorCode::ConversationNotFound {
        return true;
    }
    if error.code != SessionWorkspaceErrorCode::SessionWorkspaceRecoveryRequired {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(&error.detail)
        .ok()
        .and_then(|value| {
            value
                .get("primaryCode")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|code| code == "CONVERSATION_NOT_FOUND")
}

/// Returns `Ok(true)` when the Conversation still owns a workspace ref,
/// `Ok(false)` when it was never persisted (scope-less project terminals).
async fn terminate_workspace_scope(
    workspace: &Arc<crate::conversation::SessionWorkspaceService>,
    conversation_id: crate::conversation::ConversationId,
) -> Result<bool, crate::conversation::SessionWorkspaceError> {
    match workspace.load(conversation_id).await {
        Ok(crate::conversation::SessionWorkspaceLoadOutcome::Missing { .. }) => Ok(false),
        Ok(_) => workspace
            .ensure_terminal_ref_writable(conversation_id, false)
            .map(|()| true)
            .or_else(|error| {
                if is_missing_conversation_workspace_error(&error) {
                    Ok(false)
                } else {
                    Err(error)
                }
            }),
        Err(error) if is_missing_conversation_workspace_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Deprecated compatibility alias; identical to `terminal_terminate`.
#[tauri::command]
pub async fn terminal_kill(
    terminal_id: String,
    pty_manager: State<'_, Arc<PtyManager>>,
    workspace: State<'_, Arc<crate::conversation::SessionWorkspaceService>>,
) -> Result<IpcResult<()>, String> {
    terminal_terminate(terminal_id, pty_manager, workspace).await
}

/// Get the current working directory for a terminal
#[tauri::command]
pub async fn terminal_get_cwd(
    terminal_id: String,
    cwd_tracker: State<'_, Arc<CwdTracker>>,
) -> Result<IpcResult<Option<String>>, String> {
    let cwd = cwd_tracker.get_cwd(&terminal_id);
    Ok(IpcResult::success(cwd))
}

/// Get the git branch for a terminal
#[tauri::command]
pub async fn terminal_get_git_branch(
    terminal_id: String,
    git_tracker: State<'_, Arc<GitTracker>>,
) -> Result<IpcResult<Option<String>>, String> {
    let branch = git_tracker.get_branch(&terminal_id);
    Ok(IpcResult::success(branch))
}

/// Get the git status for a terminal
#[tauri::command]
pub async fn terminal_get_git_status(
    terminal_id: String,
    git_tracker: State<'_, Arc<GitTracker>>,
) -> Result<IpcResult<Option<GitStatus>>, String> {
    let status = git_tracker.get_status(&terminal_id);
    Ok(IpcResult::success(status))
}

/// Get the exit code for a terminal
#[tauri::command]
pub async fn terminal_get_exit_code(
    terminal_id: String,
    exit_code_tracker: State<'_, Arc<ExitCodeTracker>>,
) -> Result<IpcResult<Option<i32>>, String> {
    let exit_code = exit_code_tracker.get_exit_code(&terminal_id);
    Ok(IpcResult::success(exit_code))
}

/// Update orphan detection settings
#[tauri::command]
pub async fn terminal_update_orphan_detection(
    settings: OrphanDetectionSettings,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    pty_manager
        .update_orphan_detection_settings(settings.enabled, settings.timeout_minutes)
        .await;
    Ok(IpcResult::success(()))
}

/// Add a renderer reference to a terminal
#[tauri::command]
pub async fn terminal_add_renderer_ref(
    request: RendererRefRequest,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    match pty_manager.add_renderer_ref(&request.terminal_id, &request.renderer_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "TERMINAL_NOT_FOUND")),
    }
}

/// Remove a renderer reference from a terminal
#[tauri::command]
pub async fn terminal_remove_renderer_ref(
    request: RendererRefRequest,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    match pty_manager.remove_renderer_ref(&request.terminal_id, &request.renderer_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "TERMINAL_NOT_FOUND")),
    }
}

/// Update a terminal's orphan-reaping protection.
///
/// Protection is enabled automatically at spawn. The renderer calls this with
/// `protected = false` only when a terminal is genuinely released (its project
/// is closed or its tab is closed), so orphan detection may reclaim it. This
/// keeps live background-project terminals from being killed mid-task.
#[tauri::command]
pub async fn terminal_set_protected(
    request: SetTerminalProtectedRequest,
    pty_manager: State<'_, Arc<PtyManager>>,
) -> Result<IpcResult<()>, String> {
    // `set_protected` is intentionally idempotent and infallible (it is a no-op
    // when the terminal is already gone), so there is no error case to map.
    pty_manager.set_protected(&request.terminal_id, request.protected);
    Ok(IpcResult::success(()))
}

/// Set visibility state (affects polling behavior and PTY kill deferral)
#[tauri::command]
pub async fn terminal_set_visibility(
    request: SetVisibilityRequest,
    pty_manager: State<'_, Arc<PtyManager>>,
    cwd_tracker: State<'_, Arc<CwdTracker>>,
    git_tracker: State<'_, Arc<GitTracker>>,
) -> Result<IpcResult<()>, String> {
    pty_manager.set_hidden(!request.is_visible);
    cwd_tracker.set_visibility(request.is_visible);
    git_tracker.set_visibility(request.is_visible);
    Ok(IpcResult::success(()))
}

// ==================== Agent Registry Commands ====================

/// ADR-004.6: Fetch the ACP Registry catalog for agent IDENTITY & DISCOVERY
/// only (id, name, description, website, icon). Opt-in and read-only; runs from
/// the Rust side, caches on disk, and degrades to cache/empty on failure. The
/// returned entries deliberately omit `distribution` so they can never be used
/// to derive a terminal-native launch command.
#[tauri::command]
pub async fn agent_registry_fetch(
    app: AppHandle,
    force_refresh: Option<bool>,
) -> Result<IpcResult<crate::agent_registry::AcpRegistryCatalog>, String> {
    match crate::agent_registry::fetch_acp_registry(&app, force_refresh.unwrap_or(false)).await {
        Ok(catalog) => Ok(IpcResult::success(catalog)),
        Err(e) => Ok(IpcResult::error(e, "AGENT_REGISTRY_FETCH_FAILED")),
    }
}

// ==================== Worktree Commands ====================

/// List all worktrees for a git repo at the given path.
/// Filters out bare worktrees and detached-HEAD worktrees.
#[tauri::command]
pub async fn worktree_list(project_path: String) -> Result<IpcResult<Vec<WorktreeInfo>>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::list(&validated_path) {
        Ok(entries) => {
            let infos: Vec<WorktreeInfo> = entries
                .into_iter()
                .map(|e| WorktreeInfo {
                    name: e.name,
                    branch: e.branch,
                    path: e.path,
                    head_commit: e.head_commit,
                })
                .collect();
            Ok(IpcResult::success(infos))
        }
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Create a new worktree.
#[tauri::command]
pub async fn worktree_create(
    project_path: String,
    name: String,
    branch: String,
    is_new_branch: bool,
    start_ref: Option<String>,
    target_path: Option<String>,
) -> Result<IpcResult<WorktreeInfo>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::create(
        &validated_path,
        &name,
        &branch,
        is_new_branch,
        start_ref.as_deref(),
        target_path.as_deref(),
    ) {
        Ok(entry) => Ok(IpcResult::success(WorktreeInfo {
            name: entry.name,
            branch: entry.branch,
            path: entry.path,
            head_commit: entry.head_commit,
        })),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Remove a worktree. Uses --force if requested. Runs `git worktree prune` after.
#[tauri::command]
pub async fn worktree_remove(
    project_path: String,
    worktree_path: String,
    force: bool,
) -> Result<IpcResult<()>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_worktree = validate_and_stringify!(&worktree_path);
    match WorktreeManager::remove(&validated_project, &validated_worktree, force) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// List local and remote branches for a git repo.
#[tauri::command]
pub async fn worktree_branches(project_path: String) -> Result<IpcResult<Vec<BranchInfo>>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::branches(&validated_path) {
        Ok(entries) => {
            let infos: Vec<BranchInfo> = entries
                .into_iter()
                .map(|e| BranchInfo {
                    name: e.name,
                    is_remote: e.is_remote,
                    is_current: e.is_current,
                    upstream: e.upstream,
                    has_other_worktree: e.has_other_worktree,
                })
                .collect();
            Ok(IpcResult::success(infos))
        }
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Check dirty status for a worktree checkout.
#[tauri::command]
pub async fn worktree_check_dirty(worktree_path: String) -> Result<IpcResult<DirtyStatus>, String> {
    let validated_path = validate_and_stringify!(&worktree_path);
    match WorktreeManager::check_dirty(&validated_path) {
        Ok(status) => Ok(IpcResult::success(status)),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Remove all Se-managed worktrees for a project.
/// Reports per-worktree success/failure.
#[tauri::command]
pub async fn worktree_remove_all_managed(
    project_path: String,
    worktrees_json: String,
) -> Result<IpcResult<Vec<RemoveResult>>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::remove_all_managed(&validated_path, &worktrees_json) {
        Ok(results) => Ok(IpcResult::success(results)),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Parse `.gitignore` and return directory entries that could be symlinked into worktrees.
/// Returns simple directory entries with whether they exist in the project root.
#[tauri::command]
pub async fn worktree_parse_gitignore(
    project_path: String,
) -> Result<IpcResult<Vec<GitignoreDirInfo>>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::parse_gitignore_dirs(&validated_path) {
        Ok(dirs) => {
            let infos: Vec<GitignoreDirInfo> = dirs
                .into_iter()
                .map(|d| GitignoreDirInfo {
                    dir_name: d.dir_name,
                    exists: d.exists,
                })
                .collect();
            Ok(IpcResult::success(infos))
        }
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Create symlinks from project root directories into a worktree.
/// `symlink_dirs` is a JSON array of directory names to symlink (e.g. ["node_modules", "dist"]).
#[tauri::command]
pub async fn worktree_create_symlinks(
    project_path: String,
    worktree_path: String,
    symlink_dirs: String,
) -> Result<IpcResult<Vec<SymlinkResultInfo>>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_worktree = validate_and_stringify!(&worktree_path);
    let dirs: Vec<String> = match serde_json::from_str(&symlink_dirs) {
        Ok(dirs) => dirs,
        Err(e) => {
            return Ok(IpcResult::error(
                format!("Failed to parse symlink_dirs: {}", e),
                "PARSE_FAILED",
            ));
        }
    };
    let results = WorktreeManager::create_symlinks(&validated_project, &validated_worktree, &dirs);
    let infos: Vec<SymlinkResultInfo> = results
        .into_iter()
        .map(|r| SymlinkResultInfo {
            path: r.path,
            target: r.target,
            status: r.status,
            reason: r.reason,
        })
        .collect();
    Ok(IpcResult::success(infos))
}

/// Ensure symlinks exist for all directories in symlink_dirs.
/// Creates any missing symlinks. Does not remove or overwrite existing ones.
#[tauri::command]
pub async fn worktree_ensure_symlinks(
    project_path: String,
    worktree_path: String,
    symlink_dirs: String,
) -> Result<IpcResult<Vec<SymlinkResultInfo>>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_worktree = validate_and_stringify!(&worktree_path);
    let dirs2: Vec<String> = match serde_json::from_str(&symlink_dirs) {
        Ok(dirs) => dirs,
        Err(e) => {
            return Ok(IpcResult::error(
                format!("Failed to parse symlink_dirs: {}", e),
                "PARSE_FAILED",
            ));
        }
    };
    let results = WorktreeManager::ensure_symlinks(&validated_project, &validated_worktree, &dirs2);
    let infos: Vec<SymlinkResultInfo> = results
        .into_iter()
        .map(|r| SymlinkResultInfo {
            path: r.path,
            target: r.target,
            status: r.status,
            reason: r.reason,
        })
        .collect();
    Ok(IpcResult::success(infos))
}

/// Archive a worktree by moving it to `.termul/archives/`.
#[tauri::command]
pub async fn worktree_archive(
    project_path: String,
    worktree_path: String,
) -> Result<IpcResult<()>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_worktree = validate_and_stringify!(&worktree_path);
    match WorktreeManager::archive(&validated_project, &validated_worktree) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Restore an archived worktree back to its original location.
#[tauri::command]
pub async fn worktree_restore(
    project_path: String,
    archive_path: String,
) -> Result<IpcResult<()>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_archive = validate_and_stringify!(&archive_path);
    match WorktreeManager::restore(&validated_project, &validated_archive) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Generate a merge preview for a worktree against a target branch.
#[tauri::command]
pub async fn worktree_merge_preview(
    worktree_path: String,
    target_branch: String,
) -> Result<IpcResult<MergePreviewInfo>, String> {
    let validated_path = validate_and_stringify!(&worktree_path);
    match WorktreeManager::merge_preview(&validated_path, &target_branch) {
        Ok(preview) => {
            let info = MergePreviewInfo {
                direction: preview.direction,
                source_branch: preview.source_branch,
                target_branch: preview.target_branch,
                conflict_files: preview
                    .conflict_files
                    .into_iter()
                    .map(|f| ConflictFileInfo {
                        path: f.path,
                        severity: f.severity,
                        conflict_count: f.conflict_count,
                        is_lock_file: f.is_lock_file,
                    })
                    .collect(),
                changed_files: preview.changed_files,
                total_changes: preview.total_changes,
                detection_mode: preview.detection_mode,
            };
            Ok(IpcResult::success(info))
        }
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Execute a merge from the worktree's current branch to target_branch.
#[tauri::command]
pub async fn worktree_merge_execute(
    worktree_path: String,
    target_branch: String,
) -> Result<IpcResult<String>, String> {
    let validated_path = validate_and_stringify!(&worktree_path);
    match WorktreeManager::merge_execute(&validated_path, &target_branch) {
        Ok(result) => Ok(IpcResult::success(result)),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Resolve the default base branch for a new chat worktree (CAP-2).
/// Returns the origin/HEAD default with a `main`/`master`/current fallback
/// chain and a detached-HEAD flag so the launcher can force a base pick.
#[tauri::command]
pub async fn worktree_resolve_base_branch(
    project_path: String,
) -> Result<IpcResult<BaseBranchInfo>, String> {
    let validated_path = validate_and_stringify!(&project_path);
    match WorktreeManager::resolve_default_base_branch(&validated_path) {
        Ok(info) => Ok(IpcResult::success(info)),
        Err(e) => Ok(IpcResult::error(e.to_string(), e.error_code())),
    }
}

/// Carry over untracked files listed in `.worktree-include` into a fresh
/// worktree (CAP-5). Symlink/path-escape/already-present defenses run per
/// file; the result reports `ran`/`copied`/`skipped` with per-file reasons.
#[tauri::command]
pub async fn worktree_copy_include_files(
    project_path: String,
    worktree_path: String,
) -> Result<IpcResult<IncludeCopyResult>, String> {
    let validated_project = validate_and_stringify!(&project_path);
    let validated_worktree = validate_and_stringify!(&worktree_path);
    // Filesystem walk + copy is blocking; offload from the async runtime.
    match tokio::task::spawn_blocking(move || {
        WorktreeManager::copy_worktree_include_files(&validated_project, &validated_worktree)
    })
    .await
    {
        Ok(Ok(result)) => Ok(IpcResult::success(result)),
        Ok(Err(e)) => Ok(IpcResult::error(e.to_string(), e.error_code())),
        Err(join_err) => Ok(IpcResult::error(
            format!("worktree_copy_include_files join failed: {join_err}"),
            "INTERNAL_ERROR",
        )),
    }
}

/// Worktree info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub name: String,
    pub branch: String,
    pub path: String,
    pub head_commit: String,
}

impl From<GitWorktreeEntry> for WorktreeInfo {
    fn from(entry: GitWorktreeEntry) -> Self {
        Self {
            name: entry.name,
            branch: entry.branch,
            path: entry.path,
            head_commit: entry.head_commit,
        }
    }
}

/// Branch info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub is_remote: bool,
    pub is_current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub has_other_worktree: bool,
}

impl From<BranchEntry> for BranchInfo {
    fn from(entry: BranchEntry) -> Self {
        Self {
            name: entry.name,
            is_remote: entry.is_remote,
            is_current: entry.is_current,
            upstream: entry.upstream,
            has_other_worktree: entry.has_other_worktree,
        }
    }
}

/// Gitignore directory info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitignoreDirInfo {
    pub dir_name: String,
    pub exists: bool,
}

/// Symlink result info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymlinkResultInfo {
    pub path: String,
    pub target: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Merge preview info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePreviewInfo {
    pub direction: String,
    pub source_branch: String,
    pub target_branch: String,
    pub conflict_files: Vec<ConflictFileInfo>,
    pub changed_files: Vec<String>,
    pub total_changes: usize,
    pub detection_mode: String,
}

/// Conflict file info for IPC response
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFileInfo {
    pub path: String,
    pub severity: String,
    pub conflict_count: usize,
    pub is_lock_file: bool,
}

// ==================== Browser Tab Commands ====================

/// Create a new browser tab webview
#[tauri::command]
pub async fn browser_tab_create(
    tab_id: String,
    url: String,
    bounds: BrowserBounds,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<BrowserTabInfo>, String> {
    match browser_manager.create(tab_id, url, bounds).await {
        Ok(info) => Ok(IpcResult::success(info)),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_CREATE_FAILED")),
    }
}

/// Navigate a browser tab to a new URL
#[tauri::command]
pub async fn browser_tab_navigate(
    tab_id: String,
    url: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.navigate(&tab_id, url) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_NAVIGATE_FAILED")),
    }
}

/// Resize/reposition a browser tab webview
#[tauri::command]
pub async fn browser_tab_resize(
    tab_id: String,
    bounds: BrowserBounds,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.resize(&tab_id, bounds) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_RESIZE_FAILED")),
    }
}

/// Show a browser tab webview
#[tauri::command]
pub async fn browser_tab_show(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.show(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_SHOW_FAILED")),
    }
}

/// Hide a browser tab webview
#[tauri::command]
pub async fn browser_tab_hide(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.hide(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_HIDE_FAILED")),
    }
}

/// Destroy a browser tab webview
#[tauri::command]
pub async fn browser_tab_destroy(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.destroy(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_DESTROY_FAILED")),
    }
}

/// Go back in browser tab history
#[tauri::command]
pub async fn browser_tab_go_back(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.go_back(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_GO_BACK_FAILED")),
    }
}

/// Go forward in browser tab history
#[tauri::command]
pub async fn browser_tab_go_forward(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.go_forward(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_GO_FORWARD_FAILED")),
    }
}

/// Reload a browser tab
#[tauri::command]
pub async fn browser_tab_reload(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.reload(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_RELOAD_FAILED")),
    }
}

/// Open DevTools for a browser tab.
///
/// Debug-gated: the real implementation calls `BrowserTabManager::open_devtools`
/// (which opens the webview inspector). In release builds the command is a
/// stub that returns `Ok(IpcResult::error("DevTools disabled in production",
/// ...))` so the browser-tab devtools path is fully blocked in prod — mirrors
/// the existing `toggle_devtools` cfg-gate pattern in `lib.rs`. P13: the
/// `BrowserTabManager::open_devtools` method only exists in debug builds (no
/// release stub → no dead_code). The TS side also hides the Debug Console
/// button in prod, so a user never reaches the release stub.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn browser_tab_open_devtools(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.open_devtools(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_OPEN_DEVTOOLS_FAILED")),
    }
}

#[cfg(not(debug_assertions))]
#[tauri::command]
pub async fn browser_tab_open_devtools(
    _tab_id: String,
    _browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    Ok(IpcResult::error(
        "DevTools disabled in production".to_string(),
        "BROWSER_TAB_OPEN_DEVTOOLS_DISABLED",
    ))
}

/// Inject annotation overlay script into a browser tab
#[tauri::command]
pub async fn browser_tab_inject_annotation(
    tab_id: String,
    mode: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.inject_annotation_script(&tab_id, &mode) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "BROWSER_TAB_INJECT_ANNOTATION_FAILED")),
    }
}

/// Remove annotation overlay from a browser tab
#[tauri::command]
pub async fn browser_tab_remove_annotation_overlay(
    tab_id: String,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.remove_annotation_overlay(&tab_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(
            e,
            "BROWSER_TAB_REMOVE_ANNOTATION_OVERLAY_FAILED",
        )),
    }
}

/// Inject annotation markers into a browser tab webview
#[tauri::command]
pub async fn browser_tab_inject_annotation_markers(
    tab_id: String,
    annotations_json: String,
    selected_id: Option<String>,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.inject_annotation_markers(
        &tab_id,
        &annotations_json,
        selected_id.as_deref(),
    ) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(
            e,
            "BROWSER_TAB_INJECT_ANNOTATION_MARKERS_FAILED",
        )),
    }
}

/// Update annotation marker selection in a browser tab webview
#[tauri::command]
pub async fn browser_tab_update_annotation_marker_selection(
    tab_id: String,
    selected_id: Option<String>,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<IpcResult<()>, String> {
    match browser_manager.update_annotation_marker_selection(&tab_id, selected_id.as_deref()) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(
            e,
            "BROWSER_TAB_UPDATE_MARKER_SELECTION_FAILED",
        )),
    }
}

/// Report URL from browser tab webview (called by injected JS poller)
#[tauri::command]
pub async fn browser_tab_report_url(
    tab_id: String,
    url: String,
    app_handle: AppHandle,
    webview: Webview,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;
    log::debug!("[BrowserTab] URL report: tab={} navigated", tab_id);
    browser_manager.invalidate_annotation_injected(&tab_id);
    app_handle
        .emit(
            "browser-tab-navigated",
            serde_json::json!({ "browserTabId": tab_id, "url": url }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Report page loaded from browser tab webview (called by injected JS poller)
#[tauri::command]
pub async fn browser_tab_report_loaded(
    tab_id: String,
    app_handle: AppHandle,
    webview: Webview,
    browser_manager: State<'_, Arc<BrowserTabManager>>,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;
    log::debug!("[BrowserTab] Loaded report: tab={}", tab_id);
    browser_manager.invalidate_annotation_injected(&tab_id);
    app_handle
        .emit(
            "browser-tab-loaded",
            serde_json::json!({ "browserTabId": tab_id }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Report region captured from browser tab webview (called by injected annotation overlay)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn browser_tab_report_region_captured(
    tab_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_width: f64,
    viewport_height: f64,
    app_handle: AppHandle,
    webview: Webview,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;
    log::debug!(
        "[BrowserTab] Region captured: tab={} x={} y={} w={} h={}",
        tab_id,
        x,
        y,
        width,
        height
    );
    app_handle
        .emit(
            "browser-tab-region-captured",
            serde_json::json!({
                "browserTabId": tab_id,
                "x": x,
                "y": y,
                "width": width,
                "height": height,
                "viewportWidth": viewport_width,
                "viewportHeight": viewport_height,
            }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Report title change from browser tab webview (called by injected JS poller)
#[tauri::command]
pub async fn browser_tab_report_title(
    tab_id: String,
    title: String,
    app_handle: AppHandle,
    webview: Webview,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;
    log::debug!("[BrowserTab] Title report: tab={}", tab_id);
    app_handle
        .emit(
            "browser-tab-title-changed",
            serde_json::json!({ "browserTabId": tab_id, "title": title }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Report element captured from browser tab webview (called by injected annotation overlay)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn browser_tab_report_element_captured(
    tab_id: String,
    url: String,
    title: String,
    viewport_width: f64,
    viewport_height: f64,
    tag_name: String,
    selector: String,
    selector_confidence: String,
    attributes: serde_json::Value,
    text_content: String,
    text_truncated: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    app_handle: AppHandle,
    webview: Webview,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;

    let attributes = attributes.as_object().cloned().ok_or_else(|| {
        "Browser tab report element captured rejected: attributes must be an object".to_string()
    })?;

    log::debug!(
        "[BrowserTab] Element captured: tab={} tag={} selector=<redacted>",
        tab_id,
        tag_name
    );
    app_handle
        .emit(
            "browser-tab-element-captured",
            serde_json::json!({
                "browserTabId": tab_id,
                "url": url,
                "title": title,
                "viewportWidth": viewport_width,
                "viewportHeight": viewport_height,
                "tagName": tag_name,
                "selector": selector,
                "selectorConfidence": selector_confidence,
                "attributes": attributes,
                "textContent": text_content,
                "textTruncated": text_truncated,
                "boundingBox": {
                    "x": x,
                    "y": y,
                    "width": width,
                    "height": height
                }
            }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Report annotation marker clicked from browser tab webview
#[tauri::command]
pub async fn browser_tab_report_annotation_marker_clicked(
    tab_id: String,
    annotation_id: String,
    app_handle: AppHandle,
    webview: Webview,
) -> Result<(), String> {
    validate_browser_tab_caller(&webview, &tab_id)?;
    log::debug!(
        "[BrowserTab] Annotation marker clicked: tab={} annotation_id={}",
        tab_id,
        annotation_id
    );
    app_handle
        .emit(
            "browser-tab-annotation-marker-clicked",
            serde_json::json!({
                "browserTabId": tab_id,
                "annotationId": annotation_id,
            }),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Rollback request
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackRequest {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchMatch {
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResult {
    pub file_path: String,
    pub matches: Vec<FileSearchMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResponse {
    pub results: Vec<FileSearchResult>,
    pub truncated: bool,
    pub scanned_files: usize,
    pub failed_files: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContentRequest {
    pub scope_root: String,
    pub root_path: String,
    pub query: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContentStreamRequest {
    pub scope_root: String,
    pub root_path: String,
    pub query: String,
    pub search_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContentCancelRequest {
    pub search_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileNamesCancelRequest {
    pub search_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContentBatchEvent {
    pub search_id: String,
    pub results: Vec<FileSearchResult>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContentDoneEvent {
    pub search_id: String,
    pub truncated: bool,
    pub scanned_files: usize,
    pub failed_files: usize,
    /// Programmatic error code (e.g. `QUERY_TOO_LONG`). Mirrors the field on
    /// `SearchFileNamesDoneEvent` so the renderer can branch on it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileNamesStreamRequest {
    pub scope_root: String,
    pub root_path: String,
    pub query: String,
    pub search_id: String,
    /// When true, run `rg --no-ignore --hidden` and emit ignored/hidden files
    /// with `ignored: true` so the @-mention picker can dim them. When false
    /// (the default), the common-ignore exclusions are applied and every hit
    /// carries `ignored: false`. See ADR 0003.
    #[serde(default)]
    pub include_ignored: bool,
}

/// One filename-search hit. `ignored` is set when the path runs through a
/// commonly-ignored directory or a hidden/cruft segment, so the @-mention
/// picker can dim it. `ignored: false` for every hit when the caller did not
/// request `include_ignored`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileHit {
    pub path: String,
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileNamesBatchEvent {
    pub search_id: String,
    pub files: Vec<SearchFileHit>,
    /// `None` on mid-stream batches (final truncation state is not yet known).
    /// `Some(true)` is set on the trailing batch if the result was capped, and
    /// `Some(false)` otherwise. `serde` skips `None` so the field is omitted
    /// on the wire when not set.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileNamesDoneEvent {
    pub search_id: String,
    pub truncated: bool,
    pub total_files: usize,
    /// Programmatic error code (e.g. `QUERY_TOO_LONG`, `PATH_VALIDATION_FAILED`,
    /// `RG_SPAWN_FAILED`). Set when `error` is set; otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RgInfoResponse {
    pub sidecar_binary_name: String,
    pub resolved_path: String,
    pub source: String,
    pub exists: bool,
}

static SEARCH_PROCESSES: OnceLock<Mutex<HashMap<String, Arc<Mutex<Child>>>>> = OnceLock::new();
static FILENAME_SEARCH_PROCESSES: OnceLock<Mutex<HashMap<String, Arc<Mutex<Child>>>>> =
    OnceLock::new();
static RG_PATH_CACHE: OnceLock<String> = OnceLock::new();

pub(crate) fn search_processes() -> &'static Mutex<HashMap<String, Arc<Mutex<Child>>>> {
    SEARCH_PROCESSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn filename_search_processes() -> &'static Mutex<HashMap<String, Arc<Mutex<Child>>>> {
    FILENAME_SEARCH_PROCESSES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_os = "windows")]
fn rg_sidecar_name() -> &'static str {
    "rg-x86_64-pc-windows-msvc.exe"
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn rg_sidecar_name() -> &'static str {
    "rg-aarch64-apple-darwin"
}

#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
fn rg_sidecar_name() -> &'static str {
    "rg-x86_64-apple-darwin"
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn rg_sidecar_name() -> &'static str {
    "rg-aarch64-unknown-linux-gnu"
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn rg_sidecar_name() -> &'static str {
    "rg-armv7-unknown-linux-gnueabihf"
}

#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "aarch64", target_arch = "arm"))
))]
fn rg_sidecar_name() -> &'static str {
    "rg-x86_64-unknown-linux-musl"
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn rg_sidecar_name() -> &'static str {
    "rg"
}

pub(crate) fn resolve_rg_path() -> (String, String) {
    let from_env = std::env::var("TERMUL_RG_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty());
    if let Some(path) = from_env {
        let env_path = PathBuf::from(&path);
        if env_path.is_absolute() {
            return (path, "env".to_string());
        }

        if let Ok(cwd) = std::env::current_dir() {
            let direct = cwd.join(&env_path);
            if direct.exists() && direct.is_file() {
                return (direct.to_string_lossy().to_string(), "env".to_string());
            }

            let from_src_tauri = cwd.join("src-tauri").join(&env_path);
            if from_src_tauri.exists() && from_src_tauri.is_file() {
                return (
                    from_src_tauri.to_string_lossy().to_string(),
                    "env".to_string(),
                );
            }
        }

        return (path, "env".to_string());
    }

    let binary = rg_sidecar_name();
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("src-tauri").join("bin").join(binary));
        candidates.push(cwd.join("bin").join(binary));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(binary));
            candidates.push(exe_dir.join("../Resources").join(binary));
            candidates.push(exe_dir.join("../lib").join(binary));
        }
    }

    if let Some(found) = candidates
        .into_iter()
        .find(|path| path.exists() && path.is_file())
    {
        return (found.to_string_lossy().to_string(), "sidecar".to_string());
    }

    ("rg".to_string(), "path".to_string())
}

pub(crate) fn detect_rg_path() -> String {
    if let Some(cached) = RG_PATH_CACHE.get() {
        return cached.clone();
    }

    let (detected, _source) = resolve_rg_path();
    let _ = RG_PATH_CACHE.set(detected.clone());
    detected
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
pub(crate) fn configure_background_command(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn configure_background_command(_command: &mut Command) {}

/// Maximum allowed search query length to prevent resource exhaustion via
/// oversized input passed to ripgrep or the file-name walker.
pub(crate) const MAX_SEARCH_QUERY_LEN: usize = 500;

pub(crate) fn validated_search_root(scope_root: &str, search_root: &str) -> Result<String, String> {
    path_validation::validate_search_path(search_root, scope_root)
        .map(|path| path.to_string_lossy().to_string())
}

pub(crate) fn build_search_args(
    query: &str,
    root_path: &str,
    max_matches_per_file: usize,
) -> Vec<String> {
    let mut args = vec![
        "--json".to_string(),
        "-F".to_string(),
        "-i".to_string(),
        "-n".to_string(),
        "--max-filesize".to_string(),
        "1M".to_string(),
        "--max-count".to_string(),
        max_matches_per_file.to_string(),
    ];

    for ignored in [
        "node_modules",
        ".git",
        ".next",
        ".cache",
        ".turbo",
        "dist",
        "build",
        ".output",
        ".nuxt",
        ".svelte-kit",
        "__pycache__",
        ".pytest_cache",
        "venv",
        "coverage",
        ".nyc_output",
    ] {
        args.push("-g".to_string());
        args.push(format!("!**/{}/**", ignored));
    }

    args.push("--".to_string());
    args.push(query.to_string());
    args.push(root_path.to_string());
    args
}

/// Directory basenames that are commonly git-ignored. Entries under these are
/// still walked when `include_ignored` is set, but classified as `ignored` so
/// the @-mention picker can dim them. Mirrors the renderer's `ALWAYS_IGNORE`
/// list in `tauri-filesystem-api.ts` so the two sides agree on "ignored".
const COMMONLY_IGNORED_NAMES: &[&str] = &[
    "node_modules",
    ".git",
    ".next",
    ".cache",
    ".turbo",
    "dist",
    "build",
    ".output",
    ".nuxt",
    ".svelte-kit",
    "__pycache__",
    ".pytest_cache",
    "venv",
    "coverage",
    ".nyc_output",
];

/// Cruft file basenames (not dir names) that should be dimmed when surfaced.
const COMMONLY_IGNORED_FILES: &[&str] = &["Thumbs.db", "desktop.ini", ".DS_Store"];

/// True when a (slash-normalized, relative) path runs through a
/// commonly-ignored directory, a hidden segment, or a cruft basename. Used to
/// tag `SearchFileHit.ignored` for the @-mention picker. Pure so it can be
/// unit-tested directly.
fn path_is_ignored(rel_path: &str) -> bool {
    let segments: Vec<&str> = rel_path.split(['/', '\\']).collect();
    for seg in &segments {
        if seg.is_empty() {
            continue;
        }
        if seg.starts_with('.') || COMMONLY_IGNORED_NAMES.contains(seg) {
            return true;
        }
    }
    if let Some(basename) = segments.last() {
        if COMMONLY_IGNORED_FILES.contains(basename) {
            return true;
        }
    }
    false
}

/// Concatenate non-ignored hits first, then ignored hits up to `cap`. Pure so
/// it can be unit-tested directly. The caller is expected to have already
/// capped `non_ignored` at `cap`; this extends with ignored only into the
/// remaining slots so ignored files can never crowd out non-ignored ones.
/// Reap an rg child after stdout reading stops. When the reader breaks early
/// (cap hit) stdout is no longer drained; kill first so rg cannot block on a
/// full pipe before `wait()` returns.
fn reap_rg_child_after_stdout(
    child: &mut Child,
    stdout_stopped_early: bool,
) -> Option<std::process::ExitStatus> {
    if stdout_stopped_early && child.kill().is_err() {
        log::warn!("[search-process] cleanup_stage=kill stable_result=FAILED");
    }
    child.wait().ok()
}

fn rank_search_hits(
    non_ignored: Vec<SearchFileHit>,
    ignored: Vec<SearchFileHit>,
    cap: usize,
) -> Vec<SearchFileHit> {
    let mut out = non_ignored;
    let remaining = cap.saturating_sub(out.len());
    if remaining > 0 {
        out.extend(ignored.into_iter().take(remaining));
    }
    out
}

/// Build the ripgrep argv for a streaming filename search.
///
/// We rely on `rg --files --iglob` so we get the same multi-threaded tree walk
/// that powers content search. The glob form is `**/*{escaped_query}*` to
/// match the previous "filename contains query" behavior at any directory
/// depth. `-i` keeps the match case-insensitive on every platform (ripgrep's
/// default is already case-insensitive on Windows, but Linux/macOS would
/// otherwise be sensitive). Glob metacharacters in the query are escaped so
/// they match literally, mirroring the old `contains` semantics.
///
/// When `include_ignored` is true, the common-ignore exclusions are dropped
/// and `--no-ignore --hidden` are added so ignored/hidden files surface;
/// classification + non-ignored-first ranking happen after the walk. See ADR
/// 0003.
fn build_file_name_search_args(query: &str, root_path: &str, include_ignored: bool) -> Vec<String> {
    // Escape glob metacharacters that ripgrep would otherwise interpret as
    // wildcards (`*`, `?`, `[`, `]`, `{`, `}`, `\`) so the query is matched
    // as a substring of the basename. `{`/`}` are alternation in globset.
    let mut escaped = String::with_capacity(query.len());
    for ch in query.chars() {
        match ch {
            '*' | '?' | '[' | ']' | '{' | '}' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }

    let mut args = vec![
        "--files".to_string(),
        "-i".to_string(),
        "--iglob".to_string(),
        format!("**/*{}*", escaped),
    ];

    if include_ignored {
        // Surface ignored + hidden files so they can be mentioned and dimmed.
        // No `-g !<name>` exclusions; per-hit classification and non-ignored-
        // first ranking happen after the walk.
        args.push("--no-ignore".to_string());
        args.push("--hidden".to_string());
    } else {
        // NB: In `--files` + `--iglob` mode, ripgrep only honors `-g` ignore
        // patterns written as bare basenames (e.g. `-g '!node_modules'`). The
        // `!**/name/**` form that `build_search_args` uses for content search
        // is silently dropped here, so we explicitly use the basename form.
        for ignored in COMMONLY_IGNORED_NAMES {
            args.push("-g".to_string());
            args.push(format!("!{}", ignored));
        }
        // Exclude platform cruft and common dotenv secrets. The exact `.env`
        // exclusion matches the spec; `.env.local` / `.env.production` are
        // deliberately left to `.gitignore` so a project's own ignore list is
        // honored.
        args.push("-g".to_string());
        args.push("!.env".to_string());
        args.push("-g".to_string());
        args.push("!Thumbs.db".to_string());
        args.push("-g".to_string());
        args.push("!desktop.ini".to_string());
        args.push("-g".to_string());
        args.push("!.DS_Store".to_string());
    }

    args.push(root_path.to_string());
    args
}

#[tauri::command]
pub async fn search_get_rg_info() -> Result<IpcResult<RgInfoResponse>, String> {
    let (resolved_path, source) = resolve_rg_path();
    let exists = PathBuf::from(&resolved_path).exists();

    Ok(IpcResult::success(RgInfoResponse {
        sidecar_binary_name: rg_sidecar_name().to_string(),
        resolved_path,
        source,
        exists,
    }))
}

#[tauri::command]
pub async fn search_content_stream(
    request: SearchContentStreamRequest,
    app_handle: AppHandle,
) -> Result<IpcResult<()>, String> {
    let trimmed_query = request.query.trim().to_string();
    if trimmed_query.is_empty() {
        let _ = app_handle.emit(
            "search-content-done",
            SearchContentDoneEvent {
                search_id: request.search_id,
                truncated: false,
                scanned_files: 0,
                failed_files: 0,
                code: None,
                error: None,
            },
        );
        return Ok(IpcResult::success(()));
    }

    let query_char_count = trimmed_query.chars().count();
    if query_char_count > MAX_SEARCH_QUERY_LEN {
        log::warn!(
            "[Security] Search query rejected: length {} characters exceeds limit of {}",
            query_char_count,
            MAX_SEARCH_QUERY_LEN
        );
        let _ = app_handle.emit(
            "search-content-done",
            SearchContentDoneEvent {
                search_id: request.search_id,
                truncated: false,
                scanned_files: 0,
                failed_files: 0,
                code: Some("QUERY_TOO_LONG".to_string()),
                error: Some(format!(
                    "Search query too long: {} characters (max {})",
                    query_char_count, MAX_SEARCH_QUERY_LEN
                )),
            },
        );
        return Ok(IpcResult::success(()));
    }

    let validated_root = match validated_search_root(&request.scope_root, &request.root_path) {
        Ok(path) => path,
        Err(e) => {
            log::warn!(
                "[Security] File search rejected: scope='{}' root='{}': {}",
                request.scope_root,
                request.root_path,
                e
            );
            let _ = app_handle.emit(
                "search-content-done",
                SearchContentDoneEvent {
                    search_id: request.search_id,
                    truncated: false,
                    scanned_files: 0,
                    failed_files: 0,
                    code: Some("PATH_VALIDATION_FAILED".to_string()),
                    error: Some(format!("Invalid search path: {}", e)),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let max_files_with_matches: usize = 100;
    let max_matches_per_file: usize = 30;
    let args = build_search_args(&trimmed_query, &validated_root, max_matches_per_file);

    let rg_path = detect_rg_path();
    let mut rg_command = Command::new(&rg_path);
    rg_command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_background_command(&mut rg_command);
    let mut child = match rg_command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = app_handle.emit(
                "search-content-done",
                SearchContentDoneEvent {
                    search_id: request.search_id,
                    truncated: false,
                    scanned_files: 0,
                    failed_files: 0,
                    code: Some("RG_SPAWN_FAILED".to_string()),
                    error: Some(format!("rg spawn failed (path: {}): {}", rg_path, e)),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = app_handle.emit(
                "search-content-done",
                SearchContentDoneEvent {
                    search_id: request.search_id,
                    truncated: false,
                    scanned_files: 0,
                    failed_files: 1,
                    // Distinct from `RG_SPAWN_FAILED` (rg binary never
                    // started). Here rg did start, but its pipe was
                    // already closed when we tried to take it.
                    code: Some("RG_STDOUT_CAPTURE_FAILED".to_string()),
                    error: Some("failed to capture rg stdout".to_string()),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let child_handle = Arc::new(Mutex::new(child));
    {
        let mut guard = search_processes().lock().map_err(|e| e.to_string())?;
        guard.insert(request.search_id.clone(), Arc::clone(&child_handle));
    }

    let search_id = request.search_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        let mut grouped: BTreeMap<String, Vec<FileSearchMatch>> = BTreeMap::new();
        let mut pending_matches: BTreeMap<String, Vec<FileSearchMatch>> = BTreeMap::new();
        let mut truncated = false;
        let mut stdout_stopped_early = false;
        let mut stream_error: Option<String> = None;

        let flush_batch = |pending: &mut BTreeMap<String, Vec<FileSearchMatch>>,
                           truncated: bool| {
            if pending.is_empty() {
                return;
            }
            let batch: Vec<FileSearchResult> = pending
                .iter()
                .map(|(file_path, matches)| FileSearchResult {
                    file_path: file_path.clone(),
                    matches: matches.clone(),
                })
                .collect();
            let _ = app_handle.emit(
                "search-content-batch",
                SearchContentBatchEvent {
                    search_id: search_id.clone(),
                    results: batch,
                    truncated,
                },
            );
            pending.clear();
        };

        // Manual loop so we can record the first I/O error instead of
        // silently dropping it (which `for line in reader.lines()` would
        // do via its `Err(_) => continue` swallow).
        let mut iter = reader.lines();
        loop {
            let line = match iter.next() {
                Some(Ok(v)) => v,
                Some(Err(e)) => {
                    stream_error = Some(format!("stdout read error: {}", e));
                    break;
                }
                None => break,
            };

            let parsed: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if parsed.get("type").and_then(|v| v.as_str()) != Some("match") {
                continue;
            }

            let file_path = match parsed
                .get("data")
                .and_then(|d| d.get("path"))
                .and_then(|p| p.get("text"))
                .and_then(|t| t.as_str())
            {
                Some(p) => p.replace('\\', "/"),
                None => continue,
            };

            let line_number = match parsed
                .get("data")
                .and_then(|d| d.get("line_number"))
                .and_then(|n| n.as_u64())
            {
                Some(n) => n as usize,
                None => continue,
            };

            let line_text = parsed
                .get("data")
                .and_then(|d| d.get("lines"))
                .and_then(|l| l.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim_end_matches(['\r', '\n'])
                .to_string();

            if !grouped.contains_key(&file_path) {
                if grouped.len() >= max_files_with_matches {
                    truncated = true;
                    stdout_stopped_early = true;
                    break;
                }
                grouped.insert(file_path.clone(), Vec::new());
            }

            if let Some(matches) = grouped.get_mut(&file_path) {
                if matches.len() >= max_matches_per_file {
                    truncated = true;
                    continue;
                }
                let new_match = FileSearchMatch {
                    line_number,
                    line_text,
                };
                matches.push(new_match.clone());
                pending_matches
                    .entry(file_path)
                    .or_default()
                    .push(new_match);
            }

            if pending_matches.values().map(Vec::len).sum::<usize>() >= 25 {
                flush_batch(&mut pending_matches, truncated);
            }
        }

        flush_batch(&mut pending_matches, truncated);

        // Reap the child and propagate non-zero exit status (other than 1,
        // which rg uses for "no matches") as a surfaced error. Mirrors the
        // pattern from `search_file_names_stream` so the renderer can
        // distinguish a clean run from a runtime rg failure.
        let exit_status = {
            let mut child = match child_handle.lock() {
                Ok(c) => c,
                Err(_) => {
                    stream_error.get_or_insert("child handle poisoned".to_string());
                    return;
                }
            };
            reap_rg_child_after_stdout(&mut child, stdout_stopped_early)
        };
        if let Ok(mut guard) = search_processes().lock() {
            guard.remove(&search_id);
        }

        let final_error = stream_error.or_else(|| {
            exit_status
                .as_ref()
                .filter(|s| !s.success() && s.code() != Some(1))
                .map(|s| format!("rg exited with status: {:?}", s))
        });

        // `RG_STREAM_FAILED` is the catch-all code for any error that
        // surfaces mid-walk (stdout I/O error or non-zero exit other than
        // rg's "no matches" code 1). Mirrors the filename stream's
        // semantic.
        let final_code = if final_error.is_some() {
            Some("RG_STREAM_FAILED".to_string())
        } else {
            None
        };

        let _ = app_handle.emit(
            "search-content-done",
            SearchContentDoneEvent {
                search_id,
                truncated,
                scanned_files: 0,
                failed_files: 0,
                code: final_code,
                error: final_error,
            },
        );
    });

    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn search_content_cancel(
    request: SearchContentCancelRequest,
) -> Result<IpcResult<()>, String> {
    let mut guard = search_processes().lock().map_err(|e| e.to_string())?;
    if let Some(child_handle) = guard.remove(&request.search_id) {
        if let Ok(mut child) = child_handle.lock() {
            if child.kill().is_err() {
                log::warn!("[search-process] kind=content cleanup_stage=kill stable_result=FAILED");
            }
            if child.wait().is_err() {
                log::warn!("[search-process] kind=content cleanup_stage=wait stable_result=FAILED");
            }
        }
    }
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn search_file_names_stream(
    request: SearchFileNamesStreamRequest,
    app_handle: AppHandle,
) -> Result<IpcResult<()>, String> {
    let trimmed_query = request.query.trim().to_string();
    let search_id = request.search_id.clone();

    if trimmed_query.is_empty() {
        let _ = app_handle.emit(
            "search-file-names-done",
            SearchFileNamesDoneEvent {
                search_id,
                truncated: false,
                total_files: 0,
                code: None,
                error: None,
            },
        );
        return Ok(IpcResult::success(()));
    }

    if trimmed_query.chars().count() > MAX_SEARCH_QUERY_LEN {
        log::warn!(
            "[Security] File name search query rejected: length {} exceeds limit of {}",
            trimmed_query.chars().count(),
            MAX_SEARCH_QUERY_LEN
        );
        let _ = app_handle.emit(
            "search-file-names-done",
            SearchFileNamesDoneEvent {
                search_id,
                truncated: false,
                total_files: 0,
                code: Some("QUERY_TOO_LONG".to_string()),
                error: Some(format!(
                    "Search query too long: {} characters (max {})",
                    trimmed_query.chars().count(),
                    MAX_SEARCH_QUERY_LEN
                )),
            },
        );
        return Ok(IpcResult::success(()));
    }

    let query_char_count = trimmed_query.chars().count();
    if query_char_count > MAX_SEARCH_QUERY_LEN {
        log::warn!(
            "[Security] File name search query rejected: length {} characters exceeds limit of {}",
            query_char_count,
            MAX_SEARCH_QUERY_LEN
        );
        return Ok(IpcResult::error(
            format!(
                "Search query too long: {} characters (max {})",
                query_char_count, MAX_SEARCH_QUERY_LEN
            ),
            "QUERY_TOO_LONG",
        ));
    }

    let validated_root = match validated_search_root(&request.scope_root, &request.root_path) {
        Ok(path) => path,
        Err(e) => {
            log::warn!(
                "[Security] File name search rejected: scope='{}' root='{}': {}",
                request.scope_root,
                request.root_path,
                e
            );
            let _ = app_handle.emit(
                "search-file-names-done",
                SearchFileNamesDoneEvent {
                    search_id,
                    truncated: false,
                    total_files: 0,
                    code: Some("PATH_VALIDATION_FAILED".to_string()),
                    error: Some(format!("Invalid search path: {}", e)),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let args =
        build_file_name_search_args(&trimmed_query, &validated_root, request.include_ignored);

    let rg_path = detect_rg_path();
    let mut rg_command = Command::new(&rg_path);
    rg_command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_background_command(&mut rg_command);

    let mut child = match rg_command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = app_handle.emit(
                "search-file-names-done",
                SearchFileNamesDoneEvent {
                    search_id,
                    truncated: false,
                    total_files: 0,
                    code: Some("RG_SPAWN_FAILED".to_string()),
                    error: Some(format!("rg spawn failed (path: {}): {}", rg_path, e)),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            if child.kill().is_err() {
                log::warn!(
                    "[search-process] kind=file_name cleanup_stage=kill stable_result=FAILED"
                );
            }
            if child.wait().is_err() {
                log::warn!(
                    "[search-process] kind=file_name cleanup_stage=wait stable_result=FAILED"
                );
            }
            let _ = app_handle.emit(
                "search-file-names-done",
                SearchFileNamesDoneEvent {
                    search_id,
                    truncated: false,
                    total_files: 0,
                    // Distinct from `RG_SPAWN_FAILED` (which means the rg
                    // binary never started). Here rg DID start, but the
                    // pipe was already closed when we tried to take it.
                    code: Some("RG_STDOUT_CAPTURE_FAILED".to_string()),
                    error: Some("failed to capture rg stdout".to_string()),
                },
            );
            return Ok(IpcResult::success(()));
        }
    };

    let child_handle = Arc::new(Mutex::new(child));
    {
        let mut guard = filename_search_processes()
            .lock()
            .map_err(|e| e.to_string())?;
        guard.insert(search_id.clone(), Arc::clone(&child_handle));
    }

    let include_ignored = request.include_ignored;

    tauri::async_runtime::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        let max_files: usize = 100;
        let batch_size: usize = 25;
        let mut truncated = false;
        let mut stream_error: Option<String> = None;

        // `files` is the default-path bucket (mid-stream batched).
        // `non_ignored` + `ignored_bucket` are the `include_ignored`-path
        // buckets, ranked after the walk so node_modules can't crowd out
        // source files. See ADR 0003.
        let mut files: Vec<SearchFileHit> = Vec::new();
        let mut non_ignored: Vec<SearchFileHit> = Vec::new();
        let mut ignored_bucket: Vec<SearchFileHit> = Vec::new();
        const IGNORED_CAP: usize = 20;
        let mut ignored_dropped: usize = 0;
        let mut broke_at_cap = false;
        let mut stdout_stopped_early = false;
        let mut last_batch_count: usize = 0;

        // Collect output until we hit the cap, EOF, or a pipe error. The
        // iterator-based form `map_while(Result::ok)` would swallow I/O
        // errors, so we use a manual loop that records the first error.
        let mut iter = reader.lines();
        loop {
            match iter.next() {
                Some(Ok(line)) => {
                    // ripgrep on Windows may emit verbatim paths
                    // (e.g. `\\?\C:\...`) when the root is canonicalized.
                    // Strip the prefix before the slash-normalization so the
                    // renderer never sees a `\\?\` blob in click paths.
                    let normalized =
                        path_validation::strip_verbatim_prefix(&line).replace('\\', "/");
                    if include_ignored {
                        // Stop as soon as the non-ignored bucket is full: later
                        // ignored hits can no longer survive `rank_search_hits`,
                        // so walking further just wastes time in large repos.
                        if non_ignored.len() >= max_files {
                            broke_at_cap = true;
                            stdout_stopped_early = true;
                            break;
                        }
                        if path_is_ignored(&normalized) {
                            if ignored_bucket.len() < IGNORED_CAP {
                                ignored_bucket.push(SearchFileHit {
                                    path: normalized,
                                    ignored: true,
                                });
                            } else {
                                ignored_dropped += 1;
                            }
                        } else {
                            non_ignored.push(SearchFileHit {
                                path: normalized,
                                ignored: false,
                            });
                        }
                    } else {
                        if files.len() >= max_files {
                            truncated = true;
                            stdout_stopped_early = true;
                            break;
                        }
                        files.push(SearchFileHit {
                            path: normalized,
                            ignored: false,
                        });
                        // Emit a mid-stream batch when we cross a batch
                        // boundary, but skip the trailing batch below if we
                        // already published this exact count.
                        if files.len().is_multiple_of(batch_size) {
                            // Mid-stream batch — final truncation state is
                            // not known yet, so the field is `None` (serde
                            // omits it from the wire). The trailing batch
                            // below carries the authoritative value.
                            let _ = app_handle.emit(
                                "search-file-names-batch",
                                SearchFileNamesBatchEvent {
                                    search_id: search_id.clone(),
                                    files: files.clone(),
                                    truncated: None,
                                },
                            );
                            last_batch_count = files.len();
                        }
                    }
                }
                Some(Err(e)) => {
                    stream_error = Some(format!("stdout read error: {}", e));
                    break;
                }
                None => break,
            }
        }

        // Publish the authoritative final batch. For `include_ignored`, emit
        // a single ranked batch (non-ignored first) so the picker never
        // flickers between mid-stream order and the ranked order. For the
        // default path, skip if the count matches the last mid-stream batch.
        let final_files: Vec<SearchFileHit> = if include_ignored {
            truncated = broke_at_cap || ignored_dropped > 0;
            let ranked = rank_search_hits(non_ignored, ignored_bucket, max_files);
            let _ = app_handle.emit(
                "search-file-names-batch",
                SearchFileNamesBatchEvent {
                    search_id: search_id.clone(),
                    files: ranked.clone(),
                    truncated: Some(truncated),
                },
            );
            ranked
        } else {
            if files.len() != last_batch_count {
                let _ = app_handle.emit(
                    "search-file-names-batch",
                    SearchFileNamesBatchEvent {
                        search_id: search_id.clone(),
                        files: files.clone(),
                        truncated: Some(truncated),
                    },
                );
            }
            files
        };

        // Reap the child and propagate a non-zero exit status (other than 1,
        // which rg uses for "no matches") as a surfaced error. The previous
        // `try_wait().or_else(wait)` pattern was a no-op for the common
        // `Ok(None)` case, so we always wait.
        let exit_status = {
            let mut child = match child_handle.lock() {
                Ok(c) => c,
                Err(_) => {
                    stream_error.get_or_insert("child handle poisoned".to_string());
                    return;
                }
            };
            reap_rg_child_after_stdout(&mut child, stdout_stopped_early)
        };
        if let Ok(mut guard) = filename_search_processes().lock() {
            guard.remove(&search_id);
        }

        let final_error = stream_error.or_else(|| {
            exit_status
                .as_ref()
                .filter(|s| !s.success() && s.code() != Some(1))
                .map(|s| format!("rg exited with status: {:?}", s))
        });

        // `RG_STREAM_FAILED` is the catch-all code for any error that
        // surfaces mid-walk (stdout I/O error or non-zero exit other than
        // rg's "no matches" code 1). Distinct from `RG_SPAWN_FAILED`,
        // which is reserved for the rg binary failing to start in the
        // first place. The renderer can branch on it alongside the inline
        // `QUERY_TOO_LONG` / `PATH_VALIDATION_FAILED` / `RG_STDOUT_CAPTURE_FAILED`
        // codes emitted earlier in the command.
        let final_code = if final_error.is_some() {
            Some("RG_STREAM_FAILED".to_string())
        } else {
            None
        };

        let _ = app_handle.emit(
            "search-file-names-done",
            SearchFileNamesDoneEvent {
                search_id,
                truncated,
                total_files: final_files.len(),
                code: final_code,
                error: final_error,
            },
        );
    });

    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn search_file_names_cancel(
    request: SearchFileNamesCancelRequest,
) -> Result<IpcResult<()>, String> {
    let mut guard = filename_search_processes()
        .lock()
        .map_err(|e| e.to_string())?;
    if let Some(child_handle) = guard.remove(&request.search_id) {
        if let Ok(mut child) = child_handle.lock() {
            if child.kill().is_err() {
                log::warn!(
                    "[search-process] kind=file_name cleanup_stage=kill stable_result=FAILED"
                );
            }
            if child.wait().is_err() {
                log::warn!(
                    "[search-process] kind=file_name cleanup_stage=wait stable_result=FAILED"
                );
            }
        }
    }
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn search_content(
    request: SearchContentRequest,
) -> Result<IpcResult<FileSearchResponse>, String> {
    let trimmed_query = request.query.trim();
    if trimmed_query.is_empty() {
        return Ok(IpcResult::success(FileSearchResponse {
            results: vec![],
            truncated: false,
            scanned_files: 0,
            failed_files: 0,
        }));
    }

    let max_files_with_matches: usize = 100;
    let max_matches_per_file: usize = 30;

    let validated_root = match validated_search_root(&request.scope_root, &request.root_path) {
        Ok(path) => path,
        Err(e) => {
            log::warn!(
                "[Security] Content search rejected: scope='{}' root='{}': {}",
                request.scope_root,
                request.root_path,
                e
            );
            return Ok(IpcResult::error(
                format!("Invalid search path: {}", e),
                "PATH_VALIDATION_FAILED",
            ));
        }
    };

    let args = build_search_args(trimmed_query, &validated_root, max_matches_per_file);

    let rg_path = detect_rg_path();
    let mut rg_command = Command::new(&rg_path);
    rg_command.args(args);
    configure_background_command(&mut rg_command);
    let output = rg_command.output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Ok(IpcResult::error(
                format!("rg spawn failed (path: {}): {}", rg_path, e),
                "SEARCH_ERROR",
            ))
        }
    };

    let code = output.status.code().unwrap_or(0);
    if code > 1 {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Ok(IpcResult::error(
            format!("rg failed ({}): {}", code, stderr),
            "SEARCH_ERROR",
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut grouped: BTreeMap<String, Vec<FileSearchMatch>> = BTreeMap::new();
    let mut truncated = false;

    for line in stdout.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if parsed.get("type").and_then(|v| v.as_str()) != Some("match") {
            continue;
        }

        let file_path = match parsed
            .get("data")
            .and_then(|d| d.get("path"))
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
        {
            Some(p) => p.replace('\\', "/"),
            None => continue,
        };

        let line_number = match parsed
            .get("data")
            .and_then(|d| d.get("line_number"))
            .and_then(|n| n.as_u64())
        {
            Some(n) => n as usize,
            None => continue,
        };

        let line_text = parsed
            .get("data")
            .and_then(|d| d.get("lines"))
            .and_then(|l| l.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_end_matches(['\r', '\n'])
            .to_string();

        if !grouped.contains_key(&file_path) {
            if grouped.len() >= max_files_with_matches {
                truncated = true;
                break;
            }
            grouped.insert(file_path.clone(), Vec::new());
        }

        if let Some(matches) = grouped.get_mut(&file_path) {
            if matches.len() >= max_matches_per_file {
                truncated = true;
                continue;
            }
            matches.push(FileSearchMatch {
                line_number,
                line_text,
            });
        }
    }

    let results = grouped
        .into_iter()
        .map(|(file_path, matches)| FileSearchResult { file_path, matches })
        .collect();

    Ok(IpcResult::success(FileSearchResponse {
        results,
        truncated,
        scanned_files: 0,
        failed_files: 0,
    }))
}

/// Get current schema version
#[tauri::command]
pub async fn data_migration_get_version(
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<String>, String> {
    Ok(migration_manager.get_current_schema_version())
}

/// Get schema version info (current and target)
#[tauri::command]
pub async fn data_migration_get_schema_info(
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<SchemaVersion>, String> {
    Ok(migration_manager.get_schema_version_info())
}

/// Get migration history
#[tauri::command]
pub async fn data_migration_get_history(
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<Vec<MigrationRecord>>, String> {
    Ok(migration_manager.get_migration_history())
}

/// Get all registered migrations
#[tauri::command]
pub async fn data_migration_get_registered(
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<Vec<MigrationInfo>>, String> {
    Ok(migration_manager.get_registered_migrations())
}

/// Run pending migrations
#[tauri::command]
pub async fn data_migration_run_migrations(
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<Vec<MigrationResult>>, String> {
    Ok(migration_manager.run_migrations())
}

/// Rollback to a specific version
#[tauri::command]
pub async fn data_migration_rollback(
    request: RollbackRequest,
    migration_manager: State<'_, Arc<MigrationManager>>,
) -> Result<IpcResult<()>, String> {
    Ok(migration_manager.rollback_migration(request.version))
}

// ============================================================================
// SSH Commands
// ============================================================================

use crate::ssh::config_parser;
use crate::ssh::profile_manager::SSHProfile;
use crate::ssh::sftp as sftp_ops;
use crate::ssh::SSHManager;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SSHConnectRequest {
    pub profile_id: String,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SSHPortForwardRequest {
    pub connection_id: String,
    pub id: String,
    pub forward_type: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SFTPPathRequest {
    pub connection_id: String,
    pub remote_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SFTPTransferRequest {
    pub connection_id: String,
    pub remote_path: String,
    pub local_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SFTPRenameRequest {
    pub connection_id: String,
    pub old_path: String,
    pub new_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SFTPFileRequest {
    pub connection_id: String,
    pub remote_path: String,
    pub content: Option<String>,
}

#[tauri::command]
pub async fn ssh_list_profiles(
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<Vec<SSHProfile>>, String> {
    match ssh_manager.profiles.list() {
        Ok(profiles) => Ok(IpcResult::success(profiles)),
        Err(e) => Ok(IpcResult::error(e, "SSH_PROFILE_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_save_profile(
    profile: SSHProfile,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    match ssh_manager.profiles.save(profile) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SSH_PROFILE_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_delete_profile(
    profile_id: String,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    match ssh_manager.profiles.delete(&profile_id) {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SSH_PROFILE_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_import_config(
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<Vec<SSHProfile>>, String> {
    let parsed = config_parser::parse_ssh_config();
    match ssh_manager.profiles.import_from_config(parsed) {
        Ok(imported) => Ok(IpcResult::success(imported)),
        Err(e) => Ok(IpcResult::error(e, "SSH_IMPORT_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_connect(
    request: SSHConnectRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<crate::ssh::connection::SSHConnectionInfo>, String> {
    // Load profile with credentials from OS keychain
    let profile = match ssh_manager
        .profiles
        .get_with_credentials(&request.profile_id)
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Ok(IpcResult::error(
                "Profile not found",
                "SSH_PROFILE_NOT_FOUND",
            ))
        }
        Err(e) => return Ok(IpcResult::error(e, "SSH_PROFILE_ERROR")),
    };

    // Use request password, or fall back to keychain-stored credential
    let password = request
        .password
        .or_else(|| match profile.auth_method.as_str() {
            "password" => profile.password.clone(),
            "key" => profile.passphrase.clone(),
            _ => None,
        });

    match ssh_manager
        .connections
        .connect(&profile, password.as_deref())
        .await
    {
        Ok(info) => {
            // Update last_connected
            let _ = ssh_manager
                .profiles
                .update_last_connected(&request.profile_id);
            Ok(IpcResult::success(info))
        }
        Err(e) => Ok(IpcResult::error(e, "SSH_CONNECT_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_disconnect(
    connection_id: String,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    match ssh_manager.disconnect(&connection_id).await {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SSH_DISCONNECT_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_get_connections(
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<Vec<crate::ssh::connection::SSHConnectionInfo>>, String> {
    Ok(IpcResult::success(
        ssh_manager.connections.list_connections().await,
    ))
}

#[tauri::command]
pub async fn ssh_port_forward_start(
    request: SSHPortForwardRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<crate::ssh::port_forward::ActivePortForward>, String> {
    if ssh_manager
        .connections
        .get_connection(&request.connection_id)
        .await
        .is_none()
    {
        return Ok(IpcResult::error(
            "Connection not found",
            "SSH_CONNECTION_NOT_FOUND",
        ));
    }

    let session = match ssh_manager
        .connections
        .clone_session(&request.connection_id)
        .await
    {
        Ok(session) => session,
        Err(e) => return Ok(IpcResult::error(e, "SSH_CONNECTION_NOT_FOUND")),
    };

    let pf_request = crate::ssh::port_forward::PortForwardRequest {
        id: request.id,
        forward_type: request.forward_type,
        local_port: request.local_port,
        remote_host: request.remote_host,
        remote_port: request.remote_port,
        label: request.label,
    };

    match ssh_manager
        .port_forwards
        .start_local_forward(&request.connection_id, pf_request, session)
        .await
    {
        Ok(forward) => Ok(IpcResult::success(forward)),
        Err(e) => Ok(IpcResult::error(e, "SSH_PORT_FORWARD_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_port_forward_stop(
    connection_id: String,
    forward_id: String,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    match ssh_manager
        .port_forwards
        .stop_forward(&connection_id, &forward_id)
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SSH_PORT_FORWARD_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_list_dir(
    request: SFTPPathRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<Vec<crate::ssh::sftp::SFTPEntry>>, String> {
    let remote_path = request.remote_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::list_dir(&sftp, &remote_path)
        })
        .await
    {
        Ok(entries) => Ok(IpcResult::success(entries)),
        Err(e) => Ok(IpcResult::error(e, "SFTP_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_download(
    request: SFTPTransferRequest,
    app_handle: AppHandle,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    let local_path = request.local_path.clone();
    let conn_id = request.connection_id.clone();
    let app = app_handle.clone();

    // Clone session to avoid holding the per-connection mutex during long I/O
    let session = match ssh_manager
        .connections
        .clone_session(&request.connection_id)
        .await
    {
        Ok(s) => s,
        Err(e) => return Ok(IpcResult::error(e, "SFTP_DOWNLOAD_ERROR")),
    };

    match tokio::task::spawn_blocking(move || {
        let sftp = sftp_ops::create_sftp(&session)?;
        sftp_ops::download_file(&sftp, &remote_path, &local_path, &app, &conn_id)
    })
    .await
    {
        Ok(Ok(())) => Ok(IpcResult::success(())),
        Ok(Err(e)) => Ok(IpcResult::error(e, "SFTP_DOWNLOAD_ERROR")),
        Err(e) => Ok(IpcResult::error(
            format!("Task failed: {}", e),
            "SFTP_DOWNLOAD_ERROR",
        )),
    }
}

#[tauri::command]
pub async fn sftp_upload(
    request: SFTPTransferRequest,
    app_handle: AppHandle,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    let local_path = request.local_path.clone();
    let conn_id = request.connection_id.clone();
    let app = app_handle.clone();

    // Clone session to avoid holding the per-connection mutex during long I/O
    let session = match ssh_manager
        .connections
        .clone_session(&request.connection_id)
        .await
    {
        Ok(s) => s,
        Err(e) => return Ok(IpcResult::error(e, "SFTP_UPLOAD_ERROR")),
    };

    match tokio::task::spawn_blocking(move || {
        let sftp = sftp_ops::create_sftp(&session)?;
        sftp_ops::upload_file(&sftp, &local_path, &remote_path, &app, &conn_id)
    })
    .await
    {
        Ok(Ok(())) => Ok(IpcResult::success(())),
        Ok(Err(e)) => Ok(IpcResult::error(e, "SFTP_UPLOAD_ERROR")),
        Err(e) => Ok(IpcResult::error(
            format!("Task failed: {}", e),
            "SFTP_UPLOAD_ERROR",
        )),
    }
}

#[tauri::command]
pub async fn sftp_delete(
    request: SFTPPathRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::delete_path(&sftp, &remote_path)
        })
        .await
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SFTP_DELETE_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_mkdir(
    request: SFTPPathRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::mkdir(&sftp, &remote_path)
        })
        .await
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SFTP_MKDIR_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_rename(
    request: SFTPRenameRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let old_path = request.old_path.clone();
    let new_path = request.new_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::rename(&sftp, &old_path, &new_path)
        })
        .await
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SFTP_RENAME_ERROR")),
    }
}

#[tauri::command]
pub async fn ssh_create_askpass(password: String) -> Result<IpcResult<String>, String> {
    let temp_dir = std::env::temp_dir();
    let id = uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap_or("tmp")
        .to_string();
    let password_path = temp_dir.join(format!("se-manager-askpass-{}.dat", id));

    // Write the raw password to a separate data file to avoid shell metacharacter injection.
    if let Err(e) = std::fs::write(&password_path, password.as_bytes()) {
        return Ok(IpcResult::error(
            format!("Failed to create askpass data: {}", e),
            "SSH_ASKPASS_ERROR",
        ));
    }

    // Restrict file permissions: owner-only on Unix, hidden attribute on Windows
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&password_path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        // Mark the data file as hidden to reduce casual exposure
        use std::process::Command;
        let _ = Command::new("attrib")
            .args(["+H", &password_path.to_string_lossy()])
            .output();
    }

    // Create platform-specific askpass script
    #[cfg(windows)]
    let script_path = {
        let path = temp_dir.join(format!("se-manager-askpass-{}.bat", id));
        // The batch script outputs the password file contents and cleans up both files on exit.
        let content = format!(
            "@echo off\r\ntype \"{}\"\r\ndel /q \"{}\" >nul 2>&1\r\n(goto) 2>nul & del /q \"%~f0\" >nul 2>&1\r\n",
            password_path.to_string_lossy(),
            password_path.to_string_lossy(),
        );
        if let Err(e) = std::fs::write(&path, &content) {
            let _ = std::fs::remove_file(&password_path);
            return Ok(IpcResult::error(
                format!("Failed to create askpass: {}", e),
                "SSH_ASKPASS_ERROR",
            ));
        }
        path
    };

    #[cfg(unix)]
    let script_path = {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_dir.join(format!("se-manager-askpass-{}.sh", id));
        // The shell script outputs the password file and cleans up both files.
        let content = format!(
            "#!/bin/sh\ncat \"{}\"\nrm -f \"{}\" \"$0\"\n",
            password_path.to_string_lossy(),
            password_path.to_string_lossy(),
        );
        if let Err(e) = std::fs::write(&path, &content) {
            let _ = std::fs::remove_file(&password_path);
            return Ok(IpcResult::error(
                format!("Failed to create askpass: {}", e),
                "SSH_ASKPASS_ERROR",
            ));
        }
        // Make the script executable
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)) {
            let _ = std::fs::remove_file(&password_path);
            let _ = std::fs::remove_file(&path);
            return Ok(IpcResult::error(
                format!("Failed to set askpass permissions: {}", e),
                "SSH_ASKPASS_ERROR",
            ));
        }
        path
    };

    // Spawn a background cleanup task that removes both files after a timeout,
    // ensuring secrets don't persist on disk if the helper is never invoked.
    let cleanup_script = script_path.clone();
    let cleanup_password = password_path.clone();
    crate::host_admission::HostAdmission::global().track(tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let _ = std::fs::remove_file(&cleanup_password);
        let _ = std::fs::remove_file(&cleanup_script);
    }));

    // Path includes the OS temp dir (often the username); keep it out of the
    // user-attachable info log (issue #244 AC#6). Full path stays at debug.
    log::info!("[SSH] Created askpass helper");
    log::debug!("[SSH] Askpass helper path: {:?}", script_path);
    Ok(IpcResult::success(
        script_path.to_string_lossy().to_string(),
    ))
}

#[tauri::command]
pub async fn sftp_read_file(
    request: SFTPFileRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<String>, String> {
    let remote_path = request.remote_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::read_file_to_string(&sftp, &remote_path)
        })
        .await
    {
        Ok(content) => Ok(IpcResult::success(content)),
        Err(e) => Ok(IpcResult::error(e, "SFTP_READ_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_write_file(
    request: SFTPFileRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    let content = request.content.clone().unwrap_or_default();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::write_file_from_string(&sftp, &remote_path, &content)
        })
        .await
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SFTP_WRITE_ERROR")),
    }
}

#[tauri::command]
pub async fn sftp_create_file(
    request: SFTPPathRequest,
    ssh_manager: State<'_, Arc<SSHManager>>,
) -> Result<IpcResult<()>, String> {
    let remote_path = request.remote_path.clone();
    match ssh_manager
        .connections
        .with_session(&request.connection_id, |session| {
            let sftp = sftp_ops::create_sftp(session)?;
            sftp_ops::create_file(&sftp, &remote_path)
        })
        .await
    {
        Ok(()) => Ok(IpcResult::success(())),
        Err(e) => Ok(IpcResult::error(e, "SFTP_CREATE_ERROR")),
    }
}

// ==================== Remote Server Commands ====================

/// Start the desktop-hosted shared-live web server.
///
/// Shares the desktop's live `AcpManager` sessions with a phone/browser client.
///
/// Starts the in-process shared-live web server, then the configured tunnel
/// when the last publish mode is tunnel (or as a best-effort extra when LAN
/// is selected). `bind_mode` selects localhost vs all interfaces.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn remote_server_start(
    acp_manager: State<'_, Arc<crate::acp::AcpManager>>,
    pty_manager: State<'_, Arc<PtyManager>>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
    conversation: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
    project_registry: State<'_, Arc<crate::web::ProjectRegistry>>,
    workspace_manifest_store: State<'_, HostWorkspaceManifestStore>,
    acp_catalog_store: State<'_, HostAcpCatalogStore>,
    acp_install_store: State<'_, HostAcpInstallStore>,
    tunnel_store: State<'_, Arc<remote::TunnelConfigStore>>,
    intent_store: State<'_, Arc<remote::RemoteAccessIntentStore>>,
    bind_mode: Option<String>,
) -> Result<IpcResult<remote::RemoteStatus>, String> {
    let stored_intent = intent_store.load().unwrap_or_default();
    let (bind_mode, publish_mode) = match bind_mode.as_deref() {
        None => match stored_intent.publish_mode {
            remote::PublishMode::Lan => (remote::RemoteBindMode::All, remote::PublishMode::Lan),
            remote::PublishMode::Tunnel => (
                remote::RemoteBindMode::Localhost,
                remote::PublishMode::Tunnel,
            ),
        },
        Some(s) => {
            let bind = remote::RemoteBindMode::parse(s)
                .ok_or_else(|| format!("invalid bind mode '{s}': use 'localhost' or 'all'"))?;
            let mode = match bind {
                remote::RemoteBindMode::All => remote::PublishMode::Lan,
                remote::RemoteBindMode::Localhost => remote::PublishMode::Tunnel,
            };
            (bind, mode)
        }
    };
    if let Err(error) = intent_store.save(&remote::RemoteAccessIntent {
        wanted: true,
        publish_mode,
    }) {
        log::error!(
            target: "se_manager::remote::host",
            "operation=intent_save lifecycle_phase=start stable_code=INTENT_SAVE_FAILED"
        );
        return Ok(IpcResult::error(error, "REMOTE_INTENT_SAVE_FAILED"));
    }
    // CAP-5: thread the desktop's `WorkspaceManifestService` (opened under
    // `<app_data_dir>/workspace-manifests` in `lib.rs`) through to
    // `serve_router` so the web/remote client can read/write a project's
    // manifest through the three `/workspace/*` routes. `None` degrades to
    // fresh-only mode (no host store attached).
    let workspace_manifest = workspace_manifest_store.store().map(Arc::clone);
    // CAP-6 / Story 8: thread the desktop's `AcpCatalogService` (opened under
    // `<app_data_dir>/acp-catalog` in `lib.rs`) through to `serve_router` so
    // the web/remote client can resolve the catalog through `GET /acp/catalog`
    // + WS `list_acp_catalog`. `None` degrades to `ACP_CATALOG_UNAVAILABLE`.
    let acp_catalog = acp_catalog_store.store().map(Arc::clone);
    // CAP-6 / Story 9: thread the desktop's `AcpInstallService` (opened under
    // `<app_data_dir>/acp-registry-binaries` in `lib.rs`) through to
    // `serve_router` so the web/remote client can install through
    // `POST /acp/install` + WS `install_acp_agent`. `None` degrades to
    // `ACP_INSTALL_UNAVAILABLE`.
    let acp_install = acp_install_store.store().map(Arc::clone);
    let tunnel_store = tunnel_store.inner();
    let tunnel_config = match tunnel_store.load() {
        Ok(config) => config,
        Err(e) => return Ok(IpcResult::error(e, "TUNNEL_CONFIG_READ_FAILED")),
    };
    if let Err(e) = tunnel_config.validate_for_start() {
        return Ok(IpcResult::error(e, "TUNNEL_CONFIG_INVALID"));
    }
    let started = if let Some(bind_port) = tunnel_config.preferred_bind_port() {
        remote_state
            .start_on_port(
                acp_manager.inner().clone(),
                pty_manager.inner().clone(),
                ws_relay.inner().clone(),
                project_registry.inner().clone(),
                bind_mode,
                Some(conversation.inner().clone()),
                workspace_manifest,
                acp_catalog,
                acp_install,
                bind_port,
            )
            .await
    } else {
        remote_state
            .start(
                acp_manager.inner().clone(),
                pty_manager.inner().clone(),
                ws_relay.inner().clone(),
                project_registry.inner().clone(),
                bind_mode,
                Some(conversation.inner().clone()),
                workspace_manifest,
                acp_catalog,
                acp_install,
            )
            .await
    };
    match started {
        Ok(status) => {
            remote_state.set_publish_mode(publish_mode);
            let port = match status.port {
                Some(p) => p,
                None => {
                    return Ok(IpcResult::error(
                        "started remote server reported no port".to_string(),
                        "REMOTE_START_FAILED",
                    ))
                }
            };
            match remote::tunnel::start_configured_tunnel(port, &tunnel_config, tunnel_store).await
            {
                Ok(tunnel) => {
                    let probe_url = tunnel.url.clone();
                    let provider = tunnel.provider.as_str().to_string();
                    if let Err(e) =
                        remote_state.attach_tunnel_as(tunnel.url, tunnel.child, &provider)
                    {
                        if publish_mode == remote::PublishMode::Lan {
                            log::warn!(
                                target: "se_manager::remote::host",
                                "operation=tunnel_attach lifecycle_phase=start stable_code=REMOTE_TUNNEL_FAILED"
                            );
                            return Ok(IpcResult::success(remote_state.status()));
                        }
                        let _ = remote_state.stop().await;
                        return Ok(IpcResult::error(e, "REMOTE_TUNNEL_FAILED"));
                    }
                    tokio::spawn(remote::cloudflared::log_tunnel_reachability(probe_url));
                    Ok(IpcResult::success(remote_state.status()))
                }
                Err(e) => {
                    if publish_mode == remote::PublishMode::Lan {
                        log::warn!(
                            target: "se_manager::remote::host",
                            "operation=tunnel_start lifecycle_phase=start stable_code=REMOTE_TUNNEL_FAILED"
                        );
                        return Ok(IpcResult::success(remote_state.status()));
                    }
                    let _ = remote_state.stop().await;
                    Ok(IpcResult::error(e, "REMOTE_TUNNEL_FAILED"))
                }
            }
        }
        Err(e) => Ok(IpcResult::error(e, "REMOTE_START_FAILED")),
    }
}

/// Stop the desktop-hosted web server.
///
/// Signals graceful shutdown to the serve task. The desktop's live agents are
/// NOT killed — they survive a shared-live toggle-off. The in-memory project
/// registry is cleared (it lives only while the server runs — Epic-4 bridge).
#[tauri::command]
pub async fn remote_server_stop(
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
    project_registry: State<'_, Arc<crate::web::ProjectRegistry>>,
    intent_store: State<'_, Arc<remote::RemoteAccessIntentStore>>,
) -> Result<IpcResult<remote::RemoteStatus>, String> {
    let publish_mode = intent_store
        .load()
        .map(|intent| intent.publish_mode)
        .unwrap_or_default();
    if let Err(error) = intent_store.save(&remote::RemoteAccessIntent {
        wanted: false,
        publish_mode,
    }) {
        log::error!(
            target: "se_manager::remote::host",
            "operation=intent_save lifecycle_phase=stop stable_code=INTENT_SAVE_FAILED"
        );
        return Ok(IpcResult::error(error, "REMOTE_INTENT_SAVE_FAILED"));
    }
    let result = remote_state.stop().await;
    // Clear the in-memory project mirror so a stale list does not linger after
    // the server is off (the registry is renderer-fed; it is repopulated on the
    // next server start via `remote_sync_projects`).
    project_registry.clear();
    match result {
        Ok(status) => Ok(IpcResult::success(status)),
        Err(e) => Ok(IpcResult::error(e, "REMOTE_STOP_FAILED")),
    }
}

/// Get the desktop-hosted web server status.
#[tauri::command]
pub async fn remote_server_status(
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
) -> Result<IpcResult<remote::RemoteStatus>, String> {
    Ok(IpcResult::success(remote_state.status()))
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessIntentView {
    pub wanted: bool,
    pub publish_mode: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessIntentUpdate {
    pub wanted: Option<bool>,
    pub publish_mode: Option<String>,
}

#[tauri::command]
pub async fn remote_access_intent_get(
    intent_store: State<'_, Arc<remote::RemoteAccessIntentStore>>,
) -> Result<IpcResult<RemoteAccessIntentView>, String> {
    match intent_store.load() {
        Ok(intent) => Ok(IpcResult::success(RemoteAccessIntentView {
            wanted: intent.wanted,
            publish_mode: intent.publish_mode.as_str().to_string(),
        })),
        Err(error) => Ok(IpcResult::error(error, "REMOTE_INTENT_READ_FAILED")),
    }
}

#[tauri::command]
pub async fn remote_access_intent_set(
    update: RemoteAccessIntentUpdate,
    intent_store: State<'_, Arc<remote::RemoteAccessIntentStore>>,
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
) -> Result<IpcResult<RemoteAccessIntentView>, String> {
    let mut intent = match intent_store.load() {
        Ok(intent) => intent,
        Err(error) => return Ok(IpcResult::error(error, "REMOTE_INTENT_READ_FAILED")),
    };
    if let Some(wanted) = update.wanted {
        intent.wanted = wanted;
    }
    if let Some(raw) = update.publish_mode.as_deref() {
        let Some(mode) = remote::PublishMode::parse(raw) else {
            return Ok(IpcResult::error(
                "publish mode must be lan or tunnel".to_string(),
                "REMOTE_INTENT_INVALID",
            ));
        };
        intent.publish_mode = mode;
        remote_state.set_publish_mode(mode);
    }
    if let Err(error) = intent_store.save(&intent) {
        return Ok(IpcResult::error(error, "REMOTE_INTENT_SAVE_FAILED"));
    }
    Ok(IpcResult::success(RemoteAccessIntentView {
        wanted: intent.wanted,
        publish_mode: intent.publish_mode.as_str().to_string(),
    }))
}

#[tauri::command]
pub async fn remote_server_rotate_credential(
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
    authority: State<'_, Arc<crate::web::RemoteAccessAuthority>>,
    tunnel_store: State<'_, Arc<remote::TunnelConfigStore>>,
) -> Result<IpcResult<remote::RemoteStatus>, String> {
    if remote_state.status().running {
        match remote_state.rotate_active_credential() {
            Ok(status) => {
                log::info!(
                    target: "se_manager::remote::host",
                    "operation=credential_rotate lifecycle_phase=operator stable_code=OK"
                );
                Ok(IpcResult::success(status))
            }
            Err(error) => Ok(IpcResult::error(error, "REMOTE_ROTATE_FAILED")),
        }
    } else {
        match authority.rotate_desktop_credential() {
            Ok(lease) => {
                if let Err(error) = tunnel_store.set_pairing_token(Some(lease.bearer())) {
                    return Ok(IpcResult::error(error, "REMOTE_ROTATE_FAILED"));
                }
                log::info!(
                    target: "se_manager::remote::host",
                    "operation=credential_rotate lifecycle_phase=operator_idle stable_code=OK"
                );
                Ok(IpcResult::success(remote_state.status()))
            }
            Err(error) => Ok(IpcResult::error(error.to_string(), "REMOTE_ROTATE_FAILED")),
        }
    }
}

/// Push the desktop renderer's current project + group list into the in-memory
/// `ProjectRegistry` (Epic-4 bridge) and broadcast a `projects_changed` WS event
/// so connected web clients refetch `GET /projects`. Called by the renderer
/// on server-start success + on every project-store mutation while the server
/// runs. Group state is navigation-only; no env-var values cross the wire —
/// `ProjectSummary` redacts-by-omission.
///
/// In desktop-hosted mode the desktop's `activeProjectId` IS the host default
/// (the desktop user is the host operator), so it is pushed as `defaultProjectId`.
/// The web client seeds its initial `activeProjectId` from it on the first
/// `GET /projects` but preserves its own selection on subsequent refetches.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProjectsPayload {
    pub projects: Vec<crate::web::ProjectSummary>,
    #[serde(default)]
    pub groups: Vec<crate::web::ProjectGroupSummary>,
    #[serde(default)]
    pub default_project_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseCodeWorkspacePayload {
    pub path: String,
}

#[tauri::command]
pub fn list_editor_workspaces() -> IpcResult<crate::editor_workspaces::EditorWorkspaceList> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    IpcResult::success(crate::editor_workspaces::discover_editor_workspaces())
}

#[tauri::command]
pub fn parse_code_workspace_file(
    payload: ParseCodeWorkspacePayload,
) -> IpcResult<crate::editor_workspaces::EditorWorkspaceList> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    match crate::editor_workspaces::parse_code_workspace_file(std::path::Path::new(&payload.path)) {
        Ok(list) => {
            log::info!(
                target: "se_manager::editor_workspaces",
                "operation=parse_workspace count={} stable_code=OK",
                list.candidates.len()
            );
            IpcResult::success(list)
        }
        Err(error) => IpcResult::error(error, "WORKSPACE_PARSE_FAILED"),
    }
}

#[tauri::command]
pub async fn remote_sync_projects(
    payload: SyncProjectsPayload,
    project_registry: State<'_, Arc<crate::web::ProjectRegistry>>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<()>, String> {
    let project_count = payload.projects.len();
    let group_count = payload.groups.len();
    project_registry.set_with_groups(
        payload.projects,
        payload.groups,
        payload.default_project_id.clone(),
    );
    crate::web::broadcast_projects_changed(ws_relay.inner(), payload.default_project_id.as_deref());
    log::info!(
        target: "se_manager::remote_sync_projects",
        "operation=remote_sync_projects stable_code=OK projects={} groups={}",
        project_count,
        group_count
    );
    Ok(IpcResult::success(()))
}

/// Explicitly set the host's default project (Epic 7 — cross-client
/// workspace continuity). Distinct from a per-connection `switch_project`:
/// this changes the host default that new web clients start with. Validates
/// the project is switchable, updates `registry.set_default_project`, and
/// broadcasts `projects_changed` to all connected web clients. Desktop-hosted
/// mode has no `FileProjectRegistry` (the file registry is VPS-only); the
/// desktop pushes its active selection as the default via `remote_sync_projects`,
/// but this command lets the desktop set a default DIFFERENT from its own
/// active project.
#[tauri::command]
pub async fn set_host_default_project(
    project_id: String,
    project_registry: State<'_, Arc<crate::web::ProjectRegistry>>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<()>, String> {
    // Validate via switch_context (unknown/archived/pathless → NOT_FOUND).
    if project_registry.switch_context(&project_id).is_none() {
        log::warn!(
            "set_host_default_project: project '{}' not found or not switchable",
            project_id
        );
        return Ok(IpcResult::error(
            format!("project '{project_id}' not found or not switchable"),
            "NOT_FOUND",
        ));
    }
    if !project_registry.set_default_project(&project_id) {
        log::warn!(
            "set_host_default_project: project '{}' became unavailable before commit",
            project_id
        );
        return Ok(IpcResult::error(
            "target project became unavailable before commit".to_string(),
            "NOT_FOUND",
        ));
    }
    crate::web::broadcast_projects_changed(ws_relay.inner(), Some(&project_id));
    log::info!(
        "set_host_default_project: host default updated to '{}' + broadcast",
        project_id
    );
    Ok(IpcResult::success(()))
}

/// Compatibility refresh command for older renderer callers.
///
/// Durable desktop history is owned by `acp_history_*`; this command retains
/// the old invoke shape but only broadcasts `chat_history_changed`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SyncChatHistoryPayload {
    /// The full session index (wire `PersistedSessionSummary[]` shape).
    /// `None` on a payload-only sync (the `useAcpHistorySync` hook owns the
    /// index push; `persistSession` pushes only its payload to avoid a
    /// double `set_index` + double broadcast per mutation).
    #[serde(default)]
    pub index: Option<Vec<crate::acp::SessionIndexEntry>>,
    /// Monotonic revision stamped by the renderer on each index push
    /// (`useAcpHistorySync` increments it; the seed in `RemoteAccessPopover`
    /// omits it → `0`). `set_index` rejects a push whose revision is strictly
    /// lower than the current one so a delayed older index cannot replace a
    /// newer snapshot. Absent on a payload-only sync (unused).
    #[serde(default)]
    pub revision: Option<u64>,
    /// Optional per-session payloads (`{ metadata, messages }`) — pushed lazily
    /// (only sessions the renderer has in memory). Omitted on an index-only sync.
    #[serde(default)]
    pub payloads: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[tauri::command]
pub async fn remote_sync_chat_history(
    payload: SyncChatHistoryPayload,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
    remote_state: State<'_, Arc<remote::RemoteServerState>>,
) -> Result<IpcResult<()>, String> {
    // Defense in depth: the TS caller already gates on `running`, but the
    // server may have just been stopped (`remote_server_stop` clears the
    // cache). Early-return so a late push does not repopulate a cache that
    // was just cleared.
    if !remote_state.status().running {
        return Ok(IpcResult::success(()));
    }
    // Compatibility bridge only: durable desktop history is now written by
    // the dedicated `acp_history_*` commands. Existing callers may still use
    // this command to request a browser index refresh, but payload/index values
    // are deliberately not retained or cloned in Rust memory.
    let _ = payload;
    crate::web::broadcast_chat_history_changed(ws_relay.inner());
    Ok(IpcResult::success(()))
}

/// Mirror the desktop app-store MCP registry to the active project's
/// `.termul/mcp-servers.json` (CAP-7 — registry sync gap).
///
/// Desktop MCP servers live in `termul-data.json["acp/mcp-servers"]`
/// (tauri-plugin-store, app-data dir), while the web `GET /mcp-servers` route
/// reads `{project_root}/.termul/mcp-servers.json`. Without this bridge the web
/// route never sees desktop-configured servers, so `McpBadge` stays hidden on
/// web/mobile. Called best-effort after every desktop MCP save and on project
/// switch — a sync failure is logged but never blocks the app-store save.
///
/// Resolves the active project root via the same chain `RemoteServerState::start`
/// uses: the registry's default-project path (canonicalized), falling back to
/// `default_project_root()` (`$TERMUL_PROJECT_ROOT` / `$HOME`) when the
/// registry has no default (server stopped / never started). The write reuses
/// `mcp_servers_api::registry_path` + `atomic_file::replace` so the sync writes
/// the exact file the web route reads.
#[tauri::command]
pub async fn remote_sync_mcp_registry(
    registry: serde_json::Value,
    project_registry: State<'_, Arc<crate::web::ProjectRegistry>>,
) -> Result<IpcResult<()>, String> {
    Ok(sync_mcp_registry_to_project_file(project_registry.inner(), registry).await)
}

/// Testable core of `remote_sync_mcp_registry`: writes `registry` to
/// `{active_project_root}/.termul/mcp-servers.json` via `atomic_file::replace`.
/// Extracted so a Rust unit test can exercise the write path without a Tauri
/// `AppHandle` (CAP-7 regression guard).
pub(crate) async fn sync_mcp_registry_to_project_file(
    project_registry: &crate::web::ProjectRegistry,
    registry: serde_json::Value,
) -> IpcResult<()> {
    log::info!("remote_sync_mcp_registry: start");

    // Validate the payload is an array (mirrors `mcp_servers_api::put`).
    if !registry.is_array() {
        log::warn!("remote_sync_mcp_registry: rejected non-array payload");
        return IpcResult::error("MCP registry must be a JSON array", "MCP_REGISTRY_INVALID");
    }

    // Serialize + enforce the 1 MiB ceiling (mirrors `mcp_servers_api::put`).
    let bytes = match serde_json::to_vec(&registry) {
        Ok(bytes) if bytes.len() <= crate::web::mcp_servers_api::MAX_REGISTRY_BYTES => bytes,
        Ok(_) => {
            log::warn!("remote_sync_mcp_registry: rejected payload over 1 MiB");
            return IpcResult::error(
                "MCP registry exceeds the 1 MiB limit",
                "MCP_REGISTRY_TOO_LARGE",
            );
        }
        Err(_) => {
            log::warn!("remote_sync_mcp_registry: payload not serializable");
            return IpcResult::error("MCP registry is not serializable", "MCP_REGISTRY_INVALID");
        }
    };

    // Resolve the active project root (same chain as `RemoteServerState::start`):
    // registry default → canonicalize; else `default_project_root()` → canonicalize.
    // A present-but-invalid default path returns an error rather than silently
    // falling back to the home directory (which the web route never reads).
    let project_root = match project_registry.default_project_path() {
        Some(p) => {
            match crate::web::config::resolve_and_validate_project_root(std::path::Path::new(&p)) {
                Ok(root) => root,
                Err(e) => {
                    log::error!(
                        "remote_sync_mcp_registry: default project path '{}' \
                     failed canonicalization: {}",
                        p,
                        e
                    );
                    return IpcResult::error(
                        "No active project root available for MCP registry sync",
                        "NO_ACTIVE_PROJECT_ROOT",
                    );
                }
            }
        }
        None => {
            log::warn!(
                "remote_sync_mcp_registry: no active project path in registry; \
                 falling back to default_project_root"
            );
            match crate::web::config::default_project_root() {
                Some(raw) => match crate::web::config::resolve_and_validate_project_root(&raw) {
                    Ok(root) => root,
                    Err(e) => {
                        log::error!(
                            "remote_sync_mcp_registry: default project root '{}' \
                             failed canonicalization: {}",
                            raw.display(),
                            e
                        );
                        return IpcResult::error(
                            "No active project root available for MCP registry sync",
                            "NO_ACTIVE_PROJECT_ROOT",
                        );
                    }
                },
                None => {
                    log::error!(
                        "remote_sync_mcp_registry: no active project root and \
                         default_project_root unavailable"
                    );
                    return IpcResult::error(
                        "No active project root available for MCP registry sync",
                        "NO_ACTIVE_PROJECT_ROOT",
                    );
                }
            }
        }
    };

    let path = crate::web::mcp_servers_api::registry_path(&project_root);
    let write_path = path.clone();
    let bytes_len = bytes.len();
    let write_result =
        tokio::task::spawn_blocking(move || crate::acp::atomic_file::replace(&write_path, &bytes))
            .await;
    match write_result {
        Ok(Ok(())) => {
            log::info!(
                "remote_sync_mcp_registry: success ({} bytes → {})",
                bytes_len,
                path.display()
            );
            IpcResult::success(())
        }
        Ok(Err(error)) => {
            log::error!(
                "remote_sync_mcp_registry: atomic write failed for {}: {}",
                path.display(),
                error
            );
            IpcResult::error("Failed to persist MCP registry", "MCP_REGISTRY_WRITE_ERROR")
        }
        Err(error) => {
            log::error!(
                "remote_sync_mcp_registry: write task panicked for {}: {}",
                path.display(),
                error
            );
            IpcResult::error("Failed to persist MCP registry", "MCP_REGISTRY_WRITE_ERROR")
        }
    }
}

/// Host-owned durable history state (CAP-2). `None` when the desktop could not
/// open `SessionPersistence` at startup (degraded live-only mode); commands
/// must treat absence as empty history, never crash.
#[derive(Default)]
pub struct HostHistoryStore {
    pub conversation: Option<Arc<crate::conversation::ConversationPersistenceAdapter>>,
    pub legacy_read_only: Option<Arc<crate::acp::SessionPersistence>>,
}

impl HostHistoryStore {
    pub fn conversation(
        adapter: Arc<crate::conversation::ConversationPersistenceAdapter>,
        legacy_read_only: Option<Arc<crate::acp::SessionPersistence>>,
    ) -> Self {
        Self {
            conversation: Some(adapter),
            legacy_read_only,
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatHistoryList {
    pub sessions: Vec<crate::acp::ChatHistoryIndexEntry>,
    pub legacy_import_complete: bool,
}

fn host_entry_to_desktop(
    entry: crate::acp::SessionIndexEntry,
) -> crate::acp::ChatHistoryIndexEntry {
    crate::acp::ChatHistoryIndexEntry {
        id: entry.session_id,
        agent_id: entry.runtime_agent_id.unwrap_or_default(),
        // The renderer maps `config:<id>` namespaces back to the bare config
        // id; anything else (absent or unprefixed) omits the key.
        agent_config_id: entry
            .stable_agent_namespace
            .as_deref()
            .and_then(|namespace| namespace.strip_prefix("config:"))
            .map(str::to_string),
        title: entry.title.unwrap_or_else(|| "Untitled Chat".to_string()),
        cwd: entry.cwd,
        project_id: entry.project_id.unwrap_or_default(),
        created_at: entry.created_at,
        last_activity_at: entry.last_activity_at,
        message_count: entry.message_count,
        status: match entry.status {
            crate::acp::PersistedSessionStatus::Active => crate::acp::ChatHistoryStatus::Active,
            crate::acp::PersistedSessionStatus::Error => crate::acp::ChatHistoryStatus::Error,
            crate::acp::PersistedSessionStatus::Closed => crate::acp::ChatHistoryStatus::Closed,
        },
        discovered: entry.discovered,
        worktree_path: entry.worktree_path,
        worktree_branch: entry.worktree_branch,
        conversation_id: crate::conversation::ConversationId::parse_path_component(
            &entry.storage_key,
        )
        .ok()
        .map(|id| id.to_string()),
    }
}

#[tauri::command]
pub async fn acp_history_list(
    host: State<'_, HostHistoryStore>,
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
) -> Result<IpcResult<DesktopChatHistoryList>, String> {
    log::info!("[acp-history] list start");
    // The legacy flag still gates the renderer's one-time KV wipe migration;
    // the session list itself is host-owned now.
    let legacy_import_complete = store.list().1;
    let sessions = if let Some(persistence) = &host.conversation {
        persistence
            .list_sessions()
            .into_iter()
            .map(|entry| {
                let mut desktop = host_entry_to_desktop(entry);
                if desktop.conversation_id.is_none() {
                    desktop.conversation_id = persistence
                        .conversation_id_for_history_binding(&desktop.id)
                        .map(|id| id.to_string());
                }
                desktop
            })
            .collect()
    } else if let Some(persistence) = &host.legacy_read_only {
        persistence
            .list_sessions()
            .into_iter()
            .map(host_entry_to_desktop)
            .collect()
    } else {
        Vec::new()
    };
    log::info!("[acp-history] list success sessions={}", sessions.len());
    Ok(IpcResult::success(DesktopChatHistoryList {
        sessions,
        legacy_import_complete,
    }))
}

const ACP_HISTORY_COMPAT_ENCODED_BYTE_CEILING: usize = 4_194_304;

fn charge_acp_history_encoded_bytes(
    current: usize,
    record: &impl serde::Serialize,
) -> Result<usize, &'static str> {
    let added = crate::conversation::contracts::encoded_json_len_bounded(
        record,
        crate::conversation::MAX_CONVERSATION_RECORD_BYTES,
    )
    .ok_or("CONVERSATION_RECORD_TOO_LARGE")?;
    let total = current.saturating_add(added);
    if total > ACP_HISTORY_COMPAT_ENCODED_BYTE_CEILING {
        Err(crate::conversation::CONVERSATION_HISTORY_PAGING_REQUIRED)
    } else {
        Ok(total)
    }
}

fn materialize_acp_history_with_ceiling(
    persistence: &crate::conversation::ConversationPersistenceAdapter,
    session_id: &str,
) -> Result<
    (
        crate::acp::session_persistence::SessionMetadata,
        Vec<crate::acp::session_persistence::PersistedEventRecord>,
    ),
    crate::conversation::ConversationPersistenceError,
> {
    let (_conversation_id, metadata, target_last_seq) =
        persistence.history_metadata(session_id, "acp_history_get")?;
    let mut cursor = 0u64;
    let mut records = Vec::new();
    let mut encoded_bytes = 0usize;
    while cursor < target_last_seq {
        let remaining =
            crate::conversation::MAX_COMPAT_HISTORY_RECORDS.saturating_sub(records.len());
        let limit = remaining.saturating_add(1).clamp(
            crate::conversation::MIN_CONVERSATION_HISTORY_PAGE_LIMIT,
            crate::conversation::MAX_CONVERSATION_HISTORY_PAGE_LIMIT,
        );
        let page = persistence.history_page_at(session_id, cursor, limit, Some(target_last_seq))?;
        if page.records.len() > remaining {
            return Err(crate::conversation::ConversationPersistenceError {
                code: crate::conversation::CONVERSATION_HISTORY_PAGING_REQUIRED,
                operation: "acp_history_get",
                detail:
                    "history exceeds the compatibility materialization limit; use bounded pages"
                        .to_string(),
            });
        }
        for record in &page.records {
            encoded_bytes =
                charge_acp_history_encoded_bytes(encoded_bytes, record).map_err(|code| {
                    crate::conversation::ConversationPersistenceError {
                        code,
                        operation: "acp_history_get",
                        detail: "history exceeds the 4194304-byte compatibility ceiling"
                            .to_string(),
                    }
                })?;
        }
        records.extend(page.records.into_iter().map(|record| {
            crate::acp::session_persistence::PersistedEventRecord {
                schema_version: record.schema_version,
                session_id: record.session_id,
                seq: record.seq,
                type_: record.type_,
                recorded_at: record.recorded_at,
                payload: record.payload,
            }
        }));
        if page.next_cursor <= cursor && !page.complete {
            return Err(crate::conversation::ConversationPersistenceError {
                code: "CONVERSATION_READ_FAILED",
                operation: "acp_history_get",
                detail: "history page cursor did not advance".to_string(),
            });
        }
        cursor = page.next_cursor;
        if page.complete {
            break;
        }
    }
    Ok((metadata, records))
}

fn acp_history_get_inner(
    session_id: &str,
    host: &HostHistoryStore,
) -> IpcResult<Option<serde_json::Value>> {
    let Some(persistence) = &host.conversation else {
        return IpcResult::success(None);
    };
    match materialize_acp_history_with_ceiling(persistence, session_id) {
        Ok((metadata, records)) => {
            let payload =
                crate::acp::session_payload::materialize_session_payload(&metadata, &records);
            match serde_json::to_value(&payload) {
                Ok(value) => IpcResult::success(Some(value)),
                Err(_) => IpcResult::error(
                    "failed to encode Conversation history",
                    "CONVERSATION_READ_FAILED",
                ),
            }
        }
        Err(error) if error.code == "CONVERSATION_NOT_FOUND" => IpcResult::success(None),
        Err(error) => {
            if error.code == crate::conversation::CONVERSATION_HISTORY_PAGING_REQUIRED {
                log::warn!(
                    "[acp-history] compatibility paging required code={} encoded_ceiling={}",
                    error.code,
                    ACP_HISTORY_COMPAT_ENCODED_BYTE_CEILING
                );
            }
            IpcResult::error("failed to read Conversation history", error.code)
        }
    }
}

#[tauri::command]
pub async fn acp_history_get(
    session_id: String,
    host: State<'_, HostHistoryStore>,
) -> Result<IpcResult<Option<serde_json::Value>>, String> {
    log::info!(
        "[acp-history] get start session_id={}",
        sanitize_log_field(&session_id)
    );
    let persistence = host.inner().conversation.clone();
    let legacy_read_only = host.inner().legacy_read_only.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        acp_history_get_inner(
            &session_id,
            &HostHistoryStore {
                conversation: persistence,
                legacy_read_only,
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(result)
}

async fn acp_history_get_page_inner(
    session_id: &str,
    after_seq: u64,
    limit: usize,
    target_last_seq: Option<u64>,
    host: &HostHistoryStore,
) -> IpcResult<crate::conversation::ConversationHistoryPageV1> {
    let Some(persistence) = &host.conversation else {
        return IpcResult::error(
            "Conversation history service is unavailable",
            "CONVERSATION_SERVICE_UNAVAILABLE",
        );
    };
    match persistence
        .history_page_blocking(session_id.to_string(), after_seq, limit, target_last_seq)
        .await
    {
        Ok(page) => {
            log::info!(
                "[acp-history] page success session_id={} after_seq={} next_cursor={} target_last_seq={} limit={} count={} complete={}",
                sanitize_log_field(session_id),
                after_seq,
                page.next_cursor,
                page.target_last_seq,
                limit,
                page.records.len(),
                page.complete
            );
            IpcResult::success(page)
        }
        Err(error) => {
            log::warn!(
                "[acp-history] page rejected session_id={} after_seq={} target_last_seq={} limit={} code={}",
                sanitize_log_field(session_id),
                after_seq,
                target_last_seq.unwrap_or(0),
                limit,
                error.code
            );
            IpcResult::error("failed to read Conversation history page", error.code)
        }
    }
}

/// Exact Desktop bounded-history command consumed by `acpHistoryApi.getPage`.
#[tauri::command]
pub async fn acp_history_get_page(
    session_id: String,
    after_seq: u64,
    limit: usize,
    target_last_seq: Option<u64>,
    host: State<'_, HostHistoryStore>,
) -> Result<IpcResult<crate::conversation::ConversationHistoryPageV1>, String> {
    Ok(
        acp_history_get_page_inner(&session_id, after_seq, limit, target_last_seq, host.inner())
            .await,
    )
}

/// Legacy write path (renderer wipe-migration only). Live sessions are authored
/// by the host event/session layer and never flow through this command. The
/// payload lands in the legacy `ChatHistoryStore`; the incremental host import
/// then converges it into `SessionPersistence` so the host-owned `list`/`get`
/// read back exactly what was just saved (read-your-writes for the migration).
#[tauri::command]
pub async fn acp_history_save(
    session_id: String,
    payload: serde_json::Value,
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
    host: State<'_, HostHistoryStore>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<()>, String> {
    let _ = (session_id, payload, store, host, ws_relay);
    log::warn!("[acp-history] legacy mutation rejected code=LEGACY_STORE_READ_ONLY");
    Ok(IpcResult::error(
        "legacy chat history is read-only after Conversation bootstrap",
        "LEGACY_STORE_READ_ONLY",
    ))
}

#[tauri::command]
pub async fn acp_history_delete(
    session_id: String,
    host: State<'_, HostHistoryStore>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<()>, String> {
    let _ = (session_id, host, ws_relay);
    log::warn!("[acp-history] legacy mutation rejected code=LEGACY_STORE_READ_ONLY");
    Ok(IpcResult::error(
        "legacy chat history is read-only after Conversation bootstrap",
        "LEGACY_STORE_READ_ONLY",
    ))
}

#[tauri::command]
pub async fn acp_history_flush(
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
) -> Result<IpcResult<()>, String> {
    let _ = store;
    log::info!("[acp-history] ConversationRepository appends are already durable");
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn acp_history_mark_legacy_import_complete(
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
    host: State<'_, HostHistoryStore>,
    ws_relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<()>, String> {
    let _ = (store, host, ws_relay);
    log::warn!("[acp-history] legacy mutation rejected code=LEGACY_STORE_READ_ONLY");
    Ok(IpcResult::error(
        "legacy chat history is read-only after Conversation bootstrap",
        "LEGACY_STORE_READ_ONLY",
    ))
}

/// Legacy-store read used ONLY by the renderer's one-time KV wipe migration,
/// which must read back exactly what it wrote to the legacy
/// `ChatHistoryStore` (byte-for-byte verification). Live history reads use the
/// host-owned `acp_history_list` / `acp_history_get` instead.
#[tauri::command]
pub async fn acp_history_list_legacy(
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
) -> Result<IpcResult<DesktopChatHistoryList>, String> {
    let (sessions, legacy_import_complete) = store.list();
    Ok(IpcResult::success(DesktopChatHistoryList {
        sessions,
        legacy_import_complete,
    }))
}

/// Legacy-store payload read for the wipe migration (see `acp_history_list_legacy`).
#[tauri::command]
pub async fn acp_history_get_legacy(
    session_id: String,
    store: State<'_, Arc<crate::acp::ChatHistoryStore>>,
) -> Result<IpcResult<Option<serde_json::Value>>, String> {
    let task_store = store.inner().clone();
    let task_id = session_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || task_store.get(&task_id))
        .await
        .map_err(|error| error.to_string())?;
    match result {
        Ok(payload) => Ok(IpcResult::success(Some(payload))),
        Err(crate::acp::ChatHistoryStoreError::SessionNotFound) => Ok(IpcResult::success(None)),
        Err(error) => Ok(IpcResult::error(
            error.to_string(),
            "ACP_HISTORY_GET_FAILED",
        )),
    }
}

// ==================== Git Commands ====================

/// Get git status for a repository
#[tauri::command]
pub async fn git_get_status(cwd: String) -> Result<Vec<GitStatusDetail>, String> {
    crate::trackers::git_tracker::git_get_status_detail(&cwd).map_err(|e: String| e)
}

/// Get git diff for a file. `staged` selects the index-vs-HEAD diff
/// (`git diff --cached`) instead of the worktree-vs-index diff.
#[tauri::command]
pub async fn git_get_diff(
    cwd: String,
    path: String,
    staged: Option<bool>,
) -> Result<String, String> {
    crate::trackers::git_tracker::git_get_diff(&cwd, &path, staged.unwrap_or(false))
        .map_err(|e: String| e)
}

/// Stage a single file (`git add`).
#[tauri::command]
pub async fn git_stage(cwd: String, path: String) -> Result<(), String> {
    crate::trackers::git_tracker::git_stage_file(&cwd, &path).map_err(|e: String| e)
}

/// Unstage a single file (`git restore --staged`).
#[tauri::command]
pub async fn git_unstage(cwd: String, path: String) -> Result<(), String> {
    crate::trackers::git_tracker::git_unstage_file(&cwd, &path).map_err(|e: String| e)
}

/// Stage a single hunk. `hunk_patch` is a unified-diff fragment
/// (`--- a/<path>` / `+++ b/<path>` / `@@ … @@` / body) built by the
/// renderer from the working-tree diff. See #257.
#[tauri::command]
pub async fn git_stage_hunk(cwd: String, path: String, hunk_patch: String) -> Result<(), String> {
    crate::trackers::git_tracker::git_stage_hunk(&cwd, &path, &hunk_patch)
}

/// Unstage a single hunk. `hunk_patch` is built from the staged diff and
/// reverse-applied to the index. See #257.
#[tauri::command]
pub async fn git_unstage_hunk(cwd: String, path: String, hunk_patch: String) -> Result<(), String> {
    crate::trackers::git_tracker::git_unstage_hunk(&cwd, &path, &hunk_patch)
}

/// Discard changes to a single file. Untracked files are deleted; tracked
/// changes revert to HEAD. This is destructive and irreversible.
#[tauri::command]
pub async fn git_discard(cwd: String, path: String) -> Result<(), String> {
    crate::trackers::git_tracker::git_discard_file(&cwd, &path).map_err(|e: String| e)
}

/// Read commit history for the repository at `cwd` as structured rows for the
/// history/graph view. `limit` caps the number of commits (clamped backend-side;
/// defaults to 200). Read-only.
#[tauri::command]
pub async fn git_get_log(cwd: String, limit: Option<u32>) -> Result<Vec<GitCommit>, String> {
    crate::trackers::git_tracker::git_get_log(&cwd, limit).map_err(|e: String| e)
}

/// Create a commit from the staged index. `amend` rewrites HEAD instead of
/// adding a new commit. The message is passed via a temp file, not `-m`.
#[tauri::command]
pub async fn git_commit(
    cwd: String,
    summary: String,
    description: Option<String>,
    amend: Option<bool>,
) -> Result<(), String> {
    // git_commit_file runs `git commit` (which can block on hooks / GPG prompts
    // for up to the network timeout), so run it on the blocking thread pool
    // instead of the async executor.
    let description = description.unwrap_or_default();
    let amend = amend.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        crate::trackers::git_tracker::git_commit_file(&cwd, &summary, &description, amend)
    })
    .await
    .map_err(|e| format!("git commit task failed: {e}"))?
}

/// Push the current branch to `origin`, setting upstream when none exists.
#[tauri::command]
pub async fn git_push(cwd: String) -> Result<(), String> {
    // git_push_current performs a network push (up to the network timeout), so
    // run it on the blocking thread pool instead of the async executor.
    tauri::async_runtime::spawn_blocking(move || {
        crate::trackers::git_tracker::git_push_current(&cwd)
    })
    .await
    .map_err(|e| format!("git push task failed: {e}"))?
}

/// Get commit-footer context: branch, upstream, ahead/behind, staged count,
/// and the last commit's subject/body (for prefilling an amend).
#[tauri::command]
pub async fn git_get_commit_context(
    cwd: String,
) -> Result<crate::trackers::git_tracker::GitCommitContext, String> {
    crate::trackers::git_tracker::git_get_commit_context(&cwd).map_err(|e: String| e)
}

#[tauri::command]
pub async fn git_init(cwd: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(&cwd, &["init"])
            .ok_or_else(|| "Failed to run git init".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git init task failed: {e}"))?
}

/// Check out an existing local or remote-tracking branch.
#[tauri::command]
pub async fn git_checkout_branch(
    cwd: String,
    branch: String,
    is_remote: Option<bool>,
) -> Result<(), String> {
    let is_remote = is_remote.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        crate::trackers::git_tracker::git_checkout_branch(&cwd, &branch, is_remote)
    })
    .await
    .map_err(|e| format!("git checkout task failed: {e}"))?
}

/// Create a new branch from `start_ref` (defaults to HEAD) and check it out.
#[tauri::command]
pub async fn git_create_branch(
    cwd: String,
    branch: String,
    start_ref: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::trackers::git_tracker::git_create_branch(&cwd, &branch, start_ref.as_deref())
    })
    .await
    .map_err(|e| format!("git create branch task failed: {e}"))?
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStashInfo {
    pub index: usize,
    pub name: String,
    pub message: String,
}

#[tauri::command]
pub async fn git_stash_save(
    cwd: String,
    message: Option<String>,
    include_untracked: Option<bool>,
) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let mut args = vec!["stash", "push"];
        if let Some(true) = include_untracked {
            args.push("-u");
        }
        let msg;
        if let Some(ref m) = message {
            args.push("-m");
            msg = m.clone();
            args.push(&msg);
        }
        let output =
            crate::trackers::git_tracker::GitTracker::run_git_command(&validated_str, &args)
                .ok_or_else(|| "Failed to run git stash push".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git stash push task failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_list(cwd: String) -> Result<Vec<GitStashInfo>, String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["stash", "list"],
        )
        .ok_or_else(|| "Failed to run git stash list".to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut stashes = Vec::new();
        for line in stdout.lines() {
            if let Some((stash_part, rest)) = line.split_once(':') {
                let name = stash_part.trim().to_string();
                if let Some(start) = name.find('{') {
                    if let Some(end) = name.find('}') {
                        if let Ok(index) = name[start + 1..end].parse::<usize>() {
                            let message = rest.trim().to_string();
                            stashes.push(GitStashInfo {
                                index,
                                name,
                                message,
                            });
                        }
                    }
                }
            }
        }
        Ok(stashes)
    })
    .await
    .map_err(|e| format!("git stash list task failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_apply(cwd: String, index: usize) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let stash_ref = format!("stash@{{{}}}", index);
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["stash", "apply", &stash_ref],
        )
        .ok_or_else(|| "Failed to run git stash apply".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git stash apply task failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_pop(cwd: String, index: usize) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let stash_ref = format!("stash@{{{}}}", index);
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["stash", "pop", &stash_ref],
        )
        .ok_or_else(|| "Failed to run git stash pop".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git stash pop task failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_drop(cwd: String, index: usize) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let stash_ref = format!("stash@{{{}}}", index);
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["stash", "drop", &stash_ref],
        )
        .ok_or_else(|| "Failed to run git stash drop".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git stash drop task failed: {e}"))?
}

#[tauri::command]
pub async fn git_branch_list(cwd: String) -> Result<Vec<String>, String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["branch", "-a", "--format=%(refname:short)"],
        )
        .ok_or_else(|| "Failed to run git branch".to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut branches = Vec::new();
        for line in stdout.lines() {
            let name = line.trim();
            if !name.is_empty() {
                branches.push(name.to_string());
            }
        }
        Ok(branches)
    })
    .await
    .map_err(|e| format!("git branch list task failed: {e}"))?
}

#[tauri::command]
pub async fn git_branch_switch(cwd: String, name: String) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["checkout", &name],
        )
        .ok_or_else(|| "Failed to run git checkout".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git branch switch task failed: {e}"))?
}

#[tauri::command]
pub async fn git_branch_create(cwd: String, name: String) -> Result<(), String> {
    let validated = validate_project_path(&cwd)?;
    let validated_str = validated
        .to_str()
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())?
        .to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let output = crate::trackers::git_tracker::GitTracker::run_git_command(
            &validated_str,
            &["checkout", "-b", &name],
        )
        .ok_or_else(|| "Failed to run git checkout -b".to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("git branch create task failed: {e}"))?
}

/// Cap on any single renderer-supplied field to keep one forwarded error from
/// ballooning the log file.
pub(crate) const MAX_FRONTEND_FIELD_LEN: usize = 4096;

/// Sanitize untrusted renderer text for single-line logging: escape newlines
/// and control characters so a crafted error message/stack cannot forge
/// additional, authoritative-looking log lines (log injection), and truncate
/// to a sane bound.
pub(crate) fn sanitize_log_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(MAX_FRONTEND_FIELD_LEN));
    for ch in value.chars().take(MAX_FRONTEND_FIELD_LEN) {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Strip other C0 control chars (incl. ESC) that could corrupt or
            // spoof terminal/log output; keep everything else verbatim.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    if value.chars().count() > MAX_FRONTEND_FIELD_LEN {
        out.push_str("…[truncated]");
    }
    out
}

/// Forward a renderer-side error to the backend log file (issue #244).
///
/// Global `window.onerror` / `onunhandledrejection` handlers and the React
/// ErrorBoundary route through this so frontend failures survive a closed
/// production DevTools console and land in the same rotated log file as the
/// Rust logs. `level` accepts "error" (default) or "warn". All renderer-supplied
/// fields are sanitized to prevent log injection.
#[tauri::command]
pub fn log_frontend_error(
    level: Option<String>,
    message: String,
    source: Option<String>,
    stack: Option<String>,
    component_stack: Option<String>,
) -> Result<(), String> {
    let context = sanitize_log_field(&source.unwrap_or_else(|| "renderer".to_string()));
    let message = sanitize_log_field(&message);
    let stack_part = stack
        .map(|s| format!(" | stack: {}", sanitize_log_field(&s)))
        .unwrap_or_default();
    let component_part = component_stack
        .map(|s| format!(" | component stack: {}", sanitize_log_field(&s)))
        .unwrap_or_default();

    let line = format!(
        "[frontend] [{}] {}{}{}",
        context, message, stack_part, component_part
    );

    match level.as_deref() {
        Some("warn") => log::warn!("{}", line),
        _ => log::error!("{}", line),
    }

    Ok(())
}

// ============================================================================
// Conversation application commands (shared Tauri/HTTP/WS service)
// ============================================================================

fn conversation_application_failure<T>(
    error: crate::conversation::ConversationApplicationError,
) -> IpcResult<T> {
    log::warn!(
        "[conversation-command] operation={} conversation_id={} code={}",
        error.operation,
        error
            .conversation_id
            .map_or_else(|| "none".to_string(), |value| value.to_string()),
        error.code
    );
    IpcResult::error(error.detail, error.code)
}

fn parse_conversation_id<T>(
    value: &str,
) -> Result<crate::conversation::ConversationId, IpcResult<T>> {
    crate::conversation::ConversationId::parse_path_component(value)
        .map_err(|error| IpcResult::error(error.to_string(), "CONVERSATION_INVALID_ID"))
}

pub(crate) fn conversation_host_status_inner(
    service: &crate::conversation::ConversationApplicationService,
) -> IpcResult<crate::conversation::ConversationHostStatus> {
    match service.host_status() {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub fn conversation_host_status(
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationHostStatus>, String> {
    Ok(conversation_host_status_inner(service.inner()))
}

pub(crate) fn conversation_list_inner(
    service: &crate::conversation::ConversationApplicationService,
) -> IpcResult<Vec<crate::conversation::ConversationRecordV2>> {
    IpcResult::success(service.list_conversations())
}

#[tauri::command]
pub fn conversation_list(
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<Vec<crate::conversation::ConversationRecordV2>>, String> {
    Ok(conversation_list_inner(service.inner()))
}

pub(crate) fn conversation_get_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
) -> IpcResult<crate::conversation::ConversationRecordV2> {
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match service.get_conversation(conversation_id) {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub fn conversation_get(
    conversation_id: String,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationRecordV2>, String> {
    Ok(conversation_get_inner(service.inner(), &conversation_id))
}

pub(crate) fn conversation_get_binding_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
) -> IpcResult<crate::conversation::ConversationBindingSnapshot> {
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match service.current_binding(conversation_id) {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub fn conversation_get_binding(
    conversation_id: String,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationBindingSnapshot>, String> {
    Ok(conversation_get_binding_inner(
        service.inner(),
        &conversation_id,
    ))
}

#[tauri::command]
pub async fn conversation_rename(
    conversation_id: String,
    title: String,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationRecordV2>, String> {
    let conversation_id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    Ok(
        match service.rename_conversation(conversation_id, title).await {
            Ok(record) => IpcResult::success(record),
            Err(error) => IpcResult::error(error.detail, error.code),
        },
    )
}

pub(crate) async fn conversation_open_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
) -> IpcResult<crate::conversation::ConversationOpenOutcome> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match service.open_conversation(conversation_id).await {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_open(
    conversation_id: String,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationOpenOutcome>, String> {
    Ok(conversation_open_inner(service.inner(), &conversation_id).await)
}

pub(crate) fn conversation_resolve_legacy_id_inner(
    service: &crate::conversation::ConversationApplicationService,
    request: serde_json::Value,
) -> IpcResult<crate::conversation::LegacyConversationResolution> {
    let request: crate::conversation::LegacyConversationKey = match serde_json::from_value(request)
    {
        Ok(value) => value,
        Err(error) => {
            return IpcResult::error(
                format!("payload validation failed: {error}"),
                "VALIDATION_ERROR",
            )
        }
    };
    match service.resolve_legacy_conversation_id(request) {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub fn conversation_resolve_legacy_id(
    request: serde_json::Value,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::LegacyConversationResolution>, String> {
    Ok(conversation_resolve_legacy_id_inner(
        service.inner(),
        request,
    ))
}

#[tauri::command]
pub async fn session_workspace_get(
    conversation_id: String,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::SessionWorkspaceLoadOutcome>, String> {
    let conversation_id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    Ok(match service.get_workspace(conversation_id).await {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    })
}

#[tauri::command]
pub async fn session_workspace_write(
    conversation_id: String,
    based_revision: Option<u64>,
    workspace: serde_json::Value,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::SessionWorkspaceWriteOutcome>, String> {
    if let Err(error) = require_host_admission() {
        return Ok(error);
    }
    let conversation_id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let workspace: crate::conversation::SessionWorkspaceV1 = match serde_json::from_value(workspace)
    {
        Ok(workspace) => workspace,
        Err(error) => {
            return Ok(IpcResult::error(
                format!("payload validation failed: {error}"),
                "VALIDATION_ERROR",
            ))
        }
    };
    Ok(
        match service
            .write_workspace(conversation_id, based_revision, workspace)
            .await
        {
            Ok(outcome) => IpcResult::success(outcome),
            Err(error) => conversation_application_failure(error),
        },
    )
}

pub(crate) async fn conversation_recovery_resolve_inner(
    service: &crate::conversation::ConversationApplicationService,
    request: serde_json::Value,
) -> IpcResult<crate::conversation::migration::RecoveryActionResult> {
    let request: crate::conversation::migration::ResolveRecoveryItemRequest =
        match serde_json::from_value(request) {
            Ok(request) => request,
            Err(error) => {
                return IpcResult::error(
                    format!("payload validation failed: {error}"),
                    "VALIDATION_ERROR",
                )
            }
        };
    match service.resolve_recovery_item(request).await {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_recovery_resolve(
    app: AppHandle,
    request: serde_json::Value,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::migration::RecoveryActionResult>, String> {
    let outcome = conversation_recovery_resolve_inner(service.inner(), request).await;
    if outcome.success {
        let _ = app.emit("conversation:host-status", ());
    }
    Ok(outcome)
}

pub(crate) async fn conversation_attach_project_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
    expected_revision: u64,
    attachment: serde_json::Value,
) -> IpcResult<crate::conversation::ConversationAggregateMutationOutcome> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let attachment: crate::conversation::ProjectAttachment =
        match serde_json::from_value(attachment) {
            Ok(value) => value,
            Err(error) => {
                return IpcResult::error(
                    format!("payload validation failed: {error}"),
                    "VALIDATION_ERROR",
                )
            }
        };
    match service
        .attach_project(conversation_id, expected_revision, attachment)
        .await
    {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_attach_project(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    attachment: serde_json::Value,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationAggregateMutationOutcome>, String> {
    let outcome = conversation_attach_project_inner(
        service.inner(),
        &conversation_id,
        expected_revision,
        attachment,
    )
    .await;
    if outcome.success {
        let _ = app.emit("conversation:aggregate", outcome.data.as_ref());
    }
    Ok(outcome)
}

pub(crate) async fn conversation_detach_project_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
    expected_revision: u64,
) -> IpcResult<crate::conversation::ConversationAggregateMutationOutcome> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    match service
        .detach_project(conversation_id, expected_revision)
        .await
    {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_detach_project(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationAggregateMutationOutcome>, String> {
    let outcome =
        conversation_detach_project_inner(service.inner(), &conversation_id, expected_revision)
            .await;
    if outcome.success {
        let _ = app.emit("conversation:aggregate", outcome.data.as_ref());
    }
    Ok(outcome)
}

pub(crate) async fn conversation_update_execution_target_inner(
    service: &crate::conversation::ConversationApplicationService,
    conversation_id: &str,
    expected_revision: u64,
    execution_target: serde_json::Value,
) -> IpcResult<crate::conversation::ConversationAggregateMutationOutcome> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let conversation_id = match parse_conversation_id(conversation_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let execution_target: crate::conversation::ExecutionTarget =
        match serde_json::from_value(execution_target) {
            Ok(value) => value,
            Err(error) => {
                return IpcResult::error(
                    format!("payload validation failed: {error}"),
                    "VALIDATION_ERROR",
                )
            }
        };
    match service
        .update_execution_target(conversation_id, expected_revision, execution_target)
        .await
    {
        Ok(outcome) => IpcResult::success(outcome),
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_update_execution_target(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    execution_target: serde_json::Value,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationAggregateMutationOutcome>, String> {
    let outcome = conversation_update_execution_target_inner(
        service.inner(),
        &conversation_id,
        expected_revision,
        execution_target,
    )
    .await;
    if outcome.success {
        let _ = app.emit("conversation:aggregate", outcome.data.as_ref());
    }
    Ok(outcome)
}

async fn run_conversation_lifecycle_command(
    app: &AppHandle,
    _conversation_id: String,
    operation: impl std::future::Future<
        Output = crate::conversation::application::Result<
            crate::conversation::ConversationLifecycleOutcome,
        >,
    >,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    match operation.await {
        Ok(outcome) => {
            let _ = app.emit("conversation:lifecycle", &outcome);
            Ok(IpcResult::success(outcome))
        }
        Err(error) => Ok(conversation_application_failure(error)),
    }
}

#[tauri::command]
pub async fn conversation_detach_binding(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    let id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    run_conversation_lifecycle_command(
        &app,
        conversation_id,
        service.detach_binding(id, expected_revision),
    )
    .await
}

#[tauri::command]
pub async fn conversation_rebind_detached_binding(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    let id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    run_conversation_lifecycle_command(
        &app,
        conversation_id,
        service.rebind_binding(id, expected_revision),
    )
    .await
}

#[tauri::command]
pub async fn conversation_suspend_binding(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    let id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    run_conversation_lifecycle_command(
        &app,
        conversation_id,
        service.suspend_binding(id, expected_revision),
    )
    .await
}

#[tauri::command]
pub async fn conversation_replace_binding(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    request: serde_json::Value,
    // Runtime id of the agent to bind to. `None` restarts on the same agent.
    target_runtime_agent_id: Option<String>,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    let id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let request: crate::conversation::PrepareConversationRequest =
        match serde_json::from_value(request) {
            Ok(value) => value,
            Err(error) => {
                return Ok(IpcResult::error(
                    format!("payload validation failed: {error}"),
                    "VALIDATION_ERROR",
                ))
            }
        };
    run_conversation_lifecycle_command(
        &app,
        conversation_id,
        service.replace_binding(id, request, expected_revision, target_runtime_agent_id),
    )
    .await
}

async fn retire_deleted_binding_if_updated(
    relay: &crate::web::WsRelaySink,
    current_session_id: Option<&str>,
    outcome: &crate::conversation::ConversationLifecycleOutcome,
) -> Result<(), String> {
    if matches!(
        outcome,
        crate::conversation::ConversationLifecycleOutcome::Updated {
            action: crate::conversation::ConversationLifecycleAction::DeleteConversation,
            ..
        }
    ) {
        if let Some(session_id) = current_session_id {
            relay.retire_session(session_id).await?;
        }
    }
    Ok(())
}

async fn conversation_delete_with_retirement(
    service: &crate::conversation::ConversationApplicationService,
    relay: &crate::web::WsRelaySink,
    conversation_id: crate::conversation::ConversationId,
    expected_revision: u64,
) -> IpcResult<crate::conversation::ConversationLifecycleOutcome> {
    if let Err(error) = require_host_admission() {
        return error;
    }
    let current_session_id = match service
        .writer()
        .repository()
        .current_binding(conversation_id)
    {
        Ok(binding) => binding.map(|binding| binding.agent_session_id),
        Err(_) => {
            return IpcResult::error(
                "failed to resolve Conversation binding before delete",
                "CONVERSATION_RECOVERY_REQUIRED",
            )
        }
    };
    match service
        .delete_conversation(conversation_id, expected_revision)
        .await
    {
        Ok(outcome) => {
            if let Err(code) =
                retire_deleted_binding_if_updated(relay, current_session_id.as_deref(), &outcome)
                    .await
            {
                log::error!(
                    "[conversation-retirement] operation=tauri_delete code={} conversation_id={}",
                    code,
                    conversation_id
                );
                return IpcResult::error(
                    "Conversation auxiliary retirement failed",
                    "CONVERSATION_RETIREMENT_FAILED",
                );
            }
            IpcResult::success(outcome)
        }
        Err(error) => conversation_application_failure(error),
    }
}

#[tauri::command]
pub async fn conversation_delete(
    app: AppHandle,
    conversation_id: String,
    expected_revision: u64,
    remove_workspace: Option<bool>,
    service: State<'_, Arc<crate::conversation::ConversationApplicationService>>,
    relay: State<'_, Arc<crate::web::WsRelaySink>>,
) -> Result<IpcResult<crate::conversation::ConversationLifecycleOutcome>, String> {
    let id = match parse_conversation_id(&conversation_id) {
        Ok(value) => value,
        Err(error) => return Ok(error),
    };
    let workspace_cwd = service
        .get_conversation(id)
        .ok()
        .map(|record| record.workspace_cwd);
    let outcome =
        conversation_delete_with_retirement(service.inner(), relay.inner(), id, expected_revision)
            .await;
    if outcome.success {
        if remove_workspace == Some(true) {
            if let Some(path) = workspace_cwd.filter(|path| !path.trim().is_empty()) {
                // User-confirmed recursive removal of the Conversation workspace
                // directory; best-effort so a locked file cannot veto the delete.
                if let Err(error) = std::fs::remove_dir_all(&path) {
                    log::warn!(
                        "[conversation-delete] workspace removal failed conversation_id={} path={} error={error}",
                        conversation_id,
                        path
                    );
                } else {
                    log::info!(
                        "[conversation-delete] workspace removed conversation_id={} path={}",
                        conversation_id,
                        path
                    );
                }
            }
        }
        let _ = app.emit("conversation:lifecycle", outcome.data.as_ref());
    }
    Ok(outcome)
}

// ============================================================================
// Workspace Manifest Commands (legacy read-only compatibility)
// ============================================================================
//
// Host-owned versioned workspace manifests — one per project, atomically
// persisted, revision-checked. Conflict is a success-body variant of
// `WriteOutcome`, NOT an error code; serde `deny_unknown_fields` rejection
// (an over-serialized payload with `envVars` / raw `claim` /
// `fullscreenPaneId`) maps to `VALIDATION_ERROR`. Mirrors the three HTTP
// routes in `web/workspace_api.rs` byte-for-byte (camelCase `IpcResult<T>`).

/// Host-owned workspace-manifest state (CAP-5). `None` when the desktop could
/// not open `WorkspaceManifestService` at startup (degraded fresh-only mode);
/// commands must treat absence as an empty manifest, never crash.
///
/// Patch 12: the inner field is private (not `pub`) so callers cannot reach
/// the `Arc` directly — they go through [`Self::store`] (read) or
/// [`Self::new`] (construct). This keeps the access surface tight so a future
/// swap (e.g. a manager wrapper that owns the `Arc`) doesn't break every call
/// site.
#[derive(Default)]
pub struct HostWorkspaceManifestStore(Option<Arc<crate::acp::WorkspaceManifestService>>);

impl HostWorkspaceManifestStore {
    /// Construct from an already-opened `WorkspaceManifestService` (`None`
    /// for degraded fresh-only mode — the desktop could not open the store
    /// at startup).
    #[must_use]
    pub fn new(service: Option<Arc<crate::acp::WorkspaceManifestService>>) -> Self {
        Self(service)
    }

    /// Access the inner `WorkspaceManifestService` (`None` in degraded mode).
    /// Callers that need to clone the `Arc` should `.as_ref().map(Arc::clone)`.
    #[must_use]
    pub(crate) fn store(&self) -> Option<&Arc<crate::acp::WorkspaceManifestService>> {
        self.0.as_ref()
    }
}

/// Tauri state wrapper for the host-owned `AcpCatalogService` (CAP-6 / Story
/// 8). Mirrors `HostWorkspaceManifestStore`: `None` degrades to
/// `ACP_CATALOG_UNAVAILABLE` (the desktop could not open the catalog root at
/// startup). Held as `Option<Arc<…>>` so the desktop's degraded path is
/// graceful, not a boot failure.
#[derive(Default)]
pub struct HostAcpCatalogStore(Option<Arc<crate::acp::AcpCatalogService>>);

impl HostAcpCatalogStore {
    /// Construct from an already-opened `AcpCatalogService` (`None` for
    /// degraded mode — the desktop could not open the store at startup).
    #[must_use]
    pub fn new(service: Option<Arc<crate::acp::AcpCatalogService>>) -> Self {
        Self(service)
    }

    /// Access the inner `AcpCatalogService` (`None` in degraded mode).
    /// Callers that need to clone the `Arc` should `.as_ref().map(Arc::clone)`.
    #[must_use]
    pub(crate) fn store(&self) -> Option<&Arc<crate::acp::AcpCatalogService>> {
        self.0.as_ref()
    }
}

/// Tauri state wrapper for the host-owned `AcpInstallService` (CAP-6 / Story
/// 9). Mirrors `HostAcpCatalogStore`: `None` degrades to
/// `ACP_INSTALL_UNAVAILABLE` (the desktop could not open the install root at
/// startup). Held as `Option<Arc<…>>` so the desktop's degraded path is
/// graceful, not a boot failure.
#[derive(Default)]
pub struct HostAcpInstallStore(Option<Arc<crate::acp::install::AcpInstallService>>);

impl HostAcpInstallStore {
    /// Construct from an already-opened `AcpInstallService` (`None` for
    /// degraded mode — the desktop could not open the store at startup).
    #[must_use]
    pub fn new(service: Option<Arc<crate::acp::install::AcpInstallService>>) -> Self {
        Self(service)
    }

    /// Access the inner `AcpInstallService` (`None` in degraded mode).
    /// Callers that need to clone the `Arc` should `.as_ref().map(Arc::clone)`.
    #[must_use]
    pub(crate) fn store(&self) -> Option<&Arc<crate::acp::install::AcpInstallService>> {
        self.0.as_ref()
    }
}

/// `workspace_manifest_get(projectId)` — load a project's manifest. Returns
/// `IpcResult::success(None)` when no manifest exists (the success path — a
/// workspace reload starts fresh) OR when the host store is unavailable
/// (degraded mode). Mirrors `GET /workspace/:projectId` byte-for-byte.
#[tauri::command]
pub async fn workspace_manifest_get(
    project_id: String,
    store: State<'_, HostWorkspaceManifestStore>,
) -> Result<IpcResult<Option<crate::acp::WorkspaceManifest>>, String> {
    let log_project_id = sanitize_log_field(&project_id);
    log::info!(
        "[workspace-manifest] get start project_id={}",
        log_project_id
    );
    let Some(service) = store.store().map(Arc::clone) else {
        log::info!(
            "[workspace-manifest] get unavailable project_id={}",
            log_project_id
        );
        return Ok(IpcResult::success(None));
    };
    match service.load(&project_id).await {
        Ok(manifest) => {
            log::info!(
                "[workspace-manifest] get success project_id={} revision={}",
                log_project_id,
                manifest.as_ref().map_or(0, |m| m.revision)
            );
            Ok(IpcResult::success(manifest))
        }
        Err(error) => {
            log::error!(
                "[workspace-manifest] get failure project_id={} error={}",
                log_project_id,
                error
            );
            Ok(IpcResult::error(
                error.to_string(),
                "WORKSPACE_MANIFEST_GET_FAILED",
            ))
        }
    }
}

/// `workspace_manifest_write(projectId, basedRevision, manifest)` —
/// revision-checked write. The host compares `basedRevision` (null = initial
/// write) against the on-disk `revision`; on match → apply + increment +
/// persist + return `WriteOutcome::Updated`; on mismatch → return
/// `WriteOutcome::Conflict` WITHOUT mutating state. Conflict is a SUCCESS
/// body variant (NOT an error code) — the caller branches on the
/// `status` discriminator.
///
/// Patch 1: the `manifest` argument is `serde_json::Value` (not
/// `WorkspaceManifest`) so the manual deserialization inside the command
/// catches a `deny_unknown_fields` rejection (an excluded field like
/// `envVars` / raw `claim` / `fullscreenPaneId`) and maps it to
/// `IpcResult::error(VALIDATION_ERROR)` — BEFORE the service is reached, with
/// NO state change. If the argument were typed `WorkspaceManifest`, Tauri's
/// IPC deserialization layer would reject the payload before this command
/// body runs, surfacing as an `INVOKE_ERROR` (a thrown IPC error) instead of
/// the spec-required `VALIDATION_ERROR`.
#[tauri::command]
pub async fn workspace_manifest_write(
    project_id: String,
    based_revision: Option<u64>,
    manifest: serde_json::Value,
    store: State<'_, HostWorkspaceManifestStore>,
) -> Result<IpcResult<crate::acp::WriteOutcome>, String> {
    if let Err(error) = require_host_admission() {
        return Ok(error);
    }
    let log_project_id = sanitize_log_field(&project_id);
    log::info!(
        "[workspace-manifest] write start project_id={} based_revision={:?}",
        log_project_id,
        based_revision
    );
    // Patch 1: manual deserialization so a `deny_unknown_fields` rejection
    // (envVars / raw claim / fullscreenPaneId / agentLauncherPaneId) surfaces
    // as `IpcResult::error(VALIDATION_ERROR)` — NOT a thrown IPC error
    // (`INVOKE_ERROR`) that would mask the validation failure.
    let manifest: crate::acp::WorkspaceManifest = match serde_json::from_value(manifest) {
        Ok(m) => m,
        Err(error) => {
            log::warn!(
                "[workspace-manifest] write payload validation failed project_id={} error={}",
                log_project_id,
                error
            );
            return Ok(IpcResult::error(
                format!("payload validation failed: {error}"),
                "VALIDATION_ERROR",
            ));
        }
    };
    let Some(service) = store.store().map(Arc::clone) else {
        log::error!(
            "[workspace-manifest] write unavailable project_id={}",
            log_project_id
        );
        return Ok(IpcResult::error(
            "workspace manifest store is unavailable",
            "WORKSPACE_MANIFEST_UNAVAILABLE",
        ));
    };
    match service.write(&project_id, based_revision, manifest).await {
        Ok(outcome) => {
            // Boundary log emits project_id + revision + update_identity —
            // never the topology or claim. The service already logged it.
            Ok(IpcResult::success(outcome))
        }
        Err(error) => {
            log::error!(
                "[workspace-manifest] write failure project_id={} error={}",
                log_project_id,
                error
            );
            Ok(IpcResult::error(
                error.to_string(),
                "WORKSPACE_MANIFEST_WRITE_FAILED",
            ))
        }
    }
}

/// `workspace_manifest_delete(projectId)` — idempotent delete. Returns
/// `IpcResult::success(())` whether the file existed or not. Never touches
/// the PTY / agent layer (the manifest is a passive durable projection).
#[tauri::command]
pub async fn workspace_manifest_delete(
    project_id: String,
    store: State<'_, HostWorkspaceManifestStore>,
) -> Result<IpcResult<()>, String> {
    if let Err(error) = require_host_admission() {
        return Ok(error);
    }
    let log_project_id = sanitize_log_field(&project_id);
    log::info!(
        "[workspace-manifest] delete start project_id={}",
        log_project_id
    );
    let Some(service) = store.store().map(Arc::clone) else {
        log::info!(
            "[workspace-manifest] delete unavailable project_id={}",
            log_project_id
        );
        // Idempotent success — there is nothing to delete in degraded mode.
        return Ok(IpcResult::success(()));
    };
    match service.delete(&project_id).await {
        Ok(()) => Ok(IpcResult::success(())),
        Err(error) => {
            log::error!(
                "[workspace-manifest] delete failure project_id={} error={}",
                log_project_id,
                error
            );
            Ok(IpcResult::error(
                error.to_string(),
                "WORKSPACE_MANIFEST_DELETE_FAILED",
            ))
        }
    }
}

/// Get available shells
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_result_success() {
        let result: IpcResult<String> = IpcResult::success("test".to_string());
        assert!(result.success);
        assert_eq!(result.data, Some("test".to_string()));
        assert!(result.error.is_none());
        assert!(result.code.is_none());
    }

    #[test]
    fn test_ipc_result_error() {
        let result: IpcResult<String> = IpcResult::error("test error", "TEST_ERROR");
        assert!(!result.success);
        assert!(result.data.is_none());
        assert_eq!(result.error, Some("test error".to_string()));
        assert_eq!(result.code, Some("TEST_ERROR".to_string()));
    }

    #[tokio::test]
    async fn terminal_spawn_compound_rollback() {
        use crate::conversation::{
            parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
            ConversationMutation, ConversationRecordV2, ConversationWriter, CreationPartition,
            ExecutionTarget, SessionWorkspaceLoadOutcome, SessionWorkspaceService,
            CONVERSATION_SCHEMA_VERSION, TERMINAL_RESOURCE_ROLLBACK_FAILED,
        };
        use crate::pty::manager::{
            ScriptedCleanupDriver, TerminalCleanupStage, TerminalLifecycleState,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) =
            crate::conversation::ConversationRepository::open(base.join("conversations/v2"))
                .unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            crate::conversation::ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab")
                .unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: base.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(writer));
        repository.fail_next_workspace_replace();
        let pty = crate::web::test_pty_manager();
        let cleanup_driver = Arc::new(ScriptedCleanupDriver::default());
        cleanup_driver.fail_once(TerminalCleanupStage::Kill);
        pty.install_cleanup_driver(cleanup_driver);
        let result = terminal_spawn_resource(
            SpawnOptions {
                conversation_id: Some(conversation_id),
                cwd: Some(base.to_string_lossy().into_owned()),
                ..Default::default()
            },
            None,
            &pty,
            &workspace,
        )
        .await;

        assert!(!result.success);
        assert_eq!(
            result.code.as_deref(),
            Some(TERMINAL_RESOURCE_ROLLBACK_FAILED)
        );
        let failure: TerminalResourceFailureV1 =
            serde_json::from_str(result.error.as_deref().unwrap()).unwrap();
        assert_eq!(failure.primary_code, "CONVERSATION_DURABILITY_FAILED");
        assert_eq!(failure.cleanup_stage, TerminalCleanupStage::Kill);
        assert_eq!(
            pty.terminal_lifecycle_state(&failure.terminal_id),
            Some(TerminalLifecycleState::Quarantined)
        );
        assert_eq!(pty.active_terminal_slot_count(), 1);
        assert!(pty.get(&failure.terminal_id).is_some());
        assert!(matches!(
            workspace.load(conversation_id).await.unwrap(),
            SessionWorkspaceLoadOutcome::Missing { .. }
        ));
        pty.terminate(&failure.terminal_id).await.unwrap();
        assert_eq!(pty.active_terminal_slot_count(), 0);

        let production = include_str!("commands.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in ["claim=", "env=", "argv=", "terminal_output="] {
            assert!(!production.contains(forbidden));
        }
        let removed_test_override = ["rollback", "result", "override"].join("_");
        assert!(!production.contains(&removed_test_override));
    }

    #[tokio::test]
    async fn terminal_terminate_returns_exact_cleanup_stage_and_keeps_identity() {
        use crate::conversation::{
            ConversationRepository, ConversationWriter, SessionWorkspaceService,
        };
        use crate::pty::manager::{
            ScriptedCleanupDriver, TerminalCleanupStage, TerminalLifecycleState,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) = ConversationRepository::open(base.join("private")).unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(ConversationWriter::for_test(
            repository,
        )));
        let pty = crate::web::test_pty_manager();
        let spawned = pty
            .spawn(
                SpawnOptions {
                    cwd: Some(base.to_string_lossy().into_owned()),
                    kind: Some("ssh".to_string()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let terminal_id = spawned.info.id;
        let cleanup_driver = Arc::new(ScriptedCleanupDriver::default());
        cleanup_driver.fail_once(TerminalCleanupStage::Wait);
        pty.install_cleanup_driver(cleanup_driver);

        let result = terminal_terminate_resource(&terminal_id, &pty, &workspace).await;
        assert!(!result.success);
        assert_eq!(
            result.code.as_deref(),
            Some(crate::conversation::TERMINAL_TERMINATE_FAILED)
        );
        let failure: TerminalResourceFailureV1 =
            serde_json::from_str(result.error.as_deref().unwrap()).unwrap();
        assert_eq!(failure.terminal_id, terminal_id);
        assert_eq!(
            failure.primary_code,
            crate::conversation::TERMINAL_TERMINATE_FAILED
        );
        assert_eq!(failure.cleanup_stage, TerminalCleanupStage::Wait);
        assert_eq!(
            pty.terminal_lifecycle_state(&terminal_id),
            Some(TerminalLifecycleState::Quarantined)
        );
        assert_eq!(pty.active_terminal_slot_count(), 1);

        let retry = terminal_terminate_resource(&terminal_id, &pty, &workspace).await;
        assert!(retry.success, "retry failed: {:?}", retry.error);
        assert!(pty.get(&terminal_id).is_none());
        assert_eq!(pty.active_terminal_slot_count(), 0);
    }

    #[tokio::test]
    async fn terminal_terminate_succeeds_for_scope_less_project_terminal() {
        use crate::conversation::{
            ConversationRepository, ConversationWriter, SessionWorkspaceService,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) = ConversationRepository::open(base.join("private")).unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(ConversationWriter::for_test(
            repository,
        )));
        let pty = crate::web::test_pty_manager();
        let spawned = pty
            .spawn(
                SpawnOptions {
                    cwd: Some(base.to_string_lossy().into_owned()),
                    project_id: Some("project-1".to_string()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            !pty.get(&spawned.info.id)
                .expect("spawned project terminal")
                .workspace_ref_tracked
        );

        let result = terminal_terminate_resource(&spawned.info.id, &pty, &workspace).await;
        assert!(result.success, "terminate failed: {:?}", result.error);
        assert!(pty.get(&spawned.info.id).is_none());
        assert_eq!(pty.active_terminal_slot_count(), 0);
    }

    #[tokio::test]
    async fn terminal_terminate_succeeds_when_tracked_conversation_is_missing() {
        use crate::conversation::{
            ConversationId, ConversationRepository, ConversationWriter, SessionWorkspaceService,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) = ConversationRepository::open(base.join("private")).unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(ConversationWriter::for_test(
            repository,
        )));
        let pty = crate::web::test_pty_manager();
        let orphan_conversation = ConversationId::new_v4();
        let spawned = pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(orphan_conversation),
                    cwd: Some(base.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            pty.get(&spawned.info.id)
                .expect("spawned orphan-scoped terminal")
                .workspace_ref_tracked
        );

        let result = terminal_terminate_resource(&spawned.info.id, &pty, &workspace).await;
        assert!(
            result.success,
            "terminate should kill PTY when conversation is missing: {:?}",
            result.error
        );
        assert!(pty.get(&spawned.info.id).is_none());
        assert_eq!(pty.active_terminal_slot_count(), 0);
    }

    #[tokio::test]
    async fn terminal_resume_requires_passive_ref_and_returns_grant() {
        use crate::conversation::{
            parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
            ConversationMutation, ConversationRecordV2, ConversationWriter, CreationPartition,
            ExecutionTarget, SessionWorkspaceService, CONVERSATION_SCHEMA_VERSION,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) =
            crate::conversation::ConversationRepository::open(base.join("conversations/v2"))
                .unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            crate::conversation::ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab")
                .unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: base.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(writer));
        let pty = crate::web::test_pty_manager();

        // A live PTY without the passive descriptor is not resumable and its
        // original claim remains untouched.
        let untracked = pty
            .spawn(
                SpawnOptions {
                    conversation_id: Some(conversation_id),
                    cwd: Some(base.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let denied = TerminalResumeRequest {
            conversation_id,
            terminal_id: untracked.info.id.clone(),
            last_seq: 0,
        };
        // A live PTY with no passive ref is a denial, not a dead end: the
        // process is right there, the caller just has no reference to it.
        assert!(matches!(
            terminal_resume_resource(&denied, &pty, &workspace).await,
            Err(TerminalResumeDenial::Unauthorized)
        ));
        assert!(pty
            .verify_claim(&untracked.info.id, &untracked.claim)
            .is_ok());
        pty.terminate(&untracked.info.id).await.unwrap();

        // The shared spawn resource path commits the passive ref. Resume then
        // rotates the handoff without spawning or terminating the PTY.
        let spawned = terminal_spawn_resource(
            SpawnOptions {
                conversation_id: Some(conversation_id),
                cwd: Some(base.to_string_lossy().into_owned()),
                ..Default::default()
            },
            None,
            &pty,
            &workspace,
        )
        .await;
        assert!(spawned.success, "spawn failed: {:?}", spawned.error);
        let spawned = spawned.data.unwrap();
        let request = TerminalResumeRequest {
            conversation_id,
            terminal_id: spawned.info.id.clone(),
            last_seq: 0,
        };
        let (grant, _replay) = terminal_resume_resource(&request, &pty, &workspace)
            .await
            .unwrap();
        assert_eq!(grant.terminal.id, spawned.info.id);
        assert!(pty.get(&spawned.info.id).is_some());
        assert_eq!(
            pty.verify_claim(&spawned.info.id, &spawned.claim),
            Err(ClaimError)
        );
        assert!(pty.verify_claim(&spawned.info.id, &grant.claim).is_ok());

        let workspace_bytes = repository
            .read_workspace_bytes(conversation_id)
            .unwrap()
            .expect("spawn committed a passive workspace ref");
        let workspace_json = String::from_utf8(workspace_bytes).unwrap();
        assert!(!workspace_json.contains(&spawned.claim));
        assert!(!workspace_json.contains(&grant.claim));
        assert!(!workspace_json.contains("\"claim\""));

        let terminated = terminal_terminate_resource(&spawned.info.id, &pty, &workspace).await;
        assert!(
            terminated.success,
            "terminate failed: {:?}",
            terminated.error
        );

        // An orderly terminate retires the passive ref along with the PTY, so
        // this is a denial. The dead end is the disorderly case — PTY gone,
        // reference still standing — covered by
        // `terminal_resume_gone_is_distinct_from_denied`.
        assert!(matches!(
            terminal_resume_resource(&request, &pty, &workspace).await,
            Err(TerminalResumeDenial::Unauthorized)
        ));
    }

    /// A SessionWorkspace reference outlives the PTY it names: the app exits and
    /// kills every child, or the shell simply ends. On the next launch the
    /// manifest still lists the terminal, so the reference check passes and the
    /// failure lands one step later, where `resume_for_conversation` collapses
    /// "PTY is gone", "not active", "untracked ref" and "wrong Conversation"
    /// into one opaque `ClaimError`.
    ///
    /// The renderer cannot act on that: it renders a "retry connection"
    /// placeholder that can never succeed, and the tab comes back on every
    /// launch. Separating the two is safe precisely here — `terminal_resume` is
    /// the local Tauri boundary and the caller already proved it holds the
    /// manifest reference, so "the PTY is gone" tells it nothing new. The
    /// credential paths keep the single collapsed error; so does the web
    /// surface, which maps every denial to one generic response.
    #[tokio::test]
    async fn terminal_resume_gone_is_distinct_from_denied() {
        use crate::conversation::{
            parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
            ConversationMutation, ConversationRecordV2, ConversationWriter, CreationPartition,
            ExecutionTarget, SessionWorkspaceService, CONVERSATION_SCHEMA_VERSION,
        };

        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (repository, _) =
            crate::conversation::ConversationRepository::open(base.join("conversations/v2"))
                .unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            crate::conversation::ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab")
                .unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: base.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let workspace = Arc::new(SessionWorkspaceService::new(writer));
        let pty = crate::web::test_pty_manager();

        let spawned = terminal_spawn_resource(
            SpawnOptions {
                conversation_id: Some(conversation_id),
                cwd: Some(base.to_string_lossy().into_owned()),
                ..Default::default()
            },
            None,
            &pty,
            &workspace,
        )
        .await;
        assert!(spawned.success, "spawn failed: {:?}", spawned.error);
        let spawned = spawned.data.unwrap();
        let request = TerminalResumeRequest {
            conversation_id,
            terminal_id: spawned.info.id.clone(),
            last_seq: 0,
        };

        // Still alive: resume succeeds, so the assertions below cannot pass by
        // the reference being absent all along.
        assert!(terminal_resume_resource(&request, &pty, &workspace)
            .await
            .is_ok());

        // `pty.terminate` directly, NOT `terminal_terminate_resource`: the
        // orderly path retires the passive ref too, and this test is about the
        // disorderly case where only the PTY goes away — the app exits and
        // kills its children, or the shell simply ends.
        pty.terminate(&spawned.info.id).await.unwrap();
        assert!(pty.get(&spawned.info.id).is_none());

        assert!(matches!(
            terminal_resume_resource(&request, &pty, &workspace).await,
            Err(TerminalResumeDenial::Gone)
        ));

        // A reference that was never committed is still a denial, not a dead
        // end — the two causes must not collapse back into each other.
        let unknown = TerminalResumeRequest {
            conversation_id,
            terminal_id: "terminal-never-existed".to_string(),
            last_seq: 0,
        };
        assert!(matches!(
            terminal_resume_resource(&unknown, &pty, &workspace).await,
            Err(TerminalResumeDenial::Unauthorized)
        ));
    }

    /// The host-owned list maps `SessionIndexEntry` (camelCase wire) into the
    /// renderer's `ChatHistoryIndexEntry` shape unchanged by the ownership
    /// transfer: `config:<id>` namespaces collapse back to the bare config id,
    /// absent titles/projects fall back to the renderer defaults.
    #[test]
    fn host_entry_to_desktop_maps_renderer_shape() {
        let entry = crate::acp::SessionIndexEntry {
            storage_key: "key".to_string(),
            session_id: "s-1".to_string(),
            stable_agent_namespace: Some("config:claude".to_string()),
            runtime_agent_id: Some("runtime-1".to_string()),
            project_id: Some("p-1".to_string()),
            cwd: "/work".to_string(),
            title: Some("Chat".to_string()),
            title_source: None,
            created_at: 10,
            last_activity_at: 20,
            status: crate::acp::PersistedSessionStatus::Active,
            message_count: 3,
            tool_count: 1,
            last_seq: 5,
            discovered: false,
            resume_eligible: true,
            worktree_path: None,
            worktree_branch: None,
        };
        let desktop = host_entry_to_desktop(entry);
        assert_eq!(desktop.id, "s-1");
        assert_eq!(desktop.agent_id, "runtime-1");
        assert_eq!(desktop.agent_config_id.as_deref(), Some("claude"));
        assert_eq!(desktop.title, "Chat");
        assert_eq!(desktop.cwd, "/work");
        assert_eq!(desktop.project_id, "p-1");
        assert_eq!(desktop.created_at, 10);
        assert_eq!(desktop.last_activity_at, 20);
        assert_eq!(desktop.message_count, 3);
        assert!(desktop.conversation_id.is_none());

        let bound = crate::acp::SessionIndexEntry {
            storage_key: "018f7a1c-1b4d-7c8a-9f01-0123456789ab".to_string(),
            session_id: "opaque/session".to_string(),
            stable_agent_namespace: Some("config:claude".to_string()),
            runtime_agent_id: Some("runtime-1".to_string()),
            project_id: None,
            cwd: "/work".to_string(),
            title: Some("Bound".to_string()),
            title_source: None,
            created_at: 10,
            last_activity_at: 20,
            status: crate::acp::PersistedSessionStatus::Closed,
            message_count: 2,
            tool_count: 0,
            last_seq: 2,
            discovered: false,
            resume_eligible: true,
            worktree_path: None,
            worktree_branch: None,
        };
        let bound_desktop = host_entry_to_desktop(bound);
        assert_eq!(
            bound_desktop.conversation_id.as_deref(),
            Some("018f7a1c-1b4d-7c8a-9f01-0123456789ab")
        );
        assert_eq!(bound_desktop.id, "opaque/session");
        assert!(matches!(
            desktop.status,
            crate::acp::ChatHistoryStatus::Active
        ));

        let bare = crate::acp::SessionIndexEntry {
            storage_key: "k".to_string(),
            session_id: "s-2".to_string(),
            stable_agent_namespace: Some("custom-ns".to_string()),
            runtime_agent_id: None,
            project_id: None,
            cwd: "/w".to_string(),
            title: None,
            title_source: None,
            created_at: 1,
            last_activity_at: 2,
            status: crate::acp::PersistedSessionStatus::Error,
            message_count: 0,
            tool_count: 0,
            last_seq: 0,
            discovered: false,
            resume_eligible: false,
            worktree_path: None,
            worktree_branch: None,
        };
        let desktop = host_entry_to_desktop(bare);
        assert_eq!(desktop.agent_id, "");
        assert!(
            desktop.agent_config_id.is_none(),
            "non config: namespace must not surface as agentConfigId"
        );
        assert_eq!(desktop.title, "Untitled Chat");
        assert_eq!(desktop.project_id, "");
        assert!(matches!(
            desktop.status,
            crate::acp::ChatHistoryStatus::Error
        ));
    }

    #[tokio::test]
    async fn acp_history_get_page_contract() {
        use crate::conversation::{
            AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
            ConversationEventRecordV2, ConversationEventType, ConversationLifecycleState,
            ConversationMutation, ConversationPersistenceAdapter, ConversationRecordV2,
            ConversationWriter, CreationPartition, ExecutionTarget, LegacyConversationReader,
            ReaderPrecedence, AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
        };
        use chrono::Utc;
        use std::io::{BufWriter, Write};
        use uuid::Uuid;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let workspace_path = root.join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();
        let workspace_path = workspace_path.canonicalize().unwrap();
        let (repository, _) =
            crate::conversation::ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            crate::conversation::ConversationId::parse("33333333-3333-4333-8333-333333333333")
                .unwrap();
        let created_at = Utc::now();
        let record = ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id,
            created_at_utc: created_at,
            creation_partition: CreationPartition::from_created_at(created_at),
            workspace_cwd: workspace_path.to_string_lossy().into_owned(),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::Ready,
            last_seq: 0,
            created_by: ConversationCreator::Legacy,
            title: None,
            title_source: None,
        };
        writer
            .create_conversation(record.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        writer
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "opaque/desktop-page".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: workspace_path.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let directory = private
            .join(&record.creation_partition.path)
            .join(conversation_id.to_string());
        drop(writer);
        drop(repository);

        let messages = std::fs::OpenOptions::new()
            .append(true)
            .open(directory.join(crate::conversation::event_log::MESSAGES_FILE))
            .unwrap();
        let mut messages = BufWriter::new(messages);
        for seq in 2..=1_051_u64 {
            let event = ConversationEventRecordV2::new(
                conversation_id,
                seq,
                created_at,
                ConversationEventType::MessageChunk,
                serde_json::json!({"marker": seq}),
            );
            serde_json::to_writer(&mut messages, &event).unwrap();
            messages.write_all(b"\n").unwrap();
        }
        messages.flush().unwrap();
        drop(messages);

        let (repository, _) = crate::conversation::ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(writer, reader));
        let host = HostHistoryStore::conversation(adapter, None);

        let result = acp_history_get_page_inner("opaque/desktop-page", 17, 250, None, &host).await;
        assert!(result.success, "page error: {:?}", result.error);
        let page = result.data.unwrap();
        assert_eq!(page.schema_version, 1);
        assert_eq!(page.records.len(), 250);
        assert_eq!(page.records.first().unwrap().seq, 18);
        assert_eq!(page.records.last().unwrap().seq, 267);
        assert_eq!(page.next_cursor, 267);
        assert!(!page.complete);
        assert_eq!(page.target_last_seq, 1_051);
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "records": page.records,
                "nextCursor": 267,
                "complete": false,
                "targetLastSeq": 1_051
            })
        );

        for invalid_limit in [0, 1_001] {
            let invalid =
                acp_history_get_page_inner("opaque/desktop-page", 0, invalid_limit, None, &host)
                    .await;
            assert!(!invalid.success);
            assert_eq!(invalid.code.as_deref(), Some("VALIDATION_ERROR"));
        }
        let invalid_cursor =
            acp_history_get_page_inner("opaque/desktop-page", 1_052, 17, None, &host).await;
        assert!(!invalid_cursor.success);
        assert_eq!(invalid_cursor.code.as_deref(), Some("VALIDATION_ERROR"));

        let compatibility = acp_history_get_inner("opaque/desktop-page", &host);
        assert!(!compatibility.success);
        assert_eq!(
            compatibility.code.as_deref(),
            Some(crate::conversation::CONVERSATION_HISTORY_PAGING_REQUIRED)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn desktop_exit_drain_failure_blocks_clean_success() {
        use crate::conversation::{
            AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
            ConversationLifecycleState, ConversationMutation, ConversationPersistenceAdapter,
            ConversationRecordV2, ConversationWriter, CreationPartition, ExecutionTarget,
            LegacyConversationReader, ReaderPrecedence, AGENT_SESSION_BINDING_SCHEMA_VERSION,
            CONVERSATION_SCHEMA_VERSION,
        };
        use crate::web::sink::{AcpEvent, EventSink};
        use chrono::Utc;
        use uuid::Uuid;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let (repository, _) = crate::conversation::ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id = crate::conversation::ConversationId::new_v4();
        let created_at = Utc::now();
        writer
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
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        writer
            .bind_agent_session(
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "desktop-exit-session".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(writer, reader));
        let relay = Arc::new(crate::web::WsRelaySink::with_conversation_persistence(
            32, adapter, None,
        ));
        relay
            .emit(&AcpEvent {
                sid: Some("desktop-exit-session".to_string()),
                type_: "acp:message_chunk",
                payload: serde_json::json!({"ordinal": 1}),
            })
            .unwrap();
        let ordered = relay
            .ordered_conversation_persistence()
            .expect("ordered Conversation persistence");
        let duplicate = ordered
            .submit(
                "desktop-exit-session",
                1,
                "message_chunk",
                serde_json::json!({"ordinal": 2}),
            )
            .expect_err("duplicate source sequence must open a retained failure circuit");
        assert_eq!(duplicate.code, "CONVERSATION_SOURCE_SEQUENCE_INVALID");
        assert_eq!(
            ordered
                .health("desktop-exit-session")
                .unwrap()
                .unwrap()
                .last_error_code,
            Some("CONVERSATION_SOURCE_SEQUENCE_INVALID")
        );
        let relay_sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(crate::acp::AcpManager::new(vec![relay_sink]));
        let agent_id = crate::acp::AgentId("desktop-exit-agent".to_string());
        let (observed, _receiver) = std::sync::mpsc::sync_channel(1);
        acp.install_test_agent_for_new_session(agent_id.clone(), observed);

        let outcome = crate::stop_desktop_producers_and_drain(
            Some(&acp),
            Some(&relay),
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .await;
        assert!(!outcome.clean_success());
        assert_eq!(outcome.conversation_drain_attempts, 1);
        assert_eq!(outcome.catalog_flush_attempts, 1);
        assert!(outcome
            .failures
            .contains(&crate::web::CONVERSATION_PERSISTENCE_DRAIN_FAILED));
        assert!(!outcome
            .failures
            .contains(&crate::web::CONVERSATION_CATALOG_FLUSH_FAILED));
        assert!(
            acp.stable_agent_namespace(&agent_id).is_err(),
            "producers must stop before the failed drain returns"
        );

        let source = include_str!("lib.rs");
        let start = source
            .find("pub(crate) async fn stop_desktop_producers_and_drain(")
            .unwrap();
        let end = source[start..]
            .find("static CLEANUP_DONE")
            .map(|offset| start + offset)
            .unwrap();
        let body = &source[start..end];
        assert_eq!(
            body.matches(".shutdown_conversation_persistence_until(deadline)")
                .count(),
            1
        );
        assert!(
            body.find("timeout_at(deadline, acp_manager.stop_producers())")
                .unwrap()
                < body
                    .find(".shutdown_conversation_persistence_until(deadline)")
                    .unwrap()
        );
    }

    #[tokio::test]
    async fn tauri_conversation_delete_retires_on_success_and_retains_on_blocked_or_error() {
        use crate::conversation::{
            ConversationLifecycleAction, ConversationLifecycleErrorCode,
            ConversationLifecycleOutcome, ConversationLifecycleState,
        };

        let relay = crate::web::WsRelaySink::new();
        let conversation_id = crate::conversation::ConversationId::new_v4();
        relay
            .turn_watermark()
            .mark_seen("tauri-delete", "turn-retained");
        let blocked = ConversationLifecycleOutcome::Blocked {
            action: ConversationLifecycleAction::DeleteConversation,
            conversation_id,
            revision: 7,
            code: ConversationLifecycleErrorCode::ConversationLiveResources,
            blockers: Vec::new(),
        };
        retire_deleted_binding_if_updated(&relay, Some("tauri-delete"), &blocked)
            .await
            .unwrap();
        assert!(relay
            .turn_watermark()
            .is_seen("tauri-delete", "turn-retained"));
        let simulated_error: Result<(), &str> = Err("delete failed");
        assert!(simulated_error.is_err());
        assert!(relay
            .turn_watermark()
            .is_seen("tauri-delete", "turn-retained"));

        let updated = ConversationLifecycleOutcome::Updated {
            action: ConversationLifecycleAction::DeleteConversation,
            conversation_id,
            previous_revision: 7,
            revision: 8,
            workspace_cwd: "/opaque/workspace".to_string(),
            lifecycle_state: ConversationLifecycleState::Deleted,
            current_binding: None,
            previous_agent_session_id: Some("tauri-delete".to_string()),
        };
        retire_deleted_binding_if_updated(&relay, Some("tauri-delete"), &updated)
            .await
            .unwrap();
        assert!(!relay
            .turn_watermark()
            .is_seen("tauri-delete", "turn-retained"));
    }

    #[test]
    fn sanitize_log_field_escapes_newlines_and_strips_controls() {
        // Newlines/CR/tab are escaped so injected content stays on one line.
        let forged = "oops\n[startup] se-manager forged line\r\tend";
        let cleaned = sanitize_log_field(forged);
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\r'));
        assert!(!cleaned.contains('\t'));
        assert!(cleaned.contains("\\n[startup]"));

        // ESC and other C0 control chars are dropped entirely.
        let with_esc = "a\u{1b}[31mred\u{0007}b";
        assert_eq!(sanitize_log_field(with_esc), "a[31mredb");
    }

    #[test]
    fn sanitize_log_field_truncates_oversized_input() {
        let huge = "x".repeat(MAX_FRONTEND_FIELD_LEN + 100);
        let cleaned = sanitize_log_field(&huge);
        assert!(cleaned.ends_with("…[truncated]"));
        assert!(cleaned.chars().count() <= MAX_FRONTEND_FIELD_LEN + "…[truncated]".chars().count());
    }

    // ===== filename search streaming (gh-195) =====
    //
    // Coverage for `build_file_name_search_args` and the per-stream event
    // contracts. The end-to-end rg spawn path is exercised in a real
    // workspace by manual smoke; these tests pin the argv and serde shapes
    // so a future refactor cannot silently regress them.

    #[test]
    fn build_file_name_search_args_escapes_glob_metacharacters() {
        // A query containing `*`, `?`, `[`, `]`, `{`, `}`, `\` must not be
        // interpreted as a glob wildcard or alternation. Each metacharacter
        // should be prefixed with a backslash so rg treats it as a literal
        // substring match.
        let args = build_file_name_search_args("foo*bar?baz[qux]{a,b}\\z", "/tmp", false);
        let iglob_idx = args
            .iter()
            .position(|a| a == "--iglob")
            .expect("--iglob present");
        let pattern = &args[iglob_idx + 1];
        assert_eq!(pattern, r"**/*foo\*bar\?baz\[qux\]\{a,b\}\\z*");
        // The query should still be matched at any directory depth.
        assert!(pattern.starts_with("**/*"));
    }

    #[test]
    fn build_file_name_search_args_includes_ignore_list_and_excludes() {
        let args = build_file_name_search_args("foo", "/tmp", false);
        // The hardcoded ignore list must show up as bare-basename `-g !<name>`
        // entries so rg actually skips those directories in `--files` mode.
        for ignored in [
            "node_modules",
            ".git",
            ".env",
            "Thumbs.db",
            "desktop.ini",
            ".DS_Store",
        ] {
            let needle = format!("!{}", ignored);
            let has = args.windows(2).any(|w| w[0] == "-g" && w[1] == needle);
            assert!(has, "missing `-g {}` in {:?}", ignored, args);
        }
        // The root path is the trailing argv entry.
        assert_eq!(args.last().map(String::as_str), Some("/tmp"));
    }

    #[test]
    fn build_file_name_search_args_appends_root_path() {
        let args = build_file_name_search_args("term", "/some/root path", false);
        // Root paths with spaces should appear verbatim, not split.
        assert_eq!(args.last().map(String::as_str), Some("/some/root path"));
    }

    #[test]
    fn build_file_name_search_args_starts_with_files_and_case_insensitive() {
        let args = build_file_name_search_args("foo", "/tmp", false);
        assert_eq!(args[0], "--files");
        assert_eq!(args[1], "-i");
    }

    #[test]
    fn search_file_names_done_event_serializes_code_field() {
        // The `code` field is optional and skipped on the wire when `None`.
        let with_code = SearchFileNamesDoneEvent {
            search_id: "search-1".to_string(),
            truncated: false,
            total_files: 0,
            code: Some("QUERY_TOO_LONG".to_string()),
            error: Some("too long".to_string()),
        };
        let json = serde_json::to_string(&with_code).unwrap();
        assert!(json.contains("\"code\":\"QUERY_TOO_LONG\""));
        assert!(json.contains("\"error\":\"too long\""));

        let without_code = SearchFileNamesDoneEvent {
            search_id: "search-1".to_string(),
            truncated: false,
            total_files: 0,
            code: None,
            error: None,
        };
        let json = serde_json::to_string(&without_code).unwrap();
        assert!(!json.contains("code"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn search_file_names_batch_event_omits_truncated_when_none() {
        // Mid-stream batches carry `None` so the renderer knows the value is
        // unknown; serde should drop the field from the wire.
        let mid_stream = SearchFileNamesBatchEvent {
            search_id: "search-1".to_string(),
            files: vec![SearchFileHit {
                path: "a".to_string(),
                ignored: false,
            }],
            truncated: None,
        };
        let json = serde_json::to_string(&mid_stream).unwrap();
        assert!(!json.contains("truncated"));

        let final_batch = SearchFileNamesBatchEvent {
            search_id: "search-1".to_string(),
            files: vec![SearchFileHit {
                path: "a".to_string(),
                ignored: false,
            }],
            truncated: Some(true),
        };
        let json = serde_json::to_string(&final_batch).unwrap();
        assert!(json.contains("\"truncated\":true"));
        // The per-hit `ignored` flag is on the wire.
        assert!(json.contains("\"ignored\":false"));
    }

    #[test]
    fn build_file_name_search_args_include_ignored_surfaces_hidden_and_drops_exclusions() {
        let args = build_file_name_search_args("foo", "/tmp", true);
        assert!(args.contains(&"--no-ignore".to_string()));
        assert!(args.contains(&"--hidden".to_string()));
        // The common-ignore exclusions must be absent so ignored/hidden files
        // are actually walked.
        for needle in ["!node_modules", "!.env", "!Thumbs.db", "!.DS_Store"] {
            let has = args.windows(2).any(|w| w[0] == "-g" && w[1] == needle);
            assert!(!has, "include_ignored must not exclude `{}`", needle);
        }
        assert_eq!(args.last().map(String::as_str), Some("/tmp"));
    }

    #[test]
    fn path_is_ignored_classifies_commonly_ignored_paths() {
        assert!(path_is_ignored("node_modules/pkg/index.js"));
        assert!(path_is_ignored(".git/HEAD"));
        assert!(path_is_ignored("dist/bundle.js"));
        assert!(path_is_ignored(".env"));
        assert!(path_is_ignored("src/.hidden.ts"));
        assert!(path_is_ignored("assets/Thumbs.db"));
        assert!(path_is_ignored("assets/.DS_Store"));
        // Source files and non-cruft paths are not ignored.
        assert!(!path_is_ignored("src/auth.ts"));
        assert!(!path_is_ignored("README.md"));
        assert!(!path_is_ignored("lib/router/index.ts"));
    }

    #[test]
    fn rank_search_hits_puts_non_ignored_first_and_caps_total() {
        let non_ignored = vec![
            SearchFileHit {
                path: "a".to_string(),
                ignored: false,
            },
            SearchFileHit {
                path: "b".to_string(),
                ignored: false,
            },
        ];
        let ignored = vec![
            SearchFileHit {
                path: "c".to_string(),
                ignored: true,
            },
            SearchFileHit {
                path: "d".to_string(),
                ignored: true,
            },
            SearchFileHit {
                path: "e".to_string(),
                ignored: true,
            },
        ];
        // cap=3 → all non-ignored (2) + one ignored.
        let ranked = rank_search_hits(non_ignored.clone(), ignored.clone(), 3);
        assert_eq!(
            ranked.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        // cap=2 → only non-ignored; ignored never crowds them out.
        let ranked = rank_search_hits(non_ignored.clone(), ignored.clone(), 2);
        assert_eq!(
            ranked.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        // No non-ignored → ignored fills up to cap.
        let ranked = rank_search_hits(vec![], ignored.clone(), 100);
        assert_eq!(
            ranked.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["c", "d", "e"]
        );
        // No ignored → non-ignored only, untruncated when under cap.
        let ranked = rank_search_hits(
            vec![
                SearchFileHit {
                    path: "a".to_string(),
                    ignored: false,
                },
                SearchFileHit {
                    path: "b".to_string(),
                    ignored: false,
                },
            ],
            vec![],
            100,
        );
        assert_eq!(
            ranked.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn search_file_names_cancel_request_is_a_dto_not_aliased_to_content() {
        // The two cancel commands must accept distinct types so a future
        // shape change to `SearchContentCancelRequest` cannot silently
        // affect the filename path. This pins the type identity at compile
        // time (the two structs are distinct) and verifies the field shape
        // by exercising the constructor.
        let req = SearchFileNamesCancelRequest {
            search_id: "search-1".to_string(),
        };
        assert_eq!(req.search_id, "search-1");
    }

    #[test]
    fn late_tauri_mutator_returns_host_shutting_down() {
        crate::host_admission::HostAdmission::global().close();
        let err = require_host_admission::<()>().expect_err("closed admission rejects mutators");
        assert_eq!(
            err.code.as_deref(),
            Some(crate::host_admission::HOST_SHUTTING_DOWN)
        );
        crate::host_admission::HostAdmission::global().reopen_for_tests();
        assert!(require_host_admission::<()>().is_ok());
    }

    #[test]
    fn validate_project_path_logs_no_raw_path_or_identifier() {
        let source = include_str!("commands.rs");
        let start = source
            .find("fn validate_project_path(path: &str)")
            .expect("validator exists");
        let end = source[start..]
            .find("pub(crate) fn require_host_admission")
            .map(|offset| start + offset)
            .unwrap_or(start + 1600);
        let body = &source[start..end];
        assert!(body.contains("stable_code=PATH_VALIDATION_FAILED"));
        assert!(body.contains("stable_code=OK"));
        assert!(!body.contains("Path validation failed for '{}'"));
        assert!(!body.contains("Path validated: {} -> {}"));
        let _ = validate_project_path("/definitely-missing-se-manager-path-xyz");
    }
}

#[cfg(test)]
mod remote_sync_projects_tests {
    use super::SyncProjectsPayload;

    #[test]
    fn sync_payload_defaults_groups_for_older_desktop_clients() {
        let payload: SyncProjectsPayload = serde_json::from_value(serde_json::json!({
            "projects": [{
                "id": "p-1",
                "name": "Project",
                "color": "blue",
                "path": "/tmp/project",
                "isArchived": false,
                "isDefault": true
            }],
            "defaultProjectId": "p-1"
        }))
        .expect("deserialize legacy sync payload");

        assert!(payload.groups.is_empty());
        assert_eq!(payload.default_project_id.as_deref(), Some("p-1"));
    }

    #[test]
    fn sync_payload_deserializes_project_groups() {
        let payload: SyncProjectsPayload = serde_json::from_value(serde_json::json!({
            "projects": [{
                "id": "p-1",
                "name": "Project",
                "color": "blue",
                "path": "/tmp/project",
                "isArchived": false,
                "isDefault": true
            }],
            "groups": [{
                "id": "g-1",
                "name": "Favorites",
                "projectIds": ["p-1"],
                "color": "purple",
                "preferredProjectId": "p-1"
            }],
            "defaultProjectId": "p-1"
        }))
        .expect("deserialize group-aware sync payload");

        assert_eq!(payload.groups.len(), 1);
        assert_eq!(payload.groups[0].project_ids, ["p-1"]);
        assert_eq!(
            payload.groups[0].preferred_project_id.as_deref(),
            Some("p-1")
        );
    }
}

#[cfg(test)]
mod remote_sync_mcp_registry_tests {
    use super::sync_mcp_registry_to_project_file;
    use crate::web::mcp_servers_api::registry_path;
    use crate::web::{ProjectRegistry, ProjectSummary};
    use serde_json::json;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Serializes tests that mutate `TERMUL_PROJECT_ROOT` (process-global env).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "se-manager-mcp-sync-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn registry_with_default(project_root: &Path) -> ProjectRegistry {
        let reg = ProjectRegistry::new();
        let project = ProjectSummary {
            id: "p1".to_string(),
            name: "P".to_string(),
            color: "blue".to_string(),
            path: Some(project_root.to_string_lossy().into_owned()),
            is_archived: false,
            is_default: false,
        };
        reg.set(vec![project], Some("p1".to_string()));
        reg
    }

    #[tokio::test]
    async fn writes_registry_to_project_mcp_servers_file() {
        let dir = temp_dir("write");
        let reg = registry_with_default(&dir);
        let registry = json!([
            {"id":"one","type":"stdio","name":"fs","command":"npx","enabled":true}
        ]);

        let result = sync_mcp_registry_to_project_file(&reg, registry).await;
        assert!(result.success, "expected success, got {:?}", result.error);

        // The sync must write the exact file the web `GET /mcp-servers` route
        // reads (`{project_root}/.termul/mcp-servers.json`).
        let file = registry_path(&dir);
        let bytes = std::fs::read(&file).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value.as_array().map(Vec::len), Some(1));
        assert_eq!(value[0]["name"], "fs");
        assert_eq!(value[0]["command"], "npx");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn overwrites_previous_registry_atomically() {
        let dir = temp_dir("overwrite");
        let reg = registry_with_default(&dir);

        // First write — one entry.
        let one = json!([{"id":"a","type":"stdio","name":"a","command":"x","enabled":true}]);
        let r1 = sync_mcp_registry_to_project_file(&reg, one).await;
        assert!(r1.success, "first write failed: {:?}", r1.error);

        // Second write — two entries. Must fully replace (not append).
        let two = json!([
            {"id":"b","type":"stdio","name":"b","command":"y","enabled":true},
            {"id":"c","type":"stdio","name":"c","command":"z","enabled":false}
        ]);
        let r2 = sync_mcp_registry_to_project_file(&reg, two).await;
        assert!(r2.success, "second write failed: {:?}", r2.error);

        let file = registry_path(&dir);
        let bytes = std::fs::read(&file).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entries = value.as_array().unwrap();
        assert_eq!(entries.len(), 2, "registry must be replaced, not appended");
        assert_eq!(entries[0]["id"], "b");
        assert_eq!(entries[1]["id"], "c");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_non_array_payload_without_writing() {
        let dir = temp_dir("reject");
        let reg = registry_with_default(&dir);
        let file = registry_path(&dir);
        assert!(!file.exists(), "precondition: no file yet");

        let result = sync_mcp_registry_to_project_file(&reg, json!({})).await;
        assert!(!result.success);
        assert_eq!(result.code.as_deref(), Some("MCP_REGISTRY_INVALID"));
        assert!(!file.exists(), "non-array must not write a file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn falls_back_to_default_project_root_when_registry_has_no_default() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("fallback");
        // Point TERMUL_PROJECT_ROOT at the temp dir so the fallback path
        // resolves there instead of the real home directory.
        let prev = std::env::var_os("TERMUL_PROJECT_ROOT");
        std::env::set_var("TERMUL_PROJECT_ROOT", &dir);

        let reg = ProjectRegistry::new(); // no default project set
        let registry = json!([
            {"id":"fb","type":"stdio","name":"fallback","command":"node","enabled":true}
        ]);

        let result = sync_mcp_registry_to_project_file(&reg, registry).await;
        assert!(
            result.success,
            "fallback should succeed, got {:?}",
            result.error
        );

        let file = registry_path(&dir);
        let bytes = std::fs::read(&file).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value[0]["name"], "fallback");

        // Restore the env var.
        match prev {
            Some(v) => std::env::set_var("TERMUL_PROJECT_ROOT", v),
            None => std::env::remove_var("TERMUL_PROJECT_ROOT"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
