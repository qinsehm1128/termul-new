//! Explicit Conversation binding and delete lifecycle coordination.
//!
//! Renderer view close is deliberately absent from this host service. Every mutation compares the
//! caller's expected revision with canonical `ConversationRecordV2.lastSeq` while holding the
//! repository's per-Conversation lock. Explicit delete first releases live agent bindings and
//! terminates conversation-scoped PTYs; remaining live terminals still block delete. Successful
//! delete physically removes the Conversation — it is not archived.
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
    AgentSessionBinding, AgentSessionBindingState, ConversationId, ConversationLifecycleState,
    ConversationRecordV2, AGENT_SESSION_BINDING_SCHEMA_VERSION,
};
use crate::conversation::creation::{
    AgentBindingResult, ConversationCreationService, PrepareConversationRequest,
    PreparedConversation,
};
use crate::conversation::repository::{ConversationRepository, RepositoryError};
use crate::conversation::session_workspace::{
    SessionWorkspaceResourceDescriptor, SessionWorkspaceV1, SESSION_WORKSPACE_SCHEMA_VERSION,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};
use crate::pty::PtyManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationLifecycleAction {
    DetachBinding,
    RebindDetachedBinding,
    SuspendBinding,
    ReplaceBinding,
    DeleteConversation,
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
    fn replace<'a>(
        &'a self,
        previous_binding: &'a AgentSessionBinding,
        prepared: &'a PreparedConversation,
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
    ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
    {
        Box::pin(async move {
            self.create_replacement_session(previous_binding, prepared)
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
    fn terminate<'a>(
        &'a self,
        terminal_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        Box::pin(async move {
            let _ = terminal_id;
            Err("terminal terminate is not available".to_string())
        })
    }
}

impl TerminalResourceInspector for PtyManager {
    fn is_live(&self, terminal_id: &str) -> bool {
        self.get(terminal_id).is_some()
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
}

#[derive(Clone)]
pub struct ConversationLifecycleService {
    writer: Arc<ConversationWriter>,
    repository: Arc<ConversationRepository>,
    creation: Arc<ConversationCreationService>,
    provider: Arc<dyn ConversationAgentLifecycle>,
    terminals: Arc<dyn TerminalResourceInspector>,
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
        }
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
        let provider_binding =
            self.provider
                .replace(&previous, &prepared)
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
        let record = self.expected(conversation_id, expected_revision, "delete_conversation")?;
        self.release_live_resources_for_delete(&permit, conversation_id)
            .await?;
        let blockers = self.delete_blockers(conversation_id)?;
        if !blockers.is_empty() {
            log::warn!(
                "[conversation-lifecycle] delete blocked conversation_id={} blocker_count={} revision={}",
                conversation_id,
                blockers.len(),
                record.last_seq
            );
            return Ok(ConversationLifecycleOutcome::Blocked {
                action: ConversationLifecycleAction::DeleteConversation,
                conversation_id,
                revision: record.last_seq,
                code: ConversationLifecycleErrorCode::ConversationLiveResources,
                blockers,
            });
        }
        let deleted = self
            .repository
            .purge_conversation_locked(&permit, conversation_id)
            .map_err(map_repository_error)?;
        log::info!(
            "[conversation-lifecycle] conversation deleted conversation_id={}",
            conversation_id
        );
        Ok(ConversationLifecycleOutcome::Updated {
            action: ConversationLifecycleAction::DeleteConversation,
            conversation_id,
            previous_revision: record.last_seq,
            revision: deleted.last_seq,
            workspace_cwd: deleted.workspace_cwd,
            lifecycle_state: ConversationLifecycleState::Deleted,
            current_binding: None,
            previous_agent_session_id: None,
        })
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

        for terminal_id in self.live_terminal_resource_ids(conversation_id)? {
            match self.terminals.terminate(&terminal_id).await {
                Ok(()) => {
                    log::info!(
                        "[conversation-lifecycle] delete terminated terminal_id={} conversation_id={}",
                        terminal_id,
                        conversation_id
                    );
                }
                Err(detail) => {
                    log::warn!(
                        "[conversation-lifecycle] delete terminal terminate failed conversation_id={} detail={}",
                        conversation_id,
                        detail
                    );
                }
            }
        }
        Ok(())
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
        let terminal_ids = self.live_terminal_resource_ids(conversation_id)?;
        if !terminal_ids.is_empty() {
            blockers.push(ConversationDeleteBlocker::TerminalResources {
                count: terminal_ids.len(),
                ids: terminal_ids,
            });
        }
        Ok(blockers)
    }

    fn live_terminal_resource_ids(&self, conversation_id: ConversationId) -> Result<Vec<String>> {
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
                SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                    if self.terminals.is_live(&terminal_id) =>
                {
                    Some(terminal_id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}

fn provider_error_code(kind: AgentLifecycleProviderErrorKind) -> String {
    match kind {
        AgentLifecycleProviderErrorKind::Unsupported => "ACP_CLOSE_UNSUPPORTED",
        AgentLifecycleProviderErrorKind::Failed => "ACP_CLOSE_FAILED",
    }
    .to_string()
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
        parse_created_at_utc, ConversationCreator, CreationPartition, ExecutionTarget,
        ProjectAttachment, CONVERSATION_SCHEMA_VERSION, PROJECT_ATTACHMENT_SCHEMA_VERSION,
    };
    use crate::conversation::durable_fs::DurableFileSystem;
    use crate::conversation::locator::{ConversationLocator, SessionWorkspaceLocator};
    use crate::conversation::session_workspace::{
        SessionWorkspaceProjectionState, SessionWorkspaceService,
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
        ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
        {
            self.replace_calls.fetch_add(1, Ordering::SeqCst);
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
                    runtime_agent_id: "agent-runtime".to_string(),
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
    }

    impl TerminalResourceInspector for FakeTerminals {
        fn is_live(&self, terminal_id: &str) -> bool {
            self.live.lock().contains(terminal_id)
        }

        fn terminate<'a>(
            &'a self,
            terminal_id: &'a str,
        ) -> ProviderFuture<'a, std::result::Result<(), String>> {
            if self.fail_terminate.load(Ordering::SeqCst) {
                return Box::pin(async move { Err("terminate refused".to_string()) });
            }
            self.live.lock().remove(terminal_id);
            Box::pin(async move { Ok(()) })
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        repository: Arc<ConversationRepository>,
        creation: Arc<ConversationCreationService>,
        provider: Arc<FakeProvider>,
        terminals: Arc<FakeTerminals>,
        service: ConversationLifecycleService,
        id: ConversationId,
    }

    async fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let private = base.join("private");
        let visible = base.join("visible");
        std::fs::create_dir_all(&visible).unwrap();
        let (repository, _) = ConversationRepository::open(private.clone()).unwrap();
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
        let service = ConversationLifecycleService::new(
            writer,
            Arc::clone(&creation),
            provider.clone(),
            terminals.clone(),
        );
        Fixture {
            _temp: temp,
            repository,
            creation,
            provider,
            terminals,
            service,
            id,
        }
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
        let fixture = fixture().await;
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
                },
                before.last_seq,
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
                },
                before.last_seq,
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
                },
                before.last_seq,
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
                },
                before.last_seq,
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
        assert_eq!(fixture.provider.suspend_calls.load(Ordering::SeqCst), 1);
    }
}
