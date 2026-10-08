//! PTY (Pseudo-Terminal) management and terminal state trackers.
//!
//! This crate handles terminal spawning, data I/O, lifecycle, and per-terminal
//! trackers (cwd, git, exit codes, titles). It knows nothing about agent
//! sessions: an owning session is an opaque `ConversationId` scope.

pub mod claims;
pub mod da_filter;
pub mod env_refresh;
pub mod manager;
pub mod trackers;

#[cfg(target_os = "windows")]
pub mod windows;

pub use claims::RotatedClaim;
pub use da_filter::DaFilter;
pub use manager::{OutputSink, PtyManager, SpawnOptions, TerminalProgram};

/// A standalone `PtyManager` for tests in this crate and its dependents.
#[cfg(any(test, feature = "test-support"))]
pub fn test_pty_manager() -> std::sync::Arc<PtyManager> {
    use trackers::{CwdTracker, ExitCodeTracker, GitTracker, TerminalEventHub};
    let events = TerminalEventHub::standalone();
    let cwd = std::sync::Arc::new(CwdTracker::new(events.clone()));
    let git = std::sync::Arc::new(GitTracker::new(events.clone()));
    let exit = std::sync::Arc::new(ExitCodeTracker::new(events.clone()));
    std::sync::Arc::new(PtyManager::new(
        events,
        cwd,
        git,
        exit,
        TerminalProgram {
            name: "se-test".to_string(),
            version: "0.0.0-test".to_string(),
        },
    ))
}
