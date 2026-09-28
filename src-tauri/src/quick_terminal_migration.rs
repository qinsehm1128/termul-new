//! Move legacy terminal-backed Conversations into quick terminals.
//!
//! Non-destructive: the Conversation stays on disk untouched and agent session
//! surfaces stop listing terminal-backed ones. The quick terminal keeps the
//! same id, the same folder (a terminal Conversation's shell always ran in its
//! own workspace), and the shell that is still running, so nothing the user
//! had open is lost. Idempotent: an id that already has a quick terminal is
//! skipped, so an interrupted run simply resumes on the next start.

use async_trait::async_trait;
use chrono::SecondsFormat;
use std::sync::Arc;

use crate::conversation::{
    ConversationApplicationService, ConversationBackend, ConversationId,
    ConversationLifecycleState, ConversationRecordV2, SessionWorkspaceLoadOutcome,
    SessionWorkspaceResourceDescriptor,
};
use crate::core::{AcpCoreClient, TerminalServiceHandle};
use crate::quick_terminal::{QuickTerminalRecord, METHOD_IMPORT};
use crate::quick_terminal_commands::request;
use se_quick_terminal::{QuickTerminalOrigin, QuickTerminalTarget, QUICK_TERMINAL_SCHEMA_VERSION};

/// Where legacy Conversations are read from: Agent Core over IPC, or the
/// in-process application service.
#[async_trait]
pub trait LegacyConversations: Send + Sync {
    async fn list(&self) -> Result<Vec<ConversationRecordV2>, String>;
    async fn workspace(&self, id: ConversationId) -> Result<SessionWorkspaceLoadOutcome, String>;
}

pub struct AcpCoreConversations(pub Arc<AcpCoreClient>);

#[async_trait]
impl LegacyConversations for AcpCoreConversations {
    async fn list(&self) -> Result<Vec<ConversationRecordV2>, String> {
        let value = self
            .0
            .request("conversationList", serde_json::Value::Null)
            .await
            .map_err(|error| error.command_message())?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    async fn workspace(&self, id: ConversationId) -> Result<SessionWorkspaceLoadOutcome, String> {
        let value = self
            .0
            .request(
                "conversationGetWorkspace",
                serde_json::json!({ "conversationId": id.to_string() }),
            )
            .await
            .map_err(|error| error.command_message())?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }
}

pub struct LocalConversations(pub Arc<ConversationApplicationService>);

#[async_trait]
impl LegacyConversations for LocalConversations {
    async fn list(&self) -> Result<Vec<ConversationRecordV2>, String> {
        Ok(self.0.list_conversations())
    }

    async fn workspace(&self, id: ConversationId) -> Result<SessionWorkspaceLoadOutcome, String> {
        self.0
            .get_workspace(id)
            .await
            .map_err(|error| error.to_string())
    }
}

/// True for a Conversation that should live on as a quick terminal.
pub fn is_legacy_terminal(conversation: &ConversationRecordV2) -> bool {
    conversation.backend == ConversationBackend::Terminal
        && conversation.lifecycle_state != ConversationLifecycleState::Deleted
}

/// The quick terminal a legacy terminal Conversation becomes. `terminal_ids`
/// are its workspace's terminal refs; the newest is the shell to reattach.
pub fn quick_terminal_from_conversation(
    conversation: &ConversationRecordV2,
    terminal_ids: &[String],
) -> QuickTerminalRecord {
    let created = conversation
        .created_at_utc
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    QuickTerminalRecord {
        schema_version: QUICK_TERMINAL_SCHEMA_VERSION,
        id: conversation.conversation_id,
        title: conversation.title.clone(),
        target: QuickTerminalTarget::Workspace,
        cwd: conversation.workspace_cwd.clone(),
        created_at_utc: created.clone(),
        updated_at_utc: created,
        terminal_id: terminal_ids.iter().max().cloned(),
        origin: QuickTerminalOrigin::MigratedConversation,
    }
}

fn terminal_ids(outcome: SessionWorkspaceLoadOutcome) -> Vec<String> {
    match outcome {
        SessionWorkspaceLoadOutcome::Loaded { workspace } => workspace
            .resources
            .into_iter()
            .filter_map(|resource| match resource {
                SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. } => {
                    Some(terminal_id)
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MigrationReport {
    pub imported: usize,
    pub already_present: usize,
    pub failed: usize,
}

/// Import every legacy terminal Conversation that has no quick terminal yet.
pub async fn migrate(
    source: &dyn LegacyConversations,
    terminal: &TerminalServiceHandle,
) -> MigrationReport {
    let mut report = MigrationReport::default();
    let conversations = match source.list().await {
        Ok(conversations) => conversations,
        Err(error) => {
            log::error!(
                target: "se_manager::quick_terminal",
                "operation=migrate stable_code=LEGACY_LIST_FAILED error={error}"
            );
            report.failed = 1;
            return report;
        }
    };
    for conversation in conversations.iter().filter(|c| is_legacy_terminal(c)) {
        let id = conversation.conversation_id;
        // A workspace that cannot be read only costs the running-shell handoff;
        // the quick terminal itself is still created.
        let ids = match source.workspace(id).await {
            Ok(outcome) => terminal_ids(outcome),
            Err(error) => {
                log::warn!(
                    target: "se_manager::quick_terminal",
                    "operation=migrate id={id} stable_code=LEGACY_WORKSPACE_UNREADABLE error={error}"
                );
                Vec::new()
            }
        };
        let record = quick_terminal_from_conversation(conversation, &ids);
        let result: crate::commands::IpcResult<bool> =
            request(terminal, METHOD_IMPORT, &record).await;
        match (result.success, result.data) {
            (true, Some(true)) => report.imported += 1,
            (true, Some(false)) => report.already_present += 1,
            _ => {
                report.failed += 1;
                log::error!(
                    target: "se_manager::quick_terminal",
                    "operation=migrate id={id} stable_code={}",
                    result.code.as_deref().unwrap_or("QUICK_TERMINAL_IMPORT_FAILED")
                );
            }
        }
    }
    log::info!(
        target: "se_manager::quick_terminal",
        "operation=migrate imported={} already_present={} failed={} stable_code=OK",
        report.imported,
        report.already_present,
        report.failed
    );
    report
}

/// Renderer event: quick terminals were added outside the renderer's own calls.
pub const QUICK_TERMINALS_CHANGED_EVENT: &str = "quick-terminals-changed";

/// Run [`migrate`] off the setup thread; tell the renderer when records appear.
pub fn spawn_startup_migration(
    app: tauri::AppHandle,
    source: Arc<dyn LegacyConversations>,
    terminal: TerminalServiceHandle,
) {
    use tauri::Emitter;
    tauri::async_runtime::spawn(async move {
        let report = migrate(source.as_ref(), &terminal).await;
        if report.imported > 0 {
            if let Err(error) = app.emit(QUICK_TERMINALS_CHANGED_EVENT, ()) {
                log::warn!(
                    target: "se_manager::quick_terminal",
                    "operation=migrate stable_code=EVENT_EMIT_FAILED error={error}"
                );
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::conversation::{
        parse_created_at_utc, ConversationCreator, CreationPartition, ExecutionTarget,
        SessionWorkspaceProjectionState, SessionWorkspaceV1, CONVERSATION_SCHEMA_VERSION,
    };
    use crate::quick_terminal::{with_local_service, METHOD_LIST};

    struct Fixture(Vec<ConversationRecordV2>);

    #[async_trait]
    impl LegacyConversations for Fixture {
        async fn list(&self) -> Result<Vec<ConversationRecordV2>, String> {
            Ok(self.0.clone())
        }

        async fn workspace(
            &self,
            id: ConversationId,
        ) -> Result<SessionWorkspaceLoadOutcome, String> {
            let terminal = |terminal_id: &str| SessionWorkspaceResourceDescriptor::Terminal {
                terminal_id: terminal_id.to_string(),
                terminal_record_id: None,
                conversation_id: id,
            };
            Ok(SessionWorkspaceLoadOutcome::Loaded {
                workspace: Box::new(SessionWorkspaceV1 {
                    schema_version: crate::conversation::SESSION_WORKSPACE_SCHEMA_VERSION,
                    conversation_id: id,
                    revision: 1,
                    updated_at_utc: "2026-09-08T00:00:00.000Z".to_string(),
                    update_identity: None,
                    topology: None,
                    active_pane_id: None,
                    resources: vec![terminal("1790562967946"), terminal("1790563536869")],
                    projection_state: SessionWorkspaceProjectionState::Native,
                }),
            })
        }
    }

    fn conversation(
        backend: ConversationBackend,
        lifecycle_state: ConversationLifecycleState,
    ) -> ConversationRecordV2 {
        let created = parse_created_at_utc("2026-09-08T02:36:07.978Z").unwrap();
        ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::new_v4(),
            created_at_utc: created,
            creation_partition: CreationPartition::from_created_at(created),
            workspace_cwd: "/docs/Se/sessions/2026/09/08/x".to_string(),
            execution_target: ExecutionTarget::ProjectRoot {
                project_id: "p1".to_string(),
                project_root: "/work/p1".to_string(),
            },
            project_attachment: None,
            lifecycle_state,
            backend,
            last_seq: 3,
            created_by: ConversationCreator::Legacy,
            title: Some("deploy box".to_string()),
            title_source: None,
        }
    }

    #[tokio::test]
    async fn terminal_conversations_become_quick_terminals_once() {
        let profile_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let terminal = with_local_service(
            TerminalServiceHandle::in_process(crate::pty::test_pty_manager()),
            &profile_dir.path().canonicalize().unwrap(),
            &workspace_dir.path().canonicalize().unwrap(),
        );
        let legacy = conversation(
            ConversationBackend::Terminal,
            ConversationLifecycleState::Ready,
        );
        let source = Fixture(vec![
            legacy.clone(),
            conversation(
                ConversationBackend::Agent,
                ConversationLifecycleState::Ready,
            ),
            conversation(
                ConversationBackend::Terminal,
                ConversationLifecycleState::Deleted,
            ),
        ]);

        let first = migrate(&source, &terminal).await;
        assert_eq!(
            first,
            MigrationReport {
                imported: 1,
                already_present: 0,
                failed: 0
            }
        );
        let listed: crate::commands::IpcResult<Vec<QuickTerminalRecord>> =
            request(&terminal, METHOD_LIST, &serde_json::Value::Null).await;
        let records = listed.data.unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.id, legacy.conversation_id);
        assert_eq!(record.title.as_deref(), Some("deploy box"));
        // The shell always ran in the Conversation's own folder.
        assert_eq!(record.target, QuickTerminalTarget::Workspace);
        assert_eq!(record.cwd, legacy.workspace_cwd);
        assert_eq!(record.terminal_id.as_deref(), Some("1790563536869"));
        assert_eq!(record.origin, QuickTerminalOrigin::MigratedConversation);

        let second = migrate(&source, &terminal).await;
        assert_eq!(
            second,
            MigrationReport {
                imported: 0,
                already_present: 1,
                failed: 0
            }
        );
    }
}
