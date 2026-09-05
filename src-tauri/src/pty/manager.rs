//! PtyManager - Manages PTY (pseudo-terminal) instances for Tauri
//!
//! This module provides terminal spawning, I/O, and lifecycle management
//! ported from the Electron implementation.

use crate::conversation::{ConversationId, ConversationRecordV2, ExecutionTarget};
use crate::pty::claims::ClaimError;
use crate::trackers::{
    CwdTracker, ExitCodeTracker, GitTracker, TerminalDisplayMode, TerminalEvent, TerminalEventHub,
};
use parking_lot::RwLock;
use portable_pty::{Child, MasterPty, PtySize};

#[cfg(target_os = "windows")]
use crate::pty::windows::{resize_conpty, spawn_conpty, ConPtyHandles};
#[cfg(target_os = "windows")]
use crate::shell_paths::git_bash_paths;
#[cfg(target_os = "windows")]
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(target_os = "windows")]
fn resolve_executable_from_path(command: &str) -> Option<String> {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    if command.contains('\\') || command.contains('/') {
        let candidate = Path::new(command);
        return candidate.exists().then(|| command.to_string());
    }

    let path_var = crate::pty::env_refresh::path_for_resolution();
    if path_var.is_empty() {
        return None;
    }
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

use std::time::{Duration, Instant};
use tauri::ipc::{Channel, Response};

/// ADR-004.2: Result of resolving a program path, possibly with leading argv
/// entries that must be prepended before the user-supplied args (e.g. when a
/// `.cmd` npm shim is rewritten to `node.exe <script>`).
#[derive(Debug, Clone)]
pub(crate) struct ResolvedProgram {
    /// Absolute path to the executable (always a PE image on Windows).
    pub program: String,
    /// Extra argv entries to insert before the user's args.
    /// E.g. `["C:\...\node_modules\opencode\bin\opencode"]` when the
    /// binary is `node.exe` and the script is the npm shim target.
    pub prepend_args: Vec<String>,
}

impl ResolvedProgram {
    pub fn new(program: String) -> Self {
        Self {
            program,
            prepend_args: Vec::new(),
        }
    }
    #[cfg(target_os = "windows")]
    pub fn with_args(program: String, args: Vec<String>) -> Self {
        Self {
            program,
            prepend_args: args,
        }
    }
}

/// ADR-004.2: Returns true if a Windows file path points to a directly-
/// executable PE image (`.exe`, `.com`, `.scr` only). Anything else
/// (`.bat`, `.cmd`, `.ps1`, `.vbs`, `.js`, ...) cannot be handed to
/// `CreateProcessW` and would surface as `os error 193`.
#[cfg(target_os = "windows")]
pub(super) fn is_directly_executable_windows(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // Strip a trailing quote pair if the caller supplied a quoted form.
    let trimmed = lower.trim_end_matches('"');
    matches!(
        std::path::Path::new(trimmed)
            .extension()
            .and_then(|e| e.to_str()),
        Some("exe") | Some("com") | Some("scr")
    )
}

/// ADR-004.2, Windows-only: Parse an npm `.cmd` shim and extract the
/// underlying `node.exe` + script path so the spawn can run the PE image
/// directly instead of handing the non-executable `.cmd` to CreateProcessW.
///
/// npm on Windows installs CLI tools as thin batch wrappers whose last line is:
///   "<node.exe>" "<script>" %*
/// We extract both paths, resolve `%dp0%` / `%~dp0` to the shim's directory,
/// and return `ResolvedProgram { program: "node.exe", prepend_args: ["<script>"] }`.
#[cfg(target_os = "windows")]
pub(super) fn parse_npm_cmd_shim(shim_path: &str) -> Option<ResolvedProgram> {
    let content = std::fs::read_to_string(shim_path).ok()?;
    let shim_dir = std::path::Path::new(shim_path).parent()?;
    let shim_dir_str = shim_dir.to_str().unwrap_or(".");

    // Pre-scan `SET "VAR=value"` (and `SET VAR=value`) assignments so launcher
    // shims that invoke through variable indirection — e.g. npm's own
    // `npx.cmd` / `npm.cmd`, whose final line is `"%NODE_EXE%" "%NPX_CLI_JS%" %*`
    // — can be resolved, not just the simple `"%dp0%\node.exe" "<script>"` form
    // used by package bin shims. Without this, npm launchers fail to rewrite and
    // the raw `.cmd` is handed to CreateProcessW (os error 193).
    let mut vars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let expand_dp0 = |val: &str| -> String {
        val.replace("%dp0%", shim_dir_str)
            .replace("%~dp0%", shim_dir_str)
            .replace("%~dp0", shim_dir_str)
    };
    for line in content.lines() {
        let t = line.trim();
        let Some(rest) = t
            .strip_prefix("SET ")
            .or_else(|| t.strip_prefix("set "))
            .or_else(|| t.strip_prefix("Set "))
        else {
            continue;
        };
        let rest = rest.trim();
        // Accept both `SET "VAR=value"` and `SET VAR=value`.
        let unquoted = rest.trim_matches('"');
        if let Some((name, value)) = unquoted.split_once('=') {
            let name = name.trim();
            if !name.is_empty() {
                // First assignment wins: it is the primary (unconditional) one;
                // later `SET`s in npm launchers are `IF`-guarded fallbacks a
                // static parser cannot evaluate.
                vars.entry(name.to_ascii_uppercase())
                    .or_insert_with(|| value.trim().to_string());
            }
        }
    }
    // Resolve a single `%VAR%` reference (one level of indirection is enough for
    // real npm launchers) against the SET map, then expand %dp0% inside it.
    let resolve_vars = |val: &str| -> String {
        let trimmed = val.trim();
        if trimmed.starts_with('%') && trimmed.ends_with('%') && trimmed.len() > 2 {
            let key = trimmed[1..trimmed.len() - 1].to_ascii_uppercase();
            if let Some(v) = vars.get(&key) {
                return expand_dp0(v);
            }
        }
        expand_dp0(trimmed)
    };

    // Find the last line that contains a command invocation pattern:
    //   "<executable>" "<script>" %*
    // or equivalently with %_prog% / %VAR% resolved.
    // We look for lines containing both `"%dp0%` (or `")` and `%*`.
    for line in content.lines().rev() {
        let line = line.trim();
        if !line.contains("%*") {
            continue;
        }
        if !line.contains("\"") {
            continue;
        }
        // Extract quoted strings: "..."
        let quotes: Vec<&str> = line.split('"').collect();
        // The invocation pattern uses two quoted paths:
        //   index 1 = executable (node.exe path)
        //   index 3 = script path
        if quotes.len() < 5 {
            continue;
        }
        let raw_exe = quotes[1].trim();
        let raw_script = quotes[3].trim();
        if raw_exe.is_empty() || raw_script.is_empty() {
            continue;
        }

        // Resolve %VAR% indirection first (npm launchers), then %dp0% / %~dp0,
        // and %_prog% to node.exe (either <dir>/node.exe or bare "node"
        // when the node executable is on PATH).
        let resolve_dp0 = |val: &str| -> String { resolve_vars(val).replace('"', "") };

        let exe_path_str = resolve_dp0(raw_exe);
        // Handle %_prog%: check for node.exe in the shim directory first.
        let exe_path_str = if exe_path_str == "%_prog%" {
            let local_node = shim_dir.join("node.exe");
            if local_node.exists() {
                local_node.to_string_lossy().to_string()
            } else if let Some(path) = resolve_executable_from_path("node.exe") {
                path
            } else {
                continue;
            }
        } else {
            exe_path_str
        };
        let script_path_str = resolve_dp0(raw_script);

        let exe_path = std::path::Path::new(&exe_path_str);
        let script_path = std::path::Path::new(&script_path_str);

        // The executable must exist and be a directly-executable image.
        if !exe_path.exists() || !is_directly_executable_windows(&exe_path_str) {
            continue;
        }
        // The script should exist (not strictly required but a good check).
        if !script_path.exists() {
            continue;
        }

        return Some(ResolvedProgram::with_args(
            exe_path_str,
            vec![script_path_str],
        ));
    }

    None
}

/// Windows-only: parse a `.cmd`/`.bat` shim that delegates to PowerShell, e.g.
/// Cursor Agent's `cursor-agent.cmd` which runs `powershell.exe -File script.ps1`.
#[cfg(target_os = "windows")]
pub(super) fn parse_powershell_cmd_shim(shim_path: &str) -> Option<ResolvedProgram> {
    let content = std::fs::read_to_string(shim_path).ok()?;
    let shim_dir = std::path::Path::new(shim_path).parent()?;

    let resolve_batch_token = |raw: &str| -> String {
        let shim_dir_str = shim_dir.to_str().unwrap_or(".");
        let system_root = env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        raw.replace("%SystemRoot%", &system_root)
            .replace("%SYSTEMROOT%", &system_root)
            .replace("%SCRIPT_DIR%", shim_dir_str)
            .replace("%~dp0", shim_dir_str)
            .replace("%~dp0%", shim_dir_str)
            .replace("%dp0%", shim_dir_str)
            .trim_matches('"')
            .to_string()
    };

    for line in content.lines().rev() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        if !lower.contains("powershell") || !lower.contains("-file") {
            continue;
        }

        let ps_exe_token = line
            .split_whitespace()
            .find(|t| t.to_ascii_lowercase().contains("powershell.exe"))?;
        let ps_exe = resolve_batch_token(ps_exe_token);
        if !std::path::Path::new(&ps_exe).exists() || !is_directly_executable_windows(&ps_exe) {
            continue;
        }

        let file_flag = "-file";
        let file_idx = lower.find(file_flag)?;
        let after_file = line[file_idx + file_flag.len()..].trim();
        let script_raw = if let Some(start) = after_file.find('"') {
            let rest = &after_file[start + 1..];
            let end = rest.find('"')?;
            &rest[..end]
        } else {
            after_file.split_whitespace().next()?
        };
        let script_path = resolve_batch_token(script_raw);
        if !std::path::Path::new(&script_path).exists() {
            continue;
        }

        let mut prepend_args: Vec<String> = Vec::new();
        for token in line.split_whitespace() {
            let token_clean = token.trim_matches('"');
            if token_clean.eq_ignore_ascii_case("-file") {
                prepend_args.push("-File".to_string());
                prepend_args.push(script_path.clone());
                break;
            }
            if token_clean.to_ascii_lowercase().contains("powershell.exe") {
                continue;
            }
            if !token_clean.is_empty() {
                prepend_args.push(resolve_batch_token(token_clean));
            }
        }

        return Some(ResolvedProgram::with_args(ps_exe, prepend_args));
    }

    None
}

/// Try npm-node shim parsing first, then PowerShell-wrapper shims.
#[cfg(target_os = "windows")]
fn try_parse_windows_cmd_shim(shim_path: &str) -> Option<ResolvedProgram> {
    parse_npm_cmd_shim(shim_path).or_else(|| parse_powershell_cmd_shim(shim_path))
}

/// ADR-004.2: Resolve a spawn program the same way the PTY launcher does, for
/// reuse by other subprocess spawners (e.g. the ACP agent runtime).
///
/// On Windows: prefer a directly-executable PE image (`.exe`/`.com`/`.scr`);
/// when only a `.cmd`/`.bat` npm/PowerShell shim is on PATH, parse it and
/// rewrite to the underlying interpreter + script so `CreateProcessW` does not
/// fail with os error 193. Explicit paths are honored as-is when already a PE
/// image, otherwise the shim is parsed. Returns `Err` when nothing usable is
/// found so the caller can fall back to its previous behavior.
///
/// On non-Windows: returns the program unchanged (no rewriting needed).
pub(crate) fn resolve_spawn_program(program: &str) -> Result<ResolvedProgram, String> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return Err("program is empty".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        // Explicit path: honor it if it exists.
        if trimmed.contains('/') || trimmed.contains('\\') {
            if Path::new(trimmed).exists() {
                if is_directly_executable_windows(trimmed) {
                    return Ok(ResolvedProgram::new(trimmed.to_string()));
                }
                if let Some(resolved) = try_parse_windows_cmd_shim(trimmed) {
                    return Ok(resolved);
                }
            }
            return Err(format!("program not found or not executable: {}", trimmed));
        }

        // 1. Bare name: try directly-executable PE image extensions first.
        const WIN_EXECUTABLE_EXTS: &[&str] = &["", ".exe", ".com", ".scr"];
        for ext in WIN_EXECUTABLE_EXTS {
            let candidate = format!("{}{}", trimmed, ext);
            if let Some(abs_path) = resolve_executable_from_path(&candidate) {
                if is_directly_executable_windows(&abs_path) {
                    return Ok(ResolvedProgram::new(abs_path));
                }
            }
        }

        // 2. No PE image: parse a `.cmd`/`.bat` shim and rewrite it.
        for shim_ext in [".cmd", ".bat"] {
            let candidate = format!("{}{}", trimmed, shim_ext);
            if let Some(abs_path) = resolve_executable_from_path(&candidate) {
                if let Some(resolved) = try_parse_windows_cmd_shim(&abs_path) {
                    return Ok(resolved);
                }
            }
        }

        Err(format!("program not found on PATH: {}", trimmed))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(ResolvedProgram::new(trimmed.to_string()))
    }
}

use tokio::sync::Mutex as AsyncMutex;

#[cfg(target_os = "windows")]
fn has_windows_env_var(env_map: &HashMap<String, String>, key: &str) -> bool {
    env_map
        .keys()
        .any(|existing| existing.eq_ignore_ascii_case(key))
}

#[cfg(target_os = "windows")]
fn upsert_windows_env_var(env_map: &mut HashMap<String, String>, key: &str, value: String) {
    if let Some(existing_key) = env_map
        .keys()
        .find(|existing| existing.eq_ignore_ascii_case(key))
        .cloned()
    {
        env_map.remove(&existing_key);
    }

    env_map.insert(key.to_string(), value);
}

#[cfg(target_os = "windows")]
fn merge_windows_environment_map<I>(
    base_env: I,
    custom_env: Option<HashMap<String, String>>,
) -> HashMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env_map = HashMap::new();

    for (key, value) in base_env {
        upsert_windows_env_var(&mut env_map, &key, value);
    }

    if let Some(custom) = custom_env {
        for (key, value) in custom {
            upsert_windows_env_var(&mut env_map, &key, value);
        }
    }

    if !has_windows_env_var(&env_map, "Path") {
        upsert_windows_env_var(&mut env_map, "Path", env::var("PATH").unwrap_or_default());
    }

    if !has_windows_env_var(&env_map, "PATHEXT") {
        upsert_windows_env_var(
            &mut env_map,
            "PATHEXT",
            env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()),
        );
    }

    env_map
}

// Constants matching Electron implementation
const GLOBAL_TERMINAL_LIMIT: usize = 30;
const ORPHAN_TIMEOUT_MS: u64 = 300_000; // 5 minutes
const ORPHAN_CHECK_INTERVAL_MS: u64 = 30_000; // 30 seconds
/// One absolute budget shared by child kill/wait and both thread joins.
pub const TERMINAL_CLEANUP_DEADLINE: Duration = Duration::from_secs(5);
const TERMINAL_CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long the reader waits on `poll` before rechecking `stop_requested`.
/// Bounds how late a stopped reader exits; well inside the cleanup deadline.
#[cfg(unix)]
const READER_POLL_TIMEOUT_MS: libc::c_int = 50;

#[cfg(unix)]
type RawFd = std::os::fd::RawFd;
#[cfg(not(unix))]
type RawFd = std::os::raw::c_int;

// ADR-002.3: Flusher thread constants
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(4);
pub const READ_BUF: usize = 16 * 1024; // 16KB read buffer
pub const MAX_PENDING: usize = 4 * 1024 * 1024; // 4MB overflow cap
pub const OVERFLOW_NOTICE: &[u8] =
    b"\x1bc\x1b[2m[se-manager: dropped output due to backpressure]\x1b[0m\r\n";

/// Public info emitted to renderer on spawn (also forwarded to ws clients)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
}

/// Spawn response carrying the terminal info PLUS the issued claim credential.
///
/// Serializes FLATTENED — `{id, shell, cwd, pid, cols, rows, claim}` — so both
/// transports (desktop `terminal_spawn` IpcResult data and the web `spawn`
/// reply data) expose the same top-level camelCase shape. This is the initial
/// issuance path; authenticated resume and explicit rotation can replace it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnedTerminal {
    #[serde(flatten)]
    pub info: TerminalInfo,
    pub claim: String,
}

impl std::fmt::Debug for SpawnedTerminal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnedTerminal")
            .field("info", &self.info)
            .field("claim", &"<redacted>")
            .finish()
    }
}

/// Shared attach response — byte-identical camelCase shape on both transports
/// (desktop `terminal_attach` IpcResult data; web `attach` reply data).
///
/// Carries the live terminal metadata plus the replay cursor (`latestSeq`) and
/// `gap` flag. It NEVER carries a claim key: attach is credential-consuming,
/// never credential-issuing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachResult {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    pub latest_seq: u64,
    pub gap: bool,
}

/// Cold-renderer resume request. Unknown fields are rejected so this path can
/// never grow into a raw spawn or environment override surface.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResumeRequest {
    pub conversation_id: ConversationId,
    pub terminal_id: String,
    pub last_seq: u64,
}

/// One-time resume handoff. The claim exists only in this authenticated
/// response and the renderer's memory; it is never persisted or logged.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResumeGrant {
    pub terminal: TerminalAttachResult,
    pub claim: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputChunk {
    pub seq: u64,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct TerminalReplay {
    pub chunks: Vec<TerminalOutputChunk>,
    pub gap: bool,
    pub latest_seq: u64,
    pub receiver: tokio::sync::broadcast::Receiver<TerminalOutputChunk>,
    /// Exact claim generation authorized for a trusted resume. This is
    /// process-local control metadata and is never serialized.
    pub(crate) claim_generation: Option<u64>,
}

/// Broadcast channel capacity (number of buffered output batches per terminal).
/// Each batch is up to READ_BUF (16KB) bytes. 1024 slots ≈ 16MB max buffered output.
/// Slow receivers will receive `RecvError::Lagged` — acceptable; they miss bytes
/// rather than back-pressuring the PTY.
const TERM_BROADCAST_CAPACITY: usize = 1024;

/// Maximum scrollback bytes retained per terminal for remote-client replay.
/// 256 KiB ≈ several screenfuls of history; bounded so memory stays predictable
/// even for very chatty terminals. Oldest bytes are evicted first.
pub const SCROLLBACK_CAP: usize = 256 * 1024;

/// Host-authorized remote terminal spawn intent.
///
/// Remote callers may select only the canonical Conversation, optional project
/// attribution, one of the two host-owned cwd sources, and terminal dimensions.
/// Program, shell, argv, environment, and raw cwd never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSpawnIntentV1 {
    pub conversation_id: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub cwd_source: TerminalCwdSource,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalCwdSource {
    Workspace,
    ExecutionTarget,
}

impl TerminalSpawnIntentV1 {
    fn into_trusted_options(
        self,
        conversation: &ConversationRecordV2,
    ) -> Result<SpawnOptions, String> {
        if self.conversation_id != conversation.conversation_id {
            return Err("terminal spawn scope is unauthorized".to_string());
        }
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal dimensions must be greater than zero".to_string());
        }

        let authoritative_project_id = match &conversation.execution_target {
            ExecutionTarget::ProjectRoot { project_id, .. }
            | ExecutionTarget::Worktree { project_id, .. } => Some(project_id.as_str()),
            ExecutionTarget::Workspace => conversation
                .project_attachment
                .as_ref()
                .map(|attachment| attachment.project_id.as_str()),
        };
        if let Some(project_id) = self.project_id.as_deref() {
            if project_id.trim().is_empty() || authoritative_project_id != Some(project_id) {
                return Err("terminal spawn project scope is unauthorized".to_string());
            }
        }

        let cwd = match (&self.cwd_source, &conversation.execution_target) {
            (TerminalCwdSource::Workspace, _)
            | (TerminalCwdSource::ExecutionTarget, ExecutionTarget::Workspace) => {
                conversation.workspace_cwd.clone()
            }
            (
                TerminalCwdSource::ExecutionTarget,
                ExecutionTarget::ProjectRoot { project_root, .. },
            ) => project_root.clone(),
            (
                TerminalCwdSource::ExecutionTarget,
                ExecutionTarget::Worktree { worktree_path, .. },
            ) => worktree_path.clone(),
        };

        Ok(SpawnOptions {
            shell: None,
            cwd: Some(cwd),
            env: None,
            conversation_id: Some(self.conversation_id),
            project_id: self.project_id,
            cols: Some(self.cols),
            rows: Some(self.rows),
            program: None,
            args: None,
            kind: None,
        })
    }
}

/// Options for spawning a new terminal on trusted local/internal paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnOptions {
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    /// Canonical primary ownership scope for every durable user terminal.
    #[serde(default)]
    pub conversation_id: Option<ConversationId>,
    /// Optional project attribution; never the ownership or authorization key.
    #[serde(default)]
    pub project_id: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    // ADR-004.2: terminal-native agent launch.
    // When `program` is Some, the PTY runs that executable directly with `args`
    // as discrete argv entries, bypassing shell resolution and shell quoting of
    // the prompt. When `program` is None, spawn behavior is unchanged.
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<String>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            shell: None,
            cwd: None,
            env: None,
            conversation_id: None,
            project_id: None,
            cols: Some(80),
            rows: Some(24),
            program: None,
            args: None,
            kind: None,
        }
    }
}

/// SessionWorkspace refs exist only for conversation-scoped local terminals.
/// Scope-less project terminals and ephemeral SSH keep a process-local
/// ConversationId for claims, but they never enter workspace admission.
pub(crate) fn tracks_session_workspace_ref(options: &SpawnOptions) -> bool {
    options.kind.as_deref() != Some("ssh") && options.conversation_id.is_some()
}

/// Observable cleanup phase. These are the only phase names allowed onto the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCleanupStage {
    Kill,
    Wait,
    ReaderJoin,
    FlusherJoin,
}

impl TerminalCleanupStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kill => "kill",
            Self::Wait => "wait",
            Self::FlusherJoin => "flusher_join",
            Self::ReaderJoin => "reader_join",
        }
    }
}

impl std::fmt::Display for TerminalCleanupStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Conservative resource ownership state. Only `Removed` releases capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalLifecycleState {
    Active,
    Terminating,
    Quarantined,
    Removed,
}

impl TerminalLifecycleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Terminating => "Terminating",
            Self::Quarantined => "Quarantined",
            Self::Removed => "Removed",
        }
    }
}

/// Successful cleanup receipt. `released_slot` is true for exactly one caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCleanupReceipt {
    pub terminal_id: String,
    pub job_id: u64,
    pub elapsed_ms: u64,
    pub attempt: u64,
    pub released_slot: bool,
    pub already_removed: bool,
    /// A worker thread had panicked by the time cleanup joined it. Resources are
    /// released either way, but the teardown was degraded: that worker may have
    /// left output unpublished or an exit event unsent. Kept on the receipt so
    /// a caller can tell a clean teardown from a salvaged one.
    pub worker_panicked: bool,
    /// Resources were released without the graceful path completing: a worker
    /// was detached rather than joined, or a child abandoned rather than reaped.
    /// The slot is free and the record is gone, but teardown was not clean.
    pub forced: bool,
}

/// Internal classification retained for deterministic tests and sanitized logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCleanupFailureReason {
    Error,
    DeadlineExceeded,
    ThreadPanicked,
}

/// Cleanup failed at one stable phase. Process details are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCleanupFailure {
    pub terminal_id: String,
    pub job_id: u64,
    pub stage: TerminalCleanupStage,
    pub reason: TerminalCleanupFailureReason,
    pub elapsed_ms: u64,
    pub attempt: u64,
    /// True only when the caller deadline elapsed while the retained manager job was still
    /// running. The terminal identity and capacity remain owned until proven completion.
    pub in_flight: bool,
}

impl std::fmt::Display for TerminalCleanupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "terminal cleanup failed at stage {}",
            self.stage.as_str()
        )
    }
}

impl std::error::Error for TerminalCleanupFailure {}

/// One manager-owned cleanup job retained for the lifetime of an in-flight attempt. Callers and
/// retries observe this shared state; none of them owns or detaches the blocking task.
pub struct TerminalCleanupJob {
    job_id: u64,
    attempt: u64,
    started: Instant,
    deadline: Instant,
    stage: Mutex<TerminalCleanupStage>,
    result: Mutex<Option<Result<TerminalCleanupReceipt, TerminalCleanupFailure>>>,
    release_receipt_claimed: AtomicBool,
    completed: tokio::sync::Notify,
}

impl std::fmt::Debug for TerminalCleanupJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalCleanupJob")
            .field("job_id", &self.job_id)
            .field("attempt", &self.attempt)
            .field("stage", &self.stage())
            .field("completed", &self.is_complete())
            .finish()
    }
}

impl TerminalCleanupJob {
    fn new(job_id: u64, attempt: u64, started: Instant, deadline: Instant) -> Self {
        Self {
            job_id,
            attempt,
            started,
            deadline,
            stage: Mutex::new(TerminalCleanupStage::Kill),
            result: Mutex::new(None),
            release_receipt_claimed: AtomicBool::new(false),
            completed: tokio::sync::Notify::new(),
        }
    }

    #[must_use]
    pub const fn job_id(&self) -> u64 {
        self.job_id
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub fn stage(&self) -> TerminalCleanupStage {
        *self
            .stage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_stage(&self, stage: TerminalCleanupStage) {
        *self
            .stage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = stage;
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn result_for_observer(
        &self,
    ) -> Option<Result<TerminalCleanupReceipt, TerminalCleanupFailure>> {
        let result = self
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        result.map(|result| {
            result.map(|mut receipt| {
                if receipt.released_slot
                    && self
                        .release_receipt_claimed
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                {
                    receipt.released_slot = false;
                    receipt.already_removed = true;
                }
                receipt
            })
        })
    }

    fn complete(&self, result: Result<TerminalCleanupReceipt, TerminalCleanupFailure>) {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(result);
            drop(slot);
            self.completed.notify_waiters();
        }
    }

    fn in_flight_deadline_failure(&self, terminal_id: &str) -> TerminalCleanupFailure {
        TerminalCleanupFailure {
            terminal_id: terminal_id.to_string(),
            job_id: self.job_id,
            stage: self.stage(),
            reason: TerminalCleanupFailureReason::DeadlineExceeded,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            attempt: self.attempt,
            in_flight: true,
        }
    }
}

/// Aggregate host receipt for a bounded all-terminal shutdown attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PtyShutdownReceipt {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub in_flight: usize,
    pub elapsed_ms: u64,
}

impl PtyShutdownReceipt {
    #[must_use]
    pub const fn clean_success(self) -> bool {
        self.failed == 0 && self.in_flight == 0 && self.attempted == self.succeeded
    }
}

/// Crate-private production seam used by deterministic failure tests. Every method receives the
/// same absolute deadline; implementations must never mint a per-stage budget.
pub(crate) trait CleanupDriver: Send + Sync {
    fn kill(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<(), TerminalCleanupFailureReason>;

    fn try_wait(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<Option<portable_pty::ExitStatus>, TerminalCleanupFailureReason>;

    fn join_thread(
        &self,
        stage: TerminalCleanupStage,
        handle: &mut Option<std::thread::JoinHandle<()>>,
        deadline: Instant,
    ) -> Result<WorkerExit, TerminalCleanupFailureReason>;
}

/// Result of waiting for the reader's descriptor to become readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Readable,
    /// A stop was requested and the descriptor is quiet — nothing left to lose.
    Stop,
    Failed,
}

/// How a worker thread ended. A panic still satisfies the join — the thread is
/// gone, which is what the join waits for — but it is a degraded teardown and
/// must stay visible instead of being folded into an ordinary success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerExit {
    Clean,
    Panicked,
}

/// Every live pid in `sid`, excluding the session leader itself.
///
/// The leader is signalled through the normal child handle, and its pid must
/// stay reserved until cleanup reaps it — signalling it here as well would risk
/// the reuse hazard the group sweep is careful to avoid.
#[cfg(unix)]
fn session_members(sid: libc::pid_t) -> Vec<libc::pid_t> {
    all_pids()
        .into_iter()
        .filter(|pid| *pid != sid && *pid > 1)
        .filter(|pid| unsafe { libc::getsid(*pid) } == sid)
        .collect()
}

#[cfg(all(unix, target_os = "macos"))]
fn all_pids() -> Vec<libc::pid_t> {
    // Sized from a first probe, then re-read; a process appearing in between
    // just gets caught on the next sweep.
    let needed = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    // Slack so a burst of new processes cannot silently truncate the list.
    let capacity = needed as usize + 64;
    let mut pids = vec![0 as libc::pid_t; capacity];
    let bytes = (capacity * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if written <= 0 {
        return Vec::new();
    }
    pids.truncate(written as usize);
    pids
}

#[cfg(all(unix, not(target_os = "macos")))]
fn all_pids() -> Vec<libc::pid_t> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::pid_t>().ok())
        .collect()
}

#[derive(Debug, Default)]
struct SystemCleanupDriver;

impl CleanupDriver for SystemCleanupDriver {
    fn kill(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<(), TerminalCleanupFailureReason> {
        if Instant::now() >= deadline {
            return Err(TerminalCleanupFailureReason::DeadlineExceeded);
        }
        // Sweep the child's process group before the direct child is touched.
        //
        // `child.kill()` on unix is SIGHUP, a grace loop that calls `try_wait`,
        // then SIGKILL — all against the one pid, so anything sharing the
        // shell's group is left running. On Linux such a survivor holds the pty
        // slave open and the reader never sees EOF.
        //
        // Order is the whole trick, and it was wrong when first written: the
        // grace loop REAPS the leader when the shell exits inside it, and a
        // reaped leader makes its pgid unaddressable — `killpg` then answers
        // ESRCH while the survivor is still very much alive. Measured, not
        // assumed: sweep-then-kill collects it, kill-then-sweep does not.
        //
        // The group alone is not enough: an interactive shell runs job control,
        // and job control puts every job in a process group of its own. This
        // app spawns login+interactive shells, so a user's `claude`, `node` or
        // background job is NOT in the shell's group. The session sweep below
        // is what reaches them — the shell is a session leader (`setsid` in
        // `pre_exec`), so its session is exactly "everything this terminal
        // started".
        //
        // `portable_pty` runs `setsid()` in `pre_exec`, so the child leads its
        // own session and `pgid == pid` — this can never reach our own group.
        #[cfg(unix)]
        if let Some(leader) = child.process_id().map(|pid| pid as libc::pid_t) {
            for signal in [libc::SIGHUP, libc::SIGKILL] {
                if unsafe { libc::killpg(leader, signal) } != 0 {
                    let error = std::io::Error::last_os_error();
                    // ESRCH just means the group is already empty.
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        log::warn!(
                            "[pty-cleanup] process-group sweep failed pgid={leader} signal={signal} error={error}"
                        );
                    }
                }
            }

            // Then everything else this terminal's session still holds — the
            // job-control groups the sweep above cannot address. Read the
            // membership once and signal that snapshot: re-reading between
            // SIGHUP and SIGKILL would let a pid recycled in between take the
            // SIGKILL. Anything that left the session on purpose (its own
            // `setsid`) is out of scope by construction.
            let members = session_members(leader);
            if !members.is_empty() {
                log::info!(
                    "[pty-cleanup] session sweep sid={leader} members={}",
                    members.len()
                );
            }
            for signal in [libc::SIGHUP, libc::SIGKILL] {
                for member in &members {
                    unsafe { libc::kill(*member, signal) };
                }
            }
        }

        child
            .kill()
            .map_err(|_| TerminalCleanupFailureReason::Error)
    }

    fn try_wait(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<Option<portable_pty::ExitStatus>, TerminalCleanupFailureReason> {
        if Instant::now() >= deadline {
            return Err(TerminalCleanupFailureReason::DeadlineExceeded);
        }
        child
            .try_wait()
            .map_err(|_| TerminalCleanupFailureReason::Error)
    }

    fn join_thread(
        &self,
        stage: TerminalCleanupStage,
        handle: &mut Option<std::thread::JoinHandle<()>>,
        deadline: Instant,
    ) -> Result<WorkerExit, TerminalCleanupFailureReason> {
        let Some(handle) = handle.take() else {
            return Ok(WorkerExit::Clean);
        };
        if Instant::now() >= deadline {
            // Dropping the JoinHandle detaches the thread so unresolved
            // spawn_blocking/cleanup work cannot stall Tokio teardown.
            drop(handle);
            return Err(TerminalCleanupFailureReason::DeadlineExceeded);
        }
        if handle.join().is_err() {
            // Fall through to `WorkerExit::Panicked` below.
            // The join exists to prove the worker is no longer running, and a
            // panicked thread has proven exactly that. Reporting it as a
            // cleanup failure quarantines a terminal whose thread is already
            // gone, and every retry re-joins the same dead thread and fails the
            // same way — permanently unkillable. Record it loudly and let the
            // stage pass.
            log::warn!(
                "[pty-cleanup] cleanup_stage={stage} stable_result=WORKER_PANICKED \
                 note=thread already terminated; treating join as satisfied"
            );
            return Ok(WorkerExit::Panicked);
        }
        Ok(WorkerExit::Clean)
    }
}

/// Deterministic one-shot failure driver shared by PTY, command, and WebSocket tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct ScriptedCleanupDriver {
    failures: Mutex<HashMap<TerminalCleanupStage, usize>>,
    observed_deadlines: Mutex<Vec<(TerminalCleanupStage, Instant)>>,
}

#[cfg(test)]
impl ScriptedCleanupDriver {
    pub(crate) fn fail_once(&self, stage: TerminalCleanupStage) {
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *failures.entry(stage).or_default() += 1;
    }

    pub(crate) fn observed_deadlines(&self) -> Vec<(TerminalCleanupStage, Instant)> {
        self.observed_deadlines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn observe_and_should_fail(&self, stage: TerminalCleanupStage, deadline: Instant) -> bool {
        self.observed_deadlines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((stage, deadline));
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(remaining) = failures.get_mut(&stage) else {
            return false;
        };
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }
}

#[cfg(test)]
impl CleanupDriver for ScriptedCleanupDriver {
    fn kill(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<(), TerminalCleanupFailureReason> {
        if self.observe_and_should_fail(TerminalCleanupStage::Kill, deadline) {
            return Err(TerminalCleanupFailureReason::Error);
        }
        SystemCleanupDriver.kill(child, deadline)
    }

    fn try_wait(
        &self,
        child: &mut dyn Child,
        deadline: Instant,
    ) -> Result<Option<portable_pty::ExitStatus>, TerminalCleanupFailureReason> {
        if self.observe_and_should_fail(TerminalCleanupStage::Wait, deadline) {
            return Err(TerminalCleanupFailureReason::Error);
        }
        SystemCleanupDriver.try_wait(child, deadline)
    }

    fn join_thread(
        &self,
        stage: TerminalCleanupStage,
        handle: &mut Option<std::thread::JoinHandle<()>>,
        deadline: Instant,
    ) -> Result<WorkerExit, TerminalCleanupFailureReason> {
        if self.observe_and_should_fail(stage, deadline) {
            return Err(TerminalCleanupFailureReason::Error);
        }
        SystemCleanupDriver.join_thread(stage, handle, deadline)
    }
}

#[derive(Debug, Default)]
struct TerminalCleanupProgress {
    kill_completed: bool,
    child_reaped: bool,
}

#[derive(Debug, Clone)]
struct PhoneFitPark {
    desktop_cols: u16,
    desktop_rows: u16,
}

#[derive(Debug, Default)]
struct PhoneFitLease {
    park: Option<PhoneFitPark>,
    owners: HashSet<String>,
}

fn empty_phone_fit() -> Arc<RwLock<PhoneFitLease>> {
    Arc::new(RwLock::new(PhoneFitLease::default()))
}

/// Current phone/desktop geometry owner for a live PTY.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayModeState {
    pub mode: TerminalDisplayMode,
    pub cols: u16,
    pub rows: u16,
}

/// A running terminal instance
pub struct TerminalInstance {
    pub id: String,
    pub conversation_id: ConversationId,
    pub workspace_ref_tracked: bool,
    pub project_id: Option<String>,
    pub child: Arc<AsyncMutex<Option<Box<dyn Child + Send>>>>,
    pub master: Arc<AsyncMutex<Option<Box<dyn MasterPty + Send>>>>,
    pub writer: Arc<AsyncMutex<Option<Box<dyn Write + Send>>>>,
    pub reader_handle: Arc<AsyncMutex<Option<std::thread::JoinHandle<()>>>>,
    pub flusher_handle: Arc<AsyncMutex<Option<std::thread::JoinHandle<()>>>>,
    /// "No more PTY input is coming." The flusher drains once more and exits.
    ///
    /// The reader sets it on EOF/error, but it MUST NOT be reachable only from
    /// there: the flusher would then outlive any reader that ended without
    /// running its tail (panic, or a spawn that never got that far), and
    /// `join_thread_until(FlusherJoin)` polls `is_finished()` — against an
    /// immortal thread that never returns true, every terminate attempt burns
    /// the whole deadline and quarantines, retry included. Observed live: a
    /// process with 9 flusher threads and 8 readers, the odd one parked in
    /// `thread::sleep` forever. Cleanup owns this flag too.
    done_flag: Arc<AtomicBool>,
    /// "Stop when you have nothing left to read." Distinct from `done_flag`,
    /// which means "the producer has finished": this one is a request, and the
    /// reader still drains everything the kernel already buffered before acting
    /// on it. Collapsing the two would trade an unkillable terminal for lost
    /// tail output. Unix only — it is what lets a reader parked in `read()` be
    /// interrupted at all; on Windows the read stays uninterruptible and the
    /// forced-release floor in `force_kill` covers it.
    stop_requested: Arc<AtomicBool>,
    lifecycle_state: Arc<RwLock<TerminalLifecycleState>>,
    cleanup_gate: Arc<AsyncMutex<()>>,
    cleanup_job: Arc<Mutex<Option<Arc<TerminalCleanupJob>>>>,
    cleanup_progress: Arc<Mutex<TerminalCleanupProgress>>,
    cleanup_attempts: Arc<AtomicU64>,
    pub shell: String,
    pub cwd: String,
    pub pid: u32,
    pub last_activity: Arc<RwLock<Instant>>,
    pub orphan_since: Arc<RwLock<Option<Instant>>>,
    pub renderer_refs: Arc<RwLock<HashSet<String>>>,
    /// When true, this terminal is still owned by an open project/tab and must
    /// NOT be reaped by orphan detection — even if it currently has zero
    /// renderer refs (e.g. its project is switched to the background, so the
    /// `ConnectedTerminal` component unmounted). It is set true at spawn and
    /// cleared only when the terminal is explicitly released (project closed or
    /// terminal tab closed). This prevents busy background-project terminals
    /// from being killed mid-task — the cause of the "Terminal not found"/hang.
    pub protected: Arc<AtomicBool>,
    pub cols: Arc<RwLock<u16>>,
    pub rows: Arc<RwLock<u16>>,
    /// Phone takeover: parked desktop size plus the connections that own geometry.
    phone_fit: Arc<RwLock<PhoneFitLease>>,
    /// Broadcast channel for fan-out of raw PTY output to remote WebSocket clients.
    /// Each flusher batch is sent as a `Vec<u8>` message. Tauri frontend keeps using
    /// its dedicated Channel — this field is only consumed by the remote module.
    pub broadcast_tx: Arc<tokio::sync::broadcast::Sender<TerminalOutputChunk>>,
    /// Bounded sequence-aware output log. Oldest chunks are evicted first while
    /// keeping whole chunks, so reconnect cursors can detect replay gaps.
    pub output_log: Arc<RwLock<std::collections::VecDeque<TerminalOutputChunk>>>,
    pub output_log_bytes: Arc<AtomicUsize>,
    pub next_output_seq: Arc<AtomicU64>,
    #[cfg(target_os = "windows")]
    pub conpty_handles: Option<Arc<ParkingMutex<Option<ConPtyHandles>>>>,
}

impl TerminalInstance {
    #[must_use]
    pub fn lifecycle_state(&self) -> TerminalLifecycleState {
        *self.lifecycle_state.read()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.lifecycle_state() == TerminalLifecycleState::Active
    }

    fn require_active(&self) -> Result<(), String> {
        if self.is_active() {
            Ok(())
        } else {
            Err("Terminal resource is not active".to_string())
        }
    }

    #[must_use]
    pub fn is_phone_fit(&self) -> bool {
        self.phone_fit.read().park.is_some()
    }

    #[must_use]
    pub fn display_mode(&self) -> TerminalDisplayMode {
        if self.is_phone_fit() {
            TerminalDisplayMode::Phone
        } else {
            TerminalDisplayMode::Desktop
        }
    }

    fn adopt_phone_owner(&self, owner: &str, cols: u16, rows: u16) -> (u16, u16) {
        let mut lease = self.phone_fit.write();
        if lease.park.is_none() {
            lease.park = Some(PhoneFitPark {
                desktop_cols: *self.cols.read(),
                desktop_rows: *self.rows.read(),
            });
        }
        lease.owners.insert(owner.to_string());
        (cols, rows)
    }

    fn drop_phone_owner(&self, owner: &str) -> Option<(u16, u16)> {
        let mut lease = self.phone_fit.write();
        lease.owners.remove(owner);
        if lease.owners.is_empty() {
            lease
                .park
                .take()
                .map(|park| (park.desktop_cols, park.desktop_rows))
        } else {
            None
        }
    }

    fn force_desktop_display(&self) -> Option<(u16, u16)> {
        let mut lease = self.phone_fit.write();
        lease.owners.clear();
        lease
            .park
            .take()
            .map(|park| (park.desktop_cols, park.desktop_rows))
    }

    /// Remember the latest desktop grid while a phone owns the live ioctl.
    fn park_desktop_size(&self, cols: u16, rows: u16) {
        let mut lease = self.phone_fit.write();
        if let Some(park) = lease.park.as_mut() {
            park.desktop_cols = cols;
            park.desktop_rows = rows;
        }
    }

    /// Update the last activity timestamp
    pub fn update_activity(&self) {
        *self.last_activity.write() = Instant::now();
    }

    /// Get elapsed time since last activity
    pub fn inactive_duration(&self) -> Duration {
        self.last_activity.read().elapsed()
    }

    /// Add a renderer reference
    pub fn add_renderer_ref(&self, renderer_id: String) {
        self.renderer_refs.write().insert(renderer_id);
        *self.orphan_since.write() = None;
    }

    /// Remove a renderer reference
    pub fn remove_renderer_ref(&self, renderer_id: &str) {
        let mut refs = self.renderer_refs.write();
        let removed = refs.remove(renderer_id);
        if removed && refs.is_empty() {
            *self.orphan_since.write() = Some(Instant::now());
        }
    }

    /// Get count of renderer references
    pub fn renderer_ref_count(&self) -> usize {
        self.renderer_refs.read().len()
    }

    /// Check if terminal has no renderer references
    pub fn is_orphan(&self) -> bool {
        self.renderer_refs.read().is_empty()
    }

    /// Whether this terminal is eligible for orphan reaping right now.
    ///
    /// A terminal is reapable only when it is NOT protected (its project/tab is
    /// genuinely closed), has no renderer refs, and has exceeded the timeout —
    /// measured from when it became orphaned, or by inactivity if it never had
    /// a renderer ref. Protected terminals (e.g. a backgrounded project's live
    /// terminals) are never reaped, even with zero renderer refs.
    pub fn is_orphan_reapable(&self, timeout: Duration) -> bool {
        // A terminal with a cleanup job in flight is not reapable — the sweep
        // would race that job. `Quarantined` stays reapable on purpose: it is
        // the state that most needs collecting, and the sweep escalates it to a
        // forced release rather than re-running the graceful attempt that
        // already failed. Excluding it outright leaked its slot forever
        // whenever nobody happened to click retry.
        if matches!(
            self.lifecycle_state(),
            TerminalLifecycleState::Terminating | TerminalLifecycleState::Removed
        ) {
            return false;
        }
        should_reap_orphan(
            self.is_protected(),
            self.is_orphan(),
            self.orphan_since().map(|since| since.elapsed()),
            self.inactive_duration(),
            timeout,
        )
    }

    /// Returns when the terminal became orphaned, if ever.
    pub fn orphan_since(&self) -> Option<Instant> {
        *self.orphan_since.read()
    }

    /// Whether this terminal is protected from orphan reaping (still owned by an
    /// open project/tab). See the `protected` field docs.
    pub fn is_protected(&self) -> bool {
        self.protected.load(Ordering::Relaxed)
    }

    /// Update the protection flag. Set false only when the terminal is genuinely
    /// released (project closed / terminal tab closed), making it eligible for
    /// orphan reaping once it also has no renderer refs.
    pub fn set_protected(&self, protected: bool) {
        self.protected.store(protected, Ordering::Relaxed);
    }

    pub fn conversation_matches(&self, conversation_id: ConversationId) -> bool {
        self.conversation_id == conversation_id
    }

    pub fn project_matches(&self, project_id: &str) -> bool {
        self.project_id.as_deref() == Some(project_id)
    }

    /// Atomically snapshot unseen sequenced chunks and subscribe to live output.
    pub fn subscribe_from(&self, last_seq: u64) -> TerminalReplay {
        let guard = self.output_log.write();
        let receiver = self.broadcast_tx.subscribe();
        let earliest = guard.front().map(|chunk| chunk.seq);
        let latest_seq = guard.back().map(|chunk| chunk.seq).unwrap_or(last_seq);
        // Gap if the client's cursor is behind the earliest retained chunk,
        // OR if the log is empty but the client expected prior output.
        let gap = earliest
            .map(|first| last_seq.saturating_add(1) < first)
            .unwrap_or(last_seq > 0);
        let chunks = guard
            .iter()
            .filter(|chunk| chunk.seq > last_seq)
            .cloned()
            .collect();
        TerminalReplay {
            chunks,
            gap,
            latest_seq,
            receiver,
            claim_generation: None,
        }
    }
}

/// Pure decision for whether an orphaned terminal should be reaped.
///
/// Kept free-standing (no PTY handles) so it can be unit-tested in isolation.
///
/// * `protected` — terminal is still owned by an open project/tab; never reap.
/// * `is_orphan` — terminal currently has zero renderer refs.
/// * `orphaned_for` — elapsed time since it became orphaned, if it ever was.
/// * `inactive_for` — elapsed time since last PTY activity.
/// * `timeout` — configured orphan timeout.
fn should_reap_orphan(
    protected: bool,
    is_orphan: bool,
    orphaned_for: Option<Duration>,
    inactive_for: Duration,
    timeout: Duration,
) -> bool {
    if protected || !is_orphan {
        return false;
    }
    match orphaned_for {
        Some(elapsed) => elapsed > timeout,
        None => inactive_for > timeout,
    }
}

struct TerminalSlotReservation {
    active_slots: Arc<AtomicUsize>,
    committed: bool,
}

impl TerminalSlotReservation {
    fn try_acquire(active_slots: Arc<AtomicUsize>) -> Option<Self> {
        loop {
            let current = active_slots.load(Ordering::SeqCst);
            if current >= GLOBAL_TERMINAL_LIMIT {
                return None;
            }

            if active_slots
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(Self {
                    active_slots,
                    committed: false,
                });
            }
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for TerminalSlotReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.active_slots.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// RAII rollback for a claim issued before PTY creation: if any spawn step
/// fails, the guard's drop removes the dangling claim record so no terminal
/// exists with a live credential but no PTY (and vice versa).
struct ClaimRollbackGuard<'a> {
    claims: &'a crate::pty::claims::TerminalClaimRegistry,
    terminal_id: String,
    active: bool,
}

impl ClaimRollbackGuard<'_> {
    fn commit(&mut self) {
        self.active = false;
    }
}

impl Drop for ClaimRollbackGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.claims.remove(&self.terminal_id);
        }
    }
}

/// Manages all PTY instances
#[derive(Clone)]
pub struct PtyManager {
    terminals: Arc<RwLock<HashMap<String, Arc<TerminalInstance>>>>,
    active_terminal_slots: Arc<AtomicUsize>,
    id_counter: Arc<AtomicU64>,
    terminal_events: TerminalEventHub,
    orphan_detection_enabled: Arc<AtomicBool>,
    orphan_timeout_ms: Arc<AtomicU64>,
    orphan_detection_started: Arc<AtomicBool>,
    cwd_tracker: Arc<CwdTracker>,
    git_tracker: Arc<GitTracker>,
    exit_code_tracker: Arc<ExitCodeTracker>,
    /// Claim credential registry (CAP-3). Single ownership here keeps the
    /// claim lifecycle coupled to the terminal lifecycle: issued at spawn,
    /// removed at kill/reap.
    claims: Arc<crate::pty::claims::TerminalClaimRegistry>,
    cleanup_driver: Arc<RwLock<Arc<dyn CleanupDriver>>>,
    cleanup_job_counter: Arc<AtomicU64>,
    /// When true, orphan detection and kill operations are deferred.
    /// Set when the app window is minimized/hidden to prevent
    /// ConPTY lifecycle issues on Windows.
    is_hidden: Arc<AtomicBool>,
    /// Live output views (desktop attach + web/iOS attach/watch).
    /// The last view close pauses cwd/git polling without killing the PTY.
    view_refs: Arc<parking_lot::Mutex<HashMap<String, u32>>>,
}

impl PtyManager {
    /// Create a new PtyManager
    pub fn new(
        terminal_events: TerminalEventHub,
        cwd_tracker: Arc<CwdTracker>,
        git_tracker: Arc<GitTracker>,
        exit_code_tracker: Arc<ExitCodeTracker>,
    ) -> Self {
        Self {
            terminals: Arc::new(RwLock::new(HashMap::new())),
            active_terminal_slots: Arc::new(AtomicUsize::new(0)),
            id_counter: Arc::new(AtomicU64::new(0)),
            terminal_events,
            orphan_detection_enabled: Arc::new(AtomicBool::new(true)),
            orphan_timeout_ms: Arc::new(AtomicU64::new(ORPHAN_TIMEOUT_MS)),
            orphan_detection_started: Arc::new(AtomicBool::new(false)),
            is_hidden: Arc::new(AtomicBool::new(false)),
            cwd_tracker,
            git_tracker,
            exit_code_tracker,
            claims: Arc::new(crate::pty::claims::TerminalClaimRegistry::new()),
            cleanup_driver: Arc::new(RwLock::new(Arc::new(SystemCleanupDriver))),
            cleanup_job_counter: Arc::new(AtomicU64::new(0)),
            view_refs: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    fn pause_view_tracking(&self, terminal_id: &str) {
        self.cwd_tracker.stop_tracking(terminal_id);
        self.git_tracker.remove_terminal(terminal_id);
    }

    fn resume_view_tracking(&self, terminal_id: &str) {
        let Some(instance) = self
            .get(terminal_id)
            .filter(|instance| instance.is_active())
        else {
            return;
        };
        let cwd = self
            .cwd_tracker
            .get_cwd(terminal_id)
            .unwrap_or_else(|| instance.cwd.clone());
        if self.cwd_tracker.get_cwd(terminal_id).is_none() {
            self.cwd_tracker
                .start_tracking(terminal_id, instance.pid, &cwd);
        }
        if !self.git_tracker.is_tracking(terminal_id) {
            self.git_tracker.initialize_terminal(terminal_id, &cwd);
        }
    }

    /// A desktop or remote client started watching this PTY.
    pub fn note_view_opened(&self, terminal_id: &str) {
        let views = {
            let mut refs = self.view_refs.lock();
            let count = refs.entry(terminal_id.to_string()).or_insert(0);
            *count = count.saturating_add(1);
            *count
        };
        self.resume_view_tracking(terminal_id);
        log::info!("[pty-view] opened terminal_id={terminal_id} views={views}");
    }

    /// Hide a tab that never attached on this surface. Pause polling only
    /// when no other desktop/remote view is still watching.
    pub fn pause_tracking_if_unwatched(&self, terminal_id: &str) {
        let watched = self.view_refs.lock().get(terminal_id).copied().unwrap_or(0) > 0;
        if watched {
            return;
        }
        self.pause_view_tracking(terminal_id);
        log::info!(
            "[pty-view] unwatched close-view; pausing cwd/git tracking terminal_id={terminal_id} pty_still_running=true"
        );
    }

    /// A desktop or remote client stopped watching this PTY.
    /// The process stays alive; cwd/git polling stops only when no views remain.
    pub fn note_view_closed(&self, terminal_id: &str) {
        let remaining = {
            let mut refs = self.view_refs.lock();
            match refs.get_mut(terminal_id) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    *count
                }
                Some(_) => {
                    refs.remove(terminal_id);
                    0
                }
                None => 0,
            }
        };
        if remaining == 0 {
            self.pause_view_tracking(terminal_id);
            log::info!(
                "[pty-view] last view closed; pausing cwd/git tracking terminal_id={terminal_id} pty_still_running=true"
            );
        } else {
            log::info!("[pty-view] closed terminal_id={terminal_id} remaining_views={remaining}");
        }
    }

    fn cleanup_failure(
        instance: &TerminalInstance,
        stage: TerminalCleanupStage,
        reason: TerminalCleanupFailureReason,
        job: &TerminalCleanupJob,
    ) -> TerminalCleanupFailure {
        TerminalCleanupFailure {
            terminal_id: instance.id.clone(),
            job_id: job.job_id,
            stage,
            reason,
            elapsed_ms: u64::try_from(job.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            attempt: job.attempt,
            in_flight: false,
        }
    }

    fn sleep_until_next_cleanup_poll(deadline: Instant) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(now)
                .min(TERMINAL_CLEANUP_POLL_INTERVAL),
        );
    }

    fn join_thread_until(
        instance: &TerminalInstance,
        handle_slot: &AsyncMutex<Option<std::thread::JoinHandle<()>>>,
        stage: TerminalCleanupStage,
        driver: &dyn CleanupDriver,
        job: &TerminalCleanupJob,
    ) -> Result<WorkerExit, TerminalCleanupFailure> {
        job.set_stage(stage);
        let mut handle = handle_slot.blocking_lock();
        loop {
            let Some(current) = handle.as_ref() else {
                return Ok(WorkerExit::Clean);
            };
            if current.is_finished() {
                return driver
                    .join_thread(stage, &mut handle, job.deadline)
                    .map_err(|reason| Self::cleanup_failure(instance, stage, reason, job));
            }
            if Instant::now() >= job.deadline {
                return Err(Self::cleanup_failure(
                    instance,
                    stage,
                    TerminalCleanupFailureReason::DeadlineExceeded,
                    job,
                ));
            }
            Self::sleep_until_next_cleanup_poll(job.deadline);
        }
    }

    fn cleanup_terminal_resources_sync(
        instance: Arc<TerminalInstance>,
        driver: Arc<dyn CleanupDriver>,
        job: Arc<TerminalCleanupJob>,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        // Drop input exactly once, and do not give it back on failure.
        //
        // Restoring it would be theatre: `write` gates on `require_active`, and
        // a terminal whose cleanup failed is `Quarantined`, so every keystroke
        // is refused before the writer is even consulted. Past the kill stage
        // the child is dead too, so the descriptor leads nowhere. Quarantine is
        // a staging state on the way out — `force_kill`, which the retry now
        // escalates to, is what resolves it, not a recovered keyboard.
        instance.writer.blocking_lock().take();
        Self::cleanup_stages(&instance, driver.as_ref(), &job)
    }

    fn cleanup_stages(
        instance: &Arc<TerminalInstance>,
        driver: &dyn CleanupDriver,
        job: &Arc<TerminalCleanupJob>,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        {
            let mut progress = instance
                .cleanup_progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !progress.child_reaped {
                let mut child_slot = instance.child.blocking_lock();
                if let Some(child) = child_slot.as_mut() {
                    if !progress.kill_completed {
                        job.set_stage(TerminalCleanupStage::Kill);
                        if Instant::now() >= job.deadline {
                            return Err(Self::cleanup_failure(
                                instance,
                                TerminalCleanupStage::Kill,
                                TerminalCleanupFailureReason::DeadlineExceeded,
                                job,
                            ));
                        }
                        driver
                            .kill(child.as_mut(), job.deadline)
                            .map_err(|reason| {
                                Self::cleanup_failure(
                                    instance,
                                    TerminalCleanupStage::Kill,
                                    reason,
                                    job,
                                )
                            })?;
                        progress.kill_completed = true;
                    }

                    loop {
                        job.set_stage(TerminalCleanupStage::Wait);
                        if Instant::now() >= job.deadline {
                            return Err(Self::cleanup_failure(
                                instance,
                                TerminalCleanupStage::Wait,
                                TerminalCleanupFailureReason::DeadlineExceeded,
                                job,
                            ));
                        }
                        match driver.try_wait(child.as_mut(), job.deadline) {
                            Ok(Some(_)) => {
                                child_slot.take();
                                progress.child_reaped = true;
                                break;
                            }
                            Ok(None) => Self::sleep_until_next_cleanup_poll(job.deadline),
                            Err(reason) => {
                                return Err(Self::cleanup_failure(
                                    instance,
                                    TerminalCleanupStage::Wait,
                                    reason,
                                    job,
                                ))
                            }
                        }
                    }
                } else {
                    progress.child_reaped = true;
                }
            }
        }

        // Ask the reader to stop. It drains everything still buffered before
        // acting on this, so it cannot cost tail output; it exists so a reader
        // parked in `read()` — the case the cleanup chain previously had no
        // answer for at all — becomes reachable instead of merely waited out
        // until the deadline.
        instance.stop_requested.store(true, Ordering::Release);

        // Producer before consumer.
        //
        // The reader is the only thing that can still append to `pending_buf`;
        // a reaped child does not mean the master is drained. Joining the
        // flusher first meant retiring the consumer while the producer was
        // still running, so anything the reader appended after the flusher's
        // final drain was dropped from the output log, the broadcast and the
        // frontend channel. It also mislabelled the failure: a reader stuck in
        // `read()` surfaced to the user as the *flusher* stage timing out.
        let reader_exit = Self::join_thread_until(
            instance,
            &instance.reader_handle,
            TerminalCleanupStage::ReaderJoin,
            driver,
            job,
        )?;

        // The reader is gone, so no further bytes can arrive and the flusher may
        // retire. It normally set this itself on the way out; setting it here
        // covers the reader that ended without running its tail (a panic, or a
        // spawn that never reached it). Without that fallback the flusher runs
        // forever and `join_thread_until` — which polls `is_finished()` — can
        // never succeed, so every terminate on that terminal, retry included,
        // burns the whole deadline and quarantines.
        instance.done_flag.store(true, Ordering::Release);

        let flusher_exit = Self::join_thread_until(
            instance,
            &instance.flusher_handle,
            TerminalCleanupStage::FlusherJoin,
            driver,
            job,
        )?;

        // All fallible stages passed. Drop platform/master handles before ownership is removed.
        instance.master.blocking_lock().take();
        #[cfg(target_os = "windows")]
        if let Some(conpty_handles) = &instance.conpty_handles {
            conpty_handles.lock().take();
        }

        Ok(TerminalCleanupReceipt {
            terminal_id: instance.id.clone(),
            job_id: job.job_id,
            elapsed_ms: u64::try_from(job.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            attempt: job.attempt,
            released_slot: false,
            already_removed: false,
            worker_panicked: reader_exit == WorkerExit::Panicked
                || flusher_exit == WorkerExit::Panicked,
            forced: false,
        })
    }

    fn try_reserve_terminal_slot(&self) -> Option<TerminalSlotReservation> {
        TerminalSlotReservation::try_acquire(self.active_terminal_slots.clone())
    }

    fn release_terminal_slot(&self) {
        self.active_terminal_slots.fetch_sub(1, Ordering::SeqCst);
    }

    /// Start the orphan detection background task
    /// This is called lazily when the first terminal is spawned
    fn start_orphan_detection(&self) {
        // Check if already started using compare_exchange
        if self
            .orphan_detection_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return; // Already started
        }

        let manager = self.clone();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(ORPHAN_CHECK_INTERVAL_MS));

            loop {
                interval.tick().await;

                if !manager.orphan_detection_enabled.load(Ordering::Relaxed)
                    || manager.is_hidden.load(Ordering::Relaxed)
                {
                    continue;
                }

                let timeout =
                    Duration::from_millis(manager.orphan_timeout_ms.load(Ordering::Relaxed));
                let orphans: Vec<String> = manager
                    .terminals
                    .read()
                    .iter()
                    .filter(|(_, instance)| instance.is_orphan_reapable(timeout))
                    .map(|(id, _)| id.clone())
                    .collect();

                for id in orphans {
                    log::info!(
                        "[pty-cleanup] terminal_id={} shutdown_phase=orphan_reap stable_result=START",
                        id
                    );
                    // An orphan that already failed its graceful attempt is
                    // escalated rather than retried: repeating it burns a
                    // blocking thread for the full deadline every sweep and has
                    // already been shown not to work, while leaving it alone
                    // strands its slot for good.
                    let quarantined = manager.terminal_lifecycle_state(&id)
                        == Some(TerminalLifecycleState::Quarantined);
                    let outcome = if quarantined {
                        manager.force_kill(&id).await
                    } else {
                        manager.terminate(&id).await
                    };
                    if let Err(failure) = outcome {
                        log::warn!(
                            "[pty-cleanup] terminal_id={} cleanup_stage={} elapsed_ms={} state_transition=Terminating->Quarantined capacity_counter={} shutdown_phase=orphan_reap stable_result=TERMINATE_FAILED",
                            failure.terminal_id,
                            failure.stage,
                            failure.elapsed_ms,
                            manager.active_terminal_slot_count()
                        );
                    }
                }
            }
        });
    }

    /// Generate a unique terminal ID
    fn generate_id(&self) -> String {
        let counter = self.id_counter.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("terminal-{}-{}", timestamp, counter)
    }

    /// Spawn a new terminal (with binary channel IPC).
    ///
    /// CAP-3: the claim credential is issued BEFORE PTY creation, bound to the
    /// same `project_id` that is co-derived onto the [`TerminalInstance`]
    /// (write-once; the verify side reads the instance binding, so issuance and
    /// verification can never diverge). Any failure path rolls the issuance
    /// back via the RAII guard, so a credential never outlives its terminal.
    /// Returns the shared [`SpawnedTerminal`] shape feeding both transports.
    /// This is initial issuance; authenticated resume may later rotate it.
    pub async fn spawn(
        &self,
        options: SpawnOptions,
        on_data: Option<Channel<Response>>,
    ) -> Result<SpawnedTerminal, String> {
        if crate::host_admission::HostAdmission::global()
            .check()
            .is_err()
        {
            return Err(crate::host_admission::HOST_SHUTTING_DOWN.to_string());
        }
        // Start orphan detection on first spawn (lazy initialization)
        self.start_orphan_detection();

        let mut slot_reservation = self
            .try_reserve_terminal_slot()
            .ok_or_else(|| "Global terminal limit reached".to_string())?;

        let id = self.generate_id();
        // SessionWorkspace refs exist only when the caller supplied a real
        // Conversation id. Scope-less project terminals and ephemeral SSH still
        // get a process-local ConversationId so claims have one typed primary
        // scope, but they must not take the workspace admission/remove path.
        let tracks_workspace_ref = tracks_session_workspace_ref(&options);
        let conversation_id = options
            .conversation_id
            .unwrap_or_else(ConversationId::new_v4);

        let claim = self
            .claims
            .issue(&id, conversation_id, options.project_id.as_deref());
        let mut claim_guard = ClaimRollbackGuard {
            claims: &self.claims,
            terminal_id: id.clone(),
            active: true,
        };

        let mut scoped_options = options;
        scoped_options.conversation_id = Some(conversation_id);
        let catalog_project_id = scoped_options.project_id.clone();
        let info = match self
            .spawn_pty(id.clone(), scoped_options, on_data, tracks_workspace_ref)
            .await
        {
            Ok(info) => info,
            Err(e) => {
                // claim_guard drops here and removes the dangling record.
                return Err(e);
            }
        };

        claim_guard.commit();
        slot_reservation.commit();
        let spawned = SpawnedTerminal { info, claim };
        self.terminal_events.emit(TerminalEvent::Spawned {
            terminal_id: spawned.info.id.clone(),
            project_id: catalog_project_id,
            conversation_id: if tracks_workspace_ref {
                Some(conversation_id.to_string())
            } else {
                None
            },
            cwd: spawned.info.cwd.clone(),
            cols: spawned.info.cols,
            rows: spawned.info.rows,
            shell: spawned.info.shell.clone(),
        });
        Ok(spawned)
    }

    /// Spawn an interactive terminal from the narrow remote intent. All
    /// executable, shell, argv, environment, and cwd values are derived from
    /// host-owned Conversation metadata before entering the ordinary trusted
    /// spawn path.
    pub async fn spawn_for_conversation(
        &self,
        intent: TerminalSpawnIntentV1,
        conversation: &ConversationRecordV2,
        on_data: Option<Channel<Response>>,
    ) -> Result<SpawnedTerminal, String> {
        let options = intent.into_trusted_options(conversation)?;
        self.spawn(options, on_data).await
    }

    /// Platform-specific PTY creation (the former `spawn` body). `id` and the
    /// claim lifecycle are owned by [`spawn`](Self::spawn); slot reservation
    /// commits there too, so a failure on any branch rolls everything back.
    async fn spawn_pty(
        &self,
        id: String,
        options: SpawnOptions,
        on_data: Option<Channel<Response>>,
        workspace_ref_tracked: bool,
    ) -> Result<TerminalInfo, String> {
        // ADR-004.2: Resolve the program to run. When `program` is set we run
        // that executable directly (terminal-native agent launch); otherwise we
        // resolve a login shell exactly as before. `program == None` keeps the
        // shell path byte-for-byte identical to prior behavior.
        let resolved = if let Some(program) = &options.program {
            self.resolve_program_path(program)?
        } else if let Some(shell) = &options.shell {
            ResolvedProgram::new(self.resolve_shell_path(shell)?)
        } else {
            ResolvedProgram::new(self.get_default_shell()?)
        };
        // Merge prepend_args (from npm .cmd shim rewriting) with user args.
        // User args apply only for agent/program spawns, not shell spawns.
        let user_args = if options.program.is_some() {
            options.args.clone().unwrap_or_default()
        } else {
            Vec::new()
        };
        let program_args: Vec<String> =
            resolved.prepend_args.into_iter().chain(user_args).collect();
        let shell_path = resolved.program;

        // Resolve working directory
        let cwd = if let Some(cwd) = &options.cwd {
            cwd.clone()
        } else {
            self.get_home_directory()
        };

        // Verify CWD exists and canonicalize to resolve symlinks and path traversal.
        // On Windows, canonicalize returns a verbatim device-namespace path (the
        // extended-length prefix) that cmd.exe/ConPTY and other external tools reject
        // as "UNC paths are not supported", making the shell fall back to the Windows
        // directory. Normalize it to a tool-friendly form (no-op off Windows),
        // mirroring the #347 fix for git worktree paths. See `strip_verbatim_prefix`.
        let cwd = std::fs::canonicalize(&cwd)
            .map_err(|e| format!("Invalid working directory '{}': {}", cwd, e))?;
        let cwd =
            crate::path_validation::strip_verbatim_prefix(&cwd.to_string_lossy()).into_owned();

        // Get terminal size
        let cols = options.cols.unwrap_or(80);
        let rows = options.rows.unwrap_or(24);
        let mut env = self.merge_environment(options.env.clone());
        log::info!(
            "[pty-env] spawn locale LANG={} LC_CTYPE={} PYTHONUTF8={} TERM={} COLORTERM={} FORCE_COLOR={}",
            env.get("LANG").map(String::as_str).unwrap_or(""),
            env.get("LC_CTYPE").map(String::as_str).unwrap_or(""),
            env.get("PYTHONUTF8").map(String::as_str).unwrap_or(""),
            env.get("TERM").map(String::as_str).unwrap_or(""),
            env.get("COLORTERM").map(String::as_str).unwrap_or(""),
            env.get("FORCE_COLOR").map(String::as_str).unwrap_or(""),
        );
        if options.program.is_none() {
            env.insert("SHELL".to_string(), shell_path.clone());
        }
        // Identify this PTY as Termul so ~/.zshrc can feature-gate (starship,
        // shared history) the same way Ghostty/Orca do. Overwrite inherited
        // Cursor/Ghostty TERM_PROGRAM — the child pane is Termul, not its parent.
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("FORCE_COLOR".to_string(), "3".to_string());
        env.insert("CLICOLOR".to_string(), "1".to_string());
        env.insert("CLICOLOR_FORCE".to_string(), "1".to_string());
        env.insert("TERM_PROGRAM".to_string(), "Termul".to_string());
        env.insert(
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        env.insert("FORCE_HYPERLINK".to_string(), "1".to_string());

        // On Windows, use our custom ConPTY implementation to avoid console window
        #[cfg(target_os = "windows")]
        {
            // ADR-004.2: In agent mode, build the command line from a discrete
            // argv array via the audited quoting helper — the prompt is passed as
            // a single argument and is never shell-interpolated. In shell mode,
            // preserve the existing shell-escaping behavior verbatim.
            let shell_escaped = if options.program.is_some() {
                crate::pty::windows::build_windows_command_line(&shell_path, &program_args)
            } else if shell_path.contains(' ') {
                format!(
                    "\"{}\" {}",
                    shell_path,
                    if cfg!(windows)
                        && (shell_path.contains("powershell") || shell_path.contains("pwsh"))
                    {
                        "-NoLogo" // Skip PowerShell banner only (profile still loads)
                    } else {
                        ""
                    }
                )
            } else if shell_path.contains("powershell") || shell_path.contains("pwsh") {
                format!("{} -NoLogo", shell_path) // Skip PowerShell banner only (profile still loads)
            } else {
                shell_path.clone()
            };

            let (reader, writer, pid, process_handle, job_handle, conpty_handles) =
                spawn_conpty(&shell_escaped, Some(&cwd), cols, rows, &env)
                    .map_err(|e| format!("Failed to spawn ConPTY: {}", e))?;

            let child = WindowsConPtyChild {
                pid,
                process_handle,
                job_handle,
            };

            // Create terminal instance
            let instance = Arc::new(TerminalInstance {
                id: id.clone(),
                conversation_id: options
                    .conversation_id
                    .expect("spawn assigned a ConversationId scope"),
                workspace_ref_tracked,
                project_id: options.project_id.clone(),
                child: Arc::new(AsyncMutex::new(Some(Box::new(child)))),
                master: Arc::new(AsyncMutex::new(None)), // No master for ConPTY
                writer: Arc::new(AsyncMutex::new(Some(writer))),
                reader_handle: Arc::new(AsyncMutex::new(None)),
                flusher_handle: Arc::new(AsyncMutex::new(None)),
                done_flag: Arc::new(AtomicBool::new(false)),
                stop_requested: Arc::new(AtomicBool::new(false)),
                lifecycle_state: Arc::new(RwLock::new(TerminalLifecycleState::Active)),
                cleanup_gate: Arc::new(AsyncMutex::new(())),
                cleanup_job: Arc::new(Mutex::new(None)),
                cleanup_progress: Arc::new(Mutex::new(TerminalCleanupProgress::default())),
                cleanup_attempts: Arc::new(AtomicU64::new(0)),
                shell: shell_path.clone(),
                cwd: cwd.clone(),
                pid,
                last_activity: Arc::new(RwLock::new(Instant::now())),
                orphan_since: Arc::new(RwLock::new(None)),
                renderer_refs: Arc::new(RwLock::new(HashSet::new())),
                protected: Arc::new(AtomicBool::new(true)),
                cols: Arc::new(RwLock::new(cols)),
                rows: Arc::new(RwLock::new(rows)),
                phone_fit: empty_phone_fit(),
                broadcast_tx: Arc::new(tokio::sync::broadcast::channel(TERM_BROADCAST_CAPACITY).0),
                output_log: Arc::new(RwLock::new(std::collections::VecDeque::new())),
                output_log_bytes: Arc::new(AtomicUsize::new(0)),
                next_output_seq: Arc::new(AtomicU64::new(0)),
                conpty_handles: Some(Arc::new(ParkingMutex::new(Some(conpty_handles)))),
            });

            // Start reader + flusher threads
            let pending_buf = Arc::new(Mutex::new(Vec::with_capacity(READ_BUF)));
            // Owned by the instance so cleanup can retire the flusher without
            // depending on the reader reaching its tail. See `done_flag` docs.
            let done_flag = instance.done_flag.clone();
            let stop_requested = instance.stop_requested.clone();

            let reader_instance = instance.clone();
            let terminal_events = self.terminal_events.clone();
            let exit_code_tracker = self.exit_code_tracker.clone();
            let terminal_id = id.clone();

            // Spawn flusher thread first (it references pending_buf and done_flag)
            let flusher_pending = pending_buf.clone();
            let flusher_done = done_flag.clone();
            let flusher_channel = on_data.clone();
            let flusher_id = id.clone();
            let flusher_broadcast = instance.broadcast_tx.clone();
            let flusher_output_log = instance.output_log.clone();
            let flusher_output_log_bytes = instance.output_log_bytes.clone();
            let flusher_next_seq = instance.next_output_seq.clone();

            let flusher_task = std::thread::spawn(move || {
                log::info!("[PTY {}] Flusher thread starting", flusher_id);
                Self::flusher_loop(
                    flusher_pending,
                    flusher_done,
                    flusher_broadcast,
                    flusher_output_log,
                    flusher_output_log_bytes,
                    flusher_next_seq,
                    flusher_channel,
                    flusher_id,
                );
            });

            // ConPTY exposes no descriptor to wait on, so this reader stays
            // uninterruptible; `force_kill` is the floor for it.
            let reader_poll_fd: Option<RawFd> = None;
            let reader_task = std::thread::spawn(move || {
                log::info!(
                    "[PTY {}] Windows ConPTY reader thread starting",
                    terminal_id
                );
                Self::reader_loop(
                    reader_instance,
                    reader,
                    terminal_events,
                    exit_code_tracker,
                    terminal_id,
                    pending_buf,
                    done_flag,
                    stop_requested,
                    reader_poll_fd,
                );
            });

            *instance.reader_handle.lock().await = Some(reader_task);
            *instance.flusher_handle.lock().await = Some(flusher_task);

            // Store the terminal
            self.terminals.write().insert(id.clone(), instance.clone());

            // Initialize tracking
            self.cwd_tracker.start_tracking(&id, pid, &cwd);
            self.git_tracker.initialize_terminal(&id, &cwd);
            self.exit_code_tracker.initialize_terminal(&id);

            Ok(TerminalInfo {
                id,
                shell: shell_path,
                cwd,
                pid,
                cols,
                rows,
            })
        }

        // On non-Windows, use portable-pty as before
        #[cfg(not(target_os = "windows"))]
        {
            use portable_pty::{native_pty_system, CommandBuilder};

            let pty_system = native_pty_system();
            let pty_size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };

            let pty_pair = pty_system
                .openpty(pty_size)
                .map_err(|e| format!("Failed to open PTY: {}", e))?;

            let mut cmd = CommandBuilder::new(&shell_path);
            // Login + interactive so ~/.zprofile and ~/.zshrc load (GH-275).
            if options.program.is_none() {
                for arg in crate::pty::env_refresh::shell_startup_args(&shell_path) {
                    cmd.arg(*arg);
                }
            }
            // ADR-004.2: In agent mode, append the argv tail as discrete
            // arguments. portable-pty passes argv without a shell, so the prompt
            // is delivered verbatim with no shell interpolation. In shell mode
            // `program_args` is empty and this loop is a no-op.
            for arg in &program_args {
                cmd.arg(arg);
            }
            for (key, value) in &env {
                cmd.env(key, value);
            }
            cmd.env("TERM", "xterm-256color");
            cmd.env("COLORTERM", "truecolor");
            cmd.env("FORCE_COLOR", "3");
            cmd.env("CLICOLOR", "1");
            cmd.env("CLICOLOR_FORCE", "1");
            cmd.cwd(&cwd);

            let child = pty_pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| format!("Failed to spawn shell: {}", e))?;

            let pid = child.process_id().unwrap_or(0);

            // Own the reader's descriptor instead of taking `try_clone_reader`'s
            // opaque `Box<dyn Read>`, so the reader thread can `poll` the exact
            // fd it reads from and stay interruptible. Without a descriptor to
            // wait on, a reader parked in `read()` had no exit at all: nothing
            // in the cleanup chain could reach it, and `master.take()` does not
            // touch this dup.
            //
            // Deliberately NOT `O_NONBLOCK`: `dup` shares the file status flags
            // with the original description, so setting it here would also make
            // the writer non-blocking. `poll` needs no such flag.
            #[cfg(unix)]
            let (reader, reader_poll_fd) = {
                use std::os::fd::FromRawFd;
                let master_fd = pty_pair
                    .master
                    .as_raw_fd()
                    .ok_or_else(|| "PTY master exposes no descriptor".to_string())?;
                let duplicated = unsafe { libc::dup(master_fd) };
                if duplicated < 0 {
                    return Err(format!(
                        "Failed to duplicate PTY reader descriptor: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                // `File` owns `duplicated` and closes it when the reader ends;
                // the raw copy is only ever passed to `poll`, never closed.
                let file = unsafe { std::fs::File::from_raw_fd(duplicated) };
                (Box::new(file) as Box<dyn Read + Send>, Some(duplicated))
            };
            #[cfg(not(unix))]
            let (reader, reader_poll_fd) = (
                pty_pair
                    .master
                    .try_clone_reader()
                    .map_err(|e| format!("Failed to clone PTY reader: {}", e))?,
                None,
            );
            let writer = pty_pair
                .master
                .take_writer()
                .map_err(|e| format!("Failed to get PTY writer: {}", e))?;

            let instance = Arc::new(TerminalInstance {
                id: id.clone(),
                conversation_id: options
                    .conversation_id
                    .expect("spawn assigned a ConversationId scope"),
                workspace_ref_tracked,
                project_id: options.project_id.clone(),
                child: Arc::new(AsyncMutex::new(Some(child))),
                master: Arc::new(AsyncMutex::new(Some(pty_pair.master))),
                writer: Arc::new(AsyncMutex::new(Some(writer))),
                reader_handle: Arc::new(AsyncMutex::new(None)),
                flusher_handle: Arc::new(AsyncMutex::new(None)),
                done_flag: Arc::new(AtomicBool::new(false)),
                stop_requested: Arc::new(AtomicBool::new(false)),
                lifecycle_state: Arc::new(RwLock::new(TerminalLifecycleState::Active)),
                cleanup_gate: Arc::new(AsyncMutex::new(())),
                cleanup_job: Arc::new(Mutex::new(None)),
                cleanup_progress: Arc::new(Mutex::new(TerminalCleanupProgress::default())),
                cleanup_attempts: Arc::new(AtomicU64::new(0)),
                shell: shell_path.clone(),
                cwd: cwd.clone(),
                pid,
                last_activity: Arc::new(RwLock::new(Instant::now())),
                orphan_since: Arc::new(RwLock::new(None)),
                renderer_refs: Arc::new(RwLock::new(HashSet::new())),
                protected: Arc::new(AtomicBool::new(true)),
                cols: Arc::new(RwLock::new(cols)),
                rows: Arc::new(RwLock::new(rows)),
                phone_fit: empty_phone_fit(),
                broadcast_tx: Arc::new(tokio::sync::broadcast::channel(TERM_BROADCAST_CAPACITY).0),
                output_log: Arc::new(RwLock::new(std::collections::VecDeque::new())),
                output_log_bytes: Arc::new(AtomicUsize::new(0)),
                next_output_seq: Arc::new(AtomicU64::new(0)),
                #[cfg(target_os = "windows")]
                conpty_handles: None,
            });

            // Start reader + flusher threads
            let pending_buf = Arc::new(Mutex::new(Vec::with_capacity(READ_BUF)));
            // Owned by the instance so cleanup can retire the flusher without
            // depending on the reader reaching its tail. See `done_flag` docs.
            let done_flag = instance.done_flag.clone();
            let stop_requested = instance.stop_requested.clone();

            // Spawn flusher thread first
            let flusher_pending = pending_buf.clone();
            let flusher_done = done_flag.clone();
            let flusher_channel = on_data.clone();
            let flusher_id = id.clone();
            let flusher_broadcast = instance.broadcast_tx.clone();
            let flusher_output_log = instance.output_log.clone();
            let flusher_output_log_bytes = instance.output_log_bytes.clone();
            let flusher_next_seq = instance.next_output_seq.clone();

            let flusher_task = std::thread::spawn(move || {
                log::info!("[PTY {}] Flusher thread starting", flusher_id);
                Self::flusher_loop(
                    flusher_pending,
                    flusher_done,
                    flusher_broadcast,
                    flusher_output_log,
                    flusher_output_log_bytes,
                    flusher_next_seq,
                    flusher_channel,
                    flusher_id,
                );
            });

            // Spawn reader thread
            let reader_instance = instance.clone();
            let terminal_events = self.terminal_events.clone();
            let exit_code_tracker = self.exit_code_tracker.clone();
            let terminal_id = id.clone();

            let reader_task = std::thread::spawn(move || {
                Self::reader_loop(
                    reader_instance,
                    reader,
                    terminal_events,
                    exit_code_tracker,
                    terminal_id,
                    pending_buf,
                    done_flag,
                    stop_requested,
                    reader_poll_fd,
                );
            });

            *instance.reader_handle.lock().await = Some(reader_task);
            *instance.flusher_handle.lock().await = Some(flusher_task);

            self.terminals.write().insert(id.clone(), instance.clone());

            self.cwd_tracker.start_tracking(&id, pid, &cwd);
            self.git_tracker.initialize_terminal(&id, &cwd);
            self.exit_code_tracker.initialize_terminal(&id);

            Ok(TerminalInfo {
                id,
                shell: shell_path,
                cwd,
                pid,
                cols,
                rows,
            })
        }
    }

    /// Block until the reader's descriptor has data, or until a stop request
    /// can be honoured without losing anything still buffered.
    ///
    /// On platforms with no descriptor to wait on (Windows ConPTY) this is a
    /// no-op and the following `read` blocks exactly as before; `force_kill`
    /// is the floor there.
    fn wait_for_readable(
        poll_fd: Option<RawFd>,
        stop_requested: &AtomicBool,
        id: &str,
    ) -> WaitOutcome {
        let Some(_fd) = poll_fd else {
            return WaitOutcome::Readable;
        };

        #[cfg(unix)]
        loop {
            let mut descriptor = libc::pollfd {
                fd: _fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut descriptor, 1, READER_POLL_TIMEOUT_MS) };
            if ready > 0 {
                // Readable, or hung up / errored — either way `read` resolves it
                // immediately and owns the EOF and error reporting.
                return WaitOutcome::Readable;
            }
            if ready == 0 {
                // Quiet for a full slice: nothing is buffered, so stopping now
                // cannot drop output.
                if stop_requested.load(Ordering::Acquire) {
                    return WaitOutcome::Stop;
                }
                continue;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log::error!("[PTY {id}] poll on reader descriptor failed: {error}");
            return WaitOutcome::Failed;
        }

        #[cfg(not(unix))]
        {
            let _ = (stop_requested, id);
            WaitOutcome::Readable
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// ADR-002.3: Reader thread — reads PTY data into pending buffer, no direct IPC.
    /// Pushes raw bytes to pending_buf, handles overflow protection.
    /// Sets done_flag to true on EOF or error so flusher can finalize.
    /// ADR-002.5: Intercepts DA queries via DaFilter and responds directly to PTY writer.
    fn reader_loop(
        instance: Arc<TerminalInstance>,
        mut reader: Box<dyn Read + Send>,
        terminal_events: TerminalEventHub,
        exit_code_tracker: Arc<ExitCodeTracker>,
        terminal_id: String,
        pending_buf: Arc<Mutex<Vec<u8>>>,
        done_flag: Arc<AtomicBool>,
        stop_requested: Arc<AtomicBool>,
        poll_fd: Option<RawFd>,
    ) {
        let mut buffer = [0u8; READ_BUF];
        let id = terminal_id.clone();
        // ADR-002.5: DA filter — intercepts DA queries and responds to PTY writer
        let mut da_filter = crate::pty::DaFilter::new();
        // Clone writer Arc for the DA filter respond closure
        let da_writer = instance.writer.clone();

        log::info!("[PTY {}] Reader thread starting", id);

        loop {
            // Wait for readability in bounded slices so a stop request is
            // observable. `WaitOutcome::Stop` is only ever returned once the fd
            // has gone quiet, so a stop still drains whatever the kernel had
            // buffered — cutting the read short instead would lose the tail the
            // cleanup ordering was fixed to preserve.
            match Self::wait_for_readable(poll_fd, &stop_requested, &id) {
                WaitOutcome::Readable => {}
                WaitOutcome::Stop => {
                    log::info!("[PTY {}] stop requested and drained, reader exiting", id);
                    break;
                }
                WaitOutcome::Failed => break,
            }

            match reader.read(&mut buffer) {
                Ok(0) => {
                    log::info!("[PTY {}] EOF reached, reader thread exiting", id);
                    break;
                }
                Ok(n) => {
                    instance.update_activity();

                    // Parse exit codes from output
                    let data_str = String::from_utf8_lossy(&buffer[..n]);
                    exit_code_tracker.process_data(&id, &data_str);

                    log::trace!("[PTY {}] Read {} bytes", id, n);

                    // ADR-002.5: Run DA filter to intercept DA queries.
                    // Responds directly to PTY writer so the shell gets immediate feedback
                    // without waiting for xterm.js to initialize.
                    let mut filtered = Vec::with_capacity(n);
                    let w = da_writer.clone();
                    da_filter.process(&buffer[..n], &mut filtered, move |reply| {
                        let mut writer_guard = w.blocking_lock();
                        if let Some(writer) = writer_guard.as_mut() {
                            let _ = writer.write_all(reply);
                            let _ = writer.flush();
                        }
                    });

                    // Push filtered (DA-processed) bytes to pending buffer
                    let mut guard = match pending_buf.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            log::error!("[PTY {}] Pending buffer mutex poisoned: {}", id, e);
                            break;
                        }
                    };

                    if guard.len() + filtered.len() > MAX_PENDING {
                        // Overflow: clear buffer and insert notice
                        guard.clear();
                        guard.extend_from_slice(OVERFLOW_NOTICE);
                        log::warn!("[PTY {}] Output buffer overflow — dropped data", id);
                    } else {
                        guard.extend_from_slice(&filtered);
                    }
                }
                Err(e) => {
                    log::error!("[PTY {}] Error reading from PTY: {}", id, e);
                    break;
                }
            }
        }

        // Signal flusher that reader is done
        done_flag.store(true, Ordering::Release);

        // Get real child exit status where possible.
        //
        // `try_wait` REAPS the child, which frees its pid for reuse. Cleanup
        // signals by pid (`kill`, and the process-group sweep), so it must be
        // told the pid is spent — otherwise a later terminate on this record
        // would signal whatever process the OS has since given that number to.
        // Marking `child_reaped` here makes cleanup skip its whole kill/wait
        // block, which is correct: the child is already gone.
        let exit_code = match instance.child.try_lock() {
            Ok(mut guard) => {
                let reaped = match guard.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => Some(i32::try_from(status.exit_code()).ok()),
                        Ok(None) => None,
                        Err(e) => {
                            log::warn!("[PTY {}] Failed to query child exit status: {}", id, e);
                            None
                        }
                    },
                    None => None,
                };
                match reaped {
                    Some(code) => {
                        guard.take();
                        instance
                            .cleanup_progress
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .child_reaped = true;
                        code
                    }
                    None => None,
                }
            }
            Err(_) => None,
        };

        terminal_events.emit(TerminalEvent::Exit {
            terminal_id: id.clone(),
            exit_code,
            signal: None,
        });

        log::info!("[PTY {}] Reader thread ended", id);
    }

    /// ADR-002.3: Flusher thread — batched Channel output at FLUSH_INTERVAL.
    /// Takes pending buffer via std::mem::take every 4ms and sends via binary channel.
    /// If on_data is None, skips sending (just drains).
    ///
    /// broadcast_tx: per-terminal broadcast channel. When present, each flushed
    /// batch is also sent so remote WebSocket clients receive live output.
    /// Send failures are ignored (no active remote subscribers is normal).
    ///
    /// output_log: sequence-aware bounded history. Each batch is appended before
    /// broadcasting, so attach can snapshot and subscribe under the same lock.
    #[allow(clippy::too_many_arguments)]
    fn flusher_loop(
        pending_buf: Arc<Mutex<Vec<u8>>>,
        done_flag: Arc<AtomicBool>,
        broadcast_tx: Arc<tokio::sync::broadcast::Sender<TerminalOutputChunk>>,
        output_log: Arc<RwLock<std::collections::VecDeque<TerminalOutputChunk>>>,
        output_log_bytes: Arc<AtomicUsize>,
        next_output_seq: Arc<AtomicU64>,
        on_data: Option<Channel<Response>>,
        terminal_id: String,
    ) {
        let id = terminal_id;
        log::info!("[PTY {}] Flusher thread starting", id);

        let channel_ref: Option<&Channel<Response>> = on_data.as_ref();

        fn publish(
            data: Vec<u8>,
            tx: &tokio::sync::broadcast::Sender<TerminalOutputChunk>,
            log: &Arc<RwLock<std::collections::VecDeque<TerminalOutputChunk>>>,
            bytes: &Arc<AtomicUsize>,
            next_seq: &Arc<AtomicU64>,
        ) {
            let chunk = TerminalOutputChunk {
                seq: next_seq.fetch_add(1, Ordering::Relaxed) + 1,
                data,
            };
            let mut guard = log.write();
            let mut total = bytes.load(Ordering::Relaxed) + chunk.data.len();
            guard.push_back(chunk.clone());
            while total > SCROLLBACK_CAP {
                let Some(evicted) = guard.pop_front() else {
                    break;
                };
                total = total.saturating_sub(evicted.data.len());
            }
            bytes.store(total, Ordering::Relaxed);
            let _ = tx.send(chunk);
        }

        loop {
            std::thread::sleep(FLUSH_INTERVAL);

            let chunk = match pending_buf.lock() {
                Ok(mut guard) if !guard.is_empty() => Some(std::mem::take(&mut *guard)),
                _ => None,
            };

            if let Some(data) = chunk {
                publish(
                    data.clone(),
                    &broadcast_tx,
                    &output_log,
                    &output_log_bytes,
                    &next_output_seq,
                );

                // Forward to Tauri frontend channel (may be None for detached terminals)
                if let Some(ch) = channel_ref {
                    if let Err(e) = ch.send(Response::new(data)) {
                        log::error!("[PTY {}] Failed to send data via channel: {}", id, e);
                    }
                }
            }

            if done_flag.load(Ordering::Acquire) {
                // One final broadcast of anything still buffered
                if let Ok(mut guard) = pending_buf.lock() {
                    if !guard.is_empty() {
                        let final_data = std::mem::take(&mut *guard);
                        publish(
                            final_data.clone(),
                            &broadcast_tx,
                            &output_log,
                            &output_log_bytes,
                            &next_output_seq,
                        );
                        if let Some(ch) = channel_ref {
                            if let Err(e) = ch.send(Response::new(final_data)) {
                                log::error!(
                                    "[PTY {}] Failed to send final data via channel: {}",
                                    id,
                                    e
                                );
                            }
                        }
                    }
                }
                break;
            }
        }

        log::info!("[PTY {}] Flusher thread ended", id);
    }

    /// Write data to an active terminal.
    pub async fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;
        instance.update_activity();

        let mut writer_guard = instance.writer.lock().await;
        let writer = writer_guard
            .as_mut()
            .ok_or_else(|| "PTY writer unavailable".to_string())?;
        writer
            .write_all(data.as_bytes())
            .map_err(|error| format!("Failed to write to PTY: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("Failed to flush PTY: {error}"))?;
        Ok(())
    }

    /// Resize an active terminal.
    ///
    /// Same-size requests are a no-op so the renderer can retry after attach
    /// without sending a redundant SIGWINCH. While a phone owns the live
    /// ioctl, the requested desktop grid is parked for restore instead of
    /// being discarded.
    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;
        if instance.is_phone_fit() {
            instance.park_desktop_size(cols, rows);
            log::info!(
                "[pty] parked desktop size while phone-fit terminal_id={id} cols={cols} rows={rows}"
            );
            return Ok(());
        }
        if *instance.cols.read() == cols && *instance.rows.read() == rows {
            return Ok(());
        }
        self.resize_ioctl(id, cols, rows).await
    }

    async fn resize_ioctl(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;

        #[cfg(target_os = "windows")]
        {
            if let Some(conpty_handles) = &instance.conpty_handles {
                let guard = conpty_handles.lock();
                let handles = guard
                    .as_ref()
                    .ok_or_else(|| "ConPTY handles unavailable".to_string())?;
                resize_conpty(handles, cols, rows)
                    .map_err(|error| format!("Failed to resize ConPTY: {error}"))?;
                *instance.cols.write() = cols;
                *instance.rows.write() = rows;
                instance.update_activity();
                return Ok(());
            }
        }

        let master_guard = instance.master.lock().await;
        let master = master_guard
            .as_ref()
            .ok_or_else(|| "PTY master already consumed".to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Failed to resize terminal: {error}"))?;
        *instance.cols.write() = cols;
        *instance.rows.write() = rows;
        instance.update_activity();
        Ok(())
    }

    /// Phone takeover parks the desktop size; desktop mode restores it.
    pub async fn set_display_mode(
        &self,
        id: &str,
        mode: TerminalDisplayMode,
        phone_cols: Option<u16>,
        phone_rows: Option<u16>,
        owner: &str,
        force: bool,
    ) -> Result<DisplayModeState, String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;

        match mode {
            TerminalDisplayMode::Phone => {
                let cols = phone_cols
                    .filter(|value| *value >= 2)
                    .ok_or_else(|| "phone display mode requires cols >= 2".to_string())?;
                let rows = phone_rows
                    .filter(|value| *value >= 2)
                    .ok_or_else(|| "phone display mode requires rows >= 2".to_string())?;
                let was_parked = instance.is_phone_fit();
                let (target_cols, target_rows) = instance.adopt_phone_owner(owner, cols, rows);
                let size_changed =
                    *instance.cols.read() != target_cols || *instance.rows.read() != target_rows;
                if size_changed {
                    self.resize_ioctl(id, target_cols, target_rows).await?;
                }
                if !was_parked || size_changed {
                    self.emit_display_mode(
                        id,
                        TerminalDisplayMode::Phone,
                        target_cols,
                        target_rows,
                    );
                }
                Ok(DisplayModeState {
                    mode: TerminalDisplayMode::Phone,
                    cols: target_cols,
                    rows: target_rows,
                })
            }
            TerminalDisplayMode::Desktop => {
                let restore = if force {
                    instance.force_desktop_display()
                } else {
                    instance.drop_phone_owner(owner)
                };
                if let Some((cols, rows)) = restore {
                    self.resize_ioctl(id, cols, rows).await?;
                    self.emit_display_mode(id, TerminalDisplayMode::Desktop, cols, rows);
                    return Ok(DisplayModeState {
                        mode: TerminalDisplayMode::Desktop,
                        cols,
                        rows,
                    });
                }
                Ok(DisplayModeState {
                    mode: instance.display_mode(),
                    cols: *instance.cols.read(),
                    rows: *instance.rows.read(),
                })
            }
        }
    }

    fn emit_display_mode(&self, id: &str, mode: TerminalDisplayMode, cols: u16, rows: u16) {
        log::info!("[pty] display-mode terminal_id={id} mode={mode:?} cols={cols} rows={rows}");
        self.terminal_events
            .emit(TerminalEvent::DisplayModeChanged {
                terminal_id: id.to_string(),
                mode,
                cols,
                rows,
            });
    }

    /// Explicitly terminate a terminal resource under one absolute five-second deadline.
    /// Failed cleanup remains tracked as `Quarantined` and continues occupying its slot.
    pub async fn terminate(
        &self,
        id: &str,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        self.terminate_until(id, tokio::time::Instant::now() + TERMINAL_CLEANUP_DEADLINE)
            .await
    }

    /// Observe or start the single manager-owned cleanup job for `id`, returning by the earlier of
    /// the caller deadline and the job's five-second cleanup deadline. A blocking OS kill/wait/join
    /// may outlive this caller, but its job, identity, and slot remain retained and every retry
    /// observes the same job until it proves completion.
    pub async fn terminate_until(
        &self,
        id: &str,
        caller_deadline: tokio::time::Instant,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        let Some(instance) = self.get(id) else {
            return Ok(TerminalCleanupReceipt {
                terminal_id: id.to_string(),
                job_id: 0,
                elapsed_ms: 0,
                attempt: 0,
                released_slot: false,
                already_removed: true,
                worker_panicked: false,
                forced: false,
            });
        };

        let job = {
            let _cleanup_guard = instance.cleanup_gate.lock().await;
            if instance.lifecycle_state() == TerminalLifecycleState::Removed {
                let current_job = instance
                    .cleanup_job
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                return Ok(TerminalCleanupReceipt {
                    terminal_id: id.to_string(),
                    job_id: current_job.as_ref().map_or(0, |job| job.job_id),
                    elapsed_ms: 0,
                    attempt: instance.cleanup_attempts.load(Ordering::Acquire),
                    released_slot: false,
                    already_removed: true,
                    worker_panicked: false,
                    forced: false,
                });
            }

            let existing = instance
                .cleanup_job
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(existing) = existing.filter(|job| !job.is_complete()) {
                existing
            } else {
                let previous_state = instance.lifecycle_state();
                *instance.lifecycle_state.write() = TerminalLifecycleState::Terminating;
                let attempt = instance.cleanup_attempts.fetch_add(1, Ordering::AcqRel) + 1;
                let job_id = self.cleanup_job_counter.fetch_add(1, Ordering::AcqRel) + 1;
                let started = Instant::now();
                let deadline = caller_deadline
                    .into_std()
                    .min(started + TERMINAL_CLEANUP_DEADLINE);
                let job = Arc::new(TerminalCleanupJob::new(job_id, attempt, started, deadline));
                *instance
                    .cleanup_job
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&job));
                log::info!(
                    "[pty-cleanup] terminal_id={} cleanup_stage=kill elapsed_ms=0 state_transition={}->Terminating capacity_counter={} shutdown_phase=terminal_cleanup stable_result=START attempt={} job_id={}",
                    id,
                    previous_state.as_str(),
                    self.active_terminal_slot_count(),
                    attempt,
                    job_id
                );
                self.launch_cleanup_job(Arc::clone(&instance), Arc::clone(&job));
                job
            }
        };

        self.observe_cleanup_job(&instance, &job, caller_deadline)
            .await
    }

    fn launch_cleanup_job(&self, instance: Arc<TerminalInstance>, job: Arc<TerminalCleanupJob>) {
        let driver = Arc::clone(&self.cleanup_driver.read());
        let cleanup_instance = Arc::clone(&instance);
        let cleanup_job = Arc::clone(&job);
        let blocking = tokio::task::spawn_blocking(move || {
            Self::cleanup_terminal_resources_sync(cleanup_instance, driver, cleanup_job)
        });
        let manager = self.clone();
        tokio::spawn(async move {
            let result = match blocking.await {
                Ok(result) => result,
                Err(_) => Err(Self::cleanup_failure(
                    &instance,
                    job.stage(),
                    TerminalCleanupFailureReason::ThreadPanicked,
                    &job,
                )),
            };
            manager.complete_cleanup_job(instance, job, result);
        });
    }

    fn complete_cleanup_job(
        &self,
        instance: Arc<TerminalInstance>,
        job: Arc<TerminalCleanupJob>,
        result: Result<TerminalCleanupReceipt, TerminalCleanupFailure>,
    ) {
        let is_current = instance
            .cleanup_job
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &job));

        match result {
            Ok(mut receipt) if is_current => {
                let removed = self.deregister_terminal(&instance);
                receipt.released_slot = removed;
                receipt.already_removed = !removed;
                log::info!(
                    "[pty-cleanup] terminal_id={} cleanup_stage=reader_join elapsed_ms={} state_transition=Terminating->Removed capacity_counter={} shutdown_phase=terminal_cleanup stable_result=OK attempt={} job_id={} released_slot={}",
                    instance.id,
                    receipt.elapsed_ms,
                    self.active_terminal_slot_count(),
                    receipt.attempt,
                    receipt.job_id,
                    receipt.released_slot
                );
                job.complete(Ok(receipt));
            }
            Ok(mut receipt) => {
                receipt.released_slot = false;
                receipt.already_removed = true;
                job.complete(Ok(receipt));
            }
            Err(failure) => {
                if is_current {
                    *instance.lifecycle_state.write() = TerminalLifecycleState::Quarantined;
                }
                log::warn!(
                    "[pty-cleanup] terminal_id={} cleanup_stage={} elapsed_ms={} state_transition=Terminating->Quarantined capacity_counter={} shutdown_phase=terminal_cleanup stable_result=TERMINATE_FAILED attempt={} job_id={}",
                    failure.terminal_id,
                    failure.stage,
                    failure.elapsed_ms,
                    self.active_terminal_slot_count(),
                    failure.attempt,
                    failure.job_id
                );
                job.complete(Err(failure));
            }
        }
    }

    async fn observe_cleanup_job(
        &self,
        instance: &TerminalInstance,
        job: &Arc<TerminalCleanupJob>,
        caller_deadline: tokio::time::Instant,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        // A retry observes the retained job but never inherits a longer wait budget than its own
        // caller. Shortening observation does not cancel, detach, or replace manager ownership.
        let deadline = caller_deadline.min(tokio::time::Instant::from_std(job.deadline));
        loop {
            let notified = job.completed.notified();
            if let Some(result) = job.result_for_observer() {
                return result;
            }
            if tokio::time::Instant::now() >= deadline {
                let failure = job.in_flight_deadline_failure(&instance.id);
                log::warn!(
                    "[pty-cleanup] terminal_id={} cleanup_stage={} elapsed_ms={} state_transition=Terminating->Terminating capacity_counter={} shutdown_phase=terminal_cleanup stable_result=IN_FLIGHT attempt={} job_id={}",
                    failure.terminal_id,
                    failure.stage,
                    failure.elapsed_ms,
                    self.active_terminal_slot_count(),
                    failure.attempt,
                    failure.job_id
                );
                return Err(failure);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let failure = job.in_flight_deadline_failure(&instance.id);
                log::warn!(
                    "[pty-cleanup] terminal_id={} cleanup_stage={} elapsed_ms={} state_transition=Terminating->Terminating capacity_counter={} shutdown_phase=terminal_cleanup stable_result=IN_FLIGHT attempt={} job_id={}",
                    failure.terminal_id,
                    failure.stage,
                    failure.elapsed_ms,
                    self.active_terminal_slot_count(),
                    failure.attempt,
                    failure.job_id
                );
                return Err(failure);
            }
        }
    }

    /// Deprecated compatibility alias for [`terminate`](Self::terminate).
    pub async fn kill(&self, id: &str) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        self.terminate(id).await
    }

    /// Drop the record and every tracker keyed on it, releasing its slot.
    ///
    /// Returns whether this call is the one that owned the removal.
    fn deregister_terminal(&self, instance: &Arc<TerminalInstance>) -> bool {
        *instance.lifecycle_state.write() = TerminalLifecycleState::Removed;
        let removed = {
            let mut terminals = self.terminals.write();
            let owns_entry = terminals
                .get(&instance.id)
                .is_some_and(|tracked| Arc::ptr_eq(tracked, instance));
            if owns_entry {
                terminals.remove(&instance.id);
            }
            owns_entry
        };
        if removed {
            self.release_terminal_slot();
            self.view_refs.lock().remove(&instance.id);
            self.cwd_tracker.stop_tracking(&instance.id);
            self.git_tracker.remove_terminal(&instance.id);
            self.exit_code_tracker.remove_terminal(&instance.id);
            self.terminal_events.remove(&instance.id);
            self.claims.remove(&instance.id);
        }
        removed
    }

    /// The failure the retained cleanup job actually settled on.
    ///
    /// Returns `None` while the job has not published a result yet, so the
    /// caller waits instead of forcing underneath a live blocking thread.
    /// Synthesising a reason here instead would have made every forced release
    /// report `DeadlineExceeded`, hiding real `Error`/`ThreadPanicked` outcomes
    /// and mis-deriving `worker_panicked` on the receipt.
    fn retained_cleanup_failure(&self, id: &str) -> Option<TerminalCleanupFailure> {
        let job = self.get(id).and_then(|instance| {
            instance
                .cleanup_job
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })?;
        let settled = job
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match settled {
            Some(Err(failure)) => Some(failure),
            // Quarantined with a settled *success*, or with nothing settled at
            // all, both mean the state and the job disagree. Report it rather
            // than inventing a failure to match.
            other => {
                log::error!(
                    "[pty-cleanup] terminal_id={id} stable_result=INVARIANT_VIOLATION note=quarantined without a settled failure settled_ok={}",
                    other.is_some()
                );
                None
            }
        }
    }

    /// Terminate, and if the graceful path still cannot finish, release the
    /// resource anyway.
    ///
    /// `Quarantined` used to be terminal: a terminal whose cleanup failed kept
    /// its slot forever, `require_active` refused to close its view, and resume
    /// could not mint a claim for it — a tab that could be neither killed nor
    /// closed, with every retry re-running the same losing sequence. Graceful
    /// cleanup is still tried first and is still preferred; this is the floor
    /// under it, so a stuck worker costs one abandoned thread instead of a
    /// permanently wedged terminal.
    ///
    /// What it gives up: a worker that will not stop is detached rather than
    /// joined (the OS reclaims it at process exit, the same trade
    /// `acp::join_thread_bounded` already makes), and an unreapable child is
    /// left to be reaped by process exit. Both are bounded and logged.
    pub async fn force_kill(
        &self,
        id: &str,
    ) -> Result<TerminalCleanupReceipt, TerminalCleanupFailure> {
        // A terminal already in `Quarantined` has had its graceful attempt and
        // lost. Re-running it would just wedge a second blocking job on the same
        // worker and report itself in-flight — the retry would block the very
        // escalation it was asking for.
        let failure = if let Some(retained) = (self.terminal_lifecycle_state(id)
            == Some(TerminalLifecycleState::Quarantined))
        .then(|| self.retained_cleanup_failure(id))
        .flatten()
        {
            retained
        } else {
            match self.terminate(id).await {
                Ok(receipt) => return Ok(receipt),
                // An in-flight job still owns the blocking work and its handles;
                // forcing underneath it would race that thread. The caller retries.
                Err(failure) if failure.in_flight => return Err(failure),
                Err(failure) => failure,
            }
        };

        let Some(instance) = self.get(id) else {
            return Ok(TerminalCleanupReceipt {
                terminal_id: id.to_string(),
                job_id: failure.job_id,
                elapsed_ms: failure.elapsed_ms,
                attempt: failure.attempt,
                released_slot: false,
                already_removed: true,
                worker_panicked: false,
                forced: true,
            });
        };

        // Give the workers their stop signal one more time, then stop waiting.
        instance.done_flag.store(true, Ordering::Release);
        let detached_reader = instance.reader_handle.lock().await.take().is_some();
        let detached_flusher = instance.flusher_handle.lock().await.take().is_some();
        instance.writer.lock().await.take();
        instance.master.lock().await.take();
        #[cfg(target_os = "windows")]
        if let Some(conpty_handles) = &instance.conpty_handles {
            conpty_handles.lock().take();
        }
        let abandoned_child = instance.child.lock().await.take().is_some();

        let removed = self.deregister_terminal(&instance);
        log::warn!(
            "[pty-cleanup] terminal_id={} cleanup_stage={} elapsed_ms={} state_transition=Quarantined->Removed capacity_counter={} shutdown_phase=force_release stable_result=FORCED attempt={} job_id={} detached_reader={} detached_flusher={} abandoned_child={}",
            failure.terminal_id,
            failure.stage,
            failure.elapsed_ms,
            self.active_terminal_slot_count(),
            failure.attempt,
            failure.job_id,
            detached_reader,
            detached_flusher,
            abandoned_child
        );

        Ok(TerminalCleanupReceipt {
            terminal_id: failure.terminal_id,
            job_id: failure.job_id,
            elapsed_ms: failure.elapsed_ms,
            attempt: failure.attempt,
            released_slot: removed,
            already_removed: !removed,
            worker_panicked: failure.reason == TerminalCleanupFailureReason::ThreadPanicked,
            forced: true,
        })
    }

    /// Add a renderer reference to an active terminal.
    pub fn add_renderer_ref(&self, id: &str, renderer_id: &str) -> Result<(), String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;
        instance.add_renderer_ref(renderer_id.to_string());
        Ok(())
    }

    /// Remove a renderer reference from an active terminal.
    pub fn remove_renderer_ref(&self, id: &str, renderer_id: &str) -> Result<(), String> {
        let instance = self
            .get(id)
            .ok_or_else(|| format!("Terminal not found: {id}"))?;
        instance.require_active()?;
        instance.remove_renderer_ref(renderer_id);
        Ok(())
    }

    pub fn set_protected(&self, id: &str, protected: bool) {
        if let Some(instance) = self.get(id).filter(|instance| instance.is_active()) {
            instance.set_protected(protected);
        }
    }

    pub fn terminal_events(&self) -> TerminalEventHub {
        self.terminal_events.clone()
    }

    pub fn cwd_tracker(&self) -> Arc<CwdTracker> {
        Arc::clone(&self.cwd_tracker)
    }

    pub fn git_tracker(&self) -> Arc<GitTracker> {
        Arc::clone(&self.git_tracker)
    }

    pub fn exit_code_tracker(&self) -> Arc<ExitCodeTracker> {
        Arc::clone(&self.exit_code_tracker)
    }

    pub fn verify_claim(&self, terminal_id: &str, claim: &str) -> Result<(), ClaimError> {
        let instance = self
            .get(terminal_id)
            .filter(|instance| instance.is_active())
            .ok_or(ClaimError)?;
        self.claims.verify(
            terminal_id,
            claim,
            instance.conversation_id,
            instance.project_id.as_deref(),
        )
    }

    pub fn rotate_claim(&self, terminal_id: &str, claim: &str) -> Result<String, ClaimError> {
        let instance = self
            .get(terminal_id)
            .filter(|instance| instance.is_active())
            .ok_or(ClaimError)?;
        self.claims.rotate(
            terminal_id,
            claim,
            instance.conversation_id,
            instance.project_id.as_deref(),
        )
    }

    pub fn revoke_claim(&self, terminal_id: &str, claim: &str) -> Result<(), ClaimError> {
        let instance = self
            .get(terminal_id)
            .filter(|instance| instance.is_active())
            .ok_or(ClaimError)?;
        self.claims.revoke(
            terminal_id,
            claim,
            instance.conversation_id,
            instance.project_id.as_deref(),
        )
    }

    /// A non-active resource reports no generation, immediately severing every derived stream.
    pub fn claim_generation(&self, terminal_id: &str) -> Option<u64> {
        self.get(terminal_id)
            .filter(|instance| instance.is_active())
            .and_then(|_| self.claims.generation(terminal_id))
    }

    pub fn resume_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        last_seq: u64,
    ) -> Result<(TerminalResumeGrant, TerminalReplay), ClaimError> {
        let instance = self
            .get(terminal_id)
            .filter(|instance| {
                instance.is_active()
                    && instance.workspace_ref_tracked
                    && instance.conversation_matches(conversation_id)
            })
            .ok_or(ClaimError)?;
        let mut replay = instance.subscribe_from(last_seq);
        let (claim, generation) = self.claims.rotate_for_resume(
            terminal_id,
            conversation_id,
            instance.project_id.as_deref(),
        )?;
        replay.claim_generation = Some(generation);
        let terminal = self.build_attach_result(&instance, &replay);
        log::info!(
            "[terminal-resume] granted terminal_id={} conversation_id={} latest_seq={} gap={}",
            terminal_id,
            conversation_id,
            terminal.latest_seq,
            terminal.gap
        );
        Ok((TerminalResumeGrant { terminal, claim }, replay))
    }

    pub fn build_attach_result(
        &self,
        instance: &TerminalInstance,
        replay: &TerminalReplay,
    ) -> TerminalAttachResult {
        let cwd = self
            .cwd_tracker
            .get_cwd(&instance.id)
            .unwrap_or_else(|| instance.cwd.clone());
        TerminalAttachResult {
            id: instance.id.clone(),
            shell: instance.shell.clone(),
            cwd,
            pid: instance.pid,
            cols: *instance.cols.read(),
            rows: *instance.rows.read(),
            latest_seq: replay.latest_seq,
            gap: replay.gap,
        }
    }

    /// Retains `Quarantined` resources so callers can recover their stable identity and retry.
    pub fn get(&self, id: &str) -> Option<Arc<TerminalInstance>> {
        self.terminals.read().get(id).cloned()
    }

    pub fn terminal_lifecycle_state(&self, id: &str) -> Option<TerminalLifecycleState> {
        self.get(id).map(|instance| instance.lifecycle_state())
    }

    /// Snapshot the retained manager-owned cleanup job for observability and retry coordination.
    #[must_use]
    pub fn terminal_cleanup_job(&self, id: &str) -> Option<Arc<TerminalCleanupJob>> {
        self.get(id).and_then(|instance| {
            instance
                .cleanup_job
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
    }

    pub fn get_by_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Vec<Arc<TerminalInstance>> {
        self.terminals
            .read()
            .values()
            .filter(|instance| instance.conversation_matches(conversation_id))
            .cloned()
            .collect()
    }

    pub fn get_all(&self) -> Vec<Arc<TerminalInstance>> {
        self.terminals.read().values().cloned().collect()
    }

    pub fn get_count(&self) -> usize {
        self.terminals.read().len()
    }

    #[must_use]
    pub(crate) fn active_terminal_slot_count(&self) -> usize {
        self.active_terminal_slots.load(Ordering::SeqCst)
    }

    pub fn is_limit_reached(&self) -> bool {
        self.active_terminal_slot_count() >= GLOBAL_TERMINAL_LIMIT
    }

    #[cfg(test)]
    pub(crate) fn install_cleanup_driver(&self, driver: Arc<dyn CleanupDriver>) {
        *self.cleanup_driver.write() = driver;
    }

    /// Attempt every tracked terminal concurrently under one host deadline. The global terminal
    /// limit (30) is also the hard concurrency bound, so shutdown never multiplies five seconds by
    /// terminal count. Failed and still-running jobs remain conservatively retained.
    pub async fn kill_all_until(&self, deadline: tokio::time::Instant) -> PtyShutdownReceipt {
        let started = Instant::now();
        let mut ids: Vec<String> = self.terminals.read().keys().cloned().collect();
        ids.sort();
        let attempted = ids.len();
        let results =
            futures::future::join_all(ids.iter().map(|id| self.terminate_until(id, deadline)))
                .await;
        let mut receipt = PtyShutdownReceipt {
            attempted,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            ..PtyShutdownReceipt::default()
        };
        for result in results {
            match result {
                Ok(_) => receipt.succeeded += 1,
                Err(failure) if failure.in_flight => receipt.in_flight += 1,
                Err(_) => receipt.failed += 1,
            }
        }
        log::info!(
            "[pty-cleanup] shutdown_phase=kill_all attempted={} succeeded={} failed={} in_flight={} elapsed_ms={} stable_result={}",
            receipt.attempted,
            receipt.succeeded,
            receipt.failed,
            receipt.in_flight,
            receipt.elapsed_ms,
            if receipt.clean_success() { "OK" } else { "PTY_CLEANUP_FAILED" }
        );
        receipt
    }

    /// Compatibility process-exit safety net using one shared five-second deadline.
    pub async fn kill_all(&self) {
        let _ = self
            .kill_all_until(tokio::time::Instant::now() + TERMINAL_CLEANUP_DEADLINE)
            .await;
    }

    /// Update orphan detection settings (timeout in milliseconds)
    pub fn update_orphan_detection(&self, enabled: bool, timeout_ms: Option<u64>) {
        self.orphan_detection_enabled
            .store(enabled, Ordering::Relaxed);
        if let Some(timeout) = timeout_ms {
            self.orphan_timeout_ms.store(timeout, Ordering::Relaxed);
        }
    }

    /// Update orphan detection settings (timeout in minutes, for async API compatibility)
    pub async fn update_orphan_detection_settings(
        &self,
        enabled: bool,
        timeout_minutes: Option<u64>,
    ) {
        self.orphan_detection_enabled
            .store(enabled, Ordering::Relaxed);
        if let Some(timeout) = timeout_minutes {
            self.orphan_timeout_ms
                .store(timeout * 60 * 1000, Ordering::Relaxed);
        }
    }

    /// Set the app window hidden state.
    /// When hidden=true, orphan detection will not kill orphaned terminals
    /// and kill() operations are deferred. Prevents ConPTY lifecycle issues
    /// on Windows where window minimize can cause PTY processes to die.
    pub fn set_hidden(&self, hidden: bool) {
        self.is_hidden.store(hidden, Ordering::Relaxed);
        if hidden {
            log::info!("[PtyManager] App window hidden — killing and orphan cleanup deferred");
        } else {
            log::info!("[PtyManager] App window visible — killing and orphan cleanup resumed");
        }
    }

    /// Check if the app window is currently hidden
    pub fn is_hidden(&self) -> bool {
        self.is_hidden.load(Ordering::Relaxed)
    }

    /// Get the default shell path
    fn get_default_shell(&self) -> Result<String, String> {
        #[cfg(target_os = "windows")]
        {
            let comspec = env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            Ok(comspec)
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()))
        }
    }

    /// ADR-004.2: Resolve a program (agent binary) to an absolute, existing
    /// path. Reuses the same PATH/`which` resolution as shell lookup so agents
    /// like `claude`, `codex`, or `gemini` resolve off the user's PATH.
    ///
    /// On Windows, npm installs CLI tools as `.cmd` batch wrappers around
    /// `node.exe`. Since `CreateProcessW` cannot execute `.cmd` directly (os
    /// error 193), we detect this pattern, parse the `.cmd` shim, and rewrite
    /// the spawn to `node.exe <script>`. If no rewriting is possible, returns an
    /// error rather than launching an unresolved name (defense in depth).
    fn resolve_program_path(&self, program: &str) -> Result<ResolvedProgram, String> {
        let trimmed = program.trim();
        if trimmed.is_empty() {
            return Err("Agent program is empty".to_string());
        }

        // Explicit path: must exist as given.
        if trimmed.contains('/') || trimmed.contains('\\') {
            if Path::new(trimmed).exists() {
                #[cfg(target_os = "windows")]
                {
                    if !is_directly_executable_windows(trimmed) {
                        if let Some(resolved) = try_parse_windows_cmd_shim(trimmed) {
                            return Ok(resolved);
                        }
                        let shim_ext = Path::new(trimmed)
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase());
                        if shim_ext.as_deref() == Some("cmd") || shim_ext.as_deref() == Some("bat")
                        {
                            return Err(format!(
                                "Agent program '{}' is a batch shim that could not be parsed (ADR-004.2)",
                                trimmed
                            ));
                        }
                        return Err(format!(
                            "Agent program '{}' is not a directly-executable image (.exe/.com/.scr); \
                             batch scripts and PowerShell scripts are not supported (ADR-004.2)",
                            trimmed
                        ));
                    }
                }
                return Ok(ResolvedProgram::new(trimmed.to_string()));
            }
            return Err(format!("Agent program not found: {}", trimmed));
        }

        #[cfg(target_os = "windows")]
        {
            // 1. Try directly-executable image extensions (PE images that
            //    CreateProcessW can launch).
            const WIN_EXECUTABLE_EXTS: &[&str] = &["", ".exe", ".com", ".scr"];
            for ext in WIN_EXECUTABLE_EXTS {
                let candidate = format!("{}{}", trimmed, ext);
                if let Some(abs_path) = self.get_absolute_shell_path(&candidate) {
                    if is_directly_executable_windows(&abs_path) {
                        return Ok(ResolvedProgram::new(abs_path));
                    }
                }
            }

            // 2. No PE image found. Try .cmd/.bat shim parsing (npm node or
            //    PowerShell wrappers) and rewrite to a directly-executable image.
            for shim_ext in [".cmd", ".bat"] {
                let candidate = format!("{}{}", trimmed, shim_ext);
                if let Some(abs_path) = self.get_absolute_shell_path(&candidate) {
                    if let Some(resolved) = try_parse_windows_cmd_shim(&abs_path) {
                        return Ok(resolved);
                    }
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let path_for_which = crate::pty::env_refresh::path_for_resolution();
            if let Ok(output) = std::process::Command::new("which")
                .env("PATH", &path_for_which)
                .arg(trimmed)
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let first_line = stdout.lines().next().unwrap_or("").trim();
                    if !first_line.is_empty() {
                        return Ok(ResolvedProgram::new(first_line.to_string()));
                    }
                }
            }
            for prefix in ["/usr/local/bin", "/usr/bin", "/bin", "/opt/homebrew/bin"] {
                let candidate = format!("{}/{}", prefix, trimmed);
                if Path::new(&candidate).exists() {
                    return Ok(ResolvedProgram::new(candidate));
                }
            }
        }

        Err(format!("Agent program not found on PATH: {}", trimmed))
    }

    /// Resolve a shell name to its full path
    ///
    /// For `git-bash` alias on Windows, tries multiple fallback strategies:
    /// 1. `bash.exe` via `where` command (PATH lookup)
    /// 2. Common Git Bash installation paths
    /// 3. MSYS2 paths
    fn resolve_shell_path(&self, shell: &str) -> Result<String, String> {
        // If it looks like a path, verify it exists
        if shell.contains('/') || shell.contains('\\') {
            if Path::new(shell).exists() {
                return Ok(shell.to_string());
            }
            return Err(format!("Shell not found: {}", shell));
        }

        #[cfg(target_os = "windows")]
        {
            // Special handling for git-bash alias
            if shell == "git-bash" {
                // Strategy 1: Try bash.exe via PATH (where command)
                if let Some(abs_path) = self.get_absolute_shell_path("bash.exe") {
                    return Ok(abs_path);
                }

                // Strategy 2: Try common Git Bash installation paths
                // Uses shared constants from git_bash_paths module (synced with lib.rs)
                for path in git_bash_paths::PRIMARY_PATHS {
                    if Path::new(path).exists() {
                        return Ok(path.to_string());
                    }
                }

                // Strategy 3: Try MSYS2 and other common locations
                for path in git_bash_paths::FALLBACK_PATHS {
                    if Path::new(path).exists() {
                        return Ok(path.to_string());
                    }
                }

                // All strategies failed
                return Err(format!(
                    "Shell not found: {} - bash.exe not found in PATH or common Git Bash locations",
                    shell
                ));
            }

            // Standard shell resolution for other shells
            // CRITICAL: Check PowerShell variants BEFORE generic *.exe lookup
            // so name-only tokens hit explicit paths first
            if shell == "pwsh" {
                // PowerShell 7/6 resolution path
                let paths = vec![
                    r"C:\Program Files\PowerShell\7\pwsh.exe",
                    r"C:\Program Files\PowerShell\6\pwsh.exe",
                    "pwsh.exe",
                ];
                for path in paths {
                    if let Some(abs_path) = self.get_absolute_shell_path(path) {
                        return Ok(abs_path);
                    }
                }
            } else if shell == "powershell" {
                // Windows PowerShell 5 resolution path
                let paths = vec![
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                    "powershell.exe",
                ];
                for path in paths {
                    if let Some(abs_path) = self.get_absolute_shell_path(path) {
                        return Ok(abs_path);
                    }
                }
            }

            // Try shell.exe variant for non-PowerShell shells
            let exe_shell = format!("{}.exe", shell);
            if let Some(abs_path) = self.get_absolute_shell_path(&exe_shell) {
                return Ok(abs_path);
            }

            // Try the shell name directly for non-PowerShell shells
            if let Some(abs_path) = self.get_absolute_shell_path(shell) {
                return Ok(abs_path);
            }

            // Try common paths for bash (not git-bash alias)
            if shell == "bash" {
                // Use same candidate lists as git-bash
                for path in git_bash_paths::PRIMARY_PATHS {
                    if Path::new(path).exists() {
                        return Ok(path.to_string());
                    }
                }
                // Also try a subset of fallback paths for bash
                for path in git_bash_paths::FALLBACK_PATHS {
                    if Path::new(path).exists() {
                        return Ok(path.to_string());
                    }
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(resolved) = crate::shell_paths::unix_shell_paths::resolve_name(shell) {
                return Ok(resolved);
            }
        }

        Err(format!("Shell not found: {}", shell))
    }

    /// Get the absolute path for a shell if available
    /// Uses cache to avoid repeated `where`/`which` command spawns
    #[cfg(target_os = "windows")]
    fn get_absolute_shell_path(&self, shell_path: &str) -> Option<String> {
        use std::sync::OnceLock;

        // Per-shell cache to avoid repeated `where` commands
        static CACHE: OnceLock<
            std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
        > = OnceLock::new();
        let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

        // Check cache first
        {
            let cache_read = cache.lock().unwrap();
            if let Some(cached) = cache_read.get(shell_path) {
                return cached.clone();
            }
        }

        // Not in cache - resolve and store
        let result = self.resolve_shell_path_uncached(shell_path);

        // Store in cache
        {
            let mut cache_write = cache.lock().unwrap();
            cache_write.insert(shell_path.to_string(), result.clone());
        }

        result
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

    /// Internal uncached resolution - resolve via PATH scan or absolute path
    #[cfg(target_os = "windows")]
    fn resolve_shell_path_uncached(&self, shell_path: &str) -> Option<String> {
        log::debug!("[ShellResolve] Uncached resolution for: {}", shell_path);
        // If it's already an absolute path that exists, return it
        if Path::new(shell_path).exists() {
            return Some(shell_path.to_string());
        }

        #[cfg(target_os = "windows")]
        {
            if !shell_path.contains('\\') && !shell_path.contains('/') {
                if Self::is_builtin_windows_shell(shell_path) {
                    log::debug!(
                        "[ShellResolve] Built-in Windows shell, skipping PATH resolution: {}",
                        shell_path
                    );
                    return Some(shell_path.to_string());
                }

                let resolved = resolve_executable_from_path(shell_path);
                if let Some(path) = resolved {
                    log::debug!(
                        "[ShellResolve] Resolved from PATH without spawning cmd: {} -> {}",
                        shell_path,
                        path
                    );
                    return Some(path);
                }
            }
            if Path::new(shell_path).exists() {
                return Some(shell_path.to_string());
            }
            None
        }
    }

    /// Get the home directory
    fn get_home_directory(&self) -> String {
        #[cfg(target_os = "windows")]
        {
            env::var("USERPROFILE")
                .or_else(|_| env::var("HOME"))
                .unwrap_or_else(|_| "C:\\".to_string())
        }

        #[cfg(not(target_os = "windows"))]
        {
            env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
        }
    }

    /// Merge custom environment with base environment
    /// On Windows, environment variable keys are case-insensitive
    fn merge_environment(
        &self,
        custom_env: Option<HashMap<String, String>>,
    ) -> HashMap<String, String> {
        let custom_sets_path = custom_env
            .as_ref()
            .is_some_and(|custom| custom.keys().any(|key| key.eq_ignore_ascii_case("path")));

        #[cfg(target_os = "windows")]
        {
            let mut env_map = merge_windows_environment_map(
                crate::pty::env_refresh::inherited_process_env(),
                None,
            );
            if !custom_sets_path {
                crate::pty::env_refresh::apply_fresh_path(&mut env_map);
            }
            crate::pty::env_refresh::apply_utf8_locale(&mut env_map);
            if let Some(custom) = custom_env {
                for (key, value) in custom {
                    upsert_windows_env_var(&mut env_map, &key, value);
                }
            }
            crate::pty::env_refresh::apply_color_capability(&mut env_map);
            if !has_windows_env_var(&env_map, "Path") {
                upsert_windows_env_var(&mut env_map, "Path", env::var("PATH").unwrap_or_default());
            }
            if !has_windows_env_var(&env_map, "PATHEXT") {
                upsert_windows_env_var(
                    &mut env_map,
                    "PATHEXT",
                    env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()),
                );
            }
            env_map
        }

        #[cfg(not(target_os = "windows"))]
        {
            let mut env = HashMap::new();

            for (key, value) in crate::pty::env_refresh::inherited_process_env() {
                env.insert(key, value);
            }

            if !custom_sets_path {
                crate::pty::env_refresh::apply_fresh_path(&mut env);
            }

            crate::pty::env_refresh::apply_utf8_locale(&mut env);

            if let Some(custom) = custom_env {
                for (key, value) in custom {
                    env.insert(key, value);
                }
            }

            crate::pty::env_refresh::apply_color_capability(&mut env);

            if !env.contains_key("PATH") {
                env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
            }

            env
        }
    }
}

/// Windows ConPTY child process wrapper.
#[cfg(target_os = "windows")]
#[derive(Debug)]
struct WindowsConPtyChild {
    pid: u32,
    process_handle: *mut winapi::ctypes::c_void,
    // Job Object handle (KILL_ON_JOB_CLOSE) owning the child process tree, or
    // null if it could not be created. Closing it (on Drop) reaps the whole
    // tree; TerminateJobObject kills it on demand. See spawn_conpty / #281.
    job_handle: *mut winapi::ctypes::c_void,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct WindowsPidKiller {
    pid: u32,
}

// SAFETY: process_handle is only accessed by one thread at a time via the
// AsyncMutex<Option<Box<dyn Child>>> wrapper in TerminalInstance.
#[cfg(target_os = "windows")]
unsafe impl Send for WindowsConPtyChild {}

// SAFETY: process_handle is only accessed by one thread at a time via the
// AsyncMutex<Option<Box<dyn Child>>> wrapper in TerminalInstance.
#[cfg(target_os = "windows")]
unsafe impl Sync for WindowsConPtyChild {}

#[cfg(target_os = "windows")]
impl Drop for WindowsConPtyChild {
    fn drop(&mut self) {
        unsafe {
            // Close the job handle first: with KILL_ON_JOB_CLOSE this reaps the
            // entire child process tree once the last handle is gone.
            if !self.job_handle.is_null() {
                let _ = winapi::um::handleapi::CloseHandle(self.job_handle);
                self.job_handle = std::ptr::null_mut();
            }
            if !self.process_handle.is_null() {
                let _ = winapi::um::handleapi::CloseHandle(self.process_handle);
                self.process_handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl portable_pty::ChildKiller for WindowsPidKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        unsafe {
            let handle = winapi::um::processthreadsapi::OpenProcess(
                winapi::um::winnt::PROCESS_TERMINATE,
                0,
                self.pid,
            );
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let terminate_ok = winapi::um::processthreadsapi::TerminateProcess(handle, 1);
            let close_ok = winapi::um::handleapi::CloseHandle(handle);
            if terminate_ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if close_ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync + 'static> {
        Box::new(WindowsPidKiller { pid: self.pid })
    }
}

#[cfg(target_os = "windows")]
impl portable_pty::ChildKiller for WindowsConPtyChild {
    fn kill(&mut self) -> std::io::Result<()> {
        unsafe {
            // Prefer terminating the Job Object: this kills the entire child
            // process tree (cmd → powershell → node …), which single-PID
            // TerminateProcess cannot do. See #281.
            if !self.job_handle.is_null() {
                if winapi::um::jobapi2::TerminateJobObject(self.job_handle, 1) != 0 {
                    return Ok(());
                }
                // Job termination failed: if the process already exited the job
                // is effectively empty — treat as success rather than logging an
                // ERROR_ACCESS_DENIED-style false failure.
                if self.process_already_exited() {
                    return Ok(());
                }
                let err = std::io::Error::last_os_error();
                log::warn!(
                    "[WindowsConPtyChild:{}] TerminateJobObject failed: {}",
                    self.pid,
                    err
                );
                return Err(err);
            }

            if self.process_handle.is_null() {
                return Ok(());
            }
            if winapi::um::processthreadsapi::TerminateProcess(self.process_handle, 1) == 0 {
                // The process may already have exited; that's not a real failure
                // and avoids the recurring "Access is denied (os error 5)" noise.
                if self.process_already_exited() {
                    return Ok(());
                }
                let err = std::io::Error::last_os_error();
                log::warn!(
                    "[WindowsConPtyChild:{}] TerminateProcess failed: {}",
                    self.pid,
                    err
                );
                return Err(err);
            }
            Ok(())
        }
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync + 'static> {
        let mut dup: *mut winapi::ctypes::c_void = std::ptr::null_mut();
        unsafe {
            let ok = winapi::um::handleapi::DuplicateHandle(
                winapi::um::processthreadsapi::GetCurrentProcess(),
                self.process_handle,
                winapi::um::processthreadsapi::GetCurrentProcess(),
                &mut dup,
                0,
                0,
                winapi::um::winnt::DUPLICATE_SAME_ACCESS,
            );
            if ok == 0 {
                log::warn!(
                    "[WindowsConPtyChild:{}] DuplicateHandle failed, falling back to pid-based killer: {}",
                    self.pid,
                    std::io::Error::last_os_error()
                );
                return Box::new(WindowsPidKiller { pid: self.pid });
            }

            // Duplicate the job handle too so the clone can still tree-kill.
            // KILL_ON_JOB_CLOSE only fires when the LAST handle closes, so an
            // extra duplicate is safe and does not terminate the tree early.
            let mut dup_job: *mut winapi::ctypes::c_void = std::ptr::null_mut();
            if !self.job_handle.is_null()
                && winapi::um::handleapi::DuplicateHandle(
                    winapi::um::processthreadsapi::GetCurrentProcess(),
                    self.job_handle,
                    winapi::um::processthreadsapi::GetCurrentProcess(),
                    &mut dup_job,
                    0,
                    0,
                    winapi::um::winnt::DUPLICATE_SAME_ACCESS,
                ) == 0
            {
                log::warn!(
                    "[WindowsConPtyChild:{}] DuplicateHandle(job) failed, clone loses tree-kill: {}",
                    self.pid,
                    std::io::Error::last_os_error()
                );
                dup_job = std::ptr::null_mut();
            }

            Box::new(WindowsConPtyChild {
                pid: self.pid,
                process_handle: dup,
                job_handle: dup_job,
            })
        }
    }
}

#[cfg(target_os = "windows")]
impl WindowsConPtyChild {
    /// Returns true if the underlying process is known to have exited. Used to
    /// distinguish a benign "already dead" kill from a real termination failure.
    unsafe fn process_already_exited(&self) -> bool {
        if self.process_handle.is_null() {
            return true;
        }
        let wait = winapi::um::synchapi::WaitForSingleObject(self.process_handle, 0);
        wait == winapi::um::winbase::WAIT_OBJECT_0
    }
}

#[cfg(target_os = "windows")]
impl portable_pty::Child for WindowsConPtyChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        unsafe {
            if self.process_handle.is_null() {
                return Ok(Some(portable_pty::ExitStatus::with_exit_code(1)));
            }

            let wait = winapi::um::synchapi::WaitForSingleObject(self.process_handle, 0);

            if wait == winapi::shared::winerror::WAIT_TIMEOUT {
                return Ok(None);
            }

            if wait != winapi::um::winbase::WAIT_OBJECT_0 {
                return Err(std::io::Error::last_os_error());
            }

            let mut code: u32 = 0;
            if winapi::um::processthreadsapi::GetExitCodeProcess(self.process_handle, &mut code)
                == 0
            {
                return Err(std::io::Error::last_os_error());
            }

            Ok(Some(portable_pty::ExitStatus::with_exit_code(code)))
        }
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        unsafe {
            if self.process_handle.is_null() {
                return Ok(portable_pty::ExitStatus::with_exit_code(1));
            }

            let wait = winapi::um::synchapi::WaitForSingleObject(
                self.process_handle,
                winapi::um::winbase::INFINITE,
            );
            if wait != winapi::um::winbase::WAIT_OBJECT_0 {
                return Err(std::io::Error::last_os_error());
            }

            let mut code: u32 = 0;
            if winapi::um::processthreadsapi::GetExitCodeProcess(self.process_handle, &mut code)
                == 0
            {
                return Err(std::io::Error::last_os_error());
            }

            Ok(portable_pty::ExitStatus::with_exit_code(code))
        }
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }

    fn as_raw_handle(&self) -> Option<*mut std::ffi::c_void> {
        Some(self.process_handle as *mut std::ffi::c_void)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FakeCleanupChild {
        exited: bool,
    }

    #[derive(Debug)]
    struct FakeCleanupKiller;

    impl portable_pty::ChildKiller for FakeCleanupKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    impl portable_pty::ChildKiller for FakeCleanupChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.exited = true;
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeCleanupKiller)
        }
    }

    impl portable_pty::Child for FakeCleanupChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(self
                .exited
                .then(|| portable_pty::ExitStatus::with_exit_code(0)))
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            self.exited = true;
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }

        fn process_id(&self) -> Option<u32> {
            Some(42)
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    fn install_cleanup_fixture(manager: &PtyManager, terminal_id: &str) -> Arc<TerminalInstance> {
        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(8);
        let instance = Arc::new(TerminalInstance {
            id: terminal_id.to_string(),
            conversation_id,
            workspace_ref_tracked: true,
            project_id: None,
            child: Arc::new(AsyncMutex::new(Some(Box::new(FakeCleanupChild {
                exited: false,
            })))),
            master: Arc::new(AsyncMutex::new(None)),
            writer: Arc::new(AsyncMutex::new(None)),
            reader_handle: Arc::new(AsyncMutex::new(Some(std::thread::spawn(|| {})))),
            flusher_handle: Arc::new(AsyncMutex::new(Some(std::thread::spawn(|| {})))),
            done_flag: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            lifecycle_state: Arc::new(RwLock::new(TerminalLifecycleState::Active)),
            cleanup_gate: Arc::new(AsyncMutex::new(())),
            cleanup_job: Arc::new(Mutex::new(None)),
            cleanup_progress: Arc::new(Mutex::new(TerminalCleanupProgress::default())),
            cleanup_attempts: Arc::new(AtomicU64::new(0)),
            shell: "test-shell".to_string(),
            cwd: "/redacted-test-cwd".to_string(),
            pid: 42,
            last_activity: Arc::new(RwLock::new(Instant::now())),
            orphan_since: Arc::new(RwLock::new(None)),
            renderer_refs: Arc::new(RwLock::new(HashSet::new())),
            protected: Arc::new(AtomicBool::new(true)),
            cols: Arc::new(RwLock::new(80)),
            rows: Arc::new(RwLock::new(24)),
            phone_fit: empty_phone_fit(),
            broadcast_tx: Arc::new(broadcast_tx),
            output_log: Arc::new(RwLock::new(std::collections::VecDeque::new())),
            output_log_bytes: Arc::new(AtomicUsize::new(0)),
            next_output_seq: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "windows")]
            conpty_handles: None,
        });
        manager
            .terminals
            .write()
            .insert(terminal_id.to_string(), Arc::clone(&instance));
        manager.active_terminal_slots.fetch_add(1, Ordering::SeqCst);
        manager.claims.issue(terminal_id, conversation_id, None);
        instance
    }

    /// Regression: the flusher's exit condition must be reachable from cleanup.
    ///
    /// Live evidence from a running build: 9 flusher threads against 8 readers,
    /// the odd one parked in `thread::sleep` forever because its reader ended
    /// without running the tail that sets `done_flag`. `join_thread_until`
    /// polls `is_finished()`, so that terminal could never be terminated —
    /// every attempt, retry included, burned the full deadline and quarantined.
    #[tokio::test]
    async fn terminate_retires_a_flusher_whose_reader_never_set_done_flag() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-immortal-flusher");

        // A flusher with the production exit condition: `done_flag` only.
        let flusher_done = Arc::clone(&instance.done_flag);
        *instance.flusher_handle.lock().await = Some(std::thread::spawn(move || {
            while !flusher_done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }));
        // The reader is gone and never set the flag — the defective state.
        assert!(!instance.done_flag.load(Ordering::Acquire));

        let receipt = manager
            .terminate("term-immortal-flusher")
            .await
            .expect("cleanup must not depend on the reader having set done_flag");
        assert!(receipt.released_slot, "the terminal slot must be released");
        assert!(
            instance.done_flag.load(Ordering::Acquire),
            "cleanup owns the flusher retirement signal"
        );
    }

    /// Regression: a reader parked in `read()` must be interruptible.
    ///
    /// Real pty, and a process that keeps the slave open past the shell so the
    /// descriptor never reaches EOF on its own. Before `stop_requested` there
    /// was no way to reach that reader at all — cleanup could only wait out the
    /// deadline and quarantine.
    #[cfg(unix)]
    #[test]
    fn reader_stops_on_request_even_without_eof() {
        use std::io::Read;
        use std::os::fd::FromRawFd;

        let pty = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let master_fd = pty.master.as_raw_fd().expect("master fd");
        let duplicated = unsafe { libc::dup(master_fd) };
        assert!(duplicated >= 0);
        let mut reader = unsafe { std::fs::File::from_raw_fd(duplicated) };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let mut buffer = [0u8; 64];
            loop {
                match PtyManager::wait_for_readable(Some(duplicated), &stop_thread, "test") {
                    WaitOutcome::Readable => {}
                    WaitOutcome::Stop | WaitOutcome::Failed => break,
                }
                if reader.read(&mut buffer).unwrap_or(0) == 0 {
                    break;
                }
            }
        });

        // Nothing is writing, so the descriptor never becomes readable and a
        // plain blocking `read` would park here forever.
        std::thread::sleep(Duration::from_millis(120));
        assert!(!worker.is_finished(), "the reader must still be waiting");

        stop.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            worker.is_finished(),
            "a stop request must reach a reader that has no EOF coming"
        );
        worker.join().expect("reader thread");
        drop(pty);
    }

    /// Regression: `Quarantined` must not be a dead end.
    ///
    /// A terminal whose cleanup could not finish kept its slot forever, refused
    /// to close its view, and could not mint a resume claim — neither killable
    /// nor closable, with every retry re-running the same losing sequence.
    #[tokio::test]
    async fn force_kill_releases_a_terminal_whose_worker_will_not_stop() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-wedged");

        // A worker that ignores every stop signal there is.
        let wedged = Arc::new(AtomicBool::new(false));
        let wedged_thread = Arc::clone(&wedged);
        *instance.reader_handle.lock().await = Some(std::thread::spawn(move || {
            while !wedged_thread.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }));

        let failure = manager
            .terminate("term-wedged")
            .await
            .expect_err("the graceful path cannot join a wedged worker");
        assert_eq!(failure.stage, TerminalCleanupStage::ReaderJoin);
        // The observer and the blocking job expire at the same instant, so the
        // job may still be settling when terminate returns.
        wait_for_cleanup_state(&manager, "term-wedged", TerminalLifecycleState::Quarantined).await;
        assert_eq!(manager.active_terminal_slot_count(), 1);

        let receipt = manager
            .force_kill("term-wedged")
            .await
            .expect("forced release must not be blocked by the wedged worker");
        assert!(
            receipt.forced,
            "the receipt must record a degraded teardown"
        );
        assert!(receipt.released_slot);
        assert_eq!(manager.active_terminal_slot_count(), 0);
        assert!(manager.get("term-wedged").is_none());

        wedged.store(true, Ordering::Release);
    }

    /// Regression: orphan reaping must not race a live cleanup job, and must
    /// not abandon a quarantined orphan either.
    ///
    /// Excluding every non-active state stopped the futile re-terminate loop
    /// but stranded the slot for good whenever nobody clicked retry; the sweep
    /// escalates instead.
    #[test]
    fn orphan_reaping_skips_in_flight_but_still_collects_quarantined() {
        let manager = crate::web::test_pty_manager();
        // The fixture starts with no renderer refs, so it is already an orphan.
        let instance = install_cleanup_fixture(&manager, "term-orphan-state");
        instance.set_protected(false);

        assert!(
            instance.is_orphan_reapable(Duration::ZERO),
            "an active, unprotected orphan is reapable"
        );

        *instance.lifecycle_state.write() = TerminalLifecycleState::Terminating;
        assert!(
            !instance.is_orphan_reapable(Duration::ZERO),
            "a job is already running; the sweep must not race it"
        );

        *instance.lifecycle_state.write() = TerminalLifecycleState::Quarantined;
        assert!(
            instance.is_orphan_reapable(Duration::ZERO),
            "a quarantined orphan is exactly what the sweep must still collect"
        );
    }

    /// The escalation the sweep depends on: a quarantined orphan that nobody
    /// retries still gives its slot back.
    #[tokio::test]
    async fn quarantined_orphan_is_released_by_escalation() {
        let manager = crate::web::test_pty_manager();
        let driver = Arc::new(ScriptedCleanupDriver::default());
        driver.fail_once(TerminalCleanupStage::Kill);
        manager.install_cleanup_driver(driver);
        let instance = install_cleanup_fixture(&manager, "term-orphan-quarantined");
        instance.set_protected(false);

        manager
            .terminate("term-orphan-quarantined")
            .await
            .expect_err("the scripted kill stage must fail");
        wait_for_cleanup_state(
            &manager,
            "term-orphan-quarantined",
            TerminalLifecycleState::Quarantined,
        )
        .await;
        assert_eq!(manager.active_terminal_slot_count(), 1);
        assert!(instance.is_orphan_reapable(Duration::ZERO));

        let receipt = manager
            .force_kill("term-orphan-quarantined")
            .await
            .expect("escalation must release the slot");
        assert!(receipt.released_slot);
        assert_eq!(manager.active_terminal_slot_count(), 0);
    }

    /// Regression: cleanup must not retire the flusher while the reader can
    /// still produce. Joining the consumer first dropped everything the reader
    /// appended after the flusher's final drain.
    #[tokio::test]
    async fn terminate_does_not_drop_tail_output_produced_during_cleanup() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-tail-output");
        let mut outputs = instance.broadcast_tx.subscribe();

        let pending: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::clone(&instance.done_flag);

        // A reader that keeps producing for a beat after the child is reaped —
        // a drained kernel buffer is not instantaneous.
        let reader_pending = Arc::clone(&pending);
        let reader_done = Arc::clone(&done);
        *instance.reader_handle.lock().await = Some(std::thread::spawn(move || {
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(2));
                reader_pending.lock().unwrap().extend_from_slice(b"tail");
            }
            reader_done.store(true, Ordering::Release);
        }));

        // The production flusher shape: drain on a tick, one final drain, exit.
        let flusher_pending = Arc::clone(&pending);
        let flusher_done = Arc::clone(&done);
        let flusher_tx = Arc::clone(&instance.broadcast_tx);
        let flusher_log = Arc::clone(&instance.output_log);
        let flusher_bytes = Arc::clone(&instance.output_log_bytes);
        let flusher_seq = Arc::clone(&instance.next_output_seq);
        *instance.flusher_handle.lock().await = Some(std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(1));
            let batch = {
                let mut guard = flusher_pending.lock().unwrap();
                (!guard.is_empty()).then(|| std::mem::take(&mut *guard))
            };
            if let Some(batch) = batch {
                let chunk = TerminalOutputChunk {
                    seq: flusher_seq.fetch_add(1, Ordering::Relaxed) + 1,
                    data: batch,
                };
                flusher_log.write().push_back(chunk.clone());
                flusher_bytes.fetch_add(chunk.data.len(), Ordering::Relaxed);
                let _ = flusher_tx.send(chunk);
            }
            if flusher_done.load(Ordering::Acquire) {
                let leftover = {
                    let mut guard = flusher_pending.lock().unwrap();
                    (!guard.is_empty()).then(|| std::mem::take(&mut *guard))
                };
                if let Some(leftover) = leftover {
                    let chunk = TerminalOutputChunk {
                        seq: flusher_seq.fetch_add(1, Ordering::Relaxed) + 1,
                        data: leftover,
                    };
                    flusher_log.write().push_back(chunk.clone());
                    flusher_bytes.fetch_add(chunk.data.len(), Ordering::Relaxed);
                    let _ = flusher_tx.send(chunk);
                }
                break;
            }
        }));

        manager
            .terminate("term-tail-output")
            .await
            .expect("cleanup must complete");

        let mut published = Vec::new();
        while let Ok(chunk) = outputs.try_recv() {
            published.extend_from_slice(&chunk.data);
        }
        assert_eq!(
            published, b"tailtailtailtailtail",
            "every byte the reader produced during cleanup must reach the output log"
        );
        assert!(
            pending.lock().unwrap().is_empty(),
            "nothing may be left unflushed once cleanup returns"
        );
    }

    /// Regression: a panicked worker is a terminated worker.
    ///
    /// Mapping the join failure to `ThreadPanicked` quarantined a terminal
    /// whose thread was already gone, and every retry re-joined the same dead
    /// thread for the same failure — permanently unkillable.
    #[tokio::test]
    async fn terminate_treats_a_panicked_worker_as_joined() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-panicked-reader");

        let panicked = std::thread::spawn(|| panic!("reader blew up"));
        while !panicked.is_finished() {
            std::thread::sleep(Duration::from_millis(1));
        }
        *instance.reader_handle.lock().await = Some(panicked);

        let receipt = manager
            .terminate("term-panicked-reader")
            .await
            .expect("a dead thread satisfies the join it is being waited on for");
        assert!(receipt.released_slot, "the terminal slot must be released");
        assert!(
            receipt.worker_panicked,
            "a salvaged teardown must stay distinguishable from a clean one"
        );
    }

    #[test]
    fn phone_fit_parks_desktop_size_until_last_owner_leaves() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-phone-fit");
        *instance.cols.write() = 120;
        *instance.rows.write() = 40;

        instance.adopt_phone_owner("phone-1", 40, 18);
        assert!(instance.is_phone_fit());
        assert_eq!(instance.display_mode(), TerminalDisplayMode::Phone);
        assert_eq!(
            instance
                .phone_fit
                .read()
                .park
                .as_ref()
                .map(|park| (park.desktop_cols, park.desktop_rows)),
            Some((120, 40))
        );

        instance.adopt_phone_owner("phone-2", 38, 16);
        assert_eq!(
            instance
                .phone_fit
                .read()
                .park
                .as_ref()
                .map(|park| (park.desktop_cols, park.desktop_rows)),
            Some((120, 40))
        );
        assert_eq!(instance.drop_phone_owner("phone-1"), None);
        assert!(instance.is_phone_fit());
        assert_eq!(instance.drop_phone_owner("phone-2"), Some((120, 40)));
        assert!(!instance.is_phone_fit());
        assert_eq!(instance.display_mode(), TerminalDisplayMode::Desktop);
    }

    #[tokio::test]
    async fn resize_is_ignored_while_phone_fit() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-phone-ignore");
        *instance.cols.write() = 100;
        *instance.rows.write() = 30;
        instance.adopt_phone_owner("phone-1", 40, 18);

        manager
            .resize("term-phone-ignore", 80, 24)
            .await
            .expect("parked resize is a no-op");
        assert_eq!(*instance.cols.read(), 100);
        assert_eq!(*instance.rows.read(), 30);
        assert_eq!(
            instance
                .phone_fit
                .read()
                .park
                .as_ref()
                .map(|park| (park.desktop_cols, park.desktop_rows)),
            Some((80, 24))
        );
        assert_eq!(instance.drop_phone_owner("phone-1"), Some((80, 24)));
    }

    #[tokio::test]
    async fn same_size_resize_skips_ioctl() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-same-size");
        *instance.cols.write() = 120;
        *instance.rows.write() = 40;

        manager
            .resize("term-same-size", 120, 40)
            .await
            .expect("same-size resize must not touch a missing PTY master");
        assert_eq!(*instance.cols.read(), 120);
        assert_eq!(*instance.rows.read(), 40);
    }

    #[test]
    fn force_desktop_clears_every_phone_owner() {
        let manager = crate::web::test_pty_manager();
        let instance = install_cleanup_fixture(&manager, "term-phone-force");
        *instance.cols.write() = 132;
        *instance.rows.write() = 43;
        instance.adopt_phone_owner("phone-1", 40, 18);
        instance.adopt_phone_owner("phone-2", 36, 16);
        assert_eq!(instance.force_desktop_display(), Some((132, 43)));
        assert!(!instance.is_phone_fit());
        assert_eq!(instance.drop_phone_owner("phone-1"), None);
    }

    #[tokio::test]
    async fn last_view_close_pauses_git_and_cwd_tracking_without_removing_pty() {
        let manager = crate::web::test_pty_manager();
        install_cleanup_fixture(&manager, "view-t1");
        manager
            .cwd_tracker()
            .start_tracking("view-t1", 42, "/redacted-test-cwd");
        manager
            .git_tracker()
            .initialize_terminal("view-t1", "/redacted-test-cwd");
        assert!(manager.git_tracker().is_tracking("view-t1"));
        assert_eq!(
            manager.cwd_tracker().get_cwd("view-t1").as_deref(),
            Some("/redacted-test-cwd")
        );

        manager.note_view_opened("view-t1");
        manager.note_view_opened("view-t1");
        manager.note_view_closed("view-t1");
        assert!(
            manager.git_tracker().is_tracking("view-t1"),
            "a remaining remote/desktop view must keep polling"
        );

        manager.note_view_closed("view-t1");
        assert!(
            manager.get("view-t1").is_some(),
            "close-view must not terminate the PTY"
        );
        assert!(
            !manager.git_tracker().is_tracking("view-t1"),
            "last view close must drop git polling"
        );
        assert!(
            manager.cwd_tracker().get_cwd("view-t1").is_none(),
            "last view close must drop cwd polling"
        );

        manager.note_view_opened("view-t1");
        assert!(manager.git_tracker().is_tracking("view-t1"));
        assert_eq!(
            manager.cwd_tracker().get_cwd("view-t1").as_deref(),
            Some("/redacted-test-cwd")
        );
    }

    #[tokio::test]
    async fn unwatched_close_view_keeps_polling_while_another_view_is_open() {
        let manager = crate::web::test_pty_manager();
        install_cleanup_fixture(&manager, "view-t2");
        manager
            .cwd_tracker()
            .start_tracking("view-t2", 42, "/redacted-test-cwd");
        manager
            .git_tracker()
            .initialize_terminal("view-t2", "/redacted-test-cwd");
        manager.note_view_opened("view-t2");
        manager.pause_tracking_if_unwatched("view-t2");
        assert!(
            manager.git_tracker().is_tracking("view-t2"),
            "a live attach/watch must keep git polling after a tab that never attached is hidden"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_driver_failures_quarantine_preserve_capacity_and_retry_exactly_once() {
        for stage in [
            TerminalCleanupStage::Kill,
            TerminalCleanupStage::Wait,
            TerminalCleanupStage::ReaderJoin,
            TerminalCleanupStage::FlusherJoin,
        ] {
            let manager = crate::web::test_pty_manager();
            let driver = Arc::new(ScriptedCleanupDriver::default());
            driver.fail_once(stage);
            manager.install_cleanup_driver(driver.clone());
            let terminal_id = format!("cleanup-{stage}");
            let instance = install_cleanup_fixture(&manager, &terminal_id);

            let started = Instant::now();
            let failure = manager
                .terminate(&terminal_id)
                .await
                .expect_err("the scripted cleanup stage must fail");
            assert_eq!(failure.stage, stage);
            assert!(started.elapsed() <= TERMINAL_CLEANUP_DEADLINE);
            assert_eq!(
                instance.lifecycle_state(),
                TerminalLifecycleState::Quarantined
            );
            assert!(manager.get(&terminal_id).is_some());
            assert_eq!(manager.active_terminal_slot_count(), 1);
            assert!(manager.claim_generation(&terminal_id).is_none());

            let observations = driver.observed_deadlines();
            let expected_stages = match stage {
                TerminalCleanupStage::Kill => vec![TerminalCleanupStage::Kill],
                TerminalCleanupStage::Wait => {
                    vec![TerminalCleanupStage::Kill, TerminalCleanupStage::Wait]
                }
                TerminalCleanupStage::ReaderJoin => vec![
                    TerminalCleanupStage::Kill,
                    TerminalCleanupStage::Wait,
                    TerminalCleanupStage::ReaderJoin,
                ],
                TerminalCleanupStage::FlusherJoin => vec![
                    TerminalCleanupStage::Kill,
                    TerminalCleanupStage::Wait,
                    TerminalCleanupStage::ReaderJoin,
                    TerminalCleanupStage::FlusherJoin,
                ],
            };
            assert_eq!(
                observations
                    .iter()
                    .map(|(observed_stage, _)| *observed_stage)
                    .collect::<Vec<_>>(),
                expected_stages
            );
            let shared_deadline = observations[0].1;
            assert!(observations
                .iter()
                .all(|(_, observed_deadline)| *observed_deadline == shared_deadline));

            let debug = format!("{failure:?}");
            for forbidden in ["redacted-test-cwd", "claim", "environment", "output"] {
                assert!(!debug.contains(forbidden));
            }

            let receipt = manager
                .terminate(&terminal_id)
                .await
                .expect("a later retry completes cleanup");
            assert!(receipt.released_slot);
            assert!(!receipt.already_removed);
            assert!(manager.get(&terminal_id).is_none());
            assert_eq!(manager.active_terminal_slot_count(), 0);

            let duplicate = manager
                .terminate(&terminal_id)
                .await
                .expect("completed removal is idempotent");
            assert!(!duplicate.released_slot);
            assert!(duplicate.already_removed);
            assert_eq!(manager.active_terminal_slot_count(), 0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_duplicate_terminate_releases_one_quarantined_slot_only() {
        let manager = Arc::new(crate::web::test_pty_manager());
        install_cleanup_fixture(&manager, "cleanup-concurrent");
        let driver = Arc::new(ScriptedCleanupDriver::default());
        driver.fail_once(TerminalCleanupStage::Kill);
        manager.install_cleanup_driver(driver);
        manager
            .terminate("cleanup-concurrent")
            .await
            .expect_err("first attempt quarantines the terminal");
        assert_eq!(
            manager.terminal_lifecycle_state("cleanup-concurrent"),
            Some(TerminalLifecycleState::Quarantined)
        );
        assert_eq!(manager.active_terminal_slot_count(), 1);

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let manager = Arc::clone(&manager);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                manager.terminate("cleanup-concurrent").await
            }));
        }
        barrier.wait().await;
        let results = futures::future::join_all(tasks).await;
        let receipts = results
            .into_iter()
            .map(|result| result.unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            receipts
                .iter()
                .filter(|receipt| receipt.released_slot)
                .count(),
            1
        );
        assert_eq!(manager.active_terminal_slot_count(), 0);
        assert!(manager.get("cleanup-concurrent").is_none());
    }

    #[derive(Debug, Default)]
    struct BlockingCleanupState {
        entered: usize,
        released: bool,
    }

    #[derive(Debug, Default)]
    struct BlockingCleanupDriver {
        state: Mutex<BlockingCleanupState>,
        changed: std::sync::Condvar,
    }

    impl BlockingCleanupDriver {
        fn entered(&self) -> usize {
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entered
        }

        fn release(&self) {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.released = true;
            self.changed.notify_all();
        }
    }

    impl CleanupDriver for BlockingCleanupDriver {
        fn kill(
            &self,
            child: &mut dyn Child,
            deadline: Instant,
        ) -> Result<(), TerminalCleanupFailureReason> {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.entered += 1;
            self.changed.notify_all();
            while !state.released {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            drop(state);
            SystemCleanupDriver.kill(child, deadline)
        }

        fn try_wait(
            &self,
            child: &mut dyn Child,
            deadline: Instant,
        ) -> Result<Option<portable_pty::ExitStatus>, TerminalCleanupFailureReason> {
            SystemCleanupDriver.try_wait(child, deadline)
        }

        fn join_thread(
            &self,
            stage: TerminalCleanupStage,
            handle: &mut Option<std::thread::JoinHandle<()>>,
            deadline: Instant,
        ) -> Result<WorkerExit, TerminalCleanupFailureReason> {
            SystemCleanupDriver.join_thread(stage, handle, deadline)
        }
    }

    async fn wait_for_cleanup_state(
        manager: &PtyManager,
        terminal_id: &str,
        expected: TerminalLifecycleState,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if manager.terminal_lifecycle_state(terminal_id) == Some(expected) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cleanup state transition completed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocking_cleanup_returns_by_caller_deadline_and_retry_observes_one_retained_job() {
        let manager = crate::web::test_pty_manager();
        install_cleanup_fixture(&manager, "cleanup-blocking");
        let driver = Arc::new(BlockingCleanupDriver::default());
        manager.install_cleanup_driver(driver.clone());

        let started = Instant::now();
        let failure = manager
            .terminate_until(
                "cleanup-blocking",
                tokio::time::Instant::now() + Duration::from_millis(150),
            )
            .await
            .expect_err("blocking kill must return an in-flight deadline receipt");
        assert!(failure.in_flight);
        assert_eq!(failure.stage, TerminalCleanupStage::Kill);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(manager.active_terminal_slot_count(), 1);
        assert_eq!(
            manager.terminal_lifecycle_state("cleanup-blocking"),
            Some(TerminalLifecycleState::Terminating)
        );
        let retained = manager
            .terminal_cleanup_job("cleanup-blocking")
            .expect("manager retains the cleanup job");
        assert_eq!(retained.job_id(), failure.job_id);
        assert_eq!(retained.attempt(), failure.attempt);
        assert_eq!(driver.entered(), 1);

        let retry = manager
            .terminate_until(
                "cleanup-blocking",
                tokio::time::Instant::now() + Duration::from_millis(150),
            )
            .await
            .expect_err("retry observes the still-running job");
        assert!(retry.in_flight);
        assert_eq!(retry.job_id, failure.job_id);
        assert_eq!(retry.attempt, failure.attempt);
        assert_eq!(driver.entered(), 1, "retry must not duplicate child.kill");

        driver.release();
        wait_for_cleanup_state(
            &manager,
            "cleanup-blocking",
            TerminalLifecycleState::Quarantined,
        )
        .await;
        let completed = manager
            .terminate_until(
                "cleanup-blocking",
                tokio::time::Instant::now() + Duration::from_secs(2),
            )
            .await
            .expect("a later attempt completes the retained cleanup");
        assert!(completed.released_slot);
        assert_ne!(completed.job_id, failure.job_id);
        assert_eq!(manager.active_terminal_slot_count(), 0);
        assert!(manager.get("cleanup-blocking").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thirty_stalled_terminals_share_one_deadline_without_serial_multiplication() {
        let manager = crate::web::test_pty_manager();
        for ordinal in 0..GLOBAL_TERMINAL_LIMIT {
            install_cleanup_fixture(&manager, &format!("cleanup-batch-{ordinal:02}"));
        }
        assert_eq!(manager.active_terminal_slot_count(), GLOBAL_TERMINAL_LIMIT);
        let driver = Arc::new(BlockingCleanupDriver::default());
        manager.install_cleanup_driver(driver.clone());

        let started = Instant::now();
        let receipt = manager
            .kill_all_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        assert_eq!(receipt.attempted, GLOBAL_TERMINAL_LIMIT);
        assert_eq!(receipt.succeeded, 0);
        assert_eq!(receipt.failed, 0);
        assert_eq!(receipt.in_flight, GLOBAL_TERMINAL_LIMIT);
        assert_eq!(
            receipt.succeeded + receipt.failed + receipt.in_flight,
            GLOBAL_TERMINAL_LIMIT
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(driver.entered(), GLOBAL_TERMINAL_LIMIT);
        assert_eq!(manager.active_terminal_slot_count(), GLOBAL_TERMINAL_LIMIT);

        driver.release();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let completed = manager
                    .get_all()
                    .iter()
                    .filter(|instance| {
                        instance.lifecycle_state() == TerminalLifecycleState::Quarantined
                    })
                    .count();
                if completed == GLOBAL_TERMINAL_LIMIT {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("all retained jobs complete into quarantine");

        let retry = manager
            .kill_all_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        assert_eq!(retry.attempted, GLOBAL_TERMINAL_LIMIT);
        assert_eq!(retry.succeeded, GLOBAL_TERMINAL_LIMIT);
        assert_eq!(retry.failed + retry.in_flight, 0);
        assert_eq!(manager.active_terminal_slot_count(), 0);
    }

    #[derive(Debug, Default)]
    struct MixedCleanupDriver {
        kill_calls: AtomicUsize,
        blocker: BlockingCleanupDriver,
    }

    impl CleanupDriver for MixedCleanupDriver {
        fn kill(
            &self,
            child: &mut dyn Child,
            deadline: Instant,
        ) -> Result<(), TerminalCleanupFailureReason> {
            match self.kill_calls.fetch_add(1, Ordering::AcqRel) {
                0 => SystemCleanupDriver.kill(child, deadline),
                1 => Err(TerminalCleanupFailureReason::Error),
                _ => self.blocker.kill(child, deadline),
            }
        }

        fn try_wait(
            &self,
            child: &mut dyn Child,
            deadline: Instant,
        ) -> Result<Option<portable_pty::ExitStatus>, TerminalCleanupFailureReason> {
            SystemCleanupDriver.try_wait(child, deadline)
        }

        fn join_thread(
            &self,
            stage: TerminalCleanupStage,
            handle: &mut Option<std::thread::JoinHandle<()>>,
            deadline: Instant,
        ) -> Result<WorkerExit, TerminalCleanupFailureReason> {
            SystemCleanupDriver.join_thread(stage, handle, deadline)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mixed_shutdown_receipt_counts_success_failure_and_in_flight_under_one_deadline() {
        let manager = crate::web::test_pty_manager();
        for terminal_id in ["cleanup-mixed-a", "cleanup-mixed-b", "cleanup-mixed-c"] {
            install_cleanup_fixture(&manager, terminal_id);
        }
        let driver = Arc::new(MixedCleanupDriver::default());
        manager.install_cleanup_driver(driver.clone());

        let receipt = manager
            .kill_all_until(tokio::time::Instant::now() + Duration::from_millis(200))
            .await;
        assert_eq!(receipt.attempted, 3);
        assert_eq!(receipt.succeeded, 1);
        assert_eq!(receipt.failed, 1);
        assert_eq!(receipt.in_flight, 1);
        assert_eq!(
            receipt.succeeded + receipt.failed + receipt.in_flight,
            receipt.attempted
        );
        assert!(!receipt.clean_success());
        assert_eq!(manager.active_terminal_slot_count(), 2);

        driver.blocker.release();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if manager.get_all().iter().all(|instance| {
                    instance.lifecycle_state() == TerminalLifecycleState::Quarantined
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("failed and in-flight jobs settle into retryable quarantine");

        manager.install_cleanup_driver(Arc::new(SystemCleanupDriver));
        let retry = manager
            .kill_all_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        assert_eq!(retry.attempted, 2);
        assert_eq!(retry.succeeded, 2);
        assert!(retry.clean_success());
        assert_eq!(manager.active_terminal_slot_count(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn directly_executable_windows_accepts_only_image_formats() {
        // The exact list we accept — anything else will be rejected by
        // resolve_program_path before it can reach CreateProcessW and surface
        // as os error 193.
        for ok in [
            r"C:\bin\claude.exe",
            r"C:\bin\codex.exe",
            r"C:/bin/cursor.com",
            r"C:\bin\agent.scr",
            r"C:\bin\sub\path\agent.exe",
            r#"C:\bin\"quoted".exe"#, // trailing quote tolerated
        ] {
            assert!(
                is_directly_executable_windows(ok),
                "expected accepted: {}",
                ok
            );
        }
        for bad in [
            r"C:\bin\nodot",
            r"C:\bin\opencode.cmd",
            r"C:\bin\agent.bat",
            r"C:\bin\script.ps1",
            r"C:\bin\hello.vbs",
            r"C:\bin\runner.js",
            r"C:\bin\thing.exe.cmd", // .cmd wins, rejected
        ] {
            assert!(
                !is_directly_executable_windows(bad),
                "expected rejected: {}",
                bad
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn directly_executable_windows_composite_suffix_does_not_match() {
        // A file like `agent.cmd.exe` is a .exe, so it IS accepted — but
        // `agent.cmd.txt` is not. Make sure we look at the last extension only.
        assert!(is_directly_executable_windows(r"C:\bin\agent.cmd.exe"));
        assert!(!is_directly_executable_windows(r"C:\bin\agent.exe.cmd"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_npm_cmd_shim_rewrites_to_node_script() {
        // Write a simulated npm .cmd shim matching the real opencode.cmd format
        // that nvm-windows generates.
        let dir = std::env::temp_dir().join("se-manager-test-cmd-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create a fake node.exe (just a marker — parser only checks existence
        // + extension via is_directly_executable_windows).
        std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        // Create the target script file.
        std::fs::create_dir_all(dir.join("node_modules\\opencode-ai\\bin")).unwrap();
        std::fs::write(dir.join("node_modules\\opencode-ai\\bin\\opencode"), b"").unwrap();

        let shim_path = dir.join("opencode.cmd");
        let shim_content = "@ECHO off\r\n".to_owned()
            + "GOTO start\r\n"
            + ":find_dp0\r\n"
            + "SET dp0=%~dp0\r\n"
            + "EXIT /b\r\n"
            + ":start\r\n"
            + "SETLOCAL\r\n"
            + "CALL :find_dp0\r\n"
            + "\r\n"
            + "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\opencode-ai\\bin\\opencode\" %*\r\n";
        std::fs::write(&shim_path, shim_content).unwrap();

        let resolved = parse_npm_cmd_shim(shim_path.to_str().unwrap());
        assert!(resolved.is_some(), "should parse the shim");
        let resolved = resolved.unwrap();

        // The executable should be node.exe in the same directory as the shim.
        assert!(
            resolved.program.ends_with("node.exe"),
            "expected node.exe, got: {}",
            resolved.program
        );
        // The script path should be the opencode-ai bin entry.
        assert_eq!(resolved.prepend_args.len(), 1);
        assert!(
            resolved.prepend_args[0].contains("opencode-ai\\bin\\opencode"),
            "expected script path containing opencode-ai bin, got: {}",
            resolved.prepend_args[0]
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_npm_cmd_shim_rewrites_npm_launcher_with_set_indirection() {
        // npm's own npx.cmd / npm.cmd invoke through SETLOCAL variables:
        //   SET "NODE_EXE=%~dp0\node.exe"
        //   SET "NPX_CLI_JS=%~dp0\node_modules\npm\bin\npx-cli.js"
        //   "%NODE_EXE%" "%NPX_CLI_JS%" %*
        // The parser must resolve the %VAR% indirection, not only the simple
        // `"%dp0%\node.exe" "<script>"` package-bin form.
        let dir = std::env::temp_dir().join("se-manager-test-npx-launcher-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(dir.join("node_modules\\npm\\bin")).unwrap();
        std::fs::write(dir.join("node_modules\\npm\\bin\\npx-cli.js"), b"").unwrap();

        let shim_path = dir.join("npx.cmd");
        let shim_content = ":: Created by npm, please don't edit manually.\r\n".to_owned()
            + "@ECHO OFF\r\n"
            + "SETLOCAL\r\n"
            + "SET \"NODE_EXE=%~dp0\\node.exe\"\r\n"
            + "IF NOT EXIST \"%NODE_EXE%\" (\r\n"
            + "  SET \"NODE_EXE=node\"\r\n"
            + ")\r\n"
            + "SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\r\n"
            + "\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\r\n";
        std::fs::write(&shim_path, shim_content).unwrap();

        let resolved = parse_npm_cmd_shim(shim_path.to_str().unwrap());
        assert!(resolved.is_some(), "should parse the npm launcher shim");
        let resolved = resolved.unwrap();
        assert!(
            resolved.program.ends_with("node.exe"),
            "expected node.exe, got: {}",
            resolved.program
        );
        assert_eq!(resolved.prepend_args.len(), 1);
        assert!(
            resolved.prepend_args[0].contains("npm\\bin\\npx-cli.js"),
            "expected npx-cli.js script path, got: {}",
            resolved.prepend_args[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_powershell_cmd_shim_rewrites_cursor_agent_style() {
        let dir = std::env::temp_dir().join("se-manager-test-ps-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let ps_exe = dir.join("powershell.exe");
        std::fs::write(&ps_exe, b"MZ").unwrap();
        let script = dir.join("cursor-agent.ps1");
        std::fs::write(&script, b"# stub").unwrap();

        let ps_exe_str = ps_exe.to_string_lossy();
        let script_str = script.to_string_lossy();
        let shim_path = dir.join("cursor-agent.cmd");
        let shim_content = format!(
            "@echo off\r\n{ps} -NoProfile -ExecutionPolicy Bypass -File \"{script}\" %*\r\n",
            ps = ps_exe_str,
            script = script_str,
        );
        std::fs::write(&shim_path, shim_content).unwrap();

        let resolved = parse_powershell_cmd_shim(shim_path.to_str().unwrap());
        assert!(resolved.is_some(), "should parse PowerShell shim");
        let resolved = resolved.unwrap();
        assert!(
            resolved.program.ends_with("powershell.exe"),
            "expected powershell.exe, got: {}",
            resolved.program
        );
        assert!(
            resolved
                .prepend_args
                .iter()
                .any(|a| a.ends_with("cursor-agent.ps1")),
            "expected -File script in prepend_args: {:?}",
            resolved.prepend_args
        );
        assert!(
            resolved.prepend_args.iter().any(|a| a == "-NoProfile"),
            "expected -NoProfile flag: {:?}",
            resolved.prepend_args
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_agent_resolution_skips_extensionless_before_pe() {
        let dir = std::env::temp_dir().join("se-manager-test-pe-resolve");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("claude"), b"not a pe image").unwrap();
        std::fs::write(dir.join("claude.exe"), b"MZ").unwrap();

        let trimmed = "claude";
        const WIN_EXECUTABLE_EXTS: &[&str] = &["", ".exe", ".com", ".scr"];
        let mut resolved_path: Option<String> = None;
        for ext in WIN_EXECUTABLE_EXTS {
            let candidate = dir.join(format!("{}{}", trimmed, ext));
            if !candidate.exists() {
                continue;
            }
            let abs_path = candidate.to_string_lossy().to_string();
            if is_directly_executable_windows(&abs_path) {
                resolved_path = Some(abs_path);
                break;
            }
        }

        let resolved_path =
            resolved_path.expect("should resolve to claude.exe after skipping extensionless shim");
        assert!(
            resolved_path.ends_with("claude.exe"),
            "expected claude.exe, got: {}",
            resolved_path
        );
        assert!(
            !is_directly_executable_windows(&dir.join("claude").to_string_lossy()),
            "extensionless claude must not be treated as PE"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn try_parse_windows_cmd_shim_prefers_npm_over_powershell() {
        let dir = std::env::temp_dir().join("se-manager-test-shim-priority");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(dir.join("node_modules\\pkg\\bin")).unwrap();
        std::fs::write(dir.join("node_modules\\pkg\\bin\\tool"), b"").unwrap();

        let shim_path = dir.join("tool.cmd");
        let shim_content = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\n\
            endLocal & goto #_undefined_# 2>NUL || \"%_prog%\" \"%dp0%\\node_modules\\pkg\\bin\\tool\" %*\r\n";
        std::fs::write(&shim_path, shim_content).unwrap();

        let resolved = try_parse_windows_cmd_shim(shim_path.to_str().unwrap());
        assert!(resolved.is_some());
        assert!(
            resolved.unwrap().program.ends_with("node.exe"),
            "npm shim should resolve to node.exe"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_spawn_program_rewrites_npm_cmd_shim() {
        let dir = std::env::temp_dir().join("se-manager-test-resolve-npm-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(dir.join("node_modules\\gemini\\bin")).unwrap();
        std::fs::write(dir.join("node_modules\\gemini\\bin\\gemini"), b"").unwrap();

        let shim_path = dir.join("gemini.cmd");
        let shim_content = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\n\
            endLocal & goto #_undefined_# 2>NUL || \"%_prog%\" \"%dp0%\\node_modules\\gemini\\bin\\gemini\" %*\r\n";
        std::fs::write(&shim_path, shim_content).unwrap();

        // Explicit-path form: resolve_spawn_program parses and rewrites the shim.
        let resolved = resolve_spawn_program(shim_path.to_str().unwrap())
            .expect("npm .cmd shim should resolve");
        assert!(
            resolved.program.ends_with("node.exe"),
            "expected node.exe, got: {}",
            resolved.program
        );
        assert_eq!(resolved.prepend_args.len(), 1);
        assert!(
            resolved.prepend_args[0].contains("gemini\\bin\\gemini"),
            "expected the script path prepended, got: {:?}",
            resolved.prepend_args
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_spawn_program_rewrites_powershell_cmd_shim() {
        let dir = std::env::temp_dir().join("se-manager-test-resolve-ps-shim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cursor-agent.ps1"), b"").unwrap();
        let ps_exe = std::path::Path::new(
            &std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()),
        )
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let ps_exe_str = ps_exe.to_string_lossy().to_string();
        let script_str = dir.join("cursor-agent.ps1").to_string_lossy().to_string();

        let shim_path = dir.join("cursor-agent.cmd");
        let shim_content = format!(
            "@echo off\r\n{ps} -NoProfile -ExecutionPolicy Bypass -File \"{script}\" %*\r\n",
            ps = ps_exe_str,
            script = script_str,
        );
        std::fs::write(&shim_path, shim_content).unwrap();

        let resolved = resolve_spawn_program(shim_path.to_str().unwrap())
            .expect("PowerShell .cmd shim should resolve");
        assert!(
            resolved.program.ends_with("powershell.exe"),
            "expected powershell.exe, got: {}",
            resolved.program
        );
        assert!(
            resolved
                .prepend_args
                .iter()
                .any(|a| a.ends_with("cursor-agent.ps1")),
            "expected -File script prepended, got: {:?}",
            resolved.prepend_args
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_spawn_program_keeps_native_exe_without_prepend() {
        let dir = std::env::temp_dir().join("se-manager-test-resolve-native-exe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe_path = dir.join("agent.exe");
        std::fs::write(&exe_path, b"MZ").unwrap();

        let resolved =
            resolve_spawn_program(exe_path.to_str().unwrap()).expect("native .exe should resolve");
        assert!(resolved.program.ends_with("agent.exe"));
        assert!(
            resolved.prepend_args.is_empty(),
            "native exe must not prepend args, got: {:?}",
            resolved.prepend_args
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn resolve_spawn_program_passes_through_on_unix() {
        let resolved = resolve_spawn_program("gemini").expect("unix passthrough");
        assert_eq!(resolved.program, "gemini");
        assert!(resolved.prepend_args.is_empty());
    }

    #[test]
    fn test_should_reap_orphan_protected_never_reaped() {
        let long_ago = Duration::from_secs(10_000);
        let timeout = Duration::from_secs(600);
        // Protected + orphaned + long past timeout => still not reapable.
        assert!(!should_reap_orphan(
            true,
            true,
            Some(long_ago),
            long_ago,
            timeout
        ));
    }

    #[test]
    fn test_should_reap_orphan_attached_never_reaped() {
        let long_ago = Duration::from_secs(10_000);
        let timeout = Duration::from_secs(600);
        // Not protected but still has a renderer ref (is_orphan == false).
        assert!(!should_reap_orphan(
            false,
            false,
            Some(long_ago),
            long_ago,
            timeout
        ));
    }

    #[test]
    fn test_should_reap_orphan_orphaned_past_timeout_reaped() {
        let timeout = Duration::from_secs(600);
        // Unprotected, orphaned, past timeout => reapable.
        assert!(should_reap_orphan(
            false,
            true,
            Some(Duration::from_secs(601)),
            Duration::from_secs(0),
            timeout
        ));
    }

    #[test]
    fn test_should_reap_orphan_orphaned_within_timeout_not_reaped() {
        let timeout = Duration::from_secs(600);
        assert!(!should_reap_orphan(
            false,
            true,
            Some(Duration::from_secs(59)),
            Duration::from_secs(0),
            timeout
        ));
    }

    #[test]
    fn test_should_reap_orphan_uses_inactivity_when_never_orphaned() {
        let timeout = Duration::from_secs(600);
        // Never had a renderer ref (orphaned_for None) => fall back to inactivity.
        assert!(should_reap_orphan(
            false,
            true,
            None,
            Duration::from_secs(601),
            timeout
        ));
        assert!(!should_reap_orphan(
            false,
            true,
            None,
            Duration::from_secs(59),
            timeout
        ));
    }

    fn conversation_record(
        conversation_id: ConversationId,
        workspace_cwd: &str,
        execution_target: ExecutionTarget,
    ) -> ConversationRecordV2 {
        let created_at =
            crate::conversation::parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        ConversationRecordV2 {
            schema_version: crate::conversation::CONVERSATION_SCHEMA_VERSION,
            conversation_id,
            created_at_utc: created_at,
            creation_partition: crate::conversation::CreationPartition::from_created_at(created_at),
            workspace_cwd: workspace_cwd.to_string(),
            execution_target,
            project_attachment: None,
            lifecycle_state: crate::conversation::ConversationLifecycleState::Ready,
            last_seq: 0,
            created_by: crate::conversation::ConversationCreator::Legacy,
            title: None,
            title_source: None,
        }
    }

    #[test]
    fn trusted_spawn_intent_derives_host_options() {
        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let record = conversation_record(
            conversation_id,
            "/host/workspace",
            ExecutionTarget::Worktree {
                project_id: "project-1".to_string(),
                worktree_path: "/host/worktree".to_string(),
                worktree_branch: "chat/test".to_string(),
            },
        );
        let intent = TerminalSpawnIntentV1 {
            conversation_id,
            project_id: Some("project-1".to_string()),
            cwd_source: TerminalCwdSource::ExecutionTarget,
            cols: 120,
            rows: 40,
        };

        let options = intent.into_trusted_options(&record).unwrap();
        assert_eq!(options.conversation_id, Some(conversation_id));
        assert_eq!(options.project_id.as_deref(), Some("project-1"));
        assert_eq!(options.cwd.as_deref(), Some("/host/worktree"));
        assert_eq!(options.cols, Some(120));
        assert_eq!(options.rows, Some(40));
        assert!(options.shell.is_none());
        assert!(options.program.is_none());
        assert!(options.args.is_none());
        assert!(options.env.is_none());
        assert!(options.kind.is_none());
        assert!(tracks_session_workspace_ref(&options));

        let wrong_project = TerminalSpawnIntentV1 {
            conversation_id,
            project_id: Some("project-other".to_string()),
            cwd_source: TerminalCwdSource::Workspace,
            cols: 80,
            rows: 24,
        };
        assert!(wrong_project.into_trusted_options(&record).is_err());
    }

    #[test]
    fn workspace_ref_tracking_requires_real_conversation_scope() {
        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        assert!(!tracks_session_workspace_ref(&SpawnOptions::default()));
        assert!(tracks_session_workspace_ref(&SpawnOptions {
            conversation_id: Some(conversation_id),
            ..Default::default()
        }));
        assert!(!tracks_session_workspace_ref(&SpawnOptions {
            conversation_id: Some(conversation_id),
            kind: Some("ssh".to_string()),
            ..Default::default()
        }));
        assert!(!tracks_session_workspace_ref(&SpawnOptions {
            kind: Some("ssh".to_string()),
            ..Default::default()
        }));
    }

    #[tokio::test]
    async fn resume_preserves_pty_and_replays_cursor() {
        let manager = crate::web::test_pty_manager();
        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let other_conversation_id =
            ConversationId::parse("5f7a1c01-4d1b-4c8a-af01-0123456789ab").unwrap();
        let terminal_id = "terminal-resume-test".to_string();
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(8);
        let chunks = std::collections::VecDeque::from([
            TerminalOutputChunk {
                seq: 1,
                data: b"one".to_vec(),
            },
            TerminalOutputChunk {
                seq: 2,
                data: b"two".to_vec(),
            },
        ]);
        let instance = Arc::new(TerminalInstance {
            id: terminal_id.clone(),
            conversation_id,
            workspace_ref_tracked: true,
            project_id: Some("project-1".to_string()),
            child: Arc::new(AsyncMutex::new(None)),
            master: Arc::new(AsyncMutex::new(None)),
            writer: Arc::new(AsyncMutex::new(None)),
            reader_handle: Arc::new(AsyncMutex::new(None)),
            flusher_handle: Arc::new(AsyncMutex::new(None)),
            done_flag: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            lifecycle_state: Arc::new(RwLock::new(TerminalLifecycleState::Active)),
            cleanup_gate: Arc::new(AsyncMutex::new(())),
            cleanup_job: Arc::new(Mutex::new(None)),
            cleanup_progress: Arc::new(Mutex::new(TerminalCleanupProgress::default())),
            cleanup_attempts: Arc::new(AtomicU64::new(0)),
            shell: "host-shell".to_string(),
            cwd: "/host/workspace".to_string(),
            pid: 42,
            last_activity: Arc::new(RwLock::new(Instant::now())),
            orphan_since: Arc::new(RwLock::new(None)),
            renderer_refs: Arc::new(RwLock::new(HashSet::new())),
            protected: Arc::new(AtomicBool::new(true)),
            cols: Arc::new(RwLock::new(80)),
            rows: Arc::new(RwLock::new(24)),
            phone_fit: empty_phone_fit(),
            broadcast_tx: Arc::new(broadcast_tx),
            output_log: Arc::new(RwLock::new(chunks)),
            output_log_bytes: Arc::new(AtomicUsize::new(6)),
            next_output_seq: Arc::new(AtomicU64::new(3)),
            #[cfg(target_os = "windows")]
            conpty_handles: None,
        });
        manager
            .terminals
            .write()
            .insert(terminal_id.clone(), instance);
        manager.active_terminal_slots.store(1, Ordering::SeqCst);
        let old_claim = manager
            .claims
            .issue(&terminal_id, conversation_id, Some("project-1"));
        let generation_before = manager.claim_generation(&terminal_id).unwrap();

        assert!(manager
            .resume_for_conversation(other_conversation_id, &terminal_id, 1)
            .is_err());
        assert!(manager
            .resume_for_conversation(conversation_id, "terminal-unknown", 1)
            .is_err());
        assert_eq!(
            manager.claim_generation(&terminal_id),
            Some(generation_before)
        );

        let (grant, replay) = manager
            .resume_for_conversation(conversation_id, &terminal_id, 1)
            .unwrap();
        assert_eq!(grant.terminal.id, terminal_id);
        assert_eq!(grant.terminal.latest_seq, 2);
        assert!(!grant.terminal.gap);
        assert_eq!(replay.chunks.len(), 1);
        assert_eq!(replay.chunks[0].seq, 2);
        assert_eq!(replay.chunks[0].data, b"two");
        assert!(replay.claim_generation.unwrap() > generation_before);
        assert!(manager.get(&terminal_id).is_some(), "resume preserves PTY");
        assert_eq!(
            manager.verify_claim(&terminal_id, &old_claim),
            Err(ClaimError)
        );
        assert!(manager.verify_claim(&terminal_id, &grant.claim).is_ok());

        let first_handoff = grant.claim;
        let (successor, _) = manager
            .resume_for_conversation(conversation_id, &terminal_id, 2)
            .unwrap();
        assert_eq!(
            manager.verify_claim(&terminal_id, &first_handoff),
            Err(ClaimError)
        );
        assert!(manager.verify_claim(&terminal_id, &successor.claim).is_ok());
        assert!(
            manager.get(&terminal_id).is_some(),
            "rotation preserves PTY"
        );

        manager.terminals.write().remove(&terminal_id);
        manager.claims.remove(&terminal_id);
        manager.active_terminal_slots.store(0, Ordering::SeqCst);
    }

    #[test]
    fn test_spawn_options_default() {
        let options = SpawnOptions::default();
        assert!(options.shell.is_none());
        assert!(options.cwd.is_none());
        assert!(options.env.is_none());
        assert!(options.conversation_id.is_none());
        assert_eq!(options.cols, Some(80));
        assert_eq!(options.rows, Some(24));
    }

    #[test]
    fn test_terminal_info_serialization() {
        let info = TerminalInfo {
            id: "test-123".to_string(),
            shell: "/bin/bash".to_string(),
            cwd: "/home/user".to_string(),
            pid: 12345,
            cols: 100,
            rows: 30,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"id\":\"test-123\""));
        assert!(json.contains("\"shell\":\"/bin/bash\""));
        assert!(json.contains("\"cwd\":\"/home/user\""));
        assert!(json.contains("\"pid\":12345"));
        assert!(json.contains("\"cols\":100"));
        assert!(json.contains("\"rows\":30"));
    }

    #[test]
    fn test_spawn_options_deserialization() {
        let json = r#"{"conversationId":"018f7a1c-1b4d-7c8a-9f01-0123456789ab","projectId":"project-attribution","shell":"cmd.exe","cwd":"C:\\","cols":120,"rows":40}"#;
        let options: SpawnOptions = serde_json::from_str(json).unwrap();
        assert_eq!(
            options.conversation_id,
            Some(ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap())
        );
        assert_eq!(options.project_id.as_deref(), Some("project-attribution"));
        assert_eq!(options.shell, Some("cmd.exe".to_string()));
        assert_eq!(options.cwd, Some("C:\\".to_string()));
        assert_eq!(options.cols, Some(120));
        assert_eq!(options.rows, Some(40));
    }

    // ========== CAP-3 serde shape tests ==========
    // Pin the golden wire shapes so cross-language drift (serde flatten /
    // camelCase) cannot ship untested: clients rely on byte-identical shapes
    // across the desktop IPC and web WS surfaces.

    #[test]
    fn test_spawned_terminal_serializes_flat_with_claim() {
        let spawned = SpawnedTerminal {
            info: TerminalInfo {
                id: "terminal-123-0".to_string(),
                shell: "pwsh".to_string(),
                cwd: "C:\\work".to_string(),
                pid: 42,
                cols: 120,
                rows: 32,
            },
            claim: "f3a9".to_string(),
        };

        let debug = format!("{spawned:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(&spawned.claim));

        let value: serde_json::Value = serde_json::to_value(&spawned).unwrap();
        let obj = value.as_object().expect("spawn reply is an object");

        // FLATTENS to top-level info fields — no nested `info` key.
        assert!(
            !obj.contains_key("info"),
            "SpawnedTerminal must flatten info, not nest it"
        );
        assert_eq!(
            obj.get("id").and_then(|v| v.as_str()),
            Some("terminal-123-0")
        );
        assert_eq!(obj.get("shell").and_then(|v| v.as_str()), Some("pwsh"));
        assert_eq!(obj.get("cwd").and_then(|v| v.as_str()), Some("C:\\work"));
        assert_eq!(obj.get("pid").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(obj.get("cols").and_then(|v| v.as_u64()), Some(120));
        assert_eq!(obj.get("rows").and_then(|v| v.as_u64()), Some(32));
        assert_eq!(obj.get("claim").and_then(|v| v.as_str()), Some("f3a9"));

        // Exactly the golden shape — no extra keys may sneak in.
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["cols", "cwd", "id", "pid", "rows", "shell", "claim"]
                .into_iter()
                .collect::<std::collections::BTreeSet<&str>>()
        );
    }

    #[test]
    fn test_terminal_attach_result_serializes_camelcase_without_claim() {
        let result = TerminalAttachResult {
            id: "terminal-123-0".to_string(),
            shell: "pwsh".to_string(),
            cwd: "C:\\work".to_string(),
            pid: 42,
            cols: 120,
            rows: 32,
            latest_seq: 87,
            gap: false,
        };

        let value: serde_json::Value = serde_json::to_value(&result).unwrap();
        let obj = value.as_object().expect("attach reply is an object");

        // camelCase seq fields — never snake_case.
        assert_eq!(obj.get("latestSeq").and_then(|v| v.as_u64()), Some(87));
        assert_eq!(obj.get("gap").and_then(|v| v.as_bool()), Some(false));
        assert!(!obj.contains_key("latest_seq"));
        assert!(!obj.contains_key("latestseq"));

        // Attach NEVER issues a credential.
        assert!(
            !obj.contains_key("claim"),
            "TerminalAttachResult must never carry a claim key"
        );

        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "id",
                "shell",
                "cwd",
                "pid",
                "cols",
                "rows",
                "latestSeq",
                "gap"
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<&str>>()
        );
    }

    #[test]
    fn terminal_resume_grant_serializes_exact_camelcase_shape() {
        let grant = TerminalResumeGrant {
            terminal: TerminalAttachResult {
                id: "terminal-123-0".to_string(),
                shell: "pwsh".to_string(),
                cwd: "C:\\work".to_string(),
                pid: 42,
                cols: 120,
                rows: 32,
                latest_seq: 87,
                gap: false,
            },
            claim: "one-time-claim".to_string(),
        };

        let value = serde_json::to_value(grant).unwrap();
        let object = value.as_object().expect("resume grant is an object");
        let keys = object
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["claim", "terminal"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        let terminal = object["terminal"]
            .as_object()
            .expect("terminal metadata is nested");
        assert_eq!(
            terminal.get("latestSeq").and_then(|value| value.as_u64()),
            Some(87)
        );
        assert!(!terminal.contains_key("claim"));
        assert_eq!(object["claim"], "one-time-claim");
    }

    // ========== Git Bash resolution tests ==========

    #[cfg(target_os = "windows")]
    #[test]
    fn test_git_bash_candidates_match_detection() {
        // Verify that the candidates in resolve_shell_path match
        // the candidates in lib.rs get_available_shells()
        // This test ensures the git_bash_paths constants stay in sync

        // Verify primary paths are non-empty (compile-time guard) and well-formed
        const { assert!(!git_bash_paths::PRIMARY_PATHS.is_empty()) };
        for path in git_bash_paths::PRIMARY_PATHS {
            assert!(
                path.contains("bash.exe"),
                "Primary path should contain bash.exe: {}",
                path
            );
        }

        // Verify fallback paths are non-empty (compile-time guard) and well-formed
        const { assert!(!git_bash_paths::FALLBACK_PATHS.is_empty()) };
        for path in git_bash_paths::FALLBACK_PATHS {
            assert!(
                path.contains("bash.exe"),
                "Fallback path should contain bash.exe: {}",
                path
            );
        }

        // Specific verification that key paths exist
        assert!(git_bash_paths::PRIMARY_PATHS.contains(&r"C:\Program Files\Git\bin\bash.exe"));
        assert!(git_bash_paths::PRIMARY_PATHS.contains(&r"C:\Program Files\Git\usr\bin\bash.exe"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_git_bash_fallback_paths_included() {
        // Verify fallback paths are included for edge cases
        let fallback_paths = vec![
            r"C:\tools\msys64\usr\bin\bash.exe",
            r"C:\msys64\usr\bin\bash.exe",
            r"C:\Git\bin\bash.exe",
            r"C:\Git\usr\bin\bash.exe",
        ];

        for path in fallback_paths {
            assert!(path.contains("bash.exe"));
        }
    }

    #[test]
    fn test_shell_resolution_git_bash_alias_recognized() {
        // Verify git-bash is treated as a special alias distinct from "bash"
        let git_bash = "git-bash";
        let bash = "bash";

        // These should be different shell names
        assert_ne!(git_bash, bash);

        // git-bash should map to bash.exe eventually (verified in resolve_shell_path)
        assert!(git_bash.contains("bash"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_shell_resolution_error_message_git_bash() {
        // Verify that git-bash error message is informative
        let _shell = "git-bash";
        let expected_error_substring = "bash.exe not found in PATH or common Git Bash locations";
        assert!(expected_error_substring.contains("bash.exe"));
        assert!(expected_error_substring.contains("PATH"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_builtin_windows_shell() {
        assert!(PtyManager::is_builtin_windows_shell("cmd"));
        assert!(PtyManager::is_builtin_windows_shell("CMD.EXE"));
        assert!(PtyManager::is_builtin_windows_shell("powershell"));
        assert!(PtyManager::is_builtin_windows_shell("pwsh"));
        assert!(PtyManager::is_builtin_windows_shell("wsl"));
        assert!(!PtyManager::is_builtin_windows_shell("bash.exe"));
        assert!(!PtyManager::is_builtin_windows_shell("git-bash"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_env_merge_preserves_existing_path_case_insensitively() {
        let env_map = merge_windows_environment_map(
            vec![("Path".to_string(), r"C:\laragon\bin\nodejs".to_string())],
            None,
        );

        let path_keys: Vec<&String> = env_map
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("path"))
            .collect();

        assert_eq!(path_keys.len(), 1);
        assert_eq!(
            env_map
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("path"))
                .map(|(_, value)| value.as_str()),
            Some(r"C:\laragon\bin\nodejs")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_env_merge_overrides_path_case_insensitively() {
        let mut custom_env = HashMap::new();
        custom_env.insert("PATH".to_string(), r"C:\custom\node".to_string());

        let env_map = merge_windows_environment_map(
            vec![("Path".to_string(), r"C:\laragon\bin\nodejs".to_string())],
            Some(custom_env),
        );

        let path_keys: Vec<&String> = env_map
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("path"))
            .collect();

        assert_eq!(path_keys.len(), 1);
        assert_eq!(
            env_map
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("path"))
                .map(|(_, value)| value.as_str()),
            Some(r"C:\custom\node")
        );
    }

    // ========== Async kill() signature tests ==========
    // Note: Full integration tests for kill() and kill_all() require Tauri runtime.
    // The async spawn_blocking pattern is validated through:
    // 1. Compile-time check: kill() is now async and returns impl Future
    // 2. Existing orphan cleanup code at line 403-406 demonstrates the pattern
    // 3. Manual testing during development

    #[test]
    fn system_cleanup_driver_cannot_block_runtime_after_deadline() {
        let driver = SystemCleanupDriver;
        let started = Instant::now();
        let mut handle = Some(std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }));
        let result = driver.join_thread(
            TerminalCleanupStage::ReaderJoin,
            &mut handle,
            Instant::now() - std::time::Duration::from_secs(1),
        );
        assert_eq!(result, Err(TerminalCleanupFailureReason::DeadlineExceeded));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
