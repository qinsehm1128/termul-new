//! Native plugin host, loaded only inside Plugin Core.

mod loader;
mod abi_worker;
mod adapter;
mod broker;
mod registry;
mod config;
mod validation;
mod runtime;

pub use runtime::{HostDiagnostics, HostOptions, PluginHost, RestartReason};
