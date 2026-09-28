use std::collections::HashSet;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::json;
use uuid::Uuid;

use super::migration::MigrationMapV1;
use super::*;

const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";

type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
struct MatrixProvider {
    addressable: Mutex<bool>,
    suspended: Mutex<Vec<String>>,
    replacements: Mutex<Vec<String>>,
}

impl ConversationAgentLifecycle for MatrixProvider {
    fn owns_session<'a>(&'a self, _binding: &'a AgentSessionBinding) -> ProviderFuture<'a, bool> {
        Box::pin(async move { *self.addressable.lock() })
    }

    fn suspend<'a>(
        &'a self,
        binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async move {
            self.suspended.lock().push(binding.agent_session_id.clone());
            Ok(())
        })
    }

    fn replace<'a>(
        &'a self,
        previous_binding: &'a AgentSessionBinding,
        _prepared: &'a PreparedConversation,
        _target_runtime_agent_id: Option<&'a str>,
    ) -> ProviderFuture<'a, std::result::Result<AgentBindingResult, AgentLifecycleProviderError>>
    {
        Box::pin(async move {
            self.replacements
                .lock()
                .push(previous_binding.agent_session_id.clone());
            Ok(AgentBindingResult {
                agent_session_id: "opaque/replacement".to_string(),
                runtime_agent_id: "runtime-replacement".to_string(),
                stable_agent_namespace: "config:replacement".to_string(),
            })
        })
    }

    fn abort_replacement<'a>(
        &'a self,
        _binding: &'a AgentSessionBinding,
    ) -> ProviderFuture<'a, std::result::Result<(), AgentLifecycleProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn register_binding(&self, _agent_session_id: &str, _conversation_id: ConversationId) {}
}

#[derive(Default)]
struct MatrixTerminals(Mutex<HashSet<String>>);

impl TerminalResourceInspector for MatrixTerminals {
    fn is_live(&self, terminal_id: &str) -> bool {
        self.0.lock().contains(terminal_id)
    }

    fn terminate<'a>(
        &'a self,
        terminal_id: &'a str,
    ) -> ProviderFuture<'a, std::result::Result<(), String>> {
        self.0.lock().remove(terminal_id);
        Box::pin(async move { Ok(()) })
    }
}

struct MatrixFixture {
    _temp: tempfile::TempDir,
    repository: Arc<ConversationRepository>,
    workspace: Arc<SessionWorkspaceService>,
    application: Arc<ConversationApplicationService>,
    provider: Arc<MatrixProvider>,
    terminals: Arc<MatrixTerminals>,
    id: ConversationId,
}

fn fixed_time() -> DateTime<Utc> {
    parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap()
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.0
    }
}

struct FixedConversationId(ConversationId);

impl ConversationIdGenerator for FixedConversationId {
    fn generate(&self) -> ConversationId {
        self.0
    }
}

async fn fixture() -> MatrixFixture {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let private = base.join("state/conversations/v2");
    let visible = base.join("visible/sessions/2026/08/15").join(ID);
    std::fs::create_dir_all(&visible).unwrap();
    let (repository, report) = ConversationRepository::open(private.clone()).unwrap();
    assert_eq!(report.valid_conversation_count, 0);
    let writer = ConversationWriter::for_test(Arc::clone(&repository));
    let id = ConversationId::parse(ID).unwrap();
    let created_at = fixed_time();
    writer
        .create_conversation(
            ConversationRecordV2 {
                schema_version: CONVERSATION_SCHEMA_VERSION,
                conversation_id: id,
                created_at_utc: created_at,
                creation_partition: CreationPartition::from_created_at(created_at),
                workspace_cwd: visible.to_string_lossy().into_owned(),
                execution_target: ExecutionTarget::Workspace,
                project_attachment: None,
                lifecycle_state: ConversationLifecycleState::Ready,
                backend: crate::ConversationBackend::Agent,
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
                runtime_agent_id: "runtime-original".to_string(),
                stable_agent_namespace: "config:original".to_string(),
                execution_cwd: visible.to_string_lossy().into_owned(),
                bound_at_utc: created_at,
                state: AgentSessionBindingState::Active,
            },
            created_at,
        )
        .await
        .unwrap();

    let creation = Arc::new(
        ConversationCreationService::new(
            Arc::clone(&writer),
            ConversationLocator::new(private).unwrap(),
            SessionWorkspaceLocator::new(base.join("visible")).unwrap(),
        )
        .unwrap(),
    );
    let reader = Arc::new(ConversationReader::new(
        Arc::clone(&repository),
        LegacyConversationReader::default(),
        ReaderPrecedence::ConversationV2Only,
    ));
    let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
    let application = Arc::new(ConversationApplicationService::new(
        reader,
        Arc::clone(&writer),
        Arc::clone(&workspace),
        &MigrationMapV1 {
            schema_version: migration::MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            entries: Vec::new(),
        },
        MigrationHostMode::Standalone,
        MigrationPhase::Finalized,
        ReaderPrecedence::ConversationV2Only,
    ));
    let provider = Arc::new(MatrixProvider::default());
    *provider.addressable.lock() = true;
    let terminals = Arc::new(MatrixTerminals::default());
    application
        .attach_lifecycle(ConversationLifecycleService::new(
            Arc::clone(&writer),
            creation,
            provider.clone(),
            terminals.clone(),
        ))
        .unwrap();

    MatrixFixture {
        _temp: temp,
        repository,
        workspace,
        application,
        provider,
        terminals,
        id,
    }
}

fn revision(fixture: &MatrixFixture) -> u64 {
    fixture
        .repository
        .get_conversation(fixture.id)
        .unwrap()
        .last_seq
}

#[tokio::test]
async fn projectless_creation_is_durable_before_opaque_agent_binding() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let private = base.join("state/conversations/v2");
    let visible = base.join("visible");
    let (repository, report) = ConversationRepository::open(private.clone()).unwrap();
    assert_eq!(report.valid_conversation_count, 0);
    let writer = ConversationWriter::for_test(Arc::clone(&repository));
    let conversation_id = ConversationId::parse(ID).unwrap();
    let created_at = fixed_time();
    let service = Arc::new(
        ConversationCreationService::with_sources(
            writer,
            ConversationLocator::new(private.clone()).unwrap(),
            SessionWorkspaceLocator::new(visible).unwrap(),
            DurableFileSystem::new(),
            Arc::new(FixedClock(created_at)),
            Arc::new(FixedConversationId(conversation_id)),
        )
        .unwrap(),
    );
    let repository_at_gate = Arc::clone(&repository);

    let prepared = service
        .create_with_agent_gate(
            PrepareConversationRequest::new(ExecutionTarget::Workspace),
            move |prepared| async move {
                let canonical = repository_at_gate
                    .get_conversation(prepared.conversation_id)
                    .unwrap();
                assert_eq!(
                    canonical.lifecycle_state,
                    ConversationLifecycleState::InitializingAgent
                );
                assert!(std::path::Path::new(&prepared.workspace_cwd).is_dir());
                assert!(private
                    .join("2026/08/15")
                    .join(prepared.conversation_id.to_string())
                    .join(CONVERSATION_METADATA_FILE)
                    .is_file());
                assert!(repository_at_gate
                    .current_binding(prepared.conversation_id)
                    .unwrap()
                    .is_none());
                Ok(AgentBindingResult {
                    agent_session_id: "opaque/provider/session".to_string(),
                    runtime_agent_id: "runtime-agent".to_string(),
                    stable_agent_namespace: "config:provider".to_string(),
                })
            },
        )
        .await
        .unwrap();

    let completed = repository.get_conversation(conversation_id).unwrap();
    assert_eq!(prepared.conversation_id, conversation_id);
    assert_eq!(prepared.created_at_utc, format_created_at_utc(&created_at));
    assert_eq!(completed.conversation_id, conversation_id);
    assert_eq!(completed.created_at_utc, created_at);
    assert_eq!(completed.creation_partition.path, "2026/08/15");
    assert_eq!(completed.workspace_cwd, prepared.workspace_cwd);
    assert_eq!(completed.lifecycle_state, ConversationLifecycleState::Ready);
    let binding = repository
        .current_binding(conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding.agent_session_id, "opaque/provider/session");
    assert_ne!(binding.agent_session_id, conversation_id.to_string());
}

#[tokio::test]
async fn lifecycle_matrix() {
    let fixture = fixture().await;
    let identity = fixture.repository.get_conversation(fixture.id).unwrap();
    let cases = [
        ("detach", AgentSessionBindingState::Detached),
        ("rebind", AgentSessionBindingState::Active),
        ("suspend", AgentSessionBindingState::Suspended),
        ("replace", AgentSessionBindingState::Active),
    ];

    for (operation, expected_state) in cases {
        let expected_revision = revision(&fixture);
        let outcome = match operation {
            "detach" => {
                fixture
                    .application
                    .detach_binding(fixture.id, expected_revision)
                    .await
            }
            "rebind" => {
                fixture
                    .application
                    .rebind_binding(fixture.id, expected_revision)
                    .await
            }
            "suspend" => {
                fixture
                    .application
                    .suspend_binding(fixture.id, expected_revision)
                    .await
            }
            "replace" => {
                fixture
                    .application
                    .replace_binding(
                        fixture.id,
                        PrepareConversationRequest {
                            schema_version: 1,
                            conversation_id: Some(fixture.id),
                            project_attachment: None,
                            execution_target: ExecutionTarget::Workspace,
                            backend: ConversationBackend::Agent,
                        },
                        expected_revision,
                        None,
                    )
                    .await
            }
            _ => unreachable!(),
        }
        .unwrap();
        let ConversationLifecycleOutcome::Updated {
            conversation_id,
            revision: outcome_revision,
            current_binding,
            ..
        } = outcome
        else {
            panic!("{operation} must update")
        };
        assert_eq!(conversation_id, fixture.id, "operation={operation}");
        assert!(
            outcome_revision > expected_revision,
            "operation={operation}"
        );
        assert_eq!(
            current_binding.unwrap().state,
            expected_state,
            "operation={operation}"
        );
        let current = fixture.repository.get_conversation(fixture.id).unwrap();
        assert_eq!(current.conversation_id, identity.conversation_id);
        assert_eq!(current.created_at_utc, identity.created_at_utc);
        assert_eq!(current.creation_partition, identity.creation_partition);
        assert_eq!(current.workspace_cwd, identity.workspace_cwd);
    }

    assert_eq!(
        fixture.provider.suspended.lock().as_slice(),
        ["opaque/original"]
    );
    assert_eq!(
        fixture.provider.replacements.lock().as_slice(),
        ["opaque/original"]
    );

    fixture
        .workspace
        .add_terminal_ref(fixture.id, "terminal-live")
        .await
        .unwrap();
    fixture
        .terminals
        .0
        .lock()
        .insert("terminal-live".to_string());
    let events = fixture.repository.read_events(fixture.id, 0).unwrap();
    for expected in [
        ConversationEventType::BindingDetached,
        ConversationEventType::BindingRebound,
        ConversationEventType::BindingSuspended,
        ConversationEventType::BindingReplaced,
    ] {
        assert!(events.iter().any(|event| event.type_ == expected));
    }
    let deleted = fixture
        .application
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
    assert!(!fixture.terminals.is_live("terminal-live"));
    assert!(fixture.repository.get_conversation(fixture.id).is_err());
}

#[tokio::test]
async fn ready_aggregate_mutations_preserve_identity_workspace_and_revision_cas() {
    let fixture = fixture().await;
    let before = fixture.repository.get_conversation(fixture.id).unwrap();
    let project_root = fixture._temp.path().join("project-root");
    std::fs::create_dir_all(&project_root).unwrap();
    let project_root = project_root.canonicalize().unwrap();
    let attachment = ProjectAttachment {
        schema_version: PROJECT_ATTACHMENT_SCHEMA_VERSION,
        project_id: "project-validation".to_string(),
        attached_at_utc: parse_created_at_utc("2026-08-15T10:00:00.000Z").unwrap(),
        project_path_snapshot: project_root.to_string_lossy().into_owned(),
        worktree_path: None,
        worktree_branch: None,
    };

    let attached = fixture
        .application
        .attach_project(fixture.id, before.last_seq, attachment.clone())
        .await
        .unwrap();
    assert_eq!(
        attached.action,
        ConversationAggregateMutationAction::AttachProject
    );
    assert_eq!(attached.identity_before, attached.identity_after);
    assert_eq!(
        attached.identity_before.conversation_id,
        before.conversation_id
    );
    assert_eq!(
        attached.identity_before.created_at_utc,
        format_created_at_utc(&before.created_at_utc)
    );
    assert_eq!(
        attached.identity_before.creation_partition,
        before.creation_partition
    );
    assert_eq!(attached.identity_before.workspace_cwd, before.workspace_cwd);
    assert_eq!(attached.project_attachment, Some(attachment.clone()));

    let target = ExecutionTarget::ProjectRoot {
        project_id: attachment.project_id.clone(),
        project_root: attachment.project_path_snapshot.clone(),
    };
    let retargeted = fixture
        .application
        .update_execution_target(fixture.id, attached.revision, target.clone())
        .await
        .unwrap();
    assert_eq!(
        retargeted.action,
        ConversationAggregateMutationAction::UpdateExecutionTarget
    );
    assert_eq!(retargeted.execution_target, target);
    assert_eq!(retargeted.identity_before, retargeted.identity_after);

    let stale = fixture
        .application
        .detach_project(fixture.id, attached.revision)
        .await
        .unwrap_err();
    assert_eq!(stale.code, "CONVERSATION_CONFLICT");
    assert_eq!(
        fixture.repository.get_conversation(fixture.id).unwrap(),
        retargeted.conversation
    );

    let workspace = fixture
        .application
        .update_execution_target(fixture.id, retargeted.revision, ExecutionTarget::Workspace)
        .await
        .unwrap();
    let detached = fixture
        .application
        .detach_project(fixture.id, workspace.revision)
        .await
        .unwrap();
    assert_eq!(
        detached.action,
        ConversationAggregateMutationAction::DetachProject
    );
    assert!(detached.project_attachment.is_none());
    assert_eq!(detached.execution_target, ExecutionTarget::Workspace);
    assert_eq!(
        detached.conversation.conversation_id,
        before.conversation_id
    );
    assert_eq!(detached.conversation.created_at_utc, before.created_at_utc);
    assert_eq!(
        detached.conversation.creation_partition,
        before.creation_partition
    );
    assert_eq!(detached.conversation.workspace_cwd, before.workspace_cwd);
}

#[tokio::test]
async fn repository_catalog_workspace_and_application_recovery_matrix() {
    let fixture = fixture().await;
    let catalog_path = fixture.repository.root().join("catalog.json");
    fixture
        .repository
        .flush_catalog_until(tokio::time::Instant::now() + std::time::Duration::from_secs(2))
        .await
        .unwrap();
    let canonical_catalog = std::fs::read(&catalog_path).unwrap();
    std::fs::write(&catalog_path, b"not-json").unwrap();
    let (reopened, report) =
        ConversationRepository::open(fixture.repository.root().to_path_buf()).unwrap();
    assert_eq!(std::fs::read(&catalog_path).unwrap(), canonical_catalog);
    assert_eq!(reopened.list_conversations().len(), 1);
    assert!(report
        .recovery_items
        .iter()
        .any(|item| item.kind == RepositoryRecoveryKind::CatalogIgnored));

    let open = fixture
        .application
        .open_conversation(fixture.id)
        .await
        .unwrap();
    assert_eq!(open.conversation.conversation_id, fixture.id);
    assert!(matches!(
        open.workspace,
        SessionWorkspaceLoadOutcome::Missing { .. }
    ));

    let write = fixture
        .application
        .write_workspace(
            fixture.id,
            None,
            SessionWorkspaceV1 {
                schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                conversation_id: fixture.id,
                revision: 0,
                updated_at_utc: String::new(),
                update_identity: Some("validation-matrix".to_string()),
                topology: None,
                active_pane_id: None,
                resources: Vec::new(),
                projection_state: SessionWorkspaceProjectionState::Native,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        write,
        SessionWorkspaceWriteOutcome::Updated { revision: 1, .. }
    ));
    let conflict = fixture
        .application
        .write_workspace(
            fixture.id,
            None,
            SessionWorkspaceV1 {
                schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                conversation_id: fixture.id,
                revision: 0,
                updated_at_utc: String::new(),
                update_identity: Some("stale".to_string()),
                topology: None,
                active_pane_id: None,
                resources: Vec::new(),
                projection_state: SessionWorkspaceProjectionState::Native,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        conflict,
        SessionWorkspaceWriteOutcome::Conflict {
            current_revision: 1,
            ..
        }
    ));

    let workspace_path = fixture.repository.workspace_path(fixture.id).unwrap();
    std::fs::write(&workspace_path, b"{corrupt").unwrap();
    let preserved = std::fs::read(&workspace_path).unwrap();
    assert!(matches!(
        fixture.application.get_workspace(fixture.id).await.unwrap(),
        SessionWorkspaceLoadOutcome::RecoveryRequired { .. }
    ));
    assert_eq!(std::fs::read(workspace_path).unwrap(), preserved);
}

#[tokio::test]
async fn hybrid_legacy_first_full_mutation_matrix() {
    const NEW_ID: &str = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("conversations/v2");
    let (repository, _) = ConversationRepository::open(root).unwrap();
    let seed_writer = ConversationWriter::for_test(Arc::clone(&repository));
    let created_at = fixed_time();
    for (id, cwd) in [(ID, "/legacy"), (NEW_ID, "/new-v2")] {
        seed_writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id: ConversationId::parse(id).unwrap(),
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: cwd.to_string(),
                    execution_target: ExecutionTarget::Workspace,
                    project_attachment: None,
                    lifecycle_state: ConversationLifecycleState::Ready,
                    backend: crate::ConversationBackend::Agent,
                    last_seq: 0,
                    created_by: ConversationCreator::Legacy,
                    title: None,
                    title_source: None,
                },
                ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
    }

    let mapped = ConversationId::parse(ID).unwrap();
    let new_v2 = ConversationId::parse(NEW_ID).unwrap();
    let authority = Arc::new(ConversationWriteAuthority::new(
        repository.as_ref(),
        ReaderPrecedence::HybridLegacyFirst,
        [mapped],
    ));
    let writer = ConversationWriter::new(Arc::clone(&repository), authority).unwrap();

    for mutation in ConversationMutation::RUNTIME {
        let denied = writer.authorize(mapped, mutation).unwrap_err();
        assert_eq!(
            denied.code,
            ConversationErrorCode::LegacyCompatibilityReadOnly,
            "mapped legacy mutation {} must be denied",
            mutation.as_str()
        );
        assert!(
            writer.authorize(new_v2, mutation).is_ok(),
            "new v2-only mutation {} must remain writable",
            mutation.as_str()
        );
    }
    for mutation in [
        ConversationMutation::MigrationStageCreate,
        ConversationMutation::MigrationStageEvent,
        ConversationMutation::MigrationStageProvenance,
        ConversationMutation::MigrationStageSync,
    ] {
        assert_eq!(
            writer.authorize(new_v2, mutation).unwrap_err().code,
            ConversationErrorCode::ConversationRecoveryRequired
        );
    }
}

#[test]
fn migration_phase_and_shutdown_boundary_matrix() {
    use MigrationPhase::*;
    let transitions = [
        (Detected, Quiescing),
        (Quiescing, Inventoried),
        (Inventoried, Staging),
        (Staging, Verifying),
        (Verifying, CutoverPending),
        (CutoverPending, Committed),
        (Committed, ObservationWindow),
    ];
    let mut journal = MigrationJournalV1::new("a".repeat(64), fixed_time());
    for (current, next) in transitions {
        assert_eq!(journal.phase, current);
        advance_phase(&mut journal, next, fixed_time(), false).unwrap();
    }

    let evidence = json!({
        "conversationId": ID,
        "operationId": journal.operation_id,
        "phase": journal.phase,
        "workspaceRevision": 1,
        "terminalId": "terminal-live",
        "transport": "matrix"
    });
    assert_eq!(evidence["phase"], "observation_window");
}

#[test]
fn large_corpus_bootstrap_retains_zero_historical_payload_bytes() {
    const CONVERSATION_COUNT: u64 = 100;
    const EVENTS_PER_CONVERSATION: u64 = 10_000;

    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap().join("large-corpus");
    std::fs::create_dir_all(&directory).unwrap();
    for file in EVENT_LOG_FILES {
        std::fs::write(directory.join(file), b"").unwrap();
    }
    let source_id = ConversationId::parse("20000000-0000-4000-8000-000000000000").unwrap();
    let recorded_at_utc = fixed_time();
    let messages = std::fs::File::create(directory.join(MESSAGES_FILE)).unwrap();
    let mut messages = BufWriter::new(messages);
    for seq in 1..=EVENTS_PER_CONVERSATION {
        let event = ConversationEventRecordV2::new(
            source_id,
            seq,
            recorded_at_utc,
            ConversationEventType::MessageChunk,
            serde_json::Value::Null,
        );
        serde_json::to_writer(&mut messages, &event).unwrap();
        messages.write_all(b"\n").unwrap();
    }
    messages.flush().unwrap();
    drop(messages);

    let scan =
        crate::event_log::scan_event_log(&directory, source_id, &DurableFileSystem::new()).unwrap();
    // Retained-memory accounting depends on the compact post-validation state, not repeated parse
    // CPU. Validate one real 10,000-event stream, then materialize 100 disjoint bootstrap states.
    let compact_states = (0..CONVERSATION_COUNT)
        .map(|index| {
            (
                ConversationId::parse(&format!("20000000-0000-4000-8000-{index:012x}")).unwrap(),
                scan.clone(),
            )
        })
        .collect::<Vec<_>>();
    let metrics =
        crate::repository::bootstrap_scan_metrics(compact_states.iter().map(|(_, scan)| scan));
    let expected_offsets_per_stream =
        EVENTS_PER_CONVERSATION.div_ceil(crate::event_log::SPARSE_OFFSET_STRIDE);

    assert_eq!(compact_states.len(), CONVERSATION_COUNT as usize);
    assert_eq!(
        compact_states
            .iter()
            .map(|(conversation_id, _)| *conversation_id)
            .collect::<HashSet<_>>()
            .len(),
        CONVERSATION_COUNT as usize
    );
    assert_eq!(
        metrics.scanned_event_count,
        CONVERSATION_COUNT * EVENTS_PER_CONVERSATION
    );
    assert_eq!(metrics.retained_payload_bytes, 0);
    assert_eq!(
        metrics.sparse_index_entry_count,
        (CONVERSATION_COUNT * expected_offsets_per_stream) as usize
    );
    for (_, scan) in compact_states {
        assert_eq!(
            scan.sparse_offsets.messages.event_count,
            EVENTS_PER_CONVERSATION
        );
        assert!(scan.sparse_offsets.messages.entries.len() <= expected_offsets_per_stream as usize);
        assert!(scan.sparse_offsets.tool_calls.entries.is_empty());
        assert!(scan.sparse_offsets.bindings.entries.is_empty());
        assert!(scan.sparse_offsets.attachments.entries.is_empty());
    }
}
