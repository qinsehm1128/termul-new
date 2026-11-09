//! Host-owned registry for upgraded ACP and terminal WebSocket connections.
//!
//! TASK-004 registers every upgrade. TASK-005 joins the registry under the
//! host deadline: stop admission, revoke generations, cancel and await, then
//! stop producers and drain.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradedConnectionKind {
    Acp,
    Terminal,
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradedConnectionReceipt {
    pub active: u64,
    pub failed: u64,
    pub timed_out: u64,
    pub cancelled: u64,
}

struct RegisteredConnection {
    id: Uuid,
    kind: UpgradedConnectionKind,
    device_id: Option<String>,
    cancel: CancellationToken,
}

struct RegistryInner {
    connections: Vec<RegisteredConnection>,
    failed: u64,
    timed_out: u64,
    cancelled: u64,
    admitting: bool,
}

pub struct UpgradedConnectionRegistry {
    inner: Mutex<RegistryInner>,
    generation: AtomicU64,
    /// Signalled whenever a connection completes, so `join_all` can wait.
    completed: Notify,
}

/// One admitted upgraded connection. The connection task holds it for its
/// whole life and stops when its token is cancelled; dropping it, however the
/// task ends, completes the connection.
pub struct UpgradedConnectionTicket {
    registry: Arc<UpgradedConnectionRegistry>,
    id: Uuid,
    cancel: CancellationToken,
}

impl UpgradedConnectionTicket {
    /// Bind a connection authenticated by its first frame. The ticket stays
    /// admitted throughout, avoiding an untracked connection during authentication.
    pub fn bind_device(&self, device_id: &str) {
        let mut inner = self.registry.inner.lock();
        if let Some(connection) = inner.connections.iter_mut().find(|c| c.id == self.id) {
            connection.device_id = Some(device_id.to_owned());
        }
    }

    /// Cancelled when the host shuts down; the connection must then end.
    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for UpgradedConnectionTicket {
    fn drop(&mut self) {
        self.registry.complete(self.id, false);
    }
}

impl Default for UpgradedConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl UpgradedConnectionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                connections: Vec::new(),
                failed: 0,
                timed_out: 0,
                cancelled: 0,
                admitting: true,
            }),
            generation: AtomicU64::new(1),
            completed: Notify::new(),
        }
    }

    /// Process-wide registry so TASK-005 can join without an AppState field.
    #[must_use]
    pub fn global() -> Arc<Self> {
        static GLOBAL: std::sync::OnceLock<Arc<UpgradedConnectionRegistry>> =
            std::sync::OnceLock::new();
        GLOBAL
            .get_or_init(|| Arc::new(UpgradedConnectionRegistry::new()))
            .clone()
    }

    pub fn stop_admission(&self) {
        self.inner.lock().admitting = false;
    }

    pub fn revoke_generations(&self) -> u64 {
        self.generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1)
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Admit an upgraded connection, or `None` once the host has stopped
    /// admission; the caller must then drop the socket without serving it.
    pub fn admit(
        self: &Arc<Self>,
        kind: UpgradedConnectionKind,
    ) -> Option<UpgradedConnectionTicket> {
        self.admit_inner(kind, None)
    }

    pub fn admit_device(
        self: &Arc<Self>,
        kind: UpgradedConnectionKind,
        device_id: &str,
    ) -> Option<UpgradedConnectionTicket> {
        self.admit_inner(kind, Some(device_id.to_owned()))
    }

    /// Counts include cancelled connections until their task releases its ticket.
    pub fn device_counts(&self) -> std::collections::HashMap<String, u64> {
        let mut counts = std::collections::HashMap::new();
        for connection in &self.inner.lock().connections {
            if let Some(device_id) = &connection.device_id {
                *counts.entry(device_id.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    pub fn device_connection_count(&self, device_id: &str) -> u64 {
        self.inner
            .lock()
            .connections
            .iter()
            .filter(|connection| connection.device_id.as_deref() == Some(device_id))
            .count() as u64
    }

    pub fn cancel_device(&self, device_id: &str) -> u64 {
        let mut inner = self.inner.lock();
        let mut cancelled = 0;
        for connection in &inner.connections {
            if connection.device_id.as_deref() == Some(device_id)
                && !connection.cancel.is_cancelled()
            {
                connection.cancel.cancel();
                cancelled += 1;
            }
        }
        inner.cancelled = inner.cancelled.saturating_add(cancelled);
        cancelled
    }

    fn admit_inner(
        self: &Arc<Self>,
        kind: UpgradedConnectionKind,
        device_id: Option<String>,
    ) -> Option<UpgradedConnectionTicket> {
        let mut inner = self.inner.lock();
        if !inner.admitting {
            inner.failed = inner.failed.saturating_add(1);
            log::info!(
                "[upgraded-connections] admission refused kind={:?} session_count={}",
                kind,
                inner.connections.len()
            );
            return None;
        }
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        inner.connections.push(RegisteredConnection {
            id,
            kind,
            device_id,
            cancel: cancel.clone(),
        });
        Some(UpgradedConnectionTicket {
            registry: Arc::clone(self),
            id,
            cancel,
        })
    }

    pub fn cancel(&self, id: Uuid) -> bool {
        let mut inner = self.inner.lock();
        let Some(connection) = inner
            .connections
            .iter()
            .find(|connection| connection.id == id)
        else {
            return false;
        };
        if connection.cancel.is_cancelled() {
            return true;
        }
        connection.cancel.cancel();
        inner.cancelled = inner.cancelled.saturating_add(1);
        true
    }

    pub fn cancel_all(&self) -> u64 {
        let ids: Vec<Uuid> = self
            .inner
            .lock()
            .connections
            .iter()
            .map(|connection| connection.id)
            .collect();
        let mut cancelled = 0u64;
        for id in ids {
            if self.cancel(id) {
                cancelled = cancelled.saturating_add(1);
            }
        }
        cancelled
    }

    pub fn complete(&self, id: Uuid, failed: bool) {
        let mut inner = self.inner.lock();
        if let Some(connection) = inner
            .connections
            .iter()
            .find(|connection| connection.id == id)
        {
            log::info!(
                "[upgraded-connections] complete kind={:?} failed={} session_count={}",
                connection.kind,
                failed,
                inner.connections.len().saturating_sub(1)
            );
        }
        inner.connections.retain(|connection| connection.id != id);
        if failed {
            inner.failed = inner.failed.saturating_add(1);
        }
        drop(inner);
        self.completed.notify_waiters();
    }

    #[must_use]
    pub fn receipt(&self) -> UpgradedConnectionReceipt {
        let inner = self.inner.lock();
        UpgradedConnectionReceipt {
            active: inner.connections.len() as u64,
            failed: inner.failed,
            timed_out: inner.timed_out,
            cancelled: inner.cancelled,
        }
    }

    /// Stop admission, cancel every connection and wait, up to `deadline`, for
    /// each to complete. Connections still open at the deadline stay counted
    /// as active and are added to `timed_out`.
    pub async fn join_all(&self, deadline: Duration) -> UpgradedConnectionReceipt {
        self.stop_admission();
        self.revoke_generations();
        self.cancel_all();
        let drained = async {
            loop {
                let notified = self.completed.notified();
                tokio::pin!(notified);
                // Register before checking, so a completion in between is not missed.
                notified.as_mut().enable();
                if self.inner.lock().connections.is_empty() {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(deadline, drained).await.is_err() {
            let mut inner = self.inner.lock();
            inner.timed_out = inner
                .timed_out
                .saturating_add(inner.connections.len() as u64);
        }
        self.receipt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_cancellation_is_selective_and_idempotent() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let acp = registry
            .admit_device(UpgradedConnectionKind::Acp, "phone-a")
            .unwrap();
        let terminal = registry
            .admit_device(UpgradedConnectionKind::Terminal, "phone-a")
            .unwrap();
        let other = registry
            .admit_device(UpgradedConnectionKind::Acp, "phone-b")
            .unwrap();
        let legacy = registry.admit(UpgradedConnectionKind::Acp).unwrap();
        assert_eq!(registry.device_connection_count("phone-a"), 2);
        assert_eq!(registry.device_counts().get("phone-b"), Some(&1));
        assert_eq!(registry.cancel_device("phone-a"), 2);
        assert_eq!(registry.cancel_device("phone-a"), 0);
        assert!(acp.cancel_token().is_cancelled());
        assert!(terminal.cancel_token().is_cancelled());
        assert!(!other.cancel_token().is_cancelled());
        assert!(!legacy.cancel_token().is_cancelled());
        assert_eq!(registry.device_connection_count("phone-a"), 2);
        drop(acp);
        drop(terminal);
        assert_eq!(registry.device_connection_count("phone-a"), 0);
        assert_eq!(registry.receipt().active, 2);
    }

    #[test]
    fn frame_authentication_tags_existing_admission() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let ticket = registry.admit(UpgradedConnectionKind::Terminal).unwrap();
        assert!(registry.device_counts().is_empty());
        ticket.bind_device("phone");
        assert_eq!(registry.device_connection_count("phone"), 1);
        assert_eq!(registry.cancel_device("phone"), 1);
        assert!(ticket.cancel_token().is_cancelled());
        drop(ticket);
        assert!(registry.device_counts().is_empty());
    }

    /// A connection task that ends when its ticket is cancelled.
    fn cooperative(ticket: UpgradedConnectionTicket) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            ticket.cancel_token().cancelled().await;
            drop(ticket);
        })
    }

    #[tokio::test]
    async fn a_ticket_counts_while_held_and_completes_when_dropped() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let acp = registry
            .admit(UpgradedConnectionKind::Acp)
            .expect("admitted");
        let terminal = registry
            .admit(UpgradedConnectionKind::Terminal)
            .expect("admitted");
        assert_eq!(registry.receipt().active, 2);
        drop(terminal);
        assert_eq!(registry.receipt().active, 1);
        drop(acp);
        assert_eq!(registry.receipt().active, 0);
    }

    #[tokio::test]
    async fn join_all_cancels_and_waits_for_every_connection_to_end() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let tasks = [
            cooperative(registry.admit(UpgradedConnectionKind::Acp).unwrap()),
            cooperative(registry.admit(UpgradedConnectionKind::Terminal).unwrap()),
        ];

        let receipt = registry.join_all(Duration::from_secs(2)).await;

        assert_eq!(receipt.active, 0);
        assert_eq!(receipt.cancelled, 2);
        assert_eq!(receipt.timed_out, 0);
        for task in tasks {
            assert!(
                task.is_finished(),
                "join_all returned before the task ended"
            );
        }
    }

    #[tokio::test]
    async fn a_connection_that_ignores_cancel_is_reported_still_active() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let stuck = registry.admit(UpgradedConnectionKind::Terminal).unwrap();

        let receipt = registry.join_all(Duration::from_millis(50)).await;

        assert_eq!(receipt.active, 1);
        assert_eq!(receipt.timed_out, 1);
        drop(stuck);
        assert_eq!(registry.receipt().active, 0);
    }

    #[tokio::test]
    async fn no_connection_is_admitted_after_shutdown_begins() {
        let registry = Arc::new(UpgradedConnectionRegistry::new());
        let _ = registry.join_all(Duration::from_millis(10)).await;

        assert!(registry.admit(UpgradedConnectionKind::Acp).is_none());
        assert_eq!(registry.receipt().failed, 1);
        assert_eq!(registry.receipt().active, 0);
    }
}
