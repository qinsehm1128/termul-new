//! Platform-aware durable filesystem operations for canonical Conversation storage.
//!
//! New Conversation writers use this module instead of the legacy ACP atomic-file helper so
//! metadata, indexes, layout descriptors, migration journals, and JSONL flush boundaries share
//! one durability policy. Same-directory replacement never truncates the previous target before
//! the replacement bytes are synced, and injected interruptions report only complete old/new
//! namespace states.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Immediate replacement or JSONL flush-boundary durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityLevel {
    Immediate,
    FlushBoundary,
}

/// Directory creation policy. Private Conversation roots are owner-only on Unix; visible
/// SessionWorkspace roots retain the caller/parent permission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryPermissions {
    PrivateOwnerOnly,
    Inherit,
}

/// Namespace state observed when a crash hook interrupts a durable operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NamespaceState {
    OldComplete,
    NewComplete,
}

/// Injectable interruption boundaries used by restart/crash tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashPoint {
    BeforeTempSync,
    AfterTempSync,
    AfterReplace,
    AfterNamespaceSync,
    AfterJsonlAppend,
}

/// Disposition of the uniquely named temp owned by the interrupted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedTempDisposition {
    RetainedForRetry,
    RemovedAfterOwnerCheck,
    None,
}

/// Complete namespace classification returned by normal completion and injected interruption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableWriteOutcome {
    pub namespace_state: NamespaceState,
    pub crash_point: CrashPoint,
    pub owned_temp_disposition: OwnedTempDisposition,
}

/// Test seam for deterministic interruption without process-global fault injection.
pub trait CrashInjector: Send + Sync {
    fn should_interrupt(&self, point: CrashPoint) -> bool;
}

/// Durable filesystem failure with the exact platform step and target retained for boundary logs.
#[derive(Debug)]
pub enum DurableFsError {
    InvalidPath {
        path: PathBuf,
        reason: &'static str,
    },
    SymlinkComponent {
        path: PathBuf,
    },
    InvalidJsonLine {
        path: PathBuf,
        reason: String,
    },
    Io {
        operation: &'static str,
        step: &'static str,
        path: PathBuf,
        source: io::Error,
        outcome: Option<DurableWriteOutcome>,
    },
}

impl fmt::Display for DurableFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { path, reason } => {
                write!(
                    formatter,
                    "invalid durable path '{}': {reason}",
                    path.display()
                )
            }
            Self::SymlinkComponent { path } => write!(
                formatter,
                "durable path contains a symlink or junction component: '{}'",
                path.display()
            ),
            Self::InvalidJsonLine { path, reason } => write!(
                formatter,
                "invalid JSONL record for '{}': {reason}",
                path.display()
            ),
            Self::Io {
                operation,
                step,
                path,
                source,
                ..
            } => write!(
                formatter,
                "durable filesystem {operation} failed at {step} for '{}': {source}",
                path.display()
            ),
        }
    }
}

impl DurableFsError {
    /// Complete namespace classification available when a replacement I/O step fails.
    #[must_use]
    pub fn durable_outcome(&self) -> Option<DurableWriteOutcome> {
        match self {
            Self::Io { outcome, .. } => *outcome,
            Self::InvalidPath { .. }
            | Self::SymlinkComponent { .. }
            | Self::InvalidJsonLine { .. } => None,
        }
    }
}

impl std::error::Error for DurableFsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidPath { .. }
            | Self::SymlinkComponent { .. }
            | Self::InvalidJsonLine { .. } => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, DurableFsError>;

#[cfg(test)]
#[derive(Default)]
struct DurableFsTestState {
    replace_count: AtomicU64,
    catalog_replace_count: AtomicU64,
    fail_catalog_replaces_remaining: AtomicUsize,
}

/// Shared durable filesystem policy with an optional per-instance crash-test hook.
#[derive(Clone, Default)]
pub struct DurableFileSystem {
    crash_injector: Option<Arc<dyn CrashInjector>>,
    #[cfg(test)]
    test_state: Arc<DurableFsTestState>,
}

impl fmt::Debug for DurableFileSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableFileSystem")
            .field("crash_injector", &self.crash_injector.is_some())
            .finish()
    }
}

impl DurableFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_crash_injector(crash_injector: Arc<dyn CrashInjector>) -> Self {
        Self {
            crash_injector: Some(crash_injector),
            #[cfg(test)]
            test_state: Arc::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn reset_replace_counters(&self) {
        self.test_state.replace_count.store(0, Ordering::Release);
        self.test_state
            .catalog_replace_count
            .store(0, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn catalog_replace_count(&self) -> u64 {
        self.test_state
            .catalog_replace_count
            .load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_catalog_replaces(&self, count: usize) {
        self.test_state
            .fail_catalog_replaces_remaining
            .store(count, Ordering::Release);
    }

    /// Replace a file using a same-directory, create-new temp containing the target filename,
    /// process id, and a UUID. Immediate success is reported only after the declared platform
    /// file and namespace durability steps complete.
    pub fn replace_bytes(&self, target: &Path, bytes: &[u8]) -> Result<DurableWriteOutcome> {
        #[cfg(test)]
        {
            self.test_state.replace_count.fetch_add(1, Ordering::AcqRel);
            if target.file_name().and_then(|name| name.to_str()) == Some("catalog.json") {
                self.test_state
                    .catalog_replace_count
                    .fetch_add(1, Ordering::AcqRel);
                if self
                    .test_state
                    .fail_catalog_replaces_remaining
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
                {
                    return Err(self.io_error(
                        "replace_bytes",
                        "injected_cache_write",
                        target,
                        io::Error::other("injected catalog cache-write failure"),
                    ));
                }
            }
        }
        let parent = target.parent().ok_or_else(|| DurableFsError::InvalidPath {
            path: target.to_path_buf(),
            reason: "target has no parent directory",
        })?;
        reject_symlink_components(parent, true)?;
        reject_symlink_components(target, true)?;

        let temp = OwnedTemp::new(target)?;
        let mut file = open_owned_temp(&temp).map_err(|error| {
            self.io_error_with_outcome(
                "replace_bytes",
                "temp_create",
                temp.path(),
                error,
                old_complete(CrashPoint::BeforeTempSync, OwnedTempDisposition::None),
            )
        })?;

        if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
            drop(file);
            let disposition = if remove_owned_temp(&temp).is_ok() {
                OwnedTempDisposition::RemovedAfterOwnerCheck
            } else {
                OwnedTempDisposition::RetainedForRetry
            };
            return Err(self.io_error_with_outcome(
                "replace_bytes",
                "temp_write",
                target,
                error,
                old_complete(CrashPoint::BeforeTempSync, disposition),
            ));
        }

        if self.interrupts(CrashPoint::BeforeTempSync) {
            drop(file);
            remove_owned_temp(&temp).map_err(|error| {
                self.io_error_with_outcome(
                    "replace_bytes",
                    "owned_temp_remove",
                    temp.path(),
                    error,
                    old_complete(
                        CrashPoint::BeforeTempSync,
                        OwnedTempDisposition::RetainedForRetry,
                    ),
                )
            })?;
            return Ok(old_complete(
                CrashPoint::BeforeTempSync,
                OwnedTempDisposition::RemovedAfterOwnerCheck,
            ));
        }

        if let Err(error) = sync_immediate_file(&file) {
            drop(file);
            let disposition = if remove_owned_temp(&temp).is_ok() {
                OwnedTempDisposition::RemovedAfterOwnerCheck
            } else {
                OwnedTempDisposition::RetainedForRetry
            };
            return Err(self.io_error_with_outcome(
                "replace_bytes",
                platform_file_sync_step(),
                target,
                error,
                old_complete(CrashPoint::BeforeTempSync, disposition),
            ));
        }

        if self.interrupts(CrashPoint::AfterTempSync) {
            drop(file);
            return Ok(old_complete(
                CrashPoint::AfterTempSync,
                OwnedTempDisposition::RetainedForRetry,
            ));
        }

        // Windows replacement requires all target/temp handles to be closed before MoveFileExW.
        drop(file);
        if let Err(error) = replace_namespace(temp.path(), target) {
            let disposition = if remove_owned_temp(&temp).is_ok() {
                OwnedTempDisposition::RemovedAfterOwnerCheck
            } else {
                OwnedTempDisposition::RetainedForRetry
            };
            return Err(self.io_error_with_outcome(
                "replace_bytes",
                "rename",
                target,
                error,
                old_complete(CrashPoint::AfterTempSync, disposition),
            ));
        }

        if self.interrupts(CrashPoint::AfterReplace) {
            return Ok(new_complete(CrashPoint::AfterReplace));
        }

        if let Err(error) = sync_parent_namespace(parent) {
            return Err(self.io_error_with_outcome(
                "replace_bytes",
                "namespace_sync",
                target,
                error,
                new_complete(CrashPoint::AfterReplace),
            ));
        }
        let _interrupted = self.interrupts(CrashPoint::AfterNamespaceSync);

        Ok(new_complete(CrashPoint::AfterNamespaceSync))
    }

    /// Create directory components without following symlinks. Newly created private components
    /// use mode `0o700` on Unix; visible workspace components inherit the caller/parent policy.
    pub fn create_dir_durable(&self, path: &Path, permissions: DirectoryPermissions) -> Result<()> {
        if path.as_os_str().is_empty() {
            return Err(DurableFsError::InvalidPath {
                path: path.to_path_buf(),
                reason: "directory path is empty",
            });
        }

        let mut current = PathBuf::new();
        let mut created_any = false;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(DurableFsError::InvalidPath {
                        path: path.to_path_buf(),
                        reason: "parent traversal is not allowed",
                    });
                }
                Component::Normal(part) => {
                    current.push(part);
                    match fs::symlink_metadata(&current) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            return Err(DurableFsError::SymlinkComponent {
                                path: current.clone(),
                            });
                        }
                        Ok(metadata) if metadata.is_dir() => {}
                        Ok(_) => {
                            return Err(DurableFsError::InvalidPath {
                                path: current.clone(),
                                reason: "path component is not a directory",
                            });
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            create_one_directory(&current, permissions).map_err(|error| {
                                self.io_error(
                                    "create_dir_durable",
                                    "directory_create",
                                    &current,
                                    error,
                                )
                            })?;
                            if let Some(parent) = current.parent() {
                                sync_parent_namespace(parent).map_err(|error| {
                                    self.io_error(
                                        "create_dir_durable",
                                        "namespace_sync",
                                        &current,
                                        error,
                                    )
                                })?;
                            }
                            created_any = true;
                        }
                        Err(error) => {
                            return Err(self.io_error(
                                "create_dir_durable",
                                "component_metadata",
                                &current,
                                error,
                            ));
                        }
                    }
                }
            }
        }

        if created_any {
            log::info!(
                "[conversation-fs] durable directory initialized root={}",
                path.display()
            );
        }
        Ok(())
    }

    /// Append exactly one complete, newline-terminated JSON value and flush the userspace buffer.
    /// `sync_file_and_namespace` upgrades the log to Immediate durability at a conversation flush.
    pub fn append_jsonl(&self, target: &Path, json_record: &[u8]) -> Result<DurableWriteOutcome> {
        validate_json_line(target, json_record)?;
        let parent = target.parent().ok_or_else(|| DurableFsError::InvalidPath {
            path: target.to_path_buf(),
            reason: "JSONL target has no parent directory",
        })?;
        reject_symlink_components(parent, true)?;
        reject_symlink_components(target, true)?;

        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(target)
            .map_err(|error| self.io_error("append_jsonl", "append_open", target, error))?;
        file.write_all(json_record)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .map_err(|error| self.io_error("append_jsonl", "append_flush", target, error))?;

        // The hook runs only after a complete line was written and flushed; it can never classify
        // a partial JSON target as complete.
        let _interrupted = self.interrupts(CrashPoint::AfterJsonlAppend);
        Ok(DurableWriteOutcome {
            namespace_state: NamespaceState::NewComplete,
            crash_point: CrashPoint::AfterJsonlAppend,
            owned_temp_disposition: OwnedTempDisposition::None,
        })
    }

    /// Upgrade one file and its containing namespace to the strongest supported platform policy.
    pub fn sync_file_and_namespace(&self, target: &Path) -> Result<()> {
        let parent = target.parent().ok_or_else(|| DurableFsError::InvalidPath {
            path: target.to_path_buf(),
            reason: "sync target has no parent directory",
        })?;
        reject_symlink_components(parent, true)?;
        reject_symlink_components(target, true)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target)
            .map_err(|error| {
                self.io_error("sync_file_and_namespace", "file_open", target, error)
            })?;
        sync_immediate_file(&file).map_err(|error| {
            self.io_error(
                "sync_file_and_namespace",
                platform_file_sync_step(),
                target,
                error,
            )
        })?;
        drop(file);
        sync_parent_namespace(parent).map_err(|error| {
            self.io_error("sync_file_and_namespace", "namespace_sync", target, error)
        })
    }

    fn interrupts(&self, point: CrashPoint) -> bool {
        self.crash_injector
            .as_ref()
            .is_some_and(|injector| injector.should_interrupt(point))
    }

    fn io_error(
        &self,
        operation: &'static str,
        step: &'static str,
        path: &Path,
        source: io::Error,
    ) -> DurableFsError {
        self.io_error_inner(operation, step, path, source, None)
    }

    fn io_error_with_outcome(
        &self,
        operation: &'static str,
        step: &'static str,
        path: &Path,
        source: io::Error,
        outcome: DurableWriteOutcome,
    ) -> DurableFsError {
        self.io_error_inner(operation, step, path, source, Some(outcome))
    }

    fn io_error_inner(
        &self,
        operation: &'static str,
        step: &'static str,
        path: &Path,
        source: io::Error,
        outcome: Option<DurableWriteOutcome>,
    ) -> DurableFsError {
        log::error!(
            "[conversation-fs] durability failure operation={} target={} platform_step={} os_error={}",
            operation,
            path.display(),
            step,
            source
        );
        DurableFsError::Io {
            operation,
            step,
            path: path.to_path_buf(),
            source,
            outcome,
        }
    }
}

fn old_complete(
    crash_point: CrashPoint,
    owned_temp_disposition: OwnedTempDisposition,
) -> DurableWriteOutcome {
    DurableWriteOutcome {
        namespace_state: NamespaceState::OldComplete,
        crash_point,
        owned_temp_disposition,
    }
}

fn new_complete(crash_point: CrashPoint) -> DurableWriteOutcome {
    DurableWriteOutcome {
        namespace_state: NamespaceState::NewComplete,
        crash_point,
        owned_temp_disposition: OwnedTempDisposition::None,
    }
}

/// Convenience wrapper using the production (non-injecting) policy.
pub fn replace_bytes(target: &Path, bytes: &[u8]) -> Result<DurableWriteOutcome> {
    DurableFileSystem::new().replace_bytes(target, bytes)
}

/// Convenience wrapper using the production (non-injecting) policy.
pub fn create_dir_durable(path: &Path, permissions: DirectoryPermissions) -> Result<()> {
    DurableFileSystem::new().create_dir_durable(path, permissions)
}

/// Convenience wrapper using the production (non-injecting) policy.
pub fn append_jsonl(target: &Path, json_record: &[u8]) -> Result<DurableWriteOutcome> {
    DurableFileSystem::new().append_jsonl(target, json_record)
}

/// Convenience wrapper using the production (non-injecting) policy.
pub fn sync_file_and_namespace(target: &Path) -> Result<()> {
    DurableFileSystem::new().sync_file_and_namespace(target)
}

fn validate_json_line(target: &Path, json_record: &[u8]) -> Result<()> {
    if json_record.is_empty() {
        return Err(DurableFsError::InvalidJsonLine {
            path: target.to_path_buf(),
            reason: "record is empty".to_string(),
        });
    }
    if json_record.contains(&b'\n') || json_record.contains(&b'\r') {
        return Err(DurableFsError::InvalidJsonLine {
            path: target.to_path_buf(),
            reason: "record contains a line separator".to_string(),
        });
    }
    serde_json::from_slice::<serde_json::Value>(json_record).map_err(|error| {
        DurableFsError::InvalidJsonLine {
            path: target.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    Ok(())
}

fn reject_symlink_components(path: &Path, include_final: bool) -> Result<()> {
    let mut current = PathBuf::new();
    let components = path.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(DurableFsError::InvalidPath {
                    path: path.to_path_buf(),
                    reason: "parent traversal is not allowed",
                });
            }
        }
        if !include_final && index + 1 == components.len() {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(DurableFsError::SymlinkComponent {
                    path: current.clone(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DurableFsError::Io {
                    operation: "validate_path",
                    step: "component_metadata",
                    path: current.clone(),
                    source: error,
                    outcome: None,
                });
            }
        }
    }
    Ok(())
}

fn create_one_directory(path: &Path, permissions: DirectoryPermissions) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    if permissions == DirectoryPermissions::PrivateOwnerOnly {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let _ = permissions;
    builder.create(path)
}

fn open_owned_temp(temp: &OwnedTemp) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(temp.path())
}

fn sync_immediate_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;

        // F_FULLFSYNC is mandatory for Immediate durability on macOS. ENOTSUP/EINVAL are
        // surfaced to the caller for stable CONVERSATION_DURABILITY_UNSUPPORTED mapping; no
        // silent fallback reports weaker durability as success.
        let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_file_sync_step() -> &'static str {
    "full_sync"
}

#[cfg(not(target_os = "macos"))]
fn platform_file_sync_step() -> &'static str {
    "file_sync"
}

#[cfg(windows)]
fn replace_namespace(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // Bounded Windows guarantee: FlushFileBuffers is provided by File::sync_all above, then the
    // closed-handle move request uses MOVEFILE_WRITE_THROUGH. Windows has no portable parent
    // directory fsync, so restart recovery must accept either complete namespace generation.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_namespace(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(unix)]
fn sync_parent_namespace(parent: &Path) -> io::Result<()> {
    // Linux/macOS: the rename is not complete until the containing namespace is synced.
    File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_namespace(_parent: &Path) -> io::Result<()> {
    // Windows exposes no portable parent-directory fsync. MOVEFILE_WRITE_THROUGH above provides
    // the documented bounded move guarantee; recovery classifies the namespace as old/new complete.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_parent_namespace(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[derive(Debug)]
struct OwnedTemp {
    path: PathBuf,
    expected_file_name: String,
}

impl OwnedTemp {
    fn new(target: &Path) -> Result<Self> {
        let target_name = target
            .file_name()
            .ok_or_else(|| DurableFsError::InvalidPath {
                path: target.to_path_buf(),
                reason: "target has no filename",
            })?;
        let expected_file_name = format!(
            "{}.{}.{}.tmp",
            target_name.to_string_lossy(),
            std::process::id(),
            Uuid::new_v4()
        );
        Ok(Self {
            path: target.with_file_name(&expected_file_name),
            expected_file_name,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn remove_owned_temp(temp: &OwnedTemp) -> io::Result<()> {
    if temp.path.file_name().and_then(|name| name.to_str()) != Some(&temp.expected_file_name) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "temp ownership filename check failed",
        ));
    }
    match fs::symlink_metadata(&temp.path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "temp ownership type check failed",
            ))
        }
        Ok(_) => fs::remove_file(&temp.path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct InterruptAt(CrashPoint);

    impl CrashInjector for InterruptAt {
        fn should_interrupt(&self, point: CrashPoint) -> bool {
            point == self.0
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "termul-conversation-fs-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn injected(point: CrashPoint) -> DurableFileSystem {
        DurableFileSystem::with_crash_injector(Arc::new(InterruptAt(point)))
    }

    #[test]
    fn replacement_uses_same_directory_temp_and_completes_immediate_sync() {
        let root = temp_dir("replace");
        let target = root.join("conversation.json");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();

        let outcome = replace_bytes(&target, br#"{"generation":"new"}"#).unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterNamespaceSync);
        assert_eq!(fs::read(&target).unwrap(), br#"{"generation":"new"}"#);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_before_temp_sync_preserves_old_and_removes_only_owned_temp() {
        let root = temp_dir("before-sync");
        let target = root.join("conversation.json");
        let unrelated = root.join("conversation.json.someone-else.tmp");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();
        fs::write(&unrelated, b"owned by someone else").unwrap();

        let outcome = injected(CrashPoint::BeforeTempSync)
            .replace_bytes(&target, br#"{"generation":"new"}"#)
            .unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::OldComplete);
        assert_eq!(outcome.crash_point, CrashPoint::BeforeTempSync);
        assert_eq!(
            outcome.owned_temp_disposition,
            OwnedTempDisposition::RemovedAfterOwnerCheck
        );
        assert_eq!(fs::read(&target).unwrap(), br#"{"generation":"old"}"#);
        assert_eq!(fs::read(&unrelated).unwrap(), b"owned by someone else");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_after_temp_sync_preserves_old_and_retains_complete_owned_temp() {
        let root = temp_dir("after-temp-sync");
        let target = root.join("conversation.json");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();

        let outcome = injected(CrashPoint::AfterTempSync)
            .replace_bytes(&target, br#"{"generation":"new"}"#)
            .unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::OldComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterTempSync);
        assert_eq!(
            outcome.owned_temp_disposition,
            OwnedTempDisposition::RetainedForRetry
        );
        assert_eq!(fs::read(&target).unwrap(), br#"{"generation":"old"}"#);
        let temps = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert_eq!(temps.len(), 1);
        assert_eq!(
            fs::read(temps[0].path()).unwrap(),
            br#"{"generation":"new"}"#
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_after_replace_exposes_only_complete_new_json() {
        let root = temp_dir("after-replace");
        let target = root.join("conversation.json");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();

        let outcome = injected(CrashPoint::AfterReplace)
            .replace_bytes(&target, br#"{"generation":"new"}"#)
            .unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterReplace);
        assert_eq!(outcome.owned_temp_disposition, OwnedTempDisposition::None);
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(value["generation"], "new");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_after_namespace_sync_reports_complete_new_namespace() {
        let root = temp_dir("after-namespace");
        let target = root.join("conversation.json");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();

        let outcome = injected(CrashPoint::AfterNamespaceSync)
            .replace_bytes(&target, br#"{"generation":"new"}"#)
            .unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterNamespaceSync);
        assert_eq!(fs::read(&target).unwrap(), br#"{"generation":"new"}"#);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn jsonl_append_is_complete_flushed_and_injectable_at_boundary() {
        let root = temp_dir("jsonl");
        let target = root.join("messages.jsonl");
        let outcome = injected(CrashPoint::AfterJsonlAppend)
            .append_jsonl(&target, br#"{"schemaVersion":2,"seq":1}"#)
            .unwrap();
        assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
        assert_eq!(outcome.crash_point, CrashPoint::AfterJsonlAppend);
        assert_eq!(
            fs::read(&target).unwrap(),
            b"{\"schemaVersion\":2,\"seq\":1}\n"
        );
        sync_file_and_namespace(&target).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn jsonl_rejects_empty_multiline_and_invalid_records_without_writing() {
        let root = temp_dir("invalid-jsonl");
        let target = root.join("messages.jsonl");
        for invalid in [
            b"".as_slice(),
            b"{not-json}".as_slice(),
            b"{}\n{}".as_slice(),
        ] {
            assert!(append_jsonl(&target, invalid).is_err());
        }
        assert!(!target.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn private_directory_components_are_owner_only_on_unix() {
        let root = temp_dir("private-dir");
        let nested = root.join("private").join("2026").join("08").join("15");
        create_dir_durable(&nested, DirectoryPermissions::PrivateOwnerOnly).unwrap();
        assert!(nested.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [
                root.join("private"),
                root.join("private").join("2026"),
                root.join("private").join("2026").join("08"),
                nested.clone(),
            ] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn durable_operations_reject_symlink_components() {
        let root = temp_dir("symlink");
        let outside = root.join("outside");
        let link = root.join("link");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let error = replace_bytes(&link.join("conversation.json"), b"{}").unwrap_err();
        assert!(matches!(error, DurableFsError::SymlinkComponent { .. }));
        assert!(!outside.join("conversation.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn outcome_serializes_with_exact_wire_values() {
        let value = serde_json::to_value(DurableWriteOutcome {
            namespace_state: NamespaceState::OldComplete,
            crash_point: CrashPoint::AfterTempSync,
            owned_temp_disposition: OwnedTempDisposition::RetainedForRetry,
        })
        .unwrap();
        assert_eq!(value["namespaceState"], "oldComplete");
        assert_eq!(value["crashPoint"], "after_temp_sync");
        assert_eq!(value["ownedTempDisposition"], "retained_for_retry");
    }
}
