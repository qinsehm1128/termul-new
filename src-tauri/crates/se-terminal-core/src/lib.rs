//! Terminal Core: the PTY service that outlives the GUI, its in-process and
//! remote handles, and the client the GUI uses to reach it.
//!
//! Everything the `se-terminal-core` executable runs is in this crate or its
//! dependencies, so its release build identity is this crate plus its Cargo
//! dependency closure (`scripts/release/component-build-inputs.json`).

pub mod handles;
pub mod ipc;
pub mod process;
pub mod quick_terminal;
pub mod terminal;
pub mod transport;
