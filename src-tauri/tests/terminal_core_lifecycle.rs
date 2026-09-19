//! Real-process Terminal Core lifecycle: spawn, disconnect, reconnect, replay.

#![cfg(unix)]

use se_manager_lib::core::{CoreEndpoint, CoreRole, TerminalCoreClient};
use se_manager_lib::SpawnOptions;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::Duration;

struct CoreChild(std::process::Child);

impl Drop for CoreChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for_client(endpoint: &CoreEndpoint) -> TerminalCoreClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match TerminalCoreClient::connect(endpoint).await {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("terminal core process did not become ready: {error}"),
        }
    }
}

async fn collect_until_marker(
    output: &mut tokio::sync::mpsc::Receiver<se_manager_lib::core::OutputFrame>,
    marker: &str,
    timeout: Duration,
) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut collected = String::new();
    loop {
        if collected.contains(marker) {
            return collected;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for marker {marker:?} in {collected:?}");
        }
        match tokio::time::timeout(remaining, output.recv()).await {
            Ok(Some(frame)) => collected.push_str(&String::from_utf8_lossy(&frame.data)),
            Ok(None) => panic!("output closed before marker {marker:?}: {collected:?}"),
            Err(_) => panic!("timed out waiting for marker {marker:?} in {collected:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_core_process_survives_client_disconnect() {
    let profile = tempfile::tempdir().unwrap();
    let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
    let exe = env!("CARGO_BIN_EXE_se-manager");
    let child = Command::new(exe)
        .arg("--terminal-core")
        .env("TERMUL_CORE_PROFILE_ROOT", profile.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn --terminal-core");
    let mut child = CoreChild(child);

    let client = wait_for_client(&endpoint).await;
    let spawned = client
        .spawn(SpawnOptions {
            cwd: Some(profile.path().to_string_lossy().into_owned()),
            cols: Some(80),
            rows: Some(24),
            shell: Some("/bin/sh".into()),
            env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
            ..Default::default()
        })
        .await
        .expect("spawn PTY in core process");
    let terminal_id = spawned.info.id.clone();
    let claim = spawned.claim.clone();

    client
        .write(&terminal_id, "printf 'TERMUL_CORE_PROCESS_MARKER\\n'\n")
        .await
        .expect("write marker");
    let mut session = client
        .attach(&terminal_id, &claim, 0)
        .await
        .expect("attach");
    collect_until_marker(
        &mut session.output,
        "TERMUL_CORE_PROCESS_MARKER",
        Duration::from_secs(10),
    )
    .await;
    drop(session);
    drop(client);

    let client = wait_for_client(&endpoint).await;
    let mut session = client
        .attach(&terminal_id, &claim, 0)
        .await
        .expect("reattach after GUI client disconnect");
    let replayed = collect_until_marker(
        &mut session.output,
        "TERMUL_CORE_PROCESS_MARKER",
        Duration::from_secs(10),
    )
    .await;
    assert!(
        replayed.contains("TERMUL_CORE_PROCESS_MARKER"),
        "core process replay missing marker: {replayed:?}"
    );

    client.shutdown().await.expect("shutdown core process");
    let _ = child.0.wait();
}
