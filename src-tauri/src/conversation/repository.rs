//! Sole live writer for canonical Conversation metadata and event history.
//!
//! The repository owns all v2 JSON/JSONL mutations. Legacy stores are intentionally absent from
//! this module. Every canonical path is re-derived through [`ConversationLocator`], appends are
//! serialized by a per-Conversation async mutex, and `catalog.json` is rewritten last as a
//! disposable deterministic cache.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, Notify, OwnedMutexGuard};
use tokio::time::Instant as TokioInstant;
use uuid::Uuid;

use crate::conversation::catalog::{
    rebuild_catalog, AcceptedCanonicalConversation, CatalogRecoveryIssue, CatalogReplaceAck,
    CatalogReplaceFence, ConversationCatalog, ConversationCatalogSnapshot,
    ConversationProvenanceFileV1, CATALOG_FILE, CONVERSATION_METADATA_FILE, PROVENANCE_FILE,
};
#[cfg(test)]
use crate::conversation::catalog::{CatalogAdmissionMetrics, ConversationCatalogGeneration};
use crate::conversation::contracts::{
    encoded_json_len_bounded, AgentSessionBinding, AgentSessionBindingState, ConversationErrorCode,
    ConversationHistorySummaryV1, ConversationId, ConversationLifecycleState, ConversationRecordV2,
    ConversationTitleSource, ExecutionTarget, ProjectAttachment,
    AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    MAX_CONVERSATION_RECORD_BYTES, PROJECT_ATTACHMENT_SCHEMA_VERSION,
};
use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};
use crate::conversation::event_log::{
    apply_event, encode_event_record_bounded, read_event_page as read_event_page_from_log,
    BindingEventPayloadV1, BindingReplacementPayloadV1, ConversationEventRecordV2,
    ConversationEventType, ConversationFrontier, EventLogRepairWarning, EventLogScan,
    EventRecordEncodingError, ExecutionTargetEventPayloadV1, ProjectAttachmentEventPayloadV1,
    CONVERSATION_EVENT_SCHEMA_VERSION, EVENT_LOG_FILES, MAX_EVENT_PAGE_LIMIT, MIN_EVENT_PAGE_LIMIT,
};
use crate::conversation::locator::ConversationLocator;
use crate::conversation::write_authority::RepositoryWritePermit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRecoveryKind {
    TornTailRepaired,
    CorruptAuthoritativeRecord,
    UnsupportedAuthoritativeSchema,
    WorkspaceRecoveryRequired,
    IncompleteCreationRecovered,
    CatalogIgnored,
    CatalogRewriteFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryRecoveryItem {
    pub code: ConversationErrorCode,
    pub kind: RepositoryRecoveryKind,
    pub conversation_id: Option<ConversationId>,
    pub relative_path: String,
    pub detail: String,
    pub repaired: bool,
    pub requires_action: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryOpenReport {
    pub valid_conversation_count: usize,
    pub recovery_items: Vec<RepositoryRecoveryItem>,
    pub scanned_event_count: u64,
    pub sparse_index_entry_count: usize,
    pub retained_payload_bytes: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BootstrapScanMetrics {
    pub scanned_event_count: u64,
    pub sparse_index_entry_count: usize,
    pub retained_payload_bytes: usize,
}

pub(crate) fn bootstrap_scan_metrics<'a>(
    scans: impl IntoIterator<Item = &'a EventLogScan>,
) -> BootstrapScanMetrics {
    let mut metrics = BootstrapScanMetrics::default();
    for scan in scans {
        metrics.scanned_event_count = metrics
            .scanned_event_count
            .saturating_add(scan.event_count());
        metrics.sparse_index_entry_count = metrics
            .sparse_index_entry_count
            .saturating_add(scan.sparse_index_entry_count());
    }
    metrics
}

#[derive(Debug, Clone, Default)]
pub struct ConversationMetadataUpdate {
    pub lifecycle_state: Option<ConversationLifecycleState>,
    pub execution_target: Option<ExecutionTarget>,
    pub title: Option<String>,
    pub title_source: Option<ConversationTitleSource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationAggregateMutationRecord {
    pub before: ConversationRecordV2,
    pub after: ConversationRecordV2,
    pub event: ConversationEventRecordV2,
}

#[derive(Debug, Clone)]
struct ConversationState {
    record: ConversationRecordV2,
    scan: Arc<EventLogScan>,
    provenance: Option<ConversationProvenanceFileV1>,
}

pub const CATALOG_FLUSH_DEBOUNCE: Duration = Duration::from_millis(100);
pub const CATALOG_FLUSH_MAX_DELAY: Duration = Duration::from_secs(1);
pub const CATALOG_FLUSH_RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogFlushReceipt {
    pub requested_generation: u64,
    pub flushed_generation: u64,
    pub write_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFlushFailureStage {
    Deadline,
    Serialization,
    Replacement,
    Worker,
}

/// Typed, secret-safe failure receipt. Pending generation state is retained for deterministic
/// retry after either serialization or atomic replacement failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogFlushError {
    pub code: &'static str,
    pub stage: CatalogFlushFailureStage,
    pub generation: u64,
    pub pending_generation: u64,
    pub detail: String,
}

impl fmt::Display for CatalogFlushError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} while flushing catalog generation {} at {:?}: {}",
            self.code, self.generation, self.stage, self.detail
        )
    }
}

impl std::error::Error for CatalogFlushError {}

/// Repository-allocated canonical sequence. `event.seq` on the durability path
/// IS the ticket sequence consumed by ordered persistence (TASK-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalSequenceTicket {
    pub seq: u64,
}

impl CanonicalSequenceTicket {
    #[must_use]
    pub const fn from_allocated_seq(seq: u64) -> Self {
        Self { seq }
    }

    #[must_use]
    pub fn from_event(event: &ConversationEventRecordV2) -> Self {
        Self { seq: event.seq }
    }
}

struct CatalogCacheState {
    catalog: ConversationCatalog,
    first_dirty_at: Option<Instant>,
    last_dirty_at: Option<Instant>,
    flushed_generation: u64,
    write_count: u64,
    last_conversation_id: Option<ConversationId>,
}

/// Coalesces disposable `catalog.json` snapshots without blocking canonical mutations.
///
/// Each mutation updates one in-memory entry, advances a generation, and schedules one weakly
/// owned task. The task waits for 100 ms of quiet but never later than one second after the first
/// dirty generation. Replacement runs on Tokio's blocking pool and retries without clearing a
/// newer generation. A host barrier can flush the latest generation under one absolute deadline.
pub struct CatalogFlushCoordinator {
    state: ParkingMutex<CatalogCacheState>,
    flush_lock: TokioMutex<()>,
    durable_fs: DurableFileSystem,
    path: PathBuf,
    scheduled: AtomicBool,
    wake: Notify,
    replace_fence: Arc<CatalogReplaceFence>,
    in_flight_replace: ParkingMutex<Option<tokio::task::JoinHandle<CatalogReplaceOutcome>>>,
    #[cfg(test)]
    fail_serializations_remaining: std::sync::atomic::AtomicUsize,
}

type CatalogReplaceOutcome =
    std::result::Result<CatalogReplaceAck, crate::conversation::durable_fs::DurableFsError>;

impl CatalogFlushCoordinator {
    fn new(
        catalog: ConversationCatalog,
        durable_fs: DurableFileSystem,
        path: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: ParkingMutex::new(CatalogCacheState {
                catalog,
                first_dirty_at: None,
                last_dirty_at: None,
                flushed_generation: 0,
                write_count: 0,
                last_conversation_id: None,
            }),
            flush_lock: TokioMutex::new(()),
            durable_fs,
            path,
            scheduled: AtomicBool::new(false),
            wake: Notify::new(),
            replace_fence: Arc::new(CatalogReplaceFence::new()),
            in_flight_replace: ParkingMutex::new(None),
            #[cfg(test)]
            fail_serializations_remaining: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn upsert(
        self: &Arc<Self>,
        record: &ConversationRecordV2,
        frontier: &ConversationFrontier,
    ) -> u64 {
        let generation = self.admit_generation(record, frontier);
        self.wake.notify_waiters();
        self.schedule();
        generation
    }

    fn remove(self: &Arc<Self>, conversation_id: ConversationId) -> u64 {
        let now = Instant::now();
        let generation = {
            let mut state = self.state.lock();
            let generation = state.catalog.remove(conversation_id);
            state.first_dirty_at.get_or_insert(now);
            state.last_dirty_at = Some(now);
            state.last_conversation_id = Some(conversation_id);
            generation
        };
        self.wake.notify_waiters();
        self.schedule();
        generation
    }

    fn admit_generation(
        &self,
        record: &ConversationRecordV2,
        frontier: &ConversationFrontier,
    ) -> u64 {
        let now = Instant::now();
        let mut state = self.state.lock();
        let generation = state.catalog.upsert(record, frontier);
        state.first_dirty_at.get_or_insert(now);
        state.last_dirty_at = Some(now);
        state.last_conversation_id = Some(record.conversation_id);
        generation
    }

    fn schedule(self: &Arc<Self>) {
        if self
            .scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(self);
        repository_runtime_handle().spawn(async move {
            Self::run_scheduled(weak).await;
        });
    }

    async fn run_scheduled(weak: Weak<Self>) {
        loop {
            let Some(coordinator) = weak.upgrade() else {
                return;
            };
            let due = {
                let state = coordinator.state.lock();
                let Some(first_dirty_at) = state.first_dirty_at else {
                    coordinator.scheduled.store(false, Ordering::Release);
                    return;
                };
                let last_dirty_at = state.last_dirty_at.unwrap_or(first_dirty_at);
                (last_dirty_at + CATALOG_FLUSH_DEBOUNCE)
                    .min(first_dirty_at + CATALOG_FLUSH_MAX_DELAY)
            };
            let due = TokioInstant::from_std(due);
            tokio::select! {
                () = tokio::time::sleep_until(due) => {}
                () = coordinator.wake.notified() => continue,
            }

            if let Err(error) = coordinator.flush_once(None).await {
                log::warn!(
                    "[conversation-repository] catalog cache retry code={} generation={} duration_ms=0",
                    error.code,
                    error.generation
                );
                tokio::time::sleep(CATALOG_FLUSH_RETRY_DELAY).await;
                continue;
            }

            let dirty = {
                let state = coordinator.state.lock();
                state.flushed_generation < state.catalog.generation()
            };
            if dirty {
                continue;
            }
            coordinator.scheduled.store(false, Ordering::Release);
            let raced_dirty = {
                let state = coordinator.state.lock();
                state.flushed_generation < state.catalog.generation()
            };
            if raced_dirty {
                coordinator.schedule();
            }
            return;
        }
    }

    async fn flush_once(
        &self,
        deadline: Option<TokioInstant>,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        let _guard = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, self.flush_lock.lock())
                .await
                .map_err(|_| self.deadline_error())?,
            None => self.flush_lock.lock().await,
        };
        // Capture only immutable chunk Arcs under the global lock. No entry flattening or JSON
        // serialization is allowed in this critical section.
        let generation = {
            let state = self.state.lock();
            let generation = state.catalog.capture();
            if state.flushed_generation >= generation.generation {
                return Ok(CatalogFlushReceipt {
                    requested_generation: generation.generation,
                    flushed_generation: state.flushed_generation,
                    write_count: state.write_count,
                });
            }
            generation
        };
        let started = Instant::now();
        #[cfg(test)]
        let fail_serialization = self
            .fail_serializations_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok();
        #[cfg(not(test))]
        let fail_serialization = false;
        let serialize_generation = generation.clone();
        let serialize = tokio::task::spawn_blocking(move || {
            if fail_serialization {
                Err(())
            } else {
                Ok(serialize_generation.deterministic_bytes())
            }
        });
        let bytes = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, serialize)
                .await
                .map_err(|_| self.deadline_error())?,
            None => serialize.await,
        }
        .map_err(|_| {
            self.flush_error(
                CatalogFlushFailureStage::Worker,
                generation.generation,
                "catalog serialization task failed",
            )
        })?
        .map_err(|()| {
            self.flush_error(
                CatalogFlushFailureStage::Serialization,
                generation.generation,
                "catalog generation serialization failed",
            )
        })?;

        self.reap_finished_replace().await;
        let durable_fs = self.durable_fs.clone();
        let path = self.path.clone();
        let fence = Arc::clone(&self.replace_fence);
        let replace_generation = generation.generation;
        let mut replace = tokio::task::spawn_blocking(move || {
            fence.replace_generation(&durable_fs, &path, replace_generation, &bytes)
        });
        let ack = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, &mut replace).await {
                Ok(joined) => joined,
                Err(_) => {
                    // Keep the in-flight replace fenced. timeout_at must not let
                    // an abandoned older generation overwrite a newer disk write.
                    *self.in_flight_replace.lock() = Some(replace);
                    return Err(self.deadline_error());
                }
            },
            None => replace.await,
        }
        .map_err(|_| {
            self.flush_error(
                CatalogFlushFailureStage::Worker,
                generation.generation,
                "catalog replacement task failed",
            )
        })?
        .map_err(|_| {
            self.flush_error(
                CatalogFlushFailureStage::Replacement,
                generation.generation,
                "catalog atomic replacement failed",
            )
        })?;
        if !ack.committed {
            let receipt = {
                let state = self.state.lock();
                CatalogFlushReceipt {
                    requested_generation: generation.generation,
                    flushed_generation: state.flushed_generation.max(ack.committed_generation),
                    write_count: state.write_count,
                }
            };
            return Ok(receipt);
        }

        let receipt = {
            let mut state = self.state.lock();
            state.write_count = state.write_count.saturating_add(1);
            state.flushed_generation = state
                .flushed_generation
                .max(ack.committed_generation)
                .max(generation.generation);
            if state.catalog.generation() == generation.generation {
                state.first_dirty_at = None;
                state.last_dirty_at = None;
            }
            CatalogFlushReceipt {
                requested_generation: generation.generation,
                flushed_generation: state.flushed_generation,
                write_count: state.write_count,
            }
        };
        log::info!(
            "[conversation-repository] catalog flush complete generation={} entry_count={} write_count={} duration_ms={}",
            generation.generation,
            generation.entry_count,
            receipt.write_count,
            started.elapsed().as_millis()
        );
        Ok(receipt)
    }

    pub async fn flush_until(
        &self,
        deadline: TokioInstant,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        let mut last_error = None;
        loop {
            if TokioInstant::now() >= deadline {
                return Err(last_error.unwrap_or_else(|| self.deadline_error()));
            }
            match self.flush_once(Some(deadline)).await {
                Ok(receipt) => {
                    let current_generation = self.state.lock().catalog.generation();
                    if receipt.flushed_generation >= current_generation {
                        return Ok(CatalogFlushReceipt {
                            requested_generation: current_generation,
                            ..receipt
                        });
                    }
                }
                Err(error) if error.code == "CONVERSATION_CATALOG_FLUSH_DEADLINE" => {
                    return Err(error);
                }
                Err(error) => last_error = Some(error),
            }
            let retry_at = (TokioInstant::now() + CATALOG_FLUSH_RETRY_DELAY).min(deadline);
            tokio::time::sleep_until(retry_at).await;
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ConversationCatalogSnapshot {
        let generation = self.state.lock().catalog.capture();
        generation.snapshot()
    }

    #[must_use]
    pub fn flushed_generation(&self) -> u64 {
        self.state.lock().flushed_generation
    }

    #[must_use]
    pub fn write_count(&self) -> u64 {
        self.state.lock().write_count
    }

    fn deadline_error(&self) -> CatalogFlushError {
        let generation = self.state.lock().catalog.generation();
        CatalogFlushError {
            code: "CONVERSATION_CATALOG_FLUSH_DEADLINE",
            stage: CatalogFlushFailureStage::Deadline,
            generation,
            pending_generation: generation,
            detail: "catalog cache flush exceeded the host deadline".to_string(),
        }
    }

    async fn reap_finished_replace(&self) {
        let finished = {
            let mut parked = self.in_flight_replace.lock();
            match parked.as_ref() {
                Some(handle) if handle.is_finished() => parked.take(),
                _ => None,
            }
        };
        if let Some(handle) = finished {
            let _ = handle.await;
        }
    }

    fn flush_error(
        &self,
        stage: CatalogFlushFailureStage,
        generation: u64,
        detail: &'static str,
    ) -> CatalogFlushError {
        CatalogFlushError {
            code: "CATALOG_FLUSH_FAILED",
            stage,
            generation,
            pending_generation: self.state.lock().catalog.generation(),
            detail: detail.to_string(),
        }
    }

    #[cfg(test)]
    fn reset_write_counters(&self) {
        self.durable_fs.reset_replace_counters();
        self.state.lock().write_count = 0;
    }

    #[cfg(test)]
    fn durable_catalog_write_count(&self) -> u64 {
        self.durable_fs.catalog_replace_count()
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        catalog: ConversationCatalog,
        durable_fs: DurableFileSystem,
        path: PathBuf,
    ) -> Arc<Self> {
        Self::new(catalog, durable_fs, path)
    }

    #[cfg(test)]
    pub(crate) fn admit_for_test(
        self: &Arc<Self>,
        record: &ConversationRecordV2,
        frontier: &ConversationFrontier,
    ) -> u64 {
        self.admit_generation(record, frontier)
    }

    #[cfg(test)]
    pub(crate) fn capture_for_test(&self) -> ConversationCatalogGeneration {
        self.state.lock().catalog.capture()
    }

    #[cfg(test)]
    pub(crate) fn fail_next_writes(&self, count: usize) {
        self.durable_fs.fail_next_catalog_replaces(count);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_serializations(&self, count: usize) {
        self.fail_serializations_remaining
            .store(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn pending_generation(&self) -> u64 {
        self.state.lock().catalog.generation()
    }

    #[cfg(test)]
    pub(crate) fn last_admission_metrics(&self) -> CatalogAdmissionMetrics {
        self.state.lock().catalog.last_admission_metrics()
    }

    #[cfg(test)]
    pub(crate) async fn flush_once_for_test(
        &self,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        self.flush_once(None).await
    }

    #[cfg(test)]
    pub(crate) async fn flush_once_until_for_test(
        &self,
        deadline: TokioInstant,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        self.flush_once(Some(deadline)).await
    }

    #[cfg(test)]
    pub(crate) fn stall_catalog_replace(&self, generation: u64) {
        self.replace_fence.stall_generation(generation);
    }

    #[cfg(test)]
    pub(crate) fn release_catalog_replace_stall(&self) {
        self.replace_fence.release_stall();
    }

    #[cfg(test)]
    pub(crate) fn wait_for_catalog_replace_stall(&self, timeout: Duration) -> bool {
        self.replace_fence.wait_until_stalled(timeout)
    }

    #[cfg(test)]
    pub(crate) fn last_written_catalog_generation(&self) -> u64 {
        self.replace_fence.last_written_generation()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConversationBindingKeys {
    active: Option<String>,
    history: HashSet<String>,
}

#[derive(Debug, Default)]
struct RepositoryBindingIndex {
    active_owners: HashMap<String, HashSet<ConversationId>>,
    history_owners: HashMap<String, HashSet<ConversationId>>,
    by_conversation: HashMap<ConversationId, ConversationBindingKeys>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryBindingIndexStats {
    pub active_binding_count: usize,
    pub history_binding_count: usize,
    pub conversation_count: usize,
    pub generation: u64,
}

impl RepositoryBindingIndex {
    fn from_states(states: &HashMap<ConversationId, ConversationState>) -> Self {
        let mut index = Self::default();
        for (conversation_id, state) in states {
            let keys = binding_keys(state);
            index.install(*conversation_id, keys, false);
        }
        index.generation = 0;
        index
    }

    fn refresh(&mut self, conversation_id: ConversationId, keys: ConversationBindingKeys) -> u64 {
        if self.by_conversation.get(&conversation_id) == Some(&keys) {
            return self.generation;
        }
        self.remove(conversation_id);
        self.install(conversation_id, keys, true)
    }

    fn remove(&mut self, conversation_id: ConversationId) {
        let Some(previous) = self.by_conversation.remove(&conversation_id) else {
            return;
        };
        if let Some(active) = previous.active {
            remove_owner(&mut self.active_owners, &active, conversation_id);
        }
        for history in previous.history {
            remove_owner(&mut self.history_owners, &history, conversation_id);
        }
    }

    fn install(
        &mut self,
        conversation_id: ConversationId,
        keys: ConversationBindingKeys,
        advance_generation: bool,
    ) -> u64 {
        if let Some(active) = keys.active.as_ref() {
            self.active_owners
                .entry(active.clone())
                .or_default()
                .insert(conversation_id);
        }
        for history in &keys.history {
            self.history_owners
                .entry(history.clone())
                .or_default()
                .insert(conversation_id);
        }
        self.by_conversation.insert(conversation_id, keys);
        if advance_generation {
            self.generation = self.generation.saturating_add(1);
        }
        self.generation
    }

    fn resolve_active(&self, agent_session_id: &str) -> Option<ConversationId> {
        unique_owner(self.active_owners.get(agent_session_id))
    }

    fn resolve_history(&self, agent_session_id: &str) -> Option<ConversationId> {
        unique_owner(self.history_owners.get(agent_session_id))
    }

    fn stats(&self) -> RepositoryBindingIndexStats {
        RepositoryBindingIndexStats {
            active_binding_count: self
                .active_owners
                .values()
                .filter(|owners| owners.len() == 1)
                .count(),
            history_binding_count: self
                .history_owners
                .values()
                .filter(|owners| owners.len() == 1)
                .count(),
            conversation_count: self.by_conversation.len(),
            generation: self.generation,
        }
    }
}

fn binding_keys(state: &ConversationState) -> ConversationBindingKeys {
    let mut history = state
        .scan
        .frontier
        .binding
        .history
        .iter()
        .map(|binding| binding.agent_session_id.clone())
        .collect::<HashSet<_>>();
    let active = state
        .scan
        .frontier
        .binding
        .current
        .as_ref()
        .and_then(|binding| {
            history.insert(binding.agent_session_id.clone());
            (binding.state == AgentSessionBindingState::Active
                && state.record.lifecycle_state == ConversationLifecycleState::Ready)
                .then(|| binding.agent_session_id.clone())
        });
    ConversationBindingKeys { active, history }
}

fn unique_owner(owners: Option<&HashSet<ConversationId>>) -> Option<ConversationId> {
    let owners = owners?;
    (owners.len() == 1).then(|| *owners.iter().next().expect("owner length checked"))
}

fn remove_owner(
    owners: &mut HashMap<String, HashSet<ConversationId>>,
    agent_session_id: &str,
    conversation_id: ConversationId,
) {
    if let Some(conversations) = owners.get_mut(agent_session_id) {
        conversations.remove(&conversation_id);
        if conversations.is_empty() {
            owners.remove(agent_session_id);
        }
    }
}

pub const ACTIVE_TAIL_CACHE_MAX_CONVERSATIONS: usize = 8;
pub const ACTIVE_TAIL_CACHE_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
struct ActiveTailCacheEntry {
    after_seq: u64,
    range_end_seq: u64,
    limit: usize,
    scan_last_seq: u64,
    records: Vec<ConversationEventRecordV2>,
    retained_bytes: usize,
    last_used: u64,
}

#[derive(Debug, Default)]
struct ActiveTailCache {
    entries: HashMap<ConversationId, ActiveTailCacheEntry>,
    retained_bytes: usize,
    clock: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTailCacheStats {
    pub conversation_count: usize,
    pub retained_bytes: usize,
    pub conversation_ids: Vec<ConversationId>,
}

impl ActiveTailCache {
    fn get(
        &mut self,
        conversation_id: ConversationId,
        after_seq: u64,
        limit: usize,
        scan_last_seq: u64,
    ) -> Option<Vec<ConversationEventRecordV2>> {
        let entry = self.entries.get(&conversation_id)?;
        let expected_end = entry.records.last().map_or(after_seq, |record| record.seq);
        if entry.after_seq != after_seq
            || entry.range_end_seq != expected_end
            || entry.limit != limit
            || entry.scan_last_seq != scan_last_seq
            || entry
                .records
                .iter()
                .any(|record| record.conversation_id != conversation_id)
        {
            return None;
        }
        self.clock = self.clock.saturating_add(1);
        let entry = self
            .entries
            .get_mut(&conversation_id)
            .expect("validated cache entry remains present");
        entry.last_used = self.clock;
        Some(entry.records.clone())
    }

    fn insert_prevalidated(
        &mut self,
        conversation_id: ConversationId,
        after_seq: u64,
        limit: usize,
        scan_last_seq: u64,
        records: Vec<ConversationEventRecordV2>,
        retained_bytes: usize,
    ) {
        self.invalidate(conversation_id);
        while self.entries.len() >= ACTIVE_TAIL_CACHE_MAX_CONVERSATIONS
            || self.retained_bytes.saturating_add(retained_bytes) > ACTIVE_TAIL_CACHE_MAX_BYTES
        {
            let Some(lru_id) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(conversation_id, _)| *conversation_id)
            else {
                break;
            };
            self.invalidate(lru_id);
        }
        self.clock = self.clock.saturating_add(1);
        let range_end_seq = records.last().map_or(after_seq, |record| record.seq);
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.entries.insert(
            conversation_id,
            ActiveTailCacheEntry {
                after_seq,
                range_end_seq,
                limit,
                scan_last_seq,
                records,
                retained_bytes,
                last_used: self.clock,
            },
        );
    }

    fn invalidate(&mut self, conversation_id: ConversationId) {
        if let Some(entry) = self.entries.remove(&conversation_id) {
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }

    fn stats(&self) -> ActiveTailCacheStats {
        let mut conversation_ids = self.entries.keys().copied().collect::<Vec<_>>();
        conversation_ids.sort_by_key(ToString::to_string);
        ActiveTailCacheStats {
            conversation_count: conversation_ids.len(),
            retained_bytes: self.retained_bytes,
            conversation_ids,
        }
    }
}

fn encoded_page_bytes(records: &[ConversationEventRecordV2]) -> usize {
    let structural_bytes = records
        .len()
        .saturating_mul(std::mem::size_of::<ConversationEventRecordV2>());
    records.iter().fold(structural_bytes, |total, record| {
        let encoded_bytes =
            encoded_json_len_bounded(record, MAX_CONVERSATION_RECORD_BYTES).unwrap_or(usize::MAX);
        total
            .saturating_add(encoded_bytes)
            .saturating_add(value_heap_bytes(&record.payload))
    })
}

fn cache_retained_bytes(
    conversation_id: ConversationId,
    records: &[ConversationEventRecordV2],
) -> Option<usize> {
    if records.is_empty()
        || records
            .iter()
            .any(|record| record.conversation_id != conversation_id)
        || records.iter().any(event_contains_sensitive_cache_data)
    {
        return None;
    }
    let retained_bytes = encoded_page_bytes(records);
    (retained_bytes <= ACTIVE_TAIL_CACHE_MAX_BYTES).then_some(retained_bytes)
}

fn value_heap_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        Value::String(value) => value.capacity(),
        Value::Array(values) => values
            .capacity()
            .saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(values.iter().fold(0usize, |total, value| {
                total.saturating_add(value_heap_bytes(value))
            })),
        Value::Object(object) => object
            .len()
            .saturating_mul(
                std::mem::size_of::<String>()
                    .saturating_add(std::mem::size_of::<Value>())
                    .saturating_add(3 * std::mem::size_of::<usize>()),
            )
            .saturating_add(object.iter().fold(0usize, |total, (key, value)| {
                total
                    .saturating_add(key.capacity())
                    .saturating_add(value_heap_bytes(value))
            })),
    }
}

fn event_contains_sensitive_cache_data(record: &ConversationEventRecordV2) -> bool {
    fn contains_sensitive_key(value: &Value) -> bool {
        match value {
            Value::Object(object) => object.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("claim")
                    || key.eq_ignore_ascii_case("credentials")
                    || contains_sensitive_key(value)
            }),
            Value::Array(values) => values.iter().any(contains_sensitive_key),
            _ => false,
        }
    }

    contains_sensitive_key(&record.payload)
}

#[derive(Debug)]
pub struct RepositoryError {
    pub code: ConversationErrorCode,
    pub operation: &'static str,
    pub conversation_id: Option<ConversationId>,
    pub detail: String,
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}",
            self.stable_code(),
            self.operation
        )?;
        if let Some(conversation_id) = self.conversation_id {
            write!(formatter, " for Conversation {conversation_id}")?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl std::error::Error for RepositoryError {}

impl RepositoryError {
    #[must_use]
    pub fn stable_code(&self) -> String {
        if self.operation == "read_event_page"
            && self.code == ConversationErrorCode::ConversationInvalidId
        {
            "VALIDATION_ERROR".to_string()
        } else {
            stable_code(self.code)
        }
    }
}

pub type Result<T> = std::result::Result<T, RepositoryError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepositoryMode {
    Runtime,
    MigrationStaging,
}

pub struct ConversationRepository {
    instance_id: Uuid,
    mode: RepositoryMode,
    locator: ConversationLocator,
    durable_fs: DurableFileSystem,
    states: ParkingMutex<HashMap<ConversationId, ConversationState>>,
    recovery_by_id: ParkingMutex<HashMap<ConversationId, RepositoryRecoveryItem>>,
    recovery_items: ParkingMutex<Vec<RepositoryRecoveryItem>>,
    conversation_locks: ParkingMutex<HashMap<ConversationId, Arc<TokioMutex<()>>>>,
    active_tail_cache: ParkingMutex<ActiveTailCache>,
    binding_index: ParkingMutex<RepositoryBindingIndex>,
    catalog_flush: Arc<CatalogFlushCoordinator>,
    bootstrap_scanned_event_count: u64,
    bootstrap_sparse_index_entry_count: usize,
    bootstrap_duration_ms: u64,
    #[cfg(test)]
    fail_next_workspace_replace: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_agent_binding_appends_remaining: std::sync::atomic::AtomicUsize,
}

impl ConversationRepository {
    /// Open the canonical root, repair only torn final JSONL tails, recover stale creation state,
    /// and rewrite the disposable catalog from validated authoritative files.
    pub fn open(private_root: PathBuf) -> Result<(Arc<Self>, RepositoryOpenReport)> {
        Self::open_with_mode(private_root, RepositoryMode::Runtime)
    }

    /// Open the pre-admission migration target. Only MigrationWriter permits are accepted.
    pub(crate) fn open_staging(private_root: PathBuf) -> Result<(Arc<Self>, RepositoryOpenReport)> {
        Self::open_with_mode(private_root, RepositoryMode::MigrationStaging)
    }

    fn open_with_mode(
        private_root: PathBuf,
        mode: RepositoryMode,
    ) -> Result<(Arc<Self>, RepositoryOpenReport)> {
        let started_at = Instant::now();
        let durable_fs = DurableFileSystem::new();
        durable_fs
            .create_dir_durable(&private_root, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationDurabilityFailed,
                    "open",
                    None,
                    error.to_string(),
                )
            })?;
        let locator = ConversationLocator::new(private_root.clone()).map_err(|error| {
            repository_error(
                ConversationErrorCode::ConversationPathEscape,
                "open",
                None,
                error.to_string(),
            )
        })?;
        let catalog_path = private_root.join(CATALOG_FILE);
        let previous_catalog = fs::read(&catalog_path).ok();

        let mut rebuilt = rebuild_catalog(&locator, &durable_fs).map_err(|error| {
            repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "open",
                None,
                error.to_string(),
            )
        })?;
        let mut recovery_items = map_catalog_recovery(&rebuilt.recovery_issues);
        recovery_items.extend(map_repairs(&rebuilt.repairs));
        recovery_items.extend(scan_workspace_recovery(&rebuilt.accepted));

        let mut recovered_incomplete = false;
        let mut purged_deleted = false;
        rebuilt.accepted.retain(|accepted| {
            if accepted.record.lifecycle_state != ConversationLifecycleState::Deleted {
                return true;
            }
            purged_deleted = true;
            if let Err(error) = remove_conversation_directory(
                &accepted.directory,
                locator.root(),
                accepted.record.conversation_id,
            ) {
                log::warn!(
                    "[conversation-repository] leftover deleted conversation purge failed conversation_id={} error={error}",
                    accepted.record.conversation_id
                );
            } else {
                log::info!(
                    "[conversation-repository] leftover deleted conversation purged conversation_id={}",
                    accepted.record.conversation_id
                );
            }
            false
        });
        for accepted in &mut rebuilt.accepted {
            let mut changed = reconcile_metadata(accepted)?;
            if accepted.record.lifecycle_state == ConversationLifecycleState::InitializingAgent {
                accepted.record.lifecycle_state = ConversationLifecycleState::AgentFailed;
                changed = true;
                recovered_incomplete = true;
                log::warn!(
                    "[conversation-repository] incomplete creation recovered conversation_id={}",
                    accepted.record.conversation_id
                );
                recovery_items.push(RepositoryRecoveryItem {
                    code: ConversationErrorCode::ConversationCreateFailed,
                    kind: RepositoryRecoveryKind::IncompleteCreationRecovered,
                    conversation_id: Some(accepted.record.conversation_id),
                    relative_path: accepted.record.creation_partition.path.clone(),
                    detail: "stale initializing_agent state was closed to agent_failed".to_string(),
                    repaired: true,
                    requires_action: false,
                });
            }
            if changed {
                persist_metadata_at(&durable_fs, &accepted.directory, &accepted.record)?;
            }
        }

        if recovered_incomplete || purged_deleted {
            rebuilt = rebuild_catalog(&locator, &durable_fs).map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    "open",
                    None,
                    error.to_string(),
                )
            })?;
        }

        let catalog_bytes = rebuilt.catalog.deterministic_bytes();
        if previous_catalog
            .as_ref()
            .is_some_and(|previous| previous != &catalog_bytes)
        {
            log::warn!("[conversation-repository] stale or corrupt catalog ignored");
            recovery_items.push(RepositoryRecoveryItem {
                code: ConversationErrorCode::ConversationCorrupt,
                kind: RepositoryRecoveryKind::CatalogIgnored,
                conversation_id: None,
                relative_path: CATALOG_FILE.to_string(),
                detail: "catalog contents were ignored and deterministically rebuilt".to_string(),
                repaired: true,
                requires_action: false,
            });
        }
        if let Err(_error) = durable_fs.replace_bytes(&catalog_path, &catalog_bytes) {
            log::warn!(
                "[conversation-repository] cache rewrite failure code=CONVERSATION_DURABILITY_FAILED"
            );
            recovery_items.push(RepositoryRecoveryItem {
                code: ConversationErrorCode::ConversationDurabilityFailed,
                kind: RepositoryRecoveryKind::CatalogRewriteFailed,
                conversation_id: None,
                relative_path: CATALOG_FILE.to_string(),
                detail: "canonical data opened but catalog cache rewrite failed".to_string(),
                repaired: false,
                requires_action: false,
            });
        }

        let catalog = ConversationCatalog::from_file(rebuilt.catalog.clone());
        recovery_items.sort_by(|left, right| {
            left.relative_path.cmp(&right.relative_path).then_with(|| {
                left.conversation_id
                    .map(|id| id.to_string())
                    .cmp(&right.conversation_id.map(|id| id.to_string()))
            })
        });
        let scan_metrics =
            bootstrap_scan_metrics(rebuilt.accepted.iter().map(|accepted| &accepted.scan));
        let states = rebuilt
            .accepted
            .into_iter()
            .map(|accepted| {
                (
                    accepted.record.conversation_id,
                    ConversationState {
                        record: accepted.record,
                        scan: Arc::new(accepted.scan),
                        provenance: accepted.provenance,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let binding_index = RepositoryBindingIndex::from_states(&states);
        let recovery_by_id = recovery_items
            .iter()
            .filter(|item| item.requires_action)
            .filter_map(|item| item.conversation_id.map(|id| (id, item.clone())))
            .collect::<HashMap<_, _>>();
        let catalog_flush =
            CatalogFlushCoordinator::new(catalog, durable_fs.clone(), catalog_path.clone());
        let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let report = RepositoryOpenReport {
            valid_conversation_count: states.len(),
            recovery_items: recovery_items.clone(),
            scanned_event_count: scan_metrics.scanned_event_count,
            sparse_index_entry_count: scan_metrics.sparse_index_entry_count,
            retained_payload_bytes: scan_metrics.retained_payload_bytes,
            duration_ms,
        };
        let repository = Arc::new(Self {
            instance_id: Uuid::new_v4(),
            mode,
            locator,
            durable_fs,
            states: ParkingMutex::new(states),
            recovery_by_id: ParkingMutex::new(recovery_by_id),
            recovery_items: ParkingMutex::new(recovery_items),
            conversation_locks: ParkingMutex::new(HashMap::new()),
            active_tail_cache: ParkingMutex::new(ActiveTailCache::default()),
            binding_index: ParkingMutex::new(binding_index),
            catalog_flush,
            bootstrap_scanned_event_count: report.scanned_event_count,
            bootstrap_sparse_index_entry_count: report.sparse_index_entry_count,
            bootstrap_duration_ms: report.duration_ms,
            #[cfg(test)]
            fail_next_workspace_replace: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_agent_binding_appends_remaining: std::sync::atomic::AtomicUsize::new(0),
        });
        log::info!(
            "[conversation-repository] open complete valid_count={} recovery_item_count={} scanned_event_count={} sparse_index_entry_count={} retained_payload_bytes={} duration_ms={}",
            report.valid_conversation_count,
            report.recovery_items.len(),
            report.scanned_event_count,
            report.sparse_index_entry_count,
            report.retained_payload_bytes,
            report.duration_ms
        );
        Ok((repository, report))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.locator.root()
    }

    #[must_use]
    pub(crate) const fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    #[must_use]
    pub(crate) const fn is_staging(&self) -> bool {
        matches!(self.mode, RepositoryMode::MigrationStaging)
    }

    pub(crate) fn workspace_path(&self, conversation_id: ConversationId) -> Result<PathBuf> {
        // Workspace corruption is surfaced by SessionWorkspaceService itself. Do not let the
        // repository's workspace-only recovery report hide the preserved bytes from that service.
        let record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| {
                repository_error(
                    ConversationErrorCode::ConversationNotFound,
                    "workspace_path",
                    Some(conversation_id),
                    "canonical Conversation was not found".to_string(),
                )
            })?;
        Ok(self
            .conversation_dir(&record, "workspace_path")?
            .join("workspace.json"))
    }

    pub(crate) fn read_workspace_bytes(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<Vec<u8>>> {
        let path = self.workspace_path(conversation_id)?;
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "read_workspace",
                Some(conversation_id),
                error.to_string(),
            )),
        }
    }

    pub(crate) fn replace_workspace_bytes(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        bytes: &[u8],
    ) -> Result<()> {
        self.validate_write_permit(permit, conversation_id, "replace_workspace")?;
        #[cfg(test)]
        if self
            .fail_next_workspace_replace
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(repository_error(
                ConversationErrorCode::ConversationDurabilityFailed,
                "replace_workspace",
                Some(conversation_id),
                "injected workspace replacement failure".to_string(),
            ));
        }
        let path = self.workspace_path(conversation_id)?;
        self.durable_fs
            .replace_bytes(&path, bytes)
            .map_err(|error| durability_error("replace_workspace", conversation_id, error))?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_workspace_replace(&self) {
        self.fail_next_workspace_replace
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_agent_binding_appends(&self, count: usize) {
        self.fail_agent_binding_appends_remaining
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn workspace_lock(&self, conversation_id: ConversationId) -> Arc<TokioMutex<()>> {
        self.conversation_lock(conversation_id)
    }

    #[must_use]
    pub fn recovery_report(&self) -> RepositoryOpenReport {
        RepositoryOpenReport {
            valid_conversation_count: self.states.lock().len(),
            recovery_items: self.recovery_items.lock().clone(),
            scanned_event_count: self.bootstrap_scanned_event_count,
            sparse_index_entry_count: self.bootstrap_sparse_index_entry_count,
            retained_payload_bytes: 0,
            duration_ms: self.bootstrap_duration_ms,
        }
    }

    #[must_use]
    pub const fn retained_payload_bytes(&self) -> usize {
        0
    }

    #[must_use]
    pub fn sparse_index_entry_count(&self) -> usize {
        self.states
            .lock()
            .values()
            .map(|state| state.scan.sparse_index_entry_count())
            .sum()
    }

    #[must_use]
    pub fn active_tail_cache_stats(&self) -> ActiveTailCacheStats {
        self.active_tail_cache.lock().stats()
    }

    #[must_use]
    pub fn catalog_flush_coordinator(&self) -> Arc<CatalogFlushCoordinator> {
        Arc::clone(&self.catalog_flush)
    }

    pub async fn flush_catalog_until(
        &self,
        deadline: TokioInstant,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        self.catalog_flush.flush_until(deadline).await
    }

    #[must_use]
    pub fn binding_generation(&self) -> u64 {
        self.binding_index.lock().generation
    }

    #[must_use]
    pub fn binding_index_stats(&self) -> RepositoryBindingIndexStats {
        self.binding_index.lock().stats()
    }

    #[must_use]
    pub fn conversation_id_for_active_binding(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.binding_index.lock().resolve_active(agent_session_id)
    }

    #[must_use]
    pub fn conversation_id_for_history_binding(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.binding_index.lock().resolve_history(agent_session_id)
    }

    pub fn refresh_binding_index_hint(&self, conversation_id: ConversationId) -> Option<u64> {
        self.refresh_binding_index_for_conversation(conversation_id)
    }

    #[cfg(test)]
    pub(crate) fn reset_catalog_write_counters(&self) {
        self.catalog_flush.reset_write_counters();
    }

    #[cfg(test)]
    pub(crate) fn catalog_write_count(&self) -> u64 {
        self.catalog_flush.durable_catalog_write_count()
    }

    #[cfg(test)]
    pub(crate) fn fail_next_catalog_writes(&self, count: usize) {
        self.catalog_flush.fail_next_writes(count);
    }

    #[cfg(test)]
    pub(crate) fn catalog_pending_generation(&self) -> u64 {
        self.catalog_flush.pending_generation()
    }

    #[cfg(test)]
    pub(crate) fn catalog_last_admission_metrics(&self) -> CatalogAdmissionMetrics {
        self.catalog_flush.last_admission_metrics()
    }

    #[cfg(test)]
    pub(crate) async fn flush_catalog_once_for_test(
        &self,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        self.catalog_flush.flush_once_for_test().await
    }

    #[cfg(test)]
    pub(crate) fn event_log_scan(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Arc<EventLogScan>> {
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| Arc::clone(&state.scan))
            .ok_or_else(|| not_found("event_log_scan", conversation_id))
    }

    pub(crate) async fn create_conversation(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        record: ConversationRecordV2,
    ) -> Result<ConversationRecordV2> {
        self.validate_write_permit(permit, record.conversation_id, "create_conversation")?;
        validate_new_record(&record)?;
        let lock = self.conversation_lock(record.conversation_id);
        let _guard = lock.lock().await;
        if self.states.lock().contains_key(&record.conversation_id)
            || self
                .recovery_by_id
                .lock()
                .contains_key(&record.conversation_id)
        {
            return Err(repository_error(
                ConversationErrorCode::ConversationCreateFailed,
                "create_conversation",
                Some(record.conversation_id),
                "ConversationId already exists".to_string(),
            ));
        }
        let directory = self
            .locator
            .private_dir(record.conversation_id, &record.creation_partition)
            .map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationPathEscape,
                    "create_conversation",
                    Some(record.conversation_id),
                    error.to_string(),
                )
            })?;
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(repository_error(
                ConversationErrorCode::ConversationCreateFailed,
                "create_conversation",
                Some(record.conversation_id),
                "canonical directory already exists".to_string(),
            ));
        }
        self.durable_fs
            .create_dir_durable(&directory, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(|error| {
                durability_error("create_conversation", record.conversation_id, error)
            })?;
        // Metadata is authoritative and durable before any stream exists or event can be appended.
        persist_metadata_at(&self.durable_fs, &directory, &record)?;
        for file in EVENT_LOG_FILES {
            self.durable_fs
                .replace_bytes(&directory.join(file), b"")
                .map_err(|error| {
                    durability_error("create_conversation", record.conversation_id, error)
                })?;
        }
        let state = ConversationState {
            record: record.clone(),
            scan: Arc::new(EventLogScan::default()),
            provenance: None,
        };
        self.states.lock().insert(record.conversation_id, state);
        drop(_guard);
        self.mark_catalog_entry_dirty(record.conversation_id);
        log::info!(
            "[conversation-repository] conversation created conversation_id={}",
            record.conversation_id
        );
        Ok(record)
    }

    pub fn get_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        if let Some(item) = self.recovery_by_id.lock().get(&conversation_id).cloned() {
            return Err(repository_error(
                item.code,
                "get_conversation",
                Some(conversation_id),
                item.detail,
            ));
        }
        let mut record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| {
                repository_error(
                    ConversationErrorCode::ConversationNotFound,
                    "get_conversation",
                    Some(conversation_id),
                    "canonical Conversation was not found".to_string(),
                )
            })?;
        if record.title.is_none() {
            if let Ok(summary) = self.history_summary(conversation_id) {
                record.title = summary.title;
                record.title_source = summary.title_source;
            }
        }
        Ok(record)
    }

    #[must_use]
    pub fn list_conversations(&self) -> Vec<ConversationRecordV2> {
        let mut records = self
            .states
            .lock()
            .values()
            .map(|state| state.record.clone())
            .collect::<Vec<_>>();
        // Untitled records fall back to the event-log derived title so lists
        // show the first-question prefix instead of opaque ids.
        for record in records.iter_mut() {
            if record.title.is_none() {
                if let Ok(summary) = self.history_summary(record.conversation_id) {
                    record.title = summary.title;
                    record.title_source = summary.title_source;
                }
            }
        }
        records.sort_by_key(|record| record.conversation_id.to_string());
        records
    }

    pub(crate) async fn update_metadata(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        update: ConversationMetadataUpdate,
    ) -> Result<ConversationRecordV2> {
        self.validate_write_permit(permit, conversation_id, "update_metadata")?;
        let lock = self.conversation_lock(conversation_id);
        let _guard = lock.lock().await;
        self.check_recovery(conversation_id, "update_metadata")?;
        let mut record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found("update_metadata", conversation_id))?;
        if let Some(lifecycle_state) = update.lifecycle_state {
            record.lifecycle_state = lifecycle_state;
        }
        if let Some(execution_target) = update.execution_target {
            record.execution_target = execution_target;
        }
        if let Some(title) = update.title {
            record.title = Some(title);
        }
        if let Some(title_source) = update.title_source {
            record.title_source = Some(title_source);
        }
        self.persist_record_metadata(&record, "update_metadata")?;
        self.states
            .lock()
            .get_mut(&conversation_id)
            .expect("per-Conversation lock preserves state")
            .record = record.clone();
        self.refresh_binding_index_for_conversation(conversation_id);
        drop(_guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(record)
    }

    pub(crate) async fn append_event(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "append_event")?;
        let lock = self.conversation_lock(conversation_id);
        let guard = lock.lock().await;
        let event = self.append_event_locked(conversation_id, recorded_at_utc, type_, payload)?;
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(event)
    }

    /// Append one coordinator-owned event. The canonical sequence is allocated inside the
    /// per-Conversation lock by [`Self::append_event_locked`]. A stale reserved `expected_seq`
    /// returns retryable [`ConversationErrorCode::ConversationConflict`] so the caller can
    /// re-reserve; it never maps to sequence-invalid, frontier-mismatch, or recovery-required.
    pub(crate) async fn append_ordered_event(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        expected_seq: u64,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "append_ordered_event")?;
        let lock = self.conversation_lock(conversation_id);
        let guard = lock.lock().await;
        let current = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.last_seq)
            .ok_or_else(|| not_found("append_ordered_event", conversation_id))?;
        let allocated = next_canonical_seq(current, conversation_id, "append_ordered_event")?;
        if expected_seq != allocated {
            return Err(repository_error(
                ConversationErrorCode::ConversationConflict,
                "append_ordered_event",
                Some(conversation_id),
                format!("stale reserved seq {expected_seq}, allocated next is {allocated}"),
            ));
        }
        let event = self.append_event_locked(conversation_id, recorded_at_utc, type_, payload)?;
        debug_assert_eq!(event.seq, allocated);
        debug_assert_eq!(CanonicalSequenceTicket::from_event(&event).seq, event.seq);
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(event)
    }

    pub fn read_event_page(
        &self,
        conversation_id: ConversationId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<ConversationEventRecordV2>> {
        if !(MIN_EVENT_PAGE_LIMIT..=MAX_EVENT_PAGE_LIMIT).contains(&limit) {
            return Err(repository_error(
                ConversationErrorCode::ConversationInvalidId,
                "read_event_page",
                Some(conversation_id),
                format!(
                    "event page limit must be between {MIN_EVENT_PAGE_LIMIT} and {MAX_EVENT_PAGE_LIMIT}"
                ),
            ));
        }
        self.check_recovery(conversation_id, "read_event_page")?;
        let (record, scan) = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| (state.record.clone(), Arc::clone(&state.scan)))
            .ok_or_else(|| not_found("read_event_page", conversation_id))?;
        if record.last_seq != scan.last_seq() {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "read_event_page",
                Some(conversation_id),
                "metadata lastSeq does not match the validated event-log frontier".to_string(),
            ));
        }
        if after_seq > record.last_seq {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "read_event_page",
                Some(conversation_id),
                format!("cursor {after_seq} is ahead of lastSeq {}", record.last_seq),
            ));
        }
        if let Some(records) =
            self.active_tail_cache
                .lock()
                .get(conversation_id, after_seq, limit, scan.last_seq())
        {
            return Ok(records);
        }
        let directory = self.conversation_dir(&record, "read_event_page")?;
        let records = read_event_page_from_log(
            &directory,
            conversation_id,
            scan.as_ref(),
            after_seq,
            limit,
        )
        .map_err(|source| {
            log::error!(
                "[conversation-repository] event page rejected code={} conversation_id={} stream_file={} seq={} offset={}",
                source.stable_code(),
                conversation_id,
                source
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown"),
                source.seq.unwrap_or(after_seq),
                source
                    .byte_offset
                    .map_or_else(|| "unknown".to_string(), |offset| offset.to_string())
            );
            repository_error(
                source.code,
                "read_event_page",
                Some(conversation_id),
                source.detail,
            )
        })?;
        let scan_is_current = self
            .states
            .lock()
            .get(&conversation_id)
            .is_some_and(|state| state.scan.last_seq() == scan.last_seq());
        if scan_is_current {
            // Measure the owned page before cloning it into the disposable cache. Oversized or
            // sensitive pages return directly without the prior memory-amplifying deep clone.
            if let Some(retained_bytes) = cache_retained_bytes(conversation_id, &records) {
                self.active_tail_cache.lock().insert_prevalidated(
                    conversation_id,
                    after_seq,
                    limit,
                    scan.last_seq(),
                    records.clone(),
                    retained_bytes,
                );
            }
        }
        Ok(records)
    }

    /// Traverse canonical JSONL on Tokio's dedicated blocking pool. Async Tauri/HTTP/WS adapters
    /// use this entry point so bounded filesystem reads never monopolize a runtime worker.
    pub async fn read_event_page_blocking(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<ConversationEventRecordV2>> {
        let repository = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            repository.read_event_page(conversation_id, after_seq, limit)
        })
        .await
        .map_err(|_| {
            repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "read_event_page",
                Some(conversation_id),
                "blocking history traversal task failed".to_string(),
            )
        })?
    }

    /// Compatibility full-history wrapper implemented exclusively through bounded pages.
    pub fn read_events(
        &self,
        conversation_id: ConversationId,
        after_seq: u64,
    ) -> Result<Vec<ConversationEventRecordV2>> {
        self.check_recovery(conversation_id, "read_events")?;
        let target_last_seq = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.last_seq)
            .ok_or_else(|| not_found("read_events", conversation_id))?;
        if after_seq > target_last_seq {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "read_events",
                Some(conversation_id),
                format!("cursor {after_seq} is ahead of lastSeq {target_last_seq}"),
            ));
        }
        let mut cursor = after_seq;
        let mut records = Vec::new();
        while cursor < target_last_seq {
            let page = self.read_event_page(conversation_id, cursor, MAX_EVENT_PAGE_LIMIT)?;
            let page = page
                .into_iter()
                .take_while(|record| record.seq <= target_last_seq)
                .collect::<Vec<_>>();
            let Some(last_seq) = page.last().map(|record| record.seq) else {
                return Err(repository_error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    "read_events",
                    Some(conversation_id),
                    format!(
                        "paged compatibility read stopped at seq {cursor} before lastSeq {target_last_seq}"
                    ),
                ));
            };
            cursor = last_seq;
            records.extend(page);
        }
        Ok(records)
    }

    pub fn current_binding(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<AgentSessionBinding>> {
        self.check_recovery(conversation_id, "current_binding")?;
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| state.scan.frontier.binding.current.clone())
            .ok_or_else(|| not_found("current_binding", conversation_id))
    }

    pub fn binding_history(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<AgentSessionBinding>> {
        self.check_recovery(conversation_id, "binding_history")?;
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| state.scan.frontier.binding.history.to_vec())
            .ok_or_else(|| not_found("binding_history", conversation_id))
    }

    pub fn history_summary(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationHistorySummaryV1> {
        self.check_recovery(conversation_id, "history_summary")?;
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| ConversationHistorySummaryV1 {
                conversation_id,
                title: state.scan.frontier.summary.title.clone(),
                title_source: state.scan.frontier.summary.title_source,
                last_activity_at_utc: state
                    .scan
                    .frontier
                    .summary
                    .last_activity_at_utc
                    .map_or(state.record.created_at_utc, |event_time| {
                        event_time.max(state.record.created_at_utc)
                    }),
                message_count: state.scan.frontier.summary.message_count,
                tool_count: state.scan.frontier.summary.tool_count,
            })
            .ok_or_else(|| not_found("history_summary", conversation_id))
    }

    pub fn conversation_frontier(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationFrontier> {
        self.check_recovery(conversation_id, "conversation_frontier")?;
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| state.scan.frontier.clone())
            .ok_or_else(|| not_found("conversation_frontier", conversation_id))
    }

    pub(crate) async fn bind_agent_session(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        mut binding: AgentSessionBinding,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "bind_agent_session")?;
        binding.state = AgentSessionBindingState::Active;
        validate_binding_input(&binding, conversation_id, "bind_agent_session")?;
        self.append_event(
            permit,
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingBound,
            serde_json::to_value(BindingEventPayloadV1 { binding }).map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationBindFailed,
                    "bind_agent_session",
                    Some(conversation_id),
                    error.to_string(),
                )
            })?,
        )
        .await
    }

    // Retained for the authority facade and focused repository tests; lifecycle production flows
    // use the locked compound operation so revision checks and provider effects stay atomic.
    #[allow(dead_code)]
    pub(crate) async fn detach_agent_binding(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "detach_agent_binding")?;
        let guard = self.lifecycle_lock(conversation_id).await;
        let event = self.detach_agent_binding_locked(permit, conversation_id, recorded_at_utc)?;
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(event)
    }

    #[allow(dead_code)]
    pub(crate) async fn rebind_detached_binding(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "rebind_detached_binding")?;
        let guard = self.lifecycle_lock(conversation_id).await;
        let event =
            self.rebind_detached_binding_locked(permit, conversation_id, recorded_at_utc)?;
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(event)
    }

    /// Record suspension only after the provider confirms close/suspend success.
    /// A false confirmation performs no append and leaves materialized state unchanged.
    #[allow(dead_code)]
    pub(crate) async fn suspend_agent_binding(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        provider_confirmed: bool,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<Option<ConversationEventRecordV2>> {
        self.validate_write_permit(permit, conversation_id, "suspend_agent_binding")?;
        if !provider_confirmed {
            return Ok(None);
        }
        let guard = self.lifecycle_lock(conversation_id).await;
        let event = self.suspend_agent_binding_locked(permit, conversation_id, recorded_at_utc)?;
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(Some(event))
    }

    pub(crate) async fn replace_agent_binding(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        binding: AgentSessionBinding,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "replace_agent_binding")?;
        let guard = self.lifecycle_lock(conversation_id).await;
        let event =
            self.replace_agent_binding_locked(permit, conversation_id, binding, recorded_at_utc)?;
        drop(guard);
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(event)
    }

    pub(crate) async fn lifecycle_lock(
        &self,
        conversation_id: ConversationId,
    ) -> OwnedMutexGuard<()> {
        self.conversation_lock(conversation_id).lock_owned().await
    }

    pub(crate) fn ensure_expected_revision_locked(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
        operation: &'static str,
    ) -> Result<ConversationRecordV2> {
        self.check_recovery(conversation_id, operation)?;
        let record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found(operation, conversation_id))?;
        if record.last_seq != expected_revision {
            log::warn!(
                "[conversation-repository] stale lifecycle revision conversation_id={} expected_revision={} current_revision={}",
                conversation_id,
                expected_revision,
                record.last_seq
            );
            return Err(repository_error(
                ConversationErrorCode::ConversationConflict,
                operation,
                Some(conversation_id),
                format!(
                    "expected Conversation lastSeq {expected_revision}, current lastSeq is {}",
                    record.last_seq
                ),
            ));
        }
        Ok(record)
    }

    pub(crate) fn detach_agent_binding_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "detach_agent_binding")?;
        let mut binding = self.current_binding(conversation_id)?.ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ConversationBindingNotFound,
                "detach_agent_binding",
                Some(conversation_id),
                "no current binding".to_string(),
            )
        })?;
        if binding.state != AgentSessionBindingState::Active {
            return Err(repository_error(
                ConversationErrorCode::ConversationBindingNotActive,
                "detach_agent_binding",
                Some(conversation_id),
                "detach requires the current active binding".to_string(),
            ));
        }
        binding.state = AgentSessionBindingState::Detached;
        self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingDetached,
            serde_json::to_value(BindingEventPayloadV1 { binding }).expect("binding serializes"),
        )
    }

    pub(crate) fn rebind_detached_binding_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "rebind_detached_binding")?;
        let mut binding = self.current_binding(conversation_id)?.ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ConversationBindingNotFound,
                "rebind_detached_binding",
                Some(conversation_id),
                "no current binding".to_string(),
            )
        })?;
        if binding.state != AgentSessionBindingState::Detached {
            return Err(repository_error(
                ConversationErrorCode::ConversationBindingNotDetached,
                "rebind_detached_binding",
                Some(conversation_id),
                "rebind requires the current detached binding".to_string(),
            ));
        }
        binding.state = AgentSessionBindingState::Active;
        self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingRebound,
            serde_json::to_value(BindingEventPayloadV1 { binding }).expect("binding serializes"),
        )
    }

    pub(crate) fn suspend_agent_binding_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "suspend_agent_binding")?;
        let mut binding = self.current_binding(conversation_id)?.ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ConversationBindingNotFound,
                "suspend_agent_binding",
                Some(conversation_id),
                "no current binding".to_string(),
            )
        })?;
        if binding.state != AgentSessionBindingState::Active {
            return Err(repository_error(
                ConversationErrorCode::ConversationBindingNotActive,
                "suspend_agent_binding",
                Some(conversation_id),
                "suspend requires the current active binding".to_string(),
            ));
        }
        binding.state = AgentSessionBindingState::Suspended;
        self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingSuspended,
            serde_json::to_value(BindingEventPayloadV1 { binding }).expect("binding serializes"),
        )
    }

    /// Delete may release Active or Detached bindings after a best-effort ACP close.
    pub(crate) fn release_binding_for_delete_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<Option<ConversationEventRecordV2>> {
        self.validate_write_permit(permit, conversation_id, "release_binding_for_delete")?;
        let Some(mut binding) = self.current_binding(conversation_id)? else {
            return Ok(None);
        };
        if !matches!(
            binding.state,
            AgentSessionBindingState::Active | AgentSessionBindingState::Detached
        ) {
            return Ok(None);
        }
        binding.state = AgentSessionBindingState::Suspended;
        Ok(Some(self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingSuspended,
            serde_json::to_value(BindingEventPayloadV1 { binding }).expect("binding serializes"),
        )?))
    }

    pub(crate) fn replace_agent_binding_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        mut binding: AgentSessionBinding,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "replace_agent_binding")?;
        validate_binding_input(&binding, conversation_id, "replace_agent_binding")?;
        let mut previous_binding = self.current_binding(conversation_id)?.ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ConversationBindingNotFound,
                "replace_agent_binding",
                Some(conversation_id),
                "no binding exists to replace".to_string(),
            )
        })?;
        previous_binding.state = AgentSessionBindingState::Replaced;
        binding.state = AgentSessionBindingState::Active;
        let event = self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::BindingReplaced,
            serde_json::to_value(BindingReplacementPayloadV1 {
                previous_binding,
                binding,
            })
            .expect("binding replacement serializes"),
        )?;
        log::info!(
            "[conversation-repository] binding replaced conversation_id={}",
            conversation_id
        );
        Ok(event)
    }

    pub(crate) async fn refresh_lifecycle_catalog(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<()> {
        self.validate_write_permit(permit, conversation_id, "refresh_lifecycle_catalog")?;
        // Mutation success is not published until its immutable catalog generation is admitted.
        // Only later serialization/replacement remains asynchronous and coalesced.
        self.mark_catalog_entry_dirty(conversation_id);
        Ok(())
    }

    pub(crate) async fn append_project_attachment(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        attachment: ProjectAttachment,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "append_project_attachment")?;
        if attachment.schema_version != PROJECT_ATTACHMENT_SCHEMA_VERSION {
            return Err(repository_error(
                ConversationErrorCode::ConversationUnsupportedSchema,
                "append_project_attachment",
                Some(conversation_id),
                "unsupported project attachment schemaVersion".to_string(),
            ));
        }
        if self
            .get_conversation(conversation_id)?
            .project_attachment
            .is_some()
        {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "append_project_attachment",
                Some(conversation_id),
                "a project attachment is already materialized".to_string(),
            ));
        }
        let event = self
            .append_event(
                permit,
                conversation_id,
                recorded_at_utc,
                ConversationEventType::ProjectAttached,
                serde_json::to_value(ProjectAttachmentEventPayloadV1 { attachment }).unwrap(),
            )
            .await?;
        log::info!(
            "[conversation-repository] project attachment changed conversation_id={} action=attach",
            conversation_id
        );
        Ok(event)
    }

    // Published through ConversationWriter as part of the exhaustive mutation authority even
    // though no current production transport exposes detach yet.
    #[allow(dead_code)]
    pub(crate) async fn detach_project_attachment(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationEventRecordV2> {
        self.validate_write_permit(permit, conversation_id, "detach_project_attachment")?;
        let attachment = self
            .get_conversation(conversation_id)?
            .project_attachment
            .ok_or_else(|| {
                repository_error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    "detach_project_attachment",
                    Some(conversation_id),
                    "no project attachment is materialized".to_string(),
                )
            })?;
        let event = self
            .append_event(
                permit,
                conversation_id,
                recorded_at_utc,
                ConversationEventType::ProjectDetached,
                serde_json::to_value(ProjectAttachmentEventPayloadV1 { attachment }).unwrap(),
            )
            .await?;
        log::info!(
            "[conversation-repository] project attachment changed conversation_id={} action=detach",
            conversation_id
        );
        Ok(event)
    }

    pub(crate) async fn attach_project_cas(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        expected_revision: u64,
        attachment: ProjectAttachment,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationAggregateMutationRecord> {
        self.validate_write_permit(permit, conversation_id, "attach_project")?;
        let _guard = self.lifecycle_lock(conversation_id).await;
        let before = self.ensure_expected_revision_locked(
            conversation_id,
            expected_revision,
            "attach_project",
        )?;
        ensure_ready_aggregate(&before, "attach_project")?;
        validate_project_attachment_input(&attachment, conversation_id, "attach_project")?;
        validate_execution_target_input(
            &before.execution_target,
            Some(&attachment),
            conversation_id,
            "attach_project",
        )?;
        if before.project_attachment.is_some() {
            return Err(repository_error(
                ConversationErrorCode::ConversationConflict,
                "attach_project",
                Some(conversation_id),
                "Conversation already has a project attachment".to_string(),
            ));
        }
        let event = self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::ProjectAttached,
            serde_json::to_value(ProjectAttachmentEventPayloadV1 { attachment }).map_err(
                |error| {
                    repository_error(
                        ConversationErrorCode::ValidationError,
                        "attach_project",
                        Some(conversation_id),
                        error.to_string(),
                    )
                },
            )?,
        )?;
        let after = self.aggregate_after(&before, "attach_project")?;
        drop(_guard);
        self.mark_catalog_entry_dirty(conversation_id);
        log::info!(
            "[conversation-repository] aggregate mutation operation=attach_project conversation_id={} previous_revision={} revision={} code=OK",
            conversation_id,
            before.last_seq,
            after.last_seq
        );
        Ok(ConversationAggregateMutationRecord {
            before,
            after,
            event,
        })
    }

    pub(crate) async fn detach_project_cas(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        expected_revision: u64,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationAggregateMutationRecord> {
        self.validate_write_permit(permit, conversation_id, "detach_project")?;
        let _guard = self.lifecycle_lock(conversation_id).await;
        let before = self.ensure_expected_revision_locked(
            conversation_id,
            expected_revision,
            "detach_project",
        )?;
        ensure_ready_aggregate(&before, "detach_project")?;
        if !matches!(before.execution_target, ExecutionTarget::Workspace) {
            return Err(repository_error(
                ConversationErrorCode::ValidationError,
                "detach_project",
                Some(conversation_id),
                "switch executionTarget to workspace before detaching its project".to_string(),
            ));
        }
        let attachment = before.project_attachment.clone().ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ConversationConflict,
                "detach_project",
                Some(conversation_id),
                "Conversation has no project attachment".to_string(),
            )
        })?;
        let event = self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::ProjectDetached,
            serde_json::to_value(ProjectAttachmentEventPayloadV1 { attachment }).map_err(
                |error| {
                    repository_error(
                        ConversationErrorCode::ValidationError,
                        "detach_project",
                        Some(conversation_id),
                        error.to_string(),
                    )
                },
            )?,
        )?;
        let after = self.aggregate_after(&before, "detach_project")?;
        drop(_guard);
        self.mark_catalog_entry_dirty(conversation_id);
        log::info!(
            "[conversation-repository] aggregate mutation operation=detach_project conversation_id={} previous_revision={} revision={} code=OK",
            conversation_id,
            before.last_seq,
            after.last_seq
        );
        Ok(ConversationAggregateMutationRecord {
            before,
            after,
            event,
        })
    }

    pub(crate) async fn update_execution_target_cas(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        expected_revision: u64,
        execution_target: ExecutionTarget,
        recorded_at_utc: DateTime<Utc>,
    ) -> Result<ConversationAggregateMutationRecord> {
        self.validate_write_permit(permit, conversation_id, "update_execution_target")?;
        let _guard = self.lifecycle_lock(conversation_id).await;
        let before = self.ensure_expected_revision_locked(
            conversation_id,
            expected_revision,
            "update_execution_target",
        )?;
        ensure_ready_aggregate(&before, "update_execution_target")?;
        validate_execution_target_input(
            &execution_target,
            before.project_attachment.as_ref(),
            conversation_id,
            "update_execution_target",
        )?;
        if before.execution_target == execution_target {
            return Err(repository_error(
                ConversationErrorCode::ConversationConflict,
                "update_execution_target",
                Some(conversation_id),
                "executionTarget is already current".to_string(),
            ));
        }
        let event = self.append_event_locked(
            conversation_id,
            recorded_at_utc,
            ConversationEventType::ExecutionTargetUpdated,
            serde_json::to_value(ExecutionTargetEventPayloadV1 { execution_target }).map_err(
                |error| {
                    repository_error(
                        ConversationErrorCode::ValidationError,
                        "update_execution_target",
                        Some(conversation_id),
                        error.to_string(),
                    )
                },
            )?,
        )?;
        let after = self.aggregate_after(&before, "update_execution_target")?;
        drop(_guard);
        self.mark_catalog_entry_dirty(conversation_id);
        log::info!(
            "[conversation-repository] aggregate mutation operation=update_execution_target conversation_id={} previous_revision={} revision={} target_kind={} code=OK",
            conversation_id,
            before.last_seq,
            after.last_seq,
            execution_target_kind(&after.execution_target)
        );
        Ok(ConversationAggregateMutationRecord {
            before,
            after,
            event,
        })
    }

    fn aggregate_after(
        &self,
        before: &ConversationRecordV2,
        operation: &'static str,
    ) -> Result<ConversationRecordV2> {
        let after = self.get_conversation(before.conversation_id)?;
        if immutable_identity(before) != immutable_identity(&after) {
            log::error!(
                "[conversation-repository] immutable aggregate identity changed operation={} conversation_id={} code=CONVERSATION_RECOVERY_REQUIRED",
                operation,
                before.conversation_id
            );
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                operation,
                Some(before.conversation_id),
                "aggregate mutation changed immutable Conversation identity".to_string(),
            ));
        }
        Ok(after)
    }

    pub(crate) async fn write_provenance(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        provenance: ConversationProvenanceFileV1,
    ) -> Result<()> {
        self.validate_write_permit(permit, conversation_id, "write_provenance")?;
        provenance.validate().map_err(|detail| {
            repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "write_provenance",
                Some(conversation_id),
                detail.to_string(),
            )
        })?;
        let lock = self.conversation_lock(conversation_id);
        let _guard = lock.lock().await;
        self.check_recovery(conversation_id, "write_provenance")?;
        let record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found("write_provenance", conversation_id))?;
        let directory = self.conversation_dir(&record, "write_provenance")?;
        let path = directory.join(PROVENANCE_FILE);
        let mut bytes = serde_json::to_vec_pretty(&provenance).map_err(|error| {
            repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "write_provenance",
                Some(conversation_id),
                error.to_string(),
            )
        })?;
        bytes.push(b'\n');
        if let Ok(existing) = fs::read(&path) {
            if existing == bytes {
                return Ok(());
            }
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "write_provenance",
                Some(conversation_id),
                "immutable provenance already exists with different bytes".to_string(),
            ));
        }
        self.durable_fs
            .replace_bytes(&path, &bytes)
            .map_err(|error| durability_error("write_provenance", conversation_id, error))?;
        self.states
            .lock()
            .get_mut(&conversation_id)
            .expect("per-Conversation lock preserves state")
            .provenance = Some(provenance);
        Ok(())
    }

    pub fn read_provenance(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ConversationProvenanceFileV1>> {
        self.check_recovery(conversation_id, "read_provenance")?;
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| state.provenance.clone())
            .ok_or_else(|| not_found("read_provenance", conversation_id))
    }

    pub(crate) async fn sync_conversation(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<()> {
        self.validate_write_permit(permit, conversation_id, "sync_conversation")?;
        let lock = self.conversation_lock(conversation_id);
        let _guard = lock.lock().await;
        self.check_recovery(conversation_id, "sync_conversation")?;
        let record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found("sync_conversation", conversation_id))?;
        let directory = self.conversation_dir(&record, "sync_conversation")?;
        for file in std::iter::once(CONVERSATION_METADATA_FILE).chain(EVENT_LOG_FILES) {
            self.durable_fs
                .sync_file_and_namespace(&directory.join(file))
                .map_err(|error| durability_error("sync_conversation", conversation_id, error))?;
        }
        if directory.join(PROVENANCE_FILE).exists() {
            self.durable_fs
                .sync_file_and_namespace(&directory.join(PROVENANCE_FILE))
                .map_err(|error| durability_error("sync_conversation", conversation_id, error))?;
        }
        log::info!(
            "[conversation-repository] conversation synced conversation_id={}",
            conversation_id
        );
        Ok(())
    }

    /// Explicit delete removes the Conversation from the live repository and
    /// deletes its private directory. This is not an archive/tombstone.
    #[allow(dead_code)]
    pub(crate) async fn mark_deleted(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        self.validate_write_permit(permit, conversation_id, "mark_deleted")?;
        let guard = self.lifecycle_lock(conversation_id).await;
        let record = self.purge_conversation_locked(permit, conversation_id)?;
        drop(guard);
        Ok(record)
    }

    pub(crate) fn purge_conversation_locked(
        self: &Arc<Self>,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        self.validate_write_permit(permit, conversation_id, "purge_conversation")?;
        self.check_recovery(conversation_id, "purge_conversation")?;
        let record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found("purge_conversation", conversation_id))?;
        let directory = self.conversation_dir(&record, "purge_conversation")?;
        remove_conversation_directory(&directory, self.locator.root(), conversation_id)?;
        self.states.lock().remove(&conversation_id);
        self.recovery_by_id.lock().remove(&conversation_id);
        self.recovery_items
            .lock()
            .retain(|item| item.conversation_id != Some(conversation_id));
        self.active_tail_cache.lock().invalidate(conversation_id);
        self.binding_index.lock().remove(conversation_id);
        self.catalog_flush.remove(conversation_id);
        log::info!(
            "[conversation-repository] conversation purged conversation_id={}",
            conversation_id
        );
        Ok(record)
    }

    /// Provider success followed by a canonical append failure must never leave the in-process
    /// materialization falsely advertising an active binding. This fail-closed marker is best
    /// effort durable and always updates the in-memory frontier before returning recovery-required.
    pub(crate) fn mark_lifecycle_recovery_required_locked(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2> {
        self.validate_write_permit(permit, conversation_id, "lifecycle_recovery")?;
        self.check_recovery(conversation_id, "lifecycle_recovery")?;
        let mut record = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| state.record.clone())
            .ok_or_else(|| not_found("lifecycle_recovery", conversation_id))?;
        record.lifecycle_state = ConversationLifecycleState::RecoveryRequired;
        self.states
            .lock()
            .get_mut(&conversation_id)
            .expect("per-Conversation lock preserves state")
            .record = record.clone();
        self.refresh_binding_index_for_conversation(conversation_id);
        self.mark_catalog_entry_dirty(conversation_id);
        if let Err(error) = self.persist_record_metadata(&record, "lifecycle_recovery") {
            log::error!(
                "[conversation-repository] lifecycle recovery marker persistence failed conversation_id={} code={}",
                conversation_id,
                stable_code(error.code)
            );
            return Err(error);
        }
        Ok(record)
    }

    /// Sole in-lock canonical sequence allocator. Lifecycle title/attach/detach/target/binding
    /// writers and ordered relay appends all consume this lane.
    fn append_event_locked(
        &self,
        conversation_id: ConversationId,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
    ) -> Result<ConversationEventRecordV2> {
        self.check_recovery(conversation_id, "append_event")?;
        #[cfg(test)]
        if matches!(
            type_,
            ConversationEventType::BindingBound
                | ConversationEventType::BindingReplaced
                | ConversationEventType::CreationFailed
        ) && self
            .fail_agent_binding_appends_remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err(repository_error(
                ConversationErrorCode::ConversationDurabilityFailed,
                "append_event",
                Some(conversation_id),
                "injected agent binding append failure".to_string(),
            ));
        }
        let stream = type_.stream();
        let (record, mut frontier, expected_stream_bytes) = self
            .states
            .lock()
            .get(&conversation_id)
            .map(|state| {
                (
                    state.record.clone(),
                    state.scan.frontier.clone(),
                    state.scan.sparse_offsets.stream(stream).validated_bytes,
                )
            })
            .ok_or_else(|| not_found("append_event", conversation_id))?;
        if record.lifecycle_state == ConversationLifecycleState::RecoveryRequired
            || record.lifecycle_state == ConversationLifecycleState::Deleted
        {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "append_event",
                Some(conversation_id),
                "Conversation lifecycle does not admit appends".to_string(),
            ));
        }
        if record.last_seq != frontier.last_seq {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "append_event",
                Some(conversation_id),
                "metadata lastSeq does not match the validated frontier".to_string(),
            ));
        }
        let seq = next_canonical_seq(record.last_seq, conversation_id, "append_event")?;
        let event = ConversationEventRecordV2 {
            schema_version: CONVERSATION_EVENT_SCHEMA_VERSION,
            conversation_id,
            seq,
            recorded_at_utc,
            type_,
            payload,
        };
        apply_event(&mut frontier, &event).map_err(|error| {
            repository_error(
                error.code,
                "append_event",
                Some(conversation_id),
                format!(
                    "frontier validation conflict at seq {} type {:?}: {}",
                    event.seq, event.type_, error.detail
                ),
            )
        })?;
        let directory = self.conversation_dir(&record, "append_event")?;
        let bytes = encode_event_record_bounded(&event).map_err(|error| match error {
            EventRecordEncodingError::TooLarge => repository_error(
                ConversationErrorCode::ConversationRecordTooLarge,
                "append_event",
                Some(conversation_id),
                format!("encoded canonical record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"),
            ),
            EventRecordEncodingError::Serialization => repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "append_event",
                Some(conversation_id),
                "canonical event serialization failed".to_string(),
            ),
        })?;
        let stream_path = directory.join(stream.file_name());
        let actual_stream_bytes = fs::metadata(&stream_path)
            .map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    "append_event",
                    Some(conversation_id),
                    format!("event stream metadata cannot be read: {error}"),
                )
            })?
            .len();
        if actual_stream_bytes != expected_stream_bytes {
            return Err(repository_error(
                ConversationErrorCode::ConversationRecoveryRequired,
                "append_event",
                Some(conversation_id),
                format!(
                    "event stream length {actual_stream_bytes} differs from validated bytes {expected_stream_bytes}"
                ),
            ));
        }
        self.durable_fs
            .append_jsonl(&stream_path, &bytes)
            .map_err(|error| durability_error("append_event", conversation_id, error))?;
        let encoded_line_bytes = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| {
                repository_error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    "append_event",
                    Some(conversation_id),
                    "encoded event length overflow".to_string(),
                )
            })?;

        let persisted_record = {
            let mut states = self.states.lock();
            let state = states
                .get_mut(&conversation_id)
                .expect("per-Conversation lock preserves state");
            let scan = Arc::make_mut(&mut state.scan);
            scan.frontier = frontier;
            scan.record_appended(stream, seq, expected_stream_bytes, encoded_line_bytes);
            state.record.last_seq = seq;
            if scan.frontier.attachment.has_events {
                state.record.project_attachment = scan.frontier.attachment.current.clone();
            }
            if let Some(execution_target) = &scan.frontier.execution_target {
                state.record.execution_target = execution_target.clone();
            }
            if let Some(lifecycle_state) = scan.frontier.lifecycle_state {
                state.record.lifecycle_state = lifecycle_state;
            }
            state.record.clone()
        };
        // The append is already authoritative. Retain its compact frontier even if metadata
        // materialization fails, preventing a duplicate seq in the same process.
        self.active_tail_cache.lock().invalidate(conversation_id);
        if matches!(
            event.type_,
            ConversationEventType::BindingBound
                | ConversationEventType::BindingDetached
                | ConversationEventType::BindingRebound
                | ConversationEventType::BindingSuspended
                | ConversationEventType::BindingReplaced
        ) {
            self.refresh_binding_index_for_conversation(conversation_id);
        }
        self.persist_record_metadata(&persisted_record, "append_event")?;
        Ok(event)
    }

    fn persist_record_metadata(
        &self,
        record: &ConversationRecordV2,
        operation: &'static str,
    ) -> Result<()> {
        let directory = self.conversation_dir(record, operation)?;
        persist_metadata_at(&self.durable_fs, &directory, record)
    }

    fn conversation_dir(
        &self,
        record: &ConversationRecordV2,
        operation: &'static str,
    ) -> Result<PathBuf> {
        self.locator
            .private_dir(record.conversation_id, &record.creation_partition)
            .map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationPathEscape,
                    operation,
                    Some(record.conversation_id),
                    error.to_string(),
                )
            })
    }

    pub(crate) fn clear_recovery_item(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<()> {
        self.validate_write_permit(permit, conversation_id, "clear_recovery_item")?;
        self.recovery_by_id.lock().remove(&conversation_id);
        self.recovery_items
            .lock()
            .retain(|item| item.conversation_id != Some(conversation_id));
        log::info!(
            "[conversation-repository] actionable recovery cleared conversation_id={}",
            conversation_id
        );
        Ok(())
    }

    fn validate_write_permit(
        &self,
        permit: &RepositoryWritePermit,
        conversation_id: ConversationId,
        operation: &'static str,
    ) -> Result<()> {
        if permit.admits(self.instance_id, conversation_id, self.is_staging()) {
            return Ok(());
        }
        log::error!(
            "[conversation-repository] write capability bypass rejected operation={} conversation_id={} mutation={}",
            operation,
            conversation_id,
            permit.mutation().as_str()
        );
        Err(repository_error(
            ConversationErrorCode::ConversationRecoveryRequired,
            operation,
            Some(conversation_id),
            "repository write permit does not match this repository, scope, or ConversationId"
                .to_string(),
        ))
    }

    fn check_recovery(
        &self,
        conversation_id: ConversationId,
        operation: &'static str,
    ) -> Result<()> {
        if let Some(item) = self.recovery_by_id.lock().get(&conversation_id).cloned() {
            return Err(repository_error(
                item.code,
                operation,
                Some(conversation_id),
                item.detail,
            ));
        }
        Ok(())
    }

    fn conversation_lock(&self, conversation_id: ConversationId) -> Arc<TokioMutex<()>> {
        let mut locks = self.conversation_locks.lock();
        Arc::clone(
            locks
                .entry(conversation_id)
                .or_insert_with(|| Arc::new(TokioMutex::new(()))),
        )
    }

    fn catalog_entry_snapshot(
        &self,
        conversation_id: ConversationId,
    ) -> Option<(ConversationRecordV2, ConversationFrontier)> {
        self.states
            .lock()
            .get(&conversation_id)
            .map(|state| (state.record.clone(), state.scan.frontier.clone()))
    }

    fn mark_catalog_entry_dirty(&self, conversation_id: ConversationId) {
        let Some((record, frontier)) = self.catalog_entry_snapshot(conversation_id) else {
            return;
        };
        self.catalog_flush.upsert(&record, &frontier);
    }

    fn refresh_binding_index_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Option<u64> {
        let keys = self.states.lock().get(&conversation_id).map(binding_keys)?;
        let generation = self.binding_index.lock().refresh(conversation_id, keys);
        log::info!(
            "[conversation-repository] binding index refreshed conversation_id={} generation={}",
            conversation_id,
            generation
        );
        Some(generation)
    }
}

fn repository_runtime_handle() -> tokio::runtime::Handle {
    tokio::runtime::Handle::try_current()
        .unwrap_or_else(|_| tauri::async_runtime::handle().inner().clone())
}

fn immutable_identity(
    record: &ConversationRecordV2,
) -> (
    ConversationId,
    DateTime<Utc>,
    crate::conversation::CreationPartition,
    String,
) {
    (
        record.conversation_id,
        record.created_at_utc,
        record.creation_partition.clone(),
        record.workspace_cwd.clone(),
    )
}

fn ensure_ready_aggregate(record: &ConversationRecordV2, operation: &'static str) -> Result<()> {
    if record.lifecycle_state == ConversationLifecycleState::Ready {
        return Ok(());
    }
    let code = if matches!(
        record.lifecycle_state,
        ConversationLifecycleState::RecoveryRequired | ConversationLifecycleState::Deleted
    ) {
        ConversationErrorCode::ConversationRecoveryRequired
    } else {
        ConversationErrorCode::ValidationError
    };
    Err(repository_error(
        code,
        operation,
        Some(record.conversation_id),
        "only ready Conversations admit attachment or execution-target mutations".to_string(),
    ))
}

fn validate_project_attachment_input(
    attachment: &ProjectAttachment,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<()> {
    if attachment.schema_version != PROJECT_ATTACHMENT_SCHEMA_VERSION
        || attachment.project_id.trim().is_empty()
        || attachment.project_id != attachment.project_id.trim()
    {
        return Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            "project attachment has an invalid schema or projectId".to_string(),
        ));
    }
    validate_canonical_directory(
        &attachment.project_path_snapshot,
        "projectPathSnapshot",
        conversation_id,
        operation,
    )?;
    match (&attachment.worktree_path, &attachment.worktree_branch) {
        (None, None) => Ok(()),
        (Some(path), Some(branch)) if !branch.trim().is_empty() && branch == branch.trim() => {
            validate_canonical_directory(path, "worktreePath", conversation_id, operation)
        }
        _ => Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            "worktreePath and worktreeBranch must either both be present or both be absent"
                .to_string(),
        )),
    }
}

fn validate_execution_target_input(
    target: &ExecutionTarget,
    attachment: Option<&ProjectAttachment>,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<()> {
    match target {
        ExecutionTarget::Workspace => Ok(()),
        ExecutionTarget::ProjectRoot {
            project_id,
            project_root,
        } => {
            let attachment =
                matching_attachment(attachment, project_id, conversation_id, operation)?;
            validate_canonical_directory(project_root, "projectRoot", conversation_id, operation)?;
            if project_root != &attachment.project_path_snapshot {
                return Err(repository_error(
                    ConversationErrorCode::ValidationError,
                    operation,
                    Some(conversation_id),
                    "projectRoot must match the attached project path snapshot".to_string(),
                ));
            }
            Ok(())
        }
        ExecutionTarget::Worktree {
            project_id,
            worktree_path,
            worktree_branch,
        } => {
            let attachment =
                matching_attachment(attachment, project_id, conversation_id, operation)?;
            if worktree_branch.trim().is_empty() || worktree_branch != worktree_branch.trim() {
                return Err(repository_error(
                    ConversationErrorCode::ValidationError,
                    operation,
                    Some(conversation_id),
                    "worktreeBranch must be non-empty and trimmed".to_string(),
                ));
            }
            validate_canonical_directory(
                worktree_path,
                "worktreePath",
                conversation_id,
                operation,
            )?;
            if let Some(attached_path) = &attachment.worktree_path {
                if attached_path != worktree_path
                    || attachment.worktree_branch.as_deref() != Some(worktree_branch.as_str())
                {
                    return Err(repository_error(
                        ConversationErrorCode::ValidationError,
                        operation,
                        Some(conversation_id),
                        "worktree target does not match the attached worktree snapshot".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }
}

fn matching_attachment<'a>(
    attachment: Option<&'a ProjectAttachment>,
    project_id: &str,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<&'a ProjectAttachment> {
    if project_id.trim().is_empty() || project_id != project_id.trim() {
        return Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            "execution target projectId must be non-empty and trimmed".to_string(),
        ));
    }
    attachment
        .filter(|attachment| attachment.project_id == project_id)
        .ok_or_else(|| {
            repository_error(
                ConversationErrorCode::ValidationError,
                operation,
                Some(conversation_id),
                "execution target projectId does not match the current project attachment"
                    .to_string(),
            )
        })
}

fn validate_canonical_directory(
    value: &str,
    field: &str,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<()> {
    let path = Path::new(value);
    if value.trim().is_empty() || value != value.trim() || !path.is_absolute() {
        return Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            format!("{field} must be a trimmed absolute directory path"),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            format!("{field} must reference an existing directory"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            format!("{field} must reference a non-symlink directory"),
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|_| {
        repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            format!("{field} cannot be canonicalized"),
        )
    })?;
    if canonical != path {
        return Err(repository_error(
            ConversationErrorCode::ValidationError,
            operation,
            Some(conversation_id),
            format!("{field} must use its canonical path without traversal or link aliases"),
        ));
    }
    Ok(())
}

fn execution_target_kind(target: &ExecutionTarget) -> &'static str {
    match target {
        ExecutionTarget::Workspace => "workspace",
        ExecutionTarget::ProjectRoot { .. } => "project_root",
        ExecutionTarget::Worktree { .. } => "worktree",
    }
}

fn validate_new_record(record: &ConversationRecordV2) -> Result<()> {
    if record.schema_version != CONVERSATION_SCHEMA_VERSION {
        return Err(repository_error(
            ConversationErrorCode::ConversationUnsupportedSchema,
            "create_conversation",
            Some(record.conversation_id),
            "unsupported conversation schemaVersion".to_string(),
        ));
    }
    if record.last_seq != 0
        || crate::conversation::contracts::CreationPartition::from_created_at(record.created_at_utc)
            != record.creation_partition
        || record.workspace_cwd.is_empty()
    {
        return Err(repository_error(
            ConversationErrorCode::ConversationCreateFailed,
            "create_conversation",
            Some(record.conversation_id),
            "new metadata must have lastSeq 0, a matching UTC partition, and workspaceCwd"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_binding_input(
    binding: &AgentSessionBinding,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<()> {
    if binding.schema_version != AGENT_SESSION_BINDING_SCHEMA_VERSION
        || binding.agent_session_id.is_empty()
        || binding.runtime_agent_id.is_empty()
        || binding.stable_agent_namespace.is_empty()
        || binding.execution_cwd.is_empty()
    {
        return Err(repository_error(
            ConversationErrorCode::ConversationBindFailed,
            operation,
            Some(conversation_id),
            "binding has an invalid schema or empty required opaque field".to_string(),
        ));
    }
    Ok(())
}

fn reconcile_metadata(accepted: &mut AcceptedCanonicalConversation) -> Result<bool> {
    let path = accepted.directory.join(CONVERSATION_METADATA_FILE);
    let bytes = fs::read(&path).map_err(|error| {
        repository_error(
            ConversationErrorCode::ConversationRecoveryRequired,
            "open",
            Some(accepted.record.conversation_id),
            error.to_string(),
        )
    })?;
    let current: ConversationRecordV2 = serde_json::from_slice(&bytes).map_err(|error| {
        repository_error(
            ConversationErrorCode::ConversationRecoveryRequired,
            "open",
            Some(accepted.record.conversation_id),
            error.to_string(),
        )
    })?;
    Ok(current != accepted.record)
}

fn remove_conversation_directory(
    directory: &Path,
    private_root: &Path,
    conversation_id: ConversationId,
) -> Result<()> {
    let canonical_root = private_root.canonicalize().map_err(|error| {
        repository_error(
            ConversationErrorCode::ConversationPathEscape,
            "purge_conversation",
            Some(conversation_id),
            format!("private root cannot be resolved: {error}"),
        )
    })?;
    match directory.canonicalize() {
        Ok(canonical_dir) => {
            if !canonical_dir.starts_with(&canonical_root) || canonical_dir == canonical_root {
                return Err(repository_error(
                    ConversationErrorCode::ConversationPathEscape,
                    "purge_conversation",
                    Some(conversation_id),
                    "conversation directory is outside the private root".to_string(),
                ));
            }
            fs::remove_dir_all(&canonical_dir).map_err(|error| {
                repository_error(
                    ConversationErrorCode::ConversationDurabilityFailed,
                    "purge_conversation",
                    Some(conversation_id),
                    format!("conversation directory could not be removed: {error}"),
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(repository_error(
            ConversationErrorCode::ConversationDurabilityFailed,
            "purge_conversation",
            Some(conversation_id),
            format!("conversation directory cannot be resolved: {error}"),
        )),
    }
}

fn persist_metadata_at(
    durable_fs: &DurableFileSystem,
    directory: &Path,
    record: &ConversationRecordV2,
) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(record).map_err(|error| {
        repository_error(
            ConversationErrorCode::ConversationRecoveryRequired,
            "persist_metadata",
            Some(record.conversation_id),
            error.to_string(),
        )
    })?;
    bytes.push(b'\n');
    durable_fs
        .replace_bytes(&directory.join(CONVERSATION_METADATA_FILE), &bytes)
        .map_err(|error| durability_error("persist_metadata", record.conversation_id, error))?;
    Ok(())
}

fn scan_workspace_recovery(
    accepted: &[AcceptedCanonicalConversation],
) -> Vec<RepositoryRecoveryItem> {
    let mut items = Vec::new();
    for conversation in accepted {
        let path = conversation.directory.join("workspace.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                items.push(RepositoryRecoveryItem {
                    code: ConversationErrorCode::ConversationRecoveryRequired,
                    kind: RepositoryRecoveryKind::WorkspaceRecoveryRequired,
                    conversation_id: Some(conversation.record.conversation_id),
                    relative_path: format!(
                        "{}/{}/workspace.json",
                        conversation.record.creation_partition.path,
                        conversation.record.conversation_id
                    ),
                    detail: format!("workspace.json could not be read: {error}"),
                    repaired: false,
                    requires_action: true,
                });
                continue;
            }
        };
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                items.push(RepositoryRecoveryItem {
                    code: ConversationErrorCode::ConversationCorrupt,
                    kind: RepositoryRecoveryKind::WorkspaceRecoveryRequired,
                    conversation_id: Some(conversation.record.conversation_id),
                    relative_path: format!(
                        "{}/{}/workspace.json",
                        conversation.record.creation_partition.path,
                        conversation.record.conversation_id
                    ),
                    detail: format!("workspace.json is corrupt: {error}"),
                    repaired: false,
                    requires_action: true,
                });
                continue;
            }
        };
        let schema_version = value.get("schemaVersion").and_then(Value::as_u64);
        let workspace_conversation_id = value.get("conversationId").and_then(Value::as_str);
        if schema_version != Some(1) {
            items.push(RepositoryRecoveryItem {
                code: ConversationErrorCode::ConversationUnsupportedSchema,
                kind: RepositoryRecoveryKind::WorkspaceRecoveryRequired,
                conversation_id: Some(conversation.record.conversation_id),
                relative_path: format!(
                    "{}/{}/workspace.json",
                    conversation.record.creation_partition.path,
                    conversation.record.conversation_id
                ),
                detail: "workspace.json has an unsupported schemaVersion".to_string(),
                repaired: false,
                requires_action: true,
            });
        } else if workspace_conversation_id
            != Some(conversation.record.conversation_id.to_string()).as_deref()
        {
            items.push(RepositoryRecoveryItem {
                code: ConversationErrorCode::ConversationCorrupt,
                kind: RepositoryRecoveryKind::WorkspaceRecoveryRequired,
                conversation_id: Some(conversation.record.conversation_id),
                relative_path: format!(
                    "{}/{}/workspace.json",
                    conversation.record.creation_partition.path,
                    conversation.record.conversation_id
                ),
                detail: "workspace.json ConversationId does not match its canonical directory"
                    .to_string(),
                repaired: false,
                requires_action: true,
            });
        }
    }
    items
}

fn map_catalog_recovery(issues: &[CatalogRecoveryIssue]) -> Vec<RepositoryRecoveryItem> {
    issues
        .iter()
        .map(|issue| RepositoryRecoveryItem {
            code: issue.code,
            kind: if issue.code == ConversationErrorCode::ConversationUnsupportedSchema {
                RepositoryRecoveryKind::UnsupportedAuthoritativeSchema
            } else {
                RepositoryRecoveryKind::CorruptAuthoritativeRecord
            },
            conversation_id: issue.conversation_id,
            relative_path: issue.relative_path.clone(),
            detail: issue.detail.clone(),
            repaired: false,
            requires_action: true,
        })
        .collect()
}

fn map_repairs(repairs: &[EventLogRepairWarning]) -> Vec<RepositoryRecoveryItem> {
    repairs
        .iter()
        .map(|repair| RepositoryRecoveryItem {
            code: ConversationErrorCode::ConversationRecoveryRequired,
            kind: RepositoryRecoveryKind::TornTailRepaired,
            conversation_id: Some(repair.conversation_id),
            relative_path: repair.stream.clone(),
            detail: format!(
                "torn final record truncated after preserving {}",
                repair.backup_file
            ),
            repaired: true,
            requires_action: false,
        })
        .collect()
}

fn not_found(operation: &'static str, conversation_id: ConversationId) -> RepositoryError {
    repository_error(
        ConversationErrorCode::ConversationNotFound,
        operation,
        Some(conversation_id),
        "canonical Conversation was not found".to_string(),
    )
}

fn next_canonical_seq(
    last_seq: u64,
    conversation_id: ConversationId,
    operation: &'static str,
) -> Result<u64> {
    last_seq.checked_add(1).ok_or_else(|| {
        repository_error(
            ConversationErrorCode::ConversationRecoveryRequired,
            operation,
            Some(conversation_id),
            "canonical sequence overflow".to_string(),
        )
    })
}

fn repository_error(
    code: ConversationErrorCode,
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    detail: String,
) -> RepositoryError {
    if matches!(
        code,
        ConversationErrorCode::ConversationCorrupt
            | ConversationErrorCode::ConversationRecoveryRequired
            | ConversationErrorCode::ConversationUnsupportedSchema
            | ConversationErrorCode::ConversationDurabilityFailed
            | ConversationErrorCode::ConversationDurabilityUnsupported
    ) {
        log::error!(
            "[conversation-repository] operation failed code={} operation={} conversation_id={}",
            stable_code(code),
            operation,
            conversation_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
    }
    RepositoryError {
        code,
        operation,
        conversation_id,
        detail,
    }
}

fn stable_code(code: ConversationErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
}

fn durability_error(
    operation: &'static str,
    conversation_id: ConversationId,
    error: impl fmt::Display,
) -> RepositoryError {
    repository_error(
        ConversationErrorCode::ConversationDurabilityFailed,
        operation,
        Some(conversation_id),
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::catalog::{
        ConversationCatalog, ConversationCatalogFileV1, CATALOG_SCHEMA_VERSION,
        EMPTY_CATALOG_GENERATED_AT_UTC,
    };
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationCreator, CreationPartition,
        PROJECT_ATTACHMENT_SCHEMA_VERSION,
    };
    use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};
    use serde_json::json;
    use std::io::{BufWriter, Write};
    use tempfile::TempDir;
    use uuid::Uuid;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    fn fixture() -> (
        TempDir,
        Arc<ConversationRepository>,
        Arc<ConversationWriter>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("private");
        let (repository, report) = ConversationRepository::open(root).unwrap();
        assert_eq!(report.valid_conversation_count, 0);
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        (temp, repository, writer)
    }

    fn record() -> ConversationRecordV2 {
        let created_at_utc = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::parse(ID).unwrap(),
            created_at_utc,
            creation_partition: CreationPartition::from_created_at(created_at_utc),
            workspace_cwd: format!("/visible/sessions/2026/08/15/{ID}"),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::InitializingAgent,
            last_seq: 0,
            created_by: ConversationCreator::Legacy,
            title: None,
            title_source: None,
        }
    }

    fn record_for_id(id: ConversationId) -> ConversationRecordV2 {
        let mut value = record();
        value.conversation_id = id;
        value.workspace_cwd = format!("/visible/sessions/2026/08/15/{id}");
        value.lifecycle_state = ConversationLifecycleState::Ready;
        value
    }

    fn time(second: u32) -> DateTime<Utc> {
        parse_created_at_utc(&format!("2026-08-15T09:45:{second:02}.000Z")).unwrap()
    }

    fn binding(id: &str, opaque: &str) -> AgentSessionBinding {
        AgentSessionBinding {
            schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
            binding_id: Uuid::parse_str(id).unwrap(),
            agent_session_id: opaque.to_string(),
            runtime_agent_id: "runtime-1".to_string(),
            stable_agent_namespace: "config:test".to_string(),
            execution_cwd: format!("/visible/sessions/2026/08/15/{ID}"),
            bound_at_utc: time(16),
            state: AgentSessionBindingState::Active,
        }
    }

    #[tokio::test]
    async fn create_writes_exact_canonical_files_only_under_private_root() {
        let (temp, repository, writer) = fixture();
        let workspace = temp.path().join("visible-workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("user-file.txt"), b"keep").unwrap();
        let mut value = record();
        value.workspace_cwd = workspace.to_string_lossy().into_owned();
        writer
            .create_conversation(value.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let directory = repository
            .locator
            .private_dir(value.conversation_id, &value.creation_partition)
            .unwrap();
        let mut names = fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                "attachments.jsonl",
                "bindings.jsonl",
                "conversation.json",
                "messages.jsonl",
                "tool-calls.jsonl",
            ]
        );
        assert_eq!(fs::read(workspace.join("user-file.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&workspace).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn mark_deleted_removes_conversation_from_repository_and_disk() {
        let (_temp, repository, writer) = fixture();
        let mut value = record();
        value.lifecycle_state = ConversationLifecycleState::Ready;
        writer
            .create_conversation(value.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let conversation_id = value.conversation_id;
        let directory = repository
            .locator
            .private_dir(conversation_id, &value.creation_partition)
            .unwrap();
        assert!(directory.exists());
        writer.mark_deleted(conversation_id).await.unwrap();
        assert!(repository.get_conversation(conversation_id).is_err());
        assert!(!directory.exists());
        assert!(repository
            .list_conversations()
            .iter()
            .all(|record| record.conversation_id != conversation_id));
        let root = repository.root().to_path_buf();
        drop(writer);
        drop(repository);
        let (reopened, _) = ConversationRepository::open(root).unwrap();
        assert!(reopened.get_conversation(conversation_id).is_err());
        assert!(reopened
            .list_conversations()
            .iter()
            .all(|record| record.conversation_id != conversation_id));
    }

    #[tokio::test]
    async fn leftover_deleted_metadata_is_purged_on_open_instead_of_resurrected() {
        let (_temp, repository, writer) = fixture();
        let mut value = record();
        value.lifecycle_state = ConversationLifecycleState::Ready;
        writer
            .create_conversation(value.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let conversation_id = value.conversation_id;
        writer
            .bind_agent_session(
                conversation_id,
                binding("b2832b54-2ca4-4db4-93fd-f93bf6793114", "agent/opaque:first"),
                time(16),
            )
            .await
            .unwrap();
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .lifecycle_state,
            ConversationLifecycleState::Ready
        );
        let directory = repository
            .locator
            .private_dir(conversation_id, &value.creation_partition)
            .unwrap();
        let mut leftover = repository.get_conversation(conversation_id).unwrap();
        leftover.lifecycle_state = ConversationLifecycleState::Deleted;
        fs::write(
            directory.join(CONVERSATION_METADATA_FILE),
            serde_json::to_vec_pretty(&leftover).unwrap(),
        )
        .unwrap();
        let root = repository.root().to_path_buf();
        drop(writer);
        drop(repository);
        let (reopened, _) = ConversationRepository::open(root).unwrap();
        assert!(reopened.get_conversation(conversation_id).is_err());
        assert!(!directory.exists());
        assert!(reopened
            .list_conversations()
            .iter()
            .all(|record| record.conversation_id != conversation_id));
    }

    #[tokio::test]
    async fn deleting_or_corrupting_catalog_rebuilds_byte_identically_and_cannot_hide_or_invent() {
        let (_temp, repository, writer) = fixture();
        let mut ready = record();
        ready.lifecycle_state = ConversationLifecycleState::Ready;
        writer
            .create_conversation(ready, ConversationMutation::CreateConversation)
            .await
            .unwrap();
        repository
            .flush_catalog_until(TokioInstant::now() + Duration::from_secs(2))
            .await
            .expect("initial catalog generation flush");
        let catalog_path = repository.root().join(CATALOG_FILE);
        let expected = fs::read(&catalog_path).unwrap();
        fs::remove_file(&catalog_path).unwrap();
        let (reopened, _) = ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        assert_eq!(fs::read(&catalog_path).unwrap(), expected);
        assert_eq!(reopened.list_conversations().len(), 1);

        fs::write(
            &catalog_path,
            br#"{"schemaVersion":1,"generatedAtUtc":"2099-01-01T00:00:00.000Z","conversations":[{"conversationId":"ffffffff-ffff-4fff-8fff-ffffffffffff"}]}"#,
        )
        .unwrap();
        let (reopened, report) =
            ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        assert_eq!(fs::read(&catalog_path).unwrap(), expected);
        assert_eq!(
            reopened.list_conversations()[0].conversation_id.to_string(),
            ID
        );
        assert!(report
            .recovery_items
            .iter()
            .any(|item| item.kind == RepositoryRecoveryKind::CatalogIgnored));
        let decoded: ConversationCatalogFileV1 = serde_json::from_slice(&expected).unwrap();
        assert_eq!(decoded.schema_version, CATALOG_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn global_sequence_is_serialized_across_message_and_tool_streams() {
        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for index in 0..20u32 {
            let writer = Arc::clone(&writer);
            tasks.push(tokio::spawn(async move {
                let type_ = if index % 2 == 0 {
                    ConversationEventType::MessageChunk
                } else {
                    ConversationEventType::ToolCall
                };
                writer
                    .append_event(
                        ConversationId::parse(ID).unwrap(),
                        time(20),
                        type_,
                        json!({"structural":"only"}),
                        ConversationMutation::AcpEventAppend,
                    )
                    .await
                    .unwrap()
                    .seq
            }));
        }
        let mut sequences = Vec::new();
        for task in tasks {
            sequences.push(task.await.unwrap());
        }
        sequences.sort_unstable();
        assert_eq!(sequences, (1..=20).collect::<Vec<_>>());
        assert_eq!(
            repository
                .get_conversation(ConversationId::parse(ID).unwrap())
                .unwrap()
                .last_seq,
            20
        );
    }

    #[tokio::test]
    async fn append_work_is_constant_and_mutations_do_not_scan_the_catalog() {
        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        crate::conversation::event_log::reset_operation_counters();
        crate::conversation::catalog::reset_catalog_scan_counter();

        for seq in 1..=3_u32 {
            writer
                .append_event(
                    ConversationId::parse(ID).unwrap(),
                    time(20 + seq),
                    ConversationEventType::MessageChunk,
                    json!({"role":"agent"}),
                    ConversationMutation::AcpEventAppend,
                )
                .await
                .unwrap();
        }

        assert_eq!(crate::conversation::event_log::operation_counters(), (3, 0));
        assert_eq!(crate::conversation::catalog::catalog_scan_count(), 0);
        assert_eq!(
            repository
                .history_summary(ConversationId::parse(ID).unwrap())
                .unwrap()
                .message_count,
            3
        );
    }

    #[tokio::test]
    async fn catalog_flush_is_async_and_coalesced() {
        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        repository
            .flush_catalog_until(TokioInstant::now() + Duration::from_secs(2))
            .await
            .expect("initial catalog flush");
        repository.reset_catalog_write_counters();

        let coordinator = repository.catalog_flush_coordinator();
        let (record, frontier) = repository
            .catalog_entry_snapshot(ConversationId::parse(ID).unwrap())
            .expect("catalog entry snapshot");
        let flush_guard = coordinator.flush_lock.lock().await;
        for _ in 0..10_000 {
            coordinator.upsert(&record, &frontier);
        }
        assert_eq!(
            repository.catalog_write_count(),
            0,
            "ordinary mutations must not replace catalog.json synchronously"
        );
        let requested_generation = coordinator.snapshot().generation;
        drop(flush_guard);

        let receipt = repository
            .flush_catalog_until(TokioInstant::now() + Duration::from_secs(2))
            .await
            .expect("coalesced catalog flush");
        assert_eq!(receipt.requested_generation, requested_generation);
        assert!(receipt.flushed_generation >= requested_generation);
        assert_eq!(repository.catalog_write_count(), 1);

        let rebuilt = rebuild_catalog(&repository.locator, &repository.durable_fs)
            .expect("authoritative catalog rebuild")
            .catalog
            .deterministic_bytes();
        assert_eq!(coordinator.snapshot().bytes, rebuilt);
        assert_eq!(
            fs::read(repository.root().join(CATALOG_FILE)).unwrap(),
            rebuilt
        );
    }

    #[tokio::test]
    async fn binding_history_distinguishes_detach_rebound_suspend_and_replacement() {
        let (_temp, repository, writer) = fixture();
        let original_record = record();
        let workspace_cwd = original_record.workspace_cwd.clone();
        writer
            .create_conversation(
                original_record.clone(),
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let conversation_id = original_record.conversation_id;
        let first = binding("b2832b54-2ca4-4db4-93fd-f93bf6793114", "agent/opaque:first");
        writer
            .bind_agent_session(conversation_id, first.clone(), time(16))
            .await
            .unwrap();
        writer
            .detach_agent_binding(conversation_id, time(17))
            .await
            .unwrap();
        assert_eq!(
            repository
                .current_binding(conversation_id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Detached
        );
        writer
            .rebind_detached_binding(conversation_id, time(18))
            .await
            .unwrap();
        assert_eq!(
            repository
                .current_binding(conversation_id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Active
        );
        let before_failed_suspend = repository.read_events(conversation_id, 0).unwrap();
        assert!(writer
            .suspend_agent_binding(conversation_id, false, time(19))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            repository.read_events(conversation_id, 0).unwrap(),
            before_failed_suspend
        );
        assert_eq!(
            repository
                .current_binding(conversation_id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Active
        );
        writer
            .suspend_agent_binding(conversation_id, true, time(20))
            .await
            .unwrap();
        assert_eq!(
            repository
                .current_binding(conversation_id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Suspended
        );
        let second = binding(
            "c3943c65-3db5-4ec5-a4e0-0a4cf78a4225",
            "agent/opaque:second",
        );
        writer
            .replace_agent_binding(conversation_id, second.clone(), time(21))
            .await
            .unwrap();
        let history = repository.binding_history(conversation_id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].agent_session_id, "agent/opaque:first");
        assert_eq!(history[0].state, AgentSessionBindingState::Replaced);
        assert_eq!(history[1].agent_session_id, "agent/opaque:second");
        assert_eq!(history[1].state, AgentSessionBindingState::Active);
        let replacement = repository
            .read_events(conversation_id, 0)
            .unwrap()
            .into_iter()
            .find(|event| event.type_ == ConversationEventType::BindingReplaced)
            .unwrap();
        assert_eq!(
            replacement.payload["previousBinding"]["agentSessionId"],
            "agent/opaque:first"
        );
        let after = repository.get_conversation(conversation_id).unwrap();
        assert_eq!(after.conversation_id, conversation_id);
        assert_eq!(after.workspace_cwd, workspace_cwd);
    }

    #[tokio::test]
    async fn project_attach_and_detach_change_only_attachment_materialization() {
        let (_temp, repository, writer) = fixture();
        let value = record();
        let conversation_id = value.conversation_id;
        let workspace_cwd = value.workspace_cwd.clone();
        let identity = (
            value.conversation_id,
            value.created_at_utc,
            value.creation_partition.clone(),
        );
        writer
            .create_conversation(value, ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let attachment = ProjectAttachment {
            schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
            project_id: "project-opaque".to_string(),
            attached_at_utc: time(22),
            project_path_snapshot: "/projects/example".to_string(),
            worktree_path: None,
            worktree_branch: None,
        };
        writer
            .append_project_attachment(conversation_id, attachment.clone(), time(22))
            .await
            .unwrap();
        let attached = repository.get_conversation(conversation_id).unwrap();
        assert_eq!(attached.project_attachment, Some(attachment));
        assert_eq!(attached.workspace_cwd, workspace_cwd);
        assert_eq!(
            (
                attached.conversation_id,
                attached.created_at_utc,
                attached.creation_partition.clone()
            ),
            identity
        );
        writer
            .detach_project_attachment(conversation_id, time(23))
            .await
            .unwrap();
        let detached = repository.get_conversation(conversation_id).unwrap();
        assert_eq!(detached.project_attachment, None);
        assert_eq!(detached.workspace_cwd, workspace_cwd);
        assert_eq!(
            (
                detached.conversation_id,
                detached.created_at_utc,
                detached.creation_partition
            ),
            identity
        );
    }

    #[tokio::test]
    async fn bootstrap_retains_no_payload_and_full_wrapper_matches_pages() {
        let (_temp, repository, writer) = fixture();
        let value = record_for_id(ConversationId::parse(ID).unwrap());
        writer
            .create_conversation(value.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let root = repository.root().to_path_buf();
        let directory = repository
            .locator
            .private_dir(value.conversation_id, &value.creation_partition)
            .unwrap();
        drop(writer);
        drop(repository);

        let messages =
            fs::File::create(directory.join(crate::conversation::event_log::MESSAGES_FILE))
                .unwrap();
        let mut messages = BufWriter::new(messages);
        for seq in 1..=2_048_u64 {
            let event = ConversationEventRecordV2::new(
                value.conversation_id,
                seq,
                time(20),
                ConversationEventType::MessageChunk,
                json!({"marker":seq}),
            );
            serde_json::to_writer(&mut messages, &event).unwrap();
            messages.write_all(b"\n").unwrap();
        }
        messages.flush().unwrap();
        drop(messages);

        let (repository, report) = ConversationRepository::open(root).unwrap();
        assert_eq!(report.scanned_event_count, 2_048);
        assert_eq!(report.sparse_index_entry_count, 8);
        assert_eq!(report.retained_payload_bytes, 0);
        assert_eq!(repository.retained_payload_bytes(), 0);
        assert_eq!(repository.active_tail_cache_stats().retained_bytes, 0);
        let scan = repository.event_log_scan(value.conversation_id).unwrap();
        assert_eq!(scan.sparse_offsets.messages.entries.len(), 8);
        assert_eq!(scan.sparse_offsets.messages.event_count, 2_048);
        assert!(scan
            .sparse_offsets
            .messages
            .entries
            .windows(2)
            .all(|window| window[1].seq - window[0].seq == 256));

        let compatibility = repository.read_events(value.conversation_id, 0).unwrap();
        assert_eq!(compatibility.len(), 2_048);
        for limit in [1, 17, 256, 1_000] {
            let mut cursor = 0;
            let mut paged = Vec::new();
            while cursor < 2_048 {
                let page = repository
                    .read_event_page(value.conversation_id, cursor, limit)
                    .unwrap();
                assert!(!page.is_empty());
                assert!(page.len() <= limit);
                cursor = page.last().unwrap().seq;
                paged.extend(page);
            }
            assert_eq!(paged, compatibility, "limit={limit}");
        }

        let missing = ConversationId::parse("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap();
        for invalid_limit in [0, MAX_EVENT_PAGE_LIMIT + 1] {
            let error = repository
                .read_event_page(missing, 0, invalid_limit)
                .unwrap_err();
            assert_eq!(error.stable_code(), "VALIDATION_ERROR");
            assert!(error.to_string().starts_with("VALIDATION_ERROR during"));
            assert_eq!(error.operation, "read_event_page");
        }
    }

    #[tokio::test]
    async fn active_tail_cache_bounds_lru_invalidation_and_conversation_isolation() {
        let (_temp, repository, writer) = fixture();
        let mut ids = Vec::new();
        for index in 0..10_u64 {
            let id =
                ConversationId::parse(&format!("10000000-0000-4000-8000-{index:012x}")).unwrap();
            writer
                .create_conversation(record_for_id(id), ConversationMutation::CreateConversation)
                .await
                .unwrap();
            writer
                .append_event(
                    id,
                    time(20),
                    ConversationEventType::MessageChunk,
                    json!({"conversationMarker":index}),
                    ConversationMutation::AcpEventAppend,
                )
                .await
                .unwrap();
            ids.push(id);
        }

        for id in ids.iter().take(ACTIVE_TAIL_CACHE_MAX_CONVERSATIONS) {
            let page = repository.read_event_page(*id, 0, 17).unwrap();
            assert!(page.iter().all(|event| event.conversation_id == *id));
        }
        let full = repository.active_tail_cache_stats();
        assert_eq!(full.conversation_count, ACTIVE_TAIL_CACHE_MAX_CONVERSATIONS);
        assert!(full.retained_bytes <= ACTIVE_TAIL_CACHE_MAX_BYTES);

        repository.read_event_page(ids[0], 0, 17).unwrap();
        repository.read_event_page(ids[8], 0, 17).unwrap();
        let after_lru = repository.active_tail_cache_stats();
        assert!(after_lru.conversation_ids.contains(&ids[0]));
        assert!(!after_lru.conversation_ids.contains(&ids[1]));
        assert!(after_lru.conversation_ids.contains(&ids[8]));

        writer
            .append_event(
                ids[0],
                time(21),
                ConversationEventType::MessageChunk,
                json!({"conversationMarker":"appended"}),
                ConversationMutation::AcpEventAppend,
            )
            .await
            .unwrap();
        let invalidated = repository.active_tail_cache_stats();
        assert!(!invalidated.conversation_ids.contains(&ids[0]));
        assert!(invalidated.conversation_ids.contains(&ids[8]));
        let appended = repository.read_event_page(ids[0], 1, 17).unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].conversation_id, ids[0]);

        writer
            .append_event(
                ids[9],
                time(21),
                ConversationEventType::MessageChunk,
                json!({"credentials":"never-cache-or-cross-conversation"}),
                ConversationMutation::AcpEventAppend,
            )
            .await
            .unwrap();
        let sensitive = repository.read_event_page(ids[9], 0, 17).unwrap();
        assert!(sensitive
            .iter()
            .all(|event| event.conversation_id == ids[9]));
        assert!(!repository
            .active_tail_cache_stats()
            .conversation_ids
            .contains(&ids[9]));
        let isolated = repository.read_event_page(ids[8], 0, 17).unwrap();
        assert!(isolated.iter().all(|event| event.conversation_id == ids[8]));
        assert!(isolated
            .iter()
            .all(|event| event.payload.get("credentials").is_none()));

        for id in ids.iter().take(8) {
            let after_seq = repository.get_conversation(*id).unwrap().last_seq;
            writer
                .append_event(
                    *id,
                    time(22),
                    ConversationEventType::MessageChunk,
                    json!({"blob":"x".repeat(250_000)}),
                    ConversationMutation::AcpEventAppend,
                )
                .await
                .unwrap();
            let page = repository.read_event_page(*id, after_seq, 17).unwrap();
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].conversation_id, *id);
        }
        let bounded = repository.active_tail_cache_stats();
        assert!(bounded.conversation_count <= ACTIVE_TAIL_CACHE_MAX_CONVERSATIONS);
        assert!(bounded.retained_bytes <= ACTIVE_TAIL_CACHE_MAX_BYTES);
    }

    #[tokio::test]
    async fn torn_tail_is_repaired_but_middle_corruption_surfaces_recovery_required() {
        use std::io::Write;

        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        writer
            .append_event(
                ConversationId::parse(ID).unwrap(),
                time(16),
                ConversationEventType::MessageChunk,
                json!({"role":"agent"}),
                ConversationMutation::AcpEventAppend,
            )
            .await
            .unwrap();
        let directory = repository
            .locator
            .private_dir(
                ConversationId::parse(ID).unwrap(),
                &record().creation_partition,
            )
            .unwrap();
        let messages = directory.join(crate::conversation::event_log::MESSAGES_FILE);
        fs::OpenOptions::new()
            .append(true)
            .open(&messages)
            .unwrap()
            .write_all(b"{torn")
            .unwrap();
        let (_reopened, report) =
            ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        assert!(report
            .recovery_items
            .iter()
            .any(|item| item.kind == RepositoryRecoveryKind::TornTailRepaired));

        fs::write(&messages, b"{bad}\n{}\n").unwrap();
        let bytes = fs::read(&messages).unwrap();
        let (reopened, report) =
            ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        assert!(reopened.list_conversations().is_empty());
        assert!(report.recovery_items.iter().any(|item| {
            item.code == ConversationErrorCode::ConversationRecoveryRequired && item.requires_action
        }));
        assert_eq!(fs::read(messages).unwrap(), bytes);
    }

    #[tokio::test]
    async fn allocate_canonical_sequence_inside_repository_lock_returns_ticket_seq() {
        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let conversation_id = ConversationId::parse(ID).unwrap();
        let permit = writer
            .authorize(conversation_id, ConversationMutation::AcpEventAppend)
            .unwrap();
        let first = repository
            .append_ordered_event(
                &permit,
                conversation_id,
                1,
                time(20),
                ConversationEventType::MessageChunk,
                json!({"structural":"first"}),
            )
            .await
            .unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(CanonicalSequenceTicket::from_event(&first).seq, first.seq);
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .last_seq,
            1
        );
        let second = repository
            .append_ordered_event(
                &permit,
                conversation_id,
                2,
                time(21),
                ConversationEventType::MessageChunk,
                json!({"structural":"second"}),
            )
            .await
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(CanonicalSequenceTicket::from_event(&second).seq, second.seq);
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .last_seq,
            2
        );
    }

    #[tokio::test]
    async fn lifecycle_append_racing_reserved_relay_seq_reconciles_instead_of_corruption() {
        let (_temp, repository, writer) = fixture();
        writer
            .create_conversation(record(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        let conversation_id = ConversationId::parse(ID).unwrap();
        let reserved_relay_seq = 1;
        let lifecycle = writer
            .append_event(
                conversation_id,
                time(20),
                ConversationEventType::LocalTitleGenerated,
                json!({"title":"lifecycle"}),
                ConversationMutation::AcpEventAppend,
            )
            .await
            .unwrap();
        assert_eq!(lifecycle.seq, reserved_relay_seq);
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .last_seq,
            1
        );
        let permit = writer
            .authorize(conversation_id, ConversationMutation::AcpEventAppend)
            .unwrap();
        let raced = repository
            .append_ordered_event(
                &permit,
                conversation_id,
                reserved_relay_seq,
                time(21),
                ConversationEventType::MessageChunk,
                json!({"structural":"stale-reservation"}),
            )
            .await
            .unwrap_err();
        assert_eq!(raced.code, ConversationErrorCode::ConversationConflict);
        assert_ne!(raced.stable_code(), "CONVERSATION_SOURCE_SEQUENCE_INVALID");
        assert_ne!(
            raced.stable_code(),
            "CONVERSATION_PERSISTENCE_FRONTIER_MISMATCH"
        );
        assert_ne!(
            raced.code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .last_seq,
            1
        );
        let reconciled = repository
            .append_ordered_event(
                &permit,
                conversation_id,
                2,
                time(22),
                ConversationEventType::MessageChunk,
                json!({"structural":"reconciled"}),
            )
            .await
            .unwrap();
        assert_eq!(reconciled.seq, 2);
        assert_eq!(
            repository
                .get_conversation(conversation_id)
                .unwrap()
                .last_seq,
            2
        );
        let page = repository.read_event_page(conversation_id, 0, 17).unwrap();
        let seqs: Vec<u64> = page.iter().map(|event| event.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_catalog_replace_cannot_commit_stale_generation() {
        let temp = tempfile::tempdir().unwrap();
        let catalog_path = temp.path().canonicalize().unwrap().join(CATALOG_FILE);
        let coordinator = CatalogFlushCoordinator::new_for_test(
            ConversationCatalog::from_file(ConversationCatalogFileV1 {
                schema_version: CATALOG_SCHEMA_VERSION,
                generated_at_utc: EMPTY_CATALOG_GENERATED_AT_UTC.to_string(),
                conversations: Vec::new(),
            }),
            DurableFileSystem::new(),
            catalog_path.clone(),
        );
        let mut first = record();
        first.last_seq = 1;
        let frontier_g = ConversationFrontier {
            last_seq: 1,
            ..ConversationFrontier::default()
        };
        let generation_g = coordinator.admit_for_test(&first, &frontier_g);
        coordinator.stall_catalog_replace(generation_g);
        let stalled = Arc::clone(&coordinator);
        let stale_deadline = TokioInstant::now() + Duration::from_millis(250);
        let stale_flush =
            tokio::spawn(async move { stalled.flush_once_until_for_test(stale_deadline).await });
        assert!(
            coordinator.wait_for_catalog_replace_stall(Duration::from_secs(2)),
            "generation G replace must enter the fence stall"
        );
        tokio::time::sleep_until(stale_deadline + Duration::from_millis(50)).await;
        let mut newer = first.clone();
        newer.last_seq = 2;
        let frontier_h = ConversationFrontier {
            last_seq: 2,
            ..ConversationFrontier::default()
        };
        let generation_h = coordinator.admit_for_test(&newer, &frontier_h);
        assert!(generation_h > generation_g);
        let flushed = coordinator.flush_once_for_test().await.unwrap();
        assert_eq!(flushed.flushed_generation, generation_h);
        assert_eq!(coordinator.flushed_generation(), generation_h);
        coordinator.release_catalog_replace_stall();
        let stale = stale_flush.await.unwrap();
        assert!(
            stale.is_err(),
            "generation G flush must time out while stalled"
        );
        assert_eq!(coordinator.flushed_generation(), generation_h);
        assert_eq!(coordinator.last_written_catalog_generation(), generation_h);
        let on_disk: ConversationCatalogFileV1 =
            serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
        assert_eq!(on_disk.conversations[0].last_seq, 2);
    }

    #[test]
    fn frontier_clone_with_max_plan_usage_stays_within_262144_p99() {
        // Measurement (RR3-PERF-006): latest_usage/latest_plan are Option<Arc<Value>>.
        // Cloning ConversationFrontier increments Arc refcounts and does not deep-copy the
        // 262144-byte JSON trees. Additional allocation per clone is therefore two Arc
        // pointer slots (size_of::<Arc<Value>>() each). p99 of those samples must stay
        // <= MAX_CONVERSATION_RECORD_BYTES (262144). Pointer equality proves the trees
        // were not cloned.
        const SAMPLES: usize = 10_000;
        let usage_blob = "u".repeat(MAX_CONVERSATION_RECORD_BYTES);
        let plan_blob = "p".repeat(MAX_CONVERSATION_RECORD_BYTES);
        let frontier = ConversationFrontier {
            latest_usage: Some(Arc::new(json!({"pad": usage_blob}))),
            latest_plan: Some(Arc::new(json!({"pad": plan_blob}))),
            ..ConversationFrontier::default()
        };
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut clones = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let cloned = frontier.clone();
            assert!(
                Arc::ptr_eq(
                    frontier.latest_usage.as_ref().unwrap(),
                    cloned.latest_usage.as_ref().unwrap()
                ),
                "usage replacement must be an Arc pointer clone"
            );
            assert!(
                Arc::ptr_eq(
                    frontier.latest_plan.as_ref().unwrap(),
                    cloned.latest_plan.as_ref().unwrap()
                ),
                "plan replacement must be an Arc pointer clone"
            );
            let additional =
                std::mem::size_of::<std::sync::Arc<serde_json::Value>>().saturating_mul(2);
            samples.push(additional);
            clones.push(cloned);
        }
        samples.sort_unstable();
        let p99 = samples[samples.len() * 99 / 100];
        assert!(
            p99 <= MAX_CONVERSATION_RECORD_BYTES,
            "frontier clone additional allocation p99 was {p99} bytes"
        );
        assert_eq!(clones.len(), SAMPLES);
    }
}
