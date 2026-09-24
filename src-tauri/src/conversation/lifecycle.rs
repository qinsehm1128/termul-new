//! Explicit Conversation binding and delete lifecycle coordination.
//!
//! Renderer view close is deliberately absent from this host service. Every mutation compares the
//! caller's expected revision with canonical `ConversationRecordV2.lastSeq` while holding the
//! repository's per-Conversation lock. Explicit delete is a durable Saga: capture scope → observe
//! → terminate pending → termination confirmed → purge pending → completed. Phase transitions are
//! journaled before/after remote calls so ACP restart resumes the same deterministic operation ID.
//! Unknown, unavailable, mismatched, or still live terminals block delete and are not retried until
//! Terminal Core is observable; `AlreadyGone` is successful cleanup. Successful delete physically
//! removes the Conversation — it is not archived.
//!
//! Title, attach, detach, target, and binding mutations consume the repository's in-lock
//! canonical sequence allocator (`append_event` / `append_event_locked`) and never open a second
//! ordering lane. A reserved relay cursor that races one of these writers must reconcile.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::acp::AcpManager;
use crate::conversation::contracts::{
    AgentSessionBinding, AgentSessionBindingState, ConversationErrorCode, ConversationId,
    ConversationLifecycleState, ConversationRecordV2, ExecutionTarget,
    AGENT_SESSION_BINDING_SCHEMA_VERSION,
};
use crate::conversation::creation::{
    AgentBindingResult, ConversationCreationService, PrepareConversationRequest,
    PreparedConversation,
};
use crate::conversation::lifecycle_journal::{
    deterministic_operation_id, deterministic_recreate_operation_id, DurableTerminalCwdSource,
    DurableTerminalSpawnIntentV1, LifecycleJournalError, LifecycleJournalErrorCode,
    LifecycleOperationJournal, LifecycleOperationKind, LifecycleOperationPhase,
    LifecycleOperationRecordV1, LifecycleOperationStatus, TerminalEnvironmentPolicyV1,
    TerminalKindPolicyV1, TerminalProgramPolicyV1, TerminalRecoveryState,
    DURABLE_TERMINAL_SPAWN_INTENT_SCHEMA_VERSION,
};
use crate::conversation::repository::{ConversationRepository, RepositoryError};
use crate::conversation::session_workspace::{
    SessionWorkspaceResourceDescriptor, SessionWorkspaceService, SessionWorkspaceV1,
    SESSION_WORKSPACE_SCHEMA_VERSION,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};
use crate::pty::manager::{TerminalCwdSource, TerminalSpawnIntentV1};
use crate::pty::PtyManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationLifecycleAction {
    DetachBinding,
    RebindDetachedBinding,
    SuspendBinding,
    ReplaceBinding,
    DeleteConversation,
    RecreateTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConversationDeleteBlocker {
    LiveBinding { count: usize, ids: Vec<String> },
    TerminalResources { count: usize, ids: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConversationLifecycleOutcome {
    Updated {
        action: ConversationLifecycleAction,
        conversation_id: ConversationId,
        previous_revision: u64,
        revision: u64,
        workspace_cwd: String,
        lifecycle_state: ConversationLifecycleState,
        current_binding: Option<AgentSessionBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_agent_session_id: Option<String>,
    },
    Blocked {
        action: ConversationLifecycleAction,
        conversation_id: ConversationId,
        revision: u64,
        code: ConversationLifecycleErrorCode,
        blockers: Vec<ConversationDeleteBlocker>,
    },
    TerminalRecovered {
        action: ConversationLifecycleAction,
        conversation_id: ConversationId,
        previous_terminal_id: String,
        replacement_terminal_id: String,
        state: TerminalRecoveryState,
        revision: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConversationLifecycleErrorCode {
    ConversationConflict,
    ConversationNotFound,
    ConversationBindingNotFound,
    ConversationBindingNotActive,
    ConversationBindingNotDetached,
    ConversationBindingNotAddressable,
    ConversationLiveResources,
    ConversationRecoveryRequired,
    ConversationDurabilityFailed,
    LegacyCompatibilityReadOnly,
    AcpCloseUnsupported,
    AcpCloseFailed,
    AcpReplaceFailed,
    AcpCompensationFailed,
    ValidationError,
}

impl ConversationLifecycleErrorCode {
    #[must_use]
    pub fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
    }
}

#[derive(Debug)]
pub struct ConversationLifecycleError {
    pub code: ConversationLifecycleErrorCode,
    pub operation: &'static str,
    pub conversation_id: Option<ConversationId>,
    pub detail: String,
}

impl fmt::Display for ConversationLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for ConversationLifecycleError {}

pub type Result<T> = std::result::Result<T, ConversationLifecycleError>;

type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLifecycleProviderErrorKind {
    Unsupported,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AgentLifecycleProviderError {
    pub kind: AgentLifecycleProviderErrorKind,
    pub detail: String,
}

pub trait ConversationAgentLifecycle: Send + Sync {
    fn owns_session<'a>(&'a self, binding: &'a AgentSessionBinding) -> ProviderFuture<'a, bool>;
    fn suspend<'a>(
        &'a self,
        binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>>;
    /// Open the replacement session.
    ///
    /// `target_runtime_agent_id` selects WHICH live agent gets the new session.
    /// `None` keeps the previous binding's agent (a plain restart); `Some` moves
    /// the Conversation onto a different agent. The Conversation's identity,
    /// directory and transcript are unaffected either way — only the binding
    /// changes, which is exactly what `AgentSessionBindingState::Replaced` is for.
    fn replace<'a>(
        &'a self,
        previous_binding: &'a AgentSessionBinding,
        prepared: &'a PreparedConversation,
        target_runtime_agent_id: Option<&'a str>,
    ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>;
    fn abort_replacement<'a>(
        &'a self,
        binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>>;
    fn register_binding(&self, agent_session_id: &str, conversation_id: ConversationId);
}

impl ConversationAgentLifecycle for AcpManager {
    fn owns_session<'a>(&'a self, binding: &'a AgentSessionBinding) -> ProviderFuture<'a, bool> {
        Box::pin(async move {
            self.owns_session(
                &crate::acp::AgentId(binding.runtime_agent_id.clone()),
                crate::acp::SessionId::new(binding.agent_session_id.clone()),
            )
            .await
            .unwrap_or(false)
        })
    }

    fn suspend<'a>(
        &'a self,
        binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async move {
            self.close_conversation_session(binding)
                .await
                .map_err(|detail| AgentLifecycleProviderError {
                    kind: if detail.contains("does not support session/close") {
                        AgentLifecycleProviderErrorKind::Unsupported
                    } else {
                        AgentLifecycleProviderErrorKind::Failed
                    },
                    detail,
                })
        })
    }

    fn replace<'a>(
        &'a self,
        previous_binding: &'a AgentSessionBinding,
        prepared: &'a PreparedConversation,
        target_runtime_agent_id: Option<&'a str>,
    ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
    {
        Box::pin(async move {
            self.create_replacement_session(previous_binding, prepared, target_runtime_agent_id)
                .await
                .map_err(|detail| AgentLifecycleProviderError {
                    kind: AgentLifecycleProviderErrorKind::Failed,
                    detail,
                })
        })
    }

    fn abort_replacement<'a>(
        &'a self,
        binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async move {
            self.abort_replacement_session(binding)
                .await
                .map_err(|detail| AgentLifecycleProviderError {
                    kind: if detail.contains("does not support session/close") {
                        AgentLifecycleProviderErrorKind::Unsupported
                    } else {
                        AgentLifecycleProviderErrorKind::Failed
                    },
                    detail,
                })
        })
    }

    fn register_binding(&self, agent_session_id: &str, conversation_id: ConversationId) {
        self.register_conversation_binding(agent_session_id, conversation_id);
        self.commit_replacement_session(agent_session_id);
    }
}

pub trait TerminalResourceInspector: Send + Sync {
    fn is_live(&self, terminal_id: &str) -> bool;
    /// False when this inspector cannot see the process that owns PTYs.
    /// Conversation delete/suspend must then treat listed terminal resources
    /// as live instead of claiming they are gone.
    fn observes_live_terminals(&self) -> bool {
        true
    }
    fn observe_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_ids: &'a [String],
    ) -> ProviderFuture<'a, std::result::Result<Vec<String>, String>> {
        Box::pin(async move {
            if !self.observes_live_terminals() {
                return Err("terminal liveness observation is unavailable".to_string());
            }
            let _ = conversation_id;
            Ok(terminal_ids
                .iter()
                .filter(|terminal_id| self.is_live(terminal_id))
                .cloned()
                .collect())
        })
    }
    fn terminate_for_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_id: &'a str,
        operation_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let _ = (conversation_id, operation_id);
            self.terminate(terminal_id).await
        })
    }
    fn terminate<'a>(
        &'a self,
        terminal_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let _ = terminal_id;
            Err("terminal terminate is not available".to_string())
        })
    }
    fn spawn_for_conversation<'a>(
        &'a self,
        intent: TerminalSpawnIntentV1,
        conversation: &'a ConversationRecordV2,
    ) -> ProviderFuture<'a, std::result::Result<String, String>> {
        Box::pin(async move {
            let _ = (intent, conversation);
            Err("conversation terminal spawn is not available".to_string())
        })
    }
}

impl TerminalResourceInspector for PtyManager {
    fn is_live(&self, terminal_id: &str) -> bool {
        self.get(terminal_id).is_some()
    }

    fn observe_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_ids: &'a [String],
    ) -> ProviderFuture<'a, std::result::Result<Vec<String>, String>> {
        Box::pin(async move {
            if terminal_ids.is_empty() {
                return Ok(self
                    .get_all()
                    .into_iter()
                    .filter(|instance| {
                        instance.workspace_ref_tracked
                            && instance.conversation_matches(conversation_id)
                            && instance.is_active()
                    })
                    .map(|instance| instance.id.clone())
                    .collect());
            }
            let mut live = Vec::new();
            for terminal_id in terminal_ids {
                let Some(instance) = self.get(terminal_id) else {
                    continue;
                };
                if !instance.workspace_ref_tracked
                    || !instance.conversation_matches(conversation_id)
                {
                    return Err(format!("terminal scope mismatch for {terminal_id}"));
                }
                if instance.is_active() {
                    live.push(terminal_id.clone());
                }
            }
            Ok(live)
        })
    }

    fn terminate_for_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_id: &'a str,
        operation_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let Some(instance) = self.get(terminal_id) else {
                return Ok(());
            };
            if !instance.workspace_ref_tracked || !instance.conversation_matches(conversation_id) {
                return Err(format!("terminal scope mismatch for {terminal_id}"));
            }
            let _ = operation_id;
            self.terminate(terminal_id)
                .await
                .map(|_| ())
                .map_err(|failure| {
                    format!(
                        "terminal_id={} cleanup_stage={}",
                        failure.terminal_id,
                        failure.stage.as_str()
                    )
                })
        })
    }

    fn terminate<'a>(
        &'a self,
        terminal_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            self.terminate(terminal_id)
                .await
                .map(|_| ())
                .map_err(|failure| {
                    format!(
                        "terminal_id={} cleanup_stage={}",
                        failure.terminal_id,
                        failure.stage.as_str()
                    )
                })
        })
    }

    fn spawn_for_conversation<'a>(
        &'a self,
        intent: TerminalSpawnIntentV1,
        conversation: &'a ConversationRecordV2,
    ) -> ProviderFuture<'a, std::result::Result<String, String>> {
        Box::pin(async move {
            PtyManager::spawn_for_conversation(self, intent, conversation, None)
                .await
                .map(|spawned| spawned.info.id)
        })
    }
}

impl TerminalResourceInspector for crate::core::TerminalServiceHandle {
    fn is_live(&self, terminal_id: &str) -> bool {
        self.runtime().is_live(terminal_id)
    }

    fn observes_live_terminals(&self) -> bool {
        self.runtime().observes_live_terminals()
    }

    fn observe_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_ids: &'a [String],
    ) -> ProviderFuture<'a, std::result::Result<Vec<String>, String>> {
        Box::pin(async move {
            self.runtime()
                .observe_conversation(conversation_id, terminal_ids)
                .await
                .map(|observation| observation.live_terminal_ids)
                .map_err(|error| error.to_string())
        })
    }

    fn terminate_for_conversation<'a>(
        &'a self,
        conversation_id: ConversationId,
        terminal_id: &'a str,
        operation_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            self.runtime()
                .terminate_for_conversation(conversation_id, terminal_id, operation_id)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn terminate<'a>(
        &'a self,
        terminal_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            self.runtime()
                .terminate(terminal_id)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn spawn_for_conversation<'a>(
        &'a self,
        intent: TerminalSpawnIntentV1,
        conversation: &'a ConversationRecordV2,
    ) -> ProviderFuture<'a, std::result::Result<String, String>> {
        Box::pin(async move {
            self.runtime()
                .spawn_for_conversation(intent, conversation)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

#[derive(Clone)]
pub struct ConversationLifecycleService {
    writer: Arc<ConversationWriter>,
    repository: Arc<ConversationRepository>,
    creation: Arc<ConversationCreationService>,
    provider: Arc<dyn ConversationAgentLifecycle>,
    terminals: Arc<dyn TerminalResourceInspector>,
    journal: Option<Arc<LifecycleOperationJournal>>,
}

impl ConversationLifecycleService {
    #[must_use]
    pub fn new(
        writer: Arc<ConversationWriter>,
        creation: Arc<ConversationCreationService>,
        provider: Arc<dyn ConversationAgentLifecycle>,
        terminals: Arc<dyn TerminalResourceInspector>,
    ) -> Self {
        let repository = Arc::clone(writer.repository());
        Self {
            writer,
            repository,
            creation,
            provider,
            terminals,
            journal: None,
        }
    }

    /// Attach the ACP-local operation journal used by resumable delete.
    ///
    /// Canonical Conversation records remain the business authority; the journal
    /// only records in-flight phase transitions. Composition sites that own a
    /// profile/state root should attach it so ACP restart can resume.
    #[must_use]
    pub fn with_journal(mut self, journal: Arc<LifecycleOperationJournal>) -> Self {
        self.journal = Some(journal);
        self
    }

    pub fn from_manager(acp: Arc<AcpManager>, pty: Arc<PtyManager>) -> Result<Self> {
        let creation = acp.conversation_creation().ok_or_else(|| {
            lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "construct",
                None,
                "bootstrap-published ConversationCreationService is unavailable",
            )
        })?;
        Ok(Self::new(Arc::clone(creation.writer()), creation, acp, pty))
    }

    pub fn from_terminal(
        acp: Arc<AcpManager>,
        terminal: crate::core::TerminalServiceHandle,
    ) -> Result<Self> {
        let creation = acp.conversation_creation().ok_or_else(|| {
            lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "construct",
                None,
                "bootstrap-published ConversationCreationService is unavailable",
            )
        })?;
        Ok(Self::new(
            Arc::clone(creation.writer()),
            creation,
            acp,
            Arc::new(terminal),
        ))
    }

    /// Recreate Conversation terminals whose PTYs are no longer observable.
    /// Host-owned spawn intent is journaled; claims/env/raw cwd are never persisted.
    pub async fn recover_lost_conversation_terminals(
        &self,
    ) -> Result<Vec<ConversationLifecycleOutcome>> {
        if !self.terminals.observes_live_terminals() {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationLiveResources,
                "recreate_terminal",
                None,
                "ACP Core cannot observe conversation terminals; recreation is fail-closed until Terminal Core is linked",
            ));
        }
        let mut outcomes = Vec::new();
        for conversation in self.repository.list_conversations() {
            if conversation.lifecycle_state == ConversationLifecycleState::Deleted {
                continue;
            }
            let terminal_ids = match self.terminal_resource_ids(conversation.conversation_id) {
                Ok(ids) => ids,
                Err(error) => {
                    log::warn!(
                        "[conversation-lifecycle] operation=recreate_terminal stable_code={} conversation_id={} detail={}",
                        error.code.as_str(),
                        conversation.conversation_id,
                        error.detail
                    );
                    continue;
                }
            };
            for terminal_id in terminal_ids {
                match self
                    .recover_lost_terminal(conversation.conversation_id, &terminal_id)
                    .await
                {
                    Ok(outcome) => outcomes.push(outcome),
                    Err(error) => {
                        log::warn!(
                            "[conversation-lifecycle] operation=recreate_terminal stable_code={} conversation_id={} terminal_id={} detail={}",
                            error.code.as_str(),
                            conversation.conversation_id,
                            terminal_id,
                            error.detail
                        );
                    }
                }
            }
        }
        Ok(outcomes)
    }

    pub async fn recover_lost_terminal(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
    ) -> Result<ConversationLifecycleOutcome> {
        if terminal_id.trim().is_empty() {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ValidationError,
                "recreate_terminal",
                Some(conversation_id),
                "terminal id must not be blank",
            ));
        }
        if !self.terminals.observes_live_terminals() {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationLiveResources,
                "recreate_terminal",
                Some(conversation_id),
                "ACP Core cannot observe conversation terminals; recreation is fail-closed until Terminal Core is linked",
            ));
        }
        let conversation = {
            let _guard = self.repository.lifecycle_lock(conversation_id).await;
            self.repository
                .get_conversation(conversation_id)
                .map_err(map_repository_error)?
        };
        if conversation.lifecycle_state == ConversationLifecycleState::Deleted {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationNotFound,
                "recreate_terminal",
                Some(conversation_id),
                "deleted Conversation cannot recreate terminals",
            ));
        }

        let operation_id = deterministic_recreate_operation_id(conversation_id, terminal_id);
        let mut record = match self.load_recreate_journal(operation_id)? {
            Some(existing) => existing,
            None => {
                let intent = host_owned_spawn_intent(&conversation)?;
                let started = LifecycleOperationRecordV1::start_recreate(
                    conversation_id,
                    conversation.last_seq,
                    terminal_id.to_string(),
                    intent,
                    Utc::now(),
                )
                .map_err(map_journal_error)?;
                self.persist_journal(&started)?;
                started
            }
        };
        if record.kind != LifecycleOperationKind::RecreateTerminal
            || record.conversation_id != conversation_id
        {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "recreate_terminal",
                Some(conversation_id),
                "lifecycle journal recreate record is out of scope",
            ));
        }
        record.bump_attempt(Utc::now());
        self.persist_journal(&record)?;

        if record.phase == LifecycleOperationPhase::Completed
            && record.terminal_recovery_state == Some(TerminalRecoveryState::Recreated)
        {
            let replacement = record.replacement_terminal_id.clone().ok_or_else(|| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                    "recreate_terminal",
                    Some(conversation_id),
                    "completed recreated journal is missing replacement terminal id",
                )
            })?;
            return Ok(terminal_recovered_outcome(
                conversation_id,
                terminal_id,
                &replacement,
                TerminalRecoveryState::Recreated,
                conversation.last_seq,
            ));
        }
        if record.phase == LifecycleOperationPhase::Completed
            && record.terminal_recovery_state == Some(TerminalRecoveryState::Active)
        {
            self.advance_recreate(
                &mut record,
                LifecycleOperationPhase::Observe,
                LifecycleOperationStatus::InFlight,
                TerminalRecoveryState::Active,
                None,
            )?;
        }

        let intent = match &record.spawn_intent {
            Some(intent) => intent.clone(),
            None => {
                let intent = host_owned_spawn_intent(&conversation)?;
                record.spawn_intent = Some(intent.clone());
                self.persist_journal(&record)?;
                intent
            }
        };
        if intent.conversation_id != conversation_id {
            self.fail_closed_recreate(
                &mut record,
                "spawn intent conversation does not match the recovered Conversation",
            )?;
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ValidationError,
                "recreate_terminal",
                Some(conversation_id),
                "spawn intent conversation does not match the recovered Conversation",
            ));
        }

        if record.phase == LifecycleOperationPhase::Recreated
            || (record.phase == LifecycleOperationPhase::Completed
                && record.terminal_recovery_state == Some(TerminalRecoveryState::Recreated))
        {
            let replacement_id = record.replacement_terminal_id.clone().ok_or_else(|| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                    "recreate_terminal",
                    Some(conversation_id),
                    "recreated journal is missing replacement terminal id",
                )
            })?;
            SessionWorkspaceService::new(Arc::clone(&self.writer))
                .replace_terminal_ref(conversation_id, terminal_id, &replacement_id)
                .await
                .map_err(|source| {
                    lifecycle_error(
                        ConversationLifecycleErrorCode::ConversationDurabilityFailed,
                        "recreate_terminal",
                        Some(conversation_id),
                        source.detail,
                    )
                })?;
            self.advance_recreate(
                &mut record,
                LifecycleOperationPhase::Completed,
                LifecycleOperationStatus::Completed,
                TerminalRecoveryState::Recreated,
                None,
            )?;
            log::info!(
                "[conversation-lifecycle] operation=recreate_terminal stable_code=RECREATED conversation_id={} previous_terminal_id={} replacement_terminal_id={}",
                conversation_id,
                terminal_id,
                replacement_id
            );
            return Ok(terminal_recovered_outcome(
                conversation_id,
                terminal_id,
                &replacement_id,
                TerminalRecoveryState::Recreated,
                conversation.last_seq,
            ));
        }

        if record.phase <= LifecycleOperationPhase::Observe {
            self.advance_recreate(
                &mut record,
                LifecycleOperationPhase::Observe,
                LifecycleOperationStatus::InFlight,
                TerminalRecoveryState::Active,
                None,
            )?;
        }

        let live = match self
            .terminals
            .observe_conversation(conversation_id, &[terminal_id.to_string()])
            .await
        {
            Ok(live) => live,
            Err(error) => {
                let code = terminal_blocker_code(&error);
                self.advance_recreate(
                    &mut record,
                    LifecycleOperationPhase::Observe,
                    LifecycleOperationStatus::FailedClosed,
                    TerminalRecoveryState::Lost,
                    Some(code.to_string()),
                )?;
                return Err(lifecycle_error(
                    if code == "TERMINAL_OWNERSHIP_MISMATCH" {
                        ConversationLifecycleErrorCode::ValidationError
                    } else {
                        ConversationLifecycleErrorCode::ConversationLiveResources
                    },
                    "recreate_terminal",
                    Some(conversation_id),
                    format!("terminal observation failed before recreate: {error}"),
                ));
            }
        };
        if live.iter().any(|id| id == terminal_id) {
            self.advance_recreate(
                &mut record,
                LifecycleOperationPhase::Completed,
                LifecycleOperationStatus::Completed,
                TerminalRecoveryState::Active,
                None,
            )?;
            log::info!(
                "[conversation-lifecycle] operation=recreate_terminal stable_code=ACTIVE conversation_id={} terminal_id={}",
                conversation_id,
                terminal_id
            );
            return Ok(terminal_recovered_outcome(
                conversation_id,
                terminal_id,
                terminal_id,
                TerminalRecoveryState::Active,
                conversation.last_seq,
            ));
        }

        self.advance_recreate(
            &mut record,
            LifecycleOperationPhase::Observe,
            LifecycleOperationStatus::NeedsRecovery,
            TerminalRecoveryState::Lost,
            None,
        )?;
        self.advance_recreate(
            &mut record,
            LifecycleOperationPhase::RecreatePending,
            LifecycleOperationStatus::InFlight,
            TerminalRecoveryState::Recreating,
            None,
        )?;

        let replacement_id = if let Some(existing) = record.replacement_terminal_id.clone() {
            existing
        } else {
            match self
                .adopt_or_spawn_replacement(conversation_id, terminal_id, &intent, &conversation)
                .await
            {
                Ok(replacement_id) => replacement_id,
                Err(error) => {
                    let code = terminal_blocker_code(&error.detail);
                    self.advance_recreate(
                        &mut record,
                        LifecycleOperationPhase::RecreatePending,
                        LifecycleOperationStatus::NeedsRecovery,
                        TerminalRecoveryState::Recreating,
                        Some(code.to_string()),
                    )?;
                    return Err(error);
                }
            }
        };
        record.replacement_terminal_id = Some(replacement_id.clone());
        self.advance_recreate(
            &mut record,
            LifecycleOperationPhase::Recreated,
            LifecycleOperationStatus::InFlight,
            TerminalRecoveryState::Recreated,
            None,
        )?;

        SessionWorkspaceService::new(Arc::clone(&self.writer))
            .replace_terminal_ref(conversation_id, terminal_id, &replacement_id)
            .await
            .map_err(|source| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ConversationDurabilityFailed,
                    "recreate_terminal",
                    Some(conversation_id),
                    source.detail,
                )
            })?;
        self.advance_recreate(
            &mut record,
            LifecycleOperationPhase::Completed,
            LifecycleOperationStatus::Completed,
            TerminalRecoveryState::Recreated,
            None,
        )?;
        log::info!(
            "[conversation-lifecycle] operation=recreate_terminal stable_code=RECREATED conversation_id={} previous_terminal_id={} replacement_terminal_id={}",
            conversation_id,
            terminal_id,
            replacement_id
        );
        Ok(terminal_recovered_outcome(
            conversation_id,
            terminal_id,
            &replacement_id,
            TerminalRecoveryState::Recreated,
            conversation.last_seq,
        ))
    }

    pub async fn detach_agent_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::BindingDetach)
            .map_err(map_repository_error)?;
        let _guard = self.repository.lifecycle_lock(conversation_id).await;
        let record = self.expected(conversation_id, expected_revision, "detach_binding")?;
        self.repository
            .detach_agent_binding_locked(&permit, conversation_id, Utc::now())
            .map_err(map_repository_error)?;
        self.repository
            .refresh_lifecycle_catalog(&permit, conversation_id)
            .await
            .map_err(map_repository_error)?;
        self.updated(ConversationLifecycleAction::DetachBinding, &record, None)
    }

    pub async fn rebind_detached_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::BindingRebind)
            .map_err(map_repository_error)?;
        let _guard = self.repository.lifecycle_lock(conversation_id).await;
        let record = self.expected(conversation_id, expected_revision, "rebind_binding")?;
        let binding = self.current_binding(conversation_id, "rebind_binding")?;
        if binding.state != AgentSessionBindingState::Detached {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationBindingNotDetached,
                "rebind_binding",
                Some(conversation_id),
                "rebind requires the current detached binding",
            ));
        }
        if !self.provider.owns_session(&binding).await {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationBindingNotAddressable,
                "rebind_binding",
                Some(conversation_id),
                "the detached opaque session is no longer addressable by its original agent",
            ));
        }
        self.repository
            .rebind_detached_binding_locked(&permit, conversation_id, Utc::now())
            .map_err(map_repository_error)?;
        self.repository
            .refresh_lifecycle_catalog(&permit, conversation_id)
            .await
            .map_err(map_repository_error)?;
        self.updated(
            ConversationLifecycleAction::RebindDetachedBinding,
            &record,
            None,
        )
    }

    pub async fn suspend_agent_binding(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::BindingSuspend)
            .map_err(map_repository_error)?;
        let _guard = self.repository.lifecycle_lock(conversation_id).await;
        let record = self.expected(conversation_id, expected_revision, "suspend_binding")?;
        let binding = self.current_binding(conversation_id, "suspend_binding")?;
        if binding.state != AgentSessionBindingState::Active {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationBindingNotActive,
                "suspend_binding",
                Some(conversation_id),
                "suspend requires the current active binding",
            ));
        }
        self.observe_terminal_resource_ids(conversation_id, "suspend_binding")
            .await?;
        if let Err(source) = self.provider.suspend(&binding).await {
            let code = match source.kind {
                AgentLifecycleProviderErrorKind::Unsupported => {
                    ConversationLifecycleErrorCode::AcpCloseUnsupported
                }
                AgentLifecycleProviderErrorKind::Failed => {
                    ConversationLifecycleErrorCode::AcpCloseFailed
                }
            };
            log::warn!(
                "[conversation-lifecycle] suspend rejected conversation_id={} code={}",
                conversation_id,
                code.as_str()
            );
            return Err(lifecycle_error(
                code,
                "suspend_binding",
                Some(conversation_id),
                source.detail,
            ));
        }
        if let Err(error) =
            self.repository
                .suspend_agent_binding_locked(&permit, conversation_id, Utc::now())
        {
            let primary_code = error.stable_code();
            let compensation = self.creation.record_agent_compensation_failure_locked(
                conversation_id,
                &primary_code,
                None,
                None,
                Some(binding.binding_id),
            );
            let code = if compensation.has_secondary_failure() {
                ConversationLifecycleErrorCode::AcpCompensationFailed
            } else {
                ConversationLifecycleErrorCode::ConversationRecoveryRequired
            };
            return Err(lifecycle_error(
                code,
                "suspend_binding",
                Some(conversation_id),
                compensation.wire_detail(),
            ));
        }
        self.repository
            .refresh_lifecycle_catalog(&permit, conversation_id)
            .await
            .map_err(map_repository_error)?;
        self.updated(ConversationLifecycleAction::SuspendBinding, &record, None)
    }

    pub async fn replace_agent_binding(
        &self,
        conversation_id: ConversationId,
        mut request: PrepareConversationRequest,
        expected_revision: u64,
        target_runtime_agent_id: Option<String>,
    ) -> Result<ConversationLifecycleOutcome> {
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::BindingReplace)
            .map_err(map_repository_error)?;
        let _guard = self.repository.lifecycle_lock(conversation_id).await;
        let record = self.expected(conversation_id, expected_revision, "replace_binding")?;
        request.conversation_id = Some(conversation_id);
        let previous = self.current_binding(conversation_id, "replace_binding")?;
        let prepared = self
            .creation
            .prepare_replacement(&request)
            .map_err(|source| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ValidationError,
                    "replace_binding",
                    Some(conversation_id),
                    source.detail,
                )
            })?;
        let provider_binding = self
            .provider
            .replace(&previous, &prepared, target_runtime_agent_id.as_deref())
            .await
            .map_err(|source| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::AcpReplaceFailed,
                    "replace_binding",
                    Some(conversation_id),
                    source.detail,
                )
            })?;
        let replacement = AgentSessionBinding {
            schema_version: AGENT_SESSION_BINDING_SCHEMA_VERSION,
            binding_id: Uuid::new_v4(),
            agent_session_id: provider_binding.agent_session_id,
            runtime_agent_id: provider_binding.runtime_agent_id,
            stable_agent_namespace: provider_binding.stable_agent_namespace,
            execution_cwd: prepared.execution_cwd,
            bound_at_utc: Utc::now(),
            state: AgentSessionBindingState::Active,
        };
        if let Err(error) = self.repository.replace_agent_binding_locked(
            &permit,
            conversation_id,
            replacement.clone(),
            replacement.bound_at_utc,
        ) {
            let provider_close_code = self
                .provider
                .abort_replacement(&replacement)
                .await
                .err()
                .map(|source| provider_error_code(source.kind));
            let primary_code = error.stable_code();
            let compensation = self.creation.record_agent_compensation_failure_locked(
                conversation_id,
                &primary_code,
                provider_close_code.as_deref(),
                None,
                Some(replacement.binding_id),
            );
            let code = if compensation.has_secondary_failure() {
                ConversationLifecycleErrorCode::AcpCompensationFailed
            } else {
                ConversationLifecycleErrorCode::ConversationRecoveryRequired
            };
            return Err(lifecycle_error(
                code,
                "replace_binding",
                Some(conversation_id),
                compensation.wire_detail(),
            ));
        }
        self.provider
            .register_binding(&replacement.agent_session_id, conversation_id);
        self.repository
            .refresh_lifecycle_catalog(&permit, conversation_id)
            .await
            .map_err(map_repository_error)?;
        self.updated(
            ConversationLifecycleAction::ReplaceBinding,
            &record,
            Some(previous.agent_session_id),
        )
    }

    pub async fn delete_conversation(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
    ) -> Result<ConversationLifecycleOutcome> {
        let permit = self
            .writer
            .authorize(conversation_id, ConversationMutation::ConversationTombstone)
            .map_err(map_repository_error)?;
        let _guard = self.repository.lifecycle_lock(conversation_id).await;
        let operation_id =
            deterministic_operation_id(LifecycleOperationKind::DeleteConversation, conversation_id);
        let existing = self.load_delete_journal(operation_id)?;

        if let Some(existing) = existing.as_ref() {
            if existing.status == LifecycleOperationStatus::Completed
                || existing.phase == LifecycleOperationPhase::Completed
            {
                return self.completed_delete_outcome(conversation_id, existing);
            }
            if existing.phase >= LifecycleOperationPhase::TerminatePending {
                let mut record = existing.clone();
                record.bump_attempt(Utc::now());
                self.persist_journal(&record)?;
                return self
                    .run_delete_saga(&permit, conversation_id, &mut record)
                    .await;
            }
        }

        if self.conversation_missing(conversation_id)? {
            if let Some(existing) = existing.as_ref() {
                let mut record = existing.clone();
                return self.finish_already_purged(&mut record, conversation_id);
            }
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationNotFound,
                "delete_conversation",
                Some(conversation_id),
                "canonical Conversation was not found",
            ));
        }

        let snapshot = self.expected(conversation_id, expected_revision, "delete_conversation")?;
        let terminal_ids = self.terminal_resource_ids(conversation_id)?;
        let mut record = match existing {
            Some(existing) => {
                let mut record = LifecycleOperationRecordV1::start(
                    LifecycleOperationKind::DeleteConversation,
                    conversation_id,
                    snapshot.last_seq,
                    terminal_ids,
                    Utc::now(),
                )
                .map_err(map_journal_error)?;
                record.attempts = existing.attempts.saturating_add(1);
                record.created_at_utc = existing.created_at_utc;
                record.validate().map_err(map_journal_error)?;
                record
            }
            None => LifecycleOperationRecordV1::start(
                LifecycleOperationKind::DeleteConversation,
                conversation_id,
                snapshot.last_seq,
                terminal_ids,
                Utc::now(),
            )
            .map_err(map_journal_error)?,
        };
        self.persist_journal(&record)?;
        self.run_delete_saga(&permit, conversation_id, &mut record)
            .await
    }

    /// Resume unfinished delete Sagas after ACP restart once Terminal Core is
    /// observable. Recreate-terminal records are left for a later task.
    pub async fn resume_incomplete_delete_operations(
        &self,
    ) -> Result<Vec<ConversationLifecycleOutcome>> {
        let Some(journal) = &self.journal else {
            return Ok(Vec::new());
        };
        let incomplete = journal.list_incomplete().map_err(map_journal_error)?;
        let mut outcomes = Vec::new();
        for record in incomplete {
            if record.kind != LifecycleOperationKind::DeleteConversation {
                continue;
            }
            outcomes.push(
                self.delete_conversation(record.conversation_id, record.expected_revision)
                    .await?,
            );
        }
        Ok(outcomes)
    }

    async fn run_delete_saga(
        &self,
        permit: &crate::conversation::write_authority::RepositoryWritePermit,
        conversation_id: ConversationId,
        record: &mut LifecycleOperationRecordV1,
    ) -> Result<ConversationLifecycleOutcome> {
        let terminal_ids = record.captured_terminal_scope.terminal_ids.clone();

        if self.conversation_missing(conversation_id)? {
            return self.finish_already_purged(record, conversation_id);
        }

        if record.phase <= LifecycleOperationPhase::CaptureScope {
            self.advance_journal(
                record,
                LifecycleOperationPhase::Observe,
                LifecycleOperationStatus::InFlight,
                None,
            )?;
        }

        let needs_terminate = record.phase <= LifecycleOperationPhase::Observe
            || record.phase == LifecycleOperationPhase::TerminatePending;
        if needs_terminate {
            if terminal_ids.is_empty() {
                if record.phase <= LifecycleOperationPhase::TerminatePending {
                    self.advance_journal(
                        record,
                        LifecycleOperationPhase::TerminationConfirmed,
                        LifecycleOperationStatus::InFlight,
                        None,
                    )?;
                }
            } else {
                if let Err(error) = self
                    .terminals
                    .observe_conversation(conversation_id, &terminal_ids)
                    .await
                {
                    return self.block_delete_terminals(
                        record,
                        conversation_id,
                        terminal_ids,
                        format!("terminal observation failed before delete: {error}"),
                        terminal_blocker_code(&error),
                    );
                }
                if record.phase <= LifecycleOperationPhase::Observe {
                    self.advance_journal(
                        record,
                        LifecycleOperationPhase::TerminatePending,
                        LifecycleOperationStatus::InFlight,
                        None,
                    )?;
                }
                let failed = self
                    .terminate_live_resources_for_delete(conversation_id, &terminal_ids)
                    .await;
                if !failed.is_empty() {
                    return self.block_delete_terminals(
                        record,
                        conversation_id,
                        failed,
                        "one or more conversation terminals could not be terminated",
                        "TERMINAL_CLEANUP_FAILED",
                    );
                }
                self.advance_journal(
                    record,
                    LifecycleOperationPhase::TerminationConfirmed,
                    LifecycleOperationStatus::InFlight,
                    None,
                )?;
            }
        }

        if record.phase <= LifecycleOperationPhase::PurgePending {
            if !terminal_ids.is_empty() {
                let remaining = self
                    .reobserve_after_terminate(record, conversation_id, &terminal_ids)
                    .await?;
                if let Some(outcome) = remaining {
                    return Ok(outcome);
                }
            }
            self.advance_journal(
                record,
                LifecycleOperationPhase::PurgePending,
                LifecycleOperationStatus::InFlight,
                None,
            )?;
        }

        if self.conversation_missing(conversation_id)? {
            return self.finish_already_purged(record, conversation_id);
        }

        let current = self
            .repository
            .get_conversation(conversation_id)
            .map_err(map_repository_error)?;
        self.release_live_resources_for_delete(permit, conversation_id)
            .await?;
        let blockers = self.delete_blockers(conversation_id)?;
        if !blockers.is_empty() {
            self.advance_journal(
                record,
                LifecycleOperationPhase::PurgePending,
                LifecycleOperationStatus::Blocked,
                Some("CONVERSATION_LIVE_RESOURCES".to_string()),
            )?;
            log::warn!(
                "[conversation-lifecycle] delete blocked conversation_id={} blocker_count={} revision={}",
                conversation_id,
                blockers.len(),
                current.last_seq
            );
            return Ok(ConversationLifecycleOutcome::Blocked {
                action: ConversationLifecycleAction::DeleteConversation,
                conversation_id,
                revision: current.last_seq,
                code: ConversationLifecycleErrorCode::ConversationLiveResources,
                blockers,
            });
        }

        let deleted = match self
            .repository
            .purge_conversation_locked(permit, conversation_id)
        {
            Ok(deleted) => deleted,
            Err(error) if is_conversation_not_found(&error) => {
                return self.finish_already_purged(record, conversation_id);
            }
            Err(error) => return Err(map_repository_error(error)),
        };
        self.advance_journal(
            record,
            LifecycleOperationPhase::Completed,
            LifecycleOperationStatus::Completed,
            None,
        )?;
        log::info!(
            "[conversation-lifecycle] conversation deleted conversation_id={}",
            conversation_id
        );
        Ok(ConversationLifecycleOutcome::Updated {
            action: ConversationLifecycleAction::DeleteConversation,
            conversation_id,
            previous_revision: current.last_seq,
            revision: deleted.last_seq,
            workspace_cwd: deleted.workspace_cwd,
            lifecycle_state: ConversationLifecycleState::Deleted,
            current_binding: None,
            previous_agent_session_id: None,
        })
    }

    fn load_delete_journal(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<LifecycleOperationRecordV1>> {
        let Some(journal) = &self.journal else {
            return Ok(None);
        };
        journal.load(operation_id).map_err(map_journal_error)
    }

    fn persist_journal(&self, record: &LifecycleOperationRecordV1) -> Result<()> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        journal.persist(record).map_err(map_journal_error)?;
        Ok(())
    }

    fn load_recreate_journal(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<LifecycleOperationRecordV1>> {
        let Some(journal) = &self.journal else {
            return Ok(None);
        };
        journal.load(operation_id).map_err(map_journal_error)
    }

    fn advance_recreate(
        &self,
        record: &mut LifecycleOperationRecordV1,
        phase: LifecycleOperationPhase,
        status: LifecycleOperationStatus,
        state: TerminalRecoveryState,
        last_error_code: Option<String>,
    ) -> Result<()> {
        record.terminal_recovery_state = Some(state);
        self.advance_journal(record, phase, status, last_error_code)
    }

    fn fail_closed_recreate(
        &self,
        record: &mut LifecycleOperationRecordV1,
        detail: &str,
    ) -> Result<()> {
        let _ = detail;
        self.advance_recreate(
            record,
            LifecycleOperationPhase::Observe,
            LifecycleOperationStatus::FailedClosed,
            TerminalRecoveryState::Lost,
            Some("TERMINAL_OWNERSHIP_MISMATCH".to_string()),
        )
    }

    async fn adopt_or_spawn_replacement(
        &self,
        conversation_id: ConversationId,
        previous_terminal_id: &str,
        intent: &DurableTerminalSpawnIntentV1,
        conversation: &ConversationRecordV2,
    ) -> Result<String> {
        let live = self
            .terminals
            .observe_conversation(conversation_id, &[])
            .await
            .map_err(|error| {
                lifecycle_error(
                    if terminal_blocker_code(&error) == "TERMINAL_OWNERSHIP_MISMATCH" {
                        ConversationLifecycleErrorCode::ValidationError
                    } else {
                        ConversationLifecycleErrorCode::ConversationLiveResources
                    },
                    "recreate_terminal",
                    Some(conversation_id),
                    format!("terminal observation failed while adopting replacement: {error}"),
                )
            })?;
        let extras: Vec<String> = live
            .into_iter()
            .filter(|id| id != previous_terminal_id)
            .collect();
        match extras.len() {
            0 => {
                let spawn_intent = spawn_intent_from_durable(intent)?;
                self.terminals
                    .spawn_for_conversation(spawn_intent, conversation)
                    .await
                    .map_err(|error| {
                        lifecycle_error(
                            if error.ends_with("scope is unauthorized")
                                || error.to_ascii_lowercase().contains("scope mismatch")
                            {
                                ConversationLifecycleErrorCode::ValidationError
                            } else {
                                ConversationLifecycleErrorCode::ConversationRecoveryRequired
                            },
                            "recreate_terminal",
                            Some(conversation_id),
                            error,
                        )
                    })
            }
            1 => Ok(extras[0].clone()),
            _ => Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "recreate_terminal",
                Some(conversation_id),
                format!("ambiguous replacement terminals for {previous_terminal_id}: {extras:?}"),
            )),
        }
    }

    fn advance_journal(
        &self,
        record: &mut LifecycleOperationRecordV1,
        phase: LifecycleOperationPhase,
        status: LifecycleOperationStatus,
        last_error_code: Option<String>,
    ) -> Result<()> {
        record
            .advance(phase, status, last_error_code, Utc::now())
            .map_err(map_journal_error)?;
        self.persist_journal(record)
    }

    fn conversation_missing(&self, conversation_id: ConversationId) -> Result<bool> {
        match self.repository.get_conversation(conversation_id) {
            Ok(_) => Ok(false),
            Err(error) if is_conversation_not_found(&error) => Ok(true),
            Err(error) => Err(map_repository_error(error)),
        }
    }

    fn completed_delete_outcome(
        &self,
        conversation_id: ConversationId,
        record: &LifecycleOperationRecordV1,
    ) -> Result<ConversationLifecycleOutcome> {
        if !self.conversation_missing(conversation_id)? {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "delete_conversation",
                Some(conversation_id),
                "lifecycle journal is completed but Conversation still exists",
            ));
        }
        Ok(deleted_conversation_outcome(
            conversation_id,
            record.expected_revision,
            record.expected_revision,
            String::new(),
        ))
    }

    fn finish_already_purged(
        &self,
        record: &mut LifecycleOperationRecordV1,
        conversation_id: ConversationId,
    ) -> Result<ConversationLifecycleOutcome> {
        let revision = record.expected_revision;
        self.advance_journal(
            record,
            LifecycleOperationPhase::Completed,
            LifecycleOperationStatus::Completed,
            None,
        )?;
        Ok(deleted_conversation_outcome(
            conversation_id,
            revision,
            revision,
            String::new(),
        ))
    }

    async fn reobserve_after_terminate(
        &self,
        record: &mut LifecycleOperationRecordV1,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Result<Option<ConversationLifecycleOutcome>> {
        let remaining = match self
            .terminals
            .observe_conversation(conversation_id, terminal_ids)
            .await
        {
            Ok(remaining) => remaining,
            Err(error) => {
                return self
                    .block_delete_terminals(
                        record,
                        conversation_id,
                        terminal_ids.to_vec(),
                        format!("terminal observation failed after cleanup: {error}"),
                        terminal_blocker_code(&error),
                    )
                    .map(Some);
            }
        };
        if remaining.is_empty() {
            return Ok(None);
        }
        let failed = self
            .terminate_live_resources_for_delete(conversation_id, &remaining)
            .await;
        if !failed.is_empty() {
            return self
                .block_delete_terminals(
                    record,
                    conversation_id,
                    failed,
                    "one or more conversation terminals could not be terminated",
                    "TERMINAL_CLEANUP_FAILED",
                )
                .map(Some);
        }
        match self
            .terminals
            .observe_conversation(conversation_id, terminal_ids)
            .await
        {
            Ok(still_live) if still_live.is_empty() => Ok(None),
            Ok(still_live) => self
                .block_delete_terminals(
                    record,
                    conversation_id,
                    still_live,
                    "conversation terminals remain live after cleanup",
                    "TERMINAL_CLEANUP_FAILED",
                )
                .map(Some),
            Err(error) => self
                .block_delete_terminals(
                    record,
                    conversation_id,
                    terminal_ids.to_vec(),
                    format!("terminal observation failed after cleanup: {error}"),
                    terminal_blocker_code(&error),
                )
                .map(Some),
        }
    }

    fn block_delete_terminals(
        &self,
        record: &mut LifecycleOperationRecordV1,
        conversation_id: ConversationId,
        ids: Vec<String>,
        detail: impl Into<String>,
        error_code: &str,
    ) -> Result<ConversationLifecycleOutcome> {
        let phase = record.phase;
        self.advance_journal(
            record,
            phase,
            LifecycleOperationStatus::Blocked,
            Some(error_code.to_string()),
        )?;
        let revision = self
            .repository
            .get_conversation(conversation_id)
            .map(|current| current.last_seq)
            .unwrap_or(record.expected_revision);
        Ok(self.terminal_blocked(conversation_id, revision, ids, detail))
    }

    fn expected(
        &self,
        conversation_id: ConversationId,
        expected_revision: u64,
        operation: &'static str,
    ) -> Result<ConversationRecordV2> {
        self.repository
            .ensure_expected_revision_locked(conversation_id, expected_revision, operation)
            .map_err(map_repository_error)
    }

    fn current_binding(
        &self,
        conversation_id: ConversationId,
        operation: &'static str,
    ) -> Result<AgentSessionBinding> {
        self.repository
            .current_binding(conversation_id)
            .map_err(map_repository_error)?
            .ok_or_else(|| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ConversationBindingNotFound,
                    operation,
                    Some(conversation_id),
                    "Conversation has no current ACP binding",
                )
            })
    }

    fn updated(
        &self,
        action: ConversationLifecycleAction,
        before: &ConversationRecordV2,
        previous_agent_session_id: Option<String>,
    ) -> Result<ConversationLifecycleOutcome> {
        let after = self
            .repository
            .get_conversation(before.conversation_id)
            .map_err(map_repository_error)?;
        let current_binding = self
            .repository
            .current_binding(before.conversation_id)
            .map_err(map_repository_error)?;
        log::info!(
            "[conversation-lifecycle] action={:?} conversation_id={} revision={} binding_id={}",
            action,
            before.conversation_id,
            after.last_seq,
            current_binding
                .as_ref()
                .map(|binding| binding.binding_id.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        Ok(ConversationLifecycleOutcome::Updated {
            action,
            conversation_id: before.conversation_id,
            previous_revision: before.last_seq,
            revision: after.last_seq,
            workspace_cwd: after.workspace_cwd,
            lifecycle_state: after.lifecycle_state,
            current_binding,
            previous_agent_session_id,
        })
    }

    async fn release_live_resources_for_delete(
        &self,
        permit: &crate::conversation::write_authority::RepositoryWritePermit,
        conversation_id: ConversationId,
    ) -> Result<()> {
        if let Some(binding) = self
            .repository
            .current_binding(conversation_id)
            .map_err(map_repository_error)?
        {
            if matches!(
                binding.state,
                AgentSessionBindingState::Active | AgentSessionBindingState::Detached
            ) {
                if let Err(source) = self.provider.suspend(&binding).await {
                    log::warn!(
                        "[conversation-lifecycle] delete suspends binding best-effort conversation_id={} code={}",
                        conversation_id,
                        provider_error_code(source.kind)
                    );
                }
                if let Err(error) = self.repository.release_binding_for_delete_locked(
                    permit,
                    conversation_id,
                    Utc::now(),
                ) {
                    log::warn!(
                        "[conversation-lifecycle] delete binding release failed conversation_id={} code={}",
                        conversation_id,
                        error.stable_code()
                    );
                }
            }
        }

        Ok(())
    }

    async fn terminate_live_resources_for_delete(
        &self,
        conversation_id: ConversationId,
        terminal_ids: &[String],
    ) -> Vec<String> {
        let mut failed = Vec::new();
        for terminal_id in terminal_ids {
            let operation_id = format!("conversation-delete:{conversation_id}:{terminal_id}");
            match self
                .terminals
                .terminate_for_conversation(conversation_id, terminal_id, &operation_id)
                .await
            {
                Ok(()) => log::info!(
                    "[conversation-lifecycle] delete terminated terminal_id={} conversation_id={}",
                    terminal_id,
                    conversation_id
                ),
                Err(_detail) => {
                    log::warn!(
                        "[conversation-lifecycle] delete terminal terminate failed conversation_id={} terminal_id={} code=TERMINAL_CLEANUP_FAILED",
                        conversation_id,
                        terminal_id
                    );
                    failed.push(terminal_id.clone());
                }
            }
        }
        failed
    }

    fn delete_blockers(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<ConversationDeleteBlocker>> {
        let mut blockers = Vec::new();
        if let Some(binding) = self
            .repository
            .current_binding(conversation_id)
            .map_err(map_repository_error)?
        {
            if matches!(
                binding.state,
                AgentSessionBindingState::Active | AgentSessionBindingState::Detached
            ) {
                blockers.push(ConversationDeleteBlocker::LiveBinding {
                    count: 1,
                    ids: vec![binding.agent_session_id],
                });
            }
        }
        Ok(blockers)
    }

    fn terminal_blocked(
        &self,
        conversation_id: ConversationId,
        revision: u64,
        mut ids: Vec<String>,
        detail: impl Into<String>,
    ) -> ConversationLifecycleOutcome {
        ids.sort();
        ids.dedup();
        log::warn!(
            "[conversation-lifecycle] delete blocked conversation_id={} code=CONVERSATION_LIVE_RESOURCES detail={}",
            conversation_id,
            detail.into()
        );
        ConversationLifecycleOutcome::Blocked {
            action: ConversationLifecycleAction::DeleteConversation,
            conversation_id,
            revision,
            code: ConversationLifecycleErrorCode::ConversationLiveResources,
            blockers: vec![ConversationDeleteBlocker::TerminalResources {
                count: ids.len(),
                ids,
            }],
        }
    }

    fn terminal_resource_ids(&self, conversation_id: ConversationId) -> Result<Vec<String>> {
        let Some(bytes) = self
            .repository
            .read_workspace_bytes(conversation_id)
            .map_err(map_repository_error)?
        else {
            return Ok(Vec::new());
        };
        let workspace: SessionWorkspaceV1 = serde_json::from_slice(&bytes).map_err(|error| {
            lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "delete_conversation",
                Some(conversation_id),
                format!("workspace.json cannot be decoded for delete blockers: {error}"),
            )
        })?;
        if workspace.schema_version != SESSION_WORKSPACE_SCHEMA_VERSION
            || workspace.conversation_id != conversation_id
        {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                "delete_conversation",
                Some(conversation_id),
                "workspace.json identity/schema is invalid",
            ));
        }
        let mut ids = workspace
            .resources
            .into_iter()
            .filter_map(|resource| match resource {
                SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. } => {
                    Some(terminal_id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    async fn observe_terminal_resource_ids(
        &self,
        conversation_id: ConversationId,
        operation: &'static str,
    ) -> Result<Vec<String>> {
        let ids = self.terminal_resource_ids(conversation_id)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if !self.terminals.observes_live_terminals() {
            return Err(lifecycle_error(
                ConversationLifecycleErrorCode::ConversationLiveResources,
                operation,
                Some(conversation_id),
                format!(
                    "ACP Core cannot observe conversation terminals {:?}; operation is fail-closed until Terminal Core is linked",
                    ids
                ),
            ));
        }
        self.terminals
            .observe_conversation(conversation_id, &ids)
            .await
            .map_err(|detail| {
                lifecycle_error(
                    ConversationLifecycleErrorCode::ConversationRecoveryRequired,
                    operation,
                    Some(conversation_id),
                    format!("conversation terminal observation failed: {detail}"),
                )
            })
    }
}

fn provider_error_code(kind: AgentLifecycleProviderErrorKind) -> String {
    match kind {
        AgentLifecycleProviderErrorKind::Unsupported => "ACP_CLOSE_UNSUPPORTED",
        AgentLifecycleProviderErrorKind::Failed => "ACP_CLOSE_FAILED",
    }
    .to_string()
}

fn deleted_conversation_outcome(
    conversation_id: ConversationId,
    previous_revision: u64,
    revision: u64,
    workspace_cwd: String,
) -> ConversationLifecycleOutcome {
    ConversationLifecycleOutcome::Updated {
        action: ConversationLifecycleAction::DeleteConversation,
        conversation_id,
        previous_revision,
        revision,
        workspace_cwd,
        lifecycle_state: ConversationLifecycleState::Deleted,
        current_binding: None,
        previous_agent_session_id: None,
    }
}

fn terminal_recovered_outcome(
    conversation_id: ConversationId,
    previous_terminal_id: &str,
    replacement_terminal_id: &str,
    state: TerminalRecoveryState,
    revision: u64,
) -> ConversationLifecycleOutcome {
    ConversationLifecycleOutcome::TerminalRecovered {
        action: ConversationLifecycleAction::RecreateTerminal,
        conversation_id,
        previous_terminal_id: previous_terminal_id.to_string(),
        replacement_terminal_id: replacement_terminal_id.to_string(),
        state,
        revision,
    }
}

fn host_owned_spawn_intent(
    conversation: &ConversationRecordV2,
) -> Result<DurableTerminalSpawnIntentV1> {
    let project_id = match &conversation.execution_target {
        ExecutionTarget::ProjectRoot { project_id, .. }
        | ExecutionTarget::Worktree { project_id, .. } => Some(project_id.clone()),
        ExecutionTarget::Workspace => conversation
            .project_attachment
            .as_ref()
            .map(|attachment| attachment.project_id.clone()),
    };
    if project_id
        .as_ref()
        .is_some_and(|project_id| project_id.trim().is_empty())
    {
        return Err(lifecycle_error(
            ConversationLifecycleErrorCode::ValidationError,
            "recreate_terminal",
            Some(conversation.conversation_id),
            "conversation project scope is unauthorized",
        ));
    }
    let intent = DurableTerminalSpawnIntentV1 {
        schema_version: DURABLE_TERMINAL_SPAWN_INTENT_SCHEMA_VERSION,
        conversation_id: conversation.conversation_id,
        project_id,
        cwd_source: DurableTerminalCwdSource::Workspace,
        cols: 80,
        rows: 24,
        program_policy: TerminalProgramPolicyV1::HostDefaultShell,
        env_policy: TerminalEnvironmentPolicyV1::HostInherited,
        kind: TerminalKindPolicyV1::ConversationInteractive,
    };
    intent
        .validate("recreate_terminal")
        .map_err(map_journal_error)?;
    Ok(intent)
}

fn spawn_intent_from_durable(
    intent: &DurableTerminalSpawnIntentV1,
) -> Result<TerminalSpawnIntentV1> {
    if intent.kind != TerminalKindPolicyV1::ConversationInteractive
        || intent.program_policy != TerminalProgramPolicyV1::HostDefaultShell
        || intent.env_policy != TerminalEnvironmentPolicyV1::HostInherited
    {
        return Err(lifecycle_error(
            ConversationLifecycleErrorCode::ValidationError,
            "recreate_terminal",
            Some(intent.conversation_id),
            "spawn intent policy is not host-owned",
        ));
    }
    Ok(TerminalSpawnIntentV1 {
        conversation_id: intent.conversation_id,
        project_id: intent.project_id.clone(),
        cwd_source: match intent.cwd_source {
            DurableTerminalCwdSource::Workspace => TerminalCwdSource::Workspace,
            DurableTerminalCwdSource::ExecutionTarget => TerminalCwdSource::ExecutionTarget,
        },
        cols: intent.cols,
        rows: intent.rows,
    })
}

fn is_conversation_not_found(error: &RepositoryError) -> bool {
    error.code == ConversationErrorCode::ConversationNotFound
}

fn terminal_blocker_code(detail: &str) -> &'static str {
    let lowered = detail.to_ascii_lowercase();
    if lowered.contains("unauthor")
        || lowered.contains("scope mismatch")
        || lowered.contains("ownership")
    {
        "TERMINAL_OWNERSHIP_MISMATCH"
    } else if lowered.contains("unavailable")
        || lowered.contains("cannot observe")
        || lowered.contains("not linked")
        || lowered.contains("observation")
    {
        "TERMINAL_UNAVAILABLE"
    } else {
        "TERMINAL_CLEANUP_FAILED"
    }
}

fn map_journal_error(source: LifecycleJournalError) -> ConversationLifecycleError {
    let code = match source.code {
        LifecycleJournalErrorCode::LifecycleJournalDurabilityFailed
        | LifecycleJournalErrorCode::LifecycleJournalIoFailed => {
            ConversationLifecycleErrorCode::ConversationDurabilityFailed
        }
        _ => ConversationLifecycleErrorCode::ConversationRecoveryRequired,
    };
    lifecycle_error(
        code,
        source.operation,
        source.conversation_id,
        source.detail,
    )
}

fn map_repository_error(source: RepositoryError) -> ConversationLifecycleError {
    use crate::conversation::contracts::ConversationErrorCode;
    let code = match source.code {
        ConversationErrorCode::ConversationConflict => {
            ConversationLifecycleErrorCode::ConversationConflict
        }
        ConversationErrorCode::ConversationNotFound => {
            ConversationLifecycleErrorCode::ConversationNotFound
        }
        ConversationErrorCode::ConversationBindingNotFound => {
            ConversationLifecycleErrorCode::ConversationBindingNotFound
        }
        ConversationErrorCode::ConversationBindingNotActive => {
            ConversationLifecycleErrorCode::ConversationBindingNotActive
        }
        ConversationErrorCode::ConversationBindingNotDetached => {
            ConversationLifecycleErrorCode::ConversationBindingNotDetached
        }
        ConversationErrorCode::ConversationBindingNotAddressable => {
            ConversationLifecycleErrorCode::ConversationBindingNotAddressable
        }
        ConversationErrorCode::ConversationDurabilityFailed
        | ConversationErrorCode::ConversationDurabilityUnsupported => {
            ConversationLifecycleErrorCode::ConversationDurabilityFailed
        }
        ConversationErrorCode::LegacyCompatibilityReadOnly => {
            ConversationLifecycleErrorCode::LegacyCompatibilityReadOnly
        }
        _ => ConversationLifecycleErrorCode::ConversationRecoveryRequired,
    };
    lifecycle_error(
        code,
        source.operation,
        source.conversation_id,
        source.detail,
    )
}

fn lifecycle_error(
    code: ConversationLifecycleErrorCode,
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    detail: impl Into<String>,
) -> ConversationLifecycleError {
    ConversationLifecycleError {
        code,
        operation,
        conversation_id,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationBackend, ConversationCreator, CreationPartition,
        ExecutionTarget, ProjectAttachment, CONVERSATION_SCHEMA_VERSION,
        PROJECT_ATTACHMENT_SCHEMA_VERSION,
    };
    use crate::conversation::durable_fs::DurableFileSystem;
    use crate::conversation::locator::{ConversationLocator, SessionWorkspaceLocator};
    use crate::conversation::session_workspace::{
        SessionWorkspaceLoadOutcome, SessionWorkspaceProjectionState, SessionWorkspaceService,
    };
    use parking_lot::Mutex;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

    #[derive(Default)]
    struct FakeProvider {
        owns: Mutex<bool>,
        suspend_error: Mutex<Option<AgentLifecycleProviderError>>,
        replace_error: Mutex<Option<AgentLifecycleProviderError>>,
        abort_error: Mutex<Option<AgentLifecycleProviderError>>,
        replacement_execution_cwds: Mutex<Vec<String>>,
        replacement_additional_roots: Mutex<Vec<Vec<String>>>,
        replacement_targets: Mutex<Vec<Option<String>>>,
        suspend_calls: AtomicUsize,
        replace_calls: AtomicUsize,
        abort_calls: AtomicUsize,
        registered: Mutex<Vec<String>>,
    }

    impl ConversationAgentLifecycle for FakeProvider {
        fn owns_session<'a>(
            &'a self,
            _binding: &'a AgentSessionBinding,
        ) -> ProviderFuture<'a, bool> {
            Box::pin(async move { *self.owns.lock() })
        }

        fn suspend<'a>(
            &'a self,
            _binding: &'a AgentSessionBinding,
        ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
            self.suspend_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { self.suspend_error.lock().clone().map_or(Ok(()), Err) })
        }

        fn replace<'a>(
            &'a self,
            _previous_binding: &'a AgentSessionBinding,
            prepared: &'a PreparedConversation,
            target_runtime_agent_id: Option<&'a str>,
        ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
        {
            self.replace_calls.fetch_add(1, Ordering::SeqCst);
            self.replacement_targets
                .lock()
                .push(target_runtime_agent_id.map(str::to_string));
            Box::pin(async move {
                self.replacement_execution_cwds
                    .lock()
                    .push(prepared.execution_cwd.clone());
                self.replacement_additional_roots
                    .lock()
                    .push(prepared.additional_directories.clone());
                if let Some(error) = self.replace_error.lock().clone() {
                    return Err(error);
                }
                Ok(AgentBindingResult {
                    agent_session_id: "opaque/replacement".to_string(),
                    runtime_agent_id: target_runtime_agent_id
                        .unwrap_or("agent-runtime")
                        .to_string(),
                    stable_agent_namespace: "config:test".to_string(),
                })
            })
        }

        fn abort_replacement<'a>(
            &'a self,
            _binding: &'a AgentSessionBinding,
        ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
            self.abort_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { self.abort_error.lock().clone().map_or(Ok(()), Err) })
        }

        fn register_binding(&self, agent_session_id: &str, _conversation_id: ConversationId) {
            self.registered.lock().push(agent_session_id.to_string());
        }
    }

    #[derive(Default)]
    struct FakeTerminals {
        live: Mutex<HashSet<String>>,
        fail_terminate: std::sync::atomic::AtomicBool,
        cannot_observe_liveness: std::sync::atomic::AtomicBool,
        fail_observe: std::sync::atomic::AtomicBool,
        ownership_mismatch: std::sync::atomic::AtomicBool,
        fail_spawn: std::sync::atomic::AtomicBool,
        observe_calls: AtomicUsize,
        terminate_calls: AtomicUsize,
        spawn_calls: AtomicUsize,
        next_spawn_id: Mutex<Option<String>>,
    }

    impl TerminalResourceInspector for FakeTerminals {
        fn is_live(&self, terminal_id: &str) -> bool {
            self.live.lock().contains(terminal_id)
        }

        fn observes_live_terminals(&self) -> bool {
            !self.cannot_observe_liveness.load(Ordering::SeqCst)
        }

        fn observe_conversation<'a>(
            &'a self,
            conversation_id: ConversationId,
            terminal_ids: &'a [String],
        ) -> ProviderFuture<'a, std::result::Result<Vec<String>, String>> {
            self.observe_calls.fetch_add(1, Ordering::SeqCst);
            let _ = conversation_id;
            if !self.observes_live_terminals() {
                return Box::pin(async move {
                    Err("terminal liveness observation is unavailable".to_string())
                });
            }
            if self.fail_observe.load(Ordering::SeqCst) {
                return Box::pin(async move { Err("terminal observation failed".to_string()) });
            }
            if self.ownership_mismatch.load(Ordering::SeqCst) {
                return Box::pin(async move { Err("terminal scope mismatch".to_string()) });
            }
            let live = if terminal_ids.is_empty() {
                self.live.lock().iter().cloned().collect()
            } else {
                terminal_ids
                    .iter()
                    .filter(|terminal_id| self.is_live(terminal_id))
                    .cloned()
                    .collect()
            };
            Box::pin(async move { Ok(live) })
        }

        fn terminate<'a>(
            &'a self,
            terminal_id: &'a str,
        ) -> ProviderFuture<'a, std::result::Result<(), String>> {
            self.terminate_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_terminate.load(Ordering::SeqCst) {
                return Box::pin(async move { Err("terminate refused".to_string()) });
            }
            self.live.lock().remove(terminal_id);
            Box::pin(async move { Ok(()) })
        }

        fn spawn_for_conversation<'a>(
            &'a self,
            intent: TerminalSpawnIntentV1,
            conversation: &'a ConversationRecordV2,
        ) -> ProviderFuture<'a, std::result::Result<String, String>> {
            self.spawn_calls.fetch_add(1, Ordering::SeqCst);
            if intent.conversation_id != conversation.conversation_id {
                return Box::pin(
                    async move { Err("terminal spawn scope is unauthorized".to_string()) },
                );
            }
            if self.fail_spawn.load(Ordering::SeqCst) {
                return Box::pin(async move { Err("spawn refused".to_string()) });
            }
            let id = self.next_spawn_id.lock().clone().unwrap_or_else(|| {
                format!("replacement-{}", self.spawn_calls.load(Ordering::SeqCst))
            });
            self.live.lock().insert(id.clone());
            Box::pin(async move { Ok(id) })
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        repository: Arc<ConversationRepository>,
        creation: Arc<ConversationCreationService>,
        provider: Arc<FakeProvider>,
        terminals: Arc<FakeTerminals>,
        journal: Arc<LifecycleOperationJournal>,
        service: ConversationLifecycleService,
        id: ConversationId,
    }

    async fn fixture() -> Fixture {
        fixture_with_catalog_flush_suppressed(false).await
    }

    async fn fixture_with_catalog_flush_suppressed(suppress_auto_catalog_flush: bool) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let private = base.join("private");
        let visible = base.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
        // Set suppression before the first mutation so no debounce task exists to race the test.
        if suppress_auto_catalog_flush {
            repository.suppress_auto_catalog_flush_for_test();
        }
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        let id = ConversationId::parse(ID).unwrap();
        let created_at = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        let workspace = visible.join("sessions/2026/08/15").join(ID);
        std::fs::create_dir_all(&workspace).unwrap();
        writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: workspace.to_string_lossy().into_owned(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    backend: crate::conversation::ConversationBackend::Agent,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
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
                    agent_session_id: "opaque/original".to_string(),
                    runtime_agent_id: "agent-runtime".to_string(),
                    stable_agent_namespace: "config:test".to_string(),
                    execution_cwd: workspace.to_string_lossy().into_owned(),
                    bound_at_utc: Utc::now(),
                    state: AgentSessionBindingState::Active,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let creation = Arc::new(
            ConversationCreationService::new(
                Arc::clone(&writer),
                ConversationLocator::new(private).unwrap(),
                SessionWorkspaceLocator::new(visible).unwrap(),
            )
            .unwrap(),
        );
        let provider = Arc::new(FakeProvider::default());
        *provider.owns.lock() = true;
        let terminals = Arc::new(FakeTerminals::default());
        let journal = Arc::new(LifecycleOperationJournal::open(&base).unwrap());
        let service = ConversationLifecycleService::new(
            writer,
            Arc::clone(&creation),
            provider.clone(),
            terminals.clone(),
        )
        .with_journal(Arc::clone(&journal));
        Fixture {
            _temp: temp,
            repository,
            creation,
            provider,
            terminals,
            journal,
            service,
            id,
        }
    }

    fn restarted_service(fixture: &Fixture) -> ConversationLifecycleService {
        ConversationLifecycleService::new(
            Arc::clone(fixture.creation.writer()),
            Arc::clone(&fixture.creation),
            Arc::clone(&fixture.provider) as Arc<dyn ConversationAgentLifecycle>,
            Arc::clone(&fixture.terminals) as Arc<dyn TerminalResourceInspector>,
        )
        .with_journal(Arc::clone(&fixture.journal))
    }

    fn persist_delete_phase(
        fixture: &Fixture,
        phase: LifecycleOperationPhase,
        status: LifecycleOperationStatus,
        terminals: &[&str],
        last_error_code: Option<&str>,
        expected_revision: u64,
    ) -> LifecycleOperationRecordV1 {
        let mut record = LifecycleOperationRecordV1::start(
            LifecycleOperationKind::DeleteConversation,
            fixture.id,
            expected_revision,
            terminals.iter().map(|id| (*id).to_string()).collect(),
            Utc::now(),
        )
        .unwrap();
        if phase != LifecycleOperationPhase::CaptureScope
            || status != LifecycleOperationStatus::InFlight
            || last_error_code.is_some()
        {
            record
                .advance(
                    phase,
                    status,
                    last_error_code.map(str::to_string),
                    Utc::now(),
                )
                .unwrap();
        }
        fixture.journal.persist(&record).unwrap();
        record
    }

    fn loaded_delete_journal(fixture: &Fixture) -> LifecycleOperationRecordV1 {
        let operation_id =
            deterministic_operation_id(LifecycleOperationKind::DeleteConversation, fixture.id);
        fixture
            .journal
            .load(operation_id)
            .unwrap()
            .expect("delete journal record")
    }

    fn assert_deleted(outcome: ConversationLifecycleOutcome) {
        assert!(matches!(
            outcome,
            ConversationLifecycleOutcome::Updated {
                action: ConversationLifecycleAction::DeleteConversation,
                lifecycle_state: ConversationLifecycleState::Deleted,
                current_binding: None,
                ..
            }
        ));
    }

    fn assert_live_resources_blocked(outcome: ConversationLifecycleOutcome) {
        assert!(matches!(
            outcome,
            ConversationLifecycleOutcome::Blocked {
                action: ConversationLifecycleAction::DeleteConversation,
                code: ConversationLifecycleErrorCode::ConversationLiveResources,
                ..
            }
        ));
    }

    fn revision(fixture: &Fixture) -> u64 {
        fixture
            .repository
            .get_conversation(fixture.id)
            .unwrap()
            .last_seq
    }

    #[test]
    fn close_view_is_not_a_host_lifecycle_action() {
        assert!(serde_json::from_str::<ConversationLifecycleAction>("\"closeChatView\"").is_err());
    }

    #[tokio::test]
    async fn detach_rebind_and_restart_materialize_distinct_states() {
        let fixture = fixture().await;
        fixture
            .service
            .detach_agent_binding(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_eq!(
            fixture
                .repository
                .current_binding(fixture.id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Detached
        );
        fixture
            .service
            .rebind_detached_binding(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_eq!(
            fixture
                .repository
                .current_binding(fixture.id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Active
        );
        let (reopened, _) =
            ConversationRepository::open(fixture.repository.root().to_path_buf()).unwrap();
        assert_eq!(
            reopened.current_binding(fixture.id).unwrap().unwrap().state,
            AgentSessionBindingState::Active
        );
        let event_types = reopened
            .read_events(fixture.id, 0)
            .unwrap()
            .into_iter()
            .map(|event| event.type_)
            .collect::<Vec<_>>();
        assert!(event_types.contains(&crate::conversation::ConversationEventType::BindingDetached));
        assert!(event_types.contains(&crate::conversation::ConversationEventType::BindingRebound));
    }

    #[tokio::test]
    async fn completed_mutation_admits_catalog_generation_before_failed_barrier_retry() {
        let fixture = fixture_with_catalog_flush_suppressed(true).await;
        fixture
            .repository
            .flush_catalog_until(tokio::time::Instant::now() + std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let before_generation = fixture.repository.catalog_pending_generation();
        fixture.repository.fail_next_catalog_writes(1);

        fixture
            .service
            .detach_agent_binding(fixture.id, revision(&fixture))
            .await
            .unwrap();
        let admitted_generation = fixture.repository.catalog_pending_generation();
        assert!(admitted_generation > before_generation);
        let metrics = fixture.repository.catalog_last_admission_metrics();
        assert_eq!(metrics.serialized_bytes_under_lock, 0);

        let catalog: crate::conversation::ConversationCatalogFileV1 = serde_json::from_slice(
            &fixture
                .repository
                .catalog_flush_coordinator()
                .snapshot()
                .bytes,
        )
        .unwrap();
        let admitted = catalog
            .conversations
            .iter()
            .find(|entry| entry.conversation_id == fixture.id)
            .unwrap();
        assert_eq!(admitted.lifecycle_state, ConversationLifecycleState::Ready);
        assert_eq!(admitted.last_seq, revision(&fixture));

        let repository = Arc::clone(&fixture.repository);
        let failed_barrier =
            tokio::spawn(async move { repository.flush_catalog_once_for_test().await })
                .await
                .unwrap()
                .unwrap_err();
        assert_eq!(failed_barrier.code, "CATALOG_FLUSH_FAILED");
        assert_eq!(
            failed_barrier.stage,
            crate::conversation::repository::CatalogFlushFailureStage::Replacement
        );
        assert_eq!(failed_barrier.pending_generation, admitted_generation);
        assert_eq!(
            fixture.repository.catalog_pending_generation(),
            admitted_generation
        );
        let retry = fixture
            .repository
            .flush_catalog_once_for_test()
            .await
            .unwrap();
        assert!(retry.flushed_generation >= admitted_generation);
    }

    #[tokio::test]
    async fn suspend_supported_and_failures_commit_only_after_provider_success() {
        let fixture = fixture().await;
        let before = revision(&fixture);
        *fixture.provider.suspend_error.lock() = Some(AgentLifecycleProviderError {
            kind: AgentLifecycleProviderErrorKind::Unsupported,
            detail: "agent does not support session/close".to_string(),
        });
        let error = fixture
            .service
            .suspend_agent_binding(fixture.id, before)
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ConversationLifecycleErrorCode::AcpCloseUnsupported
        );
        assert_eq!(revision(&fixture), before);
        assert_eq!(
            fixture
                .repository
                .current_binding(fixture.id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Active
        );

        *fixture.provider.suspend_error.lock() = Some(AgentLifecycleProviderError {
            kind: AgentLifecycleProviderErrorKind::Failed,
            detail: "provider failed".to_string(),
        });
        let error = fixture
            .service
            .suspend_agent_binding(fixture.id, before)
            .await
            .unwrap_err();
        assert_eq!(error.code, ConversationLifecycleErrorCode::AcpCloseFailed);
        assert_eq!(revision(&fixture), before);

        *fixture.provider.suspend_error.lock() = None;
        fixture
            .service
            .suspend_agent_binding(fixture.id, before)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .repository
                .current_binding(fixture.id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Suspended
        );
    }

    #[tokio::test]
    async fn stale_revision_prevents_provider_event_workspace_and_tombstone_mutation() {
        let fixture = fixture().await;
        let before_events = fixture.repository.read_events(fixture.id, 0).unwrap();
        let before_record = fixture.repository.get_conversation(fixture.id).unwrap();
        let error = fixture
            .service
            .suspend_agent_binding(fixture.id, before_record.last_seq - 1)
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ConversationLifecycleErrorCode::ConversationConflict
        );
        assert_eq!(fixture.provider.suspend_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture.repository.read_events(fixture.id, 0).unwrap(),
            before_events
        );
        assert_eq!(
            fixture.repository.get_conversation(fixture.id).unwrap(),
            before_record
        );
    }

    /// Switching the agent of an existing Conversation is a binding replacement:
    /// the Conversation id, its directory and its transcript are untouched, only
    /// the binding moves to the chosen runtime agent. Passing `None` keeps the
    /// current agent (a plain restart), which is the pre-existing behaviour.
    #[tokio::test]
    async fn replace_binds_the_requested_agent_and_keeps_conversation_identity() {
        let fixture = fixture().await;
        let before = fixture.repository.get_conversation(fixture.id).unwrap();

        let outcome = fixture
            .service
            .replace_agent_binding(
                fixture.id,
                PrepareConversationRequest {
                    schema_version: 1,
                    conversation_id: Some(fixture.id),
                    project_attachment: None,
                    execution_target: ExecutionTarget::Workspace,
                    backend: ConversationBackend::Agent,
                },
                before.last_seq,
                Some("agent-runtime-other".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(
            fixture.provider.replacement_targets.lock().as_slice(),
            &[Some("agent-runtime-other".to_string())],
            "the chosen agent must reach the provider"
        );
        let ConversationLifecycleOutcome::Updated {
            conversation_id,
            current_binding,
            ..
        } = outcome
        else {
            panic!("expected an updated outcome");
        };
        assert_eq!(conversation_id, fixture.id);
        assert_eq!(
            current_binding.unwrap().runtime_agent_id,
            "agent-runtime-other",
            "the Conversation is now bound to the new agent"
        );
        let after = fixture.repository.get_conversation(fixture.id).unwrap();
        assert_eq!(after.conversation_id, before.conversation_id);
        assert_eq!(after.workspace_cwd, before.workspace_cwd);
    }

    #[tokio::test]
    async fn replace_preserves_identity_workspace_and_history_while_failure_appends_nothing() {
        let fixture = fixture().await;
        let before = fixture.repository.get_conversation(fixture.id).unwrap();
        *fixture.provider.replace_error.lock() = Some(AgentLifecycleProviderError {
            kind: AgentLifecycleProviderErrorKind::Failed,
            detail: "replacement failed".to_string(),
        });
        let error = fixture
            .service
            .replace_agent_binding(
                fixture.id,
                PrepareConversationRequest {
                    schema_version: 1,
                    conversation_id: Some(fixture.id),
                    project_attachment: None,
                    execution_target: ExecutionTarget::Workspace,
                    backend: ConversationBackend::Agent,
                },
                before.last_seq,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ConversationLifecycleErrorCode::AcpReplaceFailed);
        assert_eq!(revision(&fixture), before.last_seq);

        *fixture.provider.replace_error.lock() = None;
        let outcome = fixture
            .service
            .replace_agent_binding(
                fixture.id,
                PrepareConversationRequest {
                    schema_version: 1,
                    conversation_id: Some(fixture.id),
                    project_attachment: None,
                    execution_target: ExecutionTarget::Workspace,
                    backend: ConversationBackend::Agent,
                },
                before.last_seq,
                None,
            )
            .await
            .unwrap();
        let ConversationLifecycleOutcome::Updated {
            current_binding, ..
        } = outcome
        else {
            panic!("replacement must update");
        };
        assert_eq!(
            current_binding.unwrap().agent_session_id,
            "opaque/replacement"
        );
        let after = fixture.repository.get_conversation(fixture.id).unwrap();
        assert_eq!(after.conversation_id, before.conversation_id);
        assert_eq!(after.workspace_cwd, before.workspace_cwd);
        assert_eq!(after.created_at_utc, before.created_at_utc);
        let history = fixture.repository.binding_history(fixture.id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].state, AgentSessionBindingState::Replaced);
        assert_eq!(history[1].state, AgentSessionBindingState::Active);
    }

    #[tokio::test]
    async fn replacement_uses_latest_canonical_attachment_and_target_not_stale_request_projection()
    {
        let fixture = fixture().await;
        let project_root = fixture._temp.path().join("trusted-project");
        std::fs::create_dir_all(&project_root).unwrap();
        let project_root = std::fs::canonicalize(project_root).unwrap();
        let attachment = ProjectAttachment {
            schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
            project_id: "project-1".to_string(),
            attached_at_utc: parse_created_at_utc("2026-08-15T10:00:00.000Z").unwrap(),
            project_path_snapshot: project_root.to_string_lossy().into_owned(),
            worktree_path: None,
            worktree_branch: None,
        };
        fixture
            .creation
            .writer()
            .attach_project(fixture.id, revision(&fixture), attachment, Utc::now())
            .await
            .unwrap();
        fixture
            .creation
            .writer()
            .update_execution_target(
                fixture.id,
                revision(&fixture),
                ExecutionTarget::ProjectRoot {
                    project_id: "project-1".to_string(),
                    project_root: project_root.to_string_lossy().into_owned(),
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let before = fixture.repository.get_conversation(fixture.id).unwrap();

        fixture
            .service
            .replace_agent_binding(
                fixture.id,
                PrepareConversationRequest {
                    schema_version: 1,
                    conversation_id: Some(fixture.id),
                    project_attachment: None,
                    execution_target: ExecutionTarget::Workspace,
                    backend: ConversationBackend::Agent,
                },
                before.last_seq,
                None,
            )
            .await
            .unwrap();

        // The replacement request carried a stale `Workspace` target; the canonical
        // record says ProjectRoot. Under the fixed-cwd model the cwd can no longer
        // show which target won, so the additional root is what proves it: the
        // canonical project must be exposed, not the request's empty projection.
        assert_eq!(
            fixture
                .provider
                .replacement_additional_roots
                .lock()
                .as_slice(),
            &[vec![project_root.to_string_lossy().into_owned()]]
        );
        let after = fixture.repository.get_conversation(fixture.id).unwrap();
        assert_eq!(after.project_attachment, before.project_attachment);
        assert_eq!(after.execution_target, before.execution_target);
        assert_eq!(after.conversation_id, before.conversation_id);
        assert_eq!(after.created_at_utc, before.created_at_utc);
        assert_eq!(after.creation_partition, before.creation_partition);
        assert_eq!(after.workspace_cwd, before.workspace_cwd);
    }

    #[tokio::test]
    async fn replacement_append_and_abort_failure_never_publishes_provisional_binding() {
        let fixture = fixture().await;
        let before = fixture.repository.get_conversation(fixture.id).unwrap();
        fixture.repository.fail_next_agent_binding_appends(1);
        *fixture.provider.abort_error.lock() = Some(AgentLifecycleProviderError {
            kind: AgentLifecycleProviderErrorKind::Failed,
            detail: "provider leaked SUPER_SECRET=do-not-return".to_string(),
        });

        let error = fixture
            .service
            .replace_agent_binding(
                fixture.id,
                PrepareConversationRequest {
                    schema_version: 1,
                    conversation_id: Some(fixture.id),
                    project_attachment: None,
                    execution_target: ExecutionTarget::Workspace,
                    backend: ConversationBackend::Agent,
                },
                before.last_seq,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ConversationLifecycleErrorCode::AcpCompensationFailed
        );
        let failure: crate::conversation::AgentCompensationFailure =
            serde_json::from_str(&error.detail).unwrap();
        assert_eq!(failure.primary_code, "CONVERSATION_DURABILITY_FAILED");
        assert_eq!(
            failure.provider_close_code.as_deref(),
            Some("ACP_CLOSE_FAILED")
        );
        assert!(failure.recovery_id.is_some());
        assert!(!error.detail.contains("SUPER_SECRET"));
        assert!(!error.detail.contains("opaque/replacement"));
        assert_eq!(fixture.provider.abort_calls.load(Ordering::SeqCst), 1);
        assert!(fixture.provider.registered.lock().is_empty());

        let current = fixture
            .repository
            .current_binding(fixture.id)
            .unwrap()
            .unwrap();
        assert_eq!(current.agent_session_id, "opaque/original");
        assert_eq!(current.state, AgentSessionBindingState::Active);
        let after = fixture.repository.get_conversation(fixture.id).unwrap();
        assert_eq!(
            after.lifecycle_state,
            ConversationLifecycleState::RecoveryRequired
        );
        assert_eq!(after.conversation_id, before.conversation_id);
        assert_eq!(after.workspace_cwd, before.workspace_cwd);
        assert_eq!(after.created_at_utc, before.created_at_utc);
        assert_eq!(
            fixture
                .repository
                .binding_history(fixture.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn delete_releases_live_binding_and_terminal_then_purges() {
        let fixture = fixture().await;
        fixture
            .terminals
            .live
            .lock()
            .insert("terminal-live".to_string());
        let workspace_service = SessionWorkspaceService::new(Arc::clone(fixture.creation.writer()));
        workspace_service
            .write(
                fixture.id,
                None,
                SessionWorkspaceV1 {
                    schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                    conversation_id: fixture.id,
                    revision: 0,
                    updated_at_utc: String::new(),
                    update_identity: Some("test".to_string()),
                    topology: None,
                    active_pane_id: None,
                    resources: vec![SessionWorkspaceResourceDescriptor::Terminal {
                        terminal_id: "terminal-live".to_string(),
                        terminal_record_id: None,
                        conversation_id: fixture.id,
                    }],
                    projection_state: SessionWorkspaceProjectionState::Native,
                },
            )
            .await
            .unwrap();

        let workspace_cwd = fixture
            .repository
            .get_conversation(fixture.id)
            .unwrap()
            .workspace_cwd;
        let deleted = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert!(matches!(
            deleted,
            ConversationLifecycleOutcome::Updated {
                action: ConversationLifecycleAction::DeleteConversation,
                lifecycle_state: ConversationLifecycleState::Deleted,
                current_binding: None,
                ..
            }
        ));
        assert_eq!(fixture.provider.suspend_calls.load(Ordering::SeqCst), 1);
        assert!(!fixture.terminals.is_live("terminal-live"));
        assert!(fixture.repository.get_conversation(fixture.id).is_err());
        assert!(std::path::Path::new(&workspace_cwd).exists());
        let root = fixture.repository.root().to_path_buf();
        let conversation_id = fixture.id;
        let (reopened, _) = ConversationRepository::open(root).unwrap();
        assert!(reopened.get_conversation(conversation_id).is_err());
        assert!(reopened
            .list_conversations()
            .iter()
            .all(|record| record.conversation_id != conversation_id));
        let _ = &fixture.creation;
        let _ = DurableFileSystem::new();
    }

    #[tokio::test]
    async fn delete_blocks_when_terminal_terminate_fails() {
        let fixture = fixture().await;
        fixture
            .terminals
            .fail_terminate
            .store(true, Ordering::SeqCst);
        fixture
            .terminals
            .live
            .lock()
            .insert("terminal-stuck".to_string());
        let workspace_service = SessionWorkspaceService::new(Arc::clone(fixture.creation.writer()));
        workspace_service
            .write(
                fixture.id,
                None,
                SessionWorkspaceV1 {
                    schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                    conversation_id: fixture.id,
                    revision: 0,
                    updated_at_utc: String::new(),
                    update_identity: Some("test".to_string()),
                    topology: None,
                    active_pane_id: None,
                    resources: vec![SessionWorkspaceResourceDescriptor::Terminal {
                        terminal_id: "terminal-stuck".to_string(),
                        terminal_record_id: None,
                        conversation_id: fixture.id,
                    }],
                    projection_state: SessionWorkspaceProjectionState::Native,
                },
            )
            .await
            .unwrap();

        let blocked = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert!(matches!(
            blocked,
            ConversationLifecycleOutcome::Blocked {
                action: ConversationLifecycleAction::DeleteConversation,
                code: ConversationLifecycleErrorCode::ConversationLiveResources,
                ..
            }
        ));
        assert!(fixture.terminals.is_live("terminal-stuck"));
        assert_eq!(
            fixture
                .repository
                .get_conversation(fixture.id)
                .unwrap()
                .lifecycle_state,
            ConversationLifecycleState::Ready
        );
        assert_eq!(fixture.provider.suspend_calls.load(Ordering::SeqCst), 0);
    }

    async fn write_terminal_workspace(fixture: &Fixture, terminal_id: &str) {
        let workspace_service = SessionWorkspaceService::new(Arc::clone(fixture.creation.writer()));
        workspace_service
            .write(
                fixture.id,
                None,
                SessionWorkspaceV1 {
                    schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                    conversation_id: fixture.id,
                    revision: 0,
                    updated_at_utc: String::new(),
                    update_identity: Some("test".to_string()),
                    topology: None,
                    active_pane_id: None,
                    resources: vec![SessionWorkspaceResourceDescriptor::Terminal {
                        terminal_id: terminal_id.to_string(),
                        terminal_record_id: None,
                        conversation_id: fixture.id,
                    }],
                    projection_state: SessionWorkspaceProjectionState::Native,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_blocks_when_terminal_runtime_cannot_observe_liveness() {
        let fixture = fixture().await;
        fixture
            .terminals
            .cannot_observe_liveness
            .store(true, Ordering::SeqCst);
        write_terminal_workspace(&fixture, "terminal-unobserved").await;

        let blocked = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert!(matches!(
            blocked,
            ConversationLifecycleOutcome::Blocked {
                action: ConversationLifecycleAction::DeleteConversation,
                code: ConversationLifecycleErrorCode::ConversationLiveResources,
                ..
            }
        ));
        assert_eq!(
            fixture
                .repository
                .get_conversation(fixture.id)
                .unwrap()
                .lifecycle_state,
            ConversationLifecycleState::Ready
        );
        assert!(!fixture.terminals.is_live("terminal-unobserved"));
    }

    #[tokio::test]
    async fn suspend_fails_closed_when_terminal_runtime_cannot_observe_liveness() {
        let fixture = fixture().await;
        fixture
            .terminals
            .cannot_observe_liveness
            .store(true, Ordering::SeqCst);
        write_terminal_workspace(&fixture, "terminal-unobserved").await;

        let error = fixture
            .service
            .suspend_agent_binding(fixture.id, revision(&fixture))
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ConversationLifecycleErrorCode::ConversationLiveResources
        );
        assert_eq!(
            fixture
                .repository
                .current_binding(fixture.id)
                .unwrap()
                .unwrap()
                .state,
            AgentSessionBindingState::Active
        );
        assert_eq!(fixture.provider.suspend_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn delete_blocks_detached_runtime_when_workspace_lists_terminals() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-core-owned").await;
        let service = ConversationLifecycleService::new(
            Arc::clone(fixture.creation.writer()),
            Arc::clone(&fixture.creation),
            Arc::clone(&fixture.provider) as Arc<dyn ConversationAgentLifecycle>,
            Arc::new(crate::core::TerminalServiceHandle::from_runtime(Arc::new(
                crate::core::DetachedTerminalRuntime,
            ))),
        )
        .with_journal(Arc::clone(&fixture.journal));

        let blocked = service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_live_resources_blocked(blocked);
        assert_eq!(
            fixture
                .repository
                .get_conversation(fixture.id)
                .unwrap()
                .lifecycle_state,
            ConversationLifecycleState::Ready
        );
        let journal = loaded_delete_journal(&fixture);
        assert_eq!(journal.status, LifecycleOperationStatus::Blocked);
        assert_eq!(
            journal.last_error_code.as_deref(),
            Some("TERMINAL_UNAVAILABLE")
        );
    }

    #[tokio::test]
    async fn delete_is_idempotent_for_duplicate_operation_id() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-live").await;
        fixture
            .terminals
            .live
            .lock()
            .insert("terminal-live".to_string());

        let first = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_deleted(first);
        let journal = loaded_delete_journal(&fixture);
        assert_eq!(journal.phase, LifecycleOperationPhase::Completed);
        assert_eq!(journal.status, LifecycleOperationStatus::Completed);

        let second = restarted_service(&fixture)
            .delete_conversation(fixture.id, journal.expected_revision)
            .await
            .unwrap();
        assert_deleted(second);
        assert_eq!(fixture.terminals.terminate_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn delete_treats_disappeared_terminal_as_already_gone() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-gone").await;

        let deleted = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_deleted(deleted);
        assert!(fixture.repository.get_conversation(fixture.id).is_err());
        let journal = loaded_delete_journal(&fixture);
        assert_eq!(journal.phase, LifecycleOperationPhase::Completed);
        assert_eq!(
            journal.captured_terminal_scope.terminal_ids,
            ["terminal-gone"]
        );
    }

    #[tokio::test]
    async fn delete_blocks_on_ownership_mismatch_and_keeps_conversation() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-foreign").await;
        fixture
            .terminals
            .ownership_mismatch
            .store(true, Ordering::SeqCst);

        let blocked = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_live_resources_blocked(blocked);
        assert_eq!(
            fixture
                .repository
                .get_conversation(fixture.id)
                .unwrap()
                .lifecycle_state,
            ConversationLifecycleState::Ready
        );
        let journal = loaded_delete_journal(&fixture);
        assert_eq!(journal.status, LifecycleOperationStatus::Blocked);
        assert_eq!(
            journal.last_error_code.as_deref(),
            Some("TERMINAL_OWNERSHIP_MISMATCH")
        );
        assert_eq!(fixture.terminals.terminate_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn delete_retries_blocked_state_only_after_terminal_core_is_observable() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-live").await;
        fixture
            .terminals
            .live
            .lock()
            .insert("terminal-live".to_string());
        fixture
            .terminals
            .cannot_observe_liveness
            .store(true, Ordering::SeqCst);

        let blocked = fixture
            .service
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_live_resources_blocked(blocked);
        assert!(fixture.terminals.is_live("terminal-live"));
        assert_eq!(fixture.terminals.terminate_calls.load(Ordering::SeqCst), 0);

        let still_blocked = restarted_service(&fixture)
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_live_resources_blocked(still_blocked);
        assert_eq!(fixture.terminals.terminate_calls.load(Ordering::SeqCst), 0);

        fixture
            .terminals
            .cannot_observe_liveness
            .store(false, Ordering::SeqCst);
        let deleted = restarted_service(&fixture)
            .delete_conversation(fixture.id, revision(&fixture))
            .await
            .unwrap();
        assert_deleted(deleted);
        assert!(!fixture.terminals.is_live("terminal-live"));
        assert_eq!(
            loaded_delete_journal(&fixture).phase,
            LifecycleOperationPhase::Completed
        );
    }

    #[tokio::test]
    async fn delete_resumes_each_phase_after_acp_restart() {
        for phase in [
            LifecycleOperationPhase::CaptureScope,
            LifecycleOperationPhase::Observe,
            LifecycleOperationPhase::TerminatePending,
            LifecycleOperationPhase::TerminationConfirmed,
            LifecycleOperationPhase::PurgePending,
        ] {
            let fixture = fixture().await;
            write_terminal_workspace(&fixture, "terminal-live").await;
            fixture
                .terminals
                .live
                .lock()
                .insert("terminal-live".to_string());
            persist_delete_phase(
                &fixture,
                phase,
                LifecycleOperationStatus::NeedsRecovery,
                &["terminal-live"],
                Some("TERMINAL_UNAVAILABLE"),
                revision(&fixture),
            );

            let deleted = restarted_service(&fixture)
                .delete_conversation(fixture.id, revision(&fixture))
                .await
                .unwrap();
            assert!(
                matches!(
                    deleted,
                    ConversationLifecycleOutcome::Updated {
                        action: ConversationLifecycleAction::DeleteConversation,
                        lifecycle_state: ConversationLifecycleState::Deleted,
                        current_binding: None,
                        ..
                    }
                ),
                "phase {phase:?} produced {deleted:?}"
            );
            assert!(!fixture.terminals.is_live("terminal-live"));
            assert!(fixture.repository.get_conversation(fixture.id).is_err());
            let journal = loaded_delete_journal(&fixture);
            assert_eq!(journal.phase, LifecycleOperationPhase::Completed);
            assert_eq!(journal.status, LifecycleOperationStatus::Completed);
            assert!(journal.last_error_code.is_none());
            assert!(journal.attempts >= 2);
        }
    }

    #[tokio::test]
    async fn delete_resume_after_purge_completes_when_conversation_already_gone() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-live").await;
        let expected_revision = revision(&fixture);
        persist_delete_phase(
            &fixture,
            LifecycleOperationPhase::PurgePending,
            LifecycleOperationStatus::InFlight,
            &["terminal-live"],
            None,
            expected_revision,
        );
        let deleted = fixture
            .service
            .delete_conversation(fixture.id, expected_revision)
            .await
            .unwrap();
        assert_deleted(deleted);

        persist_delete_phase(
            &fixture,
            LifecycleOperationPhase::PurgePending,
            LifecycleOperationStatus::InFlight,
            &["terminal-live"],
            None,
            expected_revision,
        );
        let resumed = restarted_service(&fixture)
            .delete_conversation(fixture.id, expected_revision)
            .await
            .unwrap();
        assert_deleted(resumed);
        assert_eq!(
            loaded_delete_journal(&fixture).phase,
            LifecycleOperationPhase::Completed
        );
    }

    #[tokio::test]
    async fn delete_stale_revision_conflicts_before_terminate_but_resumes_after() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "terminal-live").await;
        fixture
            .terminals
            .live
            .lock()
            .insert("terminal-live".to_string());
        let stale = revision(&fixture);
        persist_delete_phase(
            &fixture,
            LifecycleOperationPhase::CaptureScope,
            LifecycleOperationStatus::InFlight,
            &["terminal-live"],
            None,
            stale,
        );
        fixture
            .service
            .detach_agent_binding(fixture.id, stale)
            .await
            .unwrap();
        let current = revision(&fixture);
        assert_ne!(current, stale);

        let conflict = restarted_service(&fixture)
            .delete_conversation(fixture.id, stale)
            .await
            .unwrap_err();
        assert_eq!(
            conflict.code,
            ConversationLifecycleErrorCode::ConversationConflict
        );
        assert!(fixture.terminals.is_live("terminal-live"));

        persist_delete_phase(
            &fixture,
            LifecycleOperationPhase::TerminatePending,
            LifecycleOperationStatus::NeedsRecovery,
            &["terminal-live"],
            None,
            stale,
        );
        let deleted = restarted_service(&fixture)
            .delete_conversation(fixture.id, stale)
            .await
            .unwrap();
        assert!(
            matches!(
                deleted,
                ConversationLifecycleOutcome::Updated {
                    action: ConversationLifecycleAction::DeleteConversation,
                    lifecycle_state: ConversationLifecycleState::Deleted,
                    current_binding: None,
                    ..
                }
            ),
            "stale terminate-pending resume produced {deleted:?}"
        );
        assert!(!fixture.terminals.is_live("terminal-live"));
    }

    fn loaded_recreate_journal(fixture: &Fixture, terminal_id: &str) -> LifecycleOperationRecordV1 {
        let operation_id = deterministic_recreate_operation_id(fixture.id, terminal_id);
        fixture
            .journal
            .load(operation_id)
            .unwrap()
            .expect("recreate journal record")
    }

    fn assert_recreated(outcome: ConversationLifecycleOutcome, previous: &str, replacement: &str) {
        match outcome {
            ConversationLifecycleOutcome::TerminalRecovered {
                action: ConversationLifecycleAction::RecreateTerminal,
                previous_terminal_id,
                replacement_terminal_id,
                state: TerminalRecoveryState::Recreated,
                ..
            } => {
                assert_eq!(previous_terminal_id, previous);
                assert_eq!(replacement_terminal_id, replacement);
            }
            other => panic!("expected recreated outcome, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn recover_lost_terminal_replaces_workspace_ref_from_host_intent() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        *fixture.terminals.next_spawn_id.lock() = Some("term-new".to_string());

        let recovered = fixture
            .service
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap();
        assert_recreated(recovered, "term-old", "term-new");
        assert_eq!(fixture.terminals.spawn_calls.load(Ordering::SeqCst), 1);
        assert!(fixture.terminals.is_live("term-new"));
        assert!(!fixture.terminals.is_live("term-old"));

        let workspace = SessionWorkspaceService::new(Arc::clone(fixture.creation.writer()))
            .load(fixture.id)
            .await
            .unwrap();
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = workspace else {
            panic!("workspace loaded");
        };
        assert!(workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "term-new"
        )));
        assert!(!workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "term-old"
        )));

        let journal = loaded_recreate_journal(&fixture, "term-old");
        assert_eq!(journal.phase, LifecycleOperationPhase::Completed);
        assert_eq!(journal.status, LifecycleOperationStatus::Completed);
        assert_eq!(
            journal.terminal_recovery_state,
            Some(TerminalRecoveryState::Recreated)
        );
        assert_eq!(journal.replacement_terminal_id.as_deref(), Some("term-new"));
        let intent = journal.spawn_intent.expect("durable spawn intent");
        assert_eq!(intent.conversation_id, fixture.id);
        assert_eq!(
            intent.program_policy,
            TerminalProgramPolicyV1::HostDefaultShell
        );
        assert_eq!(
            intent.env_policy,
            TerminalEnvironmentPolicyV1::HostInherited
        );
        assert!(serde_json::to_value(&intent).unwrap().get("env").is_none());
        assert!(serde_json::to_value(&intent).unwrap().get("cwd").is_none());
        assert!(serde_json::to_value(&intent)
            .unwrap()
            .get("claim")
            .is_none());
    }

    #[tokio::test]
    async fn recover_lost_terminal_is_idempotent_for_duplicate_recovery() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        *fixture.terminals.next_spawn_id.lock() = Some("term-new".to_string());

        let first = fixture
            .service
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap();
        assert_recreated(first, "term-old", "term-new");
        let second = restarted_service(&fixture)
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap();
        assert_recreated(second, "term-old", "term-new");
        assert_eq!(fixture.terminals.spawn_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recover_lost_terminal_retries_after_spawn_before_ref_commit() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        fixture.terminals.live.lock().insert("term-new".to_string());
        let conversation = fixture.repository.get_conversation(fixture.id).unwrap();
        let mut record = LifecycleOperationRecordV1::start_recreate(
            fixture.id,
            conversation.last_seq,
            "term-old".to_string(),
            host_owned_spawn_intent(&conversation).unwrap(),
            Utc::now(),
        )
        .unwrap();
        record.replacement_terminal_id = Some("term-new".to_string());
        record.terminal_recovery_state = Some(TerminalRecoveryState::Recreated);
        record
            .advance(
                LifecycleOperationPhase::Recreated,
                LifecycleOperationStatus::InFlight,
                None,
                Utc::now(),
            )
            .unwrap();
        fixture.journal.persist(&record).unwrap();

        let recovered = restarted_service(&fixture)
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap();
        assert_recreated(recovered, "term-old", "term-new");
        assert_eq!(fixture.terminals.spawn_calls.load(Ordering::SeqCst), 0);
        let workspace = SessionWorkspaceService::new(Arc::clone(fixture.creation.writer()))
            .load(fixture.id)
            .await
            .unwrap();
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = workspace else {
            panic!("workspace loaded");
        };
        assert!(workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "term-new"
        )));
    }

    #[tokio::test]
    async fn recover_lost_terminal_fails_closed_on_mismatched_conversation_scope() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        fixture
            .terminals
            .ownership_mismatch
            .store(true, Ordering::SeqCst);

        let error = fixture
            .service
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap_err();
        assert_eq!(error.code, ConversationLifecycleErrorCode::ValidationError);
        assert_eq!(fixture.terminals.spawn_calls.load(Ordering::SeqCst), 0);
        let journal = loaded_recreate_journal(&fixture, "term-old");
        assert_eq!(journal.status, LifecycleOperationStatus::FailedClosed);
        assert_eq!(
            journal.last_error_code.as_deref(),
            Some("TERMINAL_OWNERSHIP_MISMATCH")
        );
    }

    #[tokio::test]
    async fn recover_lost_terminal_fails_closed_when_terminal_core_is_detached() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        let service = ConversationLifecycleService::new(
            Arc::clone(fixture.creation.writer()),
            Arc::clone(&fixture.creation),
            Arc::clone(&fixture.provider) as Arc<dyn ConversationAgentLifecycle>,
            Arc::new(crate::core::TerminalServiceHandle::from_runtime(Arc::new(
                crate::core::DetachedTerminalRuntime,
            ))),
        )
        .with_journal(Arc::clone(&fixture.journal));

        let error = service
            .recover_lost_terminal(fixture.id, "term-old")
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            ConversationLifecycleErrorCode::ConversationLiveResources
        );
    }

    #[tokio::test]
    async fn recover_lost_conversation_terminals_scans_workspace_refs() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-old").await;
        *fixture.terminals.next_spawn_id.lock() = Some("term-new".to_string());
        let outcomes = fixture
            .service
            .recover_lost_conversation_terminals()
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_recreated(outcomes.into_iter().next().unwrap(), "term-old", "term-new");
    }

    #[tokio::test]
    async fn recover_lost_terminal_keeps_live_terminal_without_spawning() {
        let fixture = fixture().await;
        write_terminal_workspace(&fixture, "term-live").await;
        fixture
            .terminals
            .live
            .lock()
            .insert("term-live".to_string());

        let outcome = fixture
            .service
            .recover_lost_terminal(fixture.id, "term-live")
            .await
            .unwrap();
        match outcome {
            ConversationLifecycleOutcome::TerminalRecovered {
                replacement_terminal_id,
                state: TerminalRecoveryState::Active,
                ..
            } => assert_eq!(replacement_terminal_id, "term-live"),
            other => panic!("expected active terminal, got {other:?}"),
        }
        assert_eq!(fixture.terminals.spawn_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_lost_pty_is_replaced_from_host_owned_intent() {
        let fixture = fixture().await;
        let pty = crate::web::test_pty_manager();
        let service = ConversationLifecycleService::new(
            Arc::clone(fixture.creation.writer()),
            Arc::clone(&fixture.creation),
            Arc::clone(&fixture.provider) as Arc<dyn ConversationAgentLifecycle>,
            Arc::clone(&pty) as Arc<dyn TerminalResourceInspector>,
        )
        .with_journal(Arc::clone(&fixture.journal));
        let conversation = fixture.repository.get_conversation(fixture.id).unwrap();
        let spawned = pty
            .spawn_for_conversation(
                TerminalSpawnIntentV1 {
                    conversation_id: fixture.id,
                    project_id: None,
                    cwd_source: TerminalCwdSource::Workspace,
                    cols: 80,
                    rows: 24,
                },
                &conversation,
                None,
            )
            .await
            .expect("spawn original unix pty");
        let old_id = spawned.info.id.clone();
        write_terminal_workspace(&fixture, &old_id).await;
        pty.terminate(&old_id)
            .await
            .expect("terminate original unix pty");

        let recovered = service
            .recover_lost_terminal(fixture.id, &old_id)
            .await
            .expect("recreate from durable intent");
        let replacement_id = match recovered {
            ConversationLifecycleOutcome::TerminalRecovered {
                previous_terminal_id,
                replacement_terminal_id,
                state: TerminalRecoveryState::Recreated,
                ..
            } => {
                assert_eq!(previous_terminal_id, old_id);
                assert_ne!(replacement_terminal_id, old_id);
                replacement_terminal_id
            }
            other => panic!("expected recreated unix pty, got {other:?}"),
        };
        assert!(pty.get(&replacement_id).is_some());
        assert!(pty.get(&old_id).is_none());

        let again = service
            .recover_lost_terminal(fixture.id, &old_id)
            .await
            .expect("duplicate unix recovery");
        match again {
            ConversationLifecycleOutcome::TerminalRecovered {
                replacement_terminal_id,
                ..
            } => assert_eq!(replacement_terminal_id, replacement_id),
            other => panic!("expected idempotent unix recovery, got {other:?}"),
        }
        let _ = pty.terminate(&replacement_id).await;
    }
}
