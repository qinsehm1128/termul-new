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

use super::ipc::CoreError;
use super::terminal::TerminalCoreClient;
use crate::pty::{PtyManager, SpawnOptions};
use async_trait::async_trait;
use parking_lot::RwLock;
use se_foundation::ids::ConversationId;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Weak};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalConversationObservation {
    pub conversation_id: ConversationId,
    pub live_terminal_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalTerminationOutcome {
    Terminated,
    AlreadyGone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalConversationTermination {
    pub conversation_id: ConversationId,
    pub terminal_id: String,
    pub operation_id: String,
    pub outcome: TerminalTerminationOutcome,
}

#[async_trait]
pub trait TerminalRuntimeHandle: Send + Sync {
    async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError>;
    async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError>;
    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError>;
    async fn observe_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<TerminalConversationObservation, CoreError> {
        let _ = (conversation_id, terminal_ids);
        Err(CoreError::InvalidRequest(
            "conversation terminal observation is unavailable".into(),
        ))
    }
    async fn terminate_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        operation_id: &str,
    ) -> Result<TerminalConversationTermination, CoreError> {
        let _ = (conversation_id, terminal_id, operation_id);
        Err(CoreError::InvalidRequest(
            "conversation-scoped terminal termination is unavailable".into(),
        ))
    }
    /// Spawn a terminal from options the caller already derived on the
    /// trusted host side (see `conversation::TerminalSpawnIntentV1`).
    async fn spawn_trusted(&self, options: SpawnOptions) -> Result<String, CoreError> {
        let _ = options;
        Err(CoreError::InvalidRequest(
            "conversation terminal spawn is unavailable".into(),
        ))
    }
    fn is_live(&self, terminal_id: &str) -> bool;
    /// Whether `is_live` is a real observation. Detached ACP Core runtimes
    /// cannot see Terminal Core PTYs, so callers must fail closed instead of
    /// treating every id as dead.
    fn observes_live_terminals(&self) -> bool {
        true
    }
}

fn map_spawn_scope_error(error: String) -> CoreError {
    if error.ends_with("scope is unauthorized") {
        CoreError::Unauthorized
    } else {
        CoreError::InvalidRequest(error)
    }
}

/// Terminal runtime used by the ACP Core process: it owns no PTYs and cannot
/// reach one. Every operation reports the same stable error. `is_live` is
/// always false *and* `observes_live_terminals` is false so conversation
/// delete/suspend cannot claim cleanup succeeded. Replaced by a
/// Terminal-Core-linking runtime when the ACP Core drives terminal IPC.
pub struct DetachedTerminalRuntime;

#[async_trait]
impl TerminalRuntimeHandle for DetachedTerminalRuntime {
    async fn write(&self, _terminal_id: &str, _data: &str) -> Result<(), CoreError> {
        Err(CoreError::InvalidRequest(
            "acp core has no terminal runtime linked".into(),
        ))
    }

    async fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) -> Result<(), CoreError> {
        Err(CoreError::InvalidRequest(
            "acp core has no terminal runtime linked".into(),
        ))
    }

    async fn terminate(&self, _terminal_id: &str) -> Result<(), CoreError> {
        Err(CoreError::InvalidRequest(
            "acp core has no terminal runtime linked".into(),
        ))
    }

    fn is_live(&self, _terminal_id: &str) -> bool {
        false
    }

    fn observes_live_terminals(&self) -> bool {
        false
    }
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

    async fn observe_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<TerminalConversationObservation, CoreError> {
        let manager = self.manager()?;
        let ids = if terminal_ids.is_empty() {
            manager
                .get_all()
                .into_iter()
                .filter(|instance| {
                    instance.workspace_ref_tracked
                        && instance.conversation_matches(conversation_id)
                        && instance.is_active()
                })
                .map(|instance| instance.id.clone())
                .collect()
        } else {
            terminal_ids
                .iter()
                .filter(|terminal_id| {
                    manager.get(terminal_id).is_some_and(|instance| {
                        instance.workspace_ref_tracked
                            && instance.conversation_matches(conversation_id)
                            && instance.is_active()
                    })
                })
                .cloned()
                .collect()
        };
        Ok(TerminalConversationObservation {
            conversation_id,
            live_terminal_ids: ids,
        })
    }

    async fn terminate_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        operation_id: &str,
    ) -> Result<TerminalConversationTermination, CoreError> {
        let manager = self.manager()?;
        let Some(instance) = manager.get(terminal_id) else {
            return Ok(TerminalConversationTermination {
                conversation_id,
                terminal_id: terminal_id.to_string(),
                operation_id: operation_id.to_string(),
                outcome: TerminalTerminationOutcome::AlreadyGone,
            });
        };
        if !instance.workspace_ref_tracked || !instance.conversation_matches(conversation_id) {
            return Err(CoreError::Unauthorized);
        }
        manager
            .terminate(terminal_id)
            .await
            .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
        Ok(TerminalConversationTermination {
            conversation_id,
            terminal_id: terminal_id.to_string(),
            operation_id: operation_id.to_string(),
            outcome: TerminalTerminationOutcome::Terminated,
        })
    }

    async fn spawn_trusted(&self, options: SpawnOptions) -> Result<String, CoreError> {
        let spawned = self
            .manager()?
            .spawn(options, None)
            .await
            .map_err(map_spawn_scope_error)?;
        Ok(spawned.info.id)
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
    /// Quick terminals owned by this process. `None` in Core mode, where
    /// Terminal Core owns them and requests go through `core`.
    quick_terminals: Option<Arc<se_quick_terminal::QuickTerminalService>>,
}

impl TerminalServiceHandle {
    pub fn in_process(pty: Arc<PtyManager>) -> Self {
        Self {
            runtime: Arc::new(InProcessTerminalRuntime::new(Arc::clone(&pty))),
            in_process: Some(pty),
            core: None,
            quick_terminals: None,
        }
    }

    pub fn from_runtime(runtime: Arc<dyn TerminalRuntimeHandle>) -> Self {
        Self {
            runtime,
            in_process: None,
            core: None,
            quick_terminals: None,
        }
    }

    pub fn from_core_client(client: TerminalCoreClient) -> Self {
        let client = Arc::new(client);
        Self {
            runtime: Arc::clone(&client) as Arc<dyn TerminalRuntimeHandle>,
            in_process: None,
            core: Some(client),
            quick_terminals: None,
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

    /// Host quick terminals in this process (no Terminal Core).
    #[must_use]
    pub fn with_quick_terminals(
        mut self,
        service: Arc<se_quick_terminal::QuickTerminalService>,
    ) -> Self {
        self.quick_terminals = Some(service);
        self
    }

    pub fn quick_terminals(&self) -> Option<Arc<se_quick_terminal::QuickTerminalService>> {
        self.quick_terminals.clone()
    }
}

#[derive(Clone)]
pub struct SwitchableTerminalRuntime {
    current: Arc<RwLock<Arc<dyn TerminalRuntimeHandle>>>,
}

impl SwitchableTerminalRuntime {
    pub fn new(initial: Arc<dyn TerminalRuntimeHandle>) -> Self {
        Self {
            current: Arc::new(RwLock::new(initial)),
        }
    }

    pub fn replace(&self, runtime: Arc<dyn TerminalRuntimeHandle>) {
        *self.current.write() = runtime;
    }
}

#[async_trait]
impl TerminalRuntimeHandle for SwitchableTerminalRuntime {
    async fn write(&self, terminal_id: &str, data: &str) -> Result<(), CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime.write(terminal_id, data).await
    }

    async fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime.resize(terminal_id, cols, rows).await
    }

    async fn terminate(&self, terminal_id: &str) -> Result<(), CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime.terminate(terminal_id).await
    }

    async fn observe_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<TerminalConversationObservation, CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime
            .observe_conversation(conversation_id, terminal_ids)
            .await
    }

    async fn terminate_for_conversation(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        operation_id: &str,
    ) -> Result<TerminalConversationTermination, CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime
            .terminate_for_conversation(conversation_id, terminal_id, operation_id)
            .await
    }

    async fn spawn_trusted(&self, options: SpawnOptions) -> Result<String, CoreError> {
        let runtime = Arc::clone(&self.current.read());
        runtime.spawn_trusted(options).await
    }

    fn is_live(&self, terminal_id: &str) -> bool {
        self.current.read().is_live(terminal_id)
    }

    fn observes_live_terminals(&self) -> bool {
        self.current.read().observes_live_terminals()
    }
}

impl From<Arc<PtyManager>> for TerminalServiceHandle {
    fn from(pty: Arc<PtyManager>) -> Self {
        Self::in_process(pty)
    }
}

#[cfg(test)]
mod switchable_tests {
    use super::*;

    struct StaticRuntime {
        observed: bool,
    }

    #[async_trait]
    impl TerminalRuntimeHandle for StaticRuntime {
        async fn write(&self, _terminal_id: &str, _data: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn resize(
            &self,
            _terminal_id: &str,
            _cols: u16,
            _rows: u16,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn terminate(&self, _terminal_id: &str) -> Result<(), CoreError> {
            Ok(())
        }

        fn is_live(&self, _terminal_id: &str) -> bool {
            self.observed
        }

        fn observes_live_terminals(&self) -> bool {
            self.observed
        }
    }

    #[tokio::test]
    async fn switchable_runtime_replaces_detached_observation_atomically() {
        let detached = Arc::new(DetachedTerminalRuntime);
        let switchable = SwitchableTerminalRuntime::new(detached);
        assert!(!switchable.observes_live_terminals());
        assert!(switchable
            .observe_conversation(ConversationId::new_v4(), &[])
            .await
            .is_err());

        switchable.replace(Arc::new(StaticRuntime { observed: true }));
        assert!(switchable.observes_live_terminals());
        assert!(switchable.is_live("terminal-1"));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::trackers::{CwdTracker, ExitCodeTracker, GitTracker, TerminalEventHub};

    pub(crate) fn test_pty() -> Arc<PtyManager> {
        let events = TerminalEventHub::standalone();
        let cwd = Arc::new(CwdTracker::new(events.clone()));
        let git = Arc::new(GitTracker::new(None, events.clone()));
        let exit = Arc::new(ExitCodeTracker::new(events.clone()));
        Arc::new(PtyManager::new(
            events,
            cwd,
            git,
            exit,
            crate::terminal_program(),
        ))
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
    async fn detached_terminal_runtime_cannot_observe_or_claim_liveness() {
        let runtime = DetachedTerminalRuntime;
        assert!(!runtime.observes_live_terminals());
        assert!(!runtime.is_live("any"));
        let error = runtime.terminate("any").await.unwrap_err();
        assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST");
        let handle = TerminalServiceHandle::from_runtime(Arc::new(DetachedTerminalRuntime));
        assert!(!handle.runtime().observes_live_terminals());
        assert!(!handle.runtime().is_live("any"));
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
}
