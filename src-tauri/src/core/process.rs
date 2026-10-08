//! Core process entry: what runs inside `--terminal-core` / `--acp-core`.
//!
//! Kept apart from `launcher.rs`, which is the GUI side (spawn, adopt, update
//! reconciliation, replacement). Release build identities hash source files,
//! so GUI-only launcher changes must not live in a file the Cores' identities
//! include (`scripts/release/component-build-inputs.json`).

#[cfg(any(unix, windows))]
use super::ipc::CoreError;
use super::ipc::CoreRole;
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
    let brand = crate::brand::canonical().display_name.to_string();
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

#[cfg(any(unix, windows))]
pub fn run_core_process(role: CoreRole) -> i32 {
    let logging_to_file = crate::logging::install_core_logger().is_some();
    if logging_to_file {
        log::info!(
            target: "se_manager::core",
            "operation=core_process_start role={} pid={} version={}",
            role.endpoint_name(),
            std::process::id(),
            env!("CARGO_PKG_VERSION")
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
        CoreRole::TerminalCore => {
            runtime.block_on(super::terminal::run_terminal_core(profile_root_from_env()))
        }
        CoreRole::AcpCore => runtime.block_on(super::acp::run_acp_core(profile_root_from_env())),
        CoreRole::Gui => Err(CoreError::InvalidRequest(
            "GUI is not a launchable core role".to_string(),
        )),
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
pub fn run_core_process(role: CoreRole) -> i32 {
    eprintln!(
        "{} process is not supported on this platform yet",
        role.endpoint_name()
    );
    2
}
