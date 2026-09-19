//! Shared contracts and local IPC for the independent desktop cores.
//!
//! `terminal` and `acp` process entrypoints are added in later migration
//! steps. This module starts with the transport-neutral boundary so desktop,
//! standalone, and tests can share the same service contracts.

pub mod acp;
pub mod handles;
pub mod ipc;
pub mod launcher;
pub mod terminal;
pub mod transport;
pub mod web_host;

pub use acp::{run_acp_core, run_acp_core_with_roots, AcpCoreClient, AcpCoreEvent};
pub use handles::{
    AcpRuntimeHandle, AcpServiceHandle, CoreServices, DetachedTerminalRuntime, InProcessAcpRuntime,
    InProcessTerminalRuntime, TerminalRuntimeHandle, TerminalServiceHandle,
};
pub use ipc::{
    negotiate_protocol, prepare_runtime_dir, read_frame, read_json_frame, remove_stale_socket,
    validate_hello, write_frame, write_json_frame, CoreEndpoint, CoreError, CoreErrorPayload,
    CoreEvent, CoreHello, CoreHelloAck, CoreRequest, CoreResponse, CoreRole,
    CURRENT_PROTOCOL_VERSION, MAX_FRAME_BYTES,
};
pub use launcher::{
    ensure_core, profile_root_from_env, run_core_process, CoreLaunchConfig, CoreProcess,
};
pub use terminal::{
    run_terminal_core, OutputFrame, OutputKind, TerminalAttachSession, TerminalCoreClient,
    TerminalStatus,
};
pub use web_host::{
    resolve_acp_web_host, AcpWebHost, AcpWebHostHandle, CoreAcpWebHost, CoreRelayHost,
    HostStartedPrompt, InProcessAcpWebHost, PermissionRequestInfo, SHARED_LIVE_UNAVAILABLE,
};
