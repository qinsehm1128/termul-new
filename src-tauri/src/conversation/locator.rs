//! Strict UTC Conversation path construction and bounded recovery traversal.
//!
//! Canonical paths are derived only from immutable `createdAtUtc`/`CreationPartition` and the
//! Se-owned canonical Conversation UUID. Recovery walks exactly `YYYY/MM/DD/<uuid>`, rejects
//! malformed or symlink/junction components without following them, and yields validated records
//! in deterministic path order.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use crate::conversation::contracts::{
    ConversationId, ConversationRecordV2, CreationPartition, CONVERSATION_SCHEMA_VERSION,
};

const CONVERSATION_METADATA_FILE: &str = "conversation.json";
pub const MAX_DIRECTORY_ENTRIES_PER_LEVEL: usize = 10_000;
pub const MAX_CONVERSATIONS_PER_SCAN: usize = 1_000_000;

/// Validated private Conversation directory and authoritative metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedConversation {
    pub conversation_id: ConversationId,
    pub creation_partition: CreationPartition,
    pub directory: PathBuf,
    pub relative_directory: PathBuf,
    pub record: ConversationRecordV2,
}

/// Deterministic sorted iterator returned by [`bounded_scan`].
#[derive(Debug)]
pub struct BoundedScan {
    records: std::vec::IntoIter<LocatedConversation>,
    rejected_count: usize,
}

impl BoundedScan {
    #[must_use]
    pub fn rejected_count(&self) -> usize {
        self.rejected_count
    }
}

impl Iterator for BoundedScan {
    type Item = LocatedConversation;

    fn next(&mut self) -> Option<Self::Item> {
        self.records.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }
}

impl ExactSizeIterator for BoundedScan {}

/// Locator validation/recovery failure.
#[derive(Debug)]
pub enum LocatorError {
    InvalidRoot {
        path: PathBuf,
        reason: &'static str,
    },
    InvalidRelativePath {
        path: PathBuf,
        reason: String,
    },
    InvalidPartition {
        reason: String,
    },
    SymlinkComponent {
        path: PathBuf,
    },
    BoundExceeded {
        path: PathBuf,
        bound: &'static str,
        limit: usize,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for LocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot { path, reason } => {
                write!(
                    formatter,
                    "invalid locator root '{}': {reason}",
                    path.display()
                )
            }
            Self::InvalidRelativePath { path, reason } => {
                write!(
                    formatter,
                    "invalid canonical relative path '{}': {reason}",
                    path.display()
                )
            }
            Self::InvalidPartition { reason } => {
                write!(formatter, "invalid UTC creation partition: {reason}")
            }
            Self::SymlinkComponent { path } => write!(
                formatter,
                "canonical path contains a symlink or junction component: '{}'",
                path.display()
            ),
            Self::BoundExceeded { path, bound, limit } => write!(
                formatter,
                "bounded scan exceeded {bound}={limit} at '{}'",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "locator I/O failure at '{}': {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for LocatorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidRoot { .. }
            | Self::InvalidRelativePath { .. }
            | Self::InvalidPartition { .. }
            | Self::SymlinkComponent { .. }
            | Self::BoundExceeded { .. } => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, LocatorError>;

/// Canonical private Conversation storage locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationLocator {
    private_root: PathBuf,
}

impl ConversationLocator {
    pub fn new(private_root: PathBuf) -> Result<Self> {
        validate_root(&private_root)?;
        Ok(Self { private_root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.private_root
    }

    /// `<private_root>/<YYYY>/<MM>/<DD>/<lowercase-hyphenated-uuid>`.
    pub fn private_dir(
        &self,
        conversation_id: ConversationId,
        partition: &CreationPartition,
    ) -> Result<PathBuf> {
        validate_partition(partition)?;
        Ok(self
            .private_root
            .join(format!("{:04}", partition.year))
            .join(format!("{:02}", partition.month))
            .join(format!("{:02}", partition.day))
            .join(conversation_id.to_string()))
    }

    /// Parse exactly four relative structural components without canonicalizing through aliases.
    pub fn locate_relative(&self, relative: &Path) -> Result<(ConversationId, CreationPartition)> {
        parse_relative_path(relative)
    }
}

/// Canonical user-visible SessionWorkspace directory locator. It never stores transcript data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorkspaceLocator {
    workspace_root: PathBuf,
}

impl SessionWorkspaceLocator {
    pub fn new(workspace_root: PathBuf) -> Result<Self> {
        validate_root(&workspace_root)?;
        Ok(Self { workspace_root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.workspace_root
    }

    /// `<workspace_root>/sessions/<YYYY>/<MM>/<DD>/<lowercase-hyphenated-uuid>`.
    pub fn workspace_dir(
        &self,
        conversation_id: ConversationId,
        partition: &CreationPartition,
    ) -> Result<PathBuf> {
        validate_partition(partition)?;
        Ok(self
            .workspace_root
            .join("sessions")
            .join(format!("{:04}", partition.year))
            .join(format!("{:02}", partition.month))
            .join(format!("{:02}", partition.day))
            .join(conversation_id.to_string()))
    }
}

/// Scan canonical records with the production bounds and return them in stable relative-path order.
pub fn bounded_scan(locator: &ConversationLocator) -> Result<BoundedScan> {
    scan_with_limits(
        locator,
        ScanLimits {
            entries_per_directory: MAX_DIRECTORY_ENTRIES_PER_LEVEL,
            conversations: MAX_CONVERSATIONS_PER_SCAN,
        },
    )
}

#[derive(Debug, Clone, Copy)]
struct ScanLimits {
    entries_per_directory: usize,
    conversations: usize,
}

fn scan_with_limits(locator: &ConversationLocator, limits: ScanLimits) -> Result<BoundedScan> {
    let started_at = Instant::now();
    let root = locator.root();
    log::debug!("[conversation-locator] scan start root={}", root.display());

    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
            warn_rejected(root, root, "CONVERSATION_SYMLINK_COMPONENT");
            return Err(LocatorError::SymlinkComponent {
                path: root.to_path_buf(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(LocatorError::InvalidRoot {
                path: root.to_path_buf(),
                reason: "root is not a directory",
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            log::debug!(
                "[conversation-locator] scan end root={} accepted=0 rejected=0 duration_ms={}",
                root.display(),
                started_at.elapsed().as_millis()
            );
            return Ok(BoundedScan {
                records: Vec::new().into_iter(),
                rejected_count: 0,
            });
        }
        Err(source) => {
            return Err(LocatorError::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    }

    reject_link_or_reparse_components(root)?;

    let mut rejected_count = 0usize;
    let mut seen_conversations = 0usize;
    let mut located = Vec::new();
    for year_entry in read_sorted_directory(root, limits.entries_per_directory)? {
        let year_path = year_entry.path();
        if !safe_directory_entry(root, &year_entry, &mut rejected_count)? {
            continue;
        }
        let Some(year) = parse_year(&year_entry.file_name()) else {
            rejected_count += 1;
            warn_rejected(root, &year_path, "CONVERSATION_LOCATOR_MALFORMED_YEAR");
            continue;
        };

        for month_entry in read_sorted_directory(&year_path, limits.entries_per_directory)? {
            let month_path = month_entry.path();
            if !safe_directory_entry(root, &month_entry, &mut rejected_count)? {
                continue;
            }
            let Some(month) = parse_month(&month_entry.file_name()) else {
                rejected_count += 1;
                warn_rejected(root, &month_path, "CONVERSATION_LOCATOR_MALFORMED_MONTH");
                continue;
            };

            for day_entry in read_sorted_directory(&month_path, limits.entries_per_directory)? {
                let day_path = day_entry.path();
                if !safe_directory_entry(root, &day_entry, &mut rejected_count)? {
                    continue;
                }
                let Some(day) = parse_day(&day_entry.file_name(), year, month) else {
                    rejected_count += 1;
                    warn_rejected(root, &day_path, "CONVERSATION_LOCATOR_MALFORMED_DAY");
                    continue;
                };
                let partition = CreationPartition::try_new(year, month, day).ok_or_else(|| {
                    LocatorError::InvalidPartition {
                        reason: format!("calendar-invalid {year:04}/{month:02}/{day:02}"),
                    }
                })?;

                for conversation_entry in
                    read_sorted_directory(&day_path, limits.entries_per_directory)?
                {
                    let conversation_path = conversation_entry.path();
                    if !safe_directory_entry(root, &conversation_entry, &mut rejected_count)? {
                        continue;
                    }
                    let Some(component) =
                        conversation_entry.file_name().to_str().map(str::to_owned)
                    else {
                        rejected_count += 1;
                        warn_rejected(
                            root,
                            &conversation_path,
                            "CONVERSATION_LOCATOR_MALFORMED_ID",
                        );
                        continue;
                    };
                    let conversation_id = match ConversationId::parse_path_component(&component) {
                        Ok(id) => id,
                        Err(_) => {
                            rejected_count += 1;
                            warn_rejected(
                                root,
                                &conversation_path,
                                "CONVERSATION_LOCATOR_MALFORMED_ID",
                            );
                            continue;
                        }
                    };

                    seen_conversations += 1;
                    if seen_conversations > limits.conversations {
                        log::warn!(
                            "[conversation-locator] rejected code=CONVERSATION_SCAN_BOUND_EXCEEDED relative={} bound=MAX_CONVERSATIONS_PER_SCAN limit={}",
                            sanitized_relative(root, &conversation_path),
                            limits.conversations
                        );
                        return Err(LocatorError::BoundExceeded {
                            path: conversation_path,
                            bound: "MAX_CONVERSATIONS_PER_SCAN",
                            limit: limits.conversations,
                        });
                    }

                    match load_validated_record(
                        root,
                        &conversation_path,
                        conversation_id,
                        &partition,
                    ) {
                        Ok(record) => {
                            let relative_directory = conversation_path
                                .strip_prefix(root)
                                .map(Path::to_path_buf)
                                .unwrap_or_else(|_| {
                                    PathBuf::from(partition.path.clone())
                                        .join(conversation_id.to_string())
                                });
                            located.push(LocatedConversation {
                                conversation_id,
                                creation_partition: partition.clone(),
                                directory: conversation_path,
                                relative_directory,
                                record,
                            });
                        }
                        Err(error) => {
                            rejected_count += 1;
                            log::warn!(
                                "[conversation-locator] rejected code=CONVERSATION_LOCATOR_RECORD_MISMATCH relative={} reason={}",
                                sanitized_relative(root, &conversation_path),
                                sanitize_reason(&error.to_string())
                            );
                        }
                    }
                }
            }
        }
    }

    located.sort_by(|left, right| left.relative_directory.cmp(&right.relative_directory));
    log::debug!(
        "[conversation-locator] scan end root={} accepted={} rejected={} duration_ms={}",
        root.display(),
        located.len(),
        rejected_count,
        started_at.elapsed().as_millis()
    );
    Ok(BoundedScan {
        records: located.into_iter(),
        rejected_count,
    })
}

fn validate_root(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        return Err(LocatorError::InvalidRoot {
            path: root.to_path_buf(),
            reason: "root must be absolute so cwd changes cannot relocate storage",
        });
    }
    if root
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(LocatorError::InvalidRoot {
            path: root.to_path_buf(),
            reason: "root contains traversal or current-directory components",
        });
    }
    reject_link_or_reparse_components(root)
}

pub(crate) fn reject_link_or_reparse_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(LocatorError::InvalidRoot {
                    path: path.to_path_buf(),
                    reason: "parent traversal is not allowed",
                });
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                return Err(LocatorError::SymlinkComponent {
                    path: current.clone(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(LocatorError::Io {
                    path: current.clone(),
                    source,
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_partition(partition: &CreationPartition) -> Result<()> {
    if !(1..=9999).contains(&partition.year) {
        return Err(LocatorError::InvalidPartition {
            reason: "year must be in 0001-9999".to_string(),
        });
    }
    let Some(validated) =
        CreationPartition::try_new(partition.year, partition.month, partition.day)
    else {
        return Err(LocatorError::InvalidPartition {
            reason: format!(
                "calendar-invalid {:04}/{:02}/{:02}",
                partition.year, partition.month, partition.day
            ),
        });
    };
    if validated.path != partition.path {
        return Err(LocatorError::InvalidPartition {
            reason: format!(
                "path '{}' does not match {:04}/{:02}/{:02}",
                sanitize_reason(&partition.path),
                partition.year,
                partition.month,
                partition.day
            ),
        });
    }
    Ok(())
}

fn parse_relative_path(relative: &Path) -> Result<(ConversationId, CreationPartition)> {
    if relative.is_absolute() {
        return Err(LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "path must be relative".to_string(),
        });
    }
    let mut normal = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => normal.push(value),
            Component::ParentDir => {
                return Err(LocatorError::InvalidRelativePath {
                    path: relative.to_path_buf(),
                    reason: "parent traversal is not allowed".to_string(),
                });
            }
            Component::CurDir => {
                return Err(LocatorError::InvalidRelativePath {
                    path: relative.to_path_buf(),
                    reason: "current-directory components are not allowed".to_string(),
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(LocatorError::InvalidRelativePath {
                    path: relative.to_path_buf(),
                    reason: "absolute/prefixed paths are not allowed".to_string(),
                });
            }
        }
    }
    if normal.len() != 4 {
        return Err(LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: format!("expected exactly four components, found {}", normal.len()),
        });
    }
    if normal
        .iter()
        .any(|component| contains_forbidden_bytes(component))
    {
        return Err(LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "component contains a separator, NUL, or non-UTF-8 bytes".to_string(),
        });
    }

    let year = parse_year(normal[0]).ok_or_else(|| LocatorError::InvalidRelativePath {
        path: relative.to_path_buf(),
        reason: "year must be exactly four ASCII digits in 0001-9999".to_string(),
    })?;
    let month = parse_month(normal[1]).ok_or_else(|| LocatorError::InvalidRelativePath {
        path: relative.to_path_buf(),
        reason: "month must be exactly two ASCII digits in 01-12".to_string(),
    })?;
    let day =
        parse_day(normal[2], year, month).ok_or_else(|| LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "day must be exactly two ASCII digits valid for the UTC month".to_string(),
        })?;
    let id_component = normal[3]
        .to_str()
        .ok_or_else(|| LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "ConversationId is not UTF-8".to_string(),
        })?;
    let conversation_id = ConversationId::parse_path_component(id_component).map_err(|error| {
        LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    let partition = CreationPartition::try_new(year, month, day).ok_or_else(|| {
        LocatorError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "calendar-invalid UTC date".to_string(),
        }
    })?;
    Ok((conversation_id, partition))
}

fn contains_forbidden_bytes(component: &OsStr) -> bool {
    component
        .to_str()
        .is_none_or(|value| value.contains(['/', '\\', '\0']))
}

fn parse_year(component: &OsStr) -> Option<i32> {
    let value = component.to_str()?;
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = value.parse::<i32>().ok()?;
    (1..=9999).contains(&year).then_some(year)
}

fn parse_month(component: &OsStr) -> Option<u32> {
    let value = component.to_str()?;
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let month = value.parse::<u32>().ok()?;
    (1..=12).contains(&month).then_some(month)
}

fn parse_day(component: &OsStr, year: i32, month: u32) -> Option<u32> {
    let value = component.to_str()?;
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day = value.parse::<u32>().ok()?;
    CreationPartition::try_new(year, month, day).map(|partition| partition.day)
}

fn read_sorted_directory(path: &Path, limit: usize) -> Result<Vec<fs::DirEntry>> {
    let mut entries = Vec::new();
    let reader = fs::read_dir(path).map_err(|source| LocatorError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    for entry in reader {
        let entry = entry.map_err(|source| LocatorError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        entries.push(entry);
        if entries.len() > limit {
            log::warn!(
                "[conversation-locator] rejected code=CONVERSATION_SCAN_BOUND_EXCEEDED relative={} bound=MAX_DIRECTORY_ENTRIES_PER_LEVEL limit={}",
                sanitize_component(path.as_os_str()),
                limit
            );
            return Err(LocatorError::BoundExceeded {
                path: path.to_path_buf(),
                bound: "MAX_DIRECTORY_ENTRIES_PER_LEVEL",
                limit,
            });
        }
    }
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn safe_directory_entry(
    root: &Path,
    entry: &fs::DirEntry,
    rejected_count: &mut usize,
) -> Result<bool> {
    let path = entry.path();
    let metadata = fs::symlink_metadata(&path).map_err(|source| LocatorError::Io {
        path: path.clone(),
        source,
    })?;
    if metadata_is_link_or_reparse(&metadata) {
        *rejected_count += 1;
        warn_rejected(root, &path, "CONVERSATION_SYMLINK_COMPONENT");
        return Ok(false);
    }
    if !metadata.is_dir() {
        *rejected_count += 1;
        warn_rejected(root, &path, "CONVERSATION_LOCATOR_UNEXPECTED_ENTRY");
        return Ok(false);
    }
    Ok(true)
}

fn load_validated_record(
    root: &Path,
    directory: &Path,
    conversation_id: ConversationId,
    partition: &CreationPartition,
) -> Result<ConversationRecordV2> {
    let metadata_path = directory.join(CONVERSATION_METADATA_FILE);
    let metadata = fs::symlink_metadata(&metadata_path).map_err(|source| LocatorError::Io {
        path: metadata_path.clone(),
        source,
    })?;
    if metadata_is_link_or_reparse(&metadata) {
        warn_rejected(root, &metadata_path, "CONVERSATION_SYMLINK_COMPONENT");
        return Err(LocatorError::SymlinkComponent {
            path: metadata_path,
        });
    }
    if !metadata.is_file() {
        return Err(LocatorError::InvalidRelativePath {
            path: metadata_path,
            reason: "conversation.json is not a regular file".to_string(),
        });
    }
    let bytes = fs::read(&metadata_path).map_err(|source| LocatorError::Io {
        path: metadata_path.clone(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| LocatorError::InvalidRelativePath {
            path: metadata_path.clone(),
            reason: format!("invalid conversation.json: {error}"),
        })?;
    let canonical_id = conversation_id.to_string();
    if value
        .get("conversationId")
        .and_then(serde_json::Value::as_str)
        != Some(canonical_id.as_str())
    {
        return Err(LocatorError::InvalidRelativePath {
            path: metadata_path.clone(),
            reason: "conversationId is absent, non-canonical, or does not match the directory"
                .to_string(),
        });
    }
    let record: ConversationRecordV2 =
        serde_json::from_value(value).map_err(|error| LocatorError::InvalidRelativePath {
            path: metadata_path.clone(),
            reason: format!("invalid conversation.json: {error}"),
        })?;
    if record.schema_version != CONVERSATION_SCHEMA_VERSION {
        return Err(LocatorError::InvalidRelativePath {
            path: metadata_path,
            reason: format!(
                "schemaVersion {} does not equal {}",
                record.schema_version, CONVERSATION_SCHEMA_VERSION
            ),
        });
    }
    if record.conversation_id != conversation_id {
        return Err(LocatorError::InvalidRelativePath {
            path: directory.to_path_buf(),
            reason: "conversationId does not match directory component".to_string(),
        });
    }
    if record.creation_partition != *partition {
        return Err(LocatorError::InvalidRelativePath {
            path: directory.to_path_buf(),
            reason: "creationPartition does not match directory components".to_string(),
        });
    }
    if CreationPartition::from_created_at(record.created_at_utc) != *partition {
        return Err(LocatorError::InvalidRelativePath {
            path: directory.to_path_buf(),
            reason: "createdAtUtc does not derive the directory partition".to_string(),
        });
    }
    Ok(record)
}

fn warn_rejected(root: &Path, path: &Path, code: &'static str) {
    log::warn!(
        "[conversation-locator] rejected code={} relative={}",
        code,
        sanitized_relative(root, path)
    );
}

fn sanitized_relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(sanitize_component(value)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn sanitize_component(component: &OsStr) -> String {
    let Some(value) = component.to_str() else {
        return "<non-utf8>".to_string();
    };
    let mut sanitized = String::new();
    for character in value.chars().take(96) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    if value.chars().count() > 96 {
        sanitized.push_str("...");
    }
    sanitized
}

fn sanitize_reason(reason: &str) -> String {
    reason
        .chars()
        .take(160)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationCreator, ConversationLifecycleState, ExecutionTarget,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    const CANONICAL_ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const SECOND_ID: &str = "028f7a1c-1b4d-7c8a-9f01-0123456789ab";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TZ_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "se-manager-conversation-locator-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn record(id: &str, created_at: &str, workspace_cwd: &str) -> ConversationRecordV2 {
        let created_at_utc = parse_created_at_utc(created_at).unwrap();
        ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::parse(id).unwrap(),
            created_at_utc,
            creation_partition: CreationPartition::from_created_at(created_at_utc),
            workspace_cwd: workspace_cwd.to_string(),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::Ready,
            last_seq: 0,
            created_by: ConversationCreator::Termul,
            title: None,
            title_source: None,
        }
    }

    fn write_record(root: &Path, record: &ConversationRecordV2) -> PathBuf {
        let locator = ConversationLocator::new(root.to_path_buf()).unwrap();
        let directory = locator
            .private_dir(record.conversation_id, &record.creation_partition)
            .unwrap();
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(CONVERSATION_METADATA_FILE),
            serde_json::to_vec_pretty(record).unwrap(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn private_and_visible_paths_share_immutable_utc_partition_and_uuid() {
        let id = ConversationId::parse(CANONICAL_ID).unwrap();
        let created_at = parse_created_at_utc("2026-08-15T23:59:59.999Z").unwrap();
        let partition = CreationPartition::from_created_at(created_at);
        let private = ConversationLocator::new(PathBuf::from("/private"))
            .unwrap()
            .private_dir(id, &partition)
            .unwrap();
        let visible = SessionWorkspaceLocator::new(PathBuf::from("/visible"))
            .unwrap()
            .workspace_dir(id, &partition)
            .unwrap();
        assert_eq!(
            private,
            PathBuf::from(format!("/private/2026/08/15/{CANONICAL_ID}"))
        );
        assert_eq!(
            visible,
            PathBuf::from(format!("/visible/sessions/2026/08/15/{CANONICAL_ID}"))
        );
    }

    #[test]
    fn non_utc_timezone_does_not_change_midnight_or_dst_partitions() {
        let _guard = TZ_LOCK.lock().unwrap();
        let previous = std::env::var_os("TZ");
        // SAFETY: this test serializes its own TZ mutation and the production locator never reads
        // process-local timezone state; it derives paths exclusively from DateTime<Utc>.
        unsafe { std::env::set_var("TZ", "America/New_York") };
        assert_eq!(std::env::var("TZ").unwrap(), "America/New_York");

        let locator = ConversationLocator::new(PathBuf::from("/private")).unwrap();
        let id = ConversationId::parse(CANONICAL_ID).unwrap();
        for (created_at, expected) in [
            ("2026-03-08T00:30:00.000Z", "2026/03/08"),
            ("2026-03-08T07:30:00.000Z", "2026/03/08"),
            ("2026-11-01T05:30:00.000Z", "2026/11/01"),
            ("2027-01-01T00:15:00.000Z", "2027/01/01"),
        ] {
            let utc = parse_created_at_utc(created_at).unwrap();
            let partition = CreationPartition::from_created_at(utc);
            assert_eq!(partition.path, expected);
            assert!(locator
                .private_dir(id, &partition)
                .unwrap()
                .ends_with(format!("{expected}/{CANONICAL_ID}")));
        }

        // SAFETY: restore the exact prior process value before releasing the test mutex.
        unsafe {
            if let Some(value) = previous {
                std::env::set_var("TZ", value);
            } else {
                std::env::remove_var("TZ");
            }
        }
    }

    #[test]
    fn relative_parser_rejects_aliases_traversal_separators_dates_and_depth() {
        let locator = ConversationLocator::new(PathBuf::from("/private")).unwrap();
        for invalid in [
            format!("2026/08/15/{}", CANONICAL_ID.to_ascii_uppercase()),
            format!("2026/08/15/../{CANONICAL_ID}"),
            format!("2026/08/15/{CANONICAL_ID}/extra"),
            format!("2026/08/15\\{CANONICAL_ID}"),
            format!("2026/02/30/{CANONICAL_ID}"),
            format!("2026/8/15/{CANONICAL_ID}"),
            format!("0000/01/01/{CANONICAL_ID}"),
        ] {
            assert!(
                locator.locate_relative(Path::new(&invalid)).is_err(),
                "must reject {invalid}"
            );
        }
        let (id, partition) = locator
            .locate_relative(Path::new(&format!("2028/02/29/{CANONICAL_ID}")))
            .unwrap();
        assert_eq!(id.to_string(), CANONICAL_ID);
        assert_eq!(partition.path, "2028/02/29");
    }

    #[test]
    fn path_construction_rejects_inconsistent_or_invalid_partition() {
        let locator = ConversationLocator::new(PathBuf::from("/private")).unwrap();
        let id = ConversationId::parse(CANONICAL_ID).unwrap();
        for partition in [
            CreationPartition {
                year: 2026,
                month: 2,
                day: 30,
                path: "2026/02/30".to_string(),
            },
            CreationPartition {
                year: 2026,
                month: 8,
                day: 15,
                path: "2026/08/14".to_string(),
            },
            CreationPartition {
                year: 0,
                month: 1,
                day: 1,
                path: "0000/01/01".to_string(),
            },
        ] {
            assert!(locator.private_dir(id, &partition).is_err());
        }
    }

    #[test]
    fn bounded_scan_is_sorted_and_validates_record_identity_and_partition() {
        let root = temp_dir("sorted");
        let first = record(
            CANONICAL_ID,
            "2026-08-15T09:45:15.123Z",
            "/visible/sessions/2026/08/15/first",
        );
        let second = record(
            SECOND_ID,
            "2025-12-31T23:59:59.999Z",
            "/visible/sessions/2025/12/31/second",
        );
        write_record(&root, &first);
        write_record(&root, &second);

        let mismatched = record(
            SECOND_ID,
            "2026-08-15T09:45:15.123Z",
            "/visible/sessions/2026/08/15/mismatch",
        );
        let wrong_directory = ConversationLocator::new(root.clone())
            .unwrap()
            .private_dir(first.conversation_id, &first.creation_partition)
            .unwrap()
            .with_file_name("038f7a1c-1b4d-7c8a-9f01-0123456789ab");
        fs::create_dir_all(&wrong_directory).unwrap();
        fs::write(
            wrong_directory.join(CONVERSATION_METADATA_FILE),
            serde_json::to_vec(&mismatched).unwrap(),
        )
        .unwrap();

        let scan = bounded_scan(&ConversationLocator::new(root.clone()).unwrap()).unwrap();
        assert_eq!(scan.rejected_count(), 1);
        let paths = scan
            .map(|located| located.relative_directory)
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                PathBuf::from(format!("2025/12/31/{SECOND_ID}")),
                PathBuf::from(format!("2026/08/15/{CANONICAL_ID}")),
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_rejects_noncanonical_id_repeated_inside_conversation_json() {
        let root = temp_dir("record-uppercase-id");
        let value = record(
            CANONICAL_ID,
            "2026-08-15T09:45:15.123Z",
            "/visible/sessions/2026/08/15/test",
        );
        let directory = write_record(&root, &value);
        let metadata_path = directory.join(CONVERSATION_METADATA_FILE);
        let mut json = serde_json::to_value(value).unwrap();
        json["conversationId"] = serde_json::Value::String(CANONICAL_ID.to_ascii_uppercase());
        fs::write(metadata_path, serde_json::to_vec(&json).unwrap()).unwrap();

        let scan = bounded_scan(&ConversationLocator::new(root.clone()).unwrap()).unwrap();
        assert_eq!(scan.rejected_count(), 1);
        assert_eq!(scan.count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_rejects_uppercase_invalid_calendar_and_unexpected_nesting() {
        let root = temp_dir("malformed");
        let uppercase = root
            .join("2026")
            .join("08")
            .join("15")
            .join(CANONICAL_ID.to_ascii_uppercase());
        let invalid_day = root.join("2026").join("02").join("30").join(CANONICAL_ID);
        let nested = root
            .join("2026")
            .join("08")
            .join("16")
            .join("extra")
            .join(CANONICAL_ID);
        fs::create_dir_all(uppercase).unwrap();
        fs::create_dir_all(invalid_day).unwrap();
        fs::create_dir_all(nested).unwrap();

        let scan = bounded_scan(&ConversationLocator::new(root.clone()).unwrap()).unwrap();
        assert_eq!(scan.count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scan_rejects_symlinked_year_month_day_and_uuid_components() {
        for level in ["year", "month", "day", "uuid"] {
            let root = temp_dir(level);
            let outside = temp_dir(&format!("outside-{level}"));
            let year = root.join("2026");
            let month = year.join("08");
            let day = month.join("15");
            match level {
                "year" => std::os::unix::fs::symlink(&outside, &year).unwrap(),
                "month" => {
                    fs::create_dir(&year).unwrap();
                    std::os::unix::fs::symlink(&outside, &month).unwrap();
                }
                "day" => {
                    fs::create_dir_all(&month).unwrap();
                    std::os::unix::fs::symlink(&outside, &day).unwrap();
                }
                "uuid" => {
                    fs::create_dir_all(&day).unwrap();
                    std::os::unix::fs::symlink(&outside, day.join(CANONICAL_ID)).unwrap();
                }
                _ => unreachable!(),
            }
            let scan = bounded_scan(&ConversationLocator::new(root.clone()).unwrap()).unwrap();
            assert_eq!(scan.count(), 0, "symlinked {level} must not be followed");
            assert!(fs::read_dir(&outside).unwrap().next().is_none());
            let _ = fs::remove_dir_all(root);
            let _ = fs::remove_dir_all(outside);
        }
    }

    #[test]
    fn scan_fails_when_directory_entry_bound_is_exceeded() {
        let root = temp_dir("entry-bound");
        fs::create_dir(root.join("2025")).unwrap();
        fs::create_dir(root.join("2026")).unwrap();
        let error = scan_with_limits(
            &ConversationLocator::new(root.clone()).unwrap(),
            ScanLimits {
                entries_per_directory: 1,
                conversations: MAX_CONVERSATIONS_PER_SCAN,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            LocatorError::BoundExceeded {
                bound: "MAX_DIRECTORY_ENTRIES_PER_LEVEL",
                limit: 1,
                ..
            }
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_fails_when_conversation_bound_is_exceeded() {
        let root = temp_dir("conversation-bound");
        for id in [CANONICAL_ID, SECOND_ID] {
            let value = record(
                id,
                "2026-08-15T09:45:15.123Z",
                "/visible/sessions/2026/08/15/test",
            );
            write_record(&root, &value);
        }
        let error = scan_with_limits(
            &ConversationLocator::new(root.clone()).unwrap(),
            ScanLimits {
                entries_per_directory: MAX_DIRECTORY_ENTRIES_PER_LEVEL,
                conversations: 1,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            LocatorError::BoundExceeded {
                bound: "MAX_CONVERSATIONS_PER_SCAN",
                limit: 1,
                ..
            }
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn relative_root_is_rejected_to_prevent_cwd_dependent_storage() {
        assert!(matches!(
            ConversationLocator::new(PathBuf::from("relative/root")),
            Err(LocatorError::InvalidRoot { .. })
        ));
        assert!(matches!(
            SessionWorkspaceLocator::new(PathBuf::from("relative/root")),
            Err(LocatorError::InvalidRoot { .. })
        ));
    }
}
