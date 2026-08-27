//! Renderer-shaped session payload materializer for standalone durable history.
//!
//! The standalone `termul-server` persists ACP session events as JSONL records
//! (`SessionPersistence`). `get_session_payload` must reply with the exact
//! `SessionPayload { metadata, messages }` shape the renderer's
//! `loadSessionPayload` consumes (the same shape the desktop
//! `ChatHistoryStore` serves). This module is the PURE fold of durable
//! records into that shape — transport-neutral, no I/O, no clock reads: the
//! output is a deterministic function of the records + metadata so repeated
//! reads produce identical ids, seqs, ordering, and shape (the renderer uses
//! message ids as dedup/merge keys).
//!
//! # Fold semantics (mirror the renderer's live bubbles)
//!
//! - `user_prompt` → a `user` bubble with id `turn:<turnId>` (fallback
//!   `user:seq-<seq>` when the record carries no turn id).
//! - `message_chunk` runs fold into `agent` / `thought` bubbles with id
//!   `snapshot:<role>:<firstSeq>` — the same dialect as the renderer's
//!   `installTransportRecovery`. A run splits on role change, `tool_call`, or
//!   `prompt_complete`; `tool_call_update` NEVER splits (updates preserve the
//!   original card seq). Consecutive text content coalesces into the trailing
//!   text block (`appendBlocks` semantics).
//! - Message `seq` = the run's first record seq; `timestamp` = the run's
//!   first `recorded_at`; `streaming` is always `false` (restored transcripts
//!   never shimmer).
//! - Tool calls are upserted by `toolCallId`; updates preserve the original
//!   timeline `seq`/`timestamp`. The latest valid usage snapshot and plan replace
//!   their prior values. These rules are identical across page boundaries.

use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::acp::session_persistence::{
    PersistedEventRecord, PersistedSessionStatus, SessionMetadata,
};
use crate::conversation::contracts::{
    ConversationHistoryPageV1, ConversationHistoryPageValidationError, ConversationHistoryRecordV1,
};

/// The renderer session-metadata shape (`SessionIndexEntry` in
/// `acp-history-persistence.ts`). camelCase keys; `agentConfigId` is omitted
/// when absent (never `null`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionPayloadMetadata {
    pub id: String,
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_config_id: Option<String>,
    pub title: String,
    pub cwd: String,
    pub project_id: String,
    pub created_at: u64,
    pub last_activity_at: u64,
    pub message_count: u64,
    pub last_seq: u64,
    pub status: PersistedSessionStatus,
    /// Worktree the chat runs in (CAP-4/6). Carried through the materialized
    /// payload so history reopen + post-reload resume preserve the worktree
    /// binding the agent reattaches to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

/// The renderer `ChatMessage` shape. camelCase keys; `seq` always present
/// (standalone history is seq-native — there is no pre-seq legacy).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedChatMessage {
    pub id: String,
    pub role: &'static str,
    pub blocks: Vec<Value>,
    pub streaming: bool,
    pub timestamp: u64,
    pub seq: u64,
}

/// The renderer `SessionPayload` shape served by compatibility reads and incrementally assembled
/// from bounded pages. Empty optional collections are omitted for byte compatibility with older
/// payloads.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedSessionPayload {
    pub metadata: SessionPayloadMetadata,
    pub messages: Vec<MaterializedChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPayloadAccumulatorError {
    Page(ConversationHistoryPageValidationError),
    SessionMismatch,
    CursorRegression,
    RecordSequenceConflict,
}

impl SessionPayloadAccumulatorError {
    #[must_use]
    pub const fn stable_code(&self) -> &'static str {
        "VALIDATION_ERROR"
    }
}

impl fmt::Display for SessionPayloadAccumulatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Page(error) => error.fmt(formatter),
            Self::SessionMismatch => {
                formatter.write_str("history record belongs to another session")
            }
            Self::CursorRegression => formatter.write_str("history cursor did not advance"),
            Self::RecordSequenceConflict => {
                formatter.write_str("history records are not strictly ordered")
            }
        }
    }
}

impl std::error::Error for SessionPayloadAccumulatorError {}

/// Incremental renderer projection. It retains only the materialized transcript/tool/usage/plan
/// state and the current cursor; callers can feed one bounded page at a time without ever building
/// a complete raw event vector.
#[derive(Debug, Clone)]
pub struct SessionPayloadAccumulator {
    payload: MaterializedSessionPayload,
    open_role: Option<&'static str>,
    baseline_used: Option<f64>,
    target_last_seq: Option<u64>,
}

impl SessionPayloadAccumulator {
    #[must_use]
    pub fn new(metadata: &SessionMetadata) -> Self {
        Self {
            payload: MaterializedSessionPayload {
                metadata: SessionPayloadMetadata {
                    id: metadata.session_id.clone(),
                    agent_id: metadata.runtime_agent_id.clone().unwrap_or_default(),
                    agent_config_id: metadata
                        .stable_agent_namespace
                        .as_deref()
                        .and_then(|namespace| namespace.strip_prefix("config:"))
                        .map(str::to_string),
                    title: metadata
                        .title
                        .clone()
                        .unwrap_or_else(|| "Untitled Chat".to_string()),
                    cwd: metadata.cwd.clone(),
                    project_id: metadata.project_id.clone().unwrap_or_default(),
                    created_at: metadata.created_at,
                    last_activity_at: metadata.last_activity_at,
                    message_count: 0,
                    last_seq: 0,
                    status: metadata.status.clone(),
                    worktree_path: metadata.worktree_path.clone(),
                    worktree_branch: metadata.worktree_branch.clone(),
                },
                messages: Vec::new(),
                tool_calls: Vec::new(),
                session_usage: None,
                plan: None,
            },
            open_role: None,
            baseline_used: None,
            target_last_seq: None,
        }
    }

    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.payload.metadata.last_seq
    }

    #[must_use]
    pub fn snapshot(&self) -> MaterializedSessionPayload {
        self.payload.clone()
    }

    #[must_use]
    pub fn finish(self) -> MaterializedSessionPayload {
        self.payload
    }

    pub fn push_history_page(
        &mut self,
        page: &ConversationHistoryPageV1,
        limit: usize,
    ) -> Result<(), SessionPayloadAccumulatorError> {
        page.validate(
            &self.payload.metadata.id,
            self.cursor(),
            limit,
            self.target_last_seq,
        )
        .map_err(SessionPayloadAccumulatorError::Page)?;
        self.target_last_seq = Some(page.target_last_seq);
        self.push_wire_records(&page.records, page.next_cursor)
    }

    pub fn push_records(
        &mut self,
        records: &[PersistedEventRecord],
        next_cursor: u64,
    ) -> Result<(), SessionPayloadAccumulatorError> {
        let mut previous_seq = self.cursor();
        if next_cursor < previous_seq {
            return Err(SessionPayloadAccumulatorError::CursorRegression);
        }
        for record in records {
            if record.session_id != self.payload.metadata.id {
                return Err(SessionPayloadAccumulatorError::SessionMismatch);
            }
            if record.seq <= previous_seq || record.seq > next_cursor {
                return Err(SessionPayloadAccumulatorError::RecordSequenceConflict);
            }
            self.apply_record(
                record.seq,
                record.recorded_at,
                record.type_.as_str(),
                &record.payload,
            );
            previous_seq = record.seq;
        }
        self.advance_cursor(next_cursor)
    }

    fn push_wire_records(
        &mut self,
        records: &[ConversationHistoryRecordV1],
        next_cursor: u64,
    ) -> Result<(), SessionPayloadAccumulatorError> {
        for record in records {
            self.apply_record(
                record.seq,
                record.recorded_at,
                record.type_.as_str(),
                &record.payload,
            );
        }
        self.advance_cursor(next_cursor)
    }

    fn advance_cursor(&mut self, next_cursor: u64) -> Result<(), SessionPayloadAccumulatorError> {
        if next_cursor < self.cursor() {
            return Err(SessionPayloadAccumulatorError::CursorRegression);
        }
        self.payload.metadata.last_seq = next_cursor;
        self.payload.metadata.message_count = self.payload.messages.len() as u64;
        Ok(())
    }

    fn apply_record(&mut self, seq: u64, recorded_at: u64, type_: &str, payload: &Value) {
        match type_ {
            "user_prompt" => self.push_user_prompt(seq, recorded_at, payload),
            "message_chunk" => self.push_message_chunk(seq, recorded_at, payload),
            "tool_call" => {
                self.open_role = None;
                self.upsert_tool_call(seq, recorded_at, payload);
            }
            "tool_call_update" => self.update_tool_call(payload),
            "prompt_complete" => self.open_role = None,
            "usage_update" => self.update_usage(recorded_at, payload),
            "plan_update" => self.update_plan(payload),
            _ => {}
        }
    }

    fn push_user_prompt(&mut self, seq: u64, recorded_at: u64, payload: &Value) {
        self.open_role = None;
        let turn_id = payload
            .get("turnId")
            .and_then(Value::as_str)
            .filter(|turn_id| !turn_id.is_empty());
        let id = turn_id.map_or_else(
            || format!("user:seq-{seq}"),
            |turn_id| format!("turn:{turn_id}"),
        );
        let blocks = payload
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.payload.messages.push(MaterializedChatMessage {
            id,
            role: "user",
            blocks,
            streaming: false,
            timestamp: recorded_at,
            seq,
        });
    }

    fn push_message_chunk(&mut self, seq: u64, recorded_at: u64, payload: &Value) {
        let role = if payload.get("role").and_then(Value::as_str) == Some("thought") {
            "thought"
        } else {
            "agent"
        };
        let Some(content) = payload.get("content").filter(|content| !content.is_null()) else {
            return;
        };
        if self.open_role == Some(role) {
            if let Some(last) = self.payload.messages.last_mut() {
                append_block(&mut last.blocks, content.clone());
            }
            return;
        }
        if is_empty_text_block(content) {
            return;
        }
        self.open_role = Some(role);
        self.payload.messages.push(MaterializedChatMessage {
            id: format!("snapshot:{role}:{seq}"),
            role,
            blocks: vec![content.clone()],
            streaming: false,
            timestamp: recorded_at,
            seq,
        });
    }

    fn upsert_tool_call(&mut self, seq: u64, recorded_at: u64, payload: &Value) {
        let Some(tool_call) = payload.get("toolCall").and_then(Value::as_object) else {
            return;
        };
        let Some(tool_call_id) = tool_call
            .get("toolCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut stamped = tool_call.clone();
        stamped
            .entry("timestamp".to_string())
            .or_insert_with(|| Value::from(recorded_at));
        stamped
            .entry("seq".to_string())
            .or_insert_with(|| Value::from(seq));
        if let Some(index) = self.tool_call_index(tool_call_id) {
            let previous = self.payload.tool_calls[index]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let mut merged = previous.clone();
            merged.extend(stamped);
            if let Some(value) = previous.get("timestamp") {
                merged.insert("timestamp".to_string(), value.clone());
            }
            if let Some(value) = previous.get("seq") {
                merged.insert("seq".to_string(), value.clone());
            }
            self.payload.tool_calls[index] = Value::Object(merged);
        } else {
            self.payload.tool_calls.push(Value::Object(stamped));
        }
    }

    fn update_tool_call(&mut self, payload: &Value) {
        let Some(update) = payload.get("update").and_then(Value::as_object) else {
            return;
        };
        let Some(tool_call_id) = update
            .get("toolCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let Some(index) = self.tool_call_index(tool_call_id) else {
            return;
        };
        let mut merged = self.payload.tool_calls[index]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let timeline = (merged.get("timestamp").cloned(), merged.get("seq").cloned());
        merged.extend(update.clone());
        if let Some(timestamp) = timeline.0 {
            merged.insert("timestamp".to_string(), timestamp);
        }
        if let Some(seq) = timeline.1 {
            merged.insert("seq".to_string(), seq);
        }
        self.payload.tool_calls[index] = Value::Object(merged);
    }

    fn tool_call_index(&self, tool_call_id: &str) -> Option<usize> {
        self.payload.tool_calls.iter().position(|tool_call| {
            tool_call.get("toolCallId").and_then(Value::as_str) == Some(tool_call_id)
        })
    }

    fn update_usage(&mut self, recorded_at: u64, payload: &Value) {
        let Some(used) = payload.get("used").and_then(Value::as_f64) else {
            return;
        };
        let Some(size) = payload.get("size").and_then(Value::as_f64) else {
            return;
        };
        if !used.is_finite() || !size.is_finite() || used <= 0.0 || size <= 0.0 {
            return;
        }
        let baseline_used = *self.baseline_used.get_or_insert(used);
        let mut usage = Map::new();
        usage.insert("used".to_string(), Value::from(used));
        usage.insert("size".to_string(), Value::from(size));
        usage.insert("baselineUsed".to_string(), Value::from(baseline_used));
        usage.insert("updatedAt".to_string(), Value::from(recorded_at));
        usage.insert("source".to_string(), Value::String("reported".to_string()));
        if let Some(cost) = payload.get("cost").filter(|cost| !cost.is_null()) {
            usage.insert("cost".to_string(), cost.clone());
        }
        self.payload.session_usage = Some(Value::Object(usage));
    }

    fn update_plan(&mut self, payload: &Value) {
        let Some(entries) = payload
            .get("plan")
            .and_then(|plan| plan.get("entries"))
            .and_then(Value::as_array)
        else {
            return;
        };
        self.payload.plan = Some(entries.clone());
    }
}

/// Materialize the renderer-shaped payload for one session from durable seq-sorted records. This
/// compatibility wrapper now feeds the same incremental accumulator used by bounded page handlers.
#[must_use]
pub fn materialize_session_payload(
    metadata: &SessionMetadata,
    records: &[PersistedEventRecord],
) -> MaterializedSessionPayload {
    let next_cursor = records
        .last()
        .map_or(metadata.last_seq, |record| record.seq);
    let mut accumulator = SessionPayloadAccumulator::new(metadata);
    accumulator
        .push_records(records, next_cursor)
        .expect("validated persistence records must form one ordered history");
    accumulator.finish()
}

/// `appendBlocks` semantics: text coalesces into a trailing text block; every
/// other block appends.
fn append_block(blocks: &mut Vec<Value>, incoming: Value) {
    if is_text_block(&incoming) {
        if let Some(last) = blocks.last_mut() {
            if is_text_block(last) {
                let merged = format!("{}{}", block_text(last), block_text(&incoming));
                if let Some(object) = last.as_object_mut() {
                    object.insert("text".to_string(), Value::String(merged));
                    return;
                }
            }
        }
    }
    blocks.push(incoming);
}

fn is_text_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
}

fn block_text(block: &Value) -> &str {
    block.get("text").and_then(Value::as_str).unwrap_or("")
}

/// True for a text block whose text is absent or empty (the renderer ignores
/// such a chunk when it would open a new bubble).
fn is_empty_text_block(block: &Value) -> bool {
    is_text_block(block) && block_text(block).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::session_persistence::SESSION_SCHEMA_VERSION;
    use crate::conversation::write_authority::ConversationMutation;
    use crate::conversation::{
        AgentSessionBinding, AgentSessionBindingState, ConversationCreator, ConversationEventType,
        ConversationId, ConversationLifecycleState, ConversationPersistenceAdapter,
        ConversationReader, ConversationRecordV2, ConversationRepository, ConversationWriter,
        CreationPartition, ExecutionTarget, LegacyConversationReader, ReaderPrecedence,
        AGENT_SESSION_BINDING_SCHEMA_VERSION, CONVERSATION_SCHEMA_VERSION,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;

    fn metadata() -> SessionMetadata {
        SessionMetadata {
            schema_version: SESSION_SCHEMA_VERSION,
            storage_key: "0a0b0c0d-0e0f-4a0b-8c0d-0e0f10111213".to_string(),
            session_id: "session-1".to_string(),
            stable_agent_namespace: Some("config:claude".to_string()),
            runtime_agent_id: Some("runtime-1".to_string()),
            project_id: Some("project-1".to_string()),
            cwd: "/work/project".to_string(),
            title: Some("Chat title".to_string()),
            title_source: None,
            created_at: 100,
            last_activity_at: 900,
            status: PersistedSessionStatus::Active,
            message_count: 0,
            tool_count: 0,
            last_seq: 0,
            discovered: false,
            worktree_path: Some("/work/project/.termul/worktrees/chat/abc123".to_string()),
            worktree_branch: Some("chat/abc123".to_string()),
        }
    }

    fn record(seq: u64, type_: &str, payload: Value) -> PersistedEventRecord {
        PersistedEventRecord {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: "session-1".to_string(),
            seq,
            type_: type_.to_string(),
            recorded_at: 100 + seq,
            payload,
        }
    }

    fn user_prompt(seq: u64, turn_id: Option<&str>, text: &str) -> PersistedEventRecord {
        let mut payload = json!({
            "agentId": "runtime-1",
            "sessionId": "session-1",
            "content": [{"type": "text", "text": text}],
        });
        if let Some(turn_id) = turn_id {
            payload["turnId"] = json!(turn_id);
        }
        record(seq, "user_prompt", payload)
    }

    fn chunk(seq: u64, role: &str, text: &str) -> PersistedEventRecord {
        record(
            seq,
            "message_chunk",
            json!({
                "agentId": "runtime-1",
                "sessionId": "session-1",
                "role": role,
                "content": {"type": "text", "text": text},
            }),
        )
    }

    fn tool_call(seq: u64) -> PersistedEventRecord {
        record(
            seq,
            "tool_call",
            json!({
                "agentId": "runtime-1",
                "sessionId": "session-1",
                "toolCall": {"toolCallId": "t-1", "kind": "execute", "status": "completed"},
            }),
        )
    }

    fn tool_call_update(seq: u64) -> PersistedEventRecord {
        record(
            seq,
            "tool_call_update",
            json!({
                "agentId": "runtime-1",
                "sessionId": "session-1",
                "update": {"toolCallId": "t-1", "status": "completed"},
            }),
        )
    }

    fn prompt_complete(seq: u64, turn_id: &str) -> PersistedEventRecord {
        record(
            seq,
            "prompt_complete",
            json!({"sessionId": "session-1", "turnId": turn_id, "stopReason": "end_turn"}),
        )
    }

    #[test]
    fn full_transcript_folds_into_renderer_bubbles() {
        let records = vec![
            user_prompt(1, Some("turn-1"), "hello"),
            chunk(2, "agent", "Hel"),
            chunk(3, "agent", "lo "),
            chunk(4, "thought", "thinking…"),
            tool_call(5),
            chunk(6, "agent", "world"),
            tool_call_update(7),
            chunk(8, "agent", "!"),
            prompt_complete(9, "turn-1"),
            user_prompt(10, Some("turn-2"), "next"),
            chunk(11, "agent", "reply"),
            prompt_complete(12, "turn-2"),
        ];
        let mut meta = metadata();
        meta.last_seq = 12;
        let payload = materialize_session_payload(&meta, &records);

        let ids: Vec<&str> = payload
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                "turn:turn-1",
                "snapshot:agent:2",
                "snapshot:thought:4",
                // tool_call at seq 5 splits; tool_call_update at 7 does NOT.
                "snapshot:agent:6",
                "turn:turn-2",
                "snapshot:agent:11",
            ]
        );
        let seqs: Vec<u64> = payload.messages.iter().map(|message| message.seq).collect();
        assert_eq!(seqs, vec![1, 2, 4, 6, 10, 11]);
        let timestamps: Vec<u64> = payload
            .messages
            .iter()
            .map(|message| message.timestamp)
            .collect();
        assert_eq!(timestamps, vec![101, 102, 104, 106, 110, 111]);
        // Text coalescing within a run (appendBlocks semantics).
        assert_eq!(
            payload.messages[1].blocks,
            vec![json!({"type":"text","text":"Hello "})]
        );
        assert_eq!(
            payload.messages[3].blocks,
            vec![json!({"type":"text","text":"world!"})]
        );
        assert_eq!(payload.messages[1].role, "agent");
        assert_eq!(payload.messages[2].role, "thought");
        assert!(payload.messages.iter().all(|message| !message.streaming));
        assert_eq!(payload.metadata.message_count, 6);
        assert_eq!(payload.metadata.last_seq, 12);
        assert_eq!(payload.tool_calls.len(), 1);
        assert_eq!(payload.tool_calls[0]["toolCallId"], "t-1");
        assert_eq!(payload.tool_calls[0]["seq"], 5);
        assert_eq!(payload.tool_calls[0]["timestamp"], 105);
    }

    #[test]
    fn incremental_pages_preserve_message_tool_usage_and_plan_state() {
        let mut accumulator = SessionPayloadAccumulator::new(&metadata());
        let page_one = ConversationHistoryPageV1 {
            schema_version:
                crate::conversation::contracts::CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION,
            records: vec![
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 1,
                    type_: "user_prompt".to_string(),
                    recorded_at: 101,
                    payload: user_prompt(1, Some("turn-1"), "hello").payload,
                },
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 2,
                    type_: "message_chunk".to_string(),
                    recorded_at: 102,
                    payload: chunk(2, "agent", "a").payload,
                },
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 3,
                    type_: "tool_call".to_string(),
                    recorded_at: 103,
                    payload: tool_call(3).payload,
                },
            ],
            next_cursor: 3,
            complete: false,
            target_last_seq: 7,
        };
        accumulator.push_history_page(&page_one, 3).unwrap();
        let first = accumulator.snapshot();
        assert_eq!(first.metadata.last_seq, 3);
        assert_eq!(first.messages.len(), 2);
        assert_eq!(first.tool_calls.len(), 1);

        let page_two = ConversationHistoryPageV1 {
            schema_version:
                crate::conversation::contracts::CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION,
            records: vec![
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 4,
                    type_: "message_chunk".to_string(),
                    recorded_at: 104,
                    payload: chunk(4, "agent", "b").payload,
                },
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 5,
                    type_: "tool_call_update".to_string(),
                    recorded_at: 105,
                    payload: json!({"update":{"toolCallId":"t-1","status":"failed"}}),
                },
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 6,
                    type_: "usage_update".to_string(),
                    recorded_at: 106,
                    payload: json!({"used":10,"size":100,"cost":{"amount":1.5,"currency":"USD"}}),
                },
                ConversationHistoryRecordV1 {
                    schema_version:
                        crate::conversation::contracts::CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
                    session_id: "session-1".to_string(),
                    seq: 7,
                    type_: "plan_update".to_string(),
                    recorded_at: 107,
                    payload: json!({"plan":{"entries":[{"content":"ship","status":"in_progress"}]}}),
                },
            ],
            next_cursor: 7,
            complete: true,
            target_last_seq: 7,
        };
        accumulator.push_history_page(&page_two, 4).unwrap();
        let payload = accumulator.finish();
        assert_eq!(
            payload
                .messages
                .iter()
                .map(|message| message.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 4]
        );
        assert_eq!(payload.tool_calls[0]["status"], "failed");
        assert_eq!(payload.tool_calls[0]["seq"], 3);
        assert_eq!(
            payload.session_usage.as_ref().unwrap()["baselineUsed"],
            10.0
        );
        assert_eq!(payload.plan.as_ref().unwrap()[0]["content"], "ship");
        assert_eq!(payload.metadata.last_seq, 7);
    }

    #[tokio::test]
    async fn canonical_cold_restart_materializes_usage_plan_and_empty_clear_from_real_pages() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().canonicalize().unwrap().join("private");
        let visible = temp.path().join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let conversation_id =
            ConversationId::parse("55555555-5555-4555-8555-555555555555").unwrap();
        let created_at = Utc
            .timestamp_millis_opt(1_766_000_000_000)
            .single()
            .unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: visible.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
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
                conversation_id,
                AgentSessionBinding {
                    schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
                    binding_id: Uuid::new_v4(),
                    agent_session_id: "session-cold".to_string(),
                    runtime_agent_id: "runtime-cold".to_string(),
                    stable_agent_namespace: "config:test".to_string(),
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
                ConversationEventType::UsageUpdate,
                json!({
                    "agentId":"runtime-cold",
                    "sessionId":"session-cold",
                    "used":12,
                    "size":120,
                    "cost":{"amount":2.5,"currency":"USD"}
                }),
            ),
            (
                ConversationEventType::PlanUpdate,
                json!({
                    "agentId":"runtime-cold",
                    "sessionId":"session-cold",
                    "plan":{"entries":[{"content":"ship","priority":"high","status":"in_progress"}]}
                }),
            ),
            (
                ConversationEventType::PlanUpdate,
                json!({
                    "agentId":"runtime-cold",
                    "sessionId":"session-cold",
                    "plan":{"entries":[]}
                }),
            ),
        ] {
            writer
                .append_event(
                    conversation_id,
                    created_at,
                    type_,
                    payload,
                    ConversationMutation::AcpEventAppend,
                )
                .await
                .unwrap();
        }
        drop(writer);
        drop(repository);

        let (repository, _) = ConversationRepository::open(private).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            LegacyConversationReader::default(),
            ReaderPrecedence::ConversationV2Only,
        ));
        let adapter = ConversationPersistenceAdapter::new(writer, reader);
        let (_, metadata, target_last_seq) = adapter
            .history_metadata("session-cold", "cold_restart_test")
            .unwrap();
        let mut accumulator = SessionPayloadAccumulator::new(&metadata);
        while accumulator.cursor() < target_last_seq {
            let page = adapter
                .history_page_at(
                    "session-cold",
                    accumulator.cursor(),
                    2,
                    Some(target_last_seq),
                )
                .unwrap();
            accumulator.push_history_page(&page, 2).unwrap();
        }
        let payload = accumulator.finish();
        assert_eq!(payload.session_usage.as_ref().unwrap()["used"], 12.0);
        assert_eq!(
            payload.session_usage.as_ref().unwrap()["baselineUsed"],
            12.0
        );
        assert_eq!(payload.plan, Some(Vec::new()));
        assert_eq!(
            adapter.latest_durable_plan("session-cold").unwrap(),
            Some(Vec::new())
        );
        assert_eq!(
            adapter
                .latest_durable_usage("session-cold")
                .unwrap()
                .unwrap()["used"],
            12
        );
    }

    #[test]
    fn metadata_maps_agent_config_prefix_and_fallbacks() {
        let meta = metadata();
        let payload = materialize_session_payload(&meta, &[]);
        assert_eq!(
            serde_json::to_value(&payload.metadata).unwrap(),
            json!({
                "id": "session-1",
                "agentId": "runtime-1",
                "agentConfigId": "claude",
                "title": "Chat title",
                "cwd": "/work/project",
                "projectId": "project-1",
                "createdAt": 100,
                "lastActivityAt": 900,
                "messageCount": 0,
                "lastSeq": 0,
                "status": "active",
                "worktreePath": "/work/project/.termul/worktrees/chat/abc123",
                "worktreeBranch": "chat/abc123",
            })
        );
    }

    #[test]
    fn metadata_omits_agent_config_id_without_config_prefix() {
        let mut meta = metadata();
        meta.stable_agent_namespace = Some("opaque-namespace".to_string());
        let payload = materialize_session_payload(&meta, &[]);
        let value = serde_json::to_value(&payload.metadata).unwrap();
        assert!(
            value.get("agentConfigId").is_none(),
            "agentConfigId must be omitted, not null: {value}"
        );
    }

    #[test]
    fn metadata_falls_back_for_missing_optional_fields() {
        let mut meta = metadata();
        meta.stable_agent_namespace = None;
        meta.runtime_agent_id = None;
        meta.project_id = None;
        meta.title = None;
        meta.status = PersistedSessionStatus::Error;
        let payload = materialize_session_payload(&meta, &[]);
        assert_eq!(payload.metadata.agent_id, "");
        assert_eq!(payload.metadata.agent_config_id, None);
        assert_eq!(payload.metadata.project_id, "");
        assert_eq!(payload.metadata.title, "Untitled Chat");
        assert_eq!(payload.metadata.status, PersistedSessionStatus::Error);
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["metadata"]["status"], "error");
    }

    #[test]
    fn user_prompt_without_turn_id_falls_back_to_seq_id() {
        let records = vec![user_prompt(3, None, "no turn id")];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.messages[0].id, "user:seq-3");
        assert_eq!(payload.messages[0].role, "user");
        assert_eq!(payload.messages[0].seq, 3);
    }

    #[test]
    fn user_prompt_with_empty_turn_id_falls_back_to_seq_id() {
        let records = vec![user_prompt(5, Some(""), "empty turn id")];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages[0].id, "user:seq-5");
    }

    #[test]
    fn role_change_splits_chunk_runs() {
        let records = vec![
            chunk(1, "agent", "a"),
            chunk(2, "thought", "t"),
            chunk(3, "agent", "b"),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(
            payload
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["snapshot:agent:1", "snapshot:thought:2", "snapshot:agent:3"]
        );
    }

    #[test]
    fn prompt_complete_splits_consecutive_agent_runs() {
        let records = vec![
            chunk(1, "agent", "first"),
            prompt_complete(2, "turn-1"),
            chunk(3, "agent", "second"),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 2);
        assert_eq!(payload.messages[0].id, "snapshot:agent:1");
        assert_eq!(payload.messages[1].id, "snapshot:agent:3");
    }

    #[test]
    fn tool_call_update_never_splits_the_open_run() {
        let records = vec![
            chunk(1, "agent", "a"),
            tool_call_update(2),
            chunk(3, "agent", "b"),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.messages[0].id, "snapshot:agent:1");
        assert_eq!(payload.messages[0].seq, 1);
        assert_eq!(
            payload.messages[0].blocks,
            vec![json!({"type":"text","text":"ab"})]
        );
    }

    #[test]
    fn non_text_blocks_append_without_coalescing() {
        let records = vec![
            record(
                1,
                "message_chunk",
                json!({"role":"agent","content":{"type":"text","text":"a"}}),
            ),
            record(
                2,
                "message_chunk",
                json!({"role":"agent","content":{"type":"resource","resource":{"uri":"file:///x"}}}),
            ),
            record(
                3,
                "message_chunk",
                json!({"role":"agent","content":{"type":"text","text":"b"}}),
            ),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 1);
        assert_eq!(
            payload.messages[0].blocks,
            vec![
                json!({"type":"text","text":"a"}),
                json!({"type":"resource","resource":{"uri":"file:///x"}}),
                json!({"type":"text","text":"b"}),
            ]
        );
    }

    #[test]
    fn chunks_without_content_are_skipped() {
        let records = vec![
            record(1, "message_chunk", json!({"role":"agent"})),
            record(
                2,
                "message_chunk",
                json!({"role":"agent","content":Value::Null}),
            ),
            chunk(3, "agent", "real"),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.messages[0].id, "snapshot:agent:3");
    }

    #[test]
    fn empty_text_chunk_never_opens_a_bubble() {
        let records = vec![
            record(
                1,
                "message_chunk",
                json!({"role":"agent","content":{"type":"text","text":""}}),
            ),
            chunk(2, "agent", "content"),
        ];
        let payload = materialize_session_payload(&metadata(), &records);
        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.messages[0].id, "snapshot:agent:2");
    }

    #[test]
    fn empty_registered_session_yields_empty_messages() {
        let payload = materialize_session_payload(&metadata(), &[]);
        assert!(payload.messages.is_empty());
        assert_eq!(payload.metadata.id, "session-1");
        assert_eq!(payload.metadata.message_count, 0);
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["messages"], json!([]));
    }

    #[test]
    fn double_materialization_is_identical() {
        let records = vec![
            user_prompt(1, Some("turn-1"), "hi"),
            chunk(2, "agent", "a"),
            tool_call(3),
            chunk(4, "agent", "b"),
            prompt_complete(5, "turn-1"),
            user_prompt(6, None, "again"),
            chunk(7, "thought", "hmm"),
        ];
        let mut meta = metadata();
        meta.last_seq = 7;
        let first = serde_json::to_value(materialize_session_payload(&meta, &records)).unwrap();
        let second = serde_json::to_value(materialize_session_payload(&meta, &records)).unwrap();
        assert_eq!(first, second, "materialization must be deterministic");
    }

    #[test]
    fn materialized_payload_preserves_worktree_binding() {
        // CAP-4/6: the worktree path + branch must survive materialization
        // so history reopen and post-reload resume reattach to the bound
        // worktree (not the project root) and the indicator can render.
        let payload = materialize_session_payload(&metadata(), &[]);
        assert_eq!(
            payload.metadata.worktree_path.as_deref(),
            Some("/work/project/.termul/worktrees/chat/abc123")
        );
        assert_eq!(
            payload.metadata.worktree_branch.as_deref(),
            Some("chat/abc123")
        );
    }
}
