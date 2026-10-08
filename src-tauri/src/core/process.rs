//! Core process entry on the app executable: `--terminal-core` / `--acp-core`.
//!
//! The Terminal Core also ships as its own `se-terminal-core` executable,
//! which the launcher prefers; the flag stays for a GUI that predates it.
//! Kept apart from `launcher.rs`, which is the GUI side (spawn, adopt, update
//! reconciliation, replacement).

use super::ipc::CoreRole;
pub use se_terminal_core::process::{profile_root_from_env, workspace_base_from_env};

pub fn run_core_process(role: CoreRole) -> i32 {
    match role {
        CoreRole::TerminalCore => {
            se_terminal_core::process::run_terminal_core_process(crate::terminal_program())
        }
        _ => se_terminal_core::process::run_core_process(
            role,
            env!("CARGO_PKG_VERSION"),
            super::acp::run_acp_core,
        ),
    }
}
