//! Native plugin host, loaded only inside Plugin Core.

mod abi_worker;
mod adapter;
mod broker;
mod config;
mod loader;
mod registry;
mod runtime;
mod validation;

pub use runtime::{HostDiagnostics, HostOptions, PluginHost, RestartReason};
