//! Dual-Core packaged smoke: both role processes live on ONE profile, a
//! second process adopts them via the launcher, and the full control plane
//! (terminal spawn→write→terminate, ACP health + conversation root, clean
//! shutdown) is exercised against the cargo-built binary.

#![cfg(unix)]

use se_manager_lib::core::{
    ensure_core, AcpCoreClient, CoreEndpoint, CoreLaunchConfig, CoreRole, TerminalCoreClient,
};
use se_manager_lib::SpawnOptions;
use std::collections::HashMap;
use std::time::Duration;

/// Kills by PID on drop so an assertion failure cannot leak live Cores
/// holding the profile sockets (ensure_core parks the Child globally and
/// returns `child: None`, so there is no handle to guard).
struct PidReaper(Vec<u32>);

impl Drop for PidReaper {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = kill_by_pid(*pid);
        }
    }
}

fn kill_by_pid(pid: u32) -> std::io::Result<()> {
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map(|_| ())
}

async fn wait_for_terminal(endpoint: &CoreEndpoint) -> TerminalCoreClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match TerminalCoreClient::connect(endpoint).await {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("terminal core not ready: {error}"),
        }
    }
}

async fn wait_for_acp(endpoint: &CoreEndpoint) -> AcpCoreClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match AcpCoreClient::connect(endpoint).await {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("acp core not ready: {error}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_binary_runs_both_cores_and_second_process_adopts() {
    let profile = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // Inside a test process, current_exe() is the test harness — point the
    // launcher at the real packaged binary cargo built for us.
    let config = CoreLaunchConfig {
        profile_root: profile.path().to_path_buf(),
        executable: std::path::PathBuf::from(env!("CARGO_BIN_EXE_se-manager")),
        ready_timeout: Duration::from_secs(10),
    };

    // First "process": the launcher spawns both role binaries.
    let terminal_spawned = ensure_core(CoreRole::TerminalCore, &config)
        .await
        .expect("spawn terminal core");
    assert!(!terminal_spawned.reused);
    let acp_spawned = ensure_core(CoreRole::AcpCore, &config)
        .await
        .expect("spawn acp core");
    assert!(!acp_spawned.reused);
    let _reaper = PidReaper(vec![terminal_spawned.pid, acp_spawned.pid]);

    // The ACP Core must own the conversation root on this shared profile.
    for _ in 0..100 {
        if profile.path().join("conversations").is_dir() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        profile.path().join("conversations").is_dir(),
        "acp core must create the conversation root"
    );

    let terminal_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
    let acp_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);

    // Terminal control plane through the spawned binary.
    let terminal = wait_for_terminal(&terminal_endpoint).await;
    let spawned = terminal
        .spawn(SpawnOptions {
            cwd: Some(profile.path().to_string_lossy().into_owned()),
            cols: Some(80),
            rows: Some(24),
            shell: Some("/bin/sh".into()),
            env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
            project_id: Some("smoke-project".into()),
            ..Default::default()
        })
        .await
        .expect("spawn PTY in packaged terminal core");
    terminal
        .write(&spawned.info.id, "printf 'DUAL_CORE_SMOKE\\n'\n")
        .await
        .expect("write");
    let listed = terminal.list().await.expect("list");
    assert!(listed.iter().any(|status| status.id == spawned.info.id));
    // A write RPC alone proves nothing — observe the PTY actually ran.
    let mut session = terminal
        .attach(&spawned.info.id, &spawned.claim, 0)
        .await
        .expect("attach for output");
    let mut observed = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !observed.contains("DUAL_CORE_SMOKE") && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), session.output.recv()).await {
            Ok(Some(frame)) => observed.push_str(&String::from_utf8_lossy(&frame.data)),
            _ => break,
        }
    }
    assert!(
        observed.contains("DUAL_CORE_SMOKE"),
        "PTY output marker missing: {observed:?}"
    );
    drop(session);
    terminal
        .terminate(&spawned.info.id)
        .await
        .expect("terminate");
    assert!(!terminal
        .list()
        .await
        .expect("list after terminate")
        .iter()
        .any(|status| status.id == spawned.info.id));

    // ACP control plane through the spawned binary.
    let acp = wait_for_acp(&acp_endpoint).await;
    let health = acp.health().await.expect("acp health");
    assert_eq!(health["status"], "ready");
    assert!(acp.list_agents().await.expect("listAgents").is_empty());

    // Second "process" adopts both endpoints without spawning duplicates.
    let terminal_adopted = ensure_core(CoreRole::TerminalCore, &config)
        .await
        .expect("adopt terminal core");
    assert!(terminal_adopted.reused, "second adopter must not respawn");
    let acp_adopted = ensure_core(CoreRole::AcpCore, &config)
        .await
        .expect("adopt acp core");
    assert!(acp_adopted.reused, "second adopter must not respawn");

    // Clean shutdown of both roles; endpoints stop answering.
    acp.shutdown().await.expect("acp shutdown");
    terminal.shutdown().await.expect("terminal shutdown");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(AcpCoreClient::connect(&acp_endpoint).await.is_err());
    assert!(TerminalCoreClient::connect(&terminal_endpoint)
        .await
        .is_err());

    workspace.close().unwrap();
    profile.close().unwrap();
}
