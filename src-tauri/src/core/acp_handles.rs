//! ACP service handles for desktop composition.
//!
//! Split from `handles.rs` so the Terminal Core's build identity (which hashes
//! `handles.rs`) does not change when only ACP wiring changes.

use super::acp::AcpCoreClient;
use super::handles::TerminalServiceHandle;
use super::ipc::{CoreError, CoreErrorPayload, CoreRequest, CoreResponse};
use crate::acp::AcpManager;
use crate::pty::PtyManager;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait AcpRuntimeHandle: Send + Sync {
    async fn request(&self, request: CoreRequest) -> Result<CoreResponse, CoreError>;
}

#[derive(Clone)]
pub struct InProcessAcpRuntime {
    acp: Arc<AcpManager>,
}

impl InProcessAcpRuntime {
    pub fn new(acp: Arc<AcpManager>) -> Self {
        Self { acp }
    }

    pub fn manager(&self) -> Arc<AcpManager> {
        Arc::clone(&self.acp)
    }
}

#[async_trait]
impl AcpRuntimeHandle for InProcessAcpRuntime {
    async fn request(&self, request: CoreRequest) -> Result<CoreResponse, CoreError> {
        // In-process adapter: ACP method dispatch stays on `AcpManager`.
        // Unknown core-IPC methods return a stable error instead of panicking
        // or inventing remote behavior. Core IPC dispatch lives in `core/acp.rs`.
        let _ = &self.acp;
        Ok(CoreResponse {
            id: request.id,
            result: None,
            error: Some(CoreErrorPayload {
                code: CoreError::InvalidRequest(String::new()).code().to_string(),
                message: format!(
                    "acp method '{}' is not exported over core IPC",
                    request.method
                ),
            }),
        })
    }
}

#[async_trait]
impl AcpRuntimeHandle for AcpCoreClient {
    async fn request(&self, request: CoreRequest) -> Result<CoreResponse, CoreError> {
        AcpCoreClient::raw_request(self, request).await
    }
}

/// Cloneable composition handle for ACP operations.
///
/// Desktop and shared-live inject this instead of holding only a concrete
/// `Arc<AcpManager>`. In-process callers keep using [`Self::in_process_manager`]
/// until ACP Core owns the live agent runtime.
#[derive(Clone)]
pub struct AcpServiceHandle {
    runtime: Arc<dyn AcpRuntimeHandle>,
    in_process: Option<Arc<AcpManager>>,
    core: Option<Arc<AcpCoreClient>>,
}

impl AcpServiceHandle {
    pub fn in_process(acp: Arc<AcpManager>) -> Self {
        Self {
            runtime: Arc::new(InProcessAcpRuntime::new(Arc::clone(&acp))),
            in_process: Some(acp),
            core: None,
        }
    }

    pub fn from_runtime(runtime: Arc<dyn AcpRuntimeHandle>) -> Self {
        Self {
            runtime,
            in_process: None,
            core: None,
        }
    }

    /// Core-backed handle: the runtime forwards raw core-IPC requests to the
    /// ACP Core process. There is no in-process manager in this mode.
    pub fn from_core_client(client: AcpCoreClient) -> Self {
        Self::from_core_client_arc(Arc::new(client))
    }

    pub fn from_core_client_arc(client: Arc<AcpCoreClient>) -> Self {
        Self {
            runtime: Arc::clone(&client) as Arc<dyn AcpRuntimeHandle>,
            in_process: None,
            core: Some(client),
        }
    }

    pub fn core_client(&self) -> Option<Arc<AcpCoreClient>> {
        self.core.clone()
    }

    pub fn owns_core_process(&self) -> bool {
        self.core.is_some() && self.in_process.is_none()
    }

    pub fn runtime(&self) -> Arc<dyn AcpRuntimeHandle> {
        Arc::clone(&self.runtime)
    }

    pub fn in_process_manager(&self) -> Option<Arc<AcpManager>> {
        self.in_process.clone()
    }

    pub fn require_in_process(&self) -> Result<Arc<AcpManager>, CoreError> {
        self.in_process_manager().ok_or_else(|| {
            CoreError::InvalidRequest("in-process ACP manager is unavailable".into())
        })
    }
}

impl From<Arc<AcpManager>> for AcpServiceHandle {
    fn from(acp: Arc<AcpManager>) -> Self {
        Self::in_process(acp)
    }
}

/// Desktop/standalone composition bundle. Wires ACP onto a narrow terminal
/// runtime without exposing the full PTY manager as ACP's stored dependency.
#[derive(Clone)]
pub struct CoreServices {
    pub terminal: TerminalServiceHandle,
    pub acp: AcpServiceHandle,
}

impl CoreServices {
    pub fn in_process(pty: Arc<PtyManager>, acp: Arc<AcpManager>) -> Self {
        let terminal = TerminalServiceHandle::in_process(pty);
        acp.set_terminal_service(terminal.clone());
        Self {
            terminal,
            acp: AcpServiceHandle::in_process(acp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_pty::test_pty_manager as test_pty;

    #[tokio::test]
    async fn in_process_acp_runtime_returns_stable_unknown_method() {
        let acp = Arc::new(AcpManager::new(vec![]));
        let runtime = InProcessAcpRuntime::new(acp);
        let response = runtime
            .request(CoreRequest {
                id: 7,
                method: "listAgents".into(),
                params: serde_json::Value::Null,
            })
            .await
            .unwrap();
        assert_eq!(response.id, 7);
        assert!(response.result.is_none());
        let error = response.error.expect("unknown method is a core error");
        assert_eq!(error.code, "CORE_IPC_INVALID_REQUEST");
        assert!(error.message.contains("listAgents"));
    }

    #[test]
    fn core_services_installs_narrow_terminal_runtime_on_acp() {
        let pty = test_pty();
        let acp = Arc::new(AcpManager::new(vec![]));
        let services = CoreServices::in_process(Arc::clone(&pty), Arc::clone(&acp));
        assert!(acp.terminal_runtime().is_some());
        assert!(
            Arc::ptr_eq(&acp.pty_manager().expect("compat pty"), &pty),
            "compatibility accessor still upgrades the in-process manager"
        );
        assert!(services.acp.in_process_manager().is_some());
        assert!(services.terminal.in_process_pty().is_some());
        drop(services);
        drop(pty);
        assert!(
            acp.pty_manager().is_none(),
            "ACP must not retain PTY ownership"
        );
        assert!(!acp
            .terminal_runtime()
            .expect("runtime remains installed")
            .is_live("missing"));
    }
}
