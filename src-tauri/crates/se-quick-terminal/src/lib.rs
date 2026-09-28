//! Quick terminal sessions.
//!
//! A quick terminal is a folder with a live shell in it and nothing else: no
//! agent, no transcript, no binding. It is deliberately a separate model from
//! agent sessions so neither can drag the other's lifecycle along — opening a
//! quick terminal never touches the agent Core.
//!
//! [`QuickTerminalStore`] persists one small JSON record per quick terminal.
//! [`QuickTerminalService`] adds the folder allocation and PTY handling on top.

mod record;
mod service;
mod store;

pub use record::{
    QuickTerminalOrigin, QuickTerminalRecord, QuickTerminalTarget, QUICK_TERMINAL_SCHEMA_VERSION,
};
pub use service::{
    CreateQuickTerminal, OpenQuickTerminal, QuickTerminalError, QuickTerminalOpened,
    QuickTerminalService,
};
pub use store::QuickTerminalStore;

/// Canonical identity of a quick terminal; also the PTY ownership scope.
pub use se_foundation::ids::ConversationId as QuickTerminalId;
