//! Core process entry: logging, the async runtime and the exit code around a
//! Core's service loop. Shared by the Terminal Core and the ACP Core.

use crate::ipc::{CoreError, CoreRole};
use se_pty::TerminalProgram;
use std::future::Future;
use std::path::PathBuf;

/// Resolve the user-visible workspace base (Conversation and quick terminal folders) the same way the desktop does:
/// explicit env override, else `<home>/Documents/<brand>`, else `<home>/<brand>`.
/// The GUI launcher passes its own computed root via env so both processes
/// always agree; the fallback keeps the Core runnable standalone in tests.
pub fn workspace_base_from_env() -> PathBuf {
    if let Some(root) = std::env::var_os("TERMUL_CORE_WORKSPACE_ROOT") {
        let root = PathBuf::from(root);
        if root.as_os_str().is_empty() {
            // fall through to the derived default
        } else {
            return root;
        }
    }
    #[cfg(unix)]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    #[cfg(not(any(unix, windows)))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let brand = se_foundation::brand::canonical().display_name.to_string();
    match home {
        Some(home) => home.join("Documents").join(&brand),
        None => std::env::temp_dir().join(brand),
    }
}

pub fn profile_root_from_env() -> PathBuf {
    std::env::var_os("TERMUL_CORE_PROFILE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("termul-core"))
}

/// The Terminal Core process. `program` names the app to spawned shells and
/// carries the version logged at start.
pub fn run_terminal_core_process(program: TerminalProgram) -> i32 {
    let version = program.version.clone();
    run_core_process(CoreRole::TerminalCore, &version, |profile_root| {
        crate::terminal::run_terminal_core(profile_root, program)
    })
}

/// Run one Core process: install the Core logger, build the runtime, serve
/// `role` from the profile root in the environment, and map the result to an
/// exit code.
#[cfg(any(unix, windows))]
pub fn run_core_process<F, Fut>(role: CoreRole, version: &str, serve: F) -> i32
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<(), CoreError>>,
{
    let logging_to_file = se_foundation::logging::install_core_logger().is_some();
    if logging_to_file {
        log::info!(
            target: "se_manager::core",
            "operation=core_process_start role={} pid={} version={version}",
            role.endpoint_name(),
            std::process::id(),
        );
    }
    #[cfg(unix)]
    witness_termination_signals(role);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("core runtime initialization failed: {error}");
            return 1;
        }
    };
    let result = match role {
        CoreRole::Gui => Err(CoreError::InvalidRequest(
            "GUI is not a launchable core role".to_string(),
        )),
        _ => runtime.block_on(serve(profile_root_from_env())),
    };
    match result {
        Ok(()) => {
            log::info!(
                target: "se_manager::core",
                "operation=core_process_exit role={} stable_code=OK",
                role.endpoint_name()
            );
            0
        }
        Err(error) => {
            log::error!(
                target: "se_manager::core",
                "operation=core_process_exit role={} stable_code={} error={error}",
                role.endpoint_name(),
                error.code()
            );
            if !logging_to_file {
                eprintln!("{} process failed: {error}", role.endpoint_name());
            }
            1
        }
    }
}

/// Record who ends this Core, then let the signal end it as it always did.
///
/// Cores were being killed by signals with nothing in their log: no exit line,
/// no panic, every terminal and agent gone with them. The sender is the one
/// fact that names the cause, and only the receiving process can see it. The
/// handler changes nothing else — after logging, the default action runs.
#[cfg(unix)]
fn witness_termination_signals(role: CoreRole) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::{exfiltrator::WithOrigin, SignalsInfo};

    let mut signals = match SignalsInfo::<WithOrigin>::new([SIGTERM, SIGHUP, SIGINT]) {
        Ok(signals) => signals,
        Err(error) => {
            log::warn!(
                target: "se_manager::core",
                "operation=core_signal_witness role={} stable_code=UNAVAILABLE error={error}",
                role.endpoint_name()
            );
            return;
        }
    };
    let role = role.endpoint_name();
    let _ = std::thread::Builder::new()
        .name("core-signal-witness".into())
        .spawn(move || {
            let Some(origin) = signals.forever().next() else {
                return;
            };
            let sender = origin.process.map(|process| process.pid);
            log::warn!(
                target: "se_manager::core",
                "operation=core_signal role={role} signal={} sender_pid={} sender={} stable_code=TERMINATED_BY_SIGNAL",
                signal_name(origin.signal),
                sender.map_or_else(|| "unknown".to_string(), |pid| pid.to_string()),
                sender
                    .and_then(process_path)
                    .unwrap_or_else(|| "unknown".to_string())
            );
            log::logger().flush();
            let _ = signal_hook::low_level::emulate_default_handler(origin.signal);
        });
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        other => other.to_string(),
    }
}

/// Executable of `pid`, if it is still around to ask.
#[cfg(target_os = "macos")]
fn process_path(pid: libc::pid_t) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    String::from_utf8(buffer).ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_path(pid: libc::pid_t) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.display().to_string())
}

#[cfg(not(any(unix, windows)))]
pub fn run_core_process<F, Fut>(role: CoreRole, _version: &str, _serve: F) -> i32
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<(), CoreError>>,
{
    eprintln!(
        "{} process is not supported on this platform yet",
        role.endpoint_name()
    );
    2
}

#[cfg(all(test, unix))]
mod signal_witness_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_ENV: &str = "SE_SIGNAL_WITNESS_CHILD";
    const TEST_PATH: &str =
        "process::signal_witness_tests::a_terminating_signal_is_logged_with_its_sender";

    /// Runs twice: as the test, and — re-executed with `CHILD_ENV` — as the
    /// Core-like process the test signals.
    #[test]
    fn a_terminating_signal_is_logged_with_its_sender() {
        if let Some(ready) = std::env::var_os(CHILD_ENV) {
            // The real Core entry, serving a stand-in that just waits.
            run_core_process(CoreRole::TerminalCore, "test", |_| async move {
                std::fs::write(ready, "ready").unwrap();
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            });
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("core.log");
        let ready = dir.path().join("ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_PATH, "--test-threads=1"])
            .env(CHILD_ENV, &ready)
            .env(se_foundation::logging::CORE_LOG_FILE_ENV, &log)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "child never became ready");
            std::thread::sleep(Duration::from_millis(20));
        }

        // SAFETY: plain kill(2) on our own child.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        let status = child.wait().unwrap();

        // Still ends by the signal: the witness only records.
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(&format!(
                "operation=core_signal role=terminal-core signal=SIGTERM sender_pid={} ",
                std::process::id()
            )),
            "{text}"
        );
        let test_binary = std::env::current_exe().unwrap();
        let name = test_binary.file_name().unwrap().to_string_lossy();
        assert!(
            text.lines()
                .any(|line| line.contains("sender=") && line.contains(name.as_ref())),
            "{text}"
        );
    }
}
