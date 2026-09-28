//! Conversation-scoped terminal spawn intent.
//!
//! A remote caller names only the Conversation and a host-owned cwd source;
//! this module turns that into trusted [`SpawnOptions`] from the host's own
//! Conversation record. It lives with the Conversation so the terminal runtime
//! never reads Conversation records.

use serde::{Deserialize, Serialize};

use crate::conversation::{ConversationId, ConversationRecordV2, ExecutionTarget};
use crate::pty::SpawnOptions;

/// Host-authorized remote terminal spawn intent.
///
/// Remote callers may select only the canonical Conversation, optional project
/// attribution, one of the two host-owned cwd sources, and terminal dimensions.
/// Program, shell, argv, environment, and raw cwd never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSpawnIntentV1 {
    pub conversation_id: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub cwd_source: TerminalCwdSource,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalCwdSource {
    Workspace,
    ExecutionTarget,
}

impl TerminalSpawnIntentV1 {
    pub fn into_trusted_options(
        self,
        conversation: &ConversationRecordV2,
    ) -> Result<SpawnOptions, String> {
        if self.conversation_id != conversation.conversation_id {
            return Err("terminal spawn scope is unauthorized".to_string());
        }
        if self.cols == 0 || self.rows == 0 {
            return Err("terminal dimensions must be greater than zero".to_string());
        }

        let authoritative_project_id = match &conversation.execution_target {
            ExecutionTarget::ProjectRoot { project_id, .. }
            | ExecutionTarget::Worktree { project_id, .. } => Some(project_id.as_str()),
            ExecutionTarget::Workspace => conversation
                .project_attachment
                .as_ref()
                .map(|attachment| attachment.project_id.as_str()),
        };
        if let Some(project_id) = self.project_id.as_deref() {
            if project_id.trim().is_empty() || authoritative_project_id != Some(project_id) {
                return Err("terminal spawn project scope is unauthorized".to_string());
            }
        }

        let cwd = match (&self.cwd_source, &conversation.execution_target) {
            (TerminalCwdSource::Workspace, _)
            | (TerminalCwdSource::ExecutionTarget, ExecutionTarget::Workspace) => {
                conversation.workspace_cwd.clone()
            }
            (
                TerminalCwdSource::ExecutionTarget,
                ExecutionTarget::ProjectRoot { project_root, .. },
            ) => project_root.clone(),
            (
                TerminalCwdSource::ExecutionTarget,
                ExecutionTarget::Worktree { worktree_path, .. },
            ) => worktree_path.clone(),
        };

        Ok(SpawnOptions {
            shell: None,
            cwd: Some(cwd),
            env: None,
            conversation_id: Some(self.conversation_id),
            project_id: self.project_id,
            cols: Some(self.cols),
            rows: Some(self.rows),
            program: None,
            args: None,
            kind: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::manager::tracks_session_workspace_ref;

    fn conversation_record(
        conversation_id: ConversationId,
        workspace_cwd: &str,
        execution_target: ExecutionTarget,
    ) -> ConversationRecordV2 {
        let created_at =
            crate::conversation::parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        ConversationRecordV2 {
            schema_version: crate::conversation::CONVERSATION_SCHEMA_VERSION,
            conversation_id,
            created_at_utc: created_at,
            creation_partition: crate::conversation::CreationPartition::from_created_at(created_at),
            workspace_cwd: workspace_cwd.to_string(),
            execution_target,
            project_attachment: None,
            lifecycle_state: crate::conversation::ConversationLifecycleState::Ready,
            backend: crate::conversation::ConversationBackend::Agent,
            last_seq: 0,
            created_by: crate::conversation::ConversationCreator::Legacy,
            title: None,
            title_source: None,
        }
    }

    #[test]
    fn trusted_spawn_intent_derives_host_options() {
        let conversation_id =
            ConversationId::parse("018f7a1c-1b4d-7c8a-9f01-0123456789ab").unwrap();
        let record = conversation_record(
            conversation_id,
            "/host/workspace",
            ExecutionTarget::Worktree {
                project_id: "project-1".to_string(),
                worktree_path: "/host/worktree".to_string(),
                worktree_branch: "chat/test".to_string(),
            },
        );
        let intent = TerminalSpawnIntentV1 {
            conversation_id,
            project_id: Some("project-1".to_string()),
            cwd_source: TerminalCwdSource::ExecutionTarget,
            cols: 120,
            rows: 40,
        };

        let options = intent.into_trusted_options(&record).unwrap();
        assert_eq!(options.conversation_id, Some(conversation_id));
        assert_eq!(options.project_id.as_deref(), Some("project-1"));
        assert_eq!(options.cwd.as_deref(), Some("/host/worktree"));
        assert_eq!(options.cols, Some(120));
        assert_eq!(options.rows, Some(40));
        assert!(options.shell.is_none());
        assert!(options.program.is_none());
        assert!(options.args.is_none());
        assert!(options.env.is_none());
        assert!(options.kind.is_none());
        assert!(tracks_session_workspace_ref(&options));

        let wrong_project = TerminalSpawnIntentV1 {
            conversation_id,
            project_id: Some("project-other".to_string()),
            cwd_source: TerminalCwdSource::Workspace,
            cols: 80,
            rows: 24,
        };
        assert!(wrong_project.into_trusted_options(&record).is_err());
    }
}
