//! Host-backed CLI agent session discovery.
//!
//! 1. Scan vendor folders that encode the current project path (names only).
//! 2. Lazily read the first `session_id` from each JSONL file afterwards.
//! Transcript bodies are never consumed. Resume argv is assembled in the renderer.

pub mod commands;
mod parse;
mod paths;
mod scan;
mod scope;
mod types;
mod walk;

pub use scan::{list_cli_sessions, resolve_cli_sessions};
pub use types::{
    CliSessionListArgs, CliSessionListResult, CliSessionResolveArgs, CliSessionResolveResult,
};
