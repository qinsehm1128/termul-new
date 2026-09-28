//! Terminal state trackers module
//!
//! This module contains trackers for monitoring terminal state:
//! - CWD (Current Working Directory) tracking
//! - Git branch and status tracking
//! - Exit code tracking
//! - OSC 0/2 window title tracking

pub mod cwd_tracker;
pub mod exit_code_tracker;
pub mod git_tracker;
pub mod osc_title_tracker;
pub mod terminal_events;

pub use cwd_tracker::CwdTracker;
pub use exit_code_tracker::ExitCodeTracker;
pub use git_tracker::{GitCommit, GitStatus, GitStatusDetail, GitTracker};
pub use osc_title_tracker::OscTitleTracker;
#[allow(unused_imports)]
pub use terminal_events::TerminalStateSnapshot;
pub use terminal_events::{TerminalDisplayMode, TerminalEvent, TerminalEventHub};
