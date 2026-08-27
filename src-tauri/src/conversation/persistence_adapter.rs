//! ACP-to-Conversation persistence adapter.
//!
//! The adapter is the only live ACP history writer after bootstrap. Opaque agent session ids are
//! resolved through canonical binding history; unmapped events fail closed and never fall back to
//! a legacy store.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::PlanEntry;
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::Value;

use crate::acp::session_persistence::{
    PersistedEventRecord, PersistedSessionStatus, SessionIndexEntry, SessionMetadata, TitleSource,
    SESSION_SCHEMA_VERSION,
};
use crate::conversation::contracts::{
    encoded_json_len_bounded, AgentSessionBinding, AgentSessionBindingState,
    ConversationHistoryPageV1, ConversationHistoryRecordV1, ConversationId,
    ConversationLifecycleState, ConversationTitleSource, ExecutionTarget,
    CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION, CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
    MAX_CONVERSATION_HISTORY_PAGE_BYTES, MAX_CONVERSATION_HISTORY_PAGE_LIMIT,
    MAX_CONVERSATION_RECORD_BYTES, MIN_CONVERSATION_HISTORY_PAGE_LIMIT,
};
use crate::conversation::event_log::ConversationEventType;
use crate::conversation::migration::ConversationReader;
use crate::conversation::repository::{
    CatalogFlushError, CatalogFlushReceipt, ConversationRepository,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};

#[derive(Debug)]
pub struct ConversationPersistenceError {
    pub code: &'static str,
    pub operation: &'static str,
    pub detail: String,
}

impl fmt::Display for ConversationPersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.code, self.operation, self.detail
        )
    }
}

impl std::error::Error for ConversationPersistenceError {}

pub type Result<T> = std::result::Result<T, ConversationPersistenceError>;

pub const MAX_BINDING_MISS_CACHE_ENTRIES: usize = 1024;
pub const MAX_COMPAT_HISTORY_RECORDS: usize = 1000;
pub const CONVERSATION_HISTORY_PAGING_REQUIRED: &str = "CONVERSATION_HISTORY_PAGING_REQUIRED";
pub const CONVERSATION_PERSISTENCE_COMMIT_INDETERMINATE: &str =
    "CONVERSATION_PERSISTENCE_COMMIT_INDETERMINATE";
pub const DEFAULT_DELIVERY_COMMIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BindingLookupKind {
    Active,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindingMissKey {
    kind: BindingLookupKind,
    agent_session_id: String,
}

#[derive(Debug, Default)]
struct BindingMissCache {
    generation: u64,
    entries: HashSet<BindingMissKey>,
    order: VecDeque<BindingMissKey>,
    evictions: u64,
}

impl BindingMissCache {
    fn synchronize_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.entries.clear();
            self.order.clear();
        }
    }

    fn contains(&self, key: &BindingMissKey) -> bool {
        self.entries.contains(key)
    }

    fn insert(&mut self, key: BindingMissKey) {
        if !self.entries.insert(key.clone()) {
            return;
        }
        self.order.push_back(key);
        while self.entries.len() > MAX_BINDING_MISS_CACHE_ENTRIES {
            if let Some(evicted) = self.order.pop_front() {
                if self.entries.remove(&evicted) {
                    self.evictions = self.evictions.saturating_add(1);
                }
            }
        }
    }

    fn invalidate_session(&mut self, agent_session_id: &str) {
        self.entries
            .retain(|key| key.agent_session_id != agent_session_id);
        self.order
            .retain(|key| key.agent_session_id != agent_session_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingMissCacheStats {
    pub entries: usize,
    pub generation: u64,
    pub evictions: u64,
}

pub struct ConversationPersistenceAdapter {
    writer: Arc<ConversationWriter>,
    repository: Arc<ConversationRepository>,
    reader: Arc<ConversationReader>,
    binding_misses: Mutex<BindingMissCache>,
}

impl ConversationPersistenceAdapter {
    #[must_use]
    pub fn new(writer: Arc<ConversationWriter>, reader: Arc<ConversationReader>) -> Self {
        let repository = Arc::clone(writer.repository());
        let generation = repository.binding_generation();
        Self {
            repository,
            writer,
            reader,
            binding_misses: Mutex::new(BindingMissCache {
                generation,
                ..BindingMissCache::default()
            }),
        }
    }

    /// Compatibility hook retained for callers that previously forced a repository rescan.
    /// Canonical maps are repository-owned now, so this only invalidates bounded misses.
    pub fn rebuild_binding_index(&self) {
        let generation = self.repository.binding_generation();
        self.binding_misses
            .lock()
            .synchronize_generation(generation);
    }

    pub fn register_binding(&self, agent_session_id: &str, conversation_id: ConversationId) {
        let generation = self
            .repository
            .refresh_binding_index_hint(conversation_id)
            .unwrap_or_else(|| self.repository.binding_generation());
        let mut misses = self.binding_misses.lock();
        misses.synchronize_generation(generation);
        // A lookup can race a repository refresh: it may insert a miss at the
        // new generation after the binding already exists. Registration is an
        // explicit positive observation, so invalidate this session even when
        // refreshing identical keys does not advance the generation.
        misses.invalidate_session(agent_session_id);
        drop(misses);
        log::info!(
            "[conversation-persistence] binding index observed conversation_id={} session_id={} generation={}",
            conversation_id,
            agent_session_id,
            generation
        );
    }

    #[must_use]
    pub fn conversation_id_for_session(&self, agent_session_id: &str) -> Option<ConversationId> {
        self.conversation_id_for_active_binding(agent_session_id)
    }

    /// Resolve the host-owned execution context used by scheduled-task drafts.
    /// Project association is optional metadata; the agent cannot choose a
    /// different workspace, execution directory, or ACP.
    #[must_use]
    pub fn scheduled_task_scope_for_session(
        &self,
        agent_session_id: &str,
    ) -> Option<(Option<String>, String, ExecutionTarget, String, String)> {
        let conversation_id = self.conversation_id_for_active_binding(agent_session_id)?;
        let record = self.reader.get(conversation_id).ok()?;
        let binding = self
            .binding_for_session(conversation_id, agent_session_id)
            .ok()?;
        let project_id = record
            .project_attachment
            .as_ref()
            .map(|attachment| attachment.project_id.clone());
        let agent_config_id = binding
            .stable_agent_namespace
            .strip_prefix("config:")
            .unwrap_or(binding.stable_agent_namespace.as_str())
            .to_string();
        Some((
            project_id,
            record.workspace_cwd,
            record.execution_target,
            binding.execution_cwd,
            agent_config_id,
        ))
    }

    #[must_use]
    pub fn conversation_id_for_active_binding(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.resolve_binding(BindingLookupKind::Active, agent_session_id)
    }

    #[must_use]
    pub fn conversation_id_for_current_binding(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.conversation_id_for_history_binding(agent_session_id)
    }

    #[must_use]
    pub fn conversation_id_for_history_binding(
        &self,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        self.resolve_binding(BindingLookupKind::History, agent_session_id)
    }

    fn resolve_binding(
        &self,
        kind: BindingLookupKind,
        agent_session_id: &str,
    ) -> Option<ConversationId> {
        let generation = self.repository.binding_generation();
        let key = BindingMissKey {
            kind,
            agent_session_id: agent_session_id.to_string(),
        };
        {
            let mut misses = self.binding_misses.lock();
            misses.synchronize_generation(generation);
            if misses.contains(&key) {
                return None;
            }
        }
        let resolved = match kind {
            BindingLookupKind::Active => self
                .repository
                .conversation_id_for_active_binding(agent_session_id),
            BindingLookupKind::History => self
                .repository
                .conversation_id_for_history_binding(agent_session_id),
        };
        if resolved.is_none() {
            let mut misses = self.binding_misses.lock();
            misses.synchronize_generation(self.repository.binding_generation());
            misses.insert(key);
        }
        resolved
    }

    #[must_use]
    pub fn binding_miss_cache_stats(&self) -> BindingMissCacheStats {
        let cache = self.binding_misses.lock();
        BindingMissCacheStats {
            entries: cache.entries.len(),
            generation: cache.generation,
            evictions: cache.evictions,
        }
    }

    /// Coordinator-only canonical append. Every relay cursor consumes one canonical sequence:
    /// materialized Usage/Plan events keep their complete payload, while unsupported live-only
    /// events become a payload-free `relay_cursor_advanced` marker.
    pub(super) async fn append_ordered_event(
        self: &Arc<Self>,
        agent_session_id: &str,
        source_seq: u64,
        type_: &str,
        payload: Value,
    ) -> Result<u64> {
        let conversation_id = self
            .conversation_id_for_active_binding(agent_session_id)
            .ok_or_else(|| {
                log::error!(
                    "[conversation-persistence] inactive or unmapped ACP event rejected event_type={type_}"
                );
                error(
                    "CONVERSATION_BINDING_NOT_FOUND",
                    "append_ordered_event",
                    "opaque agent session id has no canonical Conversation binding",
                )
            })?;
        let (canonical_type, canonical_payload) = canonical_event_type(type_).map_or(
            (
                ConversationEventType::RelayCursorAdvanced,
                serde_json::json!({}),
            ),
            |type_| (type_, payload),
        );
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::AcpEventAppend)
            .map_err(|source| map_append_error("append_ordered_event", source))?;
        let event = self
            .repository
            .append_ordered_event(
                &permit,
                conversation_id,
                source_seq,
                Utc::now(),
                canonical_type,
                canonical_payload,
            )
            .await
            .map_err(|source| map_append_error("append_ordered_event", source))?;
        // event.seq is the repository-allocated ticket sequence. Never trust a
        // stale reserved source_seq when it differs.
        if event.seq != source_seq {
            return Err(error(
                "CONVERSATION_CONFLICT",
                "append_ordered_event",
                format!(
                    "reserved source seq {source_seq} differs from allocated seq {}",
                    event.seq
                ),
            ));
        }
        Ok(event.seq)
    }

    /// Compatibility seam for the pre-ticket ordered worker. Visibility is restricted to the
    /// Conversation module; new producers must use [`Self::append_ordered_event`] with their
    /// explicit source cursor.
    pub(super) async fn append_acp_event(
        self: &Arc<Self>,
        agent_session_id: &str,
        type_: &str,
        payload: Value,
    ) -> Result<u64> {
        let source_seq = self
            .last_seq(agent_session_id)?
            .checked_add(1)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_SOURCE_SEQUENCE_INVALID",
                    "append_acp_event",
                    "canonical sequence overflow",
                )
            })?;
        self.append_ordered_event(agent_session_id, source_seq, type_, payload)
            .await
    }

    #[must_use]
    pub fn list_sessions(&self) -> Vec<SessionIndexEntry> {
        let mut sessions = Vec::new();
        for record in self.reader.list() {
            if record.lifecycle_state == ConversationLifecycleState::Deleted {
                continue;
            }
            let Ok(Some(binding)) = self.repository.current_binding(record.conversation_id) else {
                continue;
            };
            let Ok(summary) = self.repository.history_summary(record.conversation_id) else {
                continue;
            };
            let status = history_status(record.lifecycle_state, binding.state);
            sessions.push(SessionIndexEntry {
                storage_key: record.conversation_id.to_string(),
                session_id: binding.agent_session_id,
                stable_agent_namespace: Some(binding.stable_agent_namespace),
                runtime_agent_id: Some(binding.runtime_agent_id),
                project_id: record
                    .project_attachment
                    .as_ref()
                    .map(|attachment| attachment.project_id.clone()),
                cwd: binding.execution_cwd,
                title: summary.title,
                title_source: summary.title_source.map(acp_title_source),
                created_at: record.created_at_utc.timestamp_millis().max(0) as u64,
                last_activity_at: summary.last_activity_at_utc.timestamp_millis().max(0) as u64,
                status,
                message_count: summary.message_count,
                tool_count: summary.tool_count,
                last_seq: record.last_seq,
                discovered: false,
                resume_eligible: true,
                worktree_path: record
                    .project_attachment
                    .as_ref()
                    .and_then(|attachment| attachment.worktree_path.clone()),
                worktree_branch: record
                    .project_attachment
                    .as_ref()
                    .and_then(|attachment| attachment.worktree_branch.clone()),
            });
        }
        sessions
    }

    pub fn legacy_materialization(
        &self,
        agent_session_id: &str,
    ) -> Result<(SessionMetadata, Vec<PersistedEventRecord>)> {
        let (conversation_id, metadata, target_last_seq) =
            self.history_metadata(agent_session_id, "materialize")?;
        let persisted =
            self.legacy_events_after(conversation_id, agent_session_id, 0, target_last_seq)?;
        Ok((metadata, persisted))
    }

    /// Compatibility first-page entry point. New transports call [`Self::history_page_at`] and
    /// echo the first page's `targetLastSeq` on every continuation request.
    pub fn history_page(
        &self,
        agent_session_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<ConversationHistoryPageV1> {
        self.history_page_at(agent_session_id, after_seq, limit, None)
    }

    /// Read one byte-bounded page pinned to an optional first-page frontier.
    pub fn history_page_at(
        &self,
        agent_session_id: &str,
        after_seq: u64,
        limit: usize,
        target_last_seq: Option<u64>,
    ) -> Result<ConversationHistoryPageV1> {
        if !(MIN_CONVERSATION_HISTORY_PAGE_LIMIT..=MAX_CONVERSATION_HISTORY_PAGE_LIMIT)
            .contains(&limit)
        {
            return Err(error(
                "VALIDATION_ERROR",
                "history_page",
                format!(
                    "history page limit must be between {MIN_CONVERSATION_HISTORY_PAGE_LIMIT} and {MAX_CONVERSATION_HISTORY_PAGE_LIMIT}"
                ),
            ));
        }
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_NOT_FOUND",
                    "history_page",
                    "binding not found",
                )
            })?;
        let current_last_seq = self
            .reader
            .get(conversation_id)
            .map_err(|source| {
                error(
                    "CONVERSATION_READ_FAILED",
                    "history_page",
                    source.to_string(),
                )
            })?
            .last_seq;
        let target_last_seq = target_last_seq.unwrap_or(current_last_seq);
        if target_last_seq > current_last_seq {
            return Err(error(
                "VALIDATION_ERROR",
                "history_page",
                "targetLastSeq is ahead of the canonical frontier",
            ));
        }
        if after_seq > target_last_seq {
            return Err(error(
                "VALIDATION_ERROR",
                "history_page",
                "history cursor is ahead of targetLastSeq",
            ));
        }
        let canonical = if after_seq == target_last_seq {
            Vec::new()
        } else {
            self.repository
                .read_event_page(conversation_id, after_seq, limit)
                .map_err(|source| {
                    error(
                        repository_read_code(&source),
                        "history_page",
                        source.to_string(),
                    )
                })?
                .into_iter()
                .take_while(|event| event.seq <= target_last_seq)
                .collect::<Vec<_>>()
        };

        let mut records = Vec::new();
        let mut record_bytes = Vec::new();
        let mut next_cursor = after_seq;
        for event in canonical {
            let event_seq = event.seq;
            let Some(type_) = legacy_event_type(event.type_) else {
                next_cursor = event_seq;
                continue;
            };
            let record = ConversationHistoryRecordV1 {
                schema_version: CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                session_id: agent_session_id.to_string(),
                seq: event_seq,
                type_: type_.to_string(),
                recorded_at: event.recorded_at_utc.timestamp_millis().max(0) as u64,
                payload: event.payload,
            };
            let encoded = encoded_json_len_bounded(&record, MAX_CONVERSATION_RECORD_BYTES)
                .ok_or_else(|| {
                    error(
                        "CONVERSATION_RECORD_TOO_LARGE",
                        "history_page",
                        format!(
                            "encoded history record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"
                        ),
                    )
                })?;
            let candidate_complete = event_seq == target_last_seq;
            if history_page_encoded_bytes(
                &record_bytes,
                Some(encoded),
                event_seq,
                target_last_seq,
                candidate_complete,
            ) > MAX_CONVERSATION_HISTORY_PAGE_BYTES
            {
                break;
            }
            record_bytes.push(encoded);
            records.push(record);
            next_cursor = event_seq;
        }
        if after_seq < target_last_seq && next_cursor <= after_seq {
            return Err(error(
                "CONVERSATION_PAGE_TOO_LARGE",
                "history_page",
                "bounded canonical page could not advance within the encoded byte limit",
            ));
        }
        let page = ConversationHistoryPageV1 {
            schema_version: CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION,
            records,
            next_cursor,
            complete: next_cursor == target_last_seq,
            target_last_seq,
        };
        page.validate(agent_session_id, after_seq, limit, Some(target_last_seq))
            .map_err(|source| error(source.stable_code(), "history_page", source.to_string()))?;
        Ok(page)
    }

    /// Spawn the bounded filesystem traversal on Tokio's blocking pool.
    pub async fn history_page_blocking(
        self: &Arc<Self>,
        agent_session_id: String,
        after_seq: u64,
        limit: usize,
        target_last_seq: Option<u64>,
    ) -> Result<ConversationHistoryPageV1> {
        let adapter = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            adapter.history_page_at(&agent_session_id, after_seq, limit, target_last_seq)
        })
        .await
        .map_err(|_| {
            error(
                "CONVERSATION_READ_FAILED",
                "history_page",
                "blocking history traversal task failed",
            )
        })?
    }

    pub fn history_metadata(
        &self,
        agent_session_id: &str,
        operation: &'static str,
    ) -> Result<(ConversationId, SessionMetadata, u64)> {
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| error("CONVERSATION_NOT_FOUND", operation, "binding not found"))?;
        let record = self
            .reader
            .get(conversation_id)
            .map_err(|source| error("CONVERSATION_READ_FAILED", operation, source.to_string()))?;
        let binding = self.binding_for_session(conversation_id, agent_session_id)?;
        let summary = self
            .repository
            .history_summary(conversation_id)
            .map_err(|source| error("CONVERSATION_READ_FAILED", operation, source.to_string()))?;
        let created_at = record.created_at_utc.timestamp_millis().max(0) as u64;
        let target_last_seq = record.last_seq;
        Ok((
            conversation_id,
            SessionMetadata {
                schema_version: SESSION_SCHEMA_VERSION,
                storage_key: conversation_id.to_string(),
                session_id: agent_session_id.to_string(),
                stable_agent_namespace: Some(binding.stable_agent_namespace),
                runtime_agent_id: Some(binding.runtime_agent_id),
                project_id: record
                    .project_attachment
                    .as_ref()
                    .map(|attachment| attachment.project_id.clone()),
                cwd: binding.execution_cwd,
                title: summary.title,
                title_source: summary.title_source.map(acp_title_source),
                created_at,
                last_activity_at: summary.last_activity_at_utc.timestamp_millis().max(0) as u64,
                status: history_status(record.lifecycle_state, binding.state),
                message_count: summary.message_count,
                tool_count: summary.tool_count,
                last_seq: target_last_seq,
                discovered: false,
                worktree_path: record
                    .project_attachment
                    .as_ref()
                    .and_then(|attachment| attachment.worktree_path.clone()),
                worktree_branch: record
                    .project_attachment
                    .as_ref()
                    .and_then(|attachment| attachment.worktree_branch.clone()),
            },
            target_last_seq,
        ))
    }

    fn binding_for_session(
        &self,
        conversation_id: ConversationId,
        agent_session_id: &str,
    ) -> Result<AgentSessionBinding> {
        if let Some(binding) = self
            .repository
            .current_binding(conversation_id)
            .map_err(|source| {
                error(
                    "CONVERSATION_READ_FAILED",
                    "materialize_binding",
                    source.to_string(),
                )
            })?
            .filter(|binding| binding.agent_session_id == agent_session_id)
        {
            return Ok(binding);
        }
        self.repository
            .binding_history(conversation_id)
            .map_err(|source| {
                error(
                    "CONVERSATION_READ_FAILED",
                    "materialize_binding",
                    source.to_string(),
                )
            })?
            .into_iter()
            .find(|binding| binding.agent_session_id == agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_BINDING_NOT_FOUND",
                    "materialize_binding",
                    "binding not found",
                )
            })
    }

    pub fn last_seq(&self, agent_session_id: &str) -> Result<u64> {
        let conversation_id = self
            .conversation_id_for_active_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_BINDING_NOT_FOUND",
                    "last_seq",
                    "binding is not active",
                )
            })?;
        self.reader
            .get(conversation_id)
            .map(|record| record.last_seq)
            .map_err(|source| error("CONVERSATION_READ_FAILED", "last_seq", source.to_string()))
    }

    pub fn history_last_seq(&self, agent_session_id: &str) -> Result<u64> {
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_NOT_FOUND",
                    "history_last_seq",
                    "binding not found",
                )
            })?;
        self.reader
            .get(conversation_id)
            .map(|record| record.last_seq)
            .map_err(|source| {
                error(
                    "CONVERSATION_READ_FAILED",
                    "history_last_seq",
                    source.to_string(),
                )
            })
    }

    pub fn replay_after(
        &self,
        agent_session_id: &str,
        cursor: u64,
    ) -> Result<Vec<PersistedEventRecord>> {
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_NOT_FOUND",
                    "replay_after",
                    "binding not found",
                )
            })?;
        let last_seq = self.history_last_seq(agent_session_id)?;
        self.legacy_events_after(conversation_id, agent_session_id, cursor, last_seq)
    }

    fn legacy_events_after(
        &self,
        _conversation_id: ConversationId,
        agent_session_id: &str,
        after_seq: u64,
        target_last_seq: u64,
    ) -> Result<Vec<PersistedEventRecord>> {
        let mut cursor = after_seq;
        let mut persisted = Vec::new();
        while cursor < target_last_seq {
            let remaining = MAX_COMPAT_HISTORY_RECORDS.saturating_sub(persisted.len());
            let request_limit = remaining.saturating_add(1).clamp(
                MIN_CONVERSATION_HISTORY_PAGE_LIMIT,
                MAX_CONVERSATION_HISTORY_PAGE_LIMIT,
            );
            let page = self.history_page_at(
                agent_session_id,
                cursor,
                request_limit,
                Some(target_last_seq),
            )?;
            if page.target_last_seq != target_last_seq {
                return Err(error(
                    "CONVERSATION_READ_FAILED",
                    "materialize_events",
                    "history target changed during compatibility traversal",
                ));
            }
            if page.records.len() > remaining {
                log::warn!(
                    "[conversation-persistence] compatibility paging required code={} frontier={} record_count={}",
                    CONVERSATION_HISTORY_PAGING_REQUIRED,
                    cursor,
                    persisted.len().saturating_add(page.records.len())
                );
                return Err(error(
                    CONVERSATION_HISTORY_PAGING_REQUIRED,
                    "materialize_events",
                    "history exceeds the compatibility materialization limit; use bounded pages",
                ));
            }
            persisted.extend(page.records.into_iter().map(|record| PersistedEventRecord {
                schema_version: SESSION_SCHEMA_VERSION,
                session_id: record.session_id,
                seq: record.seq,
                type_: record.type_,
                recorded_at: record.recorded_at,
                payload: record.payload,
            }));
            if page.next_cursor <= cursor && !page.complete {
                return Err(error(
                    "CONVERSATION_READ_FAILED",
                    "materialize_events",
                    "history page cursor did not advance",
                ));
            }
            cursor = page.next_cursor;
            if page.complete {
                break;
            }
        }
        Ok(persisted)
    }

    /// Latest canonical plan replacement for cold PlanStore hydration. `Some(vec![])` is a
    /// durable clear; `None` means no plan update has ever committed.
    pub fn latest_durable_plan(&self, agent_session_id: &str) -> Result<Option<Vec<PlanEntry>>> {
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_NOT_FOUND",
                    "latest_durable_plan",
                    "binding not found",
                )
            })?;
        let frontier = self
            .repository
            .conversation_frontier(conversation_id)
            .map_err(|source| map_frontier_read_error("latest_durable_plan", source))?;
        let Some(payload) = frontier.latest_plan_arc() else {
            return Ok(None);
        };
        crate::conversation::usage_plan::validate_plan_update(payload.as_ref()).map_err(|_| {
            error(
                "CONVERSATION_RECOVERY_REQUIRED",
                "latest_durable_plan",
                "canonical plan payload failed typed schema",
            )
        })?;
        let entries = payload
            .get("plan")
            .and_then(|plan| plan.get("entries"))
            .cloned()
            .ok_or_else(|| {
                error(
                    "CONVERSATION_RECOVERY_REQUIRED",
                    "latest_durable_plan",
                    "canonical plan payload is malformed",
                )
            })?;
        serde_json::from_value(entries).map(Some).map_err(|_| {
            error(
                "CONVERSATION_RECOVERY_REQUIRED",
                "latest_durable_plan",
                "canonical plan entries could not be decoded",
            )
        })
    }

    pub fn latest_durable_usage(&self, agent_session_id: &str) -> Result<Option<Value>> {
        let conversation_id = self
            .conversation_id_for_history_binding(agent_session_id)
            .ok_or_else(|| {
                error(
                    "CONVERSATION_NOT_FOUND",
                    "latest_durable_usage",
                    "binding not found",
                )
            })?;
        let frontier = self
            .repository
            .conversation_frontier(conversation_id)
            .map_err(|source| map_frontier_read_error("latest_durable_usage", source))?;
        let Some(payload) = frontier.latest_usage_arc() else {
            return Ok(None);
        };
        crate::conversation::usage_plan::validate_usage_update(payload.as_ref()).map_err(|_| {
            error(
                "CONVERSATION_RECOVERY_REQUIRED",
                "latest_durable_usage",
                "canonical usage payload failed typed schema",
            )
        })?;
        Ok(Some((*payload).clone()))
    }

    /// Await an admitted relay cursor becoming canonical. Polling reads only the in-memory
    /// validated frontier; no filesystem traversal or payload clone occurs.
    pub async fn await_committed_seq(
        &self,
        agent_session_id: &str,
        target_seq: u64,
        timeout: Duration,
    ) -> Result<u64> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.history_last_seq(agent_session_id) {
                Ok(committed) if committed >= target_seq => return Ok(committed),
                Ok(_) => {}
                Err(error) => return Err(error),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(error(
                    CONVERSATION_PERSISTENCE_COMMIT_INDETERMINATE,
                    "await_committed_seq",
                    "admitted event did not reach the canonical frontier before the deadline",
                ));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    pub async fn flush_catalog_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> std::result::Result<CatalogFlushReceipt, CatalogFlushError> {
        self.repository.flush_catalog_until(deadline).await
    }

    pub async fn flush_all(&self) -> Result<()> {
        // Canonical JSON/JSONL appends cross their durability boundary before returning. The
        // disposable catalog has an explicit host-owned barrier and is intentionally not flushed
        // on every ordered-writer replay barrier.
        Ok(())
    }
}

fn decimal_len(value: u64) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}

fn history_page_encoded_bytes(
    existing_record_bytes: &[usize],
    candidate_record_bytes: Option<usize>,
    next_cursor: u64,
    target_last_seq: u64,
    complete: bool,
) -> usize {
    const PREFIX: &str = "{\"schemaVersion\":1,\"records\":[";
    const NEXT: &str = "],\"nextCursor\":";
    const COMPLETE: &str = ",\"complete\":";
    const TARGET: &str = ",\"targetLastSeq\":";
    let record_count = existing_record_bytes
        .len()
        .saturating_add(usize::from(candidate_record_bytes.is_some()));
    let records = existing_record_bytes
        .iter()
        .copied()
        .chain(candidate_record_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(record_count.saturating_sub(1));
    PREFIX
        .len()
        .saturating_add(records)
        .saturating_add(NEXT.len())
        .saturating_add(decimal_len(next_cursor))
        .saturating_add(COMPLETE.len())
        .saturating_add(if complete { 4 } else { 5 })
        .saturating_add(TARGET.len())
        .saturating_add(decimal_len(target_last_seq))
        .saturating_add(1)
}

fn repository_read_code(source: &crate::conversation::repository::RepositoryError) -> &'static str {
    match source.code {
        crate::conversation::contracts::ConversationErrorCode::ConversationRecordTooLarge => {
            "CONVERSATION_RECORD_TOO_LARGE"
        }
        crate::conversation::contracts::ConversationErrorCode::ConversationPageTooLarge => {
            "CONVERSATION_PAGE_TOO_LARGE"
        }
        _ => "CONVERSATION_READ_FAILED",
    }
}

fn map_frontier_read_error(
    operation: &'static str,
    source: crate::conversation::repository::RepositoryError,
) -> ConversationPersistenceError {
    let code = match source.code {
        crate::conversation::contracts::ConversationErrorCode::ConversationRecoveryRequired
        | crate::conversation::contracts::ConversationErrorCode::ConversationCorrupt => {
            "CONVERSATION_RECOVERY_REQUIRED"
        }
        _ => "CONVERSATION_READ_FAILED",
    };
    error(code, operation, source.to_string())
}

fn map_append_error(
    operation: &'static str,
    source: crate::conversation::repository::RepositoryError,
) -> ConversationPersistenceError {
    let code = match source.code {
        crate::conversation::contracts::ConversationErrorCode::LegacyCompatibilityReadOnly => {
            "LEGACY_COMPATIBILITY_READ_ONLY"
        }
        crate::conversation::contracts::ConversationErrorCode::ConversationRecordTooLarge => {
            "CONVERSATION_RECORD_TOO_LARGE"
        }
        crate::conversation::contracts::ConversationErrorCode::ConversationConflict => {
            "CONVERSATION_CONFLICT"
        }
        crate::conversation::contracts::ConversationErrorCode::ConversationRecoveryRequired => {
            "CONVERSATION_RECOVERY_REQUIRED"
        }
        _ => "CONVERSATION_EVENT_APPEND_FAILED",
    };
    error(code, operation, source.to_string())
}

fn canonical_event_type(value: &str) -> Option<ConversationEventType> {
    match value {
        "user_prompt" => Some(ConversationEventType::UserPrompt),
        "message_chunk" => Some(ConversationEventType::MessageChunk),
        "session_info_update" => Some(ConversationEventType::SessionInfoUpdate),
        "local_title_generated" => Some(ConversationEventType::LocalTitleGenerated),
        "prompt_complete" => Some(ConversationEventType::PromptComplete),
        "tool_call" => Some(ConversationEventType::ToolCall),
        "tool_call_update" => Some(ConversationEventType::ToolCallUpdate),
        "usage_update" => Some(ConversationEventType::UsageUpdate),
        "plan_update" => Some(ConversationEventType::PlanUpdate),
        // Every other relay event consumes a payload-free canonical cursor marker.
        _ => None,
    }
}

fn legacy_event_type(value: ConversationEventType) -> Option<&'static str> {
    match value {
        ConversationEventType::UserPrompt => Some("user_prompt"),
        ConversationEventType::MessageChunk => Some("message_chunk"),
        ConversationEventType::SessionInfoUpdate => Some("session_info_update"),
        ConversationEventType::LocalTitleGenerated => Some("local_title_generated"),
        ConversationEventType::PromptComplete => Some("prompt_complete"),
        ConversationEventType::ToolCall => Some("tool_call"),
        ConversationEventType::ToolCallUpdate => Some("tool_call_update"),
        ConversationEventType::UsageUpdate => Some("usage_update"),
        ConversationEventType::PlanUpdate => Some("plan_update"),
        ConversationEventType::RelayCursorAdvanced => None,
        _ => None,
    }
}

fn acp_title_source(source: ConversationTitleSource) -> TitleSource {
    match source {
        ConversationTitleSource::BackgroundGenerated => TitleSource::BackgroundGenerated,
        ConversationTitleSource::AgentSupplied => TitleSource::AgentSupplied,
        ConversationTitleSource::DerivedFirstMessage => TitleSource::DerivedFirstMessage,
        ConversationTitleSource::LocalAlias => TitleSource::LocalAlias,
    }
}

fn history_status(
    lifecycle_state: ConversationLifecycleState,
    binding_state: AgentSessionBindingState,
) -> PersistedSessionStatus {
    match binding_state {
        AgentSessionBindingState::Active
            if lifecycle_state == ConversationLifecycleState::Ready =>
        {
            PersistedSessionStatus::Active
        }
        AgentSessionBindingState::Detached | AgentSessionBindingState::Suspended => {
            PersistedSessionStatus::Closed
        }
        AgentSessionBindingState::Active | AgentSessionBindingState::Replaced => {
            PersistedSessionStatus::Error
        }
    }
}

fn error(
    code: &'static str,
    operation: &'static str,
    detail: impl Into<String>,
) -> ConversationPersistenceError {
    ConversationPersistenceError {
        code,
        operation,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        AgentSessionBinding, AgentSessionBindingState, ConversationCreator,
        ConversationLifecycleState, ConversationRecordV2, CreationPartition, ExecutionTarget,
        AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    };
    use crate::conversation::creation::ConversationCreationService;
    use crate::conversation::locator::{ConversationLocator, SessionWorkspaceLocator};
    use chrono::TimeZone;
    use std::io::{BufWriter, Write};
    use uuid::Uuid;

    #[tokio::test]
    async fn resolves_binding_and_writes_only_the_conversation_repository() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let visible = root.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = crate::conversation::ConversationId::parse("11111111-1111-4111-8111-111111111111")
            .unwrap();
        let created_at = Utc
            .timestamp_millis_opt(1_766_000_000_000)
            .single()
            .unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: visible.join("workspace").to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::InitializingAgent,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        writer
            .bind_agent_session(
                id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "opaque/session".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: visible.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(
            Arc::clone(&writer),
            reader,
        ));
        let seq = adapter
            .append_acp_event(
                "opaque/session",
                "message_chunk",
                serde_json::json!({"role":"agent","content":{"type":"text","text":"ok"}}),
            )
            .await
            .unwrap();
        assert_eq!(seq, 2);
        assert_eq!(repository.read_events(id, 0).unwrap().len(), 2);
        assert_eq!(adapter.list_sessions().len(), 1);
        let (metadata, events) = adapter.legacy_materialization("opaque/session").unwrap();
        assert_eq!(metadata.session_id, "opaque/session");
        assert_eq!(events.len(), 1);
        adapter.flush_all().await.unwrap();
        for legacy_root in ["acp-sessions", "acp-chat-history", "workspace-manifests"] {
            assert!(!temp.path().join(legacy_root).exists());
        }
        let _ = ConversationCreationService::new(
            writer,
            ConversationLocator::new(private).unwrap(),
            SessionWorkspaceLocator::new(visible).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn history_summary_and_detached_reads_survive_restart_while_writes_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let visible = root.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse("22222222-2222-4222-8222-222222222222").unwrap();
        let created_at = Utc
            .timestamp_millis_opt(1_766_000_000_000)
            .single()
            .unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: visible.join("workspace").to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::InitializingAgent,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        writer
            .bind_agent_session(
                id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "opaque/detached".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: visible.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        for (type_, payload) in [
            (
                ConversationEventType::UserPrompt,
                serde_json::json!({"content":[{"type":"text","text":"Derived"}]}),
            ),
            (
                ConversationEventType::SessionInfoUpdate,
                serde_json::json!({"title":"Agent"}),
            ),
            (
                ConversationEventType::LocalTitleGenerated,
                serde_json::json!({"title":"Background"}),
            ),
            (
                ConversationEventType::SessionInfoUpdate,
                serde_json::json!({"title":"Ignored"}),
            ),
            (
                ConversationEventType::ToolCall,
                serde_json::json!({"toolCall":{"id":"one"}}),
            ),
            (
                ConversationEventType::ToolCallUpdate,
                serde_json::json!({"update":{"id":"one"}}),
            ),
        ] {
            writer
                .append_event(
                    id,
                    created_at,
                    type_,
                    payload,
                    ConversationMutation::AcpEventAppend,
                )
                .await
                .unwrap();
        }
        writer.detach_agent_binding(id, created_at).await.unwrap();

        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(
            Arc::clone(&writer),
            reader,
        ));
        assert!(adapter
            .conversation_id_for_active_binding("opaque/detached")
            .is_none());
        assert_eq!(
            adapter.conversation_id_for_history_binding("opaque/detached"),
            Some(id)
        );
        let sessions = adapter.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Background"));
        assert_eq!(
            sessions[0].title_source,
            Some(TitleSource::BackgroundGenerated)
        );
        assert_eq!(sessions[0].message_count, 4);
        assert_eq!(sessions[0].tool_count, 2);
        assert_eq!(sessions[0].status, PersistedSessionStatus::Closed);
        let (metadata, events) = adapter.legacy_materialization("opaque/detached").unwrap();
        assert_eq!(metadata.title, sessions[0].title);
        assert_eq!(events.len(), 6);
        assert!(adapter
            .append_acp_event("opaque/detached", "message_chunk", serde_json::json!({}))
            .await
            .is_err());
        assert_eq!(
            adapter.last_seq("opaque/detached").unwrap_err().code,
            "CONVERSATION_BINDING_NOT_FOUND"
        );
        assert!(!adapter
            .replay_after("opaque/detached", 0)
            .unwrap()
            .is_empty());

        writer
            .rebind_detached_binding(id, created_at)
            .await
            .unwrap();
        writer
            .suspend_agent_binding(id, true, created_at)
            .await
            .unwrap();
        assert_eq!(
            repository.current_binding(id).unwrap().unwrap().state,
            AgentSessionBindingState::Suspended
        );
        assert_eq!(
            adapter.conversation_id_for_history_binding("opaque/detached"),
            Some(id)
        );
        assert_eq!(
            adapter.list_sessions()[0].status,
            PersistedSessionStatus::Closed
        );
        assert!(adapter
            .append_acp_event("opaque/detached", "message_chunk", serde_json::json!({}))
            .await
            .is_err());
        assert_eq!(
            adapter.last_seq("opaque/detached").unwrap_err().code,
            "CONVERSATION_BINDING_NOT_FOUND"
        );
        assert!(!adapter
            .replay_after("opaque/detached", 0)
            .unwrap()
            .is_empty());

        drop(adapter);
        drop(repository);
        let (repository, _) = ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let reopened = ConversationPersistenceAdapter::new(writer, reader);
        let sessions = reopened.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Background"));
        assert_eq!(sessions[0].message_count, 4);
        assert_eq!(sessions[0].tool_count, 2);
        assert_eq!(sessions[0].status, PersistedSessionStatus::Closed);
    }

    #[tokio::test]
    async fn history_pages_are_bounded_and_exact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let visible = root.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse("33333333-3333-4333-8333-333333333333").unwrap();
        let created_at = Utc
            .timestamp_millis_opt(1_766_000_000_000)
            .single()
            .unwrap();
        let record = ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: id,
            created_at_utc: created_at,
            creation_partition: CreationPartition::from_created_at(created_at),
            workspace_cwd: visible.join("workspace").to_string_lossy().into_owned(),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::InitializingAgent,
            last_seq: 0,
            created_by: ConversationCreator::Termul,
            title: None,
            title_source: None,
        };
        writer
            .create_conversation(record.clone(), ConversationMutation::CreateConversation)
            .await
            .unwrap();
        writer
            .bind_agent_session(
                id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "opaque/paged".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: visible.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        let directory = private
            .join(&record.creation_partition.path)
            .join(id.to_string());
        drop(writer);
        drop(repository);

        let messages = std::fs::OpenOptions::new()
            .append(true)
            .open(directory.join(crate::conversation::event_log::MESSAGES_FILE))
            .unwrap();
        let mut messages = BufWriter::new(messages);
        for seq in 2..=1_051_u64 {
            let event = crate::conversation::event_log::ConversationEventRecordV2::new(
                id,
                seq,
                created_at,
                ConversationEventType::MessageChunk,
                serde_json::json!({"marker":seq}),
            );
            serde_json::to_writer(&mut messages, &event).unwrap();
            messages.write_all(b"\n").unwrap();
        }
        messages.flush().unwrap();
        drop(messages);

        let (repository, _) = ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = ConversationPersistenceAdapter::new(writer, reader);
        let scheduled_scope = adapter
            .scheduled_task_scope_for_session("opaque/paged")
            .expect("projectless Conversation must provide scheduled-task context");
        assert_eq!(scheduled_scope.0, None);
        assert_eq!(scheduled_scope.1, record.workspace_cwd);
        assert_eq!(scheduled_scope.2, ExecutionTarget::Workspace);
        assert_eq!(scheduled_scope.4, "stable");
        let full = repository.read_events(id, 0).unwrap();
        assert_eq!(full.len(), 1_051);
        let expected = (2..=1_051_u64).collect::<Vec<_>>();
        for limit in [1, 17, 250, 1_000] {
            let mut cursor = 0;
            let mut observed = Vec::new();
            loop {
                let page = adapter
                    .history_page("opaque/paged", cursor, limit)
                    .expect("bounded history page");
                assert_eq!(
                    page.schema_version,
                    CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION
                );
                assert_eq!(page.target_last_seq, 1_051);
                assert!(page.records.len() <= limit);
                assert!(page.next_cursor > cursor || page.complete);
                observed.extend(page.records.iter().map(|record| record.seq));
                cursor = page.next_cursor;
                if page.complete {
                    break;
                }
            }
            assert_eq!(cursor, 1_051);
            assert_eq!(observed, expected, "limit {limit} preserves exact order");
        }

        let error = adapter
            .legacy_materialization("opaque/paged")
            .expect_err("compatibility materialization must require paging above 1000 records");
        assert_eq!(error.code, CONVERSATION_HISTORY_PAGING_REQUIRED);
        for invalid_limit in [0, 1_001] {
            assert_eq!(
                adapter
                    .history_page("opaque/paged", 0, invalid_limit)
                    .unwrap_err()
                    .code,
                "VALIDATION_ERROR"
            );
        }
        assert_eq!(
            adapter
                .history_page("opaque/paged", 1_052, 17)
                .unwrap_err()
                .code,
            "VALIDATION_ERROR"
        );
        let after = adapter.replay_after("opaque/paged", 1_000).unwrap();
        assert_eq!(after.len(), 51);
        assert_eq!(after.first().unwrap().seq, 1_001);
        assert_eq!(after.last().unwrap().seq, 1_051);
    }

    #[tokio::test]
    async fn binding_miss_cache_is_bounded_and_generation_invalidated() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("private");
        let visible = root.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse("44444444-4444-4444-8444-444444444444").unwrap();
        let created_at = Utc
            .timestamp_millis_opt(1_766_000_000_000)
            .single()
            .unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: visible.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::InitializingAgent,
                    last_seq: 0,
                    created_by: ConversationCreator::Termul,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = ConversationPersistenceAdapter::new(Arc::clone(&writer), reader);

        for ordinal in 0..100_000 {
            let sid = format!("missing-{}", ordinal % 10_000);
            assert!(adapter.conversation_id_for_active_binding(&sid).is_none());
        }
        let bounded = adapter.binding_miss_cache_stats();
        assert!(bounded.entries <= MAX_BINDING_MISS_CACHE_ENTRIES);
        assert!(bounded.evictions > 0);
        assert!(adapter
            .conversation_id_for_active_binding("late-bound-session")
            .is_none());
        let previous_generation = repository.binding_generation();

        writer
            .bind_agent_session(
                id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "late-bound-session".to_string(),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "stable".to_string(),
                    execution_cwd: visible.to_string_lossy().into_owned(),
                    bound_at_utc: created_at,
                    state: AgentSessionBindingState::Active,
                },
                created_at,
            )
            .await
            .unwrap();
        assert!(repository.binding_generation() > previous_generation);
        assert_eq!(
            adapter.conversation_id_for_active_binding("late-bound-session"),
            Some(id)
        );
        let invalidated = adapter.binding_miss_cache_stats();
        assert_eq!(invalidated.generation, repository.binding_generation());
        assert_eq!(invalidated.entries, 0);
        assert_eq!(repository.binding_index_stats().conversation_count, 1);

        // Reproduce the registration race: a resolver can enqueue a miss after
        // the repository has already advanced to the generation containing the
        // binding. Re-registering identical keys keeps that generation stable,
        // but must still invalidate the stale per-session miss.
        adapter.binding_misses.lock().insert(BindingMissKey {
            kind: BindingLookupKind::Active,
            agent_session_id: "late-bound-session".to_string(),
        });
        assert!(adapter
            .conversation_id_for_active_binding("late-bound-session")
            .is_none());
        let stable_generation = repository.binding_generation();
        adapter.register_binding("late-bound-session", id);
        assert_eq!(repository.binding_generation(), stable_generation);
        assert_eq!(
            adapter.conversation_id_for_active_binding("late-bound-session"),
            Some(id)
        );
    }

    #[tokio::test]
    async fn unmapped_events_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().canonicalize().unwrap().join("private");
        let (repository, _) = ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let reader = Arc::new(crate::conversation::ConversationReader::new(
            Arc::clone(&repository),
            crate::conversation::LegacyConversationReader::default(),
            crate::conversation::ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = Arc::new(ConversationPersistenceAdapter::new(writer, reader));
        let error = adapter
            .append_acp_event("unknown", "message_chunk", serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "CONVERSATION_BINDING_NOT_FOUND");
    }
}
