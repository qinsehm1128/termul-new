//! Narrow service boundaries used by the desktop adapters and Core processes.
//!
//! These traits deliberately expose capabilities rather than concrete managers.
//! Standalone and tests use in-process implementations. Desktop composition
//! injects the same handle types so a later remote Core client can replace the
//! inner runtime without changing route or command payloads.
//!
//! This task does not invent a fake remote protocol. Remote-capable handles are
//! constructed with [`TerminalServiceHandle::from_runtime`] /
//! [`AcpServiceHandle::from_runtime`] once T4/T5 own the Core servers.

use super::ipc::{CoreError, CoreErrorPayload, CoreRequest, CoreResponse};
use super::terminal::TerminalCoreClient;
use crate::acp::AcpManager;
use crate::pty::PtyManager;
use async_trait::async_trait;
use std::sync::{Arc, Weak};

#[async_trait]
pub trait TerminalRuntimeHandle: Send + Sync {
    async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError>;
    async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError>;
    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError>;
    fn is_live(&self, terminal_id: &str) -> bool;
}

#[derive(Clone)]
enum InProcessPtyRef {
    Strong(Arc<PtyManager>),
    Weak(Weak<PtyManager>),
}

#[derive(Clone)]
pub struct InProcessTerminalRuntime {
    pty: InProcessPtyRef,
}

impl InProcessTerminalRuntime {
    pub fn new(pty: Arc<PtyManager>) -> Self {
        Self {
            pty: InProcessPtyRef::Strong(pty),
        }
    }

    pub fn from_weak(pty: Weak<PtyManager>) -> Self {
        Self {
            pty: InProcessPtyRef::Weak(pty),
        }
    }

    fn manager(&self) -> Result<Arc<PtyManager>, CoreError> {
        match &self.pty {
            InProcessPtyRef::Strong(pty) => Ok(Arc::clone(pty)),
            InProcessPtyRef::Weak(pty) => pty.upgrade().ok_or_else(|| {
                CoreError::InvalidRequest("in-process terminal manager is unavailable".into())
            }),
        }
    }

    pub fn in_process_pty(&self) -> Option<Arc<PtyManager>> {
        self.manager().ok()
    }
}

#[async_trait]
impl TerminalRuntimeHandle for InProcessTerminalRuntime {
    async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError> {
        self.manager()?
            .write(terminal_id, data)
            .await
            .map_err(CoreError::InvalidRequest)
    }

    async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError> {
        self.manager()?
            .resize(terminal_id, cols, rows)
            .await
            .map_err(CoreError::InvalidRequest)
    }

    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError> {
        self.manager()?
            .terminate(terminal_id)
            .await
            .map(|_| ())
            .map_err(|error| CoreError::InvalidRequest(error.to_string()))
    }

    fn is_live(&self, terminal_id: &str) -> bool {
        self.manager()
            .ok()
            .is_some_and(|pty| pty.get(terminal_id).is_some())
    }
}

/// Cloneable composition handle for terminal operations.
///
/// Commands, desktop wiring, and shared-live take this type instead of a
/// concrete `Arc<PtyManager>`. Standalone and tests keep in-process behavior
/// through [`Self::in_process`]; spawn/claim/replay still use the compatibility
/// accessor until Terminal Core owns those operations.
#[derive(Clone)]
pub struct TerminalServiceHandle {
    runtime: Arc<dyn TerminalRuntimeHandle>,
    in_process: Option<Arc<PtyManager>>,
    core: Option<Arc<TerminalCoreClient>>,
}

impl TerminalServiceHandle {
    pub fn in_process(pty: Arc<PtyManager>) -> Self {
        Self {
            runtime: Arc::new(InProcessTerminalRuntime::new(Arc::clone(&pty))),
            in_process: Some(pty),
            core: None,
        }
    }

    pub fn from_runtime(runtime: Arc<dyn TerminalRuntimeHandle>) -> Self {
        Self {
            runtime,
            in_process: None,
            core: None,
        }
    }

    pub fn from_core_client(client: TerminalCoreClient) -> Self {
        let client = Arc::new(client);
        Self {
            runtime: Arc::clone(&client) as Arc<dyn TerminalRuntimeHandle>,
            in_process: None,
            core: Some(client),
        }
    }

    pub fn runtime(&self) -> Arc<dyn TerminalRuntimeHandle> {
        Arc::clone(&self.runtime)
    }

    /// Weak-backed runtime so ACP can call PTY operations without retaining
    /// ownership of the in-process manager.
    pub fn runtime_for_acp(&self) -> Arc<dyn TerminalRuntimeHandle> {
        match self.in_process.as_ref() {
            Some(pty) => Arc::new(InProcessTerminalRuntime::from_weak(Arc::downgrade(pty))),
            None => Arc::clone(&self.runtime),
        }
    }

    pub fn in_process_pty(&self) -> Option<Arc<PtyManager>> {
        self.in_process.clone()
    }

    pub fn require_in_process_pty(&self) -> Result<Arc<PtyManager>, CoreError> {
        self.in_process_pty().ok_or_else(|| {
            CoreError::InvalidRequest("in-process terminal manager is unavailable".into())
        })
    }

    pub fn core_client(&self) -> Option<Arc<TerminalCoreClient>> {
        self.core.clone()
    }

    pub fn owns_core_process(&self) -> bool {
        self.core.is_some() && self.in_process.is_none()
    }
}

impl From<Arc<PtyManager>> for TerminalServiceHandle {
    fn from(pty: Arc<PtyManager>) -> Self {
        Self::in_process(pty)
    }
}

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
        // T5 owns ACP method dispatch over Core IPC. The in-process adapter is
        // a real handle, not a stub server: method calls stay on `AcpManager`
        // until that migration, and unknown core-IPC methods return a stable
        // error instead of panicking or inventing remote behavior.
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

/// Cloneable composition handle for ACP operations.
///
/// Desktop and shared-live inject this instead of holding only a concrete
/// `Arc<AcpManager>`. In-process callers keep using [`Self::in_process_manager`]
/// until ACP Core owns the live agent runtime.
#[derive(Clone)]
pub struct AcpServiceHandle {
    runtime: Arc<dyn AcpRuntimeHandle>,
    in_process: Option<Arc<AcpManager>>,
}

impl AcpServiceHandle {
    pub fn in_process(acp: Arc<AcpManager>) -> Self {
        Self {
            runtime: Arc::new(InProcessAcpRuntime::new(Arc::clone(&acp))),
            in_process: Some(acp),
        }
    }

    pub fn from_runtime(runtime: Arc<dyn AcpRuntimeHandle>) -> Self {
        Self {
            runtime,
            in_process: None,
        }
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
    use crate::trackers::{CwdTracker, ExitCodeTracker, GitTracker, TerminalEventHub};

    fn test_pty() -> Arc<PtyManager> {
        let events = TerminalEventHub::standalone();
        let cwd = Arc::new(CwdTracker::new(events.clone()));
        let git = Arc::new(GitTracker::new(None, events.clone()));
        let exit = Arc::new(ExitCodeTracker::new(events.clone()));
        Arc::new(PtyManager::new(events, cwd, git, exit))
    }

    #[tokio::test]
    async fn in_process_terminal_runtime_returns_stable_errors() {
        let runtime = InProcessTerminalRuntime::new(test_pty());
        let error = runtime
            .write("missing-terminal", "echo test")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST");
        assert_eq!(error.client_message(), "invalid core IPC request");
        assert!(!runtime.is_live("missing-terminal"));
    }

    #[tokio::test]
    async fn weak_in_process_runtime_fails_after_manager_drop() {
        let pty = test_pty();
        let runtime = InProcessTerminalRuntime::from_weak(Arc::downgrade(&pty));
        drop(pty);
        assert!(!runtime.is_live("any"));
        let error = runtime.write("any", "x").await.unwrap_err();
        assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST");
    }

    #[tokio::test]
    async fn terminal_service_handle_exposes_in_process_manager() {
        let pty = test_pty();
        let handle = TerminalServiceHandle::in_process(Arc::clone(&pty));
        assert!(Arc::ptr_eq(&handle.require_in_process_pty().unwrap(), &pty));
        assert!(handle.runtime().write("missing", "x").await.is_err());
        let remote_only = TerminalServiceHandle::from_runtime(handle.runtime());
        assert!(remote_only.in_process_pty().is_none());
        match remote_only.require_in_process_pty() {
            Ok(_) => panic!("remote-only handle must not expose an in-process manager"),
            Err(error) => assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST"),
        }
    }

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
