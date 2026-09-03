//! Standalone headless `se-server` binary (Story 1.2).
//!
//! Constructs an [`AcpManager`] with a [`WsRelaySink`] stub (no Tauri
//! `AppHandle`) and serves the Axum skeleton from `termul_manager_lib::web`.
//! Live WS relay + static-embed serving are wired via the shared `web` module.
//!
//! Path is intentionally **outside** `src/bin/` so Tauri's bundler stage-2
//! disk scan (tauri#15325) does not re-add this target into the desktop
//! app bundle. Cargo still builds it via the explicit `[[bin]]` path +
//! `required-features = ["standalone-server"]`.
//!
//! This is a CONSOLE server — do NOT add `windows_subsystem = "windows"`.

use std::path::PathBuf;
use std::process::ExitCode;

use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use termul_manager_lib::server_update::{
    check_and_apply_update, current_version, embedded_public_key, is_update_enabled,
    restart_binary, restore_previous, UpdateChannel, UpdateOptions, UpdateOutcome,
    SERVER_PLATFORM_KEY,
};
use termul_manager_lib::web::config::{ParseCliError, REMOTE_AUTH_CONFIGURATION_REQUIRED};
use termul_manager_lib::web::{
    seed_from_file, serve, PermissionRendezvous, ProjectRegistry, QuestionRendezvous,
    RemoteAccessAuthority, ServerConfig, WsRelaySink,
};
use termul_manager_lib::{
    AcpCatalogService, AcpInstallService, AcpManager, CwdTracker, ExitCodeTracker,
    FileProjectRegistry, GitTracker, PtyManager, TerminalEventHub, WorkspaceManifestService,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn provision_standalone_authority(
    cfg: &ServerConfig,
) -> Result<Arc<RemoteAccessAuthority>, &'static str> {
    let token_file = cfg
        .remote_access_token_file
        .as_deref()
        .ok_or(REMOTE_AUTH_CONFIGURATION_REQUIRED)?;
    if cfg.allowed_origins.is_empty() {
        return Err(REMOTE_AUTH_CONFIGURATION_REQUIRED);
    }
    let authority =
        RemoteAccessAuthority::from_token_file(token_file).map_err(|error| error.code())?;
    authority
        .set_allowed_origins(cfg.allowed_origins.clone())
        .map_err(|error| error.code())?;
    Ok(Arc::new(authority))
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    // `--check-update`: operator-explicit one-shot self-update. Handled before
    // `ServerConfig::from_args` so the flag (unknown to the shared parser) does
    // not trip it. Performs fetch → verify → swap → reexec, then exits.
    if raw_args.iter().any(|arg| arg == "--check-update") {
        init_tracing();
        return run_one_shot_update_check();
    }

    // `--internal-mcp-plan-server`: self-spawned child of the host-injected
    // `plan` MCP tool. The agent spawns `current_exe()` with this flag
    // (the injected `McpServer::Stdio`); the child runs an rmcp MCP server over
    // stdio + forwards calls to the parent's TCP listener. Branch BEFORE any
    // tokio/app setup (AC2) so the standalone binary never inits the server
    // stack for the child path. See `acp::host_mcp::child` + spec
    // `spec-acp-host-todo-plan-tool.md`.
    if termul_manager_lib::host_mcp::is_child_invocation() {
        return ExitCode::from(termul_manager_lib::host_mcp::child::run() as u8);
    }

    let (server_args, maintenance) = match parse_conversation_maintenance_args(&raw_args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!();
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    // Parse CLI BEFORE any tokio / app setup (AC2). Maintenance selects the
    // minimal state/root parser and therefore never depends on network auth.
    let cfg = match if maintenance.is_some() {
        ServerConfig::from_maintenance_args(server_args)
    } else {
        ServerConfig::from_args(server_args)
    } {
        Ok(cfg) => cfg,
        Err(ParseCliError::Help) => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(ParseCliError::Message(msg)) => {
            eprintln!("error: {msg}");
            eprintln!();
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    init_tracing();

    if let Some(maintenance) = maintenance {
        return schedule_standalone_conversation_maintenance(&cfg, maintenance);
    }

    // Provision standalone remote-access policy before opening any application
    // store, manager, PTY, listener, or router. `ServerConfig::from_args`
    // rejects incomplete configuration for every bind mode; this defensive
    // boundary also refuses a manually constructed incomplete config. Token
    // bytes and digests are never logged.
    let authority = match provision_standalone_authority(&cfg) {
        Ok(authority) => {
            authority.set_ingress_provenance(cfg.ingress_provenance());
            authority
        }
        Err(stable_code) => {
            error!(
                target: "se_manager::web::auth",
                stable_code,
                "standalone remote-access authority provisioning failed"
            );
            return ExitCode::from(1);
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    runtime.block_on(async move {
        // The standalone host crosses the same synchronous Conversation admission gate as
        // Desktop before opening any app-managed store, manager, PTY, or network route.
        let conversation_bootstrap =
            match termul_manager_lib::conversation::ConversationBootstrap::run(
                termul_manager_lib::conversation::HostConversationRoots::standalone(
                    cfg.service_account_state_dir(),
                    cfg.conversation_workspace_root(),
                    cfg.sessions_dir.clone(),
                    cfg.workspace_manifests_dir.clone(),
                ),
                termul_manager_lib::conversation::MigrationHostMode::Standalone,
            ) {
                Ok(outcome) => outcome,
                Err(error) => {
                    error!(
                        code = error.code,
                        operation = error.operation,
                        "Conversation bootstrap aborted startup"
                    );
                    return ExitCode::from(1);
                }
            };
        info!(
            phase = ?conversation_bootstrap.migration_phase,
            precedence = ?conversation_bootstrap.reader_precedence,
            recovery_count = conversation_bootstrap.recovery_item_count,
            "Conversation repository ready; mutable-store admission opened"
        );

        // Story 1.4: construct the LIVE relay sink (per-session event logs +
        // seq counters + subscriber set) and pass it to BOTH the ACP manager
        // (as an event sink) and `serve` (so `/ws` can subscribe clients +
        // replay cursors). ConversationRepository is the sole live writer; the configured
        // legacy sessions root was consumed read-only by bootstrap and is never reopened.
        // CAP-5 / Story 5: open the host-owned workspace-manifests root. The
        // standalone binary owns its own root — NEVER shared with a desktop
        // host on the same machine (two processes on one JSONL store would
        // corrupt both). Defaults to `<state dir>/workspace-manifests`; the
        // explicit `--workspace-manifests-dir` flag overrides.
        let workspace_manifests_dir = cfg
            .workspace_manifests_dir
            .clone()
            .unwrap_or_else(|| cfg.service_account_state_dir().join("workspace-manifests"));
        let workspace_manifest =
            match WorkspaceManifestService::open_read_only(workspace_manifests_dir).await {
                Ok(service) => Some(service),
                Err(error) => {
                    eprintln!("se-server: failed to open workspace-manifests store: {error}");
                    return ExitCode::from(1);
                }
            };
        // CAP-6 / Story 8: open the host-owned ACP catalog root. The
        // standalone binary owns its own root — NEVER shared with a desktop
        // host on the same machine. Defaults to `<state dir>/acp-catalog`.
        let acp_catalog_dir = cfg
            .acp_catalog_dir
            .clone()
            .unwrap_or_else(|| cfg.service_account_state_dir().join("acp-catalog"));
        let acp_catalog = match AcpCatalogService::open(acp_catalog_dir).await {
            Ok(service) => Some(service),
            Err(error) => {
                eprintln!("se-server: failed to open acp-catalog store: {error}");
                return ExitCode::from(1);
            }
        };
        // CAP-6 / Story 9: open the host-owned verified-atomic ACP install
        // root. The standalone binary owns its own root — NEVER shared with a
        // desktop host on the same machine. Defaults to
        // `<state dir>/acp-registry-binaries`. The install service holds the
        // catalog `Arc` for the convenience `install_by_id` path.
        let acp_install_dir = cfg
            .service_account_state_dir()
            .join("acp-registry-binaries");
        termul_manager_lib::set_acp_npm_local_root(
            cfg.service_account_state_dir().join("acp-npm-packages"),
        );
        let acp_install = match AcpInstallService::open(
            acp_install_dir,
            std::sync::Arc::clone(acp_catalog.as_ref().expect("catalog opened above")),
        )
        .await
        {
            Ok(service) => Some(service),
            Err(error) => {
                eprintln!("se-server: failed to open acp-install store: {error}");
                return ExitCode::from(1);
            }
        };
        let ws_relay = Arc::new(WsRelaySink::with_conversation_persistence(
            cfg.event_log_capacity,
            Arc::clone(&conversation_bootstrap.persistence_adapter),
            None,
        ));
        let acp = Arc::new(AcpManager::with_conversation_services(
            vec![ws_relay.clone()],
            Arc::clone(&conversation_bootstrap.creation),
            Arc::clone(&conversation_bootstrap.persistence_adapter),
        ));
        let scheduled_task_root = cfg
            .service_account_state_dir()
            .join("scheduled-tasks")
            .join("v1");
        let scheduled_task_store =
            match termul_manager_lib::ScheduledTaskStore::open_with_legacy_root(
                scheduled_task_root.join("catalog"),
                Some(scheduled_task_root.join("projects")),
            ) {
                Ok(store) => Arc::new(store),
                Err(error) => {
                    eprintln!("se-server: failed to open scheduled task store: {error}");
                    return ExitCode::from(1);
                }
            };
        let scheduled_tasks = termul_manager_lib::scheduled_tasks::ScheduledTaskService::new(
            scheduled_task_store,
            Arc::new(
                termul_manager_lib::scheduled_tasks::AcpScheduledTaskExecutor::new(
                    Arc::clone(&acp),
                    Arc::clone(&ws_relay),
                ),
            ),
        );
        acp.set_scheduled_tasks(&scheduled_tasks);
        scheduled_tasks.start();
        info!(
            root = %scheduled_tasks.store().root().display(),
            "scheduled task service started"
        );
        // Story 1.7: attach the server-side permission rendezvous (bounded
        // timeout, at-most-one, first-response-wins, disconnect-deny, TOCTOU).
        // The relay snapshots `acp:permission_request` events into it; the
        // `/ws` `respond_permission` handler + disconnect cleanup enforce the
        // policy. The desktop path does NOT attach a rendezvous (it uses the
        // `acp_respond_permission` Tauri command directly).
        let rendezvous = Arc::new(PermissionRendezvous::with_policy(
            Arc::clone(&acp),
            Duration::from_secs(cfg.permission_timeout_secs),
            Duration::from_secs(cfg.permission_reconnect_grace_secs),
        ));
        ws_relay.set_rendezvous(rendezvous);
        // Issue #411: attach the server-side question rendezvous (bounded
        // timeout, first-response-wins, TOCTOU). The relay snapshots
        // `acp:question_request` events into it; the `/ws` `answer_question`
        // handler + disconnect cleanup enforce the policy. The desktop path
        // does NOT attach one (it uses the `acp_answer_question` Tauri command
        // directly).
        let question_rendezvous = Arc::new(QuestionRendezvous::with_timeout(
            Arc::clone(&acp),
            Duration::from_secs(cfg.permission_timeout_secs),
        ));
        ws_relay.set_question_rendezvous(question_rendezvous);
        // Story 4.1: the in-memory project registry. In VPS mode the
        // standalone binary is the source of truth — it seeds the registry
        // from the file-backed `FileProjectRegistry` at startup (when
        // --projects-file / $SE_PROJECTS_FILE is configured). A missing
        // file is not fatal (loads as empty, so `/projects` returns empty);
        // a corrupt/invalid file IS fatal (abort startup so a misconfigured
        // VPS is obvious). Desktop-hosted mode never reaches here (it calls
        // `serve_router` directly with a renderer-fed registry).
        let registry = Arc::new(ProjectRegistry::new());
        let mut registry_persistence = None;
        if let Some(ref projects_file) = cfg.projects_file {
            match FileProjectRegistry::load(projects_file) {
                Ok(file_reg) => {
                    let n = file_reg.roots().len();
                    info!(
                        "loaded {} project root(s) from '{}'",
                        n,
                        projects_file.display()
                    );
                    seed_from_file(&registry, &file_reg);
                    registry_persistence = Some(Arc::new(parking_lot::Mutex::new(file_reg)));
                }
                Err(e) => {
                    eprintln!(
                        "se-server: failed to load projects file '{}': {e}",
                        projects_file.display()
                    );
                    return ExitCode::from(1);
                }
            }
        }
        // The standalone binary owns its interactive PTYs and kills them only
        // after Axum drains. Desktop shared-live passes its existing manager and
        // never reaches that cleanup path.
        let terminal_events = TerminalEventHub::standalone();
        let cwd_tracker = Arc::new(CwdTracker::new(terminal_events.clone()));
        let git_tracker = Arc::new(GitTracker::with_cwd_tracker(
            cwd_tracker.clone(),
            terminal_events.clone(),
        ));
        let exit_code_tracker = Arc::new(ExitCodeTracker::new(terminal_events.clone()));
        let pty = Arc::new(PtyManager::new(
            terminal_events.clone(),
            Arc::clone(&cwd_tracker),
            Arc::clone(&git_tracker),
            Arc::clone(&exit_code_tracker),
        ));
        acp.set_pty_manager(&pty);
        let lifecycle =
            match termul_manager_lib::conversation::ConversationLifecycleService::from_manager(
                Arc::clone(&acp),
                Arc::clone(&pty),
            ) {
                Ok(service) => service,
                Err(error) => {
                    error!(
                        code = error.code.as_str(),
                        "Conversation lifecycle construction failed"
                    );
                    return ExitCode::from(1);
                }
            };
        if let Err(error) = conversation_bootstrap
            .application
            .attach_lifecycle(lifecycle)
        {
            error!(code = error.code, "Conversation lifecycle admission failed");
            return ExitCode::from(1);
        }

        let projects_file = cfg.projects_file.clone();
        // Opt-in self-update loop (default off): only runs when the operator set
        // SE_SERVER_UPDATE_ENABLED=true + SE_SERVER_UPDATE_CHANNEL. A bad
        // signature keeps the current binary running (verify-before-swap), so an
        // unattended server is never bricked by a failed update attempt.
        spawn_periodic_update_loop();
        match serve(
            acp,
            pty,
            terminal_events,
            cwd_tracker,
            git_tracker,
            exit_code_tracker,
            ws_relay,
            registry,
            registry_persistence,
            projects_file,
            cfg,
            Arc::clone(&conversation_bootstrap.application),
            workspace_manifest,
            acp_catalog,
            acp_install,
            authority,
        )
        .await
        {
            Ok(()) => {
                scheduled_tasks.shutdown(Duration::from_secs(10)).await;
                ExitCode::SUCCESS
            }
            Err(e) => {
                scheduled_tasks.shutdown(Duration::from_secs(10)).await;
                eprintln!("se-server failed: {e}");
                ExitCode::from(1)
            }
        }
    })
}

/// Initialize `tracing` + `tracing-subscriber` (EnvFilter, `RUST_LOG`; floor `info`).
/// Extracted so both the normal server path and the `--check-update` one-shot
/// share the same setup.
#[derive(Debug)]
struct StandaloneConversationMaintenance {
    action: termul_manager_lib::conversation::MigrationMaintenanceAction,
    approval_receipt_path: Option<PathBuf>,
}

fn parse_conversation_maintenance_args(
    raw_args: &[String],
) -> Result<(Vec<String>, Option<StandaloneConversationMaintenance>), String> {
    let mut server_args = Vec::new();
    let mut action = None;
    let mut approval_receipt_path = None;
    let mut index = 0;
    while index < raw_args.len() {
        match raw_args[index].as_str() {
            "--conversation-migration-control" => {
                if action.is_some() {
                    return Err(
                        "--conversation-migration-control may be specified only once".into(),
                    );
                }
                let value = raw_args
                    .get(index + 1)
                    .ok_or("missing value for --conversation-migration-control")?;
                action = Some(match value.as_str() {
                    "rollback" => {
                        termul_manager_lib::conversation::MigrationMaintenanceAction::Rollback
                    }
                    "reapply" => {
                        termul_manager_lib::conversation::MigrationMaintenanceAction::Reapply
                    }
                    "finalize" => {
                        termul_manager_lib::conversation::MigrationMaintenanceAction::Finalize
                    }
                    _ => {
                        return Err(format!(
                            "invalid --conversation-migration-control '{value}': use rollback, reapply, or finalize"
                        ));
                    }
                });
                index += 2;
            }
            "--approval-receipt" => {
                if approval_receipt_path.is_some() {
                    return Err("--approval-receipt may be specified only once".into());
                }
                let value = raw_args
                    .get(index + 1)
                    .ok_or("missing value for --approval-receipt")?;
                approval_receipt_path = Some(PathBuf::from(value));
                index += 2;
            }
            _ => {
                server_args.push(raw_args[index].clone());
                index += 1;
            }
        }
    }

    let Some(action) = action else {
        if approval_receipt_path.is_some() {
            return Err(
                "--approval-receipt requires --conversation-migration-control finalize".into(),
            );
        }
        return Ok((server_args, None));
    };
    match (action, approval_receipt_path.as_ref()) {
        (termul_manager_lib::conversation::MigrationMaintenanceAction::Finalize, None) => {
            return Err(
                "--conversation-migration-control finalize requires --approval-receipt <path>"
                    .into(),
            );
        }
        (
            termul_manager_lib::conversation::MigrationMaintenanceAction::Rollback
            | termul_manager_lib::conversation::MigrationMaintenanceAction::Reapply,
            Some(_),
        ) => {
            return Err(
                "--approval-receipt is accepted only with conversation migration finalize".into(),
            );
        }
        _ => {}
    }
    Ok((
        server_args,
        Some(StandaloneConversationMaintenance {
            action,
            approval_receipt_path,
        }),
    ))
}

fn schedule_standalone_conversation_maintenance(
    cfg: &ServerConfig,
    maintenance: StandaloneConversationMaintenance,
) -> ExitCode {
    let state_root = cfg.service_account_state_dir();
    if let Err(error) = std::fs::create_dir_all(&state_root) {
        error!(
            code = "MIGRATION_DURABILITY_FAILED",
            "failed to prepare standalone maintenance state root: {error}"
        );
        return ExitCode::from(1);
    }
    let approval_receipt = match maintenance.approval_receipt_path {
        Some(path) => match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        {
            Ok(receipt) => Some(receipt),
            Err(error) => {
                error!(
                    code = "MIGRATION_APPROVAL_INVALID",
                    "failed to read standalone maintenance approval receipt: {error}"
                );
                return ExitCode::from(1);
            }
        },
        None => None,
    };
    let request_id = approval_receipt
        .as_ref()
        .map(
            |receipt: &termul_manager_lib::conversation::ApprovalReceiptV1| {
                receipt.request_id.clone()
            },
        )
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request = termul_manager_lib::conversation::MigrationMaintenanceRequestV1 {
        action: maintenance.action,
        request_id,
        requested_at_utc: Utc::now(),
        approval_receipt,
    };
    let control = match termul_manager_lib::conversation::ConversationMigrationControlService::new(
        &state_root,
    ) {
        Ok(control) => control,
        Err(error) => {
            error!(
                code = error.code.as_str(),
                "failed to create maintenance control"
            );
            return ExitCode::from(1);
        }
    };
    match control.request(request) {
        Ok(receipt) => {
            match serde_json::to_string(&receipt) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    error!("failed to serialize maintenance receipt: {error}");
                    return ExitCode::from(1);
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            error!(
                code = error.code.as_str(),
                operation = error.operation,
                "failed to schedule standalone maintenance"
            );
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod conversation_maintenance_tests {
    use super::*;
    use termul_manager_lib::conversation::MigrationMaintenanceAction;

    #[test]
    fn standalone_control_mode_accepts_all_actions_and_preserves_server_args() {
        for (value, expected) in [
            ("rollback", MigrationMaintenanceAction::Rollback),
            ("reapply", MigrationMaintenanceAction::Reapply),
        ] {
            let args = vec![
                "--port".to_string(),
                "9090".to_string(),
                "--conversation-migration-control".to_string(),
                value.to_string(),
            ];
            let (server_args, maintenance) = parse_conversation_maintenance_args(&args).unwrap();
            assert_eq!(server_args, ["--port", "9090"]);
            let maintenance = maintenance.unwrap();
            assert_eq!(maintenance.action, expected);
            assert!(maintenance.approval_receipt_path.is_none());
        }
    }

    #[test]
    fn standalone_finalize_requires_approval_and_other_actions_reject_it() {
        let missing = vec![
            "--conversation-migration-control".to_string(),
            "finalize".to_string(),
        ];
        assert!(parse_conversation_maintenance_args(&missing)
            .unwrap_err()
            .contains("requires --approval-receipt"));

        let wrong_action = vec![
            "--conversation-migration-control".to_string(),
            "rollback".to_string(),
            "--approval-receipt".to_string(),
            "approval.json".to_string(),
        ];
        assert!(parse_conversation_maintenance_args(&wrong_action)
            .unwrap_err()
            .contains("accepted only"));

        let finalize = vec![
            "--conversation-migration-control".to_string(),
            "finalize".to_string(),
            "--approval-receipt".to_string(),
            "approval.json".to_string(),
        ];
        let (_, maintenance) = parse_conversation_maintenance_args(&finalize).unwrap();
        let maintenance = maintenance.unwrap();
        assert_eq!(maintenance.action, MigrationMaintenanceAction::Finalize);
        assert_eq!(
            maintenance.approval_receipt_path,
            Some(PathBuf::from("approval.json"))
        );
    }

    #[test]
    fn approval_flag_without_control_mode_is_rejected() {
        let args = vec![
            "--approval-receipt".to_string(),
            "approval.json".to_string(),
        ];
        assert!(parse_conversation_maintenance_args(&args)
            .unwrap_err()
            .contains("requires --conversation-migration-control finalize"));
    }

    #[test]
    fn default_standalone_args_fail_closed_before_bootstrap_admission() {
        let error = ServerConfig::from_args(Vec::<&str>::new()).unwrap_err();
        assert!(error
            .to_string()
            .contains(REMOTE_AUTH_CONFIGURATION_REQUIRED));

        let source = include_str!("server_main.rs");
        let admission = source
            .find("provision_standalone_authority(&cfg)")
            .expect("standalone authority admission call");
        let bootstrap = source
            .find("ConversationBootstrap::run")
            .expect("Conversation bootstrap call");
        assert!(admission < bootstrap);
    }

    #[tokio::test]
    async fn explicit_token_file_and_origin_admit_an_authenticated_request() {
        // Binary unit tests run in their own process, so the library's
        // process-global boundary logger cannot race another test module here.
        const TOKEN: &str = "standalone-operator-test-token";
        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("remote-access-token");
        std::fs::write(&token_path, TOKEN).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let token_path = token_path.to_string_lossy().into_owned();
        let cfg = ServerConfig::from_args([
            "--remote-access-token-file",
            token_path.as_str(),
            "--allowed-origin",
            "https://standalone.example.test",
        ])
        .unwrap();
        let authority = provision_standalone_authority(&cfg).unwrap();
        assert!(authority.verify_bearer(TOKEN).is_ok());
        assert!(authority
            .verify_origin(Some(&axum::http::HeaderValue::from_static(
                "https://standalone.example.test",
            )))
            .is_ok());

        use tower::ServiceExt;
        let app = axum::Router::new()
            .route("/projects", axum::routing::get(|| async { "admitted" }))
            .layer(axum::middleware::from_fn(
                termul_manager_lib::web::auth::capability_middleware,
            ))
            .layer(axum::Extension(
                termul_manager_lib::web::auth::RemoteRouteClass::Project,
            ))
            .layer(axum::Extension(
                termul_manager_lib::web::auth::IngressProvenance::LocalOperator,
            ))
            .layer(axum::Extension(authority));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/projects")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        40123,
                    ))))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}

fn init_tracing() {
    // Standalone composition already captures both `tracing` and `log`.
    // Host shutdown uses one absolute Instant deadline in `web::serve`.
    // `try_init` installs tracing's LogTracer bridge as well as the subscriber,
    // so shared `log` facade events are durable in standalone composition.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
}

/// Resolve the server's own binary path for the self-update swap/reexec.
fn current_binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("termul-server"))
}

/// Build the self-update options from env + the embedded pubkey. `Err` when
/// self-update is unavailable (no pubkey baked in) — the caller logs + skips.
///
/// `default_channel`: when `SE_SERVER_UPDATE_CHANNEL` is unset/invalid,
/// `Some(c)` falls back to `c` (used by the operator-explicit `--check-update`
/// one-shot, which defaults to Stable), while `None` surfaces an error (used by
/// the periodic loop, which requires the env to opt in).
fn build_update_options(default_channel: Option<UpdateChannel>) -> Result<UpdateOptions, String> {
    let channel =
        UpdateChannel::parse(&std::env::var("SE_SERVER_UPDATE_CHANNEL").unwrap_or_default())
            .or(default_channel);
    let channel = match channel {
        Some(c) => c,
        None => {
            return Err(
                "SE_SERVER_UPDATE_CHANNEL is unset or not stable/insider/nightly".to_owned(),
            )
        }
    };
    // `embedded_public_key()` returns `Result<&'static PublicKey>` backed by a
    // `OnceLock`; the `&'static` ref moves into `UpdateOptions` (and across the
    // spawned periodic task) directly — no clone needed.
    let public_key = embedded_public_key().map_err(|e| e.to_string())?;
    Ok(UpdateOptions {
        channel,
        current_version: current_version().to_owned(),
        binary_path: current_binary_path(),
        platform_key: SERVER_PLATFORM_KEY,
        public_key,
    })
}

/// `--check-update`: fetch → verify → swap, then exit SUCCESS. Operator-
/// explicit, so the channel defaults to Stable when the env is unset (the
/// periodic loop, by contrast, requires the env to opt in). Does **not**
/// re-exec: re-exec would start the server in this one-shot's place; the
/// operator restarts the server to run the new version (the `.old` binary is
/// retained for manual rollback). Never auto-restarts an unattended server
/// without this explicit trigger or the env gate.
fn run_one_shot_update_check() -> ExitCode {
    info!(
        target: "se_manager::server_update",
        "one-shot server self-update requested (--check-update)"
    );
    // Operator-explicit one-shot: default to Stable when the channel env is
    // unset (matches the `--check-update` usage docs); the env still wins when set.
    let opts = match build_update_options(Some(UpdateChannel::Stable)) {
        Ok(o) => o,
        Err(reason) => {
            error!(target: "se_manager::server_update", "self-update unavailable: {reason}");
            return ExitCode::from(1);
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            error!(
                target: "se_manager::server_update",
                "failed to start tokio runtime for update check: {e}"
            );
            return ExitCode::from(1);
        }
    };

    match runtime.block_on(check_and_apply_update(&opts)) {
        Ok(UpdateOutcome::NoUpdate) => {
            info!(
                target: "se_manager::server_update",
                "no newer server binary on channel {:?} (current {})",
                opts.channel,
                opts.current_version
            );
            ExitCode::SUCCESS
        }
        Ok(UpdateOutcome::Updated {
            new_version,
            old_path,
        }) => {
            // One-shot: apply the update but do NOT re-exec — re-exec would
            // start the server in this one-shot's place. The operator restarts
            // the server to run the new version; the `.old` binary is retained
            // for manual rollback.
            info!(
                target: "se_manager::server_update",
                "verified + swapped to {new_version}; previous binary retained at {}. \
                 Restart the server to run the new version (no auto-reexec from --check-update)",
                old_path.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!(
                target: "se_manager::server_update",
                "update check failed (keeping current binary): {e}"
            );
            ExitCode::from(1)
        }
    }
}

/// Spawn the background periodic self-update loop on the server's tokio runtime.
/// No-op (with an info log) when the operator did not opt in via env — the
/// default is off so an unattended server never auto-updates.
fn spawn_periodic_update_loop() {
    if !is_update_enabled() {
        info!(
            target: "se_manager::server_update",
            "self-update disabled (set SE_SERVER_UPDATE_ENABLED=true + \
             SE_SERVER_UPDATE_CHANNEL to opt in)"
        );
        return;
    }

    // Periodic loop: opt-in requires the channel env (no default) — an
    // unattended server never auto-updates unless the operator named a channel.
    let opts = match build_update_options(None) {
        Ok(o) => o,
        Err(reason) => {
            warn!(
                target: "se_manager::server_update",
                "SE_SERVER_UPDATE_ENABLED=true but self-update unavailable: {reason} \
                 (periodic loop disabled)"
            );
            return;
        }
    };

    // Default 6h, mirroring the desktop's periodic cadence. Overridable so an
    // operator can tune the polling frequency for their deployment.
    let interval_secs = std::env::var("SE_SERVER_UPDATE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(6 * 60 * 60);

    let channel = opts.channel;
    info!(
        target: "se_manager::server_update",
        "periodic self-update enabled on channel {:?} (every {}s)", channel, interval_secs
    );

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            ticker.tick().await;
            match check_and_apply_update(&opts).await {
                Ok(UpdateOutcome::NoUpdate) => {
                    tracing::debug!(
                        target: "se_manager::server_update",
                        "no newer server binary on channel {:?}", opts.channel
                    );
                }
                Ok(UpdateOutcome::Updated {
                    new_version,
                    old_path,
                }) => {
                    info!(
                        target: "se_manager::server_update",
                        "verified + swapped to {new_version}; restarting into the new binary"
                    );
                    // Re-exec the canonical install path (NOT current_exe(),
                    // which would resolve to the `.old` inode after the swap).
                    if let Err(e) = restart_binary(&opts.binary_path) {
                        error!(
                            target: "se_manager::server_update",
                            "reexec failed: {e}; rolling back to the previous binary"
                        );
                        // Roll the swap back so the deployment keeps running the
                        // known-good binary instead of a new binary that can't re-exec.
                        if let Err(restore_err) = restore_previous(&opts.binary_path, &old_path) {
                            error!(
                                target: "se_manager::server_update",
                                "rollback also failed: {restore_err}; the new binary is at {} \
                                 and the previous at {} — recover manually",
                                opts.binary_path.display(),
                                old_path.display()
                            );
                        }
                    }
                    // restart_binary never returns on success.
                    return;
                }
                Err(e) => {
                    warn!(
                        target: "se_manager::server_update",
                        "periodic update check failed (keeping current binary): {e}"
                    );
                }
            }
        }
    });
}

fn usage() -> &'static str {
    "Usage: se-server [--host HOST] [--port PORT] [--event-log-capacity N] [--permission-timeout SECS] [--permission-reconnect-grace SECS] [--project-root PATH] [--projects-file PATH] [--sessions-dir PATH] [--conversation-workspace-root PATH] [--workspace-manifests-dir PATH] [--acp-catalog-dir PATH] [--remote-access-token-file PATH] [--allowed-origin ORIGIN] [--check-update] [--conversation-migration-control rollback|reapply|finalize] [--approval-receipt PATH]\n\n\
     Options:\n\
        --host HOST                 Bind host (default: 127.0.0.1; use 0.0.0.0 to expose)\n\
        --port PORT                 Bind port (default: 8080)\n\
        --event-log-capacity N      Per-session event-log ring capacity (default: 4096)\n\
        --permission-timeout SECS   Permission rendezvous timeout in seconds (default: 60)\n\
        --permission-reconnect-grace SECS  Last-subscriber reconnect grace (default: 15)\n\
        --project-root PATH         Project-root boundary for /fs/* routes (default: $SE_PROJECT_ROOT or $HOME)\n\
        --projects-file PATH        VFS-roots registry file (default: $SE_PROJECTS_FILE; missing = empty list)\n\
        --sessions-dir PATH         Legacy sessions input root (default: $SE_SESSIONS_DIR or service-account state dir)\n\
        --conversation-workspace-root PATH  Visible Conversation workspaces (default: $SE_CONVERSATION_WORKSPACE_ROOT or <project-root>/Termul)\n\
        --workspace-manifests-dir PATH  Legacy workspace-manifests input root (default: <state dir>/workspace-manifests)\n\
        --acp-catalog-dir PATH      ACP catalog root (default: <state dir>/acp-catalog)\n\
        --remote-access-token-file PATH  Operator-owned bearer token file (required)\n\
        --allowed-origin ORIGIN     Allowed browser Origin; repeatable (at least one required)\n\
        --conversation-migration-control ACTION  Durably schedule rollback, reapply, or finalize\n\
                                     for the next bootstrap, then exit without opening stores,\n\
                                     managers, PTYs, listeners, or routes.\n\
        --approval-receipt PATH     ApprovalReceiptV1 JSON; required only for finalize.\n\
        --check-update              Run one opt-in self-update now: fetch the channel manifest,\n\
                                     verify the downloaded binary signature, atomically swap, and\n\
                                     reexec. Defaults to the stable channel when\n\
                                     SE_SERVER_UPDATE_CHANNEL is unset; the env wins when set.\n\
                                     (env: SE_SERVER_UPDATE_ENABLED + SE_SERVER_UPDATE_CHANNEL\n\
                                     gate the periodic loop; SE_SERVER_UPDATE_INTERVAL_SECS default 21600)\n\
        -h, --help                  Show this help"
}
