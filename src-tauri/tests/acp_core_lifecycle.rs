//! Real-process ACP Core lifecycle: spawn the packaged binary in `--acp-core`
//! mode, drive the control plane over the local socket, and verify the
//! conversation bootstrap runs inside the Core process (single writer).

#![cfg(unix)]

use se_manager_lib::core::{AcpCoreClient, CoreEndpoint, CoreRole};
use std::process::{Command, Stdio};
use std::time::Duration;

struct CoreChild(std::process::Child);

impl Drop for CoreChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for_client(endpoint: &CoreEndpoint) -> AcpCoreClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match AcpCoreClient::connect(endpoint).await {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("acp core process did not become ready: {error}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_acp_core_process_serves_control_plane_and_survives_reconnect() {
    let profile = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);
    let exe = env!("CARGO_BIN_EXE_se-manager");
    let child = Command::new(exe)
        .arg("--acp-core")
        .env("TERMUL_CORE_PROFILE_ROOT", profile.path())
        .env("TERMUL_CORE_WORKSPACE_ROOT", workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn --acp-core");
    let mut child = CoreChild(child);

    let client = wait_for_client(&endpoint).await;
    let health = client.health().await.expect("health");
    assert_eq!(health["role"], "acp-core");
    assert_eq!(health["status"], "ready");
    assert!(client.list_agents().await.expect("listAgents").is_empty());

    let history = client
        .request("historyList", serde_json::Value::Null)
        .await
        .expect("historyList over core IPC");
    assert_eq!(history.as_array().map(Vec::len), Some(0));

    // The conversation repository lives inside the Core process: the
    // conversation tree exists under the state root after bootstrap.
    let state_root = profile.path();
    let conversations_root = state_root.join("conversations");
    assert!(
        conversations_root.is_dir(),
        "acp core must own the conversation root at {}",
        conversations_root.display()
    );

    // A second client adopts the same endpoint (GUI restart semantics).
    let second = wait_for_client(&endpoint).await;
    assert!(second
        .list_agents()
        .await
        .expect("adopted listAgents")
        .is_empty());
    drop(second);
    drop(client);

    let client = wait_for_client(&endpoint).await;
    let error = client
        .request("definitelyNotAMethod", serde_json::Value::Null)
        .await
        .expect_err("unknown method must be rejected");
    assert_eq!(error.code(), "CORE_IPC_INVALID_REQUEST");
    assert!(
        matches!(error, se_manager_lib::core::CoreError::InvalidRequest(detail)
        if detail.contains("definitelyNotAMethod"))
    );

    client.shutdown().await.expect("shutdown acp core");
    let _ = child.0.wait();
    workspace.close().unwrap();
    profile.close().unwrap();
}
