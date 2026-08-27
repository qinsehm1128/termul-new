//! Deterministic disposable Conversation catalog construction.
//!
//! `catalog.json` is never an authority. Rebuild scans canonical dated directories through the
//! shared locator, validates every authoritative JSON/JSONL file, and sorts entries by
//! ConversationId. Its timestamp is derived only from accepted canonical timestamps, so deleting,
//! corrupting, or replacing the cache cannot change the rebuilt bytes.

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::contracts::{
    format_created_at_utc, ConversationErrorCode, ConversationId, ConversationLifecycleState,
    ConversationRecordV2, ConversationTitleSource, CreationPartition, CONVERSATION_SCHEMA_VERSION,
};
use crate::conversation::durable_fs::{DurableFileSystem, DurableFsError};
use crate::conversation::event_log::{
    scan_event_log, ConversationFrontier, EventLogRepairWarning, EventLogScan,
};
use crate::conversation::locator::{
    bounded_scan, ConversationLocator, LocatorError, MAX_CONVERSATIONS_PER_SCAN,
    MAX_DIRECTORY_ENTRIES_PER_LEVEL,
};

pub const CATALOG_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_FILE: &str = "catalog.json";
pub const CONVERSATION_METADATA_FILE: &str = "conversation.json";
pub const PROVENANCE_FILE: &str = "provenance.json";
pub const EMPTY_CATALOG_GENERATED_AT_UTC: &str = "1970-01-01T00:00:00.000Z";
pub const PROVENANCE_SCHEMA_VERSION: u32 = 1;
/// Immutable catalog chunk size used by mutation admission and frozen generations.
pub const CATALOG_CHUNK_ENTRIES: usize = 1_024;

/// Generation-tagged acknowledgement from a catalog.json replace.
///
/// A stale generation is refused when a newer generation has already been
/// committed. `committed` is false in that case and `committed_generation`
/// reports the generation that remains on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogReplaceAck {
    pub requested_generation: u64,
    pub committed_generation: u64,
    pub committed: bool,
}

/// Serializes catalog.json replacements so a timed-out older replace cannot
/// overwrite a newer flushed generation.
#[derive(Debug)]
pub struct CatalogReplaceFence {
    last_written_generation: AtomicU64,
    write_lock: std::sync::Mutex<()>,
    #[cfg(test)]
    stall: std::sync::Mutex<Option<CatalogReplaceStall>>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct CatalogReplaceStall {
    generation: u64,
    released: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    entered: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl Default for CatalogReplaceFence {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogReplaceFence {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_written_generation: AtomicU64::new(0),
            write_lock: std::sync::Mutex::new(()),
            #[cfg(test)]
            stall: std::sync::Mutex::new(None),
        }
    }

    #[must_use]
    pub fn last_written_generation(&self) -> u64 {
        self.last_written_generation.load(Ordering::Acquire)
    }

    /// Replace `catalog.json` only when `generation` is strictly newer than the
    /// last committed generation. The serializer lock is not held across a test
    /// stall so a newer generation can commit first and fence the stale write.
    pub fn replace_generation(
        &self,
        durable_fs: &DurableFileSystem,
        path: &Path,
        generation: u64,
        bytes: &[u8],
    ) -> std::result::Result<CatalogReplaceAck, DurableFsError> {
        {
            let _guard = self
                .write_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let last = self.last_written_generation.load(Ordering::Acquire);
            if generation <= last {
                return Ok(CatalogReplaceAck {
                    requested_generation: generation,
                    committed_generation: last,
                    committed: false,
                });
            }
        }

        #[cfg(test)]
        self.wait_if_stalled(generation);

        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let last = self.last_written_generation.load(Ordering::Acquire);
        if generation <= last {
            return Ok(CatalogReplaceAck {
                requested_generation: generation,
                committed_generation: last,
                committed: false,
            });
        }
        durable_fs.replace_bytes(path, bytes)?;
        self.last_written_generation
            .store(generation, Ordering::Release);
        Ok(CatalogReplaceAck {
            requested_generation: generation,
            committed_generation: generation,
            committed: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn stall_generation(&self, generation: u64) {
        let released = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let entered = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        *self
            .stall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CatalogReplaceStall {
            generation,
            released,
            entered,
        });
    }

    #[cfg(test)]
    pub(crate) fn release_stall(&self) {
        let stall = self
            .stall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(stall) = stall {
            let (lock, condvar) = &*stall.released;
            let mut released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            *released = true;
            condvar.notify_all();
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_until_stalled(&self, timeout: std::time::Duration) -> bool {
        let stall = self
            .stall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(stall) = stall else {
            return false;
        };
        let (lock, condvar) = &*stall.entered;
        let started = std::time::Instant::now();
        let mut entered = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*entered {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return false;
            }
            let (guard, wait_result) = condvar
                .wait_timeout(entered, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            entered = guard;
            if wait_result.timed_out() && !*entered {
                return false;
            }
        }
        true
    }

    #[cfg(test)]
    fn wait_if_stalled(&self, generation: u64) {
        let stall = self
            .stall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(stall) = stall else {
            return;
        };
        if stall.generation != generation {
            return;
        }
        {
            let (lock, condvar) = &*stall.entered;
            let mut entered = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            *entered = true;
            condvar.notify_all();
        }
        let (lock, condvar) = &*stall.released;
        let mut released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*released {
            released = condvar
                .wait(released)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogEntryV1 {
    pub conversation_id: ConversationId,
    pub created_at_utc: String,
    pub creation_partition: String,
    pub workspace_cwd: String,
    pub project_id: Option<String>,
    pub lifecycle_state: ConversationLifecycleState,
    pub title: Option<String>,
    pub title_source: Option<ConversationTitleSource>,
    pub last_activity_at_utc: String,
    pub message_count: u64,
    pub tool_count: u64,
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogFileV1 {
    pub schema_version: u32,
    pub generated_at_utc: String,
    pub conversations: Vec<ConversationCatalogEntryV1>,
}

impl ConversationCatalogFileV1 {
    #[must_use]
    pub fn deterministic_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .expect("ConversationCatalogFileV1 contains only serializable fields");
        bytes.push(b'\n');
        bytes
    }
}

/// Per-admission copy accounting. A 50,000-entry catalog references at most 49 frozen chunk
/// pointers and clones at most one 1024-entry dirty chunk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatalogAdmissionMetrics {
    pub cloned_chunk_pointers: usize,
    pub cloned_dirty_entries: usize,
    pub serialized_bytes_under_lock: usize,
}

/// Frozen immutable catalog generation captured under the global state lock. Serialization is a
/// separate operation and therefore always runs after the lock is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationCatalogGeneration {
    pub generation: u64,
    pub upsert_count: u64,
    pub entry_count: usize,
    generated_at_utc: Arc<str>,
    chunks: Arc<Vec<Arc<Vec<ConversationCatalogEntryV1>>>>,
}

impl ConversationCatalogGeneration {
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    #[must_use]
    pub fn deterministic_file(&self) -> ConversationCatalogFileV1 {
        ConversationCatalogFileV1 {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_utc: self.generated_at_utc.to_string(),
            conversations: self
                .chunks
                .iter()
                .flat_map(|chunk| chunk.iter().cloned())
                .collect(),
        }
    }

    #[must_use]
    pub fn deterministic_bytes(&self) -> Vec<u8> {
        self.deterministic_file().deterministic_bytes()
    }

    #[must_use]
    pub fn snapshot(&self) -> ConversationCatalogSnapshot {
        ConversationCatalogSnapshot {
            generation: self.generation,
            upsert_count: self.upsert_count,
            entry_count: self.entry_count,
            chunk_count: self.chunk_count(),
            bytes: self.deterministic_bytes(),
        }
    }
}

/// Stable immutable view handed to compatibility callers and tests. Production flush capture uses
/// [`ConversationCatalogGeneration`] and serializes only after releasing the global lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationCatalogSnapshot {
    pub generation: u64,
    pub upsert_count: u64,
    pub entry_count: usize,
    pub chunk_count: usize,
    pub bytes: Vec<u8>,
}

/// In-memory disposable cache updated from validated Conversation frontiers.
#[derive(Debug, Clone)]
pub struct ConversationCatalog {
    generated_at_utc: Arc<str>,
    chunks: Arc<Vec<Arc<Vec<ConversationCatalogEntryV1>>>>,
    generation: u64,
    upsert_count: u64,
    last_admission_metrics: CatalogAdmissionMetrics,
}

impl ConversationCatalog {
    #[must_use]
    pub fn from_file(mut file: ConversationCatalogFileV1) -> Self {
        file.conversations
            .sort_by_key(|entry| entry.conversation_id.to_string());
        let chunks = file
            .conversations
            .chunks(CATALOG_CHUNK_ENTRIES)
            .map(|chunk| Arc::new(chunk.to_vec()))
            .collect();
        Self {
            generated_at_utc: Arc::from(file.generated_at_utc),
            chunks: Arc::new(chunks),
            generation: 0,
            upsert_count: 0,
            last_admission_metrics: CatalogAdmissionMetrics::default(),
        }
    }

    /// Insert or replace exactly one canonical entry and advance the local dirty generation.
    pub fn upsert(
        &mut self,
        record: &ConversationRecordV2,
        frontier: &ConversationFrontier,
    ) -> u64 {
        let entry = entry_from_frontier(record, frontier);
        self.generated_at_utc = Arc::from(
            self.generated_at_utc
                .as_ref()
                .max(entry.created_at_utc.as_str())
                .max(entry.last_activity_at_utc.as_str()),
        );
        let target = record.conversation_id.to_string();
        let chunk_index = self
            .chunks
            .iter()
            .position(|chunk| {
                chunk
                    .last()
                    .is_some_and(|candidate| candidate.conversation_id.to_string() >= target)
            })
            .unwrap_or_else(|| self.chunks.len().saturating_sub(1));
        let cloned_chunk_pointers = if Arc::strong_count(&self.chunks) > 1 {
            self.chunks.len()
        } else {
            0
        };
        let chunks = Arc::make_mut(&mut self.chunks);
        if chunks.is_empty() {
            chunks.push(Arc::new(vec![entry]));
            self.last_admission_metrics = CatalogAdmissionMetrics {
                cloned_chunk_pointers,
                cloned_dirty_entries: 0,
                serialized_bytes_under_lock: 0,
            };
        } else {
            let cloned_dirty_entries = if Arc::strong_count(&chunks[chunk_index]) > 1 {
                chunks[chunk_index].len()
            } else {
                0
            };
            let chunk = Arc::make_mut(&mut chunks[chunk_index]);
            match chunk
                .binary_search_by_key(&target, |candidate| candidate.conversation_id.to_string())
            {
                Ok(index) => chunk[index] = entry,
                Err(index) => chunk.insert(index, entry),
            }
            if chunk.len() > CATALOG_CHUNK_ENTRIES {
                let right = Arc::new(chunk.split_off(chunk.len() / 2));
                chunks.insert(chunk_index + 1, right);
            }
            self.last_admission_metrics = CatalogAdmissionMetrics {
                cloned_chunk_pointers,
                cloned_dirty_entries,
                serialized_bytes_under_lock: 0,
            };
        }
        self.generation = self.generation.saturating_add(1);
        self.upsert_count = self.upsert_count.saturating_add(1);
        self.generation
    }

    /// Drop a Conversation from the disposable catalog cache after a real delete.
    pub fn remove(&mut self, conversation_id: ConversationId) -> u64 {
        let target = conversation_id.to_string();
        let chunks = Arc::make_mut(&mut self.chunks);
        let mut removed = false;
        for chunk_arc in chunks.iter_mut() {
            let chunk = Arc::make_mut(chunk_arc);
            if let Ok(index) = chunk
                .binary_search_by_key(&target, |candidate| candidate.conversation_id.to_string())
            {
                chunk.remove(index);
                removed = true;
                break;
            }
        }
        if !removed {
            return self.generation;
        }
        chunks.retain(|chunk| !chunk.is_empty());
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Capture one immutable generation without serializing or flattening entries.
    #[must_use]
    pub fn capture(&self) -> ConversationCatalogGeneration {
        ConversationCatalogGeneration {
            generation: self.generation,
            upsert_count: self.upsert_count,
            entry_count: self.len(),
            generated_at_utc: Arc::clone(&self.generated_at_utc),
            chunks: Arc::clone(&self.chunks),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ConversationCatalogSnapshot {
        self.capture().snapshot()
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn upsert_count(&self) -> u64 {
        self.upsert_count
    }

    #[must_use]
    pub const fn last_admission_metrics(&self) -> CatalogAdmissionMetrics {
        self.last_admission_metrics
    }

    #[must_use]
    pub fn deterministic_bytes(&self) -> Vec<u8> {
        self.capture().deterministic_bytes()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.len()).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|chunk| chunk.is_empty())
    }
}

#[cfg(test)]
thread_local! {
    static CATALOG_SCAN_COUNT: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_catalog_scan_counter() {
    CATALOG_SCAN_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn catalog_scan_count() -> u64 {
    CATALOG_SCAN_COUNT.get()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationProvenanceFileV1 {
    pub schema_version: u32,
    pub migration_id: String,
    pub source_records: Vec<ConversationProvenanceSourceV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationProvenanceSourceV1 {
    pub source_kind: String,
    pub relative_path: String,
    pub sha256: String,
    pub preserved_read_only: bool,
}

impl ConversationProvenanceFileV1 {
    pub fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.schema_version != PROVENANCE_SCHEMA_VERSION || self.migration_id.trim().is_empty() {
            return Err("invalid provenance schemaVersion or migrationId");
        }
        if self.source_records.iter().any(|source| {
            source.source_kind.trim().is_empty()
                || !valid_relative_source_path(&source.relative_path)
                || !valid_sha256(&source.sha256)
                || !source.preserved_read_only
        }) {
            return Err("provenance contains an invalid source record");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AcceptedCanonicalConversation {
    pub directory: PathBuf,
    pub record: ConversationRecordV2,
    pub scan: EventLogScan,
    pub provenance: Option<ConversationProvenanceFileV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRecoveryIssue {
    pub code: ConversationErrorCode,
    pub conversation_id: Option<ConversationId>,
    pub relative_path: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct CatalogRebuildResult {
    pub catalog: ConversationCatalogFileV1,
    pub accepted: Vec<AcceptedCanonicalConversation>,
    pub recovery_issues: Vec<CatalogRecoveryIssue>,
    pub repairs: Vec<EventLogRepairWarning>,
}

#[derive(Debug)]
pub enum CatalogError {
    Locator(LocatorError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    ScanBound {
        path: PathBuf,
        limit: usize,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locator(error) => write!(formatter, "catalog locator failure: {error}"),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "catalog scan I/O failure at '{}': {source}",
                    path.display()
                )
            }
            Self::ScanBound { path, limit } => write!(
                formatter,
                "catalog recovery scan exceeded limit {limit} at '{}'",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<LocatorError> for CatalogError {
    fn from(value: LocatorError) -> Self {
        Self::Locator(value)
    }
}

pub type Result<T> = std::result::Result<T, CatalogError>;

/// Rebuild a catalog exclusively from canonical metadata and validated event streams.
///
/// The returned value is not persisted by this function. Repository open and mutations write the
/// deterministic bytes last, so a cache failure never rolls back authoritative files.
pub fn rebuild_catalog(
    locator: &ConversationLocator,
    durable_fs: &DurableFileSystem,
) -> Result<CatalogRebuildResult> {
    #[cfg(test)]
    CATALOG_SCAN_COUNT.set(CATALOG_SCAN_COUNT.get() + 1);
    let scan = bounded_scan(locator)?;
    let located = scan.collect::<Vec<_>>();
    let accepted_metadata = located
        .iter()
        .map(|entry| entry.relative_directory.clone())
        .collect::<HashSet<_>>();
    let mut recovery_issues = discover_rejected_metadata(locator, &accepted_metadata)?;
    let mut accepted = Vec::new();
    let mut repairs = Vec::new();

    for located in located {
        // Re-derive through the locator before every canonical access rather than trusting a
        // cached or caller-constructed path.
        let directory =
            locator.private_dir(located.conversation_id, &located.creation_partition)?;
        let scan = match scan_event_log(&directory, located.conversation_id, durable_fs) {
            Ok(scan) => scan,
            Err(error) => {
                log::error!(
                    "[conversation-repository] authoritative event log rejected code={} conversation_id={} stream_file={}",
                    stable_code(error.code),
                    located.conversation_id,
                    error.path.file_name().and_then(|name| name.to_str()).unwrap_or("unknown")
                );
                recovery_issues.push(CatalogRecoveryIssue {
                    code: error.code,
                    conversation_id: Some(located.conversation_id),
                    relative_path: relative_display(locator.root(), &error.path),
                    detail: error.detail,
                });
                continue;
            }
        };
        repairs.extend(scan.repairs.clone());
        let provenance = match load_provenance(&directory, located.conversation_id) {
            Ok(provenance) => provenance,
            Err(issue) => {
                recovery_issues.push(issue);
                continue;
            }
        };
        let mut record = located.record;
        record.last_seq = scan.last_seq();
        if scan.frontier.attachment.has_events {
            record.project_attachment = scan.frontier.attachment.current.clone();
        }
        if let Some(execution_target) = &scan.frontier.execution_target {
            record.execution_target = execution_target.clone();
        }
        apply_scanned_lifecycle(&mut record, &scan.frontier);
        accepted.push(AcceptedCanonicalConversation {
            directory,
            record,
            scan,
            provenance,
        });
    }

    accepted.sort_by_key(|entry| entry.record.conversation_id.to_string());
    recovery_issues.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| format!("{:?}", left.code).cmp(&format!("{:?}", right.code)))
    });

    let conversations = accepted
        .iter()
        .map(|entry| entry_from_frontier(&entry.record, &entry.scan.frontier))
        .collect::<Vec<_>>();
    let generated_at_utc = conversations
        .iter()
        .flat_map(|entry| [&entry.created_at_utc, &entry.last_activity_at_utc])
        .max()
        .cloned()
        .unwrap_or_else(|| EMPTY_CATALOG_GENERATED_AT_UTC.to_string());
    Ok(CatalogRebuildResult {
        catalog: ConversationCatalogFileV1 {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_utc,
            conversations,
        },
        accepted,
        recovery_issues,
        repairs,
    })
}

/// Leftover Deleted metadata from the old archive/tombstone path must not be
/// resurrected to Ready by an event-log frontier. Physical delete removes the
/// directory; this guard only protects residual tombstones until open-time purge.
fn apply_scanned_lifecycle(record: &mut ConversationRecordV2, frontier: &ConversationFrontier) {
    if record.lifecycle_state == ConversationLifecycleState::Deleted {
        return;
    }
    if let Some(lifecycle_state) = frontier.lifecycle_state {
        record.lifecycle_state = lifecycle_state;
    }
}

fn entry_from_frontier(
    record: &ConversationRecordV2,
    frontier: &ConversationFrontier,
) -> ConversationCatalogEntryV1 {
    let last_activity_at_utc = frontier
        .summary
        .last_activity_at_utc
        .map_or(record.created_at_utc, |event_time| {
            event_time.max(record.created_at_utc)
        });
    ConversationCatalogEntryV1 {
        conversation_id: record.conversation_id,
        created_at_utc: format_created_at_utc(&record.created_at_utc),
        creation_partition: record.creation_partition.path.clone(),
        workspace_cwd: record.workspace_cwd.clone(),
        project_id: record
            .project_attachment
            .as_ref()
            .map(|attachment| attachment.project_id.clone()),
        lifecycle_state: record.lifecycle_state,
        title: frontier.summary.title.clone(),
        title_source: frontier.summary.title_source,
        last_activity_at_utc: format_created_at_utc(&last_activity_at_utc),
        message_count: frontier.summary.message_count,
        tool_count: frontier.summary.tool_count,
        last_seq: frontier.last_seq,
    }
}

fn load_provenance(
    directory: &Path,
    conversation_id: ConversationId,
) -> std::result::Result<Option<ConversationProvenanceFileV1>, CatalogRecoveryIssue> {
    let path = directory.join(PROVENANCE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CatalogRecoveryIssue {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                conversation_id: Some(conversation_id),
                relative_path: PROVENANCE_FILE.to_string(),
                detail: format!("provenance cannot be read: {error}"),
            })
        }
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| CatalogRecoveryIssue {
        code: ConversationErrorCode::ConversationRecoveryRequired,
        conversation_id: Some(conversation_id),
        relative_path: PROVENANCE_FILE.to_string(),
        detail: format!("provenance is corrupt: {error}"),
    })?;
    let found = value.get("schemaVersion").and_then(Value::as_u64);
    if found != Some(u64::from(PROVENANCE_SCHEMA_VERSION)) {
        return Err(CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationUnsupportedSchema,
            conversation_id: Some(conversation_id),
            relative_path: PROVENANCE_FILE.to_string(),
            detail: format!("unsupported provenance schemaVersion {found:?}"),
        });
    }
    let provenance: ConversationProvenanceFileV1 =
        serde_json::from_value(value).map_err(|error| CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationRecoveryRequired,
            conversation_id: Some(conversation_id),
            relative_path: PROVENANCE_FILE.to_string(),
            detail: format!("invalid provenance record: {error}"),
        })?;
    if let Err(detail) = provenance.validate() {
        return Err(CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationRecoveryRequired,
            conversation_id: Some(conversation_id),
            relative_path: PROVENANCE_FILE.to_string(),
            detail: detail.to_string(),
        });
    }
    Ok(Some(provenance))
}

fn valid_relative_source_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `bounded_scan` intentionally exposes only accepted records. This fixed-depth companion walk is
/// recovery-report-only: it uses `ConversationLocator::locate_relative` for structural authority,
/// never accepts a record, and exists so unsupported/corrupt authoritative metadata receives a
/// stable item instead of disappearing behind a rejected-count aggregate.
fn discover_rejected_metadata(
    locator: &ConversationLocator,
    accepted: &HashSet<PathBuf>,
) -> Result<Vec<CatalogRecoveryIssue>> {
    if !locator.root().exists() {
        return Ok(Vec::new());
    }
    let mut issues = Vec::new();
    let mut candidates = 0usize;
    for year in read_sorted_dirs(locator.root())? {
        for month in read_sorted_dirs(&year)? {
            for day in read_sorted_dirs(&month)? {
                for directory in read_sorted_dirs(&day)? {
                    candidates += 1;
                    if candidates > MAX_CONVERSATIONS_PER_SCAN {
                        return Err(CatalogError::ScanBound {
                            path: directory,
                            limit: MAX_CONVERSATIONS_PER_SCAN,
                        });
                    }
                    let Ok(relative) = directory.strip_prefix(locator.root()) else {
                        continue;
                    };
                    let Ok((conversation_id, partition)) = locator.locate_relative(relative) else {
                        continue;
                    };
                    if accepted.contains(relative) {
                        continue;
                    }
                    let canonical = locator.private_dir(conversation_id, &partition)?;
                    if canonical != directory {
                        continue;
                    }
                    if let Some(issue) = inspect_rejected_metadata(
                        locator.root(),
                        &directory,
                        relative,
                        conversation_id,
                        &partition,
                    )? {
                        issues.push(issue);
                    }
                }
            }
        }
    }
    Ok(issues)
}

fn read_sorted_dirs(path: &Path) -> Result<Vec<PathBuf>> {
    let reader = fs::read_dir(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut directories = Vec::new();
    for entry in reader {
        let entry = entry.map_err(|source| CatalogError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if directories.len() >= MAX_DIRECTORY_ENTRIES_PER_LEVEL {
            return Err(CatalogError::ScanBound {
                path: path.to_path_buf(),
                limit: MAX_DIRECTORY_ENTRIES_PER_LEVEL,
            });
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|source| CatalogError::Io {
            path: entry.path(),
            source,
        })?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn inspect_rejected_metadata(
    root: &Path,
    directory: &Path,
    relative: &Path,
    conversation_id: ConversationId,
    partition: &CreationPartition,
) -> Result<Option<CatalogRecoveryIssue>> {
    let path = directory.join(CONVERSATION_METADATA_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(Some(CatalogRecoveryIssue {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                conversation_id: Some(conversation_id),
                relative_path: relative_display(root, &path),
                detail: format!("conversation.json cannot be read: {error}"),
            }))
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return Ok(Some(CatalogRecoveryIssue {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                conversation_id: Some(conversation_id),
                relative_path: relative_display(root, &path),
                detail: format!("conversation.json is corrupt: {error}"),
            }))
        }
    };
    let found = value.get("schemaVersion").and_then(Value::as_u64);
    if found != Some(u64::from(CONVERSATION_SCHEMA_VERSION)) {
        return Ok(Some(CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationUnsupportedSchema,
            conversation_id: Some(conversation_id),
            relative_path: relative_display(root, &path),
            detail: format!("unsupported conversation schemaVersion {found:?}"),
        }));
    }
    let record: ConversationRecordV2 = match serde_json::from_value(value) {
        Ok(record) => record,
        Err(error) => {
            return Ok(Some(CatalogRecoveryIssue {
                code: ConversationErrorCode::ConversationRecoveryRequired,
                conversation_id: Some(conversation_id),
                relative_path: relative_display(root, &path),
                detail: format!("invalid conversation v2 metadata: {error}"),
            }))
        }
    };
    let valid = record.conversation_id == conversation_id
        && record.creation_partition == *partition
        && CreationPartition::from_created_at(record.created_at_utc) == *partition;
    if valid {
        // A valid metadata record rejected by bounded_scan should still fail closed rather than be
        // silently admitted through this reporting-only path.
        Ok(Some(CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationRecoveryRequired,
            conversation_id: Some(conversation_id),
            relative_path: relative.to_string_lossy().replace('\\', "/"),
            detail: "canonical directory was rejected by bounded validation".to_string(),
        }))
    } else {
        Ok(Some(CatalogRecoveryIssue {
            code: ConversationErrorCode::ConversationRecoveryRequired,
            conversation_id: Some(conversation_id),
            relative_path: relative_display(root, &path),
            detail: "conversation identity, timestamp, or partition does not match its directory"
                .to_string(),
        }))
    }
}

fn stable_code(code: ConversationErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationCreator, ExecutionTarget,
    };
    use crate::conversation::durable_fs::DirectoryPermissions;
    use crate::conversation::event_log::{
        ConversationEventRecordV2, ConversationEventType, EVENT_LOG_FILES,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    use crate::conversation::repository::{
        CatalogFlushCoordinator, CatalogFlushFailureStage, CATALOG_FLUSH_DEBOUNCE,
        CATALOG_FLUSH_MAX_DELAY,
    };

    static LARGE_CATALOG_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    const FIRST: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const SECOND: &str = "028f7a1c-1b4d-7c8a-9f01-0123456789ab";

    fn fixture() -> (TempDir, ConversationLocator, DurableFileSystem) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("conversations");
        let durable_fs = DurableFileSystem::new();
        durable_fs
            .create_dir_durable(&root, DirectoryPermissions::PrivateOwnerOnly)
            .unwrap();
        let locator = ConversationLocator::new(root).unwrap();
        (temp, locator, durable_fs)
    }

    fn record(id: &str, created_at: &str) -> ConversationRecordV2 {
        let created_at_utc = parse_created_at_utc(created_at).unwrap();
        ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::parse(id).unwrap(),
            created_at_utc,
            creation_partition: CreationPartition::from_created_at(created_at_utc),
            workspace_cwd: format!("/visible/sessions/{id}"),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::Ready,
            last_seq: 0,
            created_by: ConversationCreator::Termul,
            title: None,
            title_source: None,
        }
    }

    fn large_catalog(
        entry_count: usize,
    ) -> (
        ConversationCatalog,
        ConversationCatalogFileV1,
        ConversationRecordV2,
    ) {
        let mut conversations = Vec::with_capacity(entry_count);
        let mut first_record = None;
        for index in 0..entry_count {
            let id = format!("00000000-0000-4000-8000-{index:012x}");
            let value = record(&id, "2026-08-15T09:45:15.000Z");
            first_record.get_or_insert_with(|| value.clone());
            conversations.push(entry_from_frontier(
                &value,
                &ConversationFrontier::default(),
            ));
        }
        let file = ConversationCatalogFileV1 {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_utc: "2026-08-15T09:45:15.000Z".to_string(),
            conversations,
        };
        (
            ConversationCatalog::from_file(file.clone()),
            file,
            first_record.expect("large catalog has at least one record"),
        )
    }

    fn percentile_99_micros(mut samples: Vec<u128>) -> u128 {
        samples.sort_unstable();
        let index = (samples.len() * 99 / 100).min(samples.len() - 1);
        samples[index]
    }

    fn mutation_admission_p99_micros(
        coordinator: &Arc<CatalogFlushCoordinator>,
        record: &ConversationRecordV2,
        samples: usize,
    ) -> u128 {
        let frontier = ConversationFrontier::default();
        let mut timings = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            coordinator.admit_for_test(record, &frontier);
            timings.push(started.elapsed().as_micros());
        }
        percentile_99_micros(timings)
    }

    fn write_conversation(
        locator: &ConversationLocator,
        durable_fs: &DurableFileSystem,
        record: &ConversationRecordV2,
    ) -> PathBuf {
        let directory = locator
            .private_dir(record.conversation_id, &record.creation_partition)
            .unwrap();
        durable_fs
            .create_dir_durable(&directory, DirectoryPermissions::PrivateOwnerOnly)
            .unwrap();
        durable_fs
            .replace_bytes(
                &directory.join(CONVERSATION_METADATA_FILE),
                &serde_json::to_vec_pretty(record).unwrap(),
            )
            .unwrap();
        for file in EVENT_LOG_FILES {
            durable_fs
                .replace_bytes(&directory.join(file), b"")
                .unwrap();
        }
        directory
    }

    #[test]
    fn catalog_remove_drops_the_entry() {
        let mut catalog = ConversationCatalog::from_file(ConversationCatalogFileV1 {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_utc: EMPTY_CATALOG_GENERATED_AT_UTC.to_string(),
            conversations: Vec::new(),
        });
        let value = record(FIRST, "2026-08-15T09:45:15.000Z");
        catalog.upsert(&value, &ConversationFrontier::default());
        assert_eq!(catalog.len(), 1);
        catalog.remove(value.conversation_id);
        assert!(catalog.is_empty());
        assert_eq!(catalog.len(), 0);
    }

    #[test]
    fn empty_catalog_uses_exact_epoch_without_reading_the_clock() {
        let (_temp, locator, durable_fs) = fixture();
        let rebuilt = rebuild_catalog(&locator, &durable_fs).unwrap();
        assert!(rebuilt.catalog.conversations.is_empty());
        assert_eq!(
            rebuilt.catalog.generated_at_utc,
            EMPTY_CATALOG_GENERATED_AT_UTC
        );
        assert_eq!(rebuilt.catalog.schema_version, CATALOG_SCHEMA_VERSION);
    }

    #[test]
    fn incremental_upsert_matches_explicit_rebuild_for_1000_conversations() {
        let (_temp, locator, durable_fs) = fixture();
        let mut records = Vec::new();
        for index in 0..1_000_u64 {
            let id = format!("00000000-0000-4000-8000-{index:012x}");
            let value = record(&id, "2026-08-15T09:45:15.000Z");
            let directory = locator
                .private_dir(value.conversation_id, &value.creation_partition)
                .unwrap();
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(CONVERSATION_METADATA_FILE),
                serde_json::to_vec_pretty(&value).unwrap(),
            )
            .unwrap();
            for file in EVENT_LOG_FILES {
                fs::write(directory.join(file), b"").unwrap();
            }
            records.push(value);
        }

        let mut incremental = ConversationCatalog::from_file(ConversationCatalogFileV1 {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_utc: EMPTY_CATALOG_GENERATED_AT_UTC.to_string(),
            conversations: Vec::new(),
        });
        for value in records.iter().rev() {
            incremental.upsert(value, &ConversationFrontier::default());
        }
        let rebuilt = rebuild_catalog(&locator, &durable_fs).unwrap();
        assert_eq!(incremental.len(), 1_000);
        assert_eq!(
            incremental.deterministic_bytes(),
            rebuilt.catalog.deterministic_bytes()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_50000_capture_p99_le_2ms_and_mutation_p99_le_5ms() {
        let _serial = LARGE_CATALOG_TEST_LOCK.lock().await;
        let (catalog, expected_file, mutation_record) = large_catalog(50_000);
        let temp = tempfile::tempdir().unwrap();
        let durable_fs = DurableFileSystem::new();
        let coordinator = CatalogFlushCoordinator::new_for_test(
            catalog,
            durable_fs,
            temp.path().canonicalize().unwrap().join(CATALOG_FILE),
        );

        let mut capture_samples = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let started = Instant::now();
            let generation = coordinator.capture_for_test();
            capture_samples.push(started.elapsed().as_micros());
            assert_eq!(generation.entry_count, 50_000);
            assert_eq!(generation.chunk_count(), 49);
        }
        let capture_p99_micros = percentile_99_micros(capture_samples);

        // Retain the frozen generation while admitting the next mutation. This is the exact
        // production overlap that exercises pointer-vector and one-dirty-chunk copy-on-write.
        let frozen = coordinator.capture_for_test();
        coordinator.admit_for_test(&mutation_record, &ConversationFrontier::default());
        let metrics = coordinator.last_admission_metrics();
        assert!(metrics.cloned_chunk_pointers <= 49, "{metrics:?}");
        assert!(
            metrics.cloned_dirty_entries <= CATALOG_CHUNK_ENTRIES,
            "{metrics:?}"
        );
        assert_eq!(metrics.serialized_bytes_under_lock, 0);

        let serialize = tokio::task::spawn_blocking(move || frozen.deterministic_bytes());
        let mutation_p99_micros =
            mutation_admission_p99_micros(&coordinator, &mutation_record, 1_000);
        let serialized = serialize.await.unwrap();
        assert_eq!(serialized, expected_file.deterministic_bytes());
        assert!(
            capture_p99_micros <= 2_000,
            "capture p99 {capture_p99_micros}us"
        );
        assert!(
            mutation_p99_micros <= 5_000,
            "mutation admission p99 {mutation_p99_micros}us"
        );
        println!(
            "catalog_metrics entry_count=50000 chunk_count=49 capture_p99_us={capture_p99_micros} mutation_p99_us={mutation_p99_micros} serialized_bytes_under_lock=0"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_50000_heartbeat_gap_le_50ms_debounce_100ms_max_delay_1s() {
        let _serial = LARGE_CATALOG_TEST_LOCK.lock().await;
        let (catalog, _expected_file, mutation_record) = large_catalog(50_000);
        let temp = tempfile::tempdir().unwrap();
        let durable_fs = DurableFileSystem::new();
        let coordinator = CatalogFlushCoordinator::new_for_test(
            catalog,
            durable_fs,
            temp.path().canonicalize().unwrap().join(CATALOG_FILE),
        );
        coordinator.admit_for_test(&mutation_record, &ConversationFrontier::default());

        let done = Arc::new(AtomicBool::new(false));
        let heartbeat_done = Arc::clone(&done);
        let heartbeat = tokio::spawn(async move {
            let mut previous = Instant::now();
            let mut maximum_gap = Duration::ZERO;
            while !heartbeat_done.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let now = Instant::now();
                maximum_gap = maximum_gap.max(now.saturating_duration_since(previous));
                previous = now;
            }
            maximum_gap
        });
        tokio::task::yield_now().await;
        let flush_started = Instant::now();
        let receipt = coordinator.flush_once_for_test().await.unwrap();
        let flush_duration = flush_started.elapsed();
        done.store(true, Ordering::Release);
        let maximum_gap = heartbeat.await.unwrap();

        assert_eq!(CATALOG_FLUSH_DEBOUNCE, Duration::from_millis(100));
        assert_eq!(CATALOG_FLUSH_MAX_DELAY, Duration::from_secs(1));
        assert!(
            maximum_gap <= Duration::from_millis(50),
            "heartbeat gap {maximum_gap:?}"
        );
        assert!(
            flush_duration <= CATALOG_FLUSH_MAX_DELAY,
            "flush took {flush_duration:?}"
        );
        assert!(receipt.flushed_generation >= receipt.requested_generation);
        println!(
            "catalog_heartbeat entry_count=50000 max_gap_ms={} flush_ms={} debounce_ms=100 max_delay_ms=1000",
            maximum_gap.as_millis(),
            flush_duration.as_millis()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_serialization_failure_returns_catalog_flush_failed_and_keeps_mutation_p99_le_5ms(
    ) {
        let _serial = LARGE_CATALOG_TEST_LOCK.lock().await;
        let (catalog, _expected_file, mutation_record) = large_catalog(50_000);
        let temp = tempfile::tempdir().unwrap();
        let durable_fs = DurableFileSystem::new();
        let coordinator = CatalogFlushCoordinator::new_for_test(
            catalog,
            durable_fs,
            temp.path().canonicalize().unwrap().join(CATALOG_FILE),
        );
        let admitted_generation =
            coordinator.admit_for_test(&mutation_record, &ConversationFrontier::default());
        coordinator.fail_next_serializations(1);
        let error = coordinator.flush_once_for_test().await.unwrap_err();
        assert_eq!(error.code, "CATALOG_FLUSH_FAILED");
        assert_eq!(error.stage, CatalogFlushFailureStage::Serialization);
        assert_eq!(error.generation, admitted_generation);
        assert_eq!(error.pending_generation, admitted_generation);
        assert_eq!(coordinator.pending_generation(), admitted_generation);

        let mutation_p99_micros =
            mutation_admission_p99_micros(&coordinator, &mutation_record, 1_000);
        assert!(
            mutation_p99_micros <= 5_000,
            "post-serialization-failure mutation p99 {mutation_p99_micros}us"
        );
        let pending_generation = coordinator.pending_generation();
        let retry = coordinator.flush_once_for_test().await.unwrap();
        assert!(retry.flushed_generation >= pending_generation);
        println!(
            "catalog_failure stage=serialization code={} pending_generation={} mutation_p99_us={mutation_p99_micros}",
            error.code, pending_generation
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_replace_failure_returns_catalog_flush_failed_and_keeps_mutation_p99_le_5ms() {
        let _serial = LARGE_CATALOG_TEST_LOCK.lock().await;
        let (catalog, _expected_file, mutation_record) = large_catalog(50_000);
        let temp = tempfile::tempdir().unwrap();
        let durable_fs = DurableFileSystem::new();
        let coordinator = CatalogFlushCoordinator::new_for_test(
            catalog,
            durable_fs,
            temp.path().canonicalize().unwrap().join(CATALOG_FILE),
        );
        let admitted_generation =
            coordinator.admit_for_test(&mutation_record, &ConversationFrontier::default());
        coordinator.fail_next_writes(1);
        let error = coordinator.flush_once_for_test().await.unwrap_err();
        assert_eq!(error.code, "CATALOG_FLUSH_FAILED");
        assert_eq!(error.stage, CatalogFlushFailureStage::Replacement);
        assert_eq!(error.generation, admitted_generation);
        assert_eq!(error.pending_generation, admitted_generation);
        assert_eq!(coordinator.pending_generation(), admitted_generation);

        let mutation_p99_micros =
            mutation_admission_p99_micros(&coordinator, &mutation_record, 1_000);
        assert!(
            mutation_p99_micros <= 5_000,
            "post-replacement-failure mutation p99 {mutation_p99_micros}us"
        );
        let pending_generation = coordinator.pending_generation();
        let retry = coordinator.flush_once_for_test().await.unwrap();
        assert!(retry.flushed_generation >= pending_generation);
        println!(
            "catalog_failure stage=replacement code={} pending_generation={} mutation_p99_us={mutation_p99_micros}",
            error.code, pending_generation
        );
    }

    #[test]
    fn catalog_is_sorted_by_id_and_uses_maximum_validated_canonical_timestamp() {
        let (_temp, locator, durable_fs) = fixture();
        let second = record(SECOND, "2026-08-14T09:45:15.000Z");
        let first = record(FIRST, "2026-08-15T09:45:15.000Z");
        write_conversation(&locator, &durable_fs, &second);
        let first_dir = write_conversation(&locator, &durable_fs, &first);
        let event = ConversationEventRecordV2::new(
            first.conversation_id,
            1,
            parse_created_at_utc("2026-08-16T10:00:00.000Z").unwrap(),
            ConversationEventType::MessageChunk,
            json!({"role":"agent"}),
        );
        durable_fs
            .append_jsonl(
                &first_dir.join(event.type_.stream().file_name()),
                &serde_json::to_vec(&event).unwrap(),
            )
            .unwrap();

        let first_build = rebuild_catalog(&locator, &durable_fs).unwrap();
        let second_build = rebuild_catalog(&locator, &durable_fs).unwrap();
        assert_eq!(
            first_build.catalog.deterministic_bytes(),
            second_build.catalog.deterministic_bytes()
        );
        assert_eq!(
            first_build
                .catalog
                .conversations
                .iter()
                .map(|entry| entry.conversation_id.to_string())
                .collect::<Vec<_>>(),
            vec![FIRST, SECOND]
        );
        assert_eq!(
            first_build.catalog.generated_at_utc,
            "2026-08-16T10:00:00.000Z"
        );
        assert_eq!(first_build.catalog.conversations[0].last_seq, 1);
    }

    #[test]
    fn corrupt_log_is_excluded_without_rewriting_authoritative_bytes() {
        let (_temp, locator, durable_fs) = fixture();
        let value = record(FIRST, "2026-08-15T09:45:15.000Z");
        let directory = write_conversation(&locator, &durable_fs, &value);
        let path = directory.join(crate::conversation::event_log::MESSAGES_FILE);
        let bytes = b"{bad}\n{}\n";
        fs::write(&path, bytes).unwrap();

        let rebuilt = rebuild_catalog(&locator, &durable_fs).unwrap();
        assert!(rebuilt.catalog.conversations.is_empty());
        assert_eq!(rebuilt.recovery_issues.len(), 1);
        assert_eq!(
            rebuilt.recovery_issues[0].code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn unsupported_metadata_is_reported_and_never_replaced() {
        let (_temp, locator, durable_fs) = fixture();
        let value = record(FIRST, "2026-08-15T09:45:15.000Z");
        let directory = write_conversation(&locator, &durable_fs, &value);
        let path = directory.join(CONVERSATION_METADATA_FILE);
        let mut future = serde_json::to_value(value).unwrap();
        future["schemaVersion"] = json!(99);
        let bytes = serde_json::to_vec(&future).unwrap();
        fs::write(&path, &bytes).unwrap();

        let rebuilt = rebuild_catalog(&locator, &durable_fs).unwrap();
        assert!(rebuilt.catalog.conversations.is_empty());
        assert!(rebuilt.recovery_issues.iter().any(|issue| {
            issue.code == ConversationErrorCode::ConversationUnsupportedSchema
                && issue.conversation_id == Some(ConversationId::parse(FIRST).unwrap())
        }));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
