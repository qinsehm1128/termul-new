//! ACP Core process boundary.
//!
//! This module owns an ACP manager in a process independent from the GUI. The
//! standalone server keeps its existing in-process composition; desktop wiring
//! can progressively replace command adapters with `AcpCoreClient` without
//! changing renderer-visible payloads.

use super::ipc::{
    prepare_runtime_dir, read_frame, read_json_frame, remove_stale_socket, validate_hello,
    write_json_frame, CoreEndpoint, CoreError, CoreHello, CoreRequest, CoreResponse, CoreRole,
};
use crate::acp::config::{AgentConfig, AgentId};
use crate::acp::manager::AcpManager;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;

pub const METHOD_HEALTH: &str = "health";
pub const METHOD_LIST_AGENTS: &str = "listAgents";
pub const METHOD_SPAWN_AGENT: &str = "spawnAgent";
pub const METHOD_KILL_AGENT: &str = "killAgent";
pub const METHOD_SHUTDOWN: &str = "shutdown";

struct AcpCoreState {
    manager: Arc<AcpManager>,
    shutdown: watch::Sender<bool>,
}

fn invalid(detail: impl Into<String>) -> CoreError {
    CoreError::InvalidRequest(detail.into())
}

fn response(id: u64, result: Result<Value, CoreError>) -> CoreResponse {
    match result {
        Ok(result) => CoreResponse {
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => CoreResponse {
            id,
            result: None,
            error: Some(super::ipc::CoreErrorPayload {
                code: error.code().to_string(),
                message: match error {
                    CoreError::InvalidRequest(detail) if !detail.is_empty() => detail,
                    other => other.client_message().to_string(),
                },
            }),
        },
    }
}

pub async fn run_acp_core(profile_root: PathBuf) -> Result<(), CoreError> {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let endpoint = CoreEndpoint::for_profile(&profile_root, CoreRole::AcpCore);
        prepare_runtime_dir(&endpoint)?;
        let _ = remove_stale_socket(&endpoint);
        let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
        let listener = UnixListener::bind(path).map_err(CoreError::from)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(CoreError::from)?;

        // The manager and all agent driver threads belong to this process. The
        // desktop/standalone compositions remain responsible for their own
        // durable sinks until the ACP persistence adapter is moved behind this
        // same boundary in the next compatibility slice.
        let manager = Arc::new(AcpManager::new(Vec::new()));
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let state = Arc::new(AcpCoreState { manager, shutdown });
        log::info!(
            target: "se_manager::core",
            "operation=core_listen role=acp-core stable_code=READY"
        );

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        state.manager.stop_producers().await.map_err(invalid)?;
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(CoreError::from)?;
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(error) = handle_connection(stream, state).await {
                            log::debug!(target: "se_manager::core", "operation=acp_connection stable_code={}", error.code());
                        }
                    });
                }
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = profile_root;
        Err(CoreError::UnsupportedPlatform)
    }
}

#[cfg(unix)]
async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    state: Arc<AcpCoreState>,
) -> Result<(), CoreError> {
    let hello: CoreHello = read_json_frame(&mut stream).await?;
    let ack = validate_hello(&hello, CoreRole::AcpCore)?;
    write_json_frame(&mut stream, &ack).await?;

    loop {
        let payload = match read_frame(&mut stream).await {
            Ok(payload) => payload,
            Err(CoreError::Io(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        let request: CoreRequest = serde_json::from_slice(&payload).map_err(|error| {
            CoreError::InvalidFrame(format!("invalid ACP core request: {error}"))
        })?;
        let shutdown = request.method == METHOD_SHUTDOWN;
        let reply = response(request.id, dispatch(&state, &request).await);
        write_json_frame(&mut stream, &reply).await?;
        if shutdown {
            let _ = state.shutdown.send(true);
            return Ok(());
        }
    }
}

async fn dispatch(state: &AcpCoreState, request: &CoreRequest) -> Result<Value, CoreError> {
    match request.method.as_str() {
        METHOD_HEALTH => Ok(json!({"role": "acp-core", "status": "ready"})),
        METHOD_LIST_AGENTS => Ok(serde_json::to_value(state.manager.list_agents())
            .map_err(|error| invalid(error.to_string()))?),
        METHOD_SPAWN_AGENT => {
            let config: AgentConfig = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            let outcome = state.manager.spawn(config).await.map_err(invalid)?;
            serde_json::to_value(outcome).map_err(|error| invalid(error.to_string()))
        }
        METHOD_KILL_AGENT => {
            let agent_id: AgentId = serde_json::from_value(request.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            state.manager.kill(&agent_id).await.map_err(invalid)?;
            Ok(Value::Null)
        }
        METHOD_SHUTDOWN => Ok(Value::Null),
        other => Err(invalid(format!(
            "acp method '{other}' is not exported over core IPC"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_has_explicit_control_methods() {
        assert_eq!(METHOD_HEALTH, "health");
        assert_eq!(METHOD_LIST_AGENTS, "listAgents");
    }
}
