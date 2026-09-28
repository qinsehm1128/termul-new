//! Host wiring for agent sessions: the app-side implementations of the
//! agent-session crate's provider traits, and the lifecycle composition that
//! needs app types (`AcpManager`, the terminal service handle).

use std::path::Path;
use std::sync::Arc;

use crate::acp::AcpManager;
use crate::conversation::{
    AgentBindingResult, AgentLifecycleProviderError, AgentLifecycleProviderErrorKind,
    AgentSessionBinding, ConversationAgentLifecycle, ConversationApplicationError,
    ConversationApplicationService, ConversationId, ConversationLifecycleService,
    ConversationRecordV2, ManagedSkillProvisioner, PreparedConversation, ProviderFuture,
    TerminalResourceInspector, TerminalSpawnIntentV1,
};
use crate::core::TerminalServiceHandle;
use crate::pty::PtyManager;
use crate::skills::ConversationSkillProvisioner;

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

/// Give an application service everything the host provides: the lifecycle
/// runtime and the managed-skill provisioner.
pub fn attach(
    application: &ConversationApplicationService,
    lifecycle: ConversationLifecycleService,
) -> Result<(), ConversationApplicationError> {
    application.attach_lifecycle(lifecycle)?;
    application.attach_skill_provisioner(skill_provisioner())
}

/// The host's managed-skill provisioner.
#[must_use]
pub fn skill_provisioner() -> Arc<dyn ManagedSkillProvisioner> {
    Arc::new(ConversationSkillProvisioner::new())
}

impl ManagedSkillProvisioner for ConversationSkillProvisioner {
    fn provision(&self, workspace_cwd: &Path, provider_key: &str) -> Result<(), String> {
        ConversationSkillProvisioner::provision(self, workspace_cwd, provider_key)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
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
    ) -> ProviderFuture<'a, Result<(), AgentLifecycleProviderError>> {
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
    ) -> ProviderFuture<'a, Result<AgentBindingResult, AgentLifecycleProviderError>> {
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
    ) -> ProviderFuture<'a, Result<(), AgentLifecycleProviderError>> {
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
    ) -> ProviderFuture<'a, Result<Vec<String>, String>> {
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
    ) -> ProviderFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.runtime()
                .terminate_for_conversation(conversation_id, terminal_id, operation_id)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn terminate<'a>(&'a self, terminal_id: &'a str) -> ProviderFuture<'a, Result<(), String>> {
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
    ) -> ProviderFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let options = intent.into_trusted_options(conversation)?;
            self.runtime()
                .spawn_trusted(options)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::migration::MigrationHostMode;
    use crate::conversation::{ConversationBootstrap, HostConversationRoots};

    #[test]
    fn attach_gives_the_application_both_host_services() {
        let temp = tempfile::tempdir().unwrap();
        let bootstrap = ConversationBootstrap::run(
            HostConversationRoots::new(temp.path().join("state"), temp.path().join("visible")),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        let acp = Arc::new(AcpManager::with_conversation_services(
            Vec::new(),
            Arc::clone(&bootstrap.creation),
            Arc::clone(&bootstrap.persistence_adapter),
        ));
        let pty = crate::pty::test_pty_manager();
        let lifecycle = || lifecycle_from_manager(Arc::clone(&acp), Arc::clone(&pty)).unwrap();

        attach(&bootstrap.application, lifecycle()).unwrap();

        // Each slot takes one value, so a second attach proves the first filled it.
        let again = bootstrap.application.attach_lifecycle(lifecycle());
        assert_eq!(
            again.unwrap_err().code,
            "CONVERSATION_SERVICE_ALREADY_ATTACHED"
        );
        let again = bootstrap
            .application
            .attach_skill_provisioner(Arc::new(ConversationSkillProvisioner::new()));
        assert_eq!(
            again.unwrap_err().code,
            "CONVERSATION_SERVICE_ALREADY_ATTACHED"
        );
    }

    /// An Agent Core with no Terminal Core linked must never let the lifecycle
    /// believe a terminal is gone or claim a cleanup it could not do.
    #[tokio::test]
    async fn a_detached_terminal_runtime_is_an_inspector_that_fails_closed() {
        let terminals =
            TerminalServiceHandle::from_runtime(Arc::new(crate::core::DetachedTerminalRuntime));
        let id = ConversationId::new_v4();

        assert!(!terminals.observes_live_terminals());
        assert!(!terminals.is_live("t1"));
        assert!(terminals.observe_conversation(id, &[]).await.is_err());
        assert!(terminals
            .terminate_for_conversation(id, "t1", "op")
            .await
            .is_err());
        assert!(terminals.terminate("t1").await.is_err());
    }

    #[test]
    fn host_skill_provisioner_writes_the_managed_skill_idempotently() {
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().canonicalize().unwrap();
        let skills: Arc<dyn ManagedSkillProvisioner> =
            Arc::new(ConversationSkillProvisioner::new());

        skills.provision(&cwd, "claude-agent-acp").unwrap();
        // Read the name through the provisioner's own accessor: it is a brand
        // contract, and an inline copy would stop matching on a rename.
        let skill_name = crate::skills::provisioner::scheduled_task_skill_name();
        let cross_tool = cwd.join(".agents/skills").join(skill_name).join("SKILL.md");
        let provider = cwd.join(".claude/skills").join(skill_name).join("SKILL.md");
        let first = std::fs::read_to_string(&cross_tool).unwrap();
        assert!(provider.exists());

        skills.provision(&cwd, "claude-agent-acp").unwrap();
        assert_eq!(std::fs::read_to_string(cross_tool).unwrap(), first);
    }
}
