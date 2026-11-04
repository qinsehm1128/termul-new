//! Native plugin host, loaded only inside Plugin Core.

mod abi_worker;
mod adapter;
mod archive;
mod broker;
mod config;
mod host_context;
mod host_feedback;
mod host_services;
mod loader;
mod managed_process;
mod registry;
mod runtime;
mod storage;
mod validation;
mod web_sessions;

pub use runtime::{HostDiagnostics, HostOptions, PluginHost, RestartReason};
