//! Host wiring for agent sessions: the app-side implementations of the
//! agent-session crate's provider traits, and the lifecycle composition that
//! needs app types (`AcpManager`, the terminal service handle).

use std::sync::Arc;

use crate::acp::AcpManager;
use crate::conversation::{
    AgentBindingResult, AgentLifecycleProviderError, AgentLifecycleProviderErrorKind,
    AgentSessionBinding, ConversationAgentLifecycle, ConversationId, ConversationLifecycleService,
    ConversationRecordV2, PreparedConversation, ProviderFuture, TerminalResourceInspector,
    TerminalSpawnIntentV1,
};
use crate::core::TerminalServiceHandle;
use crate::pty::PtyManager;

type LifecycleResult<T> = crate::conversation::LifecycleResult<T>;

/// Lifecycle over the in-process ACP manager and PTY manager.
pub fn lifecycle_from_manager(
    acp: Arc<AcpManager>,
    pty: Arc<PtyManager>,
) -> LifecycleResult<ConversationLifecycleService> {
    ConversationLifecycleService::for_creation(acp.conversation_creation(), acp, pty)
}

/// Lifecycle over the in-process ACP manager and a (possibly Core-backed)
/// terminal service.
pub fn lifecycle_from_terminal(
    acp: Arc<AcpManager>,
    terminal: TerminalServiceHandle,
) -> LifecycleResult<ConversationLifecycleService> {
    ConversationLifecycleService::for_creation(acp.conversation_creation(), acp, Arc::new(terminal))
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

impl TerminalResourceInspector for TerminalServiceHandle {
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
            let options = intent.into_trusted_options(conversation)?;
            self.runtime()
                .spawn_trusted(options)
                .await
                .map_err(|error| error.to_string())
        })
    }
}
