//! Canonical v2 Conversation JSONL routing, validation, replay, and materialization.
//!
//! Every Conversation owns one global monotonically increasing sequence across four physical
//! streams. Stream files are append-only; replay validates their physical order, merges them by
//! `seq`, and rejects duplicate or mismatched records. Recovery repairs only an unterminated final
//! line, preserving the original bytes beside the log before atomically truncating it.

#[cfg(test)]
use std::cell::Cell;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, copy, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

use crate::conversation::contracts::{
    encoded_json_len_bounded, format_created_at_utc, parse_created_at_utc, AgentSessionBinding,
    AgentSessionBindingState, ConversationErrorCode, ConversationId, ConversationLifecycleState,
    ConversationTitleSource, ExecutionTarget, ProjectAttachment,
    AGENT_SESSION_BINDING_SCHEMA_VERSION, MAX_CONVERSATION_HISTORY_PAGE_BYTES,
    MAX_CONVERSATION_RECORD_BYTES, PROJECT_ATTACHMENT_SCHEMA_VERSION,
};
use crate::conversation::durable_fs::DurableFileSystem;

pub const CONVERSATION_EVENT_SCHEMA_VERSION: u32 = 2;
pub const SPARSE_OFFSET_STRIDE: u64 = 256;
/// Immutable sparse-index chunk size. Only the append tail is copied when an overlapping reader
/// holds the previous [`EventLogScan`] generation.
pub const SPARSE_INDEX_CHUNK_ENTRIES: usize = 1_024;
/// Immutable binding/attachment history chunk size. Entries are individually shared so cloning an
/// overlapping frontier copies at most 1024 `Arc` pointers, never opaque binding/path strings.
pub const FRONTIER_HISTORY_CHUNK_ENTRIES: usize = 1_024;
pub const MIN_EVENT_PAGE_LIMIT: usize = 1;
pub const MAX_EVENT_PAGE_LIMIT: usize = 1_000;
pub const MESSAGES_FILE: &str = "messages.jsonl";
pub const TOOL_CALLS_FILE: &str = "tool-calls.jsonl";
pub const BINDINGS_FILE: &str = "bindings.jsonl";
pub const ATTACHMENTS_FILE: &str = "attachments.jsonl";
pub const EVENT_LOG_FILES: [&str; 4] = [
    MESSAGES_FILE,
    TOOL_CALLS_FILE,
    BINDINGS_FILE,
    ATTACHMENTS_FILE,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversationEventStream {
    Messages,
    ToolCalls,
    Bindings,
    Attachments,
}

impl ConversationEventStream {
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Messages => MESSAGES_FILE,
            Self::ToolCalls => TOOL_CALLS_FILE,
            Self::Bindings => BINDINGS_FILE,
            Self::Attachments => ATTACHMENTS_FILE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationEventType {
    UserPrompt,
    MessageChunk,
    SessionInfoUpdate,
    LocalTitleGenerated,
    PromptComplete,
    ToolCall,
    ToolCallUpdate,
    UsageUpdate,
    PlanUpdate,
    /// Payload-free durable marker for a relay event that has no materialized history payload.
    RelayCursorAdvanced,
    BindingBound,
    BindingDetached,
    BindingRebound,
    BindingSuspended,
    BindingReplaced,
    ProjectAttached,
    ProjectDetached,
    ExecutionTargetUpdated,
    CreationFailed,
}

impl ConversationEventType {
    #[must_use]
    pub const fn stream(self) -> ConversationEventStream {
        match self {
            Self::ToolCall | Self::ToolCallUpdate => ConversationEventStream::ToolCalls,
            Self::BindingBound
            | Self::BindingDetached
            | Self::BindingRebound
            | Self::BindingSuspended
            | Self::BindingReplaced => ConversationEventStream::Bindings,
            Self::ProjectAttached | Self::ProjectDetached | Self::ExecutionTargetUpdated => {
                ConversationEventStream::Attachments
            }
            Self::UserPrompt
            | Self::MessageChunk
            | Self::SessionInfoUpdate
            | Self::LocalTitleGenerated
            | Self::PromptComplete
            | Self::UsageUpdate
            | Self::PlanUpdate
            | Self::RelayCursorAdvanced
            | Self::CreationFailed => ConversationEventStream::Messages,
        }
    }
}

fn serialize_utc_millis<S>(
    value: &DateTime<Utc>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format_created_at_utc(value))
}

fn deserialize_utc_millis<'de, D>(deserializer: D) -> std::result::Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse_created_at_utc(&value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEventRecordV2 {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    pub seq: u64,
    #[serde(
        serialize_with = "serialize_utc_millis",
        deserialize_with = "deserialize_utc_millis"
    )]
    pub recorded_at_utc: DateTime<Utc>,
    #[serde(rename = "type")]
    pub type_: ConversationEventType,
    pub payload: Value,
}

impl ConversationEventRecordV2 {
    #[must_use]
    pub fn new(
        conversation_id: ConversationId,
        seq: u64,
        recorded_at_utc: DateTime<Utc>,
        type_: ConversationEventType,
        payload: Value,
    ) -> Self {
        Self {
            schema_version: CONVERSATION_EVENT_SCHEMA_VERSION,
            conversation_id,
            seq,
            recorded_at_utc,
            type_,
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventRecordEncodingError {
    Serialization,
    TooLarge,
}

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedJsonBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedJsonBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(next) = self.bytes.len().checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded event length overflow",
            ));
        };
        if next > self.limit {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded event exceeds configured limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize one canonical record into a bounded buffer. The writer stops as soon as the stable
/// 256 KiB limit is crossed, so no unbounded intermediate `Vec` is created.
pub(crate) fn encode_event_record_bounded(
    record: &ConversationEventRecordV2,
) -> std::result::Result<Vec<u8>, EventRecordEncodingError> {
    let mut writer = BoundedJsonBuffer::new(MAX_CONVERSATION_RECORD_BYTES);
    match serde_json::to_writer(&mut writer, record) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.exceeded => Err(EventRecordEncodingError::TooLarge),
        Err(_) => Err(EventRecordEncodingError::Serialization),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingEventPayloadV1 {
    pub binding: AgentSessionBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingReplacementPayloadV1 {
    pub previous_binding: AgentSessionBinding,
    pub binding: AgentSessionBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAttachmentEventPayloadV1 {
    pub attachment: ProjectAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTargetEventPayloadV1 {
    pub execution_target: ExecutionTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventLogRepairWarning {
    pub conversation_id: ConversationId,
    pub stream: String,
    pub backup_file: String,
    pub truncated_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationSummaryFrontier {
    pub title: Option<String>,
    pub title_source: Option<ConversationTitleSource>,
    pub last_activity_at_utc: Option<DateTime<Utc>>,
    pub message_count: u64,
    pub tool_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConversationFrontier {
    pub binding: BindingMaterialization,
    pub attachment: AttachmentMaterialization,
    pub execution_target: Option<ExecutionTarget>,
    pub summary: ConversationSummaryFrontier,
    /// Latest full canonical usage replacement stored as an immutable Arc so
    /// overlap clones are pointer clones. Absent means no usage update has ever committed.
    pub latest_usage: Option<Arc<Value>>,
    /// Latest full canonical plan replacement stored as an immutable Arc.
    /// An empty `entries` array is a durable clear.
    pub latest_plan: Option<Arc<Value>>,
    pub lifecycle_state: Option<ConversationLifecycleState>,
    pub last_seq: u64,
}

impl Eq for ConversationFrontier {}

impl ConversationFrontier {
    /// Cheap accessor that clones the Arc, not the JSON tree.
    #[must_use]
    pub fn latest_usage_arc(&self) -> Option<Arc<Value>> {
        self.latest_usage.clone()
    }

    /// Cheap accessor that clones the Arc, not the JSON tree.
    #[must_use]
    pub fn latest_plan_arc(&self) -> Option<Arc<Value>> {
        self.latest_plan.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SparseEventOffset {
    pub seq: u64,
    pub byte_offset: u64,
}

/// Append-bounded sparse offsets: full 1024-entry chunks are immutable and shared between scan
/// generations; only the current tail is copied by [`Clone`]. The fixed chunk size also provides
/// O(1) logical indexing, allowing one `partition_point` over the complete logical sequence.
#[derive(Debug, PartialEq, Eq)]
pub struct SparseOffsetEntries {
    chunks: Arc<Vec<Arc<Vec<SparseEventOffset>>>>,
    tail: Vec<SparseEventOffset>,
}

impl Default for SparseOffsetEntries {
    fn default() -> Self {
        Self {
            chunks: Arc::new(Vec::new()),
            tail: Vec::new(),
        }
    }
}

impl Clone for SparseOffsetEntries {
    fn clone(&self) -> Self {
        let mut tail = if self.tail.is_empty() {
            Vec::new()
        } else {
            Vec::with_capacity(SPARSE_INDEX_CHUNK_ENTRIES)
        };
        tail.extend_from_slice(&self.tail);
        #[cfg(test)]
        record_sparse_index_clone(
            self.tail.len(),
            tail.capacity()
                .saturating_mul(std::mem::size_of::<SparseEventOffset>()),
        );
        Self {
            chunks: Arc::clone(&self.chunks),
            tail,
        }
    }
}

impl SparseOffsetEntries {
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks
            .len()
            .saturating_mul(SPARSE_INDEX_CHUNK_ENTRIES)
            .saturating_add(self.tail.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.tail.is_empty()
    }

    #[must_use]
    pub fn first(&self) -> Option<&SparseEventOffset> {
        self.get(0)
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&SparseEventOffset> {
        let sealed_len = self.chunks.len().saturating_mul(SPARSE_INDEX_CHUNK_ENTRIES);
        if index < sealed_len {
            let chunk_index = index / SPARSE_INDEX_CHUNK_ENTRIES;
            let entry_index = index % SPARSE_INDEX_CHUNK_ENTRIES;
            self.chunks.get(chunk_index)?.get(entry_index)
        } else {
            self.tail.get(index.saturating_sub(sealed_len))
        }
    }

    fn push(&mut self, entry: SparseEventOffset) {
        if self.tail.is_empty() && self.tail.capacity() < SPARSE_INDEX_CHUNK_ENTRIES {
            self.tail.reserve_exact(SPARSE_INDEX_CHUNK_ENTRIES);
            #[cfg(test)]
            record_sparse_index_allocation(
                SPARSE_INDEX_CHUNK_ENTRIES.saturating_mul(std::mem::size_of::<SparseEventOffset>()),
            );
        }
        self.tail.push(entry);
        if self.tail.len() == SPARSE_INDEX_CHUNK_ENTRIES {
            let sealed = Arc::new(std::mem::take(&mut self.tail));
            #[cfg(test)]
            if Arc::strong_count(&self.chunks) > 1 {
                record_sparse_index_allocation(
                    self.chunks
                        .len()
                        .saturating_mul(std::mem::size_of::<Arc<Vec<SparseEventOffset>>>()),
                );
            }
            Arc::make_mut(&mut self.chunks).push(sealed);
        }
    }

    /// Logical equivalent of slice [`partition_point`](slice::partition_point), with deterministic
    /// comparison instrumentation in tests. Logical indexing is O(1) because every sealed chunk
    /// has exactly [`SPARSE_INDEX_CHUNK_ENTRIES`] entries.
    fn partition_point<P>(&self, mut predicate: P) -> usize
    where
        P: FnMut(&SparseEventOffset) -> bool,
    {
        let mut left = 0usize;
        let mut size = self.len();
        while size > 0 {
            let half = size / 2;
            let middle = left + half;
            #[cfg(test)]
            SPARSE_LOOKUP_COMPARISONS.set(SPARSE_LOOKUP_COMPARISONS.get().saturating_add(1));
            if predicate(
                self.get(middle)
                    .expect("logical sparse-index position must exist"),
            ) {
                left = middle + 1;
                size -= half + 1;
            } else {
                size = half;
            }
        }
        left
    }

    #[must_use]
    fn anchor_at_or_before(&self, after_seq: u64) -> Option<SparseEventOffset> {
        let insertion = self.partition_point(|entry| entry.seq <= after_seq);
        if insertion == 0 {
            self.first().copied()
        } else {
            self.get(insertion - 1).copied()
        }
    }

    /// Compatibility iterator used by repository validation tests. Sparse entries are `Copy`, so
    /// a small requested window does not expose or flatten the chunk storage.
    pub fn windows(&self, size: usize) -> impl Iterator<Item = Vec<SparseEventOffset>> + '_ {
        assert!(size > 0, "window size must be non-zero");
        let window_count = self.len().checked_sub(size).map_or(0, |count| count + 1);
        (0..window_count).map(move |start| {
            (start..start + size)
                .map(|index| {
                    *self
                        .get(index)
                        .expect("sparse-index window position must exist")
                })
                .collect()
        })
    }

    #[cfg(test)]
    fn get_mut_for_test(&mut self, index: usize) -> Option<&mut SparseEventOffset> {
        let sealed_len = self.chunks.len().saturating_mul(SPARSE_INDEX_CHUNK_ENTRIES);
        if index < sealed_len {
            let chunk_index = index / SPARSE_INDEX_CHUNK_ENTRIES;
            let entry_index = index % SPARSE_INDEX_CHUNK_ENTRIES;
            Arc::make_mut(Arc::make_mut(&mut self.chunks).get_mut(chunk_index)?)
                .get_mut(entry_index)
        } else {
            self.tail.get_mut(index.saturating_sub(sealed_len))
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamSparseOffsets {
    pub entries: SparseOffsetEntries,
    pub event_count: u64,
    pub validated_bytes: u64,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventLogSparseOffsets {
    pub messages: StreamSparseOffsets,
    pub tool_calls: StreamSparseOffsets,
    pub bindings: StreamSparseOffsets,
    pub attachments: StreamSparseOffsets,
}

impl EventLogSparseOffsets {
    #[must_use]
    pub fn stream(&self, stream: ConversationEventStream) -> &StreamSparseOffsets {
        match stream {
            ConversationEventStream::Messages => &self.messages,
            ConversationEventStream::ToolCalls => &self.tool_calls,
            ConversationEventStream::Bindings => &self.bindings,
            ConversationEventStream::Attachments => &self.attachments,
        }
    }

    fn stream_mut(&mut self, stream: ConversationEventStream) -> &mut StreamSparseOffsets {
        match stream {
            ConversationEventStream::Messages => &mut self.messages,
            ConversationEventStream::ToolCalls => &mut self.tool_calls,
            ConversationEventStream::Bindings => &mut self.bindings,
            ConversationEventStream::Attachments => &mut self.attachments,
        }
    }

    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.messages.entries.len()
            + self.tool_calls.entries.len()
            + self.bindings.entries.len()
            + self.attachments.entries.len()
    }

    #[must_use]
    pub fn event_count(&self) -> u64 {
        self.messages
            .event_count
            .saturating_add(self.tool_calls.event_count)
            .saturating_add(self.bindings.event_count)
            .saturating_add(self.attachments.event_count)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventLogScan {
    pub frontier: ConversationFrontier,
    pub repairs: Vec<EventLogRepairWarning>,
    pub sparse_offsets: EventLogSparseOffsets,
}

/// Source-compatible compact replacement for the historical replay container.
pub type ConversationReplay = EventLogScan;

impl EventLogScan {
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.frontier.last_seq
    }

    #[must_use]
    pub fn last_recorded_at_utc(&self) -> Option<DateTime<Utc>> {
        self.frontier.summary.last_activity_at_utc
    }

    #[must_use]
    pub fn event_count(&self) -> u64 {
        self.sparse_offsets.event_count()
    }

    #[must_use]
    pub fn sparse_index_entry_count(&self) -> usize {
        self.sparse_offsets.entry_count()
    }

    pub(crate) fn record_appended(
        &mut self,
        stream: ConversationEventStream,
        seq: u64,
        byte_offset: u64,
        encoded_line_bytes: u64,
    ) {
        let offsets = self.sparse_offsets.stream_mut(stream);
        debug_assert_eq!(offsets.validated_bytes, byte_offset);
        debug_assert!(seq > offsets.last_seq);
        if offsets.event_count.is_multiple_of(SPARSE_OFFSET_STRIDE) {
            offsets.entries.push(SparseEventOffset { seq, byte_offset });
        }
        offsets.event_count = offsets.event_count.saturating_add(1);
        offsets.validated_bytes = byte_offset.saturating_add(encoded_line_bytes);
        offsets.last_seq = seq;
    }
}

#[cfg(test)]
thread_local! {
    static APPLY_EVENT_COUNT: Cell<u64> = const { Cell::new(0) };
    static FULL_MATERIALIZATION_COUNT: Cell<u64> = const { Cell::new(0) };
    static SPARSE_LOOKUP_COMPARISONS: Cell<usize> = const { Cell::new(0) };
    static SPARSE_INDEX_COPIED_ENTRIES: Cell<usize> = const { Cell::new(0) };
    static SPARSE_INDEX_ALLOCATED_BYTES: Cell<usize> = const { Cell::new(0) };
    static FRONTIER_HISTORY_COPIED_ENTRIES: Cell<usize> = const { Cell::new(0) };
    static TOTAL_OVERLAP_ALLOCATED_BYTES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn record_sparse_index_clone(copied_entries: usize, allocated_bytes: usize) {
    SPARSE_INDEX_COPIED_ENTRIES.set(
        SPARSE_INDEX_COPIED_ENTRIES
            .get()
            .saturating_add(copied_entries),
    );
    record_sparse_index_allocation(allocated_bytes);
}

#[cfg(test)]
fn record_sparse_index_allocation(allocated_bytes: usize) {
    SPARSE_INDEX_ALLOCATED_BYTES.set(
        SPARSE_INDEX_ALLOCATED_BYTES
            .get()
            .saturating_add(allocated_bytes),
    );
    record_total_overlap_allocation(allocated_bytes);
}

#[cfg(test)]
fn record_frontier_history_clone(copied_entries: usize, allocated_bytes: usize) {
    FRONTIER_HISTORY_COPIED_ENTRIES.set(
        FRONTIER_HISTORY_COPIED_ENTRIES
            .get()
            .saturating_add(copied_entries),
    );
    record_total_overlap_allocation(allocated_bytes);
}

#[cfg(test)]
fn record_total_overlap_allocation(allocated_bytes: usize) {
    TOTAL_OVERLAP_ALLOCATED_BYTES.set(
        TOTAL_OVERLAP_ALLOCATED_BYTES
            .get()
            .saturating_add(allocated_bytes),
    );
}

#[cfg(test)]
pub(crate) fn reset_operation_counters() {
    APPLY_EVENT_COUNT.set(0);
    FULL_MATERIALIZATION_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn operation_counters() -> (u64, u64) {
    (APPLY_EVENT_COUNT.get(), FULL_MATERIALIZATION_COUNT.get())
}

#[cfg(test)]
fn reset_sparse_lookup_comparisons() {
    SPARSE_LOOKUP_COMPARISONS.set(0);
}

#[cfg(test)]
fn sparse_lookup_comparisons() -> usize {
    SPARSE_LOOKUP_COMPARISONS.get()
}

#[cfg(test)]
fn reset_sparse_index_clone_metrics() {
    SPARSE_INDEX_COPIED_ENTRIES.set(0);
    SPARSE_INDEX_ALLOCATED_BYTES.set(0);
    FRONTIER_HISTORY_COPIED_ENTRIES.set(0);
    TOTAL_OVERLAP_ALLOCATED_BYTES.set(0);
}

#[cfg(test)]
fn sparse_index_clone_metrics() -> (usize, usize) {
    (
        SPARSE_INDEX_COPIED_ENTRIES.get(),
        SPARSE_INDEX_ALLOCATED_BYTES.get(),
    )
}

#[cfg(test)]
fn total_overlap_clone_metrics() -> (usize, usize) {
    (
        FRONTIER_HISTORY_COPIED_ENTRIES.get(),
        TOTAL_OVERLAP_ALLOCATED_BYTES.get(),
    )
}

/// Append-oriented immutable chunk history. Sealed chunks and every contained value are shared;
/// cloning an [`EventLogScan`] copies only the at-most-1024 pointer tail.
#[derive(Debug, PartialEq, Eq)]
pub struct ChunkedHistory<T> {
    chunks: Arc<Vec<Arc<Vec<Arc<T>>>>>,
    tail: Vec<Arc<T>>,
}

impl<T> Default for ChunkedHistory<T> {
    fn default() -> Self {
        Self {
            chunks: Arc::new(Vec::new()),
            tail: Vec::new(),
        }
    }
}

impl<T> Clone for ChunkedHistory<T> {
    fn clone(&self) -> Self {
        let mut tail = Vec::with_capacity(self.tail.len());
        tail.extend(self.tail.iter().cloned());
        #[cfg(test)]
        record_frontier_history_clone(
            self.tail.len(),
            tail.capacity()
                .saturating_mul(std::mem::size_of::<Arc<T>>()),
        );
        Self {
            chunks: Arc::clone(&self.chunks),
            tail,
        }
    }
}

impl<T> ChunkedHistory<T> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks
            .len()
            .saturating_mul(FRONTIER_HISTORY_CHUNK_ENTRIES)
            .saturating_add(self.tail.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.tail.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks
            .iter()
            .flat_map(|chunk| chunk.iter().map(AsRef::as_ref))
            .chain(self.tail.iter().map(AsRef::as_ref))
    }

    fn push(&mut self, value: T) {
        if self.tail.capacity() < FRONTIER_HISTORY_CHUNK_ENTRIES {
            let additional = FRONTIER_HISTORY_CHUNK_ENTRIES.saturating_sub(self.tail.capacity());
            self.tail.reserve_exact(additional);
            #[cfg(test)]
            record_total_overlap_allocation(
                additional.saturating_mul(std::mem::size_of::<Arc<T>>()),
            );
        }
        self.tail.push(Arc::new(value));
        if self.tail.len() == FRONTIER_HISTORY_CHUNK_ENTRIES {
            let sealed = Arc::new(std::mem::take(&mut self.tail));
            #[cfg(test)]
            if Arc::strong_count(&self.chunks) > 1 {
                record_total_overlap_allocation(
                    self.chunks
                        .len()
                        .saturating_mul(std::mem::size_of::<Arc<Vec<Arc<T>>>>()),
                );
            }
            Arc::make_mut(&mut self.chunks).push(sealed);
        }
    }

    fn replace(&mut self, index: usize, value: T) -> bool {
        let sealed_len = self
            .chunks
            .len()
            .saturating_mul(FRONTIER_HISTORY_CHUNK_ENTRIES);
        if index < sealed_len {
            let chunk_index = index / FRONTIER_HISTORY_CHUNK_ENTRIES;
            let entry_index = index % FRONTIER_HISTORY_CHUNK_ENTRIES;
            #[cfg(test)]
            if Arc::strong_count(&self.chunks) > 1 {
                record_total_overlap_allocation(
                    self.chunks
                        .len()
                        .saturating_mul(std::mem::size_of::<Arc<Vec<Arc<T>>>>()),
                );
            }
            let chunks = Arc::make_mut(&mut self.chunks);
            let Some(chunk) = chunks.get_mut(chunk_index) else {
                return false;
            };
            #[cfg(test)]
            if Arc::strong_count(chunk) > 1 {
                record_total_overlap_allocation(
                    FRONTIER_HISTORY_CHUNK_ENTRIES.saturating_mul(std::mem::size_of::<Arc<T>>()),
                );
            }
            let entries = Arc::make_mut(chunk);
            let Some(entry) = entries.get_mut(entry_index) else {
                return false;
            };
            *entry = Arc::new(value);
            true
        } else if let Some(entry) = self.tail.get_mut(index.saturating_sub(sealed_len)) {
            *entry = Arc::new(value);
            true
        } else {
            false
        }
    }
}

impl<T: Clone> ChunkedHistory<T> {
    #[must_use]
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingMaterialization {
    pub current: Option<AgentSessionBinding>,
    pub history: ChunkedHistory<AgentSessionBinding>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachmentMaterialization {
    pub current: Option<ProjectAttachment>,
    pub history: ChunkedHistory<ProjectAttachment>,
    pub has_events: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLogErrorKind {
    Io,
    InvalidPageLimit,
    CorruptRecord,
    UnsupportedSchema,
    ConversationMismatch,
    StreamMismatch,
    SequenceConflict,
    InvalidBindingHistory,
    InvalidAttachmentHistory,
    RecordTooLarge,
    PageTooLarge,
    Durability,
}

#[derive(Debug)]
pub struct EventLogError {
    pub code: ConversationErrorCode,
    pub kind: EventLogErrorKind,
    pub conversation_id: ConversationId,
    pub path: PathBuf,
    pub seq: Option<u64>,
    pub byte_offset: Option<u64>,
    pub detail: String,
}

impl fmt::Display for EventLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} for Conversation {} at '{}': {}",
            self.stable_code(),
            self.conversation_id,
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for EventLogError {}

impl EventLogError {
    #[must_use]
    pub fn stable_code(&self) -> String {
        match self.kind {
            EventLogErrorKind::InvalidPageLimit => "VALIDATION_ERROR".to_string(),
            EventLogErrorKind::RecordTooLarge => "CONVERSATION_RECORD_TOO_LARGE".to_string(),
            EventLogErrorKind::PageTooLarge => "CONVERSATION_PAGE_TOO_LARGE".to_string(),
            _ => stable_error_code(self.code),
        }
    }
}

pub type Result<T> = std::result::Result<T, EventLogError>;

/// Stream, conservatively repair, globally merge, and materialize compact canonical state.
///
/// At most one decoded record per physical stream is retained while scanning. Historical payloads
/// are discarded after they update the compact frontier; only every 256th physical record offset
/// remains for later bounded page reads.
pub fn scan_event_log(
    directory: &Path,
    conversation_id: ConversationId,
    durable_fs: &DurableFileSystem,
) -> Result<EventLogScan> {
    let mut scanners = event_streams()
        .into_iter()
        .map(|stream| StreamScanner::open(directory, conversation_id, stream))
        .collect::<Result<Vec<_>>>()?;
    let mut pending = scanners
        .iter_mut()
        .map(|scanner| scanner.next_record(directory, conversation_id, durable_fs))
        .collect::<Result<Vec<_>>>()?;
    let mut frontier = ConversationFrontier::default();

    while let Some(next_index) = pending
        .iter()
        .enumerate()
        .filter_map(|(index, record)| record.as_ref().map(|record| (index, record.seq)))
        .min_by_key(|(_, seq)| *seq)
        .map(|(index, _)| index)
    {
        let record = pending[next_index]
            .take()
            .expect("selected pending event exists");
        if record.seq == 0 || record.seq <= frontier.last_seq {
            return Err(error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::SequenceConflict,
                conversation_id,
                directory,
                format!("global sequence conflict at seq {}", record.seq),
            ));
        }
        apply_event(&mut frontier, &record)?;
        pending[next_index] =
            scanners[next_index].next_record(directory, conversation_id, durable_fs)?;
    }

    let mut repairs = Vec::new();
    let mut sparse_offsets = EventLogSparseOffsets::default();
    for scanner in scanners {
        repairs.extend(scanner.repairs);
        *sparse_offsets.stream_mut(scanner.stream) = scanner.offsets;
    }
    Ok(EventLogScan {
        frontier,
        repairs,
        sparse_offsets,
    })
}

/// Compatibility name retained for callers that previously replayed during validation.
/// The returned value is compact and contains no historical payload vector.
pub fn replay_conversation(
    directory: &Path,
    conversation_id: ConversationId,
    durable_fs: &DurableFileSystem,
) -> Result<EventLogScan> {
    scan_event_log(directory, conversation_id, durable_fs)
}

/// Validate and materialize an already seq-ordered record set.
///
/// This compatibility helper is intentionally reserved for explicit recovery/tests. Live appends
/// call [`apply_event`] once against the compact in-memory frontier.
pub fn materialize_records(
    records: &[ConversationEventRecordV2],
    conversation_id: ConversationId,
    directory: &Path,
) -> Result<(BindingMaterialization, AttachmentMaterialization)> {
    #[cfg(test)]
    FULL_MATERIALIZATION_COUNT.set(FULL_MATERIALIZATION_COUNT.get() + 1);
    let mut frontier = ConversationFrontier::default();
    for record in records {
        if record.conversation_id != conversation_id {
            return Err(error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::ConversationMismatch,
                conversation_id,
                directory,
                format!("record seq {} has a mismatched conversationId", record.seq),
            ));
        }
        apply_event(&mut frontier, record)?;
    }
    Ok((frontier.binding, frontier.attachment))
}

/// Apply one validated canonical event to the compact Conversation frontier.
pub fn apply_event(
    frontier: &mut ConversationFrontier,
    record: &ConversationEventRecordV2,
) -> Result<()> {
    let path = Path::new(record.type_.stream().file_name());
    if record.schema_version != CONVERSATION_EVENT_SCHEMA_VERSION {
        return Err(error(
            ConversationErrorCode::ConversationUnsupportedSchema,
            EventLogErrorKind::UnsupportedSchema,
            record.conversation_id,
            path,
            format!(
                "event schemaVersion {} is unsupported; expected {}",
                record.schema_version, CONVERSATION_EVENT_SCHEMA_VERSION
            ),
        ));
    }
    if record.seq == 0 || record.seq <= frontier.last_seq {
        return Err(error(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::SequenceConflict,
            record.conversation_id,
            path,
            format!("global sequence conflict at seq {}", record.seq),
        ));
    }

    apply_binding_event(&mut frontier.binding, record, path)?;
    apply_attachment_event(&mut frontier.attachment, record, path)?;
    apply_execution_target_event(&mut frontier.execution_target, record, path)?;
    apply_durable_replacements(frontier, record, path)?;
    apply_summary_event(&mut frontier.summary, record)?;
    match record.type_ {
        ConversationEventType::CreationFailed => {
            frontier.lifecycle_state = Some(ConversationLifecycleState::AgentFailed);
        }
        ConversationEventType::BindingBound
        | ConversationEventType::BindingReplaced
        | ConversationEventType::BindingRebound => {
            frontier.lifecycle_state = Some(ConversationLifecycleState::Ready);
        }
        _ => {}
    }
    frontier.last_seq = record.seq;
    #[cfg(test)]
    APPLY_EVENT_COUNT.set(APPLY_EVENT_COUNT.get() + 1);
    Ok(())
}

const fn event_streams() -> [ConversationEventStream; 4] {
    [
        ConversationEventStream::Messages,
        ConversationEventStream::ToolCalls,
        ConversationEventStream::Bindings,
        ConversationEventStream::Attachments,
    ]
}

enum BoundedJsonLine {
    Eof,
    Complete(Vec<u8>),
    Torn(Vec<u8>),
}

enum BoundedJsonLineError {
    Io(io::Error),
    TooLarge,
}

/// Read at most one record plus newline and one overflow sentinel byte. `Read::take` bounds the
/// allocation even for an imported JSONL line with no newline.
fn read_bounded_jsonl_line(
    reader: &mut impl BufRead,
) -> std::result::Result<BoundedJsonLine, BoundedJsonLineError> {
    let mut line = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader.fill_buf().map_err(BoundedJsonLineError::Io)?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(BoundedJsonLine::Eof)
            } else {
                Ok(BoundedJsonLine::Torn(line))
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            let content_bytes = line.len().saturating_add(newline);
            if content_bytes > MAX_CONVERSATION_RECORD_BYTES {
                return Err(BoundedJsonLineError::TooLarge);
            }
            line.extend_from_slice(&available[..=newline]);
            reader.consume(newline + 1);
            return Ok(BoundedJsonLine::Complete(line));
        }
        if line.len().saturating_add(available.len()) > MAX_CONVERSATION_RECORD_BYTES {
            return Err(BoundedJsonLineError::TooLarge);
        }
        let consumed = available.len();
        line.extend_from_slice(available);
        reader.consume(consumed);
    }
}

struct StreamScanner {
    stream: ConversationEventStream,
    path: PathBuf,
    reader: Option<BufReader<File>>,
    offset: u64,
    previous_seq: u64,
    offsets: StreamSparseOffsets,
    repairs: Vec<EventLogRepairWarning>,
}

impl StreamScanner {
    fn open(
        directory: &Path,
        conversation_id: ConversationId,
        stream: ConversationEventStream,
    ) -> Result<Self> {
        let path = directory.join(stream.file_name());
        let file = File::open(&path).map_err(|source| {
            error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::Io,
                conversation_id,
                &path,
                format!("required stream cannot be read: {source}"),
            )
        })?;
        Ok(Self {
            stream,
            path,
            reader: Some(BufReader::new(file)),
            offset: 0,
            previous_seq: 0,
            offsets: StreamSparseOffsets::default(),
            repairs: Vec::new(),
        })
    }

    fn next_record(
        &mut self,
        directory: &Path,
        conversation_id: ConversationId,
        durable_fs: &DurableFileSystem,
    ) -> Result<Option<ConversationEventRecordV2>> {
        let line_offset = self.offset;
        let line = match read_bounded_jsonl_line(
            self.reader
                .as_mut()
                .expect("stream reader remains open until EOF or torn-tail repair"),
        ) {
            Ok(BoundedJsonLine::Eof) => {
                self.offsets.validated_bytes = self.offset;
                return Ok(None);
            }
            Ok(BoundedJsonLine::Complete(line)) => line,
            Ok(BoundedJsonLine::Torn(_line)) => {
                self.reader.take();
                let warning = repair_torn_tail(
                    directory,
                    conversation_id,
                    self.stream,
                    &self.path,
                    line_offset,
                    durable_fs,
                )?;
                self.offsets.validated_bytes = line_offset;
                self.repairs.push(warning);
                return Ok(None);
            }
            Err(BoundedJsonLineError::Io(source)) => {
                return Err(error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::Io,
                    conversation_id,
                    &self.path,
                    format!("stream read failed at byte offset {line_offset}: {source}"),
                ));
            }
            Err(BoundedJsonLineError::TooLarge) => {
                return Err(error_at(
                    ConversationErrorCode::ConversationRecordTooLarge,
                    EventLogErrorKind::RecordTooLarge,
                    conversation_id,
                    &self.path,
                    None,
                    Some(line_offset),
                    format!("encoded JSONL record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"),
                ));
            }
        };
        let bytes_read = line.len();
        self.offset = self.offset.checked_add(bytes_read as u64).ok_or_else(|| {
            error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &self.path,
                "stream byte offset overflow".to_string(),
            )
        })?;
        let record = decode_record(
            &line[..line.len() - 1],
            conversation_id,
            self.stream,
            &self.path,
            line_offset,
            self.previous_seq,
        )?;
        if self
            .offsets
            .event_count
            .is_multiple_of(SPARSE_OFFSET_STRIDE)
        {
            self.offsets.entries.push(SparseEventOffset {
                seq: record.seq,
                byte_offset: line_offset,
            });
        }
        self.offsets.event_count = self.offsets.event_count.checked_add(1).ok_or_else(|| {
            error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &self.path,
                "stream event count overflow".to_string(),
            )
        })?;
        self.offsets.validated_bytes = self.offset;
        self.offsets.last_seq = record.seq;
        self.previous_seq = record.seq;
        Ok(Some(record))
    }
}

fn repair_torn_tail(
    directory: &Path,
    conversation_id: ConversationId,
    stream: ConversationEventStream,
    path: &Path,
    valid_bytes: u64,
    durable_fs: &DurableFileSystem,
) -> Result<EventLogRepairWarning> {
    let backup_name = format!("{}.corrupt-{}.bak", stream.file_name(), Uuid::new_v4());
    let backup = directory.join(&backup_name);
    let mut source_file = File::open(path).map_err(|source| {
        error(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::Io,
            conversation_id,
            path,
            format!("torn stream cannot be reopened for repair: {source}"),
        )
    })?;
    let mut backup_options = OpenOptions::new();
    backup_options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        backup_options.mode(0o600);
    }
    let mut backup_file = backup_options.open(&backup).map_err(|source| {
        error(
            ConversationErrorCode::ConversationDurabilityFailed,
            EventLogErrorKind::Durability,
            conversation_id,
            path,
            format!("torn-tail backup creation failed: {source}"),
        )
    })?;
    let copied_bytes = match copy(&mut source_file, &mut backup_file) {
        Ok(copied_bytes) => copied_bytes,
        Err(source) => {
            drop(backup_file);
            drop(source_file);
            let _ = fs::remove_file(&backup);
            return Err(error(
                ConversationErrorCode::ConversationDurabilityFailed,
                EventLogErrorKind::Durability,
                conversation_id,
                path,
                format!("torn-tail backup copy failed: {source}"),
            ));
        }
    };
    backup_file.flush().map_err(|source| {
        error(
            ConversationErrorCode::ConversationDurabilityFailed,
            EventLogErrorKind::Durability,
            conversation_id,
            path,
            format!("torn-tail backup flush failed: {source}"),
        )
    })?;
    drop(backup_file);
    drop(source_file);
    if copied_bytes < valid_bytes {
        return Err(error(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::CorruptRecord,
            conversation_id,
            path,
            "stream changed while repairing its torn tail".to_string(),
        ));
    }
    durable_fs
        .sync_file_and_namespace(&backup)
        .map_err(|source| {
            error(
                ConversationErrorCode::ConversationDurabilityFailed,
                EventLogErrorKind::Durability,
                conversation_id,
                path,
                format!("torn-tail backup durability failed: {source}"),
            )
        })?;
    let original = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| {
            error(
                ConversationErrorCode::ConversationDurabilityFailed,
                EventLogErrorKind::Durability,
                conversation_id,
                path,
                format!("torn-tail truncation open failed: {source}"),
            )
        })?;
    original.set_len(valid_bytes).map_err(|source| {
        error(
            ConversationErrorCode::ConversationDurabilityFailed,
            EventLogErrorKind::Durability,
            conversation_id,
            path,
            format!("torn-tail truncation failed: {source}"),
        )
    })?;
    drop(original);
    durable_fs.sync_file_and_namespace(path).map_err(|source| {
        error(
            ConversationErrorCode::ConversationDurabilityFailed,
            EventLogErrorKind::Durability,
            conversation_id,
            path,
            format!("torn-tail truncation durability failed: {source}"),
        )
    })?;
    let truncated_bytes = copied_bytes - valid_bytes;
    log::warn!(
        "[conversation-repository] torn tail repaired conversation_id={} stream={} truncated_bytes={}",
        conversation_id,
        stream.file_name(),
        truncated_bytes
    );
    Ok(EventLogRepairWarning {
        conversation_id,
        stream: stream.file_name().to_string(),
        backup_file: backup_name,
        truncated_bytes,
    })
}

fn decode_record(
    line: &[u8],
    conversation_id: ConversationId,
    stream: ConversationEventStream,
    path: &Path,
    offset: u64,
    previous_seq: u64,
) -> Result<ConversationEventRecordV2> {
    if line.len() > MAX_CONVERSATION_RECORD_BYTES {
        return Err(error_at(
            ConversationErrorCode::ConversationRecordTooLarge,
            EventLogErrorKind::RecordTooLarge,
            conversation_id,
            path,
            None,
            Some(offset),
            format!("encoded JSONL record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"),
        ));
    }
    if line.is_empty() {
        return Err(error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::CorruptRecord,
            conversation_id,
            path,
            None,
            Some(offset),
            format!("empty JSONL record at byte offset {offset}"),
        ));
    }
    let value: Value = serde_json::from_slice(line).map_err(|source| {
        error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::CorruptRecord,
            conversation_id,
            path,
            None,
            Some(offset),
            format!("invalid newline-terminated JSON record at byte offset {offset}: {source}"),
        )
    })?;
    let found_version = value.get("schemaVersion").and_then(Value::as_u64);
    if found_version != Some(u64::from(CONVERSATION_EVENT_SCHEMA_VERSION)) {
        return Err(error_at(
            ConversationErrorCode::ConversationUnsupportedSchema,
            EventLogErrorKind::UnsupportedSchema,
            conversation_id,
            path,
            value.get("seq").and_then(Value::as_u64),
            Some(offset),
            format!(
                "event schemaVersion {:?} is unsupported; expected {}",
                found_version, CONVERSATION_EVENT_SCHEMA_VERSION
            ),
        ));
    }
    let canonical_id = conversation_id.to_string();
    if value.get("conversationId").and_then(Value::as_str) != Some(canonical_id.as_str()) {
        return Err(error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::ConversationMismatch,
            conversation_id,
            path,
            value.get("seq").and_then(Value::as_u64),
            Some(offset),
            format!(
                "record at byte offset {offset} has a non-canonical or mismatched conversationId"
            ),
        ));
    }
    let record: ConversationEventRecordV2 = serde_json::from_value(value).map_err(|source| {
        error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::CorruptRecord,
            conversation_id,
            path,
            None,
            Some(offset),
            format!("invalid v2 event at byte offset {offset}: {source}"),
        )
    })?;
    if record.conversation_id != conversation_id {
        return Err(error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::ConversationMismatch,
            conversation_id,
            path,
            Some(record.seq),
            Some(offset),
            format!("record seq {} has a mismatched conversationId", record.seq),
        ));
    }
    if record.type_.stream() != stream {
        return Err(error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::StreamMismatch,
            conversation_id,
            path,
            Some(record.seq),
            Some(offset),
            format!("record seq {} is routed to the wrong stream", record.seq),
        ));
    }
    if record.seq == 0 || record.seq <= previous_seq {
        return Err(error_at(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::SequenceConflict,
            conversation_id,
            path,
            Some(record.seq),
            Some(offset),
            format!(
                "physical stream sequence decreases or duplicates at {}",
                record.seq
            ),
        ));
    }
    Ok(record)
}

struct PageStreamReader {
    stream: ConversationEventStream,
    path: PathBuf,
    reader: BufReader<File>,
    offset: u64,
    validated_bytes: u64,
    previous_seq: u64,
    expected_anchor_seq: Option<u64>,
}

impl PageStreamReader {
    fn open(
        directory: &Path,
        conversation_id: ConversationId,
        stream: ConversationEventStream,
        offsets: &StreamSparseOffsets,
        anchor: SparseEventOffset,
    ) -> Result<Self> {
        if anchor.byte_offset >= offsets.validated_bytes {
            return Err(error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &directory.join(stream.file_name()),
                Some(anchor.seq),
                Some(anchor.byte_offset),
                format!(
                    "sparse offset {} is outside validated stream bytes {}",
                    anchor.byte_offset, offsets.validated_bytes
                ),
            ));
        }
        let path = directory.join(stream.file_name());
        let mut file = File::open(&path).map_err(|source| {
            error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::Io,
                conversation_id,
                &path,
                Some(anchor.seq),
                Some(anchor.byte_offset),
                format!("paged stream cannot be opened: {source}"),
            )
        })?;
        file.seek(SeekFrom::Start(anchor.byte_offset))
            .map_err(|source| {
                error_at(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::Io,
                    conversation_id,
                    &path,
                    Some(anchor.seq),
                    Some(anchor.byte_offset),
                    format!(
                        "paged stream seek failed at byte offset {}: {source}",
                        anchor.byte_offset
                    ),
                )
            })?;
        Ok(Self {
            stream,
            path,
            reader: BufReader::new(file),
            offset: anchor.byte_offset,
            validated_bytes: offsets.validated_bytes,
            previous_seq: 0,
            expected_anchor_seq: Some(anchor.seq),
        })
    }

    fn next_after(
        &mut self,
        conversation_id: ConversationId,
        after_seq: u64,
    ) -> Result<Option<ConversationEventRecordV2>> {
        while let Some(record) = self.next_record(conversation_id)? {
            if record.seq > after_seq {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    fn next_record(
        &mut self,
        conversation_id: ConversationId,
    ) -> Result<Option<ConversationEventRecordV2>> {
        if self.offset == self.validated_bytes {
            return Ok(None);
        }
        if self.offset > self.validated_bytes {
            return Err(error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &self.path,
                None,
                Some(self.offset),
                format!(
                    "page cursor offset {} exceeds validated bytes {}",
                    self.offset, self.validated_bytes
                ),
            ));
        }
        let line_offset = self.offset;
        let line = match read_bounded_jsonl_line(&mut self.reader) {
            Ok(BoundedJsonLine::Complete(line)) => line,
            Ok(BoundedJsonLine::Eof | BoundedJsonLine::Torn(_)) => {
                return Err(error_at(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::CorruptRecord,
                    conversation_id,
                    &self.path,
                    None,
                    Some(line_offset),
                    format!("validated stream ended unexpectedly at byte offset {line_offset}"),
                ));
            }
            Err(BoundedJsonLineError::Io(source)) => {
                return Err(error_at(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::Io,
                    conversation_id,
                    &self.path,
                    None,
                    Some(line_offset),
                    format!("paged stream read failed at byte offset {line_offset}: {source}"),
                ));
            }
            Err(BoundedJsonLineError::TooLarge) => {
                return Err(error_at(
                    ConversationErrorCode::ConversationRecordTooLarge,
                    EventLogErrorKind::RecordTooLarge,
                    conversation_id,
                    &self.path,
                    None,
                    Some(line_offset),
                    format!("encoded JSONL record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"),
                ));
            }
        };
        let bytes_read = line.len();
        let next_offset = self.offset.checked_add(bytes_read as u64).ok_or_else(|| {
            error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &self.path,
                None,
                Some(line_offset),
                "page cursor byte offset overflow".to_string(),
            )
        })?;
        if next_offset > self.validated_bytes {
            return Err(error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &self.path,
                None,
                Some(line_offset),
                format!(
                    "record at byte offset {line_offset} crosses validated stream boundary {}",
                    self.validated_bytes
                ),
            ));
        }
        let record = decode_record(
            &line[..line.len() - 1],
            conversation_id,
            self.stream,
            &self.path,
            line_offset,
            self.previous_seq,
        )?;
        if let Some(expected_seq) = self.expected_anchor_seq.take() {
            if record.seq != expected_seq {
                return Err(error_at(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::CorruptRecord,
                    conversation_id,
                    &self.path,
                    Some(record.seq),
                    Some(line_offset),
                    format!(
                        "sparse offset at byte {line_offset} expected seq {expected_seq}, found {}",
                        record.seq
                    ),
                ));
            }
        }
        self.offset = next_offset;
        self.previous_seq = record.seq;
        Ok(Some(record))
    }
}

/// Read one globally ordered page by seeking each stream to its nearest validated sparse offset.
pub fn read_event_page(
    directory: &Path,
    conversation_id: ConversationId,
    scan: &EventLogScan,
    after_seq: u64,
    limit: usize,
) -> Result<Vec<ConversationEventRecordV2>> {
    if !(MIN_EVENT_PAGE_LIMIT..=MAX_EVENT_PAGE_LIMIT).contains(&limit) {
        return Err(error(
            ConversationErrorCode::ConversationInvalidId,
            EventLogErrorKind::InvalidPageLimit,
            conversation_id,
            directory,
            format!(
                "event page limit must be between {MIN_EVENT_PAGE_LIMIT} and {MAX_EVENT_PAGE_LIMIT}"
            ),
        ));
    }
    if after_seq > scan.last_seq() {
        return Err(error(
            ConversationErrorCode::ConversationRecoveryRequired,
            EventLogErrorKind::SequenceConflict,
            conversation_id,
            directory,
            format!(
                "page cursor {after_seq} is ahead of validated lastSeq {}",
                scan.last_seq()
            ),
        ));
    }
    if after_seq == scan.last_seq() {
        return Ok(Vec::new());
    }

    let mut readers = Vec::new();
    let mut pending = Vec::new();
    for stream in event_streams() {
        let offsets = scan.sparse_offsets.stream(stream);
        if offsets.event_count == 0 {
            continue;
        }
        let Some(anchor) = offsets.entries.anchor_at_or_before(after_seq) else {
            return Err(error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                conversation_id,
                &directory.join(stream.file_name()),
                "validated non-empty stream has no sparse offset".to_string(),
            ));
        };
        let mut reader =
            PageStreamReader::open(directory, conversation_id, stream, offsets, anchor)?;
        let next = reader.next_after(conversation_id, after_seq)?;
        readers.push(reader);
        pending.push(next);
    }

    let mut records = Vec::with_capacity(limit);
    let mut encoded_bytes = 0usize;
    let mut previous_seq = after_seq;
    while records.len() < limit {
        let Some(next_index) = pending
            .iter()
            .enumerate()
            .filter_map(|(index, record)| record.as_ref().map(|record| (index, record.seq)))
            .min_by_key(|(_, seq)| *seq)
            .map(|(index, _)| index)
        else {
            break;
        };
        let record = pending[next_index]
            .take()
            .expect("selected paged event exists");
        if record.seq <= previous_seq {
            return Err(error_at(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::SequenceConflict,
                conversation_id,
                &directory.join(record.type_.stream().file_name()),
                Some(record.seq),
                None,
                format!("global paged sequence conflict at seq {}", record.seq),
            ));
        }
        let record_bytes = encoded_json_len_bounded(&record, MAX_CONVERSATION_RECORD_BYTES)
            .ok_or_else(|| {
                error_at(
                    ConversationErrorCode::ConversationRecordTooLarge,
                    EventLogErrorKind::RecordTooLarge,
                    conversation_id,
                    &directory.join(record.type_.stream().file_name()),
                    Some(record.seq),
                    None,
                    format!(
                        "encoded canonical record exceeds {MAX_CONVERSATION_RECORD_BYTES} bytes"
                    ),
                )
            })?
            .saturating_add(1);
        if encoded_bytes.saturating_add(record_bytes) > MAX_CONVERSATION_HISTORY_PAGE_BYTES {
            if records.is_empty() {
                return Err(error_at(
                    ConversationErrorCode::ConversationPageTooLarge,
                    EventLogErrorKind::PageTooLarge,
                    conversation_id,
                    directory,
                    Some(record.seq),
                    None,
                    format!(
                        "encoded canonical page cannot advance within {MAX_CONVERSATION_HISTORY_PAGE_BYTES} bytes"
                    ),
                ));
            }
            break;
        }
        encoded_bytes = encoded_bytes.saturating_add(record_bytes);
        previous_seq = record.seq;
        records.push(record);
        pending[next_index] = readers[next_index].next_record(conversation_id)?;
    }
    Ok(records)
}

fn apply_binding_event(
    materialized: &mut BindingMaterialization,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    match record.type_ {
        ConversationEventType::BindingBound => {
            let payload = binding_payload(record, record.conversation_id, path)?;
            validate_binding(
                &payload.binding,
                AgentSessionBindingState::Active,
                record,
                path,
            )?;
            if materialized.current.is_some()
                || materialized
                    .history
                    .iter()
                    .any(|entry| entry.binding_id == payload.binding.binding_id)
            {
                return Err(history_error(
                    record.conversation_id,
                    path,
                    record.seq,
                    "binding_bound requires no current binding and a new bindingId",
                ));
            }
            materialized.history.push(payload.binding.clone());
            materialized.current = Some(payload.binding);
        }
        ConversationEventType::BindingDetached => {
            apply_same_binding_transition(
                materialized,
                record,
                path,
                AgentSessionBindingState::Active,
                AgentSessionBindingState::Detached,
                "binding_detached requires the current active opaque binding",
            )?;
        }
        ConversationEventType::BindingRebound => {
            apply_same_binding_transition(
                materialized,
                record,
                path,
                AgentSessionBindingState::Detached,
                AgentSessionBindingState::Active,
                "binding_rebound requires the same detached opaque binding",
            )?;
        }
        ConversationEventType::BindingSuspended => {
            apply_same_binding_transition(
                materialized,
                record,
                path,
                AgentSessionBindingState::Active,
                AgentSessionBindingState::Suspended,
                "binding_suspended requires the current active opaque binding",
            )?;
        }
        ConversationEventType::BindingReplaced => {
            let payload: BindingReplacementPayloadV1 =
                serde_json::from_value(record.payload.clone()).map_err(|source| {
                    history_error(
                        record.conversation_id,
                        path,
                        record.seq,
                        &format!("invalid binding_replaced payload: {source}"),
                    )
                })?;
            validate_binding(
                &payload.previous_binding,
                AgentSessionBindingState::Replaced,
                record,
                path,
            )?;
            validate_binding(
                &payload.binding,
                AgentSessionBindingState::Active,
                record,
                path,
            )?;
            let existing = materialized.current.as_ref().ok_or_else(|| {
                history_error(
                    record.conversation_id,
                    path,
                    record.seq,
                    "binding transition requires a current binding",
                )
            })?;
            if !same_opaque_binding(existing, &payload.previous_binding)
                || payload.binding.binding_id == payload.previous_binding.binding_id
                || materialized
                    .history
                    .iter()
                    .any(|entry| entry.binding_id == payload.binding.binding_id)
            {
                return Err(history_error(
                    record.conversation_id,
                    path,
                    record.seq,
                    "binding_replaced must retain the current old binding and add a new bindingId",
                ));
            }
            let previous_index = materialized
                .history
                .iter()
                .position(|entry| entry.binding_id == payload.previous_binding.binding_id)
                .ok_or_else(|| {
                    history_error(
                        record.conversation_id,
                        path,
                        record.seq,
                        "current binding is missing from binding history",
                    )
                })?;
            debug_assert!(materialized
                .history
                .replace(previous_index, payload.previous_binding));
            materialized.history.push(payload.binding.clone());
            materialized.current = Some(payload.binding);
        }
        _ => {}
    }
    Ok(())
}

fn apply_same_binding_transition(
    materialized: &mut BindingMaterialization,
    record: &ConversationEventRecordV2,
    path: &Path,
    previous_state: AgentSessionBindingState,
    next_state: AgentSessionBindingState,
    failure_detail: &str,
) -> Result<()> {
    let payload = binding_payload(record, record.conversation_id, path)?;
    validate_binding(&payload.binding, next_state, record, path)?;
    let existing = materialized.current.as_ref().ok_or_else(|| {
        history_error(
            record.conversation_id,
            path,
            record.seq,
            "binding transition requires a current binding",
        )
    })?;
    if existing.state != previous_state || !same_opaque_binding(existing, &payload.binding) {
        return Err(history_error(
            record.conversation_id,
            path,
            record.seq,
            failure_detail,
        ));
    }
    let history_index = materialized
        .history
        .iter()
        .position(|entry| entry.binding_id == payload.binding.binding_id)
        .ok_or_else(|| {
            history_error(
                record.conversation_id,
                path,
                record.seq,
                "current binding is missing from binding history",
            )
        })?;
    debug_assert!(materialized
        .history
        .replace(history_index, payload.binding.clone()));
    materialized.current = Some(payload.binding);
    Ok(())
}

fn apply_attachment_event(
    materialized: &mut AttachmentMaterialization,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    if !matches!(
        record.type_,
        ConversationEventType::ProjectAttached | ConversationEventType::ProjectDetached
    ) {
        return Ok(());
    }
    let payload: ProjectAttachmentEventPayloadV1 = serde_json::from_value(record.payload.clone())
        .map_err(|source| {
        attachment_history_error(
            record.conversation_id,
            path,
            record.seq,
            &format!("invalid attachment payload: {source}"),
        )
    })?;
    if payload.attachment.schema_version != PROJECT_ATTACHMENT_SCHEMA_VERSION {
        return Err(attachment_history_error(
            record.conversation_id,
            path,
            record.seq,
            "unsupported project attachment schemaVersion",
        ));
    }
    match record.type_ {
        ConversationEventType::ProjectAttached => {
            if materialized.current.is_some() {
                return Err(attachment_history_error(
                    record.conversation_id,
                    path,
                    record.seq,
                    "project_attached requires no materialized attachment",
                ));
            }
            materialized.history.push(payload.attachment.clone());
            materialized.current = Some(payload.attachment);
        }
        ConversationEventType::ProjectDetached => {
            if let Some(current) = &materialized.current {
                if current.project_id != payload.attachment.project_id {
                    return Err(attachment_history_error(
                        record.conversation_id,
                        path,
                        record.seq,
                        "project_detached does not match the current attachment",
                    ));
                }
            }
            if !materialized
                .history
                .iter()
                .any(|entry| entry == &payload.attachment)
            {
                materialized.history.push(payload.attachment);
            }
            materialized.current = None;
        }
        _ => unreachable!("attachment variants were filtered above"),
    }
    materialized.has_events = true;
    Ok(())
}

fn apply_execution_target_event(
    materialized: &mut Option<ExecutionTarget>,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    if record.type_ != ConversationEventType::ExecutionTargetUpdated {
        return Ok(());
    }
    let payload: ExecutionTargetEventPayloadV1 = serde_json::from_value(record.payload.clone())
        .map_err(|source| {
            attachment_history_error(
                record.conversation_id,
                path,
                record.seq,
                &format!("invalid execution_target_updated payload: {source}"),
            )
        })?;
    validate_execution_target_snapshot(&payload.execution_target, record, path)?;
    *materialized = Some(payload.execution_target);
    Ok(())
}

fn validate_execution_target_snapshot(
    target: &ExecutionTarget,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    let valid = match target {
        ExecutionTarget::Workspace => true,
        ExecutionTarget::ProjectRoot {
            project_id,
            project_root,
        } => !project_id.trim().is_empty() && !project_root.trim().is_empty(),
        ExecutionTarget::Worktree {
            project_id,
            worktree_path,
            worktree_branch,
        } => {
            !project_id.trim().is_empty()
                && !worktree_path.trim().is_empty()
                && !worktree_branch.trim().is_empty()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(attachment_history_error(
            record.conversation_id,
            path,
            record.seq,
            "execution target snapshot has empty required fields",
        ))
    }
}

fn apply_durable_replacements(
    frontier: &mut ConversationFrontier,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    match record.type_ {
        ConversationEventType::UsageUpdate => {
            crate::conversation::usage_plan::validate_usage_update(&record.payload).map_err(
                |_| {
                    error(
                        ConversationErrorCode::ConversationRecoveryRequired,
                        EventLogErrorKind::CorruptRecord,
                        record.conversation_id,
                        path,
                        format!("usage_update failed typed schema at seq {}", record.seq),
                    )
                },
            )?;
            frontier.latest_usage = Some(Arc::new(record.payload.clone()));
        }
        ConversationEventType::PlanUpdate => {
            crate::conversation::usage_plan::validate_plan_update(&record.payload).map_err(
                |_| {
                    error(
                        ConversationErrorCode::ConversationRecoveryRequired,
                        EventLogErrorKind::CorruptRecord,
                        record.conversation_id,
                        path,
                        format!("plan_update failed typed schema at seq {}", record.seq),
                    )
                },
            )?;
            frontier.latest_plan = Some(Arc::new(record.payload.clone()));
        }
        ConversationEventType::RelayCursorAdvanced if !matches!(&record.payload, Value::Object(object) if object.is_empty()) =>
        {
            return Err(error(
                ConversationErrorCode::ConversationRecoveryRequired,
                EventLogErrorKind::CorruptRecord,
                record.conversation_id,
                path,
                format!(
                    "relay_cursor_advanced must have an empty payload at seq {}",
                    record.seq
                ),
            ));
        }
        ConversationEventType::RelayCursorAdvanced => {}
        _ => {}
    }
    Ok(())
}

fn apply_summary_event(
    summary: &mut ConversationSummaryFrontier,
    record: &ConversationEventRecordV2,
) -> Result<()> {
    let path = Path::new(record.type_.stream().file_name());
    match record.type_.stream() {
        ConversationEventStream::Messages
            if !matches!(
                record.type_,
                ConversationEventType::UsageUpdate
                    | ConversationEventType::PlanUpdate
                    | ConversationEventType::RelayCursorAdvanced
            ) =>
        {
            summary.message_count = summary.message_count.checked_add(1).ok_or_else(|| {
                error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::CorruptRecord,
                    record.conversation_id,
                    path,
                    format!("message count overflow at seq {}", record.seq),
                )
            })?;
        }
        ConversationEventStream::Messages => {}
        ConversationEventStream::ToolCalls => {
            summary.tool_count = summary.tool_count.checked_add(1).ok_or_else(|| {
                error(
                    ConversationErrorCode::ConversationRecoveryRequired,
                    EventLogErrorKind::CorruptRecord,
                    record.conversation_id,
                    path,
                    format!("tool count overflow at seq {}", record.seq),
                )
            })?;
        }
        ConversationEventStream::Bindings | ConversationEventStream::Attachments => {}
    }
    if record.type_ != ConversationEventType::RelayCursorAdvanced {
        summary.last_activity_at_utc = Some(
            summary
                .last_activity_at_utc
                .map_or(record.recorded_at_utc, |current| {
                    current.max(record.recorded_at_utc)
                }),
        );
    }

    match record.type_ {
        ConversationEventType::UserPrompt if summary.title.is_none() => {
            summary.title = Some(derive_title(&record.payload));
            summary.title_source = Some(ConversationTitleSource::DerivedFirstMessage);
        }
        ConversationEventType::SessionInfoUpdate
            if title_precedence(summary.title_source)
                < title_precedence(Some(ConversationTitleSource::BackgroundGenerated)) =>
        {
            summary.title = record
                .payload
                .get("title")
                .and_then(Value::as_str)
                .map(normalize_title);
            summary.title_source = Some(ConversationTitleSource::AgentSupplied);
        }
        ConversationEventType::LocalTitleGenerated => {
            let source = if record.payload.get("titleSource").and_then(Value::as_str)
                == Some("local_alias")
            {
                ConversationTitleSource::LocalAlias
            } else {
                ConversationTitleSource::BackgroundGenerated
            };
            if title_precedence(summary.title_source) <= title_precedence(Some(source)) {
                if let Some(title) = record
                    .payload
                    .get("title")
                    .and_then(Value::as_str)
                    .map(normalize_title)
                {
                    summary.title = Some(title);
                    summary.title_source = Some(source);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn title_precedence(source: Option<ConversationTitleSource>) -> u8 {
    match source {
        None => 0,
        Some(ConversationTitleSource::DerivedFirstMessage) => 1,
        Some(ConversationTitleSource::AgentSupplied) => 2,
        Some(ConversationTitleSource::BackgroundGenerated) => 3,
        Some(ConversationTitleSource::LocalAlias) => 4,
    }
}

fn derive_title(payload: &Value) -> String {
    let text = payload
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| block.get("text").and_then(Value::as_str))
                .flatten()
        })
        .unwrap_or("Untitled Chat");
    normalize_title(text)
}

fn normalize_title(text: &str) -> String {
    fn strip_wrappers(mut value: &str) -> &str {
        loop {
            let next = value
                .trim()
                .trim_matches(['"', '\'', '`'])
                .trim_matches('_')
                .trim_matches('*')
                .trim();
            if next == value {
                return next;
            }
            value = next;
        }
    }

    let mut lines = text
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let mut sanitized = strip_wrappers(lines.next().unwrap_or_default());
    let lowercase = sanitized.to_ascii_lowercase();
    const PREAMBLES: &[&str] = &[
        "sure! here's the title:",
        "sure, here's the title:",
        "here's the title:",
        "the title is:",
        "title:",
    ];
    if let Some(prefix) = PREAMBLES
        .iter()
        .find(|prefix| lowercase.starts_with(**prefix))
    {
        sanitized = strip_wrappers(&sanitized[prefix.len()..]);
        if sanitized.is_empty() {
            sanitized = strip_wrappers(lines.next().unwrap_or_default());
        }
    } else if lowercase == "what should we do?" {
        sanitized = strip_wrappers(lines.next().unwrap_or_default());
    }
    if sanitized.is_empty() {
        return "Untitled Chat".to_string();
    }
    let bounded = sanitized.chars().take(48).collect::<String>();
    if sanitized.chars().count() > 48 {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn binding_payload(
    record: &ConversationEventRecordV2,
    conversation_id: ConversationId,
    path: &Path,
) -> Result<BindingEventPayloadV1> {
    serde_json::from_value(record.payload.clone()).map_err(|source| {
        history_error(
            conversation_id,
            path,
            record.seq,
            &format!("invalid binding payload: {source}"),
        )
    })
}

fn validate_binding(
    binding: &AgentSessionBinding,
    expected_state: AgentSessionBindingState,
    record: &ConversationEventRecordV2,
    path: &Path,
) -> Result<()> {
    if binding.schema_version != AGENT_SESSION_BINDING_SCHEMA_VERSION
        || binding.state != expected_state
        || binding.agent_session_id.is_empty()
        || binding.runtime_agent_id.is_empty()
        || binding.stable_agent_namespace.is_empty()
        || binding.execution_cwd.is_empty()
    {
        return Err(history_error(
            record.conversation_id,
            path,
            record.seq,
            "binding snapshot has an invalid schema, state, or required opaque field",
        ));
    }
    Ok(())
}

fn same_opaque_binding(left: &AgentSessionBinding, right: &AgentSessionBinding) -> bool {
    left.binding_id == right.binding_id
        && left.agent_session_id == right.agent_session_id
        && left.runtime_agent_id == right.runtime_agent_id
        && left.stable_agent_namespace == right.stable_agent_namespace
        && left.execution_cwd == right.execution_cwd
        && left.bound_at_utc == right.bound_at_utc
}

fn history_error(
    conversation_id: ConversationId,
    path: &Path,
    seq: u64,
    detail: &str,
) -> EventLogError {
    error(
        ConversationErrorCode::ConversationRecoveryRequired,
        EventLogErrorKind::InvalidBindingHistory,
        conversation_id,
        path,
        format!("invalid binding history at seq {seq}: {detail}"),
    )
}

fn attachment_history_error(
    conversation_id: ConversationId,
    path: &Path,
    seq: u64,
    detail: &str,
) -> EventLogError {
    error(
        ConversationErrorCode::ConversationRecoveryRequired,
        EventLogErrorKind::InvalidAttachmentHistory,
        conversation_id,
        path,
        format!("invalid attachment history at seq {seq}: {detail}"),
    )
}

fn stable_error_code(code: ConversationErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
}

fn error_at(
    code: ConversationErrorCode,
    kind: EventLogErrorKind,
    conversation_id: ConversationId,
    path: &Path,
    seq: Option<u64>,
    byte_offset: Option<u64>,
    detail: String,
) -> EventLogError {
    EventLogError {
        code,
        kind,
        conversation_id,
        path: path.to_path_buf(),
        seq,
        byte_offset,
        detail,
    }
}

fn error(
    code: ConversationErrorCode,
    kind: EventLogErrorKind,
    conversation_id: ConversationId,
    path: &Path,
    detail: String,
) -> EventLogError {
    error_at(code, kind, conversation_id, path, None, None, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::parse_created_at_utc;
    use crate::conversation::durable_fs::DirectoryPermissions;
    use serde_json::json;
    use std::io::Write;
    use tempfile::TempDir;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    fn fixture() -> (TempDir, PathBuf, ConversationId, DurableFileSystem) {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap().join(ID);
        let durable_fs = DurableFileSystem::new();
        durable_fs
            .create_dir_durable(&directory, DirectoryPermissions::PrivateOwnerOnly)
            .unwrap();
        for file in EVENT_LOG_FILES {
            durable_fs
                .replace_bytes(&directory.join(file), b"")
                .unwrap();
        }
        (
            temp,
            directory,
            ConversationId::parse(ID).unwrap(),
            durable_fs,
        )
    }

    fn record(seq: u64, type_: ConversationEventType) -> ConversationEventRecordV2 {
        ConversationEventRecordV2::new(
            ConversationId::parse(ID).unwrap(),
            seq,
            parse_created_at_utc("2026-08-15T09:45:15.000Z").unwrap(),
            type_,
            json!({}),
        )
    }

    fn append(
        durable_fs: &DurableFileSystem,
        directory: &Path,
        record: &ConversationEventRecordV2,
    ) {
        durable_fs
            .append_jsonl(
                &directory.join(record.type_.stream().file_name()),
                &serde_json::to_vec(record).unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn incremental_frontier_preserves_distinct_events_title_precedence_and_constant_work() {
        reset_operation_counters();
        let conversation_id = ConversationId::parse(ID).unwrap();
        let recorded_at_utc = parse_created_at_utc("2026-08-15T09:45:15.000Z").unwrap();
        let mut frontier = ConversationFrontier::default();
        let fixtures = [
            (
                ConversationEventType::UserPrompt,
                json!({"content":[{"type":"text","text":"Derived title"}]}),
            ),
            (
                ConversationEventType::SessionInfoUpdate,
                json!({"title":"Agent title"}),
            ),
            (
                ConversationEventType::LocalTitleGenerated,
                json!({"title":"Background title"}),
            ),
            (
                ConversationEventType::SessionInfoUpdate,
                json!({"title":"Must not overwrite"}),
            ),
            (
                ConversationEventType::ToolCall,
                json!({"toolCall":{"id":"one"}}),
            ),
            (
                ConversationEventType::ToolCallUpdate,
                json!({"update":{"id":"one","status":"completed"}}),
            ),
            (
                ConversationEventType::LocalTitleGenerated,
                json!({"title":"Local alias","titleSource":"local_alias"}),
            ),
            (
                ConversationEventType::LocalTitleGenerated,
                json!({"title":"Must not replace alias"}),
            ),
            (
                ConversationEventType::SessionInfoUpdate,
                json!({"title":42}),
            ),
        ];
        for seq in 1..=10_000_u64 {
            let (type_, payload) = fixtures
                .get((seq - 1) as usize)
                .cloned()
                .unwrap_or((ConversationEventType::MessageChunk, json!({"role":"agent"})));
            apply_event(
                &mut frontier,
                &ConversationEventRecordV2::new(
                    conversation_id,
                    seq,
                    recorded_at_utc,
                    type_,
                    payload,
                ),
            )
            .unwrap();
        }
        assert_eq!(frontier.last_seq, 10_000);
        assert_eq!(frontier.summary.title.as_deref(), Some("Local alias"));
        assert_eq!(
            frontier.summary.title_source,
            Some(ConversationTitleSource::LocalAlias)
        );
        assert_eq!(frontier.summary.tool_count, 2);
        assert_eq!(frontier.summary.message_count, 9_998);
        assert_eq!(operation_counters(), (10_000, 0));
    }

    #[test]
    fn chunked_index_append_is_bounded() {
        let mut scan = EventLogScan::default();
        for seq in 1..=1_000_000_u64 {
            let byte_offset = scan.sparse_offsets.messages.validated_bytes;
            scan.record_appended(ConversationEventStream::Messages, seq, byte_offset, 1);
        }
        scan.frontier.last_seq = 1_000_000;
        assert_eq!(
            scan.sparse_offsets.messages.entries.len(),
            1_000_000_usize.div_ceil(SPARSE_OFFSET_STRIDE as usize)
        );

        let original_generation = Arc::new(scan);
        let mut writer_generation = Arc::clone(&original_generation);
        let original_entries = original_generation.sparse_offsets.messages.entries.len();
        let mut allocated_samples = Vec::with_capacity(10_000);
        let mut max_copied_entries = 0usize;

        for append_index in 1..=10_000_u64 {
            // This is the expensive overlap from PERF-RR-006: the reader holds the exact current
            // generation while the writer publishes the next one through Arc::make_mut.
            let overlapping_reader = Arc::clone(&writer_generation);
            reset_sparse_index_clone_metrics();
            let stream_bytes = writer_generation.sparse_offsets.messages.validated_bytes;
            let next_seq = 1_000_000 + append_index;
            let next_generation = Arc::make_mut(&mut writer_generation);
            next_generation.record_appended(
                ConversationEventStream::Messages,
                next_seq,
                stream_bytes,
                1,
            );
            next_generation.frontier.last_seq = next_seq;
            let (copied_entries, allocated_bytes) = sparse_index_clone_metrics();
            max_copied_entries = max_copied_entries.max(copied_entries);
            allocated_samples.push(allocated_bytes);
            assert_eq!(overlapping_reader.last_seq(), next_seq - 1);
        }

        allocated_samples.sort_unstable();
        let p99_index = (allocated_samples.len() * 99 / 100).min(allocated_samples.len() - 1);
        let p99_allocated_bytes = allocated_samples[p99_index];
        assert!(
            max_copied_entries <= SPARSE_INDEX_CHUNK_ENTRIES,
            "overlapping append copied {max_copied_entries} sparse entries"
        );
        assert!(
            p99_allocated_bytes <= 256 * 1024,
            "overlapping append allocated {p99_allocated_bytes} bytes at p99"
        );
        assert_eq!(
            original_generation.sparse_offsets.messages.entries.len(),
            original_entries,
            "the held reader generation must remain immutable"
        );
    }

    #[test]
    fn chunked_frontier_histories_bound_total_overlap_allocation() {
        let mut scan = EventLogScan::default();
        let recorded_at_utc = parse_created_at_utc("2026-08-15T09:45:15.000Z").unwrap();
        for index in 0..10_000_u64 {
            scan.frontier.binding.history.push(AgentSessionBinding {
                schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                binding_id: Uuid::from_u128(u128::from(index) + 1),
                agent_session_id: format!("session-{index}"),
                runtime_agent_id: "runtime".to_string(),
                stable_agent_namespace: "config:test".to_string(),
                execution_cwd: "/workspace".to_string(),
                bound_at_utc: recorded_at_utc,
                state: AgentSessionBindingState::Replaced,
            });
            scan.frontier.attachment.history.push(ProjectAttachment {
                schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
                project_id: format!("project-{index}"),
                attached_at_utc: recorded_at_utc,
                project_path_snapshot: "/workspace".to_string(),
                worktree_path: None,
                worktree_branch: None,
            });
            let byte_offset = scan.sparse_offsets.messages.validated_bytes;
            scan.record_appended(ConversationEventStream::Messages, index + 1, byte_offset, 1);
        }
        scan.frontier.last_seq = 10_000;
        assert_eq!(scan.frontier.binding.history.len(), 10_000);
        assert_eq!(scan.frontier.attachment.history.len(), 10_000);
        assert_eq!(scan.frontier.binding.history.chunks.len(), 9);
        assert_eq!(scan.frontier.attachment.history.chunks.len(), 9);

        let original_generation = Arc::new(scan);
        let mut writer_generation = Arc::clone(&original_generation);
        let mut allocated_samples = Vec::with_capacity(10_000);
        let mut maximum_copied_history_entries = 0usize;
        for append_index in 1..=10_000_u64 {
            let overlapping_reader = Arc::clone(&writer_generation);
            reset_sparse_index_clone_metrics();
            let next_seq = 10_000 + append_index;
            let next_generation = Arc::make_mut(&mut writer_generation);
            let byte_offset = next_generation.sparse_offsets.messages.validated_bytes;
            next_generation.record_appended(
                ConversationEventStream::Messages,
                next_seq,
                byte_offset,
                1,
            );
            next_generation
                .frontier
                .binding
                .history
                .push(AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::from_u128(u128::from(next_seq) + 1),
                    agent_session_id: format!("session-{next_seq}"),
                    runtime_agent_id: "runtime".to_string(),
                    stable_agent_namespace: "config:test".to_string(),
                    execution_cwd: "/workspace".to_string(),
                    bound_at_utc: recorded_at_utc,
                    state: AgentSessionBindingState::Active,
                });
            next_generation
                .frontier
                .attachment
                .history
                .push(ProjectAttachment {
                    schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
                    project_id: format!("project-{next_seq}"),
                    attached_at_utc: recorded_at_utc,
                    project_path_snapshot: "/workspace".to_string(),
                    worktree_path: None,
                    worktree_branch: None,
                });
            next_generation.frontier.last_seq = next_seq;
            let (copied_history_entries, total_allocated_bytes) = total_overlap_clone_metrics();
            maximum_copied_history_entries =
                maximum_copied_history_entries.max(copied_history_entries);
            allocated_samples.push(total_allocated_bytes);
            assert_eq!(overlapping_reader.last_seq(), next_seq - 1);
        }
        let p99_allocated_bytes = {
            allocated_samples.sort_unstable();
            allocated_samples[allocated_samples.len() * 99 / 100]
        };
        assert!(
            maximum_copied_history_entries <= 2 * FRONTIER_HISTORY_CHUNK_ENTRIES,
            "copied {maximum_copied_history_entries} history entries"
        );
        assert!(
            p99_allocated_bytes <= 262_144,
            "total overlap allocation p99 was {p99_allocated_bytes} bytes"
        );
        assert_eq!(original_generation.frontier.binding.history.len(), 10_000);
        assert_eq!(
            original_generation.frontier.attachment.history.len(),
            10_000
        );
        println!(
            "frontier_overlap history_entries=10000 p99_allocated_bytes={p99_allocated_bytes} max_copied_entries={maximum_copied_history_entries} chunk_entries=1024"
        );
    }

    #[test]
    fn canonical_record_and_page_bytes_are_bounded_before_materialization() {
        let (_temp, directory, id, durable_fs) = fixture();
        let mut oversized = record(1, ConversationEventType::MessageChunk);
        oversized.payload = json!({"blob":"x".repeat(MAX_CONVERSATION_RECORD_BYTES)});
        assert_eq!(
            encode_event_record_bounded(&oversized),
            Err(EventRecordEncodingError::TooLarge)
        );

        for seq in 1..=30_u64 {
            let mut event = record(seq, ConversationEventType::MessageChunk);
            event.payload = json!({"blob":"x".repeat(200_000)});
            append(&durable_fs, &directory, &event);
        }
        let scan = scan_event_log(&directory, id, &durable_fs).unwrap();
        let first = read_event_page(&directory, id, &scan, 0, MAX_EVENT_PAGE_LIMIT).unwrap();
        assert!(!first.is_empty());
        assert!(first.len() < 30);
        assert!(
            encoded_json_len_bounded(&first, MAX_CONVERSATION_HISTORY_PAGE_BYTES).is_some(),
            "encoded page exceeded the 4 MiB limit"
        );
        let cursor = first.last().unwrap().seq;
        let second = read_event_page(&directory, id, &scan, cursor, MAX_EVENT_PAGE_LIMIT).unwrap();
        assert_eq!(first.len() + second.len(), 30);

        let path = directory.join(MESSAGES_FILE);
        durable_fs
            .replace_bytes(&path, &vec![b'x'; MAX_CONVERSATION_RECORD_BYTES + 1])
            .unwrap();
        let error = scan_event_log(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(error.stable_code(), "CONVERSATION_RECORD_TOO_LARGE");
        assert_eq!(error.kind, EventLogErrorKind::RecordTooLarge);
    }

    #[test]
    fn relay_cursor_marker_is_payload_free_and_non_materialized() {
        let id = ConversationId::parse(ID).unwrap();
        let recorded_at_utc = parse_created_at_utc("2026-08-15T09:45:15.000Z").unwrap();
        let mut frontier = ConversationFrontier::default();
        apply_event(
            &mut frontier,
            &ConversationEventRecordV2::new(
                id,
                1,
                recorded_at_utc,
                ConversationEventType::RelayCursorAdvanced,
                json!({}),
            ),
        )
        .unwrap();
        assert_eq!(frontier.last_seq, 1);
        assert_eq!(frontier.summary.message_count, 0);
        assert_eq!(frontier.summary.tool_count, 0);
        assert!(frontier.latest_usage.is_none());
        assert!(frontier.latest_plan.is_none());

        let error = apply_event(
            &mut frontier,
            &ConversationEventRecordV2::new(
                id,
                2,
                recorded_at_utc,
                ConversationEventType::RelayCursorAdvanced,
                json!({"payload":"forbidden"}),
            ),
        )
        .unwrap_err();
        assert_eq!(error.kind, EventLogErrorKind::CorruptRecord);
    }

    #[test]
    fn sparse_offset_lookup_is_logarithmic() {
        let mut entries = SparseOffsetEntries::default();
        for index in 0..100_000_u64 {
            entries.push(SparseEventOffset {
                seq: index.saturating_mul(2).saturating_add(1),
                byte_offset: index.saturating_mul(64),
            });
        }
        assert_eq!(entries.len(), 100_000);

        let probes = [1_u64, 99_999, 199_999];
        let mut maximum_comparisons = 0usize;
        for iteration in 0..10_000_usize {
            let after_seq = probes[iteration % probes.len()];
            reset_sparse_lookup_comparisons();
            let anchor = entries.anchor_at_or_before(after_seq).unwrap();
            let comparisons = sparse_lookup_comparisons();
            maximum_comparisons = maximum_comparisons.max(comparisons);
            assert!(anchor.seq <= after_seq);
            if let Some(next) = entries.get(entries.partition_point(|entry| entry.seq <= after_seq))
            {
                assert!(next.seq > after_seq);
            }
        }
        assert!(
            maximum_comparisons <= 19,
            "100,000-offset partition_point used {maximum_comparisons} comparisons"
        );
    }

    #[test]
    fn replay_merges_all_streams_by_one_global_sequence() {
        let (_temp, directory, id, durable_fs) = fixture();
        for record in [
            record(1, ConversationEventType::UserPrompt),
            record(3, ConversationEventType::ToolCall),
            record(2, ConversationEventType::MessageChunk),
            record(4, ConversationEventType::PromptComplete),
        ] {
            append(&durable_fs, &directory, &record);
        }
        let scan = replay_conversation(&directory, id, &durable_fs).unwrap();
        assert_eq!(
            read_event_page(&directory, id, &scan, 0, 10)
                .unwrap()
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(scan.last_seq(), 4);
        assert_eq!(scan.event_count(), 4);
    }

    #[test]
    fn paged_read_seeks_sparse_offsets_and_matches_global_full_order() {
        let (_temp, directory, id, durable_fs) = fixture();
        let mut expected = Vec::new();
        for seq in 1..=1_030_u64 {
            let type_ = if seq % 2 == 0 {
                ConversationEventType::ToolCall
            } else {
                ConversationEventType::MessageChunk
            };
            let mut event = record(seq, type_);
            event.payload = json!({"marker":seq});
            append(&durable_fs, &directory, &event);
            expected.push(event);
        }
        let scan = scan_event_log(&directory, id, &durable_fs).unwrap();
        assert_eq!(scan.event_count(), 1_030);
        assert_eq!(scan.sparse_offsets.messages.entries.len(), 3);
        assert_eq!(scan.sparse_offsets.tool_calls.entries.len(), 3);

        for limit in [1, 17, 256, 1_000] {
            let mut actual = Vec::new();
            let mut cursor = 0;
            while cursor < scan.last_seq() {
                let page = read_event_page(&directory, id, &scan, cursor, limit).unwrap();
                assert!(!page.is_empty());
                assert!(page.len() <= limit);
                cursor = page.last().unwrap().seq;
                actual.extend(page);
            }
            assert_eq!(actual, expected, "limit={limit}");
        }

        for invalid_limit in [0, MAX_EVENT_PAGE_LIMIT + 1] {
            let error = read_event_page(&directory, id, &scan, 0, invalid_limit).unwrap_err();
            assert_eq!(error.stable_code(), "VALIDATION_ERROR");
            assert_eq!(error.kind, EventLogErrorKind::InvalidPageLimit);
        }

        let mut corrupt_offsets = scan.clone();
        corrupt_offsets
            .sparse_offsets
            .messages
            .entries
            .get_mut_for_test(0)
            .unwrap()
            .byte_offset += 1;
        let error = read_event_page(&directory, id, &corrupt_offsets, 0, 17).unwrap_err();
        assert_eq!(
            error.code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(error.byte_offset, Some(1));
    }

    #[test]
    fn torn_final_record_is_backed_up_and_truncated_to_complete_newline() {
        let (_temp, directory, id, durable_fs) = fixture();
        let complete = record(1, ConversationEventType::MessageChunk);
        append(&durable_fs, &directory, &complete);
        let path = directory.join(MESSAGES_FILE);
        let complete_bytes = fs::read(&path).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(br#"{"schemaVersion":2,"seq":2"#)
            .unwrap();

        let replay = replay_conversation(&directory, id, &durable_fs).unwrap();
        assert_eq!(replay.last_seq(), 1);
        assert_eq!(replay.repairs.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), complete_bytes);
        let backup = directory.join(&replay.repairs[0].backup_file);
        assert!(backup.exists());
        assert!(fs::read(backup).unwrap().len() > complete_bytes.len());
    }

    #[test]
    fn newline_terminated_or_middle_corruption_is_not_rewritten() {
        let (_temp, directory, id, durable_fs) = fixture();
        let path = directory.join(MESSAGES_FILE);
        let bytes = b"{bad}\n{also-bad}\n";
        fs::write(&path, bytes).unwrap();
        let error = replay_conversation(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(
            error.code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains("corrupt-")));
    }

    #[test]
    fn duplicate_global_sequence_wrong_stream_and_future_schema_fail_closed() {
        let (_temp, directory, id, durable_fs) = fixture();
        append(
            &durable_fs,
            &directory,
            &record(1, ConversationEventType::MessageChunk),
        );
        append(
            &durable_fs,
            &directory,
            &record(1, ConversationEventType::ToolCall),
        );
        let duplicate = replay_conversation(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(duplicate.kind, EventLogErrorKind::SequenceConflict);

        for file in EVENT_LOG_FILES {
            durable_fs
                .replace_bytes(&directory.join(file), b"")
                .unwrap();
        }
        let wrong_stream = record(2, ConversationEventType::ToolCall);
        durable_fs
            .append_jsonl(
                &directory.join(MESSAGES_FILE),
                &serde_json::to_vec(&wrong_stream).unwrap(),
            )
            .unwrap();
        assert_eq!(
            replay_conversation(&directory, id, &durable_fs)
                .unwrap_err()
                .kind,
            EventLogErrorKind::StreamMismatch
        );

        for file in EVENT_LOG_FILES {
            durable_fs
                .replace_bytes(&directory.join(file), b"")
                .unwrap();
        }
        let mut future =
            serde_json::to_value(record(1, ConversationEventType::UserPrompt)).unwrap();
        future["schemaVersion"] = json!(99);
        durable_fs
            .append_jsonl(
                &directory.join(MESSAGES_FILE),
                &serde_json::to_vec(&future).unwrap(),
            )
            .unwrap();
        let future = replay_conversation(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(
            future.code,
            ConversationErrorCode::ConversationUnsupportedSchema
        );

        for file in EVENT_LOG_FILES {
            durable_fs
                .replace_bytes(&directory.join(file), b"")
                .unwrap();
        }
        let mut uppercase =
            serde_json::to_value(record(1, ConversationEventType::UserPrompt)).unwrap();
        uppercase["conversationId"] = json!(ID.to_ascii_uppercase());
        durable_fs
            .append_jsonl(
                &directory.join(MESSAGES_FILE),
                &serde_json::to_vec(&uppercase).unwrap(),
            )
            .unwrap();
        assert_eq!(
            replay_conversation(&directory, id, &durable_fs)
                .unwrap_err()
                .kind,
            EventLogErrorKind::ConversationMismatch
        );
    }

    #[test]
    fn malformed_usage_update_replay_moves_conversation_to_recovery_required() {
        let (_temp, directory, id, durable_fs) = fixture();
        append(
            &durable_fs,
            &directory,
            &record(1, ConversationEventType::MessageChunk),
        );
        let mut malformed = record(2, ConversationEventType::UsageUpdate);
        malformed.payload = json!({"notUsage":true});
        append(&durable_fs, &directory, &malformed);
        let error = replay_conversation(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(
            error.code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(error.kind, EventLogErrorKind::CorruptRecord);
        assert_eq!(error.stable_code(), "CONVERSATION_RECOVERY_REQUIRED");

        let mut frontier = ConversationFrontier::default();
        apply_event(
            &mut frontier,
            &record(1, ConversationEventType::MessageChunk),
        )
        .unwrap();
        let rejected = apply_event(&mut frontier, &malformed).unwrap_err();
        assert_eq!(rejected.kind, EventLogErrorKind::CorruptRecord);
        assert!(frontier.latest_usage.is_none());
        assert_eq!(frontier.last_seq, 1);
    }

    #[test]
    fn malformed_plan_update_replay_moves_conversation_to_recovery_required() {
        let (_temp, directory, id, durable_fs) = fixture();
        append(
            &durable_fs,
            &directory,
            &record(1, ConversationEventType::MessageChunk),
        );
        let mut malformed = record(2, ConversationEventType::PlanUpdate);
        malformed.payload = json!({"plan":"not-an-object"});
        append(&durable_fs, &directory, &malformed);
        let error = replay_conversation(&directory, id, &durable_fs).unwrap_err();
        assert_eq!(
            error.code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
        assert_eq!(error.kind, EventLogErrorKind::CorruptRecord);
        assert_eq!(error.stable_code(), "CONVERSATION_RECOVERY_REQUIRED");

        let mut frontier = ConversationFrontier::default();
        apply_event(
            &mut frontier,
            &record(1, ConversationEventType::MessageChunk),
        )
        .unwrap();
        let rejected = apply_event(&mut frontier, &malformed).unwrap_err();
        assert_eq!(rejected.kind, EventLogErrorKind::CorruptRecord);
        assert!(frontier.latest_plan.is_none());
        assert_eq!(frontier.last_seq, 1);
    }
}
