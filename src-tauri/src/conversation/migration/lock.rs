//! Bootstrap-owned host migration lock.
//!
//! The permanent file contains diagnostic owner metadata, but file existence never represents
//! ownership. Exclusivity comes only from the kernel-backed lock held by the guard's open handle,
//! so process termination releases ownership even when `Drop` cannot run.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};

use super::{MigrationError, MigrationErrorCode, Result};

pub const MIGRATION_LOCK_FILE: &str = "conversation-layout-v2.lock";
/// Kernel-backed exclusive lock for migration-maintenance.json load/modify/replace.
/// Process death releases the lock even when `Drop` cannot run.
pub const MIGRATION_CONTROL_LOCK_FILE: &str = "conversation-layout-v2-maintenance.lock";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockOwnerV1 {
    schema_version: u32,
    owner_token: Uuid,
    process_id: u32,
}

#[derive(Debug, Clone)]
pub struct HostMigrationLock {
    canonical_host_root: PathBuf,
    lock_path: PathBuf,
    #[cfg(test)]
    acquire_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl HostMigrationLock {
    pub fn new(host_state_root: &Path) -> Result<Self> {
        let canonical_host_root = host_state_root.canonicalize().map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "prepare_lock",
                format!("host-state root cannot be canonicalized: {error}"),
            )
        })?;
        let migration_dir = canonical_host_root.join("conversation-migrations");
        DurableFileSystem::new()
            .create_dir_durable(&migration_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "prepare_lock",
                    error.to_string(),
                )
            })?;
        Ok(Self {
            canonical_host_root,
            lock_path: migration_dir.join(MIGRATION_LOCK_FILE),
            #[cfg(test)]
            acquire_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    pub fn acquire(&self) -> Result<HostMigrationLockGuard> {
        #[cfg(test)]
        self.acquire_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.lock_path).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "open_lock",
                error.to_string(),
            )
        })?;
        if let Err(error) = FileExt::try_lock_exclusive(&file) {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                log::warn!(
                    "[conversation-migration] active host lock contention pid={} root_digest={}",
                    std::process::id(),
                    host_root_digest(&self.canonical_host_root)
                );
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationInProgress,
                    "acquire_lock",
                    "another process owns the kernel migration lock",
                ));
            }
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "acquire_lock",
                error.to_string(),
            ));
        }

        let owner_token = Uuid::new_v4();
        let owner = LockOwnerV1 {
            schema_version: 1,
            owner_token,
            process_id: std::process::id(),
        };
        let mut bytes = serde_json::to_vec(&owner).map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "write_lock_metadata",
                error.to_string(),
            )
        })?;
        bytes.push(b'\n');
        if let Err(error) = file
            .set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| file.write_all(&bytes))
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
        {
            let _ = FileExt::unlock(&file);
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationDurabilityFailed,
                "write_lock_metadata",
                error.to_string(),
            ));
        }
        log::info!(
            "[conversation-migration] host lock acquired pid={} root_digest={} operation=bootstrap_handoff",
            std::process::id(),
            host_root_digest(&self.canonical_host_root)
        );
        Ok(HostMigrationLockGuard {
            canonical_host_root: self.canonical_host_root.clone(),
            file,
            acquired_at: Instant::now(),
        })
    }

    #[cfg(test)]
    pub(super) fn acquire_count(&self) -> usize {
        self.acquire_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Debug)]
pub struct HostMigrationLockGuard {
    canonical_host_root: PathBuf,
    file: File,
    acquired_at: Instant,
}

impl HostMigrationLockGuard {
    #[must_use]
    pub fn canonical_host_root(&self) -> &Path {
        &self.canonical_host_root
    }

    pub fn validate_host_root(&self, host_state_root: &Path) -> Result<()> {
        let canonical = host_state_root.canonicalize().map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "validate_lock_guard",
                format!("host-state root cannot be canonicalized: {error}"),
            )
        })?;
        if canonical != self.canonical_host_root {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "validate_lock_guard",
                "pre-acquired migration guard belongs to a different canonical host-state root",
            ));
        }
        Ok(())
    }
}

impl Drop for HostMigrationLockGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            log::error!(
                "[conversation-migration] kernel lock release failed pid={} root_digest={} error={}",
                std::process::id(),
                host_root_digest(&self.canonical_host_root),
                error
            );
            return;
        }
        log::info!(
            "[conversation-migration] host lock released pid={} root_digest={} duration_ms={}",
            std::process::id(),
            host_root_digest(&self.canonical_host_root),
            self.acquired_at.elapsed().as_millis()
        );
    }
}

/// Host-migration-lock compatible control lock held across maintenance
/// load/validate/modify/durable-replace. Exclusivity is kernel `fs2` on a
/// permanent file under `conversation-migrations/`.
#[derive(Debug, Clone)]
pub struct MigrationControlLock {
    lock_path: PathBuf,
}

impl MigrationControlLock {
    pub fn new(host_state_root: &Path) -> Result<Self> {
        let canonical_host_root = host_state_root.canonicalize().map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationLockInvalid,
                "prepare_control_lock",
                format!("host-state root cannot be canonicalized: {error}"),
            )
        })?;
        let migration_dir = canonical_host_root.join("conversation-migrations");
        DurableFileSystem::new()
            .create_dir_durable(&migration_dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                MigrationError::new(
                    MigrationErrorCode::MigrationDurabilityFailed,
                    "prepare_control_lock",
                    error.to_string(),
                )
            })?;
        Ok(Self {
            lock_path: migration_dir.join(MIGRATION_CONTROL_LOCK_FILE),
        })
    }

    /// Blocking exclusive lock so concurrent processes serialize rather than
    /// silently last-writer-wins.
    pub fn acquire(&self) -> Result<MigrationControlLockGuard> {
        acquire_control_lock(&self.lock_path, true)
    }

    /// Non-blocking exclusive lock compatible with [`HostMigrationLock::acquire`].
    pub fn try_acquire(&self) -> Result<MigrationControlLockGuard> {
        acquire_control_lock(&self.lock_path, false)
    }
}

#[derive(Debug)]
pub struct MigrationControlLockGuard {
    file: File,
    acquired_at: Instant,
}

impl Drop for MigrationControlLockGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            log::error!(
                "[conversation-migration] control lock release failed pid={} error={}",
                std::process::id(),
                error
            );
            return;
        }
        log::info!(
            "[conversation-migration] control lock released pid={} duration_ms={}",
            std::process::id(),
            self.acquired_at.elapsed().as_millis()
        );
    }
}

fn acquire_control_lock(lock_path: &Path, blocking: bool) -> Result<MigrationControlLockGuard> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(lock_path).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationLockInvalid,
            "open_control_lock",
            error.to_string(),
        )
    })?;
    let lock_result = if blocking {
        FileExt::lock_exclusive(&file)
    } else {
        FileExt::try_lock_exclusive(&file)
    };
    if let Err(error) = lock_result {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            log::warn!(
                "[conversation-migration] control lock contention pid={}",
                std::process::id()
            );
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationInProgress,
                "acquire_control_lock",
                "another process owns the kernel maintenance control lock",
            ));
        }
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationLockInvalid,
            "acquire_control_lock",
            error.to_string(),
        ));
    }
    log::info!(
        "[conversation-migration] control lock acquired pid={}",
        std::process::id()
    );
    Ok(MigrationControlLockGuard {
        file,
        acquired_at: Instant::now(),
    })
}

fn host_root_digest(root: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(root.as_os_str().as_encoded_bytes());
    digest
        .finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_lock_is_exclusive_guard_bound_and_file_is_permanent() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let lock = HostMigrationLock::new(&canonical).unwrap();
        let guard = lock.acquire().unwrap();
        assert!(guard.validate_host_root(&canonical).is_ok());
        assert_eq!(
            lock.acquire().unwrap_err().code,
            MigrationErrorCode::MigrationInProgress
        );
        drop(guard);
        assert!(lock.lock_path.is_file());
        assert!(lock.acquire().is_ok());
    }

    #[test]
    fn stale_metadata_file_without_kernel_owner_does_not_block() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let lock = HostMigrationLock::new(&canonical).unwrap();
        std::fs::write(&lock.lock_path, b"stale diagnostic metadata").unwrap();
        assert!(lock.acquire().is_ok());
    }
}
