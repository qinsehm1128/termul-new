//! The standalone MCP gateway, end to end: the real `se-manager --mcp-core`
//! process serving the global configuration, and the real `se-mcp` stdio
//! client an agent would launch.
//!
//! Run with `--features mcp-test-fixture` (the stdio upstream fixture).

#![cfg(feature = "mcp-test-fixture")]

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

const TOKEN: &str = "se-mcp-service-test";

/// Tests that start a gateway through a path which inherits this process's
/// `SE_PROJECT_ROOT` take turns.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Gateway {
    root: tempfile::TempDir,
    settings: PathBuf,
    port: u16,
    child: Child,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_servers(root: &Path, servers: Value) {
    let dir = root.join(".se-manager");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mcp-servers.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "revision": 1,
            "builtIns": [
                {"id": "session-memory", "enabled": false},
                {"id": "project-scope", "enabled": false}
            ],
            "upstreams": servers
        }))
        .unwrap(),
    )
    .unwrap();
}

fn fixture(name: &str) -> Value {
    json!({
        "id": format!("id-{name}"),
        "name": name,
        "type": "stdio",
        "command": env!("CARGO_BIN_EXE_se-mcp-fixture"),
        "enabled": true
    })
}

fn start_gateway(servers: Value) -> Gateway {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    write_servers(&root_path, servers);
    let port = free_port();
    let settings = root_path.join(".se-manager").join("mcp-gateway.json");
    se_mcp_bridge::GatewaySettings {
        schema_version: 1,
        port,
        token: TOKEN.into(),
        executable: Some(env!("CARGO_BIN_EXE_se-manager").into()),
        profile_root: None,
        log_file: None,
    }
    .save(&settings)
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_se-manager"))
        .args(["--mcp-core", "--config"])
        .arg(&settings)
        .env("SE_PROJECT_ROOT", &root_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Gateway {
        root,
        settings,
        port,
        child,
    }
}

async fn status(gateway: &Gateway) -> Option<Value> {
    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/control/status", gateway.port))
        .bearer_auth(TOKEN)
        .send()
        .await
        .ok()?;
    response.json().await.ok()
}

/// Wait until `check` accepts the gateway status.
async fn wait_for(gateway: &Gateway, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(current) = status(gateway).await {
            if check(&current) {
                return current;
            }
        }
        assert!(
            Instant::now() < deadline,
            "gateway never reached the expected state"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn upstream_states(status: &Value) -> Vec<(String, String)> {
    status["upstreams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|upstream| {
            (
                upstream["name"].as_str().unwrap().to_owned(),
                upstream["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// One JSON-RPC exchange with an `se-mcp` child over its stdio.
struct Client {
    child: Child,
    output: BufReader<std::process::ChildStdout>,
}

impl Client {
    fn spawn(settings: &Path, mode: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_se-mcp"))
            .args(["--mode", mode, "--config"])
            .arg(settings)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self { child, output }
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn notify(&mut self, method: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "method": method})).unwrap();
        stdin.flush().unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize(client: &mut Client) {
    let reply = client.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}
        }),
    );
    assert!(reply["result"]["serverInfo"].is_object(), "{reply}");
    client.notify("notifications/initialized");
}

#[tokio::test]
async fn gateway_serves_the_global_config_and_follows_its_changes() {
    let gateway = start_gateway(json!([
        fixture("alpha"),
        {"id": "id-broken", "name": "broken", "type": "stdio", "command": "/missing/mcp", "enabled": true}
    ]));
    let ready = wait_for(&gateway, |status| {
        upstream_states(status)
            .iter()
            .all(|(_, state)| state != "connecting")
            && !upstream_states(status).is_empty()
    })
    .await;
    assert_eq!(
        upstream_states(&ready),
        [
            ("alpha".to_owned(), "connected".to_owned()),
            ("broken".to_owned(), "failed".to_owned())
        ]
    );
    assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));

    // Editing the file is enough: the gateway notices and reconnects.
    write_servers(
        &gateway.root.path().canonicalize().unwrap(),
        json!([fixture("alpha"), fixture("beta")]),
    );
    wait_for(&gateway, |status| {
        upstream_states(status)
            == [
                ("alpha".to_owned(), "connected".to_owned()),
                ("beta".to_owned(), "connected".to_owned()),
            ]
    })
    .await;
}

#[tokio::test]
async fn se_mcp_reaches_every_mode_through_the_settings_file() {
    let gateway = start_gateway(json!([fixture("alpha")]));
    wait_for(&gateway, |status| {
        upstream_states(status) == [("alpha".to_owned(), "connected".to_owned())]
    })
    .await;

    let mut grouped = Client::spawn(&gateway.settings, "grouped");
    initialize(&mut grouped);
    let tools = grouped.request(2, "tools/list", json!({}));
    let names = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["alpha_tool_list", "alpha_tool_call"]);
    let called = grouped.request(
        3,
        "tools/call",
        json!({"name": "alpha_tool_call", "arguments": {"toolName": "echo"}}),
    );
    assert_eq!(called["result"]["content"][0]["text"], "stdio", "{called}");

    let mut entry = Client::spawn(&gateway.settings, "entry");
    initialize(&mut entry);
    let servers = entry.request(
        2,
        "tools/call",
        json!({"name": "list_mcp_servers", "arguments": {}}),
    );
    let text = servers["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"alpha\""), "{text}");

    let mut direct = Client::spawn(&gateway.settings, "direct");
    initialize(&mut direct);
    let called = direct.request(
        2,
        "tools/call",
        json!({"name": "alpha_echo", "arguments": {}}),
    );
    assert_eq!(called["result"]["content"][0]["text"], "stdio", "{called}");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn se_mcp_starts_the_gateway_when_nothing_listens() {
    let gateway = start_gateway(json!([fixture("alpha")]));
    wait_for(&gateway, |status| !upstream_states(status).is_empty()).await;
    // Stop it through the control API, the way Se Manager replaces it.
    let stopped = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/control/shutdown",
            gateway.port
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), 202);
    let deadline = Instant::now() + Duration::from_secs(10);
    while status(&gateway).await.is_some() {
        assert!(Instant::now() < deadline, "gateway did not stop");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The client finds nothing on the port and starts the recorded binary.
    // SE_PROJECT_ROOT is inherited by the gateway it starts.
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var(
        "SE_PROJECT_ROOT",
        gateway.root.path().canonicalize().unwrap(),
    );
    let mut client = Client::spawn(&gateway.settings, "grouped");
    initialize(&mut client);
    assert!(status(&gateway).await.is_some());
    let _ = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/control/shutdown",
            gateway.port
        ))
        .bearer_auth(TOKEN)
        .send()
        .await;
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_desktop_handle_starts_the_gateway_and_moves_its_port() {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    write_servers(&root_path, json!([fixture("alpha")]));
    std::env::set_var("SE_PROJECT_ROOT", &root_path);
    let settings_path = root_path.join(".se-manager").join("mcp-gateway.json");
    let service = se_manager_lib::mcp_core::McpService::new(
        settings_path.clone(),
        env!("CARGO_BIN_EXE_se-manager").into(),
        root_path.join("profile"),
        None,
    );
    // The first start creates the settings (port, token) it then serves.
    let first = free_port();
    se_mcp_bridge::GatewaySettings {
        schema_version: 1,
        port: first,
        token: TOKEN.into(),
        executable: None,
        profile_root: None,
        log_file: None,
    }
    .save(&settings_path)
    .unwrap();
    let started = service.ensure_running().await.unwrap();
    assert_eq!(started.port, first);
    // Running already: a second call adopts it instead of starting another.
    assert_eq!(service.ensure_running().await.unwrap().pid, started.pid);

    let second = free_port();
    let moved = service.set_port(second).await.unwrap();
    assert_eq!(moved.port, second);
    assert_ne!(moved.pid, started.pid);
    assert!(std::net::TcpStream::connect(("127.0.0.1", first)).is_err());
    assert_eq!(
        se_mcp_bridge::GatewaySettings::load(&settings_path)
            .unwrap()
            .port,
        second
    );
    let view = service.view().await;
    assert_eq!(view.state, "running");

    let _ = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{second}/control/shutdown"))
        .bearer_auth(TOKEN)
        .send()
        .await;
}

#[tokio::test]
async fn the_gateway_exits_when_its_settings_file_is_removed() {
    let mut gateway = start_gateway(json!([]));
    wait_for(&gateway, |_| true).await;
    std::fs::remove_file(&gateway.settings).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = gateway.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "gateway kept running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
