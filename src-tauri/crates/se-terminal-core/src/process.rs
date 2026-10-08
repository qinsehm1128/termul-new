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
