//! Real Plugin Core process acceptance. Run with `bash scripts/test-plugins.sh`:
//! artifacts must already exist; this test never invokes cargo or uses a user profile.

#![cfg(unix)]

use se_manager_lib::core::{
    ensure_core, AcpCoreClient, CoreEndpoint, CoreLaunchConfig, CoreRole, PluginCoreClient,
    TerminalCoreClient,
};
use se_manager_lib::SpawnOptions;
use se_plugin_protocol::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const ID: &str = "org.termul.example";
const VIEW: &str = "example.counter";
const WAIT: Duration = Duration::from_secs(15);

/// Only PIDs returned by a fresh ensure on OUR temporary profile enter this guard.
/// Register each immediately: even a failure spawning the next Core must clean up.
/// Reap promptly and forget dead PIDs so a later drop cannot signal a recycled PID.
#[derive(Default)]
struct PidReaper(Vec<u32>);

impl PidReaper {
    fn own(&mut self, pid: u32) {
        assert!(pid > 0, "a freshly spawned Core must have a real PID");
        self.0.push(pid);
    }

    /// Observe exit WITHOUT reaping: ensure_core owns std::process::Child and
    /// must consume its status itself before it will replace a dead Core.
    async fn stopped(&self, pid: u32) -> i32 {
        assert!(self.0.contains(&pid));
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            // SAFETY: siginfo_t is plain data; waitid writes a child status.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            assert_eq!(
                result,
                0,
                "waitid({pid}): {}",
                std::io::Error::last_os_error()
            );
            if info.si_signo != 0 {
                return info.si_code;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Core {pid} did not exit"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn launcher_reaped(&mut self, pid: u32) {
        // Call immediately after successful ensure, before awaiting other work.
        self.0.retain(|owned| *owned != pid);
    }

    async fn exited(&mut self, pid: u32) -> i32 {
        assert!(self.0.contains(&pid), "may only reap our own child");
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let mut status = 0;
            // SAFETY: pid is a child recorded by this guard; status is writable.
            let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
            if result == pid as i32 {
                self.0.retain(|owned| *owned != pid);
                return status;
            }
            assert!(
                result >= 0,
                "waitpid({pid}): {}",
                std::io::Error::last_os_error()
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "Core {pid} did not exit"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for PidReaper {
    fn drop(&mut self) {
        for pid in self.0.drain(..) {
            // ensure_core may reap the old child and then fail to launch its
            // replacement. Confirm this is STILL our child before signaling;
            // ECHILD means the recorded numeric PID is no longer safe to use.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let observed = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if observed != 0 {
                continue;
            }
            // SAFETY: our unreaped child cannot have had its PID recycled.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                let mut status = 0;
                let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
                if result != 0 || std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn artifact(variant: &str) -> PathBuf {
    let root = std::env::var_os("SE_PLUGIN_TEST_ARTIFACTS")
        .expect("SE_PLUGIN_TEST_ARTIFACTS is required; run bash scripts/test-plugins.sh");
    let name = format!(
        "{}se_plugin_example{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let path = PathBuf::from(root).join(variant).join(name);
    assert!(
        path.is_file(),
        "missing REAL {variant} artifact: {}",
        path.display()
    );
    path.canonicalize().expect("canonical artifact path")
}

async fn rpc<T: DeserializeOwned>(client: &PluginCoreClient, method: &str, params: Value) -> T {
    let value = client
        .request(method, params)
        .await
        .unwrap_or_else(|error| {
            panic!("{method} failed: {error:?}");
        });
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{method} reply: {error}"))
}

async fn connect_plugin(endpoint: &CoreEndpoint) -> PluginCoreClient {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(client) = PluginCoreClient::connect(endpoint).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("Plugin Core health must become ready")
}

async fn reconnect_plugin(client: &PluginCoreClient, endpoint: &CoreEndpoint) {
    tokio::time::timeout(WAIT, async {
        loop {
            if client.reconnect(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("same GUI client must reconnect after Plugin Core initialization");
}

async fn health(client: &PluginCoreClient) -> HealthSnapshot {
    let snapshot: HealthSnapshot = rpc(client, METHOD_HEALTH, json!({})).await;
    assert!(snapshot.ready);
    assert!(
        !snapshot.safe_mode,
        "one attributed abort must not force safe mode"
    );
    assert_eq!(client.boot_id().as_deref(), Some(snapshot.boot_id.as_str()));
    snapshot
}

async fn active(client: &PluginCoreClient) -> PluginInfo {
    tokio::time::timeout(WAIT, async {
        loop {
            let snapshot: PluginSnapshot = rpc(client, METHOD_LIST, json!({})).await;
            assert_eq!(snapshot.plugins.len(), 1);
            let info = snapshot.plugins.into_iter().next().unwrap();
            assert_eq!(info.id, ID);
            if info.state == PluginState::Active {
                assert!(info.desired_enabled);
                assert!(info.generation.is_some());
                return info;
            }
            assert!(
                matches!(info.state, PluginState::Pending | PluginState::Activating),
                "plugin failed to reactivate: {info:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("installed plugin must become active")
}

async fn view(client: &PluginCoreClient) -> ViewSnapshot {
    rpc(client, METHOD_GET_VIEW, json!({"id": ID, "viewId": VIEW})).await
}

fn text(snapshot: &ViewSnapshot, id: &str) -> Value {
    let tree = serde_json::to_value(&snapshot.tree).unwrap();
    tree["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("missing {id} node in {tree}"))["props"]["text"]
        .clone()
}

fn increment(snapshot: &ViewSnapshot) -> ActionEvent {
    ActionEvent {
        schema_version: 1,
        event_id: uuid::Uuid::new_v4().to_string(),
        plugin_id: ID.into(),
        generation: snapshot.generation.clone(),
        view_id: VIEW.into(),
        view_revision: snapshot.revision,
        node_id: "increment".into(),
        action_id: "increment".into(),
        kind: ActionKind::Click,
        value: ActionValue::Null,
    }
}

async fn output_contains(
    output: &mut tokio::sync::mpsc::Receiver<se_manager_lib::core::OutputFrame>,
    marker: &str,
) {
    let mut observed = String::new();
    let result = tokio::time::timeout(WAIT, async {
        while let Some(frame) = output.recv().await {
            observed.push_str(&String::from_utf8_lossy(&frame.data));
            if observed.contains(marker) {
                return;
            }
        }
        panic!("PTY output closed before {marker}: {observed:?}");
    })
    .await;
    assert!(result.is_ok(), "missing PTY marker {marker}: {observed:?}");
}

fn process_identity(pid: u32) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "pid=,lstart=,command=", "-p", &pid.to_string()])
        .output()
        .expect("ps identity of our Core");
    assert!(output.status.success(), "Core {pid} no longer exists");
    let identity = String::from_utf8(output.stdout).unwrap();
    assert!(!identity.trim().is_empty());
    identity
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_core_isolation_and_recovery() {
    // Fail before spawning anything if the runner has not built both real libraries.
    let normal = artifact("normal");
    let fault = artifact("fault");
    let profile = tempfile::Builder::new()
        .prefix("plugin-core-smoke-")
        .tempdir()
        .unwrap();
    let config = CoreLaunchConfig {
        profile_root: profile.path().to_path_buf(),
        executable: PathBuf::from(env!("CARGO_BIN_EXE_se-manager")),
        ready_timeout: WAIT,
    };
    let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::PluginCore);
    let mut reaper = PidReaper::default();

    let first = ensure_core(CoreRole::PluginCore, &config)
        .await
        .expect("spawn Plugin Core");
    reaper.own(first.pid);
    assert!(!first.reused);
    let gui = connect_plugin(&endpoint).await;
    let initial = health(&gui).await;
    assert_eq!(initial.pid, first.pid);

    let adopted = ensure_core(CoreRole::PluginCore, &config)
        .await
        .expect("adopt Plugin Core");
    assert!(adopted.reused, "second ensure must adopt rather than spawn");
    // The adoption API returns pid=0; use the actual process health as authority.
    assert_eq!(health(&gui).await.pid, first.pid);
    gui.disconnect().await;
    drop(gui);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let gui = connect_plugin(&endpoint).await;
    assert_eq!(
        health(&gui).await,
        initial,
        "GUI disconnect must not end Core"
    );
    let unknown = gui
        .request("plugins.no_such_method", json!({}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, PLUGIN_INVALID_REQUEST);
    assert!(
        gui.is_connected(),
        "business errors must not retire the connection"
    );
    assert_eq!(health(&gui).await.pid, first.pid);

    let installed: PluginInfo = rpc(&gui, METHOD_INSTALL, json!({"path": normal})).await;
    assert_eq!(installed.id, ID);
    assert_eq!(installed.state, PluginState::Disabled);
    assert_eq!(installed.manifest.contributes.views.len(), 1);
    let contribution = &installed.manifest.contributes.views[0];
    assert_eq!(contribution.id, VIEW);
    assert_eq!(contribution.location, ViewLocation::Sidebar);
    assert_eq!(contribution.icon.as_deref(), Some("calculator"));
    let _: PluginInfo = rpc(&gui, METHOD_SET_ENABLED, json!({"id": ID, "enabled": true})).await;
    let old_info = active(&gui).await;
    let original_view = view(&gui).await;
    assert_eq!(text(&original_view, "heading"), "Hello");
    assert_eq!(text(&original_view, "count"), "0");
    assert_eq!(
        Some(&original_view.generation),
        old_info.generation.as_ref()
    );
    let incremented: ViewSnapshot = rpc(
        &gui,
        METHOD_ACTION,
        serde_json::to_value(increment(&original_view)).unwrap(),
    )
    .await;
    assert_eq!(text(&incremented, "count"), "1");
    let current: ConfigSnapshot = rpc(&gui, METHOD_GET_CONFIG, json!({"id": ID})).await;
    let saved: ConfigSnapshot = rpc(
        &gui,
        METHOD_SET_CONFIG,
        json!({
            "id": ID, "expectedRevision": current.revision,
            "values": {"greeting": "Hola", "step": 2, "showCount": true, "tone": "muted"}
        }),
    )
    .await;
    assert!(saved.revision > current.revision);
    let old_view = view(&gui).await;
    assert_eq!(text(&old_view, "heading"), "Hola");
    let old_action = increment(&old_view);

    // A deliberate restart must be clean: an unattributed external kill would
    // correctly quarantine all enabled plugins instead of auto-reactivating them.
    let _: Value = rpc(&gui, METHOD_SHUTDOWN, json!({})).await;
    assert_eq!(reaper.stopped(first.pid).await, libc::CLD_EXITED);
    let restarted = ensure_core(CoreRole::PluginCore, &config)
        .await
        .expect("restart Plugin Core");
    reaper.launcher_reaped(first.pid);
    reaper.own(restarted.pid);
    assert!(!restarted.reused);
    reconnect_plugin(&gui, &endpoint).await;
    let second_health = health(&gui).await;
    assert_eq!(second_health.pid, restarted.pid);
    assert_ne!(second_health.boot_id, initial.boot_id);
    let stale = gui
        .request(METHOD_ACTION, serde_json::to_value(old_action).unwrap())
        .await
        .unwrap_err();
    assert_eq!(stale.code, PLUGIN_STALE_GENERATION);
    let restored = active(&gui).await;
    assert_ne!(restored.generation, old_info.generation);
    let persisted: ConfigSnapshot = rpc(&gui, METHOD_GET_CONFIG, json!({"id": ID})).await;
    assert_eq!(
        persisted, saved,
        "normal plugin config must survive Core restart"
    );
    let restored_view = view(&gui).await;
    assert_eq!(text(&restored_view, "heading"), "Hola");
    assert_eq!(
        text(&restored_view, "count"),
        "0",
        "private counter resets on Core restart"
    );
    let incremented: ViewSnapshot = rpc(
        &gui,
        METHOD_ACTION,
        serde_json::to_value(increment(&restored_view)).unwrap(),
    )
    .await;
    assert_eq!(
        text(&incremented, "count"),
        "2",
        "persisted step applied by real cdylib"
    );

    // Run all three roles on this same temporary profile. Guard each successful
    // spawn before attempting the next, unlike a guard constructed after both.
    let terminal_process = ensure_core(CoreRole::TerminalCore, &config)
        .await
        .expect("spawn Terminal Core");
    reaper.own(terminal_process.pid);
    assert!(!terminal_process.reused);
    let acp_process = ensure_core(CoreRole::AcpCore, &config)
        .await
        .expect("spawn ACP Core");
    reaper.own(acp_process.pid);
    assert!(!acp_process.reused);
    let terminal_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
    let acp_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);
    let terminal = TerminalCoreClient::connect(&terminal_endpoint)
        .await
        .expect("Terminal Core ready");
    let acp = AcpCoreClient::connect(&acp_endpoint)
        .await
        .expect("ACP Core ready");
    assert_eq!(acp.health().await.unwrap()["status"], "ready");
    let terminal_identity = process_identity(terminal_process.pid);
    let acp_identity = process_identity(acp_process.pid);
    let pty = terminal
        .spawn(SpawnOptions {
            cwd: Some(profile.path().to_string_lossy().into_owned()),
            cols: Some(80),
            rows: Some(24),
            shell: Some("/bin/sh".into()),
            env: Some(HashMap::from([
                ("HOME".into(), profile.path().to_string_lossy().into_owned()),
                ("HISTFILE".into(), "/dev/null".into()),
                ("ENV".into(), "/dev/null".into()),
                ("PS1".into(), "$ ".into()),
            ])),
            ..Default::default()
        })
        .await
        .expect("spawn PTY before native fault");
    let mut session = terminal
        .attach(&pty.info.id, &pty.claim, 0)
        .await
        .expect("attach PTY");
    // Split markers in the input so terminal echo cannot satisfy these checks.
    terminal
        .write(
            &pty.info.id,
            "stty -echo; printf 'PLUGIN_CORE_%s\\n' READY\n",
        )
        .await
        .unwrap();
    output_contains(&mut session.output, "PLUGIN_CORE_READY").await;

    // Both fixtures intentionally have the SAME manifest ID. The correct IPC
    // install/enable path for the fault build is same-version reload of the
    // installed normal plugin, preserving its durable config (not a duplicate ID).
    let fault_info: PluginInfo = rpc(&gui, METHOD_RELOAD, json!({"id": ID, "path": fault})).await;
    assert_eq!(fault_info.state, PluginState::Active);
    assert_ne!(fault_info.sha256, restored.sha256);
    assert_ne!(fault_info.generation, restored.generation);
    assert!(fault_info
        .manifest
        .contributes
        .commands
        .iter()
        .any(|command| command.id == "example.abort"));
    let fault_config: ConfigSnapshot = rpc(&gui, METHOD_GET_CONFIG, json!({"id": ID})).await;
    assert_eq!(fault_config, saved);
    let error = gui
        .request(
            METHOD_COMMAND,
            json!({
                "id": ID, "generation": fault_info.generation.unwrap(),
                "commandId": "example.abort", "args": null
            }),
        )
        .await
        .expect_err("real abort must kill the Plugin Core, not return a command reply");
    assert_eq!(error.code, PLUGIN_CORE_UNAVAILABLE);
    let cause = reaper.stopped(restarted.pid).await;
    assert!(
        matches!(cause, libc::CLD_KILLED | libc::CLD_DUMPED),
        "real abort callback must kill process by signal: si_code={cause}"
    );
    assert!(!gui.is_connected());
    assert!(PluginCoreClient::connect(&endpoint).await.is_err());

    // Neither other role was restarted: compare PID + start time + executable,
    // and use the SAME clients and PTY output stream after Plugin Core has died.
    assert_eq!(process_identity(terminal_process.pid), terminal_identity);
    assert_eq!(process_identity(acp_process.pid), acp_identity);
    assert!(terminal
        .list()
        .await
        .unwrap()
        .iter()
        .any(|entry| entry.id == pty.info.id));
    assert_eq!(acp.health().await.unwrap()["status"], "ready");
    terminal
        .write(&pty.info.id, "printf 'PLUGIN_CORE_%s\\n' SURVIVES\n")
        .await
        .expect("PTY write after Plugin Core abort");
    output_contains(&mut session.output, "PLUGIN_CORE_SURVIVES").await;

    let recovered = ensure_core(CoreRole::PluginCore, &config)
        .await
        .expect("recover after native abort");
    reaper.launcher_reaped(restarted.pid);
    reaper.own(recovered.pid);
    assert!(!recovered.reused);
    reconnect_plugin(&gui, &endpoint).await;
    let final_health = health(&gui).await;
    assert_eq!(final_health.pid, recovered.pid);
    assert_ne!(final_health.boot_id, second_health.boot_id);
    // Observe twice, without enabling/reloading: quarantine must persist rather
    // than merely exposing a transient pre-activation state at boot.
    for _ in 0..2 {
        let snapshot: PluginSnapshot = rpc(&gui, METHOD_LIST, json!({})).await;
        assert_eq!(snapshot.plugins.len(), 1);
        let quarantined = &snapshot.plugins[0];
        assert_eq!(quarantined.id, ID);
        assert!(quarantined.desired_enabled);
        assert_eq!(quarantined.state, PluginState::Quarantined);
        assert!(quarantined.generation.is_none());
        assert_eq!(
            quarantined.last_error.as_ref().unwrap().code,
            PLUGIN_QUARANTINED
        );
        let config: ConfigSnapshot = rpc(&gui, METHOD_GET_CONFIG, json!({"id": ID})).await;
        assert_eq!(
            config, saved,
            "normal artifact config must survive the fault quarantine"
        );
        assert_eq!(health(&gui).await.active_resources, 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(process_identity(terminal_process.pid), terminal_identity);
    assert_eq!(process_identity(acp_process.pid), acp_identity);
    assert!(
        ensure_core(CoreRole::TerminalCore, &config)
            .await
            .unwrap()
            .reused
    );
    assert!(
        ensure_core(CoreRole::AcpCore, &config)
            .await
            .unwrap()
            .reused
    );

    drop(session);
    terminal
        .terminate(&pty.info.id)
        .await
        .expect("terminate smoke PTY");
    let _: Value = rpc(&gui, METHOD_SHUTDOWN, json!({})).await;
    acp.shutdown().await.expect("shutdown smoke ACP Core");
    terminal
        .shutdown()
        .await
        .expect("shutdown smoke Terminal Core");
    for pid in [recovered.pid, acp_process.pid, terminal_process.pid] {
        let status = reaper.exited(pid).await;
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "unclean shutdown of {pid}: {status}"
        );
    }
    assert!(
        reaper.0.is_empty(),
        "all temporary Core processes were reaped"
    );
    assert!(PluginCoreClient::connect(&endpoint).await.is_err());
    assert!(TerminalCoreClient::connect(&terminal_endpoint)
        .await
        .is_err());
    assert!(AcpCoreClient::connect(&acp_endpoint).await.is_err());
    eprintln!(
        "plugin_core_smoke: profile={} all owned Core PIDs reaped; PLUGIN_CORE_SURVIVES observed",
        profile.path().display()
    );
}
