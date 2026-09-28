//! Tests of the Conversation domain through the host: its ACP manager,
//! terminal commands, command adapters, and startup order.

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use crate::conversation::migration::MigrationHostMode;
use crate::conversation::test_support::{application_fixture as fixture, seed_recovery, ID};
use crate::conversation::write_authority::ConversationMutation;
use crate::conversation::{
    BootstrapOutcome, ConversationBootstrap, ConversationCreator, ConversationHostState,
    ConversationId, ConversationLifecycleState, ConversationRecordV2, CreationPartition,
    ExecutionTarget, HostConversationRoots, SessionWorkspaceLoadOutcome,
    SessionWorkspaceResourceDescriptor, CONVERSATION_SCHEMA_VERSION,
};

fn fixed_time() -> chrono::DateTime<chrono::Utc> {
    crate::conversation::parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap()
}

fn strip_rust_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let mut in_block = false;
    while let Some(character) = characters.next() {
        if in_block {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                in_block = false;
            }
            continue;
        }
        if character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            in_block = true;
            continue;
        }
        if character == '/' && characters.peek() == Some(&'/') {
            characters.next();
            for line_character in characters.by_ref() {
                if line_character == '\n' {
                    output.push('\n');
                    break;
                }
            }
            continue;
        }
        output.push(character);
    }
    output
}

#[test]
fn bootstrap_precedes_all_mutable_store_opens() {
    let desktop_source = strip_rust_comments(include_str!("lib.rs"));
    let desktop_setup = desktop_source.find(".setup(|app|").unwrap();
    let desktop = &desktop_source[desktop_setup..];
    let desktop_bootstrap = desktop.find("ConversationBootstrap::run").unwrap();
    for forbidden in [
        "app.manage(",
        "PtyManager::new",
        "ChatHistoryStore::open_read_only",
        "WorkspaceManifestService::open_read_only",
        "AcpCatalogService::open",
        "AcpManager::with_conversation_services",
        "RemoteServerState::with_desktop_authority",
    ] {
        let position = desktop.find(forbidden).unwrap();
        assert!(
            desktop_bootstrap < position,
            "desktop bootstrap must precede {forbidden}"
        );
    }
    let before_desktop_bootstrap = &desktop[..desktop_bootstrap];
    for forbidden_root_access in [
        "SessionPersistence::open",
        "ChatHistoryStore::open",
        "WorkspaceManifestService::open",
        "acp-sessions",
        "acp-chat-history",
        "workspace-manifests",
        "conversations/v2",
    ] {
        assert!(
            !before_desktop_bootstrap.contains(forbidden_root_access),
            "desktop plugin/config setup must not access {forbidden_root_access} before bootstrap"
        );
    }

    let standalone = strip_rust_comments(include_str!("server_main.rs"));
    let standalone_bootstrap = standalone.find("ConversationBootstrap::run").unwrap();
    for forbidden in [
        "WorkspaceManifestService::open_read_only",
        "AcpCatalogService::open",
        "FileProjectRegistry::load",
        "AcpManager::with_conversation_services",
        "PtyManager::new",
        "match serve(",
    ] {
        let position = standalone.find(forbidden).unwrap();
        assert!(
            standalone_bootstrap < position,
            "standalone bootstrap must precede {forbidden}"
        );
    }
    let before_standalone_bootstrap = &standalone[..standalone_bootstrap];
    for forbidden_store in [
        "SessionPersistence::open",
        "WorkspaceManifestService::open",
        "AcpCatalogService::open",
        "FileProjectRegistry::load",
        "AcpManager::",
        "PtyManager::new",
        "serve(",
    ] {
        assert!(
            !before_standalone_bootstrap.contains(forbidden_store),
            "standalone must not admit {forbidden_store} before bootstrap"
        );
    }
}

#[tokio::test]
async fn projectless_conversation_exists_before_acp_new() {
    let temp = tempfile::tempdir().unwrap();
    let bootstrap = ConversationBootstrap::run(
        HostConversationRoots::new(temp.path().join("state"), temp.path().join("visible")),
        MigrationHostMode::Desktop,
    )
    .unwrap();
    let manager = Arc::new(crate::AcpManager::with_conversation_services(
        Vec::new(),
        Arc::clone(&bootstrap.creation),
        Arc::clone(&bootstrap.persistence_adapter),
    ));
    let agent_id = crate::acp::AgentId("fake-agent".to_string());
    let (observed_tx, observed_rx) = std::sync::mpsc::sync_channel(1);
    manager.install_test_agent_for_new_session(agent_id.clone(), observed_tx);

    let created = manager
        .new_session_with_context(
            &agent_id,
            temp.path()
                .join("ignored-cwd")
                .to_string_lossy()
                .into_owned(),
            Vec::new(),
            crate::acp::SessionCreationContext {
                execution_target: Some(crate::conversation::ExecutionTarget::Workspace),
                ..crate::acp::SessionCreationContext::default()
            },
        )
        .await
        .unwrap();
    let (execution_cwd_seen_by_acp, existed_before_acp) = observed_rx.recv().unwrap();
    assert!(existed_before_acp);
    assert_eq!(created.persistence, "conversation");
    assert_eq!(
        created.execution_cwd.as_deref(),
        Some(execution_cwd_seen_by_acp.as_str())
    );
    assert_eq!(created.workspace_cwd, created.execution_cwd);
    let conversation_id = created.conversation_id.unwrap();
    assert!(bootstrap
        .repository
        .get_conversation(conversation_id)
        .is_ok());
    assert_eq!(
        bootstrap
            .repository
            .current_binding(conversation_id)
            .unwrap()
            .unwrap()
            .agent_session_id,
        "opaque/fake-session"
    );
}

#[tokio::test]
async fn binding_and_close_failures_return_safe_compound_receipt_and_persist_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let bootstrap = ConversationBootstrap::run(
        HostConversationRoots::new(state.clone(), temp.path().join("visible")),
        MigrationHostMode::Desktop,
    )
    .unwrap();
    bootstrap.repository.fail_next_agent_binding_appends(2);
    let manager = Arc::new(crate::AcpManager::with_conversation_services(
        Vec::new(),
        Arc::clone(&bootstrap.creation),
        Arc::clone(&bootstrap.persistence_adapter),
    ));
    let agent_id = crate::acp::AgentId("fake-agent".to_string());
    let (observed_tx, _observed_rx) = std::sync::mpsc::sync_channel(1);
    manager.install_test_agent_for_new_session_with_close_result(
        agent_id.clone(),
        observed_tx,
        Err("provider close leaked SUPER_SECRET=do-not-return".to_string()),
    );

    let error = manager
        .new_session_with_context(
            &agent_id,
            temp.path()
                .join("ignored-cwd")
                .to_string_lossy()
                .into_owned(),
            Vec::new(),
            crate::acp::SessionCreationContext {
                execution_target: Some(crate::conversation::ExecutionTarget::Workspace),
                ..crate::acp::SessionCreationContext::default()
            },
        )
        .await
        .unwrap_err();
    let failure = crate::conversation::AgentCompensationFailure::from_wire_error(&error)
        .expect("compound failure must use the stable wire receipt");
    assert_eq!(failure.primary_code, "CONVERSATION_BIND_FAILED");
    assert_eq!(
        failure.provider_close_code.as_deref(),
        Some("ACP_CLOSE_FAILED")
    );
    assert_eq!(
        failure.failure_record_code.as_deref(),
        Some("CONVERSATION_DURABILITY_FAILED")
    );
    assert!(failure.recovery_marker_code.is_none());
    assert!(failure.recovery_record_code.is_none());
    assert!(failure.recovery_id.is_some());
    assert!(!error.contains("SUPER_SECRET"));
    assert!(!error.contains("opaque/fake-session"));

    let record = bootstrap
        .repository
        .list_conversations()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(record.conversation_id, failure.conversation_id);
    assert_eq!(
        record.lifecycle_state,
        crate::conversation::ConversationLifecycleState::RecoveryRequired
    );
    assert!(bootstrap
        .repository
        .current_binding(record.conversation_id)
        .unwrap()
        .is_none());

    let recovery_bytes = fs::read(
        state
            .join("conversation-migrations")
            .join("workspace-recovery-v1")
            .join(crate::conversation::migration::RECOVERY_ITEMS_FILE),
    )
    .unwrap();
    let recovery: crate::conversation::migration::RecoveryQueueV1 =
        serde_json::from_slice(&recovery_bytes).unwrap();
    assert_eq!(recovery.items.len(), 1);
    let serialized = String::from_utf8(recovery_bytes).unwrap();
    assert!(!serialized.contains("SUPER_SECRET"));
    assert!(!serialized.contains("opaque/fake-session"));
    assert!(serialized.contains("acpCompensationFailed"));
}

#[tokio::test]
async fn tauri_command_inners_preserve_legacy_and_recovery_golden_envelopes() {
    let (_temp, repository, service) = fixture().await;
    for (source_kind, value) in [
        ("legacyStorageKey", "legacy-storage"),
        ("legacyAgentSessionId", "opaque-agent-session"),
        ("legacyChatHistoryId", "chat-history"),
    ] {
        let result = crate::commands::conversation_resolve_legacy_id_inner(
            &service,
            serde_json::json!({"sourceKind":source_kind,"value":value}),
        );
        assert!(result.success, "{source_kind}: {:?}", result.error);
        assert_eq!(result.data.unwrap().canonical_route, format!("#/c/{ID}"));
    }
    let missing = crate::commands::conversation_resolve_legacy_id_inner(
        &service,
        serde_json::json!({"sourceKind":"legacyStorageKey","value":"missing"}),
    );
    assert_eq!(missing.code.as_deref(), Some("CONVERSATION_NOT_FOUND"));

    let item = seed_recovery(&repository);
    let result = crate::commands::conversation_recovery_resolve_inner(
        &service,
        serde_json::json!({
            "recoveryId":item.recovery_id,
            "expectedRevision":item.revision,
            "action":"inspect",
            "payload":{}
        }),
    )
    .await;
    assert!(result.success, "inspect: {:?}", result.error);
    let result = result.data.unwrap();
    assert_eq!(serde_json::to_value(result.action).unwrap(), "inspect");
    assert_eq!(result.source_paths, item.source_paths);
    assert_eq!(result.source_sha256, item.source_sha256);
    assert_eq!(
        service.host_status().unwrap().state,
        ConversationHostState::Recovery
    );

    let associated = crate::commands::conversation_recovery_resolve_inner(
        &service,
        serde_json::json!({
            "recoveryId":item.recovery_id,
            "expectedRevision":item.revision,
            "idempotencyKey":"21aee10a-56b8-4624-a5e7-586c25dc8d1f",
            "action":"associateConversation",
            "payload":{"conversationId":ID}
        }),
    )
    .await;
    assert!(associated.success, "associate: {:?}", associated.error);
    assert_eq!(
        service.host_status().unwrap().state,
        ConversationHostState::Ready
    );
}

#[tokio::test]
async fn terminal_service_graph_is_host_exact() {
    const HOST_A_ID: &str = "11111111-1111-4111-8111-111111111111";
    const HOST_B_ID: &str = "22222222-2222-4222-8222-222222222222";

    async fn add_conversation(
        bootstrap: &BootstrapOutcome,
        id: &str,
        label: &str,
    ) -> ConversationId {
        let conversation_id = ConversationId::parse(id).unwrap();
        let created_at = fixed_time();
        let workspace_cwd = bootstrap.workspace_base.join(label);
        std::fs::create_dir_all(&workspace_cwd).unwrap();
        bootstrap
            .writer
            .create_conversation(
                ConversationRecordV2 {
                    schema_version: CONVERSATION_SCHEMA_VERSION,
                    conversation_id,
                    created_at_utc: created_at,
                    creation_partition: CreationPartition::from_created_at(created_at),
                    workspace_cwd: workspace_cwd.to_string_lossy().into_owned(),
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
        conversation_id
    }

    fn terminal_ids(outcome: SessionWorkspaceLoadOutcome) -> HashSet<String> {
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = outcome else {
            panic!("terminal workspace must be loaded")
        };
        workspace
            .resources
            .into_iter()
            .filter_map(|resource| match resource {
                SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. } => {
                    Some(terminal_id)
                }
                _ => None,
            })
            .collect()
    }

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let host_a = ConversationBootstrap::run(
        HostConversationRoots::new(base.join("state-a"), base.join("visible-a")),
        MigrationHostMode::Desktop,
    )
    .unwrap();
    let host_b = ConversationBootstrap::run(
        HostConversationRoots::new(base.join("state-b"), base.join("visible-b")),
        MigrationHostMode::Desktop,
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &host_a.workspace,
        &host_a.application.session_workspace()
    ));
    assert!(Arc::ptr_eq(
        &host_b.workspace,
        &host_b.application.session_workspace()
    ));
    assert!(!Arc::ptr_eq(&host_a.workspace, &host_b.workspace));

    let id_a = add_conversation(&host_a, HOST_A_ID, "host-a").await;
    let id_b = add_conversation(&host_b, HOST_B_ID, "host-b").await;
    let pty_a = crate::web::test_pty_manager();
    let pty_b = crate::web::test_pty_manager();
    let spawned_a = crate::commands::terminal_spawn_resource(
        crate::pty::SpawnOptions {
            conversation_id: Some(id_a),
            cwd: Some(host_a.workspace_base.to_string_lossy().into_owned()),
            ..Default::default()
        },
        None,
        &pty_a,
        &host_a.workspace,
    )
    .await;
    assert!(spawned_a.success, "host A spawn: {:?}", spawned_a.error);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let spawned_b = crate::commands::terminal_spawn_resource(
        crate::pty::SpawnOptions {
            conversation_id: Some(id_b),
            cwd: Some(host_b.workspace_base.to_string_lossy().into_owned()),
            ..Default::default()
        },
        None,
        &pty_b,
        &host_b.workspace,
    )
    .await;
    assert!(spawned_b.success, "host B spawn: {:?}", spawned_b.error);
    let terminal_a = spawned_a.data.unwrap().info.id;
    let terminal_b = spawned_b.data.unwrap().info.id;
    assert_ne!(terminal_a, terminal_b);

    let refs_a = terminal_ids(host_a.workspace.load(id_a).await.unwrap());
    let refs_b = terminal_ids(host_b.workspace.load(id_b).await.unwrap());
    assert_eq!(refs_a, HashSet::from([terminal_a.clone()]));
    assert_eq!(refs_b, HashSet::from([terminal_b.clone()]));
    assert!(!refs_a.contains(&terminal_b));
    assert!(!refs_b.contains(&terminal_a));

    assert!(
        crate::commands::terminal_terminate_resource(&terminal_a, &pty_a, &host_a.workspace,)
            .await
            .success
    );
    assert!(
        crate::commands::terminal_terminate_resource(&terminal_b, &pty_b, &host_b.workspace,)
            .await
            .success
    );
}

/// Terminal disconnects never kill shells, and the standalone host gates
/// startup on maintenance control and Conversation bootstrap.
#[test]
fn host_shutdown_and_startup_boundaries() {
    let terminal_ws = include_str!("web/terminal_ws.rs");
    let disconnect = terminal_ws
        .split("async fn run")
        .nth(1)
        .and_then(|tail| tail.split("struct ConnectionContext").next())
        .unwrap();
    for forbidden in [".terminate(", ".kill(", "kill_all"] {
        assert!(
            !disconnect.contains(forbidden),
            "disconnect contains {forbidden}"
        );
    }
    let standalone = include_str!("server_main.rs");
    let maintenance_gate = standalone
        .find("if let Some(maintenance) = maintenance {")
        .expect("standalone maintenance control must gate normal startup");
    let bootstrap_gate = standalone
        .find("ConversationBootstrap::run(")
        .expect("standalone startup must run Conversation bootstrap");
    let network_admission = standalone
        .find("match serve(")
        .expect("standalone startup must enter the network server through serve");
    assert!(
        maintenance_gate < bootstrap_gate && bootstrap_gate < network_admission,
        "maintenance control and Conversation bootstrap must complete before network/router admission"
    );

    let remote = include_str!("remote/host.rs");
    let production = remote
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("desktop shared-live production source");
    assert!(production.contains("serve_router("));
    assert!(!production.contains("kill_all_checked"));
}
