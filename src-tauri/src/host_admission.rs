//! Host-wide mutation admission fence.
//!
//! Close admission before producer/catalog/PTY shutdown barriers. Later Tauri
//! mutators and resource creators return [`HOST_SHUTTING_DOWN`]. Tracked
//! command tasks are cancelled and joined under the same absolute deadline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tokio::task::{AbortHandle, JoinHandle};

pub const HOST_SHUTTING_DOWN: &str = "HOST_SHUTTING_DOWN";

enum TrackedCommandTask {
    Owned(JoinHandle<()>),
    Abort(AbortHandle),
}

pub struct HostAdmission {
    open: AtomicBool,
    tasks: Mutex<Vec<TrackedCommandTask>>,
}

impl HostAdmission {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            open: AtomicBool::new(true),
            tasks: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: HostAdmission = HostAdmission::new();
        &GLOBAL
    }

    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        log::warn!(
            target: "termul::host_admission",
            "operation=host_admission stable_code={HOST_SHUTTING_DOWN}"
        );
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), &'static str> {
        if self.is_open() {
            Ok(())
        } else {
            Err(HOST_SHUTTING_DOWN)
        }
    }

    pub fn track(&self, handle: JoinHandle<()>) {
        if self.check().is_err() {
            handle.abort();
            return;
        }
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(TrackedCommandTask::Owned(handle));
    }

    pub fn track_abort(&self, abort: AbortHandle) {
        if self.check().is_err() {
            abort.abort();
            return;
        }
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(TrackedCommandTask::Abort(abort));
    }

    pub async fn drain_until(&self, deadline: tokio::time::Instant) {
        let tasks: Vec<TrackedCommandTask> = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let mut owned = Vec::new();
        for task in tasks {
            match task {
                TrackedCommandTask::Owned(handle) => {
                    handle.abort();
                    owned.push(handle);
                }
                TrackedCommandTask::Abort(abort) => abort.abort(),
            }
        }
        let join = async {
            for handle in owned {
                let _ = handle.await;
            }
        };
        let _ = tokio::time::timeout_at(deadline, join).await;
    }

    #[cfg(test)]
    pub fn reopen_for_tests(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

impl Default for HostAdmission {
    fn default() -> Self {
        Self::new()
    }
}
