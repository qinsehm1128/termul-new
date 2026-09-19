//! Narrow ACP + relay host used by the shared-live web layer.
//!
//! Two implementations:
//! - [`InProcessAcpWebHost`] — today's standalone/fallback path (direct
//!   `AcpManager` + `WsRelaySink` calls).
//! - [`CoreAcpWebHost`] — desktop Core mode (ACP Core IPC). Durable
//!   admission stays inside the Core; the GUI keeps a live-only
//!   [`WsRelaySink`] fed by [`CoreRelayHost`] so `/ws` subscribe/replay
//!   keep the existing `WsRelaySink` API (smaller diff than trait-ifying
//!   every relay call site in `web/ws.rs`).

use super::acp::{
    AcpCoreClient, METHOD_ANSWER_QUESTION, METHOD_AUTHENTICATE, METHOD_CANCEL_PROMPT,
    METHOD_CLOSE_SESSION, METHOD_COMPOSER_CONTROLS, METHOD_CONVERSATION_ID_FOR_SESSION,
    METHOD_DISPOSE_EPHEMERAL_SESSION, METHOD_HISTORY_GET, METHOD_HISTORY_GET_PAGE,
    METHOD_HISTORY_LIST, METHOD_IS_EPHEMERAL_SESSION, METHOD_IS_TURN_ACTIVE,
    METHOD_LIST_RUNNING_NAMESPACES, METHOD_LIST_SESSIONS, METHOD_LOAD_SESSION, METHOD_NEW_SESSION,
    METHOD_OWNS_SESSION, METHOD_REGISTER_CONVERSATION_BINDING, METHOD_REGISTER_DISCOVERED_SESSION,
    METHOD_RESPOND_PERMISSION, METHOD_RESUME_SESSION, METHOD_RETIRE_SESSION, METHOD_SEND_PROMPT,
    METHOD_SET_CONFIG_OPTION, METHOD_SET_MODE, METHOD_SET_MODEL, METHOD_SET_PERMISSION_POLICY,
    METHOD_SPAWN_AGENT, METHOD_STABLE_AGENT_NAMESPACE, METHOD_WAIT_TURN_IDLE,
};
use super::handles::AcpServiceHandle;
use super::ipc::CoreError;
use crate::acp::config::{AgentConfig, AgentId, PermissionPolicy, SessionId};
use crate::acp::manager::{
    NewSessionOutcome, SessionCreationContext, SessionReopenOutcome, SpawnOutcome,
};
use crate::acp::AcpManager;
use crate::conversation::ConversationId;
use crate::web::sink::{AcpEvent, EventSink, WsRelaySink};
use agent_client_protocol::schema::v1::{
    ContentBlock, ListSessionsResponse, McpServer, SessionConfigOption, StopReason,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Stable error when shared-live cannot resolve either an in-process stack
/// or a Core-backed host.
pub const SHARED_LIVE_UNAVAILABLE: &str = "SHARED_LIVE_UNAVAILABLE";

/// Prompt turn that has been accepted and is awaiting completion.
pub struct HostStartedPrompt {
    completion: oneshot::Receiver<Result<StopReason, String>>,
}

/// Manager + relay methods the web layer actually calls.
#[async_trait]
pub trait AcpWebHost: Send + Sync {
    fn in_process_manager(&self) -> Option<Arc<AcpManager>>;
    fn live_relay(&self) -> Arc<WsRelaySink>;
    fn acp_service(&self) -> AcpServiceHandle;
    fn has_persisted_history(&self) -> bool;
    fn scheduled_tasks(&self) -> Option<Arc<crate::scheduled_tasks::ScheduledTaskService>>;
    fn conversation_creation(
        &self,
    ) -> Option<Arc<crate::conversation::ConversationCreationService>>;
    fn pty_manager(&self) -> Option<Arc<crate::pty::PtyManager>>;

    async fn list_agents(&self) -> Vec<AgentId>;
    async fn list_running_namespaces(&self) -> Vec<(String, Option<String>)>;
    async fn set_permission_policy(
        &self,
        agent_id: &AgentId,
        policy: PermissionPolicy,
    ) -> Result<(), String>;
    async fn stable_agent_namespace(&self, agent_id: &AgentId) -> Result<Option<String>, String>;
    async fn conversation_id_for_current_session(&self, session_id: &str)
        -> Option<ConversationId>;
    async fn register_conversation_binding(
        &self,
        session_id: &str,
        conversation_id: ConversationId,
    );

    async fn spawn(&self, config: AgentConfig) -> Result<SpawnOutcome, String>;
    async fn kill(&self, agent_id: &AgentId) -> Result<(), String>;
    async fn authenticate(&self, agent_id: &AgentId, method_id: String) -> Result<(), String>;
    async fn new_session_with_context(
        &self,
        agent_id: &AgentId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        context: SessionCreationContext,
    ) -> Result<NewSessionOutcome, String>;
    async fn load_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String>;
    async fn resume_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String>;
    async fn close_session(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String>;
    async fn dispose_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String>;
    async fn is_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String>;
    async fn list_sessions(
        &self,
        agent_id: &AgentId,
        cwd: Option<String>,
        cursor: Option<String>,
    ) -> Result<ListSessionsResponse, String>;
    async fn composer_controls(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<SessionReopenOutcome, String>;
    async fn cancel_prompt(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String>;
    async fn set_mode(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        mode_id: String,
    ) -> Result<(), String>;
    async fn set_model(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        model_id: String,
    ) -> Result<(), String>;
    async fn set_config_option(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        config_id: String,
        value_id: String,
    ) -> Result<Vec<SessionConfigOption>, String>;
    async fn ensure_session_on_live_agent(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
    ) -> Result<AgentId, String>;
    async fn start_prompt(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
        turn_id: Option<String>,
    ) -> Result<HostStartedPrompt, String>;
    async fn wait_prompt(&self, started: HostStartedPrompt) -> Result<StopReason, String>;
    async fn persist_user_prompt(&self, sid: &str, payload: Value) -> Result<(), String>;
    async fn retire_session(&self, sid: &str) -> Result<(), String>;
    async fn respond_permission(
        &self,
        agent_id: &AgentId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), String>;
    async fn answer_question(
        &self,
        agent_id: &AgentId,
        question_id: String,
        values: Value,
    ) -> Result<(), String>;
    async fn register_discovered_session(
        &self,
        session_id: String,
        agent_id: AgentId,
        cwd: String,
        title: Option<String>,
        updated_at: Option<u64>,
        project_id: Option<String>,
    ) -> Result<Value, String>;
    async fn history_list(&self) -> Result<Value, String>;
    async fn history_get(&self, session_id: &str) -> Result<Value, String>;
    async fn history_get_page(
        &self,
        session_id: &str,
        after_seq: u64,
        limit: usize,
        target_last_seq: Option<u64>,
    ) -> Result<Value, String>;
    async fn owns_session(&self, agent_id: &AgentId, session_id: SessionId)
        -> Result<bool, String>;
    async fn is_turn_active(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String>;
    async fn wait_turn_idle(&self, agent_id: &AgentId, session_id: SessionId)
        -> Result<(), String>;
}

/// Cloneable web-layer handle. `Deref`s to [`AcpWebHost`] so existing
/// `acp.spawn().await` call sites keep working after the type swap.
#[derive(Clone)]
pub struct AcpWebHostHandle {
    inner: Arc<dyn AcpWebHost>,
}

impl AcpWebHostHandle {
    pub fn in_process(manager: Arc<AcpManager>, relay: Arc<WsRelaySink>) -> Self {
        Self {
            inner: Arc::new(InProcessAcpWebHost { manager, relay }),
        }
    }

    pub fn core(client: Arc<AcpCoreClient>, relay: Arc<WsRelaySink>) -> Self {
        Self {
            inner: Arc::new(CoreAcpWebHost { client, relay }),
        }
    }

    pub fn from_arc(inner: Arc<dyn AcpWebHost>) -> Self {
        Self { inner }
    }
}

impl std::ops::Deref for AcpWebHostHandle {
    type Target = dyn AcpWebHost;
    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

/// Direct in-process manager + relay. Byte-for-byte today's fallback.
pub struct InProcessAcpWebHost {
    pub manager: Arc<AcpManager>,
    pub relay: Arc<WsRelaySink>,
}

/// Core-IPC host. Durable writes stay in the Core; `relay` is the GUI-side
/// live fan-out populated by [`CoreRelayHost`].
pub struct CoreAcpWebHost {
    pub client: Arc<AcpCoreClient>,
    pub relay: Arc<WsRelaySink>,
}

/// Subscribes to ACP Core events and admits them into a GUI-side live-only
/// `WsRelaySink` so late WS subscribers get bounded in-memory replay.
pub struct CoreRelayHost;

impl CoreRelayHost {
    pub fn start(client: Arc<AcpCoreClient>, relay: Arc<WsRelaySink>) {
        let mut events = client.subscribe_events();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(envelope) => {
                        let _ = EventSink::emit(
                            &*relay,
                            &AcpEvent {
                                sid: envelope.sid,
                                type_: intern_event_type(&envelope.type_),
                                payload: envelope.data,
                            },
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

fn core_err(error: CoreError) -> String {
    error.command_message()
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn decode_new_session(value: Value) -> Result<NewSessionOutcome, String> {
    let session_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| "newSession response missing sessionId".to_string())?;
    let conversation_id = value
        .get("conversationId")
        .and_then(Value::as_str)
        .and_then(|raw| ConversationId::parse(raw).ok());
    Ok(NewSessionOutcome {
        persistence: "conversation",
        conversation_id,
        workspace_cwd: value
            .get("workspaceCwd")
            .and_then(Value::as_str)
            .map(str::to_string),
        execution_cwd: value
            .get("executionCwd")
            .and_then(Value::as_str)
            .map(str::to_string),
        session_id: SessionId::new(session_id),
        modes: value
            .get("modes")
            .cloned()
            .and_then(|modes| serde_json::from_value(modes).ok()),
        models: value
            .get("models")
            .cloned()
            .and_then(|models| serde_json::from_value(models).ok()),
        config_options: value
            .get("configOptions")
            .cloned()
            .and_then(|options| serde_json::from_value(options).ok()),
    })
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn intern_event_type(value: &str) -> &'static str {
    use crate::acp::events::{
        EVENT_AGENT_CRASHED, EVENT_AGENT_DISCONNECTED, EVENT_AGENT_ERROR, EVENT_AGENT_SPAWNED,
        EVENT_COMMANDS_UPDATE, EVENT_CONFIG_OPTIONS_UPDATE, EVENT_MESSAGE_CHUNK, EVENT_MODE_UPDATE,
        EVENT_PERMISSION_REQUEST, EVENT_PLAN_UPDATE, EVENT_PROMPT_COMPLETE, EVENT_QUESTION_REQUEST,
        EVENT_SESSION_CLOSED, EVENT_SESSION_CREATED, EVENT_SESSION_INFO_UPDATE, EVENT_TOOL_CALL,
        EVENT_TOOL_CALL_UPDATE, EVENT_USAGE_UPDATE,
    };
    match value {
        EVENT_AGENT_SPAWNED => EVENT_AGENT_SPAWNED,
        EVENT_SESSION_CREATED => EVENT_SESSION_CREATED,
        EVENT_MESSAGE_CHUNK => EVENT_MESSAGE_CHUNK,
        EVENT_TOOL_CALL => EVENT_TOOL_CALL,
        EVENT_TOOL_CALL_UPDATE => EVENT_TOOL_CALL_UPDATE,
        EVENT_PLAN_UPDATE => EVENT_PLAN_UPDATE,
        EVENT_COMMANDS_UPDATE => EVENT_COMMANDS_UPDATE,
        EVENT_MODE_UPDATE => EVENT_MODE_UPDATE,
        EVENT_CONFIG_OPTIONS_UPDATE => EVENT_CONFIG_OPTIONS_UPDATE,
        EVENT_PERMISSION_REQUEST => EVENT_PERMISSION_REQUEST,
        EVENT_QUESTION_REQUEST => EVENT_QUESTION_REQUEST,
        EVENT_PROMPT_COMPLETE => EVENT_PROMPT_COMPLETE,
        EVENT_AGENT_ERROR => EVENT_AGENT_ERROR,
        EVENT_AGENT_CRASHED => EVENT_AGENT_CRASHED,
        EVENT_SESSION_CLOSED => EVENT_SESSION_CLOSED,
        EVENT_AGENT_DISCONNECTED => EVENT_AGENT_DISCONNECTED,
        EVENT_SESSION_INFO_UPDATE => EVENT_SESSION_INFO_UPDATE,
        EVENT_USAGE_UPDATE => EVENT_USAGE_UPDATE,
        other => intern_unknown_event_type(other),
    }
}

fn intern_unknown_event_type(value: &str) -> &'static str {
    static INTERN: std::sync::OnceLock<parking_lot::Mutex<HashMap<String, &'static str>>> =
        std::sync::OnceLock::new();
    let mut map = INTERN
        .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
        .lock();
    if let Some(existing) = map.get(value).copied() {
        return existing;
    }
    let leaked: &'static str = Box::leak(value.to_string().into_boxed_str());
    map.insert(value.to_string(), leaked);
    leaked
}

fn json_question_values(values: Value) -> Result<Option<Vec<String>>, String> {
    if values.is_null() {
        Ok(None)
    } else {
        serde_json::from_value(values).map_err(|error| error.to_string())
    }
}

#[async_trait]
impl AcpWebHost for InProcessAcpWebHost {
    fn in_process_manager(&self) -> Option<Arc<AcpManager>> {
        Some(Arc::clone(&self.manager))
    }

    fn live_relay(&self) -> Arc<WsRelaySink> {
        Arc::clone(&self.relay)
    }

    fn acp_service(&self) -> AcpServiceHandle {
        AcpServiceHandle::in_process(Arc::clone(&self.manager))
    }

    fn has_persisted_history(&self) -> bool {
        self.relay.has_persisted_history()
    }

    fn scheduled_tasks(&self) -> Option<Arc<crate::scheduled_tasks::ScheduledTaskService>> {
        self.manager.scheduled_tasks()
    }

    fn conversation_creation(
        &self,
    ) -> Option<Arc<crate::conversation::ConversationCreationService>> {
        self.manager.conversation_creation()
    }

    fn pty_manager(&self) -> Option<Arc<crate::pty::PtyManager>> {
        self.manager.pty_manager()
    }

    async fn list_agents(&self) -> Vec<AgentId> {
        self.manager.list_agents()
    }

    async fn list_running_namespaces(&self) -> Vec<(String, Option<String>)> {
        self.manager.list_running_namespaces()
    }

    async fn set_permission_policy(
        &self,
        agent_id: &AgentId,
        policy: PermissionPolicy,
    ) -> Result<(), String> {
        self.manager.set_permission_policy(agent_id, policy)
    }

    async fn stable_agent_namespace(&self, agent_id: &AgentId) -> Result<Option<String>, String> {
        self.manager.stable_agent_namespace(agent_id)
    }

    async fn conversation_id_for_current_session(
        &self,
        session_id: &str,
    ) -> Option<ConversationId> {
        self.manager.conversation_id_for_current_session(session_id)
    }

    async fn register_conversation_binding(
        &self,
        session_id: &str,
        conversation_id: ConversationId,
    ) {
        self.manager
            .register_conversation_binding(session_id, conversation_id);
    }

    async fn spawn(&self, config: AgentConfig) -> Result<SpawnOutcome, String> {
        self.manager.spawn(config).await
    }

    async fn kill(&self, agent_id: &AgentId) -> Result<(), String> {
        self.manager.kill(agent_id).await
    }

    async fn authenticate(&self, agent_id: &AgentId, method_id: String) -> Result<(), String> {
        self.manager.authenticate(agent_id, method_id).await
    }

    async fn new_session_with_context(
        &self,
        agent_id: &AgentId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        context: SessionCreationContext,
    ) -> Result<NewSessionOutcome, String> {
        self.manager
            .new_session_with_context(agent_id, cwd, mcp_servers, context)
            .await
    }

    async fn load_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        self.manager
            .load_session(agent_id, session_id, cwd, Vec::new(), mcp_servers)
            .await
    }

    async fn resume_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        self.manager
            .resume_session(agent_id, session_id, cwd, Vec::new(), mcp_servers)
            .await
    }

    async fn close_session(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String> {
        self.manager.close_session(agent_id, session_id).await
    }

    async fn dispose_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.manager
            .dispose_ephemeral_session(agent_id, session_id)
            .await
    }

    async fn is_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        self.manager
            .is_ephemeral_session(agent_id, session_id)
            .await
    }

    async fn list_sessions(
        &self,
        agent_id: &AgentId,
        cwd: Option<String>,
        cursor: Option<String>,
    ) -> Result<ListSessionsResponse, String> {
        self.manager.list_sessions(agent_id, cwd, cursor).await
    }

    async fn composer_controls(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<SessionReopenOutcome, String> {
        self.manager.composer_controls(agent_id, session_id).await
    }

    async fn cancel_prompt(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String> {
        self.manager.cancel_prompt(agent_id, session_id).await
    }

    async fn set_mode(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        mode_id: String,
    ) -> Result<(), String> {
        self.manager.set_mode(agent_id, session_id, mode_id).await
    }

    async fn set_model(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        model_id: String,
    ) -> Result<(), String> {
        self.manager.set_model(agent_id, session_id, model_id).await
    }

    async fn set_config_option(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        config_id: String,
        value_id: String,
    ) -> Result<Vec<SessionConfigOption>, String> {
        self.manager
            .set_config_option(agent_id, session_id, config_id, value_id)
            .await
    }

    async fn ensure_session_on_live_agent(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
    ) -> Result<AgentId, String> {
        self.manager
            .ensure_session_on_live_agent(agent_id, session_id)
            .await
    }

    async fn start_prompt(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
        turn_id: Option<String>,
    ) -> Result<HostStartedPrompt, String> {
        let started = self
            .manager
            .start_prompt(agent_id, session_id, content, turn_id)
            .await?;
        Ok(HostStartedPrompt {
            completion: started.into_completion(),
        })
    }

    async fn wait_prompt(&self, started: HostStartedPrompt) -> Result<StopReason, String> {
        started
            .completion
            .await
            .map_err(|_| "agent thread dropped the prompt reply".to_string())?
    }

    async fn persist_user_prompt(&self, sid: &str, payload: Value) -> Result<(), String> {
        self.relay
            .persist_user_prompt(sid, payload)
            .await
            .map(|_| ())
    }

    async fn retire_session(&self, sid: &str) -> Result<(), String> {
        self.relay.retire_session(sid).await
    }

    async fn respond_permission(
        &self,
        agent_id: &AgentId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), String> {
        self.manager
            .respond_permission(agent_id, request_id, option_id)
            .await
    }

    async fn answer_question(
        &self,
        agent_id: &AgentId,
        question_id: String,
        values: Value,
    ) -> Result<(), String> {
        self.manager
            .answer_question(agent_id, question_id, json_question_values(values)?)
            .await
    }

    async fn register_discovered_session(
        &self,
        session_id: String,
        agent_id: AgentId,
        cwd: String,
        title: Option<String>,
        updated_at: Option<u64>,
        project_id: Option<String>,
    ) -> Result<Value, String> {
        let persistence = self
            .manager
            .persistence()
            .ok_or_else(|| "session persistence unavailable".to_string())?;
        let metadata = persistence
            .register_discovered_session(
                crate::acp::SessionRegistration {
                    session_id,
                    stable_agent_namespace: self.manager.stable_agent_namespace(&agent_id)?,
                    runtime_agent_id: Some(agent_id.0),
                    project_id,
                    cwd: cwd.into(),
                    ..Default::default()
                },
                title,
                updated_at,
            )
            .await
            .map_err(|error| error.to_string())?;
        serde_json::to_value(&metadata).map_err(|error| error.to_string())
    }

    async fn history_list(&self) -> Result<Value, String> {
        if let Some(persistence) = self.relay.conversation_persistence() {
            return serde_json::to_value(persistence.list_sessions())
                .map_err(|error| error.to_string());
        }
        let persistence = self
            .relay
            .persistence()
            .ok_or_else(|| "persisted history is unavailable".to_string())?;
        serde_json::to_value(persistence.list_sessions()).map_err(|error| error.to_string())
    }

    async fn history_get(&self, session_id: &str) -> Result<Value, String> {
        let _ = session_id;
        Err("use the in-process relay persistence path".into())
    }

    async fn history_get_page(
        &self,
        _session_id: &str,
        _after_seq: u64,
        _limit: usize,
        _target_last_seq: Option<u64>,
    ) -> Result<Value, String> {
        Err("use the in-process relay persistence path".into())
    }

    async fn owns_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        self.manager.owns_session(agent_id, session_id).await
    }

    async fn is_turn_active(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        self.manager.is_turn_active(agent_id, session_id).await
    }

    async fn wait_turn_idle(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.manager.wait_turn_idle(agent_id, session_id).await
    }
}

#[async_trait]
impl AcpWebHost for CoreAcpWebHost {
    fn in_process_manager(&self) -> Option<Arc<AcpManager>> {
        None
    }

    fn live_relay(&self) -> Arc<WsRelaySink> {
        Arc::clone(&self.relay)
    }

    fn acp_service(&self) -> AcpServiceHandle {
        AcpServiceHandle::from_core_client_arc(Arc::clone(&self.client))
    }

    fn has_persisted_history(&self) -> bool {
        true
    }

    fn scheduled_tasks(&self) -> Option<Arc<crate::scheduled_tasks::ScheduledTaskService>> {
        None
    }

    fn conversation_creation(
        &self,
    ) -> Option<Arc<crate::conversation::ConversationCreationService>> {
        None
    }

    fn pty_manager(&self) -> Option<Arc<crate::pty::PtyManager>> {
        None
    }

    async fn list_agents(&self) -> Vec<AgentId> {
        self.client.list_agents().await.unwrap_or_default()
    }

    async fn list_running_namespaces(&self) -> Vec<(String, Option<String>)> {
        match self
            .client
            .request(METHOD_LIST_RUNNING_NAMESPACES, Value::Null)
            .await
        {
            Ok(value) => decode(value).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    async fn set_permission_policy(
        &self,
        agent_id: &AgentId,
        policy: PermissionPolicy,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_SET_PERMISSION_POLICY,
                json!({ "agentId": agent_id, "policy": policy }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn stable_agent_namespace(&self, agent_id: &AgentId) -> Result<Option<String>, String> {
        let value = self
            .client
            .request(
                METHOD_STABLE_AGENT_NAMESPACE,
                json!({ "agentId": agent_id }),
            )
            .await
            .map_err(core_err)?;
        Ok(value
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    async fn conversation_id_for_current_session(
        &self,
        session_id: &str,
    ) -> Option<ConversationId> {
        let value = self
            .client
            .request(
                METHOD_CONVERSATION_ID_FOR_SESSION,
                json!({ "sessionId": session_id }),
            )
            .await
            .ok()?;
        value
            .get("conversationId")
            .and_then(Value::as_str)
            .and_then(|raw| ConversationId::parse(raw).ok())
    }

    async fn register_conversation_binding(
        &self,
        session_id: &str,
        conversation_id: ConversationId,
    ) {
        let _ = self
            .client
            .request(
                METHOD_REGISTER_CONVERSATION_BINDING,
                json!({
                    "sessionId": session_id,
                    "conversationId": conversation_id,
                }),
            )
            .await;
    }

    async fn spawn(&self, config: AgentConfig) -> Result<SpawnOutcome, String> {
        let value = self
            .client
            .request(
                METHOD_SPAWN_AGENT,
                serde_json::to_value(&config).map_err(|error| error.to_string())?,
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn kill(&self, agent_id: &AgentId) -> Result<(), String> {
        self.client.kill_agent(agent_id).await.map_err(core_err)
    }

    async fn authenticate(&self, agent_id: &AgentId, method_id: String) -> Result<(), String> {
        self.client
            .request(
                METHOD_AUTHENTICATE,
                json!({ "agentId": agent_id, "methodId": method_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn new_session_with_context(
        &self,
        agent_id: &AgentId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
        context: SessionCreationContext,
    ) -> Result<NewSessionOutcome, String> {
        let value = self
            .client
            .request(
                METHOD_NEW_SESSION,
                json!({
                    "agentId": agent_id,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                    "ephemeral": context.ephemeral,
                    "projectId": context.project_id,
                    "worktreePath": context.worktree_path,
                    "worktreeBranch": context.worktree_branch,
                    "conversationId": context.conversation_id,
                    "projectAttachment": context.project_attachment,
                    "executionTarget": context.execution_target,
                }),
            )
            .await
            .map_err(core_err)?;
        decode_new_session(value)
    }

    async fn load_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        let value = self
            .client
            .request(
                METHOD_LOAD_SESSION,
                json!({
                    "agentId": agent_id,
                    "sessionId": session_id,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                }),
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn resume_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        cwd: String,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionReopenOutcome, String> {
        let value = self
            .client
            .request(
                METHOD_RESUME_SESSION,
                json!({
                    "agentId": agent_id,
                    "sessionId": session_id,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                }),
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn close_session(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String> {
        self.client
            .request(
                METHOD_CLOSE_SESSION,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn dispose_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_DISPOSE_EPHEMERAL_SESSION,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn is_ephemeral_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let value = self
            .client
            .request(
                METHOD_IS_EPHEMERAL_SESSION,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map_err(core_err)?;
        Ok(bool_field(&value, "ephemeral"))
    }

    async fn list_sessions(
        &self,
        agent_id: &AgentId,
        cwd: Option<String>,
        cursor: Option<String>,
    ) -> Result<ListSessionsResponse, String> {
        let value = self
            .client
            .request(
                METHOD_LIST_SESSIONS,
                json!({ "agentId": agent_id, "cwd": cwd, "cursor": cursor }),
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn composer_controls(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<SessionReopenOutcome, String> {
        let value = self
            .client
            .request(
                METHOD_COMPOSER_CONTROLS,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn cancel_prompt(&self, agent_id: &AgentId, session_id: SessionId) -> Result<(), String> {
        self.client
            .request(
                METHOD_CANCEL_PROMPT,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn set_mode(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        mode_id: String,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_SET_MODE,
                json!({ "agentId": agent_id, "sessionId": session_id, "modeId": mode_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn set_model(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        model_id: String,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_SET_MODEL,
                json!({ "agentId": agent_id, "sessionId": session_id, "modelId": model_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn set_config_option(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        config_id: String,
        value_id: String,
    ) -> Result<Vec<SessionConfigOption>, String> {
        let value = self
            .client
            .request(
                METHOD_SET_CONFIG_OPTION,
                json!({
                    "agentId": agent_id,
                    "sessionId": session_id,
                    "configId": config_id,
                    "valueId": value_id,
                }),
            )
            .await
            .map_err(core_err)?;
        decode(value)
    }

    async fn ensure_session_on_live_agent(
        &self,
        agent_id: &AgentId,
        session_id: &SessionId,
    ) -> Result<AgentId, String> {
        let owns = self.owns_session(agent_id, session_id.clone()).await?;
        if owns {
            Ok(agent_id.clone())
        } else {
            Err("session does not belong to the supplied live agent".into())
        }
    }

    async fn start_prompt(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
        content: Vec<ContentBlock>,
        turn_id: Option<String>,
    ) -> Result<HostStartedPrompt, String> {
        let client = Arc::clone(&self.client);
        let agent_id = agent_id.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = async {
                let value = client
                    .request(
                        METHOD_SEND_PROMPT,
                        json!({
                            "agentId": agent_id,
                            "sessionId": session_id,
                            "content": content,
                            "turnId": turn_id,
                        }),
                    )
                    .await
                    .map_err(core_err)?;
                decode::<StopReason>(value)
            }
            .await;
            let _ = tx.send(result);
        });
        Ok(HostStartedPrompt { completion: rx })
    }

    async fn wait_prompt(&self, started: HostStartedPrompt) -> Result<StopReason, String> {
        started
            .completion
            .await
            .map_err(|_| "acp core dropped the prompt reply".to_string())?
    }

    async fn persist_user_prompt(&self, sid: &str, payload: Value) -> Result<(), String> {
        // Live fan-out on the GUI-side relay so phone WS subscribers see
        // `user_prompt` immediately. Durable admission stays inside Core
        // `sendPrompt` (this relay is live-only in Core mode).
        self.relay
            .persist_user_prompt(sid, payload)
            .await
            .map(|_| ())
    }

    async fn retire_session(&self, sid: &str) -> Result<(), String> {
        self.client
            .request(METHOD_RETIRE_SESSION, json!({ "sessionId": sid }))
            .await
            .map(|_| ())
            .map_err(core_err)?;
        let _ = self.relay.retire_session(sid).await;
        Ok(())
    }

    async fn respond_permission(
        &self,
        agent_id: &AgentId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_RESPOND_PERMISSION,
                json!({
                    "agentId": agent_id,
                    "requestId": request_id,
                    "optionId": option_id,
                }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn answer_question(
        &self,
        agent_id: &AgentId,
        question_id: String,
        values: Value,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_ANSWER_QUESTION,
                json!({
                    "agentId": agent_id,
                    "questionId": question_id,
                    "values": values,
                }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }

    async fn register_discovered_session(
        &self,
        session_id: String,
        agent_id: AgentId,
        cwd: String,
        title: Option<String>,
        updated_at: Option<u64>,
        project_id: Option<String>,
    ) -> Result<Value, String> {
        self.client
            .request(
                METHOD_REGISTER_DISCOVERED_SESSION,
                json!({
                    "sessionId": session_id,
                    "agentId": agent_id,
                    "cwd": cwd,
                    "title": title,
                    "updatedAt": updated_at,
                    "projectId": project_id,
                }),
            )
            .await
            .map_err(core_err)
    }

    async fn history_list(&self) -> Result<Value, String> {
        self.client
            .request(METHOD_HISTORY_LIST, Value::Null)
            .await
            .map_err(core_err)
    }

    async fn history_get(&self, session_id: &str) -> Result<Value, String> {
        self.client
            .request(METHOD_HISTORY_GET, json!({ "sessionId": session_id }))
            .await
            .map_err(core_err)
    }

    async fn history_get_page(
        &self,
        session_id: &str,
        after_seq: u64,
        limit: usize,
        target_last_seq: Option<u64>,
    ) -> Result<Value, String> {
        self.client
            .request(
                METHOD_HISTORY_GET_PAGE,
                json!({
                    "sessionId": session_id,
                    "afterSeq": after_seq,
                    "limit": limit,
                    "targetLastSeq": target_last_seq,
                }),
            )
            .await
            .map_err(core_err)
    }

    async fn owns_session(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let value = self
            .client
            .request(
                METHOD_OWNS_SESSION,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map_err(core_err)?;
        Ok(bool_field(&value, "owns"))
    }

    async fn is_turn_active(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<bool, String> {
        let value = self
            .client
            .request(
                METHOD_IS_TURN_ACTIVE,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map_err(core_err)?;
        Ok(bool_field(&value, "active"))
    }

    async fn wait_turn_idle(
        &self,
        agent_id: &AgentId,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.client
            .request(
                METHOD_WAIT_TURN_IDLE,
                json!({ "agentId": agent_id, "sessionId": session_id }),
            )
            .await
            .map(|_| ())
            .map_err(core_err)
    }
}

/// Build a shared-live ACP host from the desktop service handle.
///
/// Core mode uses the Core client + a GUI-side live relay (started with
/// [`CoreRelayHost`] if the caller has not already done so). Fallback uses
/// the in-process manager. Returns [`SHARED_LIVE_UNAVAILABLE`] when neither
/// is present.
pub fn resolve_acp_web_host(
    acp: &AcpServiceHandle,
    relay: Option<Arc<WsRelaySink>>,
) -> Result<AcpWebHostHandle, String> {
    if let Some(client) = acp.core_client() {
        let relay = match relay {
            Some(relay) => relay,
            None => {
                let relay = Arc::new(WsRelaySink::new());
                CoreRelayHost::start(Arc::clone(&client), Arc::clone(&relay));
                relay
            }
        };
        return Ok(AcpWebHostHandle::core(client, relay));
    }
    let manager = acp.in_process_manager().ok_or_else(|| {
        format!("{SHARED_LIVE_UNAVAILABLE}: neither ACP Core nor in-process manager is available")
    })?;
    let relay = relay.ok_or_else(|| {
        format!("{SHARED_LIVE_UNAVAILABLE}: in-process shared-live requires a WS relay")
    })?;
    Ok(AcpWebHostHandle::in_process(manager, relay))
}
