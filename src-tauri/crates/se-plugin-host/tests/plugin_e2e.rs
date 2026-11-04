//! Real native-library acceptance tests. Run through scripts/test-plugins.sh, which
//! prepares all three artifacts before starting this test binary. Never build a
//! fixture here: nested builds contend with the runner's shared target lock.

use se_plugin_host::{HostDiagnostics, HostOptions, PluginHost};
use se_plugin_protocol::*;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const ID: &str = "org.termul.example";
const VIEW: &str = "example.counter";

// Each test owns a Tokio runtime, but macOS native loading is process-global.
// Serialize real dlopen/panic/timeout scenarios without changing runner flags.
static NATIVE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Artifacts {
    normal: PathBuf,
    fault: PathBuf,
    upgrade: PathBuf,
}

fn artifacts() -> Artifacts {
    let root = PathBuf::from(std::env::var_os("SE_PLUGIN_TEST_ARTIFACTS").expect(
        "SE_PLUGIN_TEST_ARTIFACTS must point to prebuilt normal/fault/upgrade libraries; run bash scripts/test-plugins.sh",
    ));
    let filename = "activity-studio.seplugin";
    let library = |variant: &str| {
        let path = root.join(variant).join(filename);
        let metadata = fs::metadata(&path).unwrap_or_else(|error| {
            panic!("required real {variant} plugin artifact {} is missing: {error}; run bash scripts/test-plugins.sh", path.display())
        });
        assert!(
            metadata.is_file() && metadata.len() > 0,
            "invalid artifact {}",
            path.display()
        );
        path.canonicalize().expect("canonical artifact path")
    };
    Artifacts {
        normal: library("normal"),
        fault: library("fault"),
        upgrade: library("upgrade"),
    }
}

fn path_string(path: &Path) -> String {
    path.to_str().expect("UTF-8 test artifact path").into()
}

async fn enable(host: &PluginHost, enabled: bool) -> PluginInfo {
    host.set_enabled(SetEnabledRequest {
        id: ID.into(),
        enabled,
    })
    .await
    .expect("change example enabled state")
}

async fn view(host: &PluginHost) -> ViewSnapshot {
    let snapshot = host
        .get_view(GetViewRequest {
            id: ID.into(),
            view_id: VIEW.into(),
        })
        .await
        .expect("read example counter view");
    validate_view_snapshot(&snapshot).expect("valid real plugin view snapshot");
    assert_eq!(snapshot.plugin_id, ID);
    assert_eq!(snapshot.tree.view_id, VIEW);
    snapshot
}

fn button_action(snapshot: &ViewSnapshot, action_id: &str) -> ActionEvent {
    let mut stack = vec![&snapshot.tree.root];
    let node_id = loop {
        let node = stack.pop().expect("example action button must exist");
        if let UiNode::Button { id, props } = node {
            if props.action_id == action_id {
                assert!(!props.disabled);
                break id.clone();
            }
        }
        stack.extend(node.children());
    };
    ActionEvent {
        schema_version: SCHEMA_VERSION,
        event_id: Uuid::new_v4().to_string(),
        plugin_id: ID.into(),
        generation: snapshot.generation.clone(),
        view_id: VIEW.into(),
        view_revision: snapshot.revision,
        node_id,
        action_id: action_id.into(),
        kind: ActionKind::Click,
        value: ActionValue::Null,
    }
}

fn assert_all_node_types(snapshot: &ViewSnapshot) {
    let mut types = BTreeSet::new();
    let mut stack = vec![&snapshot.tree.root];
    while let Some(node) = stack.pop() {
        types.insert(match node {
            UiNode::Column { .. } => "column",
            UiNode::Row { .. } => "row",
            UiNode::Text { .. } => "text",
            UiNode::Heading { .. } => "heading",
            UiNode::Button { .. } => "button",
            UiNode::Input { .. } => "input",
            UiNode::Toggle { .. } => "toggle",
            UiNode::Select { .. } => "select",
            UiNode::List { .. } => "list",
            UiNode::Divider { .. } => "divider",
        });
        stack.extend(node.children());
    }
    assert_eq!(
        types,
        BTreeSet::from([
            "column", "row", "text", "heading", "button", "input", "toggle", "select", "list",
            "divider",
        ])
    );
}

async fn change_control(
    host: &PluginHost,
    snapshot: &ViewSnapshot,
    node_id: &str,
    value: ActionValue,
) -> ViewSnapshot {
    let action_id = match snapshot.tree.root.find(node_id) {
        Some(UiNode::Input { props, .. }) => props.action_id.clone(),
        Some(UiNode::Toggle { props, .. }) => props.action_id.clone(),
        Some(UiNode::Select { props, .. }) => props.action_id.clone(),
        other => panic!("missing editable control {node_id}: {other:?}"),
    };
    let updated = host
        .dispatch_action(ActionEvent {
            schema_version: SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            plugin_id: ID.into(),
            generation: snapshot.generation.clone(),
            view_id: VIEW.into(),
            view_revision: snapshot.revision,
            node_id: node_id.into(),
            action_id,
            kind: ActionKind::Change,
            value,
        })
        .await
        .expect("typed local control action roundtrip");
    validate_view_snapshot(&updated).unwrap();
    assert!(updated.revision > snapshot.revision);
    assert_eq!(updated.config_revision, snapshot.config_revision);
    updated
}

fn command(generation: &str, command_id: &str) -> ExecuteCommandRequest {
    ExecuteCommandRequest {
        id: ID.into(),
        generation: generation.into(),
        command_id: command_id.into(),
        args: Value::Null,
    }
}

// The example fixture deliberately exposes stable semantic node IDs. Assert
// rendered Text values, not a JSON substring (which could match an input value).
fn assert_counter(snapshot: &ViewSnapshot, greeting: &str, count: u64, tone: Tone, v2: bool) {
    match snapshot.tree.root.find("greeting") {
        Some(UiNode::Text { props, .. }) => {
            assert_eq!(props.text, greeting);
            assert_eq!(props.tone, tone);
        }
        other => panic!("missing greeting Text node: {other:?}"),
    }
    match snapshot.tree.root.find("count") {
        Some(UiNode::Text { props, .. }) => assert_eq!(props.text, count.to_string()),
        other => panic!("missing count Text node: {other:?}"),
    }
    match snapshot.tree.root.find("heading") {
        Some(UiNode::Heading { props, .. }) => {
            assert!(props.text.contains(greeting), "heading must show greeting");
            assert_eq!(props.text.contains("v2"), v2);
        }
        other => panic!("missing Heading node: {other:?}"),
    }
}

async fn config(host: &PluginHost) -> ConfigSnapshot {
    host.get_config(GetConfigRequest { id: ID.into() })
        .await
        .expect("read example configuration")
}

fn registry_bytes(profile: &Path) -> Vec<u8> {
    fs::read(profile.join("plugins/registry.json")).expect("persisted registry")
}

fn registry(profile: &Path) -> Value {
    serde_json::from_slice(&registry_bytes(profile)).expect("registry JSON")
}

fn loaded_paths(profile: &Path, boot_id: &str) -> BTreeSet<PathBuf> {
    fs::read_dir(profile.join("plugins/loaded").join(boot_id))
        .expect("real library generation directory")
        .map(|entry| entry.expect("loaded library entry").path())
        .collect()
}

fn assert_no_active_resources(d: &HostDiagnostics) {
    assert_eq!(d.active_instances, 0, "{d:?}");
    assert_eq!(d.callbacks_in_flight, 0, "{d:?}");
    assert_eq!(d.broker_registrations, 0, "{d:?}");
    assert_eq!(d.view_count, 0, "{d:?}");
    assert_eq!(d.command_count, 0, "{d:?}");
}

async fn close(host: Arc<PluginHost>) {
    host.shutdown().await.expect("clean host shutdown");
    let weak = Arc::downgrade(&host);
    drop(host);
    // Maintenance tasks briefly upgrade Weak references. Wait for their current
    // iteration to release the profile lock before testing a real reopen.
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shutdown host must release its profile lock");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn example_cdylib_end_to_end() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let artifacts = artifacts();
    tokio::time::timeout(Duration::from_secs(60), async {
        let profile = tempfile::tempdir().expect("isolated plugin profile");
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .expect("open host");
        assert!(host.list().await.unwrap().plugins.is_empty());
        let installed = host
            .install(InstallRequest {
                path: path_string(&artifacts.normal),
            })
            .await
            .expect("dlopen normal example and install");
        assert_eq!(installed.id, ID);
        assert_eq!(installed.manifest.version, "0.1.0");
        assert_eq!(installed.manifest.contributes.views.len(), 2);
        let contribution = &installed.manifest.contributes.views[0];
        assert_eq!(contribution.id, VIEW);
        assert_eq!(contribution.location, ViewLocation::Sidebar);
        assert_eq!(contribution.icon.as_deref(), Some("calculator"));
        assert_eq!(installed.state, PluginState::Disabled);
        assert!(!installed.desired_enabled);
        assert!(installed.generation.is_none());
        assert_eq!(host.list().await.unwrap().plugins, vec![installed.clone()]);
        assert_eq!(installed.manifest.contributes.commands.len(), 7);
        assert_eq!(
            installed.manifest.contributes.commands[0].id,
            "example.reset"
        );
        assert_no_active_resources(&host.diagnostics());

        let active = enable(&host, true).await;
        assert_eq!(active.state, PluginState::Active);
        assert!(active.desired_enabled && active.generation.is_some());
        let diagnostics = host.diagnostics();
        assert_eq!(diagnostics.active_instances, 1);
        assert_eq!(diagnostics.broker_registrations, 1);
        assert_eq!(diagnostics.view_count, 1);
        assert_eq!(diagnostics.command_count, 7);
        assert!(
            diagnostics.loaded_libraries >= 2,
            "install and activation must dlopen real copies"
        );
        let initial = view(&host).await;
        assert_counter(&initial, "Hello", 0, Tone::Default, false);
        assert_all_node_types(&initial);
        let one = host
            .dispatch_action(button_action(&initial, "increment"))
            .await
            .unwrap();
        assert!(one.revision > initial.revision);
        assert_counter(&one, "Hello", 1, Tone::Default, false);

        let defaults = config(&host).await;
        let values: ConfigValues = [
            ("greeting".into(), ConfigValue::String("Hola".into())),
            ("step".into(), ConfigValue::Number(2.0)),
            ("showCount".into(), ConfigValue::Boolean(true)),
            ("tone".into(), ConfigValue::String("muted".into())),
        ]
        .into_iter()
        .collect();
        let saved = host
            .set_config(SetConfigRequest {
                id: ID.into(),
                expected_revision: defaults.revision,
                values: values.clone(),
            })
            .await
            .expect("apply validated configuration to real plugin");
        assert_eq!(saved.revision, defaults.revision + 1);
        assert_eq!(saved.values, values);
        let configured = view(&host).await;
        assert_eq!(configured.config_revision, saved.revision);
        assert!(configured.revision > one.revision);
        assert_counter(&configured, "Hola", 1, Tone::Muted, false);
        let three = host
            .dispatch_action(button_action(&configured, "increment"))
            .await
            .unwrap();
        assert_counter(&three, "Hola", 3, Tone::Muted, false);

        let disk_before = registry_bytes(profile.path());
        let mut bad_type = values.clone();
        bad_type.insert("step".into(), ConfigValue::String("2".into()));
        let mut bad_enum = values.clone();
        bad_enum.insert("tone".into(), ConfigValue::String("not-a-tone".into()));
        let mut conflicting_values = values.clone();
        conflicting_values.insert(
            "greeting".into(),
            ConfigValue::String("must-not-persist".into()),
        );
        for (candidate, revision, expected_code) in [
            (bad_type, saved.revision, PLUGIN_CONFIG_INVALID),
            (bad_enum, saved.revision, PLUGIN_CONFIG_INVALID),
            (
                conflicting_values,
                defaults.revision,
                PLUGIN_CONFIG_CONFLICT,
            ),
        ] {
            let error = host
                .set_config(SetConfigRequest {
                    id: ID.into(),
                    expected_revision: revision,
                    values: candidate,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, expected_code, "{error}");
            assert_eq!(registry_bytes(profile.path()), disk_before);
            assert_eq!(config(&host).await, saved);
            assert_eq!(view(&host).await, three);
        }

        // Local UI edits roundtrip through typed change events without changing
        // the persisted config snapshot or the required 0 -> 1 -> 3 sequence.
        let local_greeting = change_control(&host, &three, "greeting-input", ActionValue::String("Local only".into())).await;
        assert_counter(&local_greeting, "Local only", 3, Tone::Muted, false);
        assert!(matches!(local_greeting.tree.root.find("greeting-input"), Some(UiNode::Input { props, .. }) if props.value == "Local only"));
        let hidden = change_control(&host, &local_greeting, "show-count", ActionValue::Boolean(false)).await;
        assert!(hidden.tree.root.find("count").is_none());
        assert!(matches!(hidden.tree.root.find("show-count"), Some(UiNode::Toggle { props, .. }) if !props.checked));
        let visible = change_control(&host, &hidden, "show-count", ActionValue::Boolean(true)).await;
        assert_counter(&visible, "Local only", 3, Tone::Muted, false);
        assert!(matches!(visible.tree.root.find("show-count"), Some(UiNode::Toggle { props, .. }) if props.checked));
        let normal_tone = change_control(&host, &visible, "tone-select", ActionValue::String("normal".into())).await;
        assert_counter(&normal_tone, "Local only", 3, Tone::Default, false);
        assert!(matches!(normal_tone.tree.root.find("tone-select"), Some(UiNode::Select { props, .. }) if props.value == "normal"));
        let muted = change_control(&host, &normal_tone, "tone-select", ActionValue::String("muted".into())).await;
        assert_counter(&muted, "Local only", 3, Tone::Muted, false);
        let restored = change_control(&host, &muted, "greeting-input", ActionValue::String("Hola".into())).await;
        assert_counter(&restored, "Hola", 3, Tone::Muted, false);
        assert_eq!(config(&host).await, saved);
        assert_eq!(registry_bytes(profile.path()), disk_before);

        // A contributed command must really execute and invalidate the view.
        let reset_reply = host.execute_command(command(&restored.generation, "example.reset"))
            .await
            .unwrap();
        assert_eq!(reset_reply.invalidate_views, vec![VIEW.to_string()]);
        assert_eq!(reset_reply.result["count"].as_f64(), Some(0.0));
        let reset = view(&host).await;
        assert!(reset.revision > restored.revision);
        assert_counter(&reset, "Hola", 0, Tone::Muted, false);
        close(host).await;
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .unwrap();
        assert_eq!(config(&host).await, saved);
        assert_eq!(
            host.list().await.unwrap().plugins[0].state,
            PluginState::Active
        );
        let reopened = view(&host).await;
        assert_counter(&reopened, "Hola", 0, Tone::Muted, false);
        assert_ne!(reopened.generation, three.generation);

        let old_action = button_action(&reopened, "increment");
        let old_registry = registry(profile.path());
        let boot_id = host.list().await.unwrap().boot_id;
        let old_paths = loaded_paths(profile.path(), &boot_id);
        let upgraded = host
            .reload(ReloadRequest {
                id: ID.into(),
                path: Some(path_string(&artifacts.upgrade)),
            })
            .await
            .expect("load 0.1.1 from a distinct generation path");
        assert_eq!(upgraded.manifest.version, "0.1.1");
        assert_eq!(upgraded.manifest.contributes.views[0].location, ViewLocation::Sidebar);
        assert_eq!(upgraded.manifest.contributes.views[0].icon.as_deref(), Some("calculator"));
        assert_eq!(upgraded.state, PluginState::Active);
        assert_ne!(upgraded.generation, Some(reopened.generation.clone()));
        let upgraded_view = view(&host).await;
        assert_counter(&upgraded_view, "Hola", 0, Tone::Muted, true);
        assert_eq!(config(&host).await, saved);
        let upgraded_registry = registry(profile.path());
        assert_ne!(
            upgraded_registry["plugins"][0]["artifactPath"],
            old_registry["plugins"][0]["artifactPath"]
        );
        let new_paths = loaded_paths(profile.path(), &boot_id);
        assert!(
            old_paths.is_subset(&new_paths),
            "loaded generations are retained until Core exit"
        );
        assert!(
            new_paths.difference(&old_paths).count() >= 2,
            "reload preflight and activation use fresh generation copies"
        );
        assert_eq!(
            host.dispatch_action(old_action).await.unwrap_err().code,
            PLUGIN_STALE_GENERATION
        );
        assert_eq!(
            host.execute_command(command(&reopened.generation, "example.reset"))
                .await
                .unwrap_err()
                .code,
            PLUGIN_STALE_GENERATION
        );
        assert_eq!(view(&host).await, upgraded_view);

        // A genuinely incompatible candidate (a downgrade) must not disturb the
        // installed artifact, configuration, or live generation.
        let before_rejected_reload = registry_bytes(profile.path());
        assert_eq!(
            host.reload(ReloadRequest {
                id: ID.into(),
                path: Some(path_string(&artifacts.normal)),
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_INVALID_MANIFEST
        );
        assert_eq!(registry_bytes(profile.path()), before_rejected_reload);
        assert_eq!(config(&host).await, saved);
        assert_eq!(view(&host).await, upgraded_view);

        let before_disable = host.diagnostics();
        let disabled = enable(&host, false).await;
        assert_eq!(disabled.state, PluginState::Disabled);
        assert!(!disabled.desired_enabled);
        assert!(disabled.generation.is_none());
        let after_disable = host.diagnostics();
        assert_no_active_resources(&after_disable);
        assert_eq!(
            after_disable.destroy_attempts,
            before_disable.destroy_attempts + 1
        );
        assert_eq!(
            after_disable.loaded_libraries, before_disable.loaded_libraries,
            "disable must never dlclose a loaded library"
        );
        assert_eq!(
            host.get_view(GetViewRequest {
                id: ID.into(),
                view_id: VIEW.into()
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_DISABLED
        );
        assert_eq!(
            host.execute_command(command(&upgraded_view.generation, "example.reset"))
                .await
                .unwrap_err()
                .code,
            PLUGIN_DISABLED
        );
        // Idempotent disable must not attempt a second destroy.
        enable(&host, false).await;
        assert_eq!(
            host.diagnostics().destroy_attempts,
            after_disable.destroy_attempts
        );
        enable(&host, true).await;
        assert_counter(&view(&host).await, "Hola", 0, Tone::Muted, true);
        enable(&host, false).await;
        let uninstalled = host
            .uninstall(UninstallRequest { id: ID.into() })
            .await
            .unwrap();
        assert!(uninstalled.removed);
        assert_eq!(uninstalled.id, ID);
        assert!(host.list().await.unwrap().plugins.is_empty());
        assert_eq!(registry(profile.path())["plugins"], serde_json::json!([]));
        assert_eq!(
            host.get_config(GetConfigRequest { id: ID.into() })
                .await
                .unwrap_err()
                .code,
            PLUGIN_NOT_FOUND
        );
        assert_no_active_resources(&host.diagnostics());
        assert!(
            artifacts.normal.is_file(),
            "uninstall must preserve user source library"
        );
        assert!(
            artifacts.upgrade.is_file(),
            "uninstall must preserve reload source library"
        );
        close(host).await;
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .unwrap();
        assert!(
            host.list().await.unwrap().plugins.is_empty(),
            "uninstall must survive reopen"
        );
        close(host).await;
    })
    .await
    .expect("real dlopen lifecycle acceptance must complete within 60 seconds");
}

// Fault cases use separate profiles: a panic quarantines its generation, so the
// timeout must not depend on resurrecting a previously poisoned instance. Abort
// is intentionally NEVER invoked here; it belongs to the process-level runner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cdylib_panic_is_contained_and_host_remains_manageable() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let artifacts = artifacts();
    tokio::time::timeout(Duration::from_secs(20), async {
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .unwrap();
        host.install(InstallRequest {
            path: path_string(&artifacts.fault),
        })
        .await
        .unwrap();
        let active = enable(&host, true).await;
        assert!(active
            .manifest
            .contributes
            .commands
            .iter()
            .any(|entry| entry.id == "example.panic"));
        let error = host
            .execute_command(command(
                active.generation.as_deref().unwrap(),
                "example.panic",
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code, PLUGIN_PANIC);
        let listed = host
            .list()
            .await
            .expect("host must still answer list after caught native panic");
        assert_eq!(listed.plugins.len(), 1);
        assert_eq!(listed.plugins[0].id, ID);
        assert!(matches!(
            listed.plugins[0].state,
            PluginState::Quarantined | PluginState::Failed
        ));
        assert_eq!(
            listed.plugins[0].last_error.as_ref().unwrap().code,
            PLUGIN_PANIC
        );
        assert_eq!(host.diagnostics().active_instances, 0);
        assert_eq!(host.diagnostics().broker_registrations, 0);
        enable(&host, false).await;
        close(host).await;
    })
    .await
    .expect("caught-panic acceptance must remain bounded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cdylib_escaped_host_is_rejected_after_teardown() {
    // Safe SDK code can move its owned Host into worker-thread TLS. The TLS
    // destructor then calls the host after destroy and worker exit; the host
    // must answer SEP_UNAVAILABLE from a still-valid (retired) context.
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let artifacts = artifacts();
    tokio::time::timeout(Duration::from_secs(20), async {
        let profile = tempfile::tempdir().unwrap();
        let result_file = profile.path().join("late-host-status");
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .unwrap();
        host.install(InstallRequest {
            path: path_string(&artifacts.fault),
        })
        .await
        .unwrap();
        let active = enable(&host, true).await;
        let mut request = command(active.generation.as_deref().unwrap(), "example.escape-host");
        request.args = serde_json::json!({ "resultFile": path_string(&result_file) });
        host.execute_command(request).await.unwrap();
        enable(&host, false).await;
        let mut status = None;
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&result_file) {
                status = Some(text);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            status.as_deref(),
            Some(se_plugin_abi::SEP_UNAVAILABLE.to_string().as_str()),
            "late callback from TLS destructor must be rejected as unavailable"
        );
        let listed = host.list().await.expect("host must stay healthy");
        assert_eq!(listed.plugins.len(), 1);
        let again = enable(&host, true).await;
        assert!(
            again.generation.is_some(),
            "plugin must re-activate after escape"
        );
        enable(&host, false).await;
        close(host).await;
    })
    .await
    .expect("escaped-host acceptance must remain bounded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cdylib_timeout_requests_restart_without_waiting_for_callback() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let artifacts = artifacts();
    tokio::time::timeout(Duration::from_secs(20), async {
        let profile = tempfile::tempdir().unwrap();
        let options = HostOptions::default();
        assert_eq!(options.callback_timeout, Duration::from_secs(2));
        let host = PluginHost::open(profile.path().into(), options)
            .await
            .unwrap();
        host.install(InstallRequest {
            path: path_string(&artifacts.fault),
        })
        .await
        .unwrap();
        let active = enable(&host, true).await;
        let mut restart = host.restart_requested();
        assert!(restart.borrow().is_none());
        let before = host.diagnostics();
        let error = tokio::time::timeout(
            Duration::from_millis(2750),
            host.execute_command(command(
                active.generation.as_deref().unwrap(),
                "example.timeout",
            )),
        )
        .await
        .expect("3-second callback must return a timeout error in under 2750ms")
        .unwrap_err();
        assert_eq!(error.code, PLUGIN_TIMEOUT);
        tokio::time::timeout(Duration::from_secs(1), restart.changed())
            .await
            .expect("restart notification must be bounded")
            .expect("restart watch remains open");
        let reason = restart.borrow().clone().expect("restart reason");
        assert_eq!(reason.plugin_id.as_deref(), Some(ID));
        assert_eq!(reason.generation, active.generation.unwrap());
        assert_eq!(reason.code, PLUGIN_TIMEOUT);
        let after = host.diagnostics();
        assert_eq!(after.active_instances, 0);
        assert_eq!(after.broker_registrations, 0);
        assert_eq!(after.view_count, 0);
        assert_eq!(after.command_count, 0);
        assert_eq!(
            after.callbacks_in_flight, 1,
            "native callback still owns its memory"
        );
        assert_eq!(
            after.destroy_attempts, before.destroy_attempts,
            "never destroy an in-flight native instance"
        );
        assert_eq!(after.loaded_libraries, before.loaded_libraries);
        let listed = host
            .list()
            .await
            .expect("host remains manageable after timeout");
        assert_eq!(
            listed.plugins[0].last_error.as_ref().unwrap().code,
            PLUGIN_TIMEOUT
        );
        // The fixture sleeps for 3 seconds, not forever. Wait for it to return
        // before dropping the TempDir so no native worker races profile cleanup.
        tokio::time::timeout(Duration::from_secs(6), async {
            while host.diagnostics().callbacks_in_flight != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("bounded timeout fixture must eventually leave foreign code");
        assert_eq!(host.diagnostics().destroy_attempts, before.destroy_attempts);
        close(host).await;
    })
    .await
    .expect("timeout acceptance must remain bounded");
}

fn activity_context() -> HostContextState {
    serde_json::from_value(serde_json::json!({
        "activeProject":{"id":"p1","name":"Pilot project","path":"/synthetic/pilot"},
        "projectIds":["p1"],"activeTerminalId":null,"activeSessionId":"s1",
        "terminals":[],"agentSessions":[{"id":"s1","projectId":"p1","status":"active","activeTurn":false,"openTurnId":null,"replaying":false}],
        "focus":{"textInput":false,"terminal":false},"truncated":false
    })).unwrap()
}

async fn publish_activity(
    host: &PluginHost,
    source: &str,
    sequence: u64,
    reset: bool,
    context: &HostContextState,
    events: Vec<HostSourceEvent>,
) -> HostContextAck {
    host.publish_host_context(
        "trusted-test-main",
        HostContextPublishRequest {
            source_id: source.into(),
            source_sequence: sequence,
            reset,
            snapshot: context.clone(),
            events,
        },
    )
    .await
    .expect("publish trusted synthetic context")
}

fn source_event(topic: &str, data: Value) -> HostSourceEvent {
    serde_json::from_value(serde_json::json!({"topic":topic,"projectId":"p1","data":data})).unwrap()
}

fn node_text(snapshot: &ViewSnapshot, id: &str) -> String {
    match snapshot.tree.root.find(id) {
        Some(UiNode::Text { props, .. }) => props.text.clone(),
        other => panic!("missing semantic activity text {id}: {other:?}"),
    }
}

async fn activity_inspect(host: &PluginHost, generation: &str) -> Value {
    host.execute_command(command(generation, "example.activity-inspect"))
        .await
        .unwrap()
        .result
}

async fn wait_activity(host: &PluginHost, id: &str, expected: &str) -> ViewSnapshot {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let snapshot = view(host).await;
            if node_text(&snapshot, id) == expected {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("activity node {id} never became {expected}"))
}

async fn wait_tickets(host: &PluginHost, generation: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let state = activity_inspect(host, generation).await;
            if state["pendingTicketIds"].as_array().unwrap().is_empty()
                && state["state"] != "Loading activity"
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("all accepted storage tickets must complete")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_pilot_native_context_storage_ui_and_generation_lifecycle() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let artifacts = artifacts();
    let profile = tempfile::tempdir().unwrap();
    let host = PluginHost::open(profile.path().into(), HostOptions::default())
        .await
        .unwrap();
    host.install(InstallRequest {
        path: path_string(&artifacts.fault),
    })
    .await
    .unwrap();
    let first = enable(&host, true).await;
    let generation = first.generation.as_deref().unwrap();
    assert_eq!(
        node_text(&view(&host).await, "activity-state"),
        "Loading activity"
    );
    assert_eq!(first.manifest.services.requires.len(), 8);
    assert_eq!(first.manifest.contributes.event_subscriptions.len(), 5);
    assert_eq!(first.manifest.contributes.menus.len(), 4);
    assert_eq!(first.manifest.contributes.keybindings[0].key, "");
    let source = Uuid::new_v4().to_string();
    let mut context = activity_context();
    let ack = publish_activity(&host, &source, 1, true, &context, vec![]).await;
    wait_activity(&host, "activity-state", "Agent idle").await;
    let initialized = wait_tickets(&host, generation).await;
    assert_eq!(initialized["sourceId"], source);
    assert_eq!(initialized["observedTurns"], 0);
    assert!(initialized["sequences"]["activity"].is_number());
    assert_eq!(
        host.heartbeat_host_context("trusted-test-main", &source)
            .unwrap(),
        ack
    );

    let terminal = TerminalContext {
        id: "t1".into(),
        pty_id: Some("pty1".into()),
        project_id: Some("p1".into()),
    };
    context.terminals.push(terminal.clone());
    publish_activity(
        &host,
        &source,
        2,
        false,
        &context,
        vec![source_event(
            "terminal.created",
            serde_json::json!({"terminal":terminal}),
        )],
    )
    .await;
    wait_activity(&host, "activity-terminal-count", "1").await;
    context.active_terminal_id = Some("t1".into());
    let ack = publish_activity(
        &host,
        &source,
        3,
        false,
        &context,
        vec![source_event(
            "terminal.activeChanged",
            serde_json::json!({"terminalId":"t1","projectId":"p1"}),
        )],
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while activity_inspect(&host, generation).await["activeTerminalId"] != "t1" {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    // Every location resolves the command from the manifest, not caller text,
    // and native on_command observes the actual clicked (not active) target.
    for (id, target, source_name) in [
        (
            "activity-terminal-tab",
            serde_json::json!({"projectId":"p1","terminalId":"t1"}),
            "terminal.tab",
        ),
        (
            "activity-terminal-context",
            serde_json::json!({"projectId":"p1","terminalId":"t1"}),
            "terminal.context",
        ),
        (
            "activity-project-context",
            serde_json::json!({"projectId":"p1"}),
            "project.context",
        ),
        (
            "activity-agent-session",
            serde_json::json!({"projectId":"p1","sessionId":"s1"}),
            "agentSession.context",
        ),
    ] {
        let reply = host
            .execute_contribution(ExecuteContributionRequest {
                plugin_id: ID.into(),
                generation: generation.into(),
                kind: ContributionKind::Menu,
                contribution_id: id.into(),
                action_index: None,
                record_revision: None,
                context_revision: ack.context_revision,
                target: serde_json::from_value(target).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(reply.result["observedTarget"]["source"], source_name);
        assert_eq!(reply.result["observedTarget"]["context"]["projectId"], "p1");
        if source_name.starts_with("terminal") {
            assert_eq!(
                reply.result["observedTarget"]["context"]["terminalId"],
                "t1"
            );
        }
    }
    // Let the deliberately bounded feedback bucket refill before turn effects.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    context.agent_sessions[0].active_turn = true;
    context.agent_sessions[0].open_turn_id = Some("turn-1".into());
    publish_activity(
        &host,
        &source,
        4,
        false,
        &context,
        vec![source_event(
            "agent.session.stateChanged",
            serde_json::json!({"session":context.agent_sessions[0],"removed":false}),
        )],
    )
    .await;
    wait_activity(&host, "activity-state", "Agent active").await;
    let running = host.host_ui_snapshot().await.unwrap();
    validate_host_ui_snapshot(&running).unwrap();
    assert!(running
        .status_items
        .iter()
        .any(|s| s.text == "Agent active"));
    let progress = running
        .progress
        .iter()
        .find(|p| p.id == "activity-turn")
        .unwrap()
        .clone();
    assert_eq!(progress.state, ProgressState::Running);
    context.agent_sessions[0].active_turn = false;
    context.agent_sessions[0].open_turn_id = None;
    publish_activity(
        &host,
        &source,
        5,
        false,
        &context,
        vec![source_event(
            "agent.session.stateChanged",
            serde_json::json!({"session":context.agent_sessions[0],"removed":false}),
        )],
    )
    .await;
    wait_activity(&host, "activity-state", "Agent idle").await;
    wait_activity(&host, "activity-observed-turns", "1").await;
    let stored = wait_tickets(&host, generation).await;
    assert_eq!(stored["projectRevision"], 1);
    let ended = host.host_ui_snapshot().await.unwrap();
    assert_eq!(
        ended
            .notifications
            .iter()
            .filter(|n| n.title == "Agent turn ended")
            .count(),
        1
    );
    let ended_notice = ended
        .notifications
        .iter()
        .find(|n| n.title == "Agent turn ended")
        .unwrap()
        .clone();
    assert!(ended.status_items.iter().any(|s| s.text == "Agent idle"));
    let completed = ended.progress.iter().find(|p| p.id == progress.id).unwrap();
    assert_eq!(completed.state, ProgressState::Completed);
    assert_eq!(
        completed.created_at, progress.created_at,
        "progress updates in place"
    );
    assert!(completed.revision > progress.revision);
    let data: Value = serde_json::from_slice(
        &fs::read(profile.path().join("plugins/data/org.termul.example.json")).unwrap(),
    )
    .unwrap();
    assert!(data.to_string().contains("observedTurns"));

    // A stale CAS travels through a real storage completion. Re-read revision
    // then recompute 1 + the one unsaved observation; never overwrite with 1.
    let mut conflict = command(generation, "example.activity-inspect");
    conflict.args = serde_json::json!({"forceConflict":true});
    host.execute_command(conflict).await.unwrap();
    let recovered_cas = wait_tickets(&host, generation).await;
    assert_eq!(recovered_cas["observedTurns"], 2);
    assert_eq!(recovered_cas["projectRevision"], 2);

    // Reset/replay replaces the baseline: no retrospective turn end and no
    // revision bump to the already-present notification.
    context.agent_sessions[0].active_turn = true;
    context.agent_sessions[0].replaying = true;
    publish_activity(&host, &source, 6, true, &context, vec![]).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    context.agent_sessions[0].active_turn = false;
    publish_activity(
        &host,
        &source,
        7,
        false,
        &context,
        vec![source_event(
            "agent.session.stateChanged",
            serde_json::json!({"session":context.agent_sessions[0],"removed":false}),
        )],
    )
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        activity_inspect(&host, generation).await["observedTurns"],
        2
    );
    assert_eq!(
        host.host_ui_snapshot()
            .await
            .unwrap()
            .notifications
            .iter()
            .find(|n| n.id == ended_notice.id)
            .unwrap()
            .revision,
        ended_notice.revision
    );

    context.terminals.clear();
    context.active_terminal_id = None;
    publish_activity(
        &host,
        &source,
        8,
        false,
        &context,
        vec![source_event(
            "terminal.closed",
            serde_json::json!({"terminal":terminal}),
        )],
    )
    .await;
    wait_activity(&host, "activity-terminal-count", "0").await;
    let invalid = host
        .execute_contribution(ExecuteContributionRequest {
            plugin_id: ID.into(),
            generation: generation.into(),
            kind: ContributionKind::Menu,
            contribution_id: "activity-terminal-context".into(),
            action_index: None,
            record_revision: None,
            context_revision: ack.context_revision,
            target: ContributionTarget {
                project_id: Some("p1".into()),
                terminal_id: Some("t1".into()),
                session_id: None,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(invalid.code, PLUGIN_INVALID_ACTION);

    // Revocation removes transient handles, while durable preference and count
    // survive a fresh instance. No old generation's action is admitted.
    host.execute_command(command(generation, "example.activity-toggle"))
        .await
        .unwrap();
    let disabled_pref = wait_tickets(&host, generation).await;
    assert_eq!(disabled_pref["enabled"], false);
    let stale_action = button_action(&view(&host).await, "increment");
    enable(&host, false).await;
    assert!(host
        .host_ui_snapshot()
        .await
        .unwrap()
        .status_items
        .is_empty());
    let second = enable(&host, true).await;
    assert_ne!(second.generation, first.generation);
    wait_activity(&host, "activity-state", "Activity disabled").await;
    let state = wait_tickets(&host, second.generation.as_deref().unwrap()).await;
    assert_eq!(state["observedTurns"], 2);
    assert_eq!(
        host.dispatch_action(stale_action).await.unwrap_err().code,
        PLUGIN_STALE_GENERATION
    );
    host.execute_command(command(
        second.generation.as_deref().unwrap(),
        "example.activity-toggle",
    ))
    .await
    .unwrap();
    wait_tickets(&host, second.generation.as_deref().unwrap()).await;
    let upgraded = host
        .reload(ReloadRequest {
            id: ID.into(),
            path: Some(path_string(&artifacts.upgrade)),
        })
        .await
        .unwrap();
    assert!(!upgraded
        .manifest
        .contributes
        .commands
        .iter()
        .any(|c| c.id.contains("inspect") || c.id.contains("fault")));
    assert_eq!(
        host.execute_command(command(
            second.generation.as_deref().unwrap(),
            "example.activity-toggle"
        ))
        .await
        .unwrap_err()
        .code,
        PLUGIN_STALE_GENERATION
    );
    wait_activity(&host, "activity-observed-turns", "2").await;
    wait_activity(&host, "activity-state", "Agent idle").await;
    host.detach_host_context("trusted-test-main");
    let lost = host.host_ui_snapshot().await.unwrap();
    assert!(
        lost.notifications.is_empty() && lost.progress.is_empty() && lost.status_items.is_empty()
    );
    let restored_source = Uuid::new_v4().to_string();
    publish_activity(&host, &restored_source, 1, true, &context, vec![]).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while host
            .host_ui_snapshot()
            .await
            .unwrap()
            .status_items
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        host.host_ui_snapshot()
            .await
            .unwrap()
            .notifications
            .is_empty(),
        "source restoration must not replay old effects"
    );
    close(host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_pilot_queue_overflow_recovers_from_latest_snapshot_without_notifications() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    let profile = tempfile::tempdir().unwrap();
    let host = PluginHost::open(profile.path().into(), HostOptions::default())
        .await
        .unwrap();
    host.install(InstallRequest {
        path: path_string(&artifacts().fault),
    })
    .await
    .unwrap();
    let active = enable(&host, true).await;
    let generation = active.generation.as_deref().unwrap();
    let source = Uuid::new_v4().to_string();
    let mut context = activity_context();
    publish_activity(&host, &source, 1, true, &context, vec![]).await;
    wait_tickets(&host, generation).await;
    // Three batches exceed the 128-event queue and the 20/s delivery bucket.
    // Final context is authoritative even when individual deltas are discarded.
    for sequence in 2..=4 {
        let mut events = vec![];
        for index in 0..128 {
            let terminal = TerminalContext {
                id: format!("overflow-{sequence}-{index}"),
                pty_id: None,
                project_id: Some("p1".into()),
            };
            if sequence == 4 && index == 127 {
                context.terminals = vec![terminal.clone()];
            }
            events.push(source_event(
                if index % 2 == 0 {
                    "terminal.created"
                } else {
                    "terminal.closed"
                },
                serde_json::json!({"terminal":terminal}),
            ));
        }
        publish_activity(&host, &source, sequence, false, &context, events).await;
    }
    let latest_revision = host
        .heartbeat_host_context("trusted-test-main", &source)
        .unwrap()
        .context_revision;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let state = activity_inspect(&host, generation).await;
            if state["contextRevision"] == latest_revision && state["terminalCount"] == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("reserved resync marker must recover latest bounded snapshot");
    assert_eq!(
        activity_inspect(&host, generation).await["observedTurns"],
        0
    );
    assert!(host
        .host_ui_snapshot()
        .await
        .unwrap()
        .notifications
        .is_empty());
    close(host).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_host_delivery_panic_and_timeout_are_quarantined() {
    let _native_guard = NATIVE_TEST_LOCK.lock().await;
    for (mode, code) in [("panic", PLUGIN_PANIC), ("timeout", PLUGIN_TIMEOUT)] {
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().into(), HostOptions::default())
            .await
            .unwrap();
        host.install(InstallRequest {
            path: path_string(&artifacts().fault),
        })
        .await
        .unwrap();
        let active = enable(&host, true).await;
        let generation = active.generation.as_deref().unwrap();
        let mut fault = command(generation, "example.activity-delivery-fault");
        fault.args = serde_json::json!({"mode":mode});
        host.execute_command(fault).await.unwrap();
        let mut restart = host.restart_requested();
        publish_activity(
            &host,
            &Uuid::new_v4().to_string(),
            1,
            true,
            &activity_context(),
            vec![],
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let info = host.list().await.unwrap().plugins.remove(0);
                if info.last_error.as_ref().is_some_and(|e| e.code == code) {
                    assert!(matches!(
                        info.state,
                        PluginState::Quarantined | PluginState::Failed
                    ));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(host.diagnostics().active_instances, 0);
        assert!(host
            .host_ui_snapshot()
            .await
            .unwrap()
            .status_items
            .is_empty());
        if mode == "timeout" {
            if restart.borrow().is_none() {
                tokio::time::timeout(Duration::from_secs(1), restart.changed())
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert_eq!(restart.borrow().as_ref().unwrap().code, PLUGIN_TIMEOUT);
            assert_eq!(restart.borrow().as_ref().unwrap().generation, generation);
            tokio::time::timeout(Duration::from_secs(6), async {
                while host.diagnostics().callbacks_in_flight != 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
        close(host).await;
    }
}
