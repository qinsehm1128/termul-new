//! Real-process ACP Core lifecycle: spawn the packaged binary in `--acp-core`
//! mode, drive the control plane over the local socket, and verify the
//! conversation bootstrap runs inside the Core process (single writer).

#![cfg(unix)]

use se_manager_lib::core::{AcpCoreClient, CoreEndpoint, CoreRole, TerminalCoreClient};
use se_manager_lib::SpawnOptions;
use serde_json::json;
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
async fn real_dual_core_delete_terminates_conversation_terminal_before_purge() {
    let profile = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let terminal_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::TerminalCore);
    let terminal_exe = env!("CARGO_BIN_EXE_se-manager");
    let terminal_child = Command::new(terminal_exe)
        .arg("--terminal-core")
        .env("TERMUL_CORE_PROFILE_ROOT", profile.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn --terminal-core");
    let mut terminal_process = CoreChild(terminal_child);
    let terminal = wait_for_terminal_client(&terminal_endpoint).await;

    let acp_endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::AcpCore);
    let acp_child = Command::new(terminal_exe)
        .arg("--acp-core")
        .env("TERMUL_CORE_PROFILE_ROOT", profile.path())
        .env("TERMUL_CORE_WORKSPACE_ROOT", workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn --acp-core");
    let mut acp_process = CoreChild(acp_child);
    let acp = wait_for_client(&acp_endpoint).await;

    let prepared = acp
        .request(
            "conversationPrepareTerminal",
            json!({
                "request": {
                    "schemaVersion": 1,
                    "executionTarget": { "kind": "workspace" },
                    "backend": "terminal"
                }
            }),
        )
        .await
        .expect("prepare terminal Conversation");
    let conversation_id = prepared["conversationId"]
        .as_str()
        .expect("prepared Conversation id")
        .to_string();
    let conversation_uuid = se_manager_lib::conversation::ConversationId::parse(&conversation_id)
        .expect("prepared Conversation UUID");
    let spawned = terminal
        .spawn(SpawnOptions {
            cwd: Some(workspace.path().to_string_lossy().into_owned()),
            conversation_id: Some(conversation_uuid),
            cols: Some(80),
            rows: Some(24),
            shell: Some("/bin/sh".into()),
            env: Some(HashMap::from([("PS1".into(), "$ ".into())])),
            ..Default::default()
        })
        .await
        .expect("spawn conversation terminal");
    acp.request(
        "conversationProvisionTerminal",
        json!({
            "conversationId": conversation_id,
            "terminalId": spawned.info.id
        }),
    )
    .await
    .expect("provision terminal reference");
    acp.request(
        "workspaceEnsureTerminalRefWritable",
        json!({ "conversationId": conversation_id, "writable": true }),
    )
    .await
    .expect("enable terminal workspace reference");
    acp.request(
        "workspaceAddTerminalRef",
        json!({
            "conversationId": conversation_id,
            "terminalId": spawned.info.id
        }),
    )
    .await
    .expect("persist terminal workspace reference");
    let observed = terminal
        .observe_conversation(conversation_uuid, std::slice::from_ref(&spawned.info.id))
        .await
        .expect("observe provisioned terminal");
    assert_eq!(observed.live_terminal_ids, vec![spawned.info.id.clone()]);
    let workspace_record = acp
        .request(
            "conversationGetWorkspace",
            json!({ "conversationId": conversation_id }),
        )
        .await
        .expect("get terminal Conversation workspace");
    assert!(
        workspace_record["workspace"]["resources"]
            .as_array()
            .is_some_and(|resources| resources
                .iter()
                .any(|resource| { resource["terminalId"] == spawned.info.id })),
        "provision must persist the terminal workspace reference: {workspace_record}"
    );
    let record = acp
        .request(
            "conversationGet",
            json!({ "conversationId": conversation_id }),
        )
        .await
        .expect("get terminal Conversation");
    let revision = record["lastSeq"].as_u64().expect("Conversation revision");

    let deleted = acp
        .request(
            "conversationDelete",
            json!({
                "conversationId": conversation_id,
                "expectedRevision": revision
            }),
        )
        .await
        .expect("delete terminal Conversation");
    assert_eq!(deleted["status"], "updated");
    assert_eq!(deleted["lifecycleState"], "deleted");
    let terminal_list = terminal.list().await.expect("list terminals after cleanup");
    assert!(
        terminal_list
            .iter()
            .all(|status| status.id != spawned.info.id),
        "delete must terminate and remove the PTY before purge"
    );

    acp.shutdown().await.expect("shutdown acp core");
    let _ = acp_process.0.wait();
    terminal.shutdown().await.expect("shutdown terminal core");
    let _ = terminal_process.0.wait();
}

async fn wait_for_terminal_client(endpoint: &CoreEndpoint) -> TerminalCoreClient {
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
