//! Exact-layout, read-only inventory for legacy Conversation migration inputs.
//!
//! Only the three historical Se stores are inspected. Project and worktree trees are never
//! traversed, every admitted regular file is hashed through a bounded streaming reader, and the
//! resulting canonical path ordering is the source snapshot used again during verification.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{MigrationError, MigrationErrorCode, Result};
use crate::conversation::durable_fs::DurableFileSystem;
use crate::conversation::locator::{
    metadata_is_link_or_reparse, reject_link_or_reparse_components,
};

pub const LEGACY_INVENTORY_SCHEMA_VERSION: u32 = 1;
pub const INVENTORY_FILE: &str = "inventory-v1.json";
const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySourceKind {
    LegacyHostSessions,
    LegacyChatHistory,
    LegacyWorkspaceManifests,
}

impl LegacySourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyHostSessions => "legacy_host_sessions",
            Self::LegacyChatHistory => "legacy_chat_history",
            Self::LegacyWorkspaceManifests => "legacy_workspace_manifests",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyInventoryFileV1 {
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyInventoryRootV1 {
    pub source_kind: LegacySourceKind,
    pub canonical_path: String,
    pub files: Vec<LegacyInventoryFileV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyInventoryV1 {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub generated_at_utc: String,
    pub roots: Vec<LegacyInventoryRootV1>,
    pub inventory_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRootSpec {
    pub source_kind: LegacySourceKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyRootConfiguration {
    pub host_state_root: PathBuf,
    pub standalone_session_roots: Vec<PathBuf>,
    pub standalone_workspace_manifest_roots: Vec<PathBuf>,
}

impl LegacyRootConfiguration {
    #[must_use]
    pub fn known_roots(&self) -> Vec<LegacyRootSpec> {
        let mut roots = vec![
            LegacyRootSpec {
                source_kind: LegacySourceKind::LegacyHostSessions,
                path: self.host_state_root.join("acp-sessions"),
            },
            LegacyRootSpec {
                source_kind: LegacySourceKind::LegacyChatHistory,
                path: self.host_state_root.join("acp-chat-history"),
            },
            LegacyRootSpec {
                source_kind: LegacySourceKind::LegacyWorkspaceManifests,
                path: self.host_state_root.join("workspace-manifests"),
            },
        ];
        roots.extend(
            self.standalone_session_roots
                .iter()
                .cloned()
                .map(|path| LegacyRootSpec {
                    source_kind: LegacySourceKind::LegacyHostSessions,
                    path,
                }),
        );
        roots.extend(
            self.standalone_workspace_manifest_roots
                .iter()
                .cloned()
                .map(|path| LegacyRootSpec {
                    source_kind: LegacySourceKind::LegacyWorkspaceManifests,
                    path,
                }),
        );
        roots
    }
}

/// Inventory exact known files and persist the pre-migration source snapshot.
pub fn inventory_legacy_roots(
    configuration: &LegacyRootConfiguration,
    operation_id: Uuid,
    generated_at_utc: DateTime<Utc>,
    operation_dir: &Path,
) -> Result<LegacyInventoryV1> {
    let mut roots = Vec::new();
    for spec in configuration.known_roots() {
        match fs::symlink_metadata(&spec.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(inventory_error("stat_inventory_root", &spec.path, error)),
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                return Err(rejected_link(&spec.path))
            }
            Ok(_) => {}
        }
        reject_link_or_reparse_components(&spec.path)
            .map_err(|error| link_error(&spec.path, error))?;
        // All components were checked without following links. Preserve that absolute lexical
        // identity instead of calling `canonicalize`, which would reopen a validation/open race by
        // following a component swapped after the checks above.
        let canonical = absolute_legacy_root(&spec.path)?;
        reject_link_or_reparse_components(&canonical)
            .map_err(|error| link_error(&canonical, error))?;
        if !canonical.is_dir() {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "inventory",
                format!(
                    "known legacy root '{}' is not a directory",
                    canonical.display()
                ),
            ));
        }
        let mut files = match spec.source_kind {
            LegacySourceKind::LegacyHostSessions => inventory_host_sessions(&canonical)?,
            LegacySourceKind::LegacyChatHistory => inventory_chat_history(&canonical)?,
            LegacySourceKind::LegacyWorkspaceManifests => {
                inventory_workspace_manifests(&canonical)?
            }
        };
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        roots.push(LegacyInventoryRootV1 {
            source_kind: spec.source_kind,
            canonical_path: canonical.to_string_lossy().into_owned(),
            files,
        });
    }
    roots.sort_by(|left, right| {
        left.source_kind
            .cmp(&right.source_kind)
            .then_with(|| left.canonical_path.cmp(&right.canonical_path))
    });
    roots.dedup_by(|left, right| {
        left.source_kind == right.source_kind && left.canonical_path == right.canonical_path
    });

    let generated_at_utc = generated_at_utc.to_rfc3339_opts(SecondsFormat::Millis, true);
    let inventory_sha256 = inventory_digest(operation_id, &generated_at_utc, &roots)?;
    let inventory = LegacyInventoryV1 {
        schema_version: LEGACY_INVENTORY_SCHEMA_VERSION,
        operation_id,
        generated_at_utc,
        roots,
        inventory_sha256,
    };
    write_inventory(operation_dir, &inventory)?;

    let file_count: usize = inventory.roots.iter().map(|root| root.files.len()).sum();
    let byte_count: u64 = inventory
        .roots
        .iter()
        .flat_map(|root| root.files.iter())
        .map(|file| file.size)
        .sum();
    log::info!(
        "[conversation-migration] legacy inventory complete operation_id={} root_count={} file_count={} byte_count={} inventory_digest={}",
        operation_id,
        inventory.roots.len(),
        file_count,
        byte_count,
        digest_prefix(&inventory.inventory_sha256)
    );
    Ok(inventory)
}

pub fn load_inventory(operation_dir: &Path) -> Result<LegacyInventoryV1> {
    let path = operation_dir.join(INVENTORY_FILE);
    let bytes = fs::read(&path).map_err(|error| inventory_error("read_inventory", &path, error))?;
    let inventory: LegacyInventoryV1 = serde_json::from_slice(&bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_inventory",
            error.to_string(),
        )
    })?;
    if inventory.schema_version != LEGACY_INVENTORY_SCHEMA_VERSION
        || inventory.inventory_sha256
            != inventory_digest(
                inventory.operation_id,
                &inventory.generated_at_utc,
                &inventory.roots,
            )?
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_inventory",
            "inventory schema or canonical digest is invalid",
        ));
    }
    Ok(inventory)
}

pub fn hash_file_streaming(path: &Path) -> Result<(u64, String)> {
    let root = path.parent().ok_or_else(|| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "hash_legacy_source",
            "legacy source has no parent directory",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "hash_legacy_source",
            "legacy source has no final component",
        )
    })?;
    hash_legacy_file_no_follow(root, Path::new(name))
}

pub(crate) fn hash_legacy_file_no_follow(root: &Path, relative: &Path) -> Result<(u64, String)> {
    let file = open_legacy_file_no_follow(root, relative)?;
    hash_open_file(file, relative)
}

// Kept as the byte-returning counterpart to the streaming hash path for migration callers that
// must securely inspect payloads; current production inventory only needs the hash variant.
#[allow(dead_code)]
pub(crate) fn read_legacy_file_no_follow(root: &Path, relative: &Path) -> Result<Vec<u8>> {
    let mut file = open_legacy_file_no_follow(root, relative)?;
    let metadata = file
        .metadata()
        .map_err(|error| inventory_error("stat_legacy_source", relative, error))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| inventory_error("read_legacy_source", relative, error))?;
    Ok(bytes)
}

fn hash_open_file(file: File, display_path: &Path) -> Result<(u64, String)> {
    let metadata = file
        .metadata()
        .map_err(|error| inventory_error("stat_legacy_source", display_path, error))?;
    if !metadata.is_file() {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "hash_legacy_source",
            "legacy source is not a regular file",
        ));
    }
    let mut reader = BufReader::with_capacity(HASH_BUFFER_BYTES, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| inventory_error("hash_legacy_source", display_path, error))?;
        if read == 0 {
            break;
        }
        size = size.checked_add(read as u64).ok_or_else(|| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "hash_legacy_source",
                "legacy source size overflow",
            )
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok((size, lower_hex(&hasher.finalize())))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(windows)]
    attributes: u32,
    #[cfg(windows)]
    created: u64,
    #[cfg(windows)]
    modified: u64,
    #[cfg(windows)]
    size: u64,
    #[cfg(not(any(unix, windows)))]
    len: u64,
    #[cfg(not(any(unix, windows)))]
    modified: Option<std::time::SystemTime>,
}

fn source_identity(metadata: &fs::Metadata) -> SourceIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        SourceIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        SourceIdentity {
            attributes: metadata.file_attributes(),
            created: metadata.creation_time(),
            modified: metadata.last_write_time(),
            size: metadata.file_size(),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        SourceIdentity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

fn validate_relative_components(relative: &Path) -> Result<Vec<std::ffi::OsString>> {
    if relative.is_absolute() {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "open_legacy_source",
            "legacy source relative path is absolute",
        ));
    }
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => Ok(value.to_os_string()),
            _ => Err(MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "open_legacy_source",
                "legacy source relative path contains traversal or alias components",
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    if components.is_empty() {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "open_legacy_source",
            "legacy source relative path is empty",
        ));
    }
    Ok(components)
}

fn snapshot_source_components(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<Vec<SourceIdentity>> {
    reject_link_or_reparse_components(root).map_err(|error| link_error(root, error))?;
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| inventory_error("stat_legacy_root", root, error))?;
    if metadata_is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
        return Err(rejected_link(root));
    }
    let mut identities = vec![source_identity(&root_metadata)];
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| inventory_error("stat_legacy_component", &current, error))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(rejected_link(&current));
        }
        let final_component = index + 1 == components.len();
        if final_component && !metadata.is_file() || !final_component && !metadata.is_dir() {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "open_legacy_source",
                "legacy source path changed type during validation",
            ));
        }
        identities.push(source_identity(&metadata));
    }
    Ok(identities)
}

#[cfg(test)]
thread_local! {
    static SECURE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn install_secure_open_hook(hook: impl FnOnce() + 'static) {
    SECURE_OPEN_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

fn run_secure_open_hook() {
    #[cfg(test)]
    SECURE_OPEN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(unix)]
fn open_legacy_file_no_follow(root: &Path, relative: &Path) -> Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let components = validate_relative_components(relative)?;
    let expected = snapshot_source_components(root, &components)?;
    run_secure_open_hook();

    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|_| source_swap(relative))?;
    if source_identity(
        &current
            .metadata()
            .map_err(|error| inventory_error("stat_open_legacy_root", root, error))?,
    ) != expected[0]
    {
        return Err(source_swap(relative));
    }

    for (index, component) in components.iter().enumerate() {
        let component = CString::new(component.as_os_str().as_bytes()).map_err(|_| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "open_legacy_source",
                "legacy source component contains NUL",
            )
        })?;
        let final_component = index + 1 == components.len();
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        if !final_component {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: `current` owns a valid directory fd, `component` is NUL-terminated, and no
        // mode argument is required without O_CREAT. The returned fd is immediately owned by File.
        let fd = unsafe { libc::openat(current.as_raw_fd(), component.as_ptr(), flags) };
        if fd < 0 {
            return Err(source_swap(relative));
        }
        // SAFETY: openat returned a new owned descriptor on success.
        let opened = unsafe { File::from_raw_fd(fd) };
        let metadata = opened
            .metadata()
            .map_err(|error| inventory_error("stat_open_legacy_source", relative, error))?;
        if metadata_is_link_or_reparse(&metadata)
            || final_component && !metadata.is_file()
            || !final_component && !metadata.is_dir()
            || source_identity(&metadata) != expected[index + 1]
        {
            return Err(source_swap(relative));
        }
        current = opened;
    }
    Ok(current)
}

#[cfg(windows)]
fn open_legacy_file_no_follow(root: &Path, relative: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let components = validate_relative_components(relative)?;
    let expected = snapshot_source_components(root, &components)?;
    run_secure_open_hook();
    let path = components
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component));
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|error| inventory_error("open_legacy_source_no_follow", relative, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| inventory_error("stat_open_legacy_source", relative, error))?;
    if metadata_is_link_or_reparse(&metadata)
        || !metadata.is_file()
        || source_identity(&metadata) != *expected.last().expect("final identity")
    {
        return Err(source_swap(relative));
    }
    let after = snapshot_source_components(root, &components)?;
    if after != expected {
        return Err(source_swap(relative));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_legacy_file_no_follow(root: &Path, relative: &Path) -> Result<File> {
    let components = validate_relative_components(relative)?;
    let expected = snapshot_source_components(root, &components)?;
    run_secure_open_hook();
    let path = components
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component));
    let file = OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(|error| inventory_error("open_legacy_source_no_follow", relative, error))?;
    let after = snapshot_source_components(root, &components)?;
    if after != expected
        || source_identity(
            &file
                .metadata()
                .map_err(|error| inventory_error("stat_open_legacy_source", relative, error))?,
        ) != *expected.last().expect("final identity")
    {
        return Err(source_swap(relative));
    }
    Ok(file)
}

fn rejected_link(path: &Path) -> MigrationError {
    log::warn!(
        "[conversation-migration] link/reparse component rejected component={} code=MIGRATION_VERIFICATION_FAILED",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<root>")
    );
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        "open_legacy_source",
        "legacy source contains a symlink, junction, or reparse component",
    )
}

fn link_error(path: &Path, error: impl std::fmt::Display) -> MigrationError {
    log::warn!(
        "[conversation-migration] link/reparse component rejected component={} code=MIGRATION_VERIFICATION_FAILED",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<root>")
    );
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        "open_legacy_source",
        error.to_string(),
    )
}

fn source_swap(relative: &Path) -> MigrationError {
    log::error!(
        "[conversation-migration] source identity changed relative={} code=MIGRATION_SOURCE_CHANGED",
        relative.to_string_lossy().replace('\\', "/")
    );
    MigrationError::new(
        MigrationErrorCode::MigrationSourceChanged,
        "open_legacy_source",
        "legacy source identity changed between validation and open",
    )
}

fn inventory_host_sessions(root: &Path) -> Result<Vec<LegacyInventoryFileV1>> {
    let mut paths = Vec::new();
    admit_exact_file(root, root.join("sessions.json"), &mut paths)?;
    for entry in sorted_directory_entries(root)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| inventory_error("stat_inventory_entry", &path, error))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(rejected_link(&path));
        }
        if !metadata.is_dir() {
            continue;
        }
        let component = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&component).is_err() {
            continue;
        }
        let session_dir = path;
        for name in ["metadata.json", "messages.jsonl", "tool-calls.jsonl"] {
            admit_exact_file(root, session_dir.join(name), &mut paths)?;
        }
    }
    hash_paths(root, paths)
}

fn inventory_chat_history(root: &Path) -> Result<Vec<LegacyInventoryFileV1>> {
    let mut paths = Vec::new();
    for name in ["index.json", "legacy-import.json"] {
        admit_exact_file(root, root.join(name), &mut paths)?;
    }
    let payloads = root.join("payloads");
    match fs::symlink_metadata(&payloads) {
        Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
            return Err(rejected_link(&payloads));
        }
        Ok(metadata) if metadata.is_dir() => {
            for entry in sorted_directory_entries(&payloads)? {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| inventory_error("stat_inventory_entry", &path, error))?;
                if metadata_is_link_or_reparse(&metadata) {
                    return Err(rejected_link(&path));
                }
                if metadata.is_file() && is_canonical_chat_payload_name(&path) {
                    paths.push(path);
                }
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(inventory_error("stat_inventory_entry", &payloads, error)),
    }
    hash_paths(root, paths)
}

fn is_canonical_chat_payload_name(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    path.extension().and_then(|value| value.to_str()) == Some("json")
        && !stem.is_empty()
        && stem.len().is_multiple_of(2)
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn inventory_workspace_manifests(root: &Path) -> Result<Vec<LegacyInventoryFileV1>> {
    let mut paths = Vec::new();
    for entry in sorted_directory_entries(root)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| inventory_error("stat_inventory_entry", &path, error))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(rejected_link(&path));
        }
        if metadata.is_file() && path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    hash_paths(root, paths)
}

fn admit_exact_file(_root: &Path, path: PathBuf, paths: &mut Vec<PathBuf>) -> Result<()> {
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
            return Err(rejected_link(&path));
        }
        Ok(metadata) if metadata.file_type().is_file() => paths.push(path),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(inventory_error("inventory", &path, error)),
    }
    Ok(())
}

fn hash_paths(root: &Path, mut paths: Vec<PathBuf>) -> Result<Vec<LegacyInventoryFileV1>> {
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(root).map_err(|_| {
                MigrationError::new(
                    MigrationErrorCode::MigrationVerificationFailed,
                    "inventory",
                    "known legacy file escaped its root",
                )
            })?;
            let relative_path = relative_path(relative)?;
            let (size, sha256) = hash_legacy_file_no_follow(root, relative)?;
            Ok(LegacyInventoryFileV1 {
                relative_path,
                size,
                sha256,
            })
        })
        .collect()
}

fn sorted_directory_entries(path: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| inventory_error("read_inventory_directory", path, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| inventory_error("read_inventory_directory", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn relative_path(path: &Path) -> Result<String> {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "inventory",
                "legacy relative path is not UTF-8",
            )
        })?;
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "..")
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "inventory",
            "legacy relative path contains an invalid component",
        ));
    }
    Ok(components.join("/"))
}

fn write_inventory(operation_dir: &Path, inventory: &LegacyInventoryV1) -> Result<()> {
    let durable_fs = DurableFileSystem::new();
    durable_fs
        .create_dir_durable(
            operation_dir,
            crate::conversation::durable_fs::DirectoryPermissions::PrivateOwnerOnly,
        )
        .map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationDurabilityFailed,
                "write_inventory",
                error.to_string(),
            )
        })?;
    let mut bytes = serde_json::to_vec_pretty(inventory).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "write_inventory",
            error.to_string(),
        )
    })?;
    bytes.push(b'\n');
    durable_fs
        .replace_bytes(&operation_dir.join(INVENTORY_FILE), &bytes)
        .map_err(|error| {
            MigrationError::new(
                MigrationErrorCode::MigrationDurabilityFailed,
                "write_inventory",
                error.to_string(),
            )
        })?;
    Ok(())
}

fn inventory_digest(
    operation_id: Uuid,
    generated_at_utc: &str,
    roots: &[LegacyInventoryRootV1],
) -> Result<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CanonicalInventory<'a> {
        schema_version: u32,
        operation_id: Uuid,
        generated_at_utc: &'a str,
        roots: &'a [LegacyInventoryRootV1],
    }
    let bytes = serde_json::to_vec(&CanonicalInventory {
        schema_version: LEGACY_INVENTORY_SCHEMA_VERSION,
        operation_id,
        generated_at_utc,
        roots,
    })
    .map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "hash_inventory",
            error.to_string(),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(lower_hex(&hasher.finalize()))
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest_prefix(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn absolute_legacy_root(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| inventory_error("resolve_inventory_root", path, error))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationVerificationFailed,
                    "resolve_inventory_root",
                    "legacy root contains parent traversal",
                ));
            }
        }
    }
    Ok(normalized)
}

fn inventory_error(operation: &'static str, path: &Path, error: io::Error) -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        operation,
        format!(
            "legacy source '{}' could not be read: {error}",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inventories_only_known_files_with_streaming_hashes_and_sorted_paths() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let host = base.join("host");
        let sessions = host.join("acp-sessions");
        let chat = host.join("acp-chat-history");
        let manifests = host.join("workspace-manifests");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(chat.join("payloads")).unwrap();
        fs::create_dir_all(&manifests).unwrap();
        let session_dir = sessions.join("018f7a1c-1b4d-7c8a-9f01-0123456789ab");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("metadata.json"), b"metadata").unwrap();
        fs::write(session_dir.join("messages.jsonl"), b"one\ntwo\n").unwrap();
        fs::write(session_dir.join("tool-calls.jsonl"), b"").unwrap();
        fs::write(sessions.join("ignored.txt"), b"ignore").unwrap();
        fs::create_dir_all(sessions.join("not-a-uuid")).unwrap();
        fs::write(sessions.join("not-a-uuid/metadata.json"), b"ignore").unwrap();
        fs::write(
            chat.join("index.json"),
            json!({"schemaVersion": 1}).to_string(),
        )
        .unwrap();
        fs::write(chat.join("payloads/aa.json"), b"payload").unwrap();
        fs::write(chat.join("payloads/not-hex.json"), b"ignore").unwrap();
        fs::create_dir_all(chat.join("payloads/nested")).unwrap();
        fs::write(chat.join("payloads/nested/ignored.json"), b"ignore").unwrap();
        fs::write(manifests.join("project.json"), b"manifest").unwrap();
        fs::create_dir_all(manifests.join("nested")).unwrap();
        fs::write(manifests.join("nested/ignored.json"), b"ignore").unwrap();

        let operation_id = Uuid::new_v4();
        let inventory = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host,
                ..Default::default()
            },
            operation_id,
            Utc::now(),
            &base.join("operation"),
        )
        .unwrap();
        let paths = inventory
            .roots
            .iter()
            .flat_map(|root| root.files.iter().map(|file| file.relative_path.as_str()))
            .collect::<Vec<_>>();
        assert!(paths.contains(&"018f7a1c-1b4d-7c8a-9f01-0123456789ab/metadata.json"));
        assert!(paths.contains(&"payloads/aa.json"));
        assert!(paths.contains(&"project.json"));
        assert!(!paths.iter().any(|path| path.contains("ignored")));
        assert_eq!(load_inventory(&base.join("operation")).unwrap(), inventory);
    }

    #[test]
    fn streaming_hash_matches_sha256_for_large_source() {
        let temp = tempfile::tempdir().unwrap();
        // macOS exposes the temp root through `/var` -> `/private/var`; the secure-open contract
        // intentionally rejects link-bearing roots, so exercise it with the root's real identity.
        let path = temp.path().canonicalize().unwrap().join("large.jsonl");
        let bytes = vec![0x5a; HASH_BUFFER_BYTES * 3 + 17];
        fs::write(&path, &bytes).unwrap();
        let (size, digest) = hash_file_streaming(&path).unwrap();
        let mut expected = Sha256::new();
        expected.update(&bytes);
        assert_eq!(size, bytes.len() as u64);
        assert_eq!(digest, lower_hex(&expected.finalize()));
    }

    #[cfg(unix)]
    #[test]
    fn inventory_rejects_symlinked_root_payloads_session_uuid_and_final_source() {
        use std::os::unix::fs::symlink;

        for case in ["root", "payloads", "session", "final"] {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let host = base.join("host");
            fs::create_dir_all(&host).unwrap();
            let outside = base.join("outside");
            fs::create_dir_all(&outside).unwrap();
            let mut configuration = LegacyRootConfiguration {
                host_state_root: host.clone(),
                ..Default::default()
            };

            match case {
                "root" => {
                    let linked = base.join("linked-sessions");
                    symlink(&outside, &linked).unwrap();
                    configuration.standalone_session_roots.push(linked);
                }
                "payloads" => {
                    let chat = host.join("acp-chat-history");
                    fs::create_dir_all(&chat).unwrap();
                    fs::write(outside.join("aa.json"), b"outside").unwrap();
                    symlink(&outside, chat.join("payloads")).unwrap();
                }
                "session" => {
                    let sessions = host.join("acp-sessions");
                    fs::create_dir_all(&sessions).unwrap();
                    fs::write(outside.join("metadata.json"), b"outside").unwrap();
                    symlink(
                        &outside,
                        sessions.join("018f7a1c-1b4d-7c8a-9f01-0123456789ab"),
                    )
                    .unwrap();
                }
                "final" => {
                    let manifests = host.join("workspace-manifests");
                    fs::create_dir_all(&manifests).unwrap();
                    fs::write(outside.join("project.json"), b"outside").unwrap();
                    symlink(outside.join("project.json"), manifests.join("project.json")).unwrap();
                }
                _ => unreachable!(),
            }

            let error = inventory_legacy_roots(
                &configuration,
                Uuid::new_v4(),
                Utc::now(),
                &base.join("operation"),
            )
            .unwrap_err();
            assert_eq!(
                error.code,
                MigrationErrorCode::MigrationVerificationFailed,
                "{case} link must fail closed"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn secure_open_detects_intermediate_component_swap() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("sessions");
        let session = root.join("018f7a1c-1b4d-7c8a-9f01-0123456789ab");
        let saved = root.join("saved-session");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&session).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(session.join("metadata.json"), b"trusted").unwrap();
        fs::write(outside.join("metadata.json"), b"outside").unwrap();

        let hook_session = session.clone();
        let hook_saved = saved.clone();
        let hook_outside = outside.clone();
        install_secure_open_hook(move || {
            fs::rename(&hook_session, &hook_saved).unwrap();
            symlink(&hook_outside, &hook_session).unwrap();
        });
        let error = hash_legacy_file_no_follow(
            &root,
            Path::new("018f7a1c-1b4d-7c8a-9f01-0123456789ab/metadata.json"),
        )
        .unwrap_err();
        assert_eq!(error.code, MigrationErrorCode::MigrationSourceChanged);

        fs::remove_file(&session).unwrap();
        fs::rename(saved, session).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn inventory_rejects_windows_junction_root() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let host = base.join("host");
        let outside = base.join("outside");
        let junction = base.join("legacy-junction");
        fs::create_dir_all(&host).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success(), "junction fixture creation failed");

        let error = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host,
                standalone_session_roots: vec![junction.clone()],
                ..Default::default()
            },
            Uuid::new_v4(),
            Utc::now(),
            &base.join("operation"),
        )
        .unwrap_err();
        assert_eq!(error.code, MigrationErrorCode::MigrationVerificationFailed);
        fs::remove_dir(junction).unwrap();
    }
}
