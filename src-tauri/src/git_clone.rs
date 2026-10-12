//! `git clone` for the new-project dialog.
//!
//! A clone can take minutes, so it reports progress while it runs, can be
//! cancelled, and gives up on its own once git stops making progress. Before
//! this the dialog showed a spinner for up to half an hour with no way to tell
//! a slow transfer from a stuck one.

use crate::trackers::git_tracker::resolve_git_binary;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

/// A clone that has printed nothing for this long is treated as stuck. git
/// redraws its progress line whenever a byte arrives, so even a very slow
/// transfer stays well inside it.
const GIT_CLONE_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Progress lines arrive many times a second; forward at most this often per
/// phase. A phase change, a non-progress line, or a finished phase always goes.
const PROGRESS_FORWARD_INTERVAL: Duration = Duration::from_millis(200);

/// How long git gets to clean up after SIGTERM before it is killed.
const TERMINATE_GRACE: Duration = Duration::from_secs(5);

/// The error a cancelled clone rejects with, so the dialog can tell a
/// cancellation from a failure.
pub const GIT_CLONE_CANCELLED: &str = "GIT_CLONE_CANCELLED";

/// The last lines kept for an error message.
const ERROR_TAIL_LINES: usize = 8;

/// Remote forms a project can be cloned from. Anything else — `file://`, a
/// local path, a `transport::` helper — is refused rather than handed to git.
fn is_allowed_clone_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_whitespace) {
        return false;
    }
    ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
        || (url.starts_with("git@") && url.contains(':'))
}

/// A single plain directory name inside `parent_dir`.
fn is_safe_clone_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// Where `git clone` would put the repository, or why it cannot. An existing
/// empty directory is fine (git accepts it); anything with content is not
/// overwritten.
fn clone_target(parent_dir: &str, dir_name: &str) -> Result<std::path::PathBuf, String> {
    if !is_safe_clone_dir_name(dir_name) {
        return Err(format!("Invalid folder name: {dir_name}"));
    }
    let parent = std::path::Path::new(parent_dir);
    if !parent.is_dir() {
        return Err(format!("Folder does not exist: {parent_dir}"));
    }
    let target = parent.join(dir_name);
    if target.exists() {
        let empty = std::fs::read_dir(&target)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(format!(
                "{} already exists and is not empty",
                target.display()
            ));
        }
    }
    Ok(target)
}

/// One line of `git clone --progress` output, with the progress fields parsed
/// out when the line has them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCloneProgress {
    pub line: String,
    /// e.g. `Receiving objects`; `remote: ` is stripped.
    pub phase: Option<String>,
    pub percent: Option<u8>,
    /// e.g. `22.25 MiB`.
    pub transferred: Option<String>,
    /// e.g. `1.28 MiB/s`.
    pub speed: Option<String>,
}

/// Parse `Receiving objects:  24% (1719/6979), 22.25 MiB | 1.28 MiB/s`.
/// Lines without a percentage (`Cloning into ...`, errors) carry only `line`.
pub fn parse_progress_line(line: &str) -> GitCloneProgress {
    let line = line.trim_end();
    let mut progress = GitCloneProgress {
        line: line.to_string(),
        ..GitCloneProgress::default()
    };
    let text = line.strip_prefix("remote: ").unwrap_or(line).trim();
    let Some((phase, rest)) = text.split_once(':') else {
        return progress;
    };
    let Some((percent, rest)) = rest.split_once('%') else {
        return progress;
    };
    let Ok(percent) = percent.trim().parse::<u8>() else {
        return progress;
    };
    progress.phase = Some(phase.trim().to_string());
    progress.percent = Some(percent.min(100));
    if let Some((_, transfer)) = rest.split_once("),") {
        let transfer = transfer.trim().trim_end_matches(", done.").trim();
        if let Some((transferred, speed)) = transfer.split_once('|') {
            progress.transferred = Some(transferred.trim().to_string());
            progress.speed = Some(speed.trim().to_string());
        }
    }
    progress
}

/// Clones in flight, so a cancel request can reach the right one.
#[derive(Default)]
pub struct GitCloneRegistry {
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl GitCloneRegistry {
    fn register(&self, clone_id: &str) -> Result<Arc<AtomicBool>, String> {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if running.contains_key(clone_id) {
            return Err(format!("Clone {clone_id} is already running"));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        running.insert(clone_id.to_string(), Arc::clone(&cancel));
        Ok(cancel)
    }

    fn unregister(&self, clone_id: &str) {
        self.running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(clone_id);
    }

    /// Ask a running clone to stop. False when no such clone is running.
    pub fn cancel(&self, clone_id: &str) -> bool {
        match self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(clone_id)
        {
            Some(cancel) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

/// Strip `user:token@` from a URL before it is logged.
fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://***@{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

fn spawn_clone(parent_dir: &str, url: &str, dir_name: &str) -> std::io::Result<Child> {
    let mut command = Command::new(resolve_git_binary());
    command
        .args(["clone", "--progress", "--", url, dir_name])
        .current_dir(parent_dir)
        // No terminal to prompt on; fail instead of waiting for input.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Progress lines are parsed; keep them in git's untranslated form.
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Own process group, so a stop reaches git-remote-https and index-pack
    // too, not only the `git clone` parent.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
}

/// Stop the clone. On Unix git gets SIGTERM first: it then removes the
/// half-written repository itself, which is safer than deleting a folder here
/// that something else may have created in the meantime.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + TERMINATE_GRACE;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

enum CloneEnd {
    Finished(std::process::ExitStatus),
    Cancelled,
    Stalled,
}

/// Forward progress, watch for cancel and stalls, and wait for git to exit.
fn run_clone(
    mut child: Child,
    cancel: &AtomicBool,
    on_progress: &Channel<GitCloneProgress>,
    stall_timeout: Duration,
) -> (CloneEnd, Vec<String>) {
    let last_output = Arc::new(Mutex::new(Instant::now()));
    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let reader = child.stderr.take().map(|stderr| {
        let last_output = Arc::clone(&last_output);
        let tail = Arc::clone(&tail);
        let on_progress = on_progress.clone();
        std::thread::spawn(move || forward_output(stderr, &last_output, &tail, &on_progress))
    });

    let end = loop {
        match child.try_wait() {
            Ok(Some(status)) => break CloneEnd::Finished(status),
            Ok(None) => {}
            Err(_) => {
                terminate(&mut child);
                break CloneEnd::Stalled;
            }
        }
        if cancel.load(Ordering::SeqCst) {
            terminate(&mut child);
            break CloneEnd::Cancelled;
        }
        let silent_for = last_output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed();
        if silent_for >= stall_timeout {
            terminate(&mut child);
            break CloneEnd::Stalled;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // The pipe closes once git and its helpers are gone, ending the reader.
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    let tail = std::mem::take(&mut *tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    (end, tail)
}

/// Read git's stderr as it arrives. It must be drained while git runs: a full
/// pipe would block git, which is how the old wait-then-read version could
/// hang until its timeout. Progress redraws end in `\r`, other lines in `\n`.
fn forward_output(
    mut stderr: impl Read,
    last_output: &Mutex<Instant>,
    tail: &Mutex<Vec<String>>,
    on_progress: &Channel<GitCloneProgress>,
) {
    let mut pending = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut last_sent: Option<(Option<String>, Instant)> = None;
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        *last_output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
        for &byte in &buffer[..read] {
            if byte != b'\r' && byte != b'\n' {
                pending.push(byte);
                continue;
            }
            if pending.is_empty() {
                continue;
            }
            let line = String::from_utf8_lossy(&pending).into_owned();
            pending.clear();
            forward_line(&line, tail, on_progress, &mut last_sent);
        }
    }
    if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending).into_owned();
        forward_line(&line, tail, on_progress, &mut last_sent);
    }
}

fn forward_line(
    line: &str,
    tail: &Mutex<Vec<String>>,
    on_progress: &Channel<GitCloneProgress>,
    last_sent: &mut Option<(Option<String>, Instant)>,
) {
    let progress = parse_progress_line(line);
    if progress.line.trim().is_empty() {
        return;
    }
    let phase_done = progress.line.ends_with("done.");
    let throttled = progress.percent.is_some()
        && !phase_done
        && last_sent.as_ref().is_some_and(|(phase, sent_at)| {
            *phase == progress.phase && sent_at.elapsed() < PROGRESS_FORWARD_INTERVAL
        });
    {
        let mut tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A redrawn progress line replaces its previous state in the tail.
        if progress.phase.is_some()
            && tail
                .last()
                .is_some_and(|last| parse_progress_line(last).phase == progress.phase)
        {
            tail.pop();
        }
        tail.push(progress.line.clone());
        let excess = tail.len().saturating_sub(ERROR_TAIL_LINES);
        tail.drain(..excess);
    }
    if throttled {
        return;
    }
    *last_sent = Some((progress.phase.clone(), Instant::now()));
    let _ = on_progress.send(progress);
}

/// Clone `url` into `<parent_dir>/<dir_name>` and return that path. Progress
/// goes to `on_progress`; `git_clone_cancel` with the same `clone_id` stops it.
#[tauri::command]
pub async fn git_clone(
    registry: tauri::State<'_, Arc<GitCloneRegistry>>,
    clone_id: String,
    url: String,
    parent_dir: String,
    dir_name: String,
    on_progress: Channel<GitCloneProgress>,
) -> Result<String, String> {
    if !is_allowed_clone_url(&url) {
        return Err(format!("Unsupported repository address: {url}"));
    }
    let target = clone_target(&parent_dir, &dir_name)?;
    let cancel = registry.register(&clone_id)?;
    let registry = Arc::clone(&registry);
    let started = Instant::now();
    log::info!(
        "[git-clone] operation=start clone_id={clone_id} url={} target={}",
        redact_url(url.trim()),
        target.display()
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        let child = spawn_clone(&parent_dir, url.trim(), &dir_name)
            .map_err(|error| format!("Failed to run git clone: {error}"))?;
        let (end, tail) = run_clone(child, &cancel, &on_progress, GIT_CLONE_STALL_TIMEOUT);
        match end {
            CloneEnd::Finished(status) if status.success() => {
                Ok(target.to_string_lossy().into_owned())
            }
            CloneEnd::Finished(_) => Err(failure_message(&tail)),
            CloneEnd::Cancelled => Err(GIT_CLONE_CANCELLED.to_string()),
            CloneEnd::Stalled => Err(format!(
                "git made no progress for {} seconds and was stopped. {}",
                GIT_CLONE_STALL_TIMEOUT.as_secs(),
                failure_message(&tail)
            )),
        }
    })
    .await
    .map_err(|error| format!("git clone task failed: {error}"))
    .and_then(|result| result);
    registry.unregister(&clone_id);
    let outcome = match &result {
        Ok(_) => "OK",
        Err(error) if error == GIT_CLONE_CANCELLED => "CANCELLED",
        Err(_) => "FAILED",
    };
    log::info!(
        "[git-clone] operation=finish clone_id={clone_id} result={outcome} elapsed_ms={}{}",
        started.elapsed().as_millis(),
        match &result {
            Err(error) if outcome == "FAILED" => format!(" error={error:?}"),
            _ => String::new(),
        }
    );
    result
}

/// Prefer git's own `fatal:` / `error:` lines; otherwise the last output.
fn failure_message(tail: &[String]) -> String {
    let errors: Vec<&str> = tail
        .iter()
        .map(String::as_str)
        .filter(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .collect();
    if errors.is_empty() {
        tail.last().cloned().unwrap_or_default()
    } else {
        errors.join("\n")
    }
}

/// Stop the clone started with `clone_id`. False when it is not running.
#[tauri::command]
pub fn git_clone_cancel(
    registry: tauri::State<'_, Arc<GitCloneRegistry>>,
    clone_id: String,
) -> bool {
    let cancelled = registry.cancel(&clone_id);
    log::info!("[git-clone] operation=cancel clone_id={clone_id} running={cancelled}");
    cancelled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_remote_urls_only() {
        for ok in [
            "https://github.com/qinsehm1128/prefect_test.git",
            "http://example.com/a/b",
            "ssh://git@github.com/a/b.git",
            "git@github.com:a/b.git",
            "git://example.com/a.git",
        ] {
            assert!(is_allowed_clone_url(ok), "{ok}");
        }
        for bad in [
            "",
            "file:///etc",
            "/Users/me/repo",
            "ext::sh -c touch% /tmp/pwned",
            "--upload-pack=touch /tmp/x",
            "https://github.com/a b",
            "git@github.com",
        ] {
            assert!(!is_allowed_clone_url(bad), "{bad}");
        }
    }

    #[test]
    fn folder_name_is_one_plain_segment() {
        assert!(is_safe_clone_dir_name("prefect_test"));
        assert!(is_safe_clone_dir_name("my.repo-2"));
        for bad in ["", ".", "..", "a/b", "../x", "-x", "a b"] {
            assert!(!is_safe_clone_dir_name(bad), "{bad}");
        }
    }

    #[test]
    fn target_refuses_a_non_empty_folder_and_accepts_an_empty_one() {
        let parent = tempfile::tempdir().unwrap();
        let parent_str = parent.path().to_string_lossy().into_owned();

        assert_eq!(
            clone_target(&parent_str, "fresh").unwrap(),
            parent.path().join("fresh")
        );

        std::fs::create_dir(parent.path().join("empty")).unwrap();
        assert!(clone_target(&parent_str, "empty").is_ok());

        std::fs::create_dir(parent.path().join("used")).unwrap();
        std::fs::write(parent.path().join("used").join("x"), "x").unwrap();
        assert!(clone_target(&parent_str, "used").is_err());

        assert!(clone_target("/definitely/not/here", "x").is_err());
    }

    #[test]
    fn parses_a_transfer_progress_line() {
        let progress =
            parse_progress_line("Receiving objects:  24% (1719/6979), 22.25 MiB | 1.28 MiB/s");
        assert_eq!(progress.phase.as_deref(), Some("Receiving objects"));
        assert_eq!(progress.percent, Some(24));
        assert_eq!(progress.transferred.as_deref(), Some("22.25 MiB"));
        assert_eq!(progress.speed.as_deref(), Some("1.28 MiB/s"));
    }

    #[test]
    fn parses_a_finished_transfer_and_a_remote_phase() {
        let done = parse_progress_line(
            "Receiving objects: 100% (6979/6979), 74.80 MiB | 1.20 MiB/s, done.",
        );
        assert_eq!(done.percent, Some(100));
        assert_eq!(done.speed.as_deref(), Some("1.20 MiB/s"));

        let remote = parse_progress_line("remote: Compressing objects:  50% (5/10)");
        assert_eq!(remote.phase.as_deref(), Some("Compressing objects"));
        assert_eq!(remote.percent, Some(50));
        assert_eq!(remote.transferred, None);
    }

    #[test]
    fn leaves_plain_lines_without_progress_fields() {
        for line in [
            "Cloning into 'TraceMemo'...",
            "remote: Enumerating objects: 6979, done.        ",
            "fatal: repository 'https://github.com/a/b.git/' not found",
        ] {
            let progress = parse_progress_line(line);
            assert_eq!(progress.line, line.trim_end());
            assert_eq!(progress.phase, None, "{line}");
            assert_eq!(progress.percent, None, "{line}");
        }
    }

    #[test]
    fn redacts_credentials_from_logged_urls() {
        assert_eq!(
            redact_url("https://user:token@github.com/a/b.git"),
            "https://***@github.com/a/b.git"
        );
        assert_eq!(
            redact_url("https://github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(
            redact_url("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
    }

    #[test]
    fn failure_message_prefers_git_errors() {
        let tail = vec![
            "Cloning into 'x'...".to_string(),
            "fatal: Authentication failed for 'https://github.com/a/b.git/'".to_string(),
        ];
        assert_eq!(
            failure_message(&tail),
            "fatal: Authentication failed for 'https://github.com/a/b.git/'"
        );
        assert_eq!(
            failure_message(&["Receiving objects:  3% (1/30)".to_string()]),
            "Receiving objects:  3% (1/30)"
        );
    }

    #[test]
    fn cancel_reaches_only_a_running_clone() {
        let registry = GitCloneRegistry::default();
        assert!(!registry.cancel("a"));
        let flag = registry.register("a").unwrap();
        assert!(registry.register("a").is_err());
        assert!(registry.cancel("a"));
        assert!(flag.load(Ordering::SeqCst));
        registry.unregister("a");
        assert!(!registry.cancel("a"));
    }

    #[cfg(unix)]
    fn local_origin(dir: &std::path::Path) -> String {
        let origin = dir.join("origin");
        let git = |args: &[&str], cwd: &std::path::Path| {
            let status = Command::new(resolve_git_binary())
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(status.status.success(), "git {args:?}: {status:?}");
        };
        std::fs::create_dir(&origin).unwrap();
        git(&["init", "-q"], &origin);
        std::fs::write(origin.join("a.txt"), "a").unwrap();
        git(&["add", "."], &origin);
        git(&["commit", "-qm", "init"], &origin);
        format!("file://{}", origin.display())
    }

    /// A channel that records what it was sent.
    fn recording_channel() -> (Channel<GitCloneProgress>, Arc<Mutex<Vec<String>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&sent);
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                sink.lock().unwrap().push(json);
            }
            Ok(())
        });
        (channel, sent)
    }

    #[cfg(unix)]
    fn shell(script: &str) -> Child {
        use std::os::unix::process::CommandExt;
        Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap()
    }

    // A real clone from a local origin: progress reaches the channel and the
    // checkout lands where it should.
    #[cfg(unix)]
    #[test]
    fn a_clone_reports_progress_and_completes() {
        let dir = tempfile::tempdir().unwrap();
        let origin = local_origin(dir.path());
        let parent = dir.path().to_string_lossy().into_owned();
        let (channel, sent) = recording_channel();

        let child = spawn_clone(&parent, &origin, "copy").unwrap();
        let (end, _) = run_clone(
            child,
            &AtomicBool::new(false),
            &channel,
            GIT_CLONE_STALL_TIMEOUT,
        );

        assert!(matches!(end, CloneEnd::Finished(status) if status.success()));
        assert!(dir.path().join("copy").join("a.txt").is_file());
        let sent = sent.lock().unwrap();
        assert!(
            sent.iter().any(|json| json.contains("Cloning into")),
            "progress was not forwarded: {sent:?}"
        );
    }

    // The old version read output only after exit; past the pipe buffer git
    // blocked on its write and the clone hung until the timeout.
    #[cfg(unix)]
    #[test]
    fn output_beyond_the_pipe_buffer_does_not_block_the_process() {
        let child = shell("i=0; while [ $i -lt 3000 ]; do printf 'Receiving objects:  1%% (1/9)\\r' >&2; i=$((i+1)); done; printf 'x%.0s' $(seq 1 70000) >&2; exit 0");
        let (channel, sent) = recording_channel();
        let (end, _) = run_clone(
            child,
            &AtomicBool::new(false),
            &channel,
            Duration::from_secs(10),
        );
        assert!(matches!(end, CloneEnd::Finished(status) if status.success()));
        // 3000 redraws of one phase are forwarded at the throttle's pace.
        let forwarded = sent.lock().unwrap().len();
        assert!(forwarded < 100, "forwarded {forwarded} progress lines");
    }

    #[cfg(unix)]
    #[test]
    fn a_silent_process_is_stopped_as_stalled() {
        let child = shell("echo 'Cloning into x...' >&2; sleep 30");
        let (channel, _) = recording_channel();
        let started = Instant::now();
        let (end, tail) = run_clone(
            child,
            &AtomicBool::new(false),
            &channel,
            Duration::from_millis(400),
        );
        assert!(matches!(end, CloneEnd::Stalled));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(tail, vec!["Cloning into x...".to_string()]);
    }

    // Cancel must SIGTERM the whole group, so git can remove its half-written
    // checkout; a SIGKILL would leave it behind.
    #[cfg(unix)]
    #[test]
    fn cancel_terminates_the_process_group_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cleaned-up");
        let helper_pid = dir.path().join("helper.pid");
        let child = shell(&format!(
            "trap 'touch {}; exit 0' TERM; sleep 30 & echo $! > {}; echo started >&2; wait",
            marker.display(),
            helper_pid.display()
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            trigger.store(true, Ordering::SeqCst);
        });
        let (channel, _) = recording_channel();
        let started = Instant::now();
        let (end, _) = run_clone(child, &cancel, &channel, GIT_CLONE_STALL_TIMEOUT);
        assert!(matches!(end, CloneEnd::Cancelled));
        assert!(marker.exists(), "the process did not get SIGTERM");
        // Like git-remote-https and index-pack, the helper must not outlive it.
        let helper: i32 = std::fs::read_to_string(&helper_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            unsafe { libc::kill(helper, 0) } != 0,
            "helper process {helper} survived the cancel"
        );
        assert!(
            started.elapsed() < TERMINATE_GRACE,
            "cancel waited for the SIGKILL fallback"
        );
    }
}
