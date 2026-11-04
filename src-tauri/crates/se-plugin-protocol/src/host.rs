//! Host-owned service contracts. Validation here is structural; authorization,
//! generation leases, declared command membership and project existence belong to the host.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_HOST_CONTEXT_BYTES: usize = 128 * 1024;
pub const MAX_HOST_UI_BYTES: usize = 128 * 1024;
pub const MAX_STORAGE_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_HOST_ACTION_ARGS_BYTES: usize = 8 * 1024;

macro_rules! dto {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
    };
}
macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),* }
    };
}
wire_enum!(AgentSessionStatus { Initializing => "initializing", Active => "active", Error => "error", Closed => "closed" });
wire_enum!(StorageScope { Profile => "profile", Project => "project" });
wire_enum!(NotificationSeverity { Info => "info", Success => "success", Warning => "warning", Error => "error" });
wire_enum!(ProgressState { Running => "running", Completed => "completed", Failed => "failed", Cancelled => "cancelled" });
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusTone {
    #[default]
    Default,
    Muted,
    Success,
    Warning,
    Danger,
}
wire_enum!(HostUiKind { Notification => "notification", Progress => "progress" });
wire_enum!(ContributionKind { Menu => "menu", Keybinding => "keybinding", Status => "status", Notification => "notification", Progress => "progress" });

dto!(ProjectContext {
    id: String,
    name: String,
    path: String
});
dto!(TerminalContext {
    id: String,
    #[serde(deserialize_with = "nullable_option")] pty_id: Option<String>,
    #[serde(deserialize_with = "nullable_option")] project_id: Option<String>
});
dto!(AgentSessionContext {
    id: String, project_id: String, status: AgentSessionStatus, active_turn: bool,
    #[serde(deserialize_with = "nullable_option")] open_turn_id: Option<String>, replaying: bool
});
dto!(HostFocus {
    text_input: bool,
    terminal: bool
});
dto!(HostContextState {
    #[serde(deserialize_with = "nullable_option")] active_project: Option<ProjectContext>,
    project_ids: Vec<String>,
    #[serde(deserialize_with = "nullable_option")] active_terminal_id: Option<String>,
    #[serde(deserialize_with = "nullable_option")] active_session_id: Option<String>,
    terminals: Vec<TerminalContext>, agent_sessions: Vec<AgentSessionContext>, focus: HostFocus, truncated: bool
});
dto!(HostContextSnapshotRequest { scope: HostScope });
dto!(HostContextSnapshot { source_id: String, context_revision: u64, sequences: BTreeMap<String, u64>, scope: HostScope, context: HostContextState });
dto!(ProjectActiveChangedData {
    #[serde(deserialize_with = "nullable_option")] previous_project_id: Option<String>,
    #[serde(deserialize_with = "nullable_option")] project_id: Option<String>
});
dto!(TerminalLifecycleData {
    terminal: TerminalContext
});
dto!(TerminalActiveChangedData {
    #[serde(deserialize_with = "nullable_option")] terminal_id: Option<String>,
    #[serde(deserialize_with = "nullable_option")] project_id: Option<String>
});
dto!(AgentSessionStateChangedData {
    session: AgentSessionContext,
    removed: bool
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HostEventData {
    ProjectActiveChanged(ProjectActiveChangedData),
    TerminalLifecycle(TerminalLifecycleData),
    TerminalActiveChanged(TerminalActiveChangedData),
    AgentSessionStateChanged(AgentSessionStateChangedData),
}
dto!(HostSourceEvent { topic: HostEventTopic, #[serde(deserialize_with = "nullable_option")] project_id: Option<String>, data: HostEventData });
dto!(HostContextPublishRequest { source_id: String, source_sequence: u64, reset: bool, snapshot: HostContextState, events: Vec<HostSourceEvent> });
dto!(HostContextHeartbeatRequest { source_id: String });
dto!(HostContextAck {
    context_revision: u64,
    source_sequence: u64
});
dto!(HostEventDeliveryInput {
    subscription_id: String, generation: String, source_id: String, sequence: u64,
    context_revision: u64, topic: HostEventTopic,
    #[serde(deserialize_with = "nullable_option")] project_id: Option<String>,
    #[serde(deserialize_with = "nullable_option")] data: Option<HostEventData>, resync_required: bool
});
dto!(HostDeliveryResult { invalidate_views: Vec<String> });
dto!(HostDeliveryReply {
    schema_version: u32,
    result: HostDeliveryResult
});

/// The `ok` discriminant is validated (true for Ok, false for Err).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HostResult<T> {
    Ok(HostSuccess<T>),
    Err(HostFailure),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    bound(deserialize = "T: Deserialize<'de>")
)]
pub struct HostSuccess<T> {
    pub ok: bool,
    #[serde(deserialize_with = "required_generic")]
    pub value: T,
}
dto!(HostFailure {
    ok: bool,
    error: PluginError
});
pub type HostRequest = ServiceRequest;
/// Strict host-origin callbacks use the existing ABI service-call envelope.
pub type HostDeliveryRequest = ServiceRequest;
dto!(HostReply { schema_version: u32, result: HostResult<Value> });
dto!(StorageGetRequest {
    request_id: String, scope: StorageScope, key: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>
});
dto!(StorageSetRequest {
    request_id: String, scope: StorageScope, key: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>,
    #[serde(deserialize_with = "required_value")] value: Value, expected_revision: u64
});
dto!(StorageDeleteRequest {
    request_id: String, scope: StorageScope, key: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>, expected_revision: u64
});
dto!(StorageTicket {
    accepted: bool,
    ticket_id: String,
    request_id: String
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StorageCompletionValue {
    Get(StorageGetValue),
    Mutation(StorageMutationValue),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StorageGetValue {
    Found(StorageFoundValue),
    Missing(StorageMissingValue),
}
dto!(StorageFoundValue {
    found: bool,
    #[serde(deserialize_with = "required_value")]
    value: Value,
    revision: u64
});
dto!(StorageMissingValue {
    found: bool,
    revision: u64
});
dto!(StorageMutationValue { revision: u64 });
dto!(StorageCompletionInput { request_id: String, ticket_id: String, result: HostResult<StorageCompletionValue> });
dto!(NotificationAction {
    label: String,
    command_id: String,
    #[serde(deserialize_with = "required_value")]
    args: Value
});
fn default_ttl() -> u32 {
    8000
}
fn default_visible() -> bool {
    true
}
dto!(NotificationUpsertRequest {
    id: String, severity: NotificationSeverity, title: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] message: Option<String>,
    #[serde(default)] actions: Vec<NotificationAction>, #[serde(default = "default_ttl")] ttl_ms: u32
});
dto!(NotificationCloseRequest { id: String });
dto!(ProgressUpsertRequest {
    id: String, title: String, state: ProgressState,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] cancel_command_id: Option<String>
});
dto!(StatusSetRequest {
    id: String, text: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] icon: Option<String>,
    #[serde(default)] tone: StatusTone,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] tooltip: Option<String>,
    #[serde(default = "default_visible")] visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>
});
dto!(StatusClearRequest { id: String });
dto!(HostAccepted {
    accepted: bool,
    id: String
});
dto!(NotificationCloseResult { closed: bool });
dto!(StatusClearResult { cleared: bool });
dto!(HostUiSnapshot { schema_version: u32, boot_id: String, revision: u64, notifications: Vec<HostNotificationRecord>, progress: Vec<HostProgressRecord>, status_items: Vec<HostStatusRecord> });
dto!(HostNotificationRecord {
    plugin_id: String, plugin_name: String, generation: String, revision: u64,
    created_at: u64, updated_at: u64, #[serde(deserialize_with = "nullable_option")] expires_at: Option<u64>,
    id: String, severity: NotificationSeverity, title: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] message: Option<String>,
    #[serde(default)] actions: Vec<NotificationAction>, #[serde(default = "default_ttl")] ttl_ms: u32
});
dto!(HostProgressRecord {
    plugin_id: String, plugin_name: String, generation: String, revision: u64,
    created_at: u64, updated_at: u64, #[serde(deserialize_with = "nullable_option")] expires_at: Option<u64>,
    id: String, title: String, state: ProgressState,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] cancel_command_id: Option<String>
});
wire_enum!(StatusLocation { StatusBar => "statusBar" });
dto!(HostStatusRecord {
    plugin_id: String, plugin_name: String, generation: String, revision: u64,
    created_at: u64, updated_at: u64, #[serde(deserialize_with = "nullable_option")] expires_at: Option<u64>,
    id: String, label: String, location: StatusLocation, scope: HostScope,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] command_id: Option<String>,
    #[serde(default)] order: i32,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] icon: Option<String>,
    #[serde(default)] tone: StatusTone,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] tooltip: Option<String>,
    #[serde(default = "default_visible")] visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>
});
dto!(HostUiGetRequest {});
dto!(HostUiDismissRequest {
    plugin_id: String,
    generation: String,
    kind: HostUiKind,
    id: String
});
dto!(HostUiDismissResult { dismissed: bool });
dto!(ContributionTarget {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] session_id: Option<String>
});
dto!(ExecuteContributionRequest {
    plugin_id: String, generation: String, kind: ContributionKind, contribution_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] action_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_option")] record_revision: Option<u64>,
    context_revision: u64, target: ContributionTarget
});

fn check(ok: bool) -> Result<(), PluginError> {
    require(ok, PLUGIN_INVALID_REQUEST, "invalid host contract")
}
fn opaque(id: &str) -> Result<(), PluginError> {
    text(id, 1, 128, PLUGIN_INVALID_REQUEST)
}
fn nullable_id(id: &Option<String>) -> Result<(), PluginError> {
    if let Some(id) = id {
        opaque(id)?;
    }
    Ok(())
}
fn local(id: &str) -> Result<(), PluginError> {
    check(is_local_id(id))
}
fn safe(n: u64) -> Result<(), PluginError> {
    check(n <= MAX_SAFE_INTEGER)
}
fn bounded<T: Serialize>(v: &T, cap: usize) -> Result<(), PluginError> {
    let value = serde_json::to_value(v)
        .map_err(|_| PluginError::new(PLUGIN_INVALID_REQUEST, "invalid host JSON"))?;
    check(js_json_bytes(&value) <= cap)
}
fn distinct<'a>(ids: impl Iterator<Item = &'a str>) -> Result<(), PluginError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        check(seen.insert(id))?;
    }
    Ok(())
}
pub fn validate_project_context(v: &ProjectContext) -> Result<(), PluginError> {
    opaque(&v.id)?;
    text(&v.name, 1, 128, PLUGIN_INVALID_REQUEST)?;
    text(&v.path, 1, 4096, PLUGIN_INVALID_REQUEST)
}
pub fn validate_terminal_context(v: &TerminalContext) -> Result<(), PluginError> {
    opaque(&v.id)?;
    nullable_id(&v.pty_id)?;
    nullable_id(&v.project_id)
}
pub fn validate_agent_session_context(v: &AgentSessionContext) -> Result<(), PluginError> {
    opaque(&v.id)?;
    opaque(&v.project_id)?;
    nullable_id(&v.open_turn_id)
}
pub fn validate_host_context_state(v: &HostContextState) -> Result<(), PluginError> {
    check(v.project_ids.len() <= 256 && v.terminals.len() <= 256 && v.agent_sessions.len() <= 256)?;
    if let Some(project) = &v.active_project {
        validate_project_context(project)?;
    }
    for id in &v.project_ids {
        opaque(id)?;
    }
    distinct(v.project_ids.iter().map(String::as_str))?;
    nullable_id(&v.active_terminal_id)?;
    nullable_id(&v.active_session_id)?;
    for terminal in &v.terminals {
        validate_terminal_context(terminal)?;
    }
    for session in &v.agent_sessions {
        validate_agent_session_context(session)?;
    }
    distinct(v.terminals.iter().map(|t| t.id.as_str()))?;
    distinct(v.agent_sessions.iter().map(|s| s.id.as_str()))?;
    bounded(v, MAX_HOST_CONTEXT_BYTES)
}
pub fn validate_host_context_snapshot(v: &HostContextSnapshot) -> Result<(), PluginError> {
    check(is_uuid(&v.source_id) && v.sequences.len() <= 16)?;
    safe(v.context_revision)?;
    for (id, seq) in &v.sequences {
        local(id)?;
        safe(*seq)?;
    }
    validate_host_context_state(&v.context)?;
    json_size(v, PLUGIN_INVALID_REQUEST)
}
fn event_data(topic: HostEventTopic, data: &HostEventData) -> Result<(), PluginError> {
    match (topic, data) {
        (HostEventTopic::ProjectActiveChanged, HostEventData::ProjectActiveChanged(v)) => {
            nullable_id(&v.previous_project_id)?;
            nullable_id(&v.project_id)
        }
        (
            HostEventTopic::TerminalCreated | HostEventTopic::TerminalClosed,
            HostEventData::TerminalLifecycle(v),
        ) => validate_terminal_context(&v.terminal),
        (HostEventTopic::TerminalActiveChanged, HostEventData::TerminalActiveChanged(v)) => {
            nullable_id(&v.terminal_id)?;
            nullable_id(&v.project_id)
        }
        (HostEventTopic::AgentSessionStateChanged, HostEventData::AgentSessionStateChanged(v)) => {
            validate_agent_session_context(&v.session)
        }
        _ => check(false),
    }
}
pub fn validate_host_source_event(v: &HostSourceEvent) -> Result<(), PluginError> {
    nullable_id(&v.project_id)?;
    event_data(v.topic, &v.data)
}
pub fn validate_host_context_publish_request(
    v: &HostContextPublishRequest,
) -> Result<(), PluginError> {
    check(is_uuid(&v.source_id) && v.events.len() <= 128)?;
    revision(v.source_sequence, PLUGIN_INVALID_REQUEST)?;
    validate_host_context_state(&v.snapshot)?;
    for event in &v.events {
        validate_host_source_event(event)?;
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_host_context_heartbeat_request(
    v: &HostContextHeartbeatRequest,
) -> Result<(), PluginError> {
    check(is_uuid(&v.source_id))
}
pub fn validate_host_context_ack(v: &HostContextAck) -> Result<(), PluginError> {
    safe(v.context_revision)?;
    revision(v.source_sequence, PLUGIN_INVALID_REQUEST)
}
pub fn validate_host_event_delivery_input(v: &HostEventDeliveryInput) -> Result<(), PluginError> {
    local(&v.subscription_id)?;
    check(is_uuid(&v.generation) && is_uuid(&v.source_id))?;
    revision(v.sequence, PLUGIN_INVALID_REQUEST)?;
    safe(v.context_revision)?;
    nullable_id(&v.project_id)?;
    check(v.resync_required == v.data.is_none())?;
    if let Some(data) = &v.data {
        event_data(v.topic, data)?;
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_host_delivery_reply(v: &HostDeliveryReply) -> Result<(), PluginError> {
    version(v.schema_version, PLUGIN_INVALID_REQUEST)?;
    check(v.result.invalidate_views.len() <= 8)?;
    for id in &v.result.invalidate_views {
        local(id)?;
    }
    distinct(v.result.invalidate_views.iter().map(String::as_str))
}
fn storage_identity(
    request: &str,
    scope: StorageScope,
    key: &str,
    project: &Option<String>,
) -> Result<(), PluginError> {
    local(request)?;
    local(key)?;
    check((scope == StorageScope::Project) == project.is_some())?;
    nullable_id(project)
}
pub fn validate_storage_get_request(v: &StorageGetRequest) -> Result<(), PluginError> {
    storage_identity(&v.request_id, v.scope, &v.key, &v.project_id)
}
pub fn validate_storage_set_request(v: &StorageSetRequest) -> Result<(), PluginError> {
    storage_identity(&v.request_id, v.scope, &v.key, &v.project_id)?;
    safe(v.expected_revision)?;
    bounded(&v.value, MAX_STORAGE_VALUE_BYTES)
}
pub fn validate_storage_delete_request(v: &StorageDeleteRequest) -> Result<(), PluginError> {
    storage_identity(&v.request_id, v.scope, &v.key, &v.project_id)?;
    safe(v.expected_revision)
}
pub fn validate_storage_ticket(v: &StorageTicket) -> Result<(), PluginError> {
    check(v.accepted && is_uuid(&v.ticket_id))?;
    local(&v.request_id)
}
fn host_error(v: &PluginError) -> Result<(), PluginError> {
    check(
        v.code.starts_with("PLUGIN_")
            && v.code.len() > 7
            && v.code[7..]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
    )?;
    text(&v.message, 1, 2048, PLUGIN_INVALID_REQUEST)
}
fn host_result<T>(v: &HostResult<T>) -> Result<(), PluginError> {
    match v {
        HostResult::Ok(v) => check(v.ok),
        HostResult::Err(v) => {
            check(!v.ok)?;
            host_error(&v.error)
        }
    }
}
pub fn validate_host_reply(v: &HostReply) -> Result<(), PluginError> {
    version(v.schema_version, PLUGIN_INVALID_REQUEST)?;
    host_result(&v.result)?;
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_storage_completion_input(v: &StorageCompletionInput) -> Result<(), PluginError> {
    local(&v.request_id)?;
    check(is_uuid(&v.ticket_id))?;
    host_result(&v.result)?;
    if let HostResult::Ok(ok) = &v.result {
        match &ok.value {
            StorageCompletionValue::Mutation(v) => safe(v.revision)?,
            StorageCompletionValue::Get(StorageGetValue::Found(v)) => {
                check(v.found)?;
                safe(v.revision)?;
                bounded(&v.value, MAX_STORAGE_VALUE_BYTES)?;
            }
            StorageCompletionValue::Get(StorageGetValue::Missing(v)) => {
                check(!v.found)?;
                safe(v.revision)?;
            }
        }
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_notification_upsert_request(
    v: &NotificationUpsertRequest,
) -> Result<(), PluginError> {
    local(&v.id)?;
    text(&v.title, 1, 128, PLUGIN_INVALID_REQUEST)?;
    if let Some(message) = &v.message {
        text(message, 0, 1024, PLUGIN_INVALID_REQUEST)?;
    }
    check(v.actions.len() <= 2 && (2000..=30000).contains(&v.ttl_ms))?;
    for action in &v.actions {
        text(&action.label, 1, 64, PLUGIN_INVALID_REQUEST)?;
        local(&action.command_id)?;
        bounded(&action.args, MAX_HOST_ACTION_ARGS_BYTES)?;
    }
    Ok(())
}
pub fn validate_notification_close_request(
    v: &NotificationCloseRequest,
) -> Result<(), PluginError> {
    local(&v.id)
}
pub fn validate_progress_upsert_request(v: &ProgressUpsertRequest) -> Result<(), PluginError> {
    local(&v.id)?;
    text(&v.title, 1, 128, PLUGIN_INVALID_REQUEST)?;
    if let Some(fraction) = v.fraction {
        check(fraction.is_finite() && (0.0..=1.0).contains(&fraction))?;
    }
    if let Some(command) = &v.cancel_command_id {
        local(command)?;
    }
    Ok(())
}
pub fn validate_status_set_request(v: &StatusSetRequest) -> Result<(), PluginError> {
    local(&v.id)?;
    text(&v.text, 1, 32, PLUGIN_INVALID_REQUEST)?;
    if let Some(icon) = &v.icon {
        check(is_view_icon(icon))?;
    }
    if let Some(tooltip) = &v.tooltip {
        text(tooltip, 0, 256, PLUGIN_INVALID_REQUEST)?;
    }
    nullable_id(&v.project_id)
}
pub fn validate_status_clear_request(v: &StatusClearRequest) -> Result<(), PluginError> {
    local(&v.id)
}
#[allow(clippy::too_many_arguments)]
fn metadata(
    plugin: &str,
    name: &str,
    generation: &str,
    rev: u64,
    created: u64,
    updated: u64,
    expires: Option<u64>,
) -> Result<(), PluginError> {
    check(is_plugin_id(plugin) && is_uuid(generation))?;
    text(name, 1, 128, PLUGIN_INVALID_REQUEST)?;
    safe(rev)?;
    safe(created)?;
    safe(updated)?;
    if let Some(expires) = expires {
        safe(expires)?;
    }
    Ok(())
}
pub fn validate_host_ui_snapshot(v: &HostUiSnapshot) -> Result<(), PluginError> {
    version(v.schema_version, PLUGIN_INVALID_REQUEST)?;
    check(is_uuid(&v.boot_id))?;
    safe(v.revision)?;
    check(v.notifications.len() + v.progress.len() + v.status_items.len() <= 128)?;
    let mut counts: BTreeMap<(&str, &str), [usize; 3]> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for n in &v.notifications {
        metadata(
            &n.plugin_id,
            &n.plugin_name,
            &n.generation,
            n.revision,
            n.created_at,
            n.updated_at,
            n.expires_at,
        )?;
        validate_notification_upsert_request(&NotificationUpsertRequest {
            id: n.id.clone(),
            severity: n.severity,
            title: n.title.clone(),
            message: n.message.clone(),
            actions: n.actions.clone(),
            ttl_ms: n.ttl_ms,
        })?;
        check(ids.insert((&n.plugin_id, &n.generation, 0, &n.id)))?;
        counts.entry((&n.plugin_id, &n.generation)).or_default()[0] += 1;
    }
    for p in &v.progress {
        metadata(
            &p.plugin_id,
            &p.plugin_name,
            &p.generation,
            p.revision,
            p.created_at,
            p.updated_at,
            p.expires_at,
        )?;
        validate_progress_upsert_request(&ProgressUpsertRequest {
            id: p.id.clone(),
            title: p.title.clone(),
            state: p.state,
            fraction: p.fraction,
            cancel_command_id: p.cancel_command_id.clone(),
        })?;
        check(ids.insert((&p.plugin_id, &p.generation, 1, &p.id)))?;
        counts.entry((&p.plugin_id, &p.generation)).or_default()[1] += 1;
    }
    for s in &v.status_items {
        metadata(
            &s.plugin_id,
            &s.plugin_name,
            &s.generation,
            s.revision,
            s.created_at,
            s.updated_at,
            s.expires_at,
        )?;
        text(&s.label, 1, 128, PLUGIN_INVALID_REQUEST)?;
        check((-100..=100).contains(&s.order))?;
        if let Some(command) = &s.command_id {
            local(command)?;
        }
        check((s.scope == HostScope::ActiveProject) == s.project_id.is_some())?;
        validate_status_set_request(&StatusSetRequest {
            id: s.id.clone(),
            text: s.text.clone(),
            icon: s.icon.clone(),
            tone: s.tone,
            tooltip: s.tooltip.clone(),
            visible: s.visible,
            project_id: s.project_id.clone(),
        })?;
        check(ids.insert((&s.plugin_id, &s.generation, 2, &s.id)))?;
        counts.entry((&s.plugin_id, &s.generation)).or_default()[2] += 1;
    }
    for count in counts.values() {
        check(count[0] <= 8 && count[1] <= 2 && count[2] <= 2)?;
    }
    bounded(v, MAX_HOST_UI_BYTES)
}
pub fn validate_host_ui_dismiss_request(v: &HostUiDismissRequest) -> Result<(), PluginError> {
    check(is_plugin_id(&v.plugin_id) && is_uuid(&v.generation))?;
    local(&v.id)
}
pub fn validate_host_ui_dismiss_result(v: &HostUiDismissResult) -> Result<(), PluginError> {
    check(v.dismissed)
}
pub fn validate_execute_contribution_request(
    v: &ExecuteContributionRequest,
) -> Result<(), PluginError> {
    check(is_plugin_id(&v.plugin_id) && is_uuid(&v.generation))?;
    local(&v.contribution_id)?;
    safe(v.context_revision)?;
    if matches!(
        v.kind,
        ContributionKind::Notification | ContributionKind::Progress
    ) {
        safe(
            v.record_revision.ok_or_else(|| {
                PluginError::new(PLUGIN_INVALID_REQUEST, "missing record revision")
            })?,
        )?;
    } else {
        check(v.record_revision.is_none())?;
    }
    check(if v.kind == ContributionKind::Notification {
        v.action_index.is_some_and(|i| i < 2)
    } else {
        v.action_index.is_none()
    })?;
    nullable_id(&v.target.project_id)?;
    nullable_id(&v.target.terminal_id)?;
    nullable_id(&v.target.session_id)
}
pub fn validate_host_request(v: &HostRequest) -> Result<(), PluginError> {
    version(v.schema_version, PLUGIN_INVALID_REQUEST)?;
    check(v.version == 1)?;
    let bytes = serde_json::to_vec(&v.input)
        .map_err(|_| PluginError::new(PLUGIN_INVALID_REQUEST, "invalid host input"))?;
    match (v.service.as_str(), v.method.as_str()) {
        ("se.host.context", "snapshot") => {
            let _: HostContextSnapshotRequest = json(&bytes, PLUGIN_INVALID_REQUEST)?;
        }
        ("se.host.storage", "get") => {
            parse_storage_get_request(&bytes)?;
        }
        ("se.host.storage", "set") => {
            parse_storage_set_request(&bytes)?;
        }
        ("se.host.storage", "delete") => {
            parse_storage_delete_request(&bytes)?;
        }
        ("se.host.feedback", "notification.upsert") => {
            parse_notification_upsert_request(&bytes)?;
        }
        ("se.host.feedback", "notification.close") => {
            parse_notification_close_request(&bytes)?;
        }
        ("se.host.feedback", "progress.upsert") => {
            parse_progress_upsert_request(&bytes)?;
        }
        ("se.host.status", "set") => {
            parse_status_set_request(&bytes)?;
        }
        ("se.host.ui", "postMessage") => {
            parse_host_ui_post_message_request(&bytes)?;
        }
        ("se.host.wake", "request") => {
            parse_host_wake_request(&bytes)?;
        }
        ("se.host.process", "status" | "start" | "stop" | "restart") => {
            parse_host_process_request(&bytes)?;
        }
        ("se.host.process", "logs") => {
            parse_host_process_logs_request(&bytes)?;
        }
        ("se.host.status", "clear") => {
            parse_status_clear_request(&bytes)?;
        }
        _ => return check(false),
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}

pub fn validate_host_context_snapshot_request(
    _: &HostContextSnapshotRequest,
) -> Result<(), PluginError> {
    Ok(())
}
pub fn validate_host_ui_get_request(_: &HostUiGetRequest) -> Result<(), PluginError> {
    Ok(())
}
pub fn validate_host_accepted(v: &HostAccepted) -> Result<(), PluginError> {
    check(v.accepted)?;
    local(&v.id)
}
pub fn validate_notification_close_result(v: &NotificationCloseResult) -> Result<(), PluginError> {
    check(v.closed)
}
pub fn validate_status_clear_result(v: &StatusClearResult) -> Result<(), PluginError> {
    check(v.cleared)
}
pub fn validate_host_delivery_request(v: &HostDeliveryRequest) -> Result<(), PluginError> {
    version(v.schema_version, PLUGIN_INVALID_REQUEST)?;
    check(v.version == 1)?;
    let bytes = serde_json::to_vec(&v.input)
        .map_err(|_| PluginError::new(PLUGIN_INVALID_REQUEST, "invalid host input"))?;
    match (v.service.as_str(), v.method.as_str()) {
        ("se.host.events", "deliver") => {
            parse_host_event_delivery_input(&bytes)?;
        }
        ("se.host.wake", "wake") => {
            parse_host_wake_delivery(&bytes)?;
        }
        ("se.host.storage", "completed") => {
            parse_storage_completion_input(&bytes)?;
        }
        _ => return check(false),
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}

// Quotas use the UTF-8 JSON.stringify representation used by the renderer.
// serde_json uses scientific notation at different thresholds (notably 1e20).
fn js_number(number: &serde_json::Number) -> String {
    let n = number.as_f64().unwrap_or(0.0);
    if n == 0.0 {
        return "0".into();
    }
    let raw = serde_json::Number::from_f64(n)
        .expect("finite JSON number")
        .to_string();
    let (mantissa, exponent) = raw.split_once('e').map_or((raw.as_str(), 0), |(m, e)| {
        (m, e.parse::<i32>().expect("JSON exponent"))
    });
    let negative = mantissa.starts_with('-');
    let m = mantissa.trim_start_matches('-');
    let decimal = m.find('.').unwrap_or(m.len()) as i32;
    let digits = m.replace('.', "");
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let point = decimal + exponent;
    let sign = if negative { "-" } else { "" };
    if (-5..=21).contains(&point) {
        if point <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        } else if point >= digits.len() as i32 {
            format!(
                "{sign}{digits}{}",
                "0".repeat(point as usize - digits.len())
            )
        } else {
            format!(
                "{sign}{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        }
    } else {
        let exp = point - 1;
        let tail = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!(
            "{sign}{}{tail}e{}{exp}",
            &digits[..1],
            if exp >= 0 { "+" } else { "" }
        )
    }
}
fn js_json_bytes(v: &Value) -> usize {
    match v {
        Value::Null => 4,
        Value::Bool(b) => {
            if *b {
                4
            } else {
                5
            }
        }
        Value::Number(n) => js_number(n).len(),
        Value::String(s) => serde_json::to_string(s).expect("JSON string").len(),
        Value::Array(a) => {
            2 + a.len().saturating_sub(1) + a.iter().map(js_json_bytes).sum::<usize>()
        }
        Value::Object(o) => {
            2 + o.len().saturating_sub(1)
                + o.iter()
                    .map(|(k, v)| {
                        serde_json::to_string(k).expect("JSON key").len() + 1 + js_json_bytes(v)
                    })
                    .sum::<usize>()
        }
    }
}

// A Value-backed service input must not erase duplicate fields before typed admission.
struct StrictHostJson(Value);
impl<'de> Deserialize<'de> for StrictHostJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictHostJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("strict host JSON")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictHostJson(Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictHostJson(Value::Bool(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictHostJson(Value::String(v.into())))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictHostJson(Value::String(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictHostJson(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictHostJson(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                let number = if v >= 0.0 && v <= MAX_SAFE_INTEGER as f64 && v.fract() == 0.0 {
                    serde_json::Number::from(v as u64)
                } else {
                    serde_json::Number::from_f64(v).ok_or_else(|| E::custom("invalid number"))?
                };
                Ok(StrictHostJson(Value::Number(number)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(StrictHostJson(v)) = a.next_element()? {
                    out.push(v);
                }
                Ok(StrictHostJson(Value::Array(out)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate host JSON property"));
                    }
                    let StrictHostJson(v) = a.next_value()?;
                    out.insert(k, v);
                }
                Ok(StrictHostJson(Value::Object(out)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
pub(crate) fn strict_contract_json<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    code: &str,
    limit: usize,
) -> Result<T, PluginError> {
    require(bytes.len() <= limit, code, "JSON exceeds contract limit")?;
    let StrictHostJson(value) = serde_json::from_slice(bytes)
        .map_err(|_| PluginError::new(code, "invalid contract JSON"))?;
    serde_json::from_value(value).map_err(|_| PluginError::new(code, "invalid contract JSON"))
}
fn host_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PluginError> {
    strict_contract_json(bytes, PLUGIN_INVALID_REQUEST, MAX_JSON_BYTES)
}

macro_rules! parser {
    ($($parse:ident : $ty:ty => $validate:ident),* $(,)?) => { $(
        pub fn $parse(bytes: &[u8]) -> Result<$ty,PluginError> {
            let v = host_json(bytes)?;
            $validate(&v)?;
            Ok(v)
        }
    )* };
}
parser!(
    parse_host_delivery_request: HostDeliveryRequest => validate_host_delivery_request,
    parse_host_context_snapshot_request: HostContextSnapshotRequest => validate_host_context_snapshot_request,
    parse_host_ui_get_request: HostUiGetRequest => validate_host_ui_get_request,
    parse_host_accepted: HostAccepted => validate_host_accepted,
    parse_notification_close_result: NotificationCloseResult => validate_notification_close_result,
    parse_status_clear_result: StatusClearResult => validate_status_clear_result,
    parse_host_request: HostRequest => validate_host_request,
    parse_host_reply: HostReply => validate_host_reply,
    parse_host_context_state: HostContextState => validate_host_context_state,
    parse_host_context_snapshot: HostContextSnapshot => validate_host_context_snapshot,
    parse_host_context_publish_request: HostContextPublishRequest => validate_host_context_publish_request,
    parse_host_context_heartbeat_request: HostContextHeartbeatRequest => validate_host_context_heartbeat_request,
    parse_host_context_ack: HostContextAck => validate_host_context_ack,
    parse_host_source_event: HostSourceEvent => validate_host_source_event,
    parse_host_event_delivery_input: HostEventDeliveryInput => validate_host_event_delivery_input,
    parse_host_delivery_reply: HostDeliveryReply => validate_host_delivery_reply,
    parse_storage_get_request: StorageGetRequest => validate_storage_get_request,
    parse_storage_set_request: StorageSetRequest => validate_storage_set_request,
    parse_storage_delete_request: StorageDeleteRequest => validate_storage_delete_request,
    parse_storage_ticket: StorageTicket => validate_storage_ticket,
    parse_storage_completion_input: StorageCompletionInput => validate_storage_completion_input,
    parse_notification_upsert_request: NotificationUpsertRequest => validate_notification_upsert_request,
    parse_notification_close_request: NotificationCloseRequest => validate_notification_close_request,
    parse_progress_upsert_request: ProgressUpsertRequest => validate_progress_upsert_request,
    parse_status_set_request: StatusSetRequest => validate_status_set_request,
    parse_status_clear_request: StatusClearRequest => validate_status_clear_request,
    parse_host_ui_snapshot: HostUiSnapshot => validate_host_ui_snapshot,
    parse_host_ui_dismiss_request: HostUiDismissRequest => validate_host_ui_dismiss_request,
    parse_host_ui_dismiss_result: HostUiDismissResult => validate_host_ui_dismiss_result,
    parse_execute_contribution_request: ExecuteContributionRequest => validate_execute_contribution_request,
);

// Required nullable properties must distinguish absent from explicitly null.
fn nullable_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
fn required_generic<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<T, D::Error> {
    T::deserialize(d)
}
fn required_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    Value::deserialize(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parsers_reject_duplicate_keys_bad_utf8_and_redact_inputs() {
        for input in [
            br#"{"requestId":"private-value","scope":"profile","key":"x","key":"y"}"#.as_slice(),
            br#"{"requestId":"private-value","scope":"profile","key":"x","unknown":"credential-like"}"#.as_slice(),
            &[0xff],
        ] {
            let error = parse_storage_get_request(input).unwrap_err();
            assert_eq!(error.code, PLUGIN_INVALID_REQUEST);
            assert!(!error.message.contains("private-value"));
            assert!(!error.message.contains("credential-like"));
        }
    }

    #[test]
    fn host_storage_preserves_present_null_and_absent_optional_fields() {
        let request = parse_storage_set_request(br#"{"requestId":"save","scope":"profile","key":"x","value":null,"expectedRevision":0}"#).unwrap();
        let serialized = serde_json::to_value(&request).unwrap();
        assert!(serialized.get("value").unwrap().is_null());
        assert!(serialized.get("projectId").is_none());
        assert!(parse_storage_set_request(
            br#"{"requestId":"save","scope":"profile","key":"x","expectedRevision":0}"#
        )
        .is_err());
        assert!(parse_host_reply(br#"{"schemaVersion":1,"result":{"ok":true}}"#).is_err());
    }

    #[test]
    fn host_envelopes_reject_nested_duplicate_properties() {
        assert!(parse_host_request(br#"{"schemaVersion":1,"service":"se.host.status","version":1,"method":"set","input":{"id":"a","id":"b","text":"ok"}}"#).is_err());
        assert!(parse_host_delivery_request(br#"{"schemaVersion":1,"service":"se.host.storage","version":1,"method":"completed","input":{"requestId":"x","ticketId":"550e8400-e29b-41d4-a716-446655440000","result":{"ok":true,"value":{"revision":0,"revision":1}}}}"#).is_err());
    }

    #[test]
    fn host_wire_ids_are_unicode_scalars() {
        for id in [r#"\ud800"#, r#"\udfff"#] {
            let input =
                format!(r#"{{"requestId":"x","scope":"project","key":"x","projectId":"{id}"}}"#);
            assert!(parse_storage_get_request(input.as_bytes()).is_err());
        }
        assert!(parse_storage_get_request(
            br#"{"requestId":"x","scope":"project","key":"x","projectId":"\ud83d\ude00"}"#
        )
        .is_ok());
    }

    #[test]
    fn host_safe_integer_parsing_accepts_equivalent_json_numbers() {
        for number in ["0", "-0", "0.0", "1", "1.0", "1e0"] {
            let input = format!(
                r#"{{"requestId":"x","scope":"profile","key":"x","expectedRevision":{number}}}"#
            );
            assert!(
                parse_storage_delete_request(input.as_bytes()).is_ok(),
                "{number}"
            );
        }
        for number in ["-1", "0.5", "9007199254740992", "1e30"] {
            let input = format!(
                r#"{{"requestId":"x","scope":"profile","key":"x","expectedRevision":{number}}}"#
            );
            assert!(
                parse_storage_delete_request(input.as_bytes()).is_err(),
                "{number}"
            );
        }
    }

    #[test]
    fn host_quota_numbers_follow_json_stringify_thresholds() {
        for (number, expected) in [
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (-0.0, "0"),
            (100.0, "100"),
            (0.001, "0.001"),
            (1.25, "1.25"),
        ] {
            assert_eq!(
                js_number(&serde_json::Number::from_f64(number).unwrap()),
                expected
            );
        }
        assert_eq!(js_json_bytes(&serde_json::json!(vec![1e20; 3000])), 66001);
    }

    #[test]
    fn host_contract_limits_apply_to_raw_input_and_programmatic_values() {
        assert!(parse_host_ui_get_request(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
        let mut request = StorageSetRequest {
            request_id: "save".into(),
            scope: StorageScope::Profile,
            key: "x".into(),
            project_id: None,
            value: Value::String("x".repeat(MAX_STORAGE_VALUE_BYTES - 2)),
            expected_revision: 0,
        };
        validate_storage_set_request(&request).unwrap();
        request.value = Value::String("x".repeat(MAX_STORAGE_VALUE_BYTES - 1));
        assert!(validate_storage_set_request(&request).is_err());
        let progress = ProgressUpsertRequest {
            id: "job".into(),
            title: "Job".into(),
            state: ProgressState::Running,
            fraction: Some(f64::NAN),
            cancel_command_id: None,
        };
        assert!(validate_progress_upsert_request(&progress).is_err());
    }
}
