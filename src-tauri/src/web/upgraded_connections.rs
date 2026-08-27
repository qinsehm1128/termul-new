//! Host-owned registry for upgraded ACP and terminal WebSocket connections.
//!
//! TASK-004 registers every upgrade. TASK-005 joins the registry under the
//! host deadline: stop admission, revoke generations, cancel and await, then
//! stop producers and drain.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradedConnectionKind {
    Acp,
    Terminal,
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
    cancel: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
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

    pub fn register(
        &self,
        kind: UpgradedConnectionKind,
        join: Option<JoinHandle<()>>,
    ) -> Option<Uuid> {
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
        inner.connections.push(RegisteredConnection {
            id,
            kind,
            cancel: Arc::new(Notify::new()),
            cancelled: Arc::new(AtomicBool::new(false)),
            join,
        });
        Some(id)
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
        if connection.cancelled.swap(true, Ordering::AcqRel) {
            return true;
        }
        connection.cancel.notify_waiters();
        if let Some(join) = connection.join.as_ref() {
            join.abort();
        }
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
    }

    pub fn mark_timed_out(&self, id: Uuid) {
        let mut inner = self.inner.lock();
        inner.connections.retain(|connection| connection.id != id);
        inner.timed_out = inner.timed_out.saturating_add(1);
    }

    pub fn cancel_token(&self, id: Uuid) -> Option<Arc<Notify>> {
        self.inner
            .lock()
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .map(|connection| Arc::clone(&connection.cancel))
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

    pub async fn join_all(&self, deadline: Duration) -> UpgradedConnectionReceipt {
        self.stop_admission();
        self.revoke_generations();
        self.cancel_all();
        let handles: Vec<JoinHandle<()>> = {
            let mut inner = self.inner.lock();
            inner
                .connections
                .iter_mut()
                .filter_map(|connection| connection.join.take())
                .collect()
        };
        let join = async {
            for handle in handles {
                let _ = handle.await;
            }
        };
        if tokio::time::timeout(deadline, join).await.is_err() {
            let mut inner = self.inner.lock();
            inner.timed_out = inner.timed_out.saturating_add(1);
        }
        let mut inner = self.inner.lock();
        inner.connections.clear();
        UpgradedConnectionReceipt {
            active: 0,
            failed: inner.failed,
            timed_out: inner.timed_out,
            cancelled: inner.cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_tracks_cancel_and_counts_upgraded_connections() {
        let registry = UpgradedConnectionRegistry::new();
        let acp = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        let terminal = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        let acp_id = registry
            .register(UpgradedConnectionKind::Acp, Some(acp))
            .expect("acp admitted");
        let terminal_id = registry
            .register(UpgradedConnectionKind::Terminal, Some(terminal))
            .expect("terminal admitted");
        assert_eq!(registry.receipt().active, 2);
        assert!(registry.cancel(acp_id));
        assert_eq!(registry.receipt().cancelled, 1);
        registry.complete(terminal_id, false);
        let receipt = registry.receipt();
        assert_eq!(receipt.active, 1);
        assert_eq!(receipt.failed, 0);
        registry.stop_admission();
        assert!(registry
            .register(UpgradedConnectionKind::Acp, None)
            .is_none());
        assert_eq!(registry.receipt().failed, 1);
        let joined = registry.join_all(Duration::from_secs(1)).await;
        assert_eq!(joined.active, 0);
        assert!(joined.cancelled >= 1);
    }
}
