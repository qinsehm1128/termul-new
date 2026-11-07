//! End-to-end source topology using only private fixtures and loopback listeners.
use async_trait::async_trait;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use se_manager_lib::mcp_core::{
    builtins::BuiltInCapability, service::start_embedded, AllowAllTools, AuthBootstrap,
    BuiltInRegistry, McpCore, McpCoreConfig, McpDomainError, McpHttpGatewayConfig,
};
use serde_json::{json, Map, Value};
use std::future::IntoFuture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_util::sync::CancellationToken;

struct InternalFixture(Arc<AtomicUsize>);
#[async_trait]
impl BuiltInCapability for InternalFixture {
    fn id(&self) -> &str {
        "workspace"
    }
    fn tools(&self) -> Vec<Tool> {
        vec![serde_json::from_value(json!({"name":"fixture","description":"Fixture only","inputSchema":{"type":"object","properties":{}}})).unwrap()]
    }
    async fn call_tool(
        &self,
        _: &str,
        _: Option<Map<String, Value>>,
        _: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "internal fixture",
        )]))
    }
}

async fn upstream(State(calls): State<Arc<AtomicUsize>>, Json(request): Json<Value>) -> Response {
    let Some(id) = request.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match request["method"].as_str().unwrap_or_default() {
        "initialize" => {
            json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"fixture-upstream","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"echo","description":"Fixture only","inputSchema":{"type":"object","properties":{}}}]})
        }
        "tools/call" => {
            calls.fetch_add(1, Ordering::SeqCst);
            json!({"content":[{"type":"text","text":"external fixture"}],"isError":false})
        }
        "resources/list" => json!({"resources":[]}),
        "prompts/list" => json!({"prompts":[]}),
        _ => json!({}),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

fn config(token: &str) -> McpHttpGatewayConfig {
    McpHttpGatewayConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        port: 0,
        path: "/mcp".into(),
        generation: 1,
        auth: AuthBootstrap::new(1, token).unwrap(),
        request_body_limit: 1024 * 1024,
        control: None,
    }
}

async fn rpc(
    client: &reqwest::Client,
    port: u16,
    token: &str,
    method: &str,
    params: Value,
) -> Value {
    let response = client
        .post(format!("http://127.0.0.1:{port}/mcp/direct"))
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-03-26")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "MCP fixture response status {}",
        response.status()
    );
    let body = response.text().await.unwrap();
    if let Ok(value) = serde_json::from_str(&body) {
        return value;
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|data| serde_json::from_str(data.trim()).ok())
        .expect("JSON or SSE RPC response")
}

fn names(result: &Value) -> Vec<String> {
    let mut names = result["result"]["tools"]
        .as_array()
        .expect("tools list")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn write_registry(root: &std::path::Path, revision: u64, url: &str, name: &str) {
    let response = se_manager_lib::web::mcp_servers_api::put_config_at(root.into(), json!({
        "schemaVersion":1,"revision":revision,
        "builtIns":[{"id":"workspace","enabled":true}],
        "upstreams":[{"id":"fixture-upstream","name":name,"enabled":true,"type":"http","url":url,"headers":[]}],
        "routing":{}
    })).await;
    assert_eq!(
        serde_json::to_value(response.0).unwrap()["success"],
        true,
        "fixture registry persistence failed"
    );
}

#[tokio::test]
async fn embedded_sources_reload_the_web_registry_without_crossing_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let external_calls = Arc::new(AtomicUsize::new(0));
    let internal_calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let fixture = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(upstream))
                .with_state(external_calls.clone()),
        )
        .with_graceful_shutdown(stopped.cancelled_owned())
        .into_future(),
    );
    write_registry(root.path(), 1, &url, "external-alpha").await;
    let mut builtins = BuiltInRegistry::empty();
    builtins.register(Arc::new(InternalFixture(internal_calls.clone())));
    let internal = Arc::new(McpCore::new_internal(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
        builtins,
    ));
    let aggregator = Arc::new(McpCore::new_aggregator(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
    ));
    let first = start_embedded(
        internal,
        root.path().into(),
        config("internal-fixture-token"),
    )
    .await
    .unwrap();
    let second = start_embedded(
        aggregator,
        root.path().into(),
        config("aggregator-fixture-token"),
    )
    .await
    .unwrap();
    let ip = first.endpoint().port;
    let ap = second.endpoint().port;
    assert_ne!(ip, ap);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (port, source) in [(ip, "internal"), (ap, "aggregator")] {
        let identity: Value = client
            .get(format!("http://127.0.0.1:{port}/mcp-core/identity"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(identity["source"], source);
        assert_eq!(identity["protocol"], "se-mcp-gateway");
    }
    let before = names(
        &rpc(
            &client,
            ip,
            "internal-fixture-token",
            "tools/list",
            json!({}),
        )
        .await,
    );
    assert_eq!(before, ["workspace_fixture"]);
    assert_eq!(
        names(
            &rpc(
                &client,
                ap,
                "aggregator-fixture-token",
                "tools/list",
                json!({})
            )
            .await
        ),
        ["external-alpha_echo"]
    );
    for (port, wrong_token) in [
        (ip, "aggregator-fixture-token"),
        (ap, "internal-fixture-token"),
    ] {
        let response = client
            .post(format!("http://127.0.0.1:{port}/mcp/direct"))
            .bearer_auth(wrong_token)
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for (port, token, tool) in [
        (ip, "internal-fixture-token", "external-alpha_echo"),
        (ap, "aggregator-fixture-token", "workspace_fixture"),
    ] {
        let result = rpc(
            &client,
            port,
            token,
            "tools/call",
            json!({"name":tool,"arguments":{}}),
        )
        .await;
        assert!(result.get("error").is_some() || result["result"]["isError"] == true);
    }
    assert_eq!(external_calls.load(Ordering::SeqCst), 0);
    assert_eq!(internal_calls.load(Ordering::SeqCst), 0);
    write_registry(root.path(), 2, &url, "external-beta").await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if names(
                &rpc(
                    &client,
                    ap,
                    "aggregator-fixture-token",
                    "tools/list",
                    json!({}),
                )
                .await,
            ) == ["external-beta_echo"]
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("aggregator must observe Web registry writes");
    assert_eq!(
        names(
            &rpc(
                &client,
                ip,
                "internal-fixture-token",
                "tools/list",
                json!({})
            )
            .await
        ),
        before
    );
    let result = rpc(
        &client,
        ip,
        "internal-fixture-token",
        "tools/call",
        json!({"name":"workspace_fixture","arguments":{}}),
    )
    .await;
    assert_ne!(result["result"]["isError"], true);
    let result = rpc(
        &client,
        ap,
        "aggregator-fixture-token",
        "tools/call",
        json!({"name":"external-beta_echo","arguments":{}}),
    )
    .await;
    assert_ne!(result["result"]["isError"], true);
    assert_eq!(internal_calls.load(Ordering::SeqCst), 1);
    assert_eq!(external_calls.load(Ordering::SeqCst), 1);
    first.shutdown().await;
    second.shutdown().await;
    stop.cancel();
    fixture.await.unwrap().unwrap();
}
