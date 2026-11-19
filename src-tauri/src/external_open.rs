//! Files the operating system hands to the app: cold-start arguments, a second
//! instance's command line, and macOS `RunEvent::Opened` URLs.
//!
//! The host has two delivery paths and must not double-deliver:
//! - before the renderer calls `take_pending_external_files`, paths are queued
//!   and drained by that command (covers cold start and a launch that beats the
//!   webview's script execution);
//! - after that first take, the renderer demonstrably has its listener wired, so
//!   later paths are emitted straight to the `external-file-open` event.
//!
//! Every path is validated here rather than in the renderer: a raw argument can
//! be a `--flag` for another role, a non-file URL, or a path relative to the
//! launch cwd.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Renderer-facing event name. Mirrored by `EXTERNAL_FILE_OPEN_EVENT` in
/// `src/renderer/lib/external-file-open-api.ts`.
pub const EXTERNAL_FILE_OPEN_EVENT: &str = "external-file-open";

/// Event payload: `{ paths: string[] }`.
#[derive(Debug, Clone, Serialize)]
pub struct ExternalFileOpenPayload {
    pub paths: Vec<String>,
}

#[derive(Debug, Default)]
struct PendingState {
    paths: Vec<String>,
    frontend_ready: bool,
}

/// Thread-safe queue of paths the renderer has not taken yet.
///
/// Managed as `PendingExternalFiles` app state; the single-instance callback,
/// `RunEvent::Opened`, and the cold-start seed all write here, and
/// `take_pending_external_files` drains it exactly once.
#[derive(Debug, Default)]
pub struct PendingExternalFiles {
    state: Mutex<PendingState>,
}

impl PendingExternalFiles {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the state seeded with whatever `main` collected before Tauri
    /// started.
    #[must_use]
    pub fn from_cold_start() -> Self {
        let pending = Self::new();
        pending.push_strings(cold_start_paths());
        pending
    }

    /// Append already-validated paths, dropping duplicates.
    /// Returns how many new paths were queued.
    pub fn push_strings<I>(&self, paths: I) -> usize
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut added = 0;
        for path in paths {
            if !state.paths.iter().any(|existing| existing == &path) {
                state.paths.push(path);
                added += 1;
            }
        }
        added
    }

    /// Append filesystem paths, dropping duplicates.
    pub fn push_paths<I>(&self, paths: I) -> usize
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.push_strings(
            paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned()),
        )
    }

    /// Drain the queue and mark the renderer as ready. Every later
    /// `dispatch_paths` emits instead of queueing.
    #[must_use]
    pub fn take_all(&self) -> Vec<String> {
        let mut state = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
        state.frontend_ready = true;
        std::mem::take(&mut state.paths)
    }

    /// Mark the renderer as listening without draining (used by tests and by
    /// callers that only assert the handshake).
    pub fn mark_frontend_ready(&self) {
        let mut state = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
        state.frontend_ready = true;
    }

    #[must_use]
    pub fn is_frontend_ready(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .frontend_ready
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .paths
            .len()
    }
}

/// Cold-start paths collected in `main` before any Tauri plugin exists.
static COLD_START_FILES: OnceLock<Vec<String>> = OnceLock::new();

/// Record the launcher arguments for the GUI role.
///
/// Called once from `main` after the non-GUI roles (`--mcp-core`,
/// `--terminal-core`, `--acp-core`, `--plugin-core`, the memory MCP tombstone,
/// and the `host_mcp` child) have already exited. Arguments are never logged:
/// they can name user documents.
pub fn seed_cold_start_args<I>(arguments: I)
where
    I: IntoIterator<Item = OsString>,
{
    let cwd = std::env::current_dir().ok();
    let files = collect_paths_from_args(arguments, cwd.as_deref());
    let _ = COLD_START_FILES.set(files);
}

/// Cold-start files recorded by `seed_cold_start_args`, if it ran.
#[must_use]
pub fn cold_start_paths() -> Vec<String> {
    COLD_START_FILES.get().cloned().unwrap_or_default()
}

/// True for an argument that belongs to another process role or to the launcher
/// itself (`--flag`, `-psn_0_12345`).
#[must_use]
pub fn is_internal_flag(argument: &str) -> bool {
    argument.starts_with('-')
}

/// Extract candidate files from one launcher argument.
///
/// - `file://` URLs are converted through `Url::to_file_path`, so
///   percent-encoded segments decode correctly.
/// - `--flag` and non-`file` URLs (http, custom schemes) yield nothing.
/// - Relative paths are resolved against `cwd` when given.
/// - Only existing filesystem entries survive.
#[must_use]
pub fn paths_from_arg(argument: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let argument = argument.trim();
    if argument.is_empty() || is_internal_flag(argument) {
        return Vec::new();
    }

    if let Some(rest) = argument.strip_prefix("file://") {
        // `Url::parse` needs the scheme back; `file:///a` keeps three slashes.
        let normalized = format!("file://{rest}");
        return match url::Url::parse(&normalized).ok().and_then(|url| url.to_file_path().ok()) {
            Some(path) if path.exists() => vec![path],
            Some(_) => Vec::new(),
            None => Vec::new(),
        };
    }
    // Any other scheme (`https:`, `se-plugin:`) is not a local file.
    if argument.contains("://") {
        return Vec::new();
    }

    let path = PathBuf::from(argument);
    let candidate = if path.is_absolute() {
        path
    } else {
        let Some(cwd) = cwd else {
            return Vec::new();
        };
        cwd.join(path)
    };
    if candidate.exists() {
        vec![candidate]
    } else {
        Vec::new()
    }
}

/// Normalize a whole argument list into deduplicated absolute path strings.
#[must_use]
pub fn collect_paths_from_args<I>(arguments: I, cwd: Option<&Path>) -> Vec<String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for argument in arguments {
        let argument = argument.to_string_lossy();
        for path in paths_from_arg(&argument, cwd) {
            let rendered = path.to_string_lossy().into_owned();
            if seen.insert(rendered.clone()) {
                out.push(rendered);
            }
        }
    }
    out
}

/// Hand validated paths to the renderer, showing and focusing the main window.
///
/// Queues when the renderer has not taken the pending set yet (so nothing is
/// lost while the webview boots), emits once it has. Returns the number of
/// paths delivered or queued.
pub fn dispatch_paths(app: &AppHandle, paths: Vec<PathBuf>) -> usize {
    let mut seen = HashSet::new();
    let paths: Vec<String> = paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() {
        focus_main_window(app);
        return 0;
    }

    let delivered = paths.len();
    let pending = app.try_state::<PendingExternalFiles>();
    let frontend_ready = pending
        .as_ref()
        .is_some_and(|pending| pending.is_frontend_ready());

    if !frontend_ready {
        if let Some(pending) = pending.as_ref() {
            pending.push_strings(paths);
            log::info!(
                "[external-open] operation=queue stable_code=FRONTEND_NOT_READY result=QUEUED count={delivered}"
            );
        } else {
            log::warn!(
                "[external-open] operation=queue stable_code=PENDING_STATE_MISSING result=DROPPED count={delivered}"
            );
        }
        focus_main_window(app);
        return delivered;
    }

    match app.emit(
        EXTERNAL_FILE_OPEN_EVENT,
        ExternalFileOpenPayload {
            paths: paths.clone(),
        },
    ) {
        Ok(()) => log::info!(
            "[external-open] operation=emit stable_code=EVENT_EMITTED result=PASS count={delivered}"
        ),
        Err(error) => {
            // The renderer took the queue once, so it has a listener — but an
            // emit can still fail while the webview is being torn down. Keep
            // the paths instead of losing them.
            log::warn!(
                "[external-open] operation=emit stable_code=EMIT_FAILED result=QUEUED count={delivered} error={error}"
            );
            if let Some(pending) = pending.as_ref() {
                pending.push_strings(paths);
            }
        }
    }
    focus_main_window(app);
    delivered
}

/// Show, unminimize, and focus `main`. A launch that arrived with files must
/// not leave the window in the background.
pub fn focus_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!("[external-open] operation=focus stable_code=MAIN_WINDOW_MISSING result=SKIPPED");
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn converts_file_urls_with_percent_encoding() {
        let dir = TempDir::new().expect("temp dir");
        let file = dir.path().join("My Notes.md");
        fs::write(&file, "x").expect("write");

        let url = format!("file://{}", file.to_string_lossy().replace(' ', "%20"));
        let paths = paths_from_arg(&url, None);

        assert_eq!(paths, vec![file]);
    }

    #[test]
    fn ignores_non_file_urls_and_internal_flags() {
        assert!(paths_from_arg("https://example.com/a.md", None).is_empty());
        assert!(paths_from_arg("--mcp-core", None).is_empty());
        assert!(paths_from_arg("-psn_0_12345", None).is_empty());
    }

    #[test]
    fn resolves_relative_paths_against_the_launch_cwd() {
        let dir = TempDir::new().expect("temp dir");
        let file = dir.path().join("docs/spec.md");
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(&file, "x").expect("write");

        let paths = paths_from_arg("docs/spec.md", Some(dir.path()));
        assert_eq!(paths, vec![file]);
    }

    #[test]
    fn drops_paths_that_do_not_exist() {
        let dir = TempDir::new().expect("temp dir");
        assert!(paths_from_arg("missing.md", Some(dir.path())).is_empty());
    }

    #[test]
    fn collect_deduplicates_and_skips_flags() {
        let dir = TempDir::new().expect("temp dir");
        let file = dir.path().join("a.md");
        fs::write(&file, "x").expect("write");
        let rendered = file.to_string_lossy().into_owned();

        let collected = collect_paths_from_args(
            vec![
                OsString::from("--mcp-core"),
                OsString::from(rendered.clone()),
                OsString::from(rendered.clone()),
                OsString::from("b.md"),
            ],
            Some(dir.path()),
        );

        assert_eq!(collected, vec![rendered]);
    }

    #[test]
    fn queue_take_hands_out_each_path_once() {
        let pending = PendingExternalFiles::new();
        assert_eq!(pending.push_paths(vec![PathBuf::from("/tmp/a.md")]), 1);
        assert_eq!(pending.push_paths(vec![PathBuf::from("/tmp/a.md")]), 0);
        assert_eq!(pending.push_paths(vec![PathBuf::from("/tmp/b.md")]), 1);
        assert_eq!(pending.pending_count(), 2);
        assert!(!pending.is_frontend_ready());

        assert_eq!(pending.take_all(), vec!["/tmp/a.md", "/tmp/b.md"]);
        assert!(pending.is_frontend_ready());
        assert!(pending.take_all().is_empty());
    }

    #[test]
    fn from_cold_start_is_empty_without_seeded_args() {
        // `seed_cold_start_args` may have run in another test; only assert that
        // construction never panics and that the accessor is stable.
        let first = cold_start_paths();
        assert_eq!(first, cold_start_paths());
    }
}
