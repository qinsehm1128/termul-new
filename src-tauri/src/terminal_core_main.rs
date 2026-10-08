//! `se-terminal-core`: the Terminal Core as its own executable. It links only
//! the `se-terminal-core` crate — no WebView, no app library — so it stays
//! small and its build identity follows that crate's dependency closure.
//! The GUI launcher starts it from beside the app executable.

// Spawned by the GUI with null stdio; no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::process::exit(se_terminal_core::process::run_terminal_core_process(
        se_pty::TerminalProgram {
            name: se_foundation::brand::canonical().display_name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    ));
}
