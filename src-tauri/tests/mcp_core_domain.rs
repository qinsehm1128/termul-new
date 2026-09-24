use std::{collections::BTreeMap, sync::Arc, time::Duration};

use serde_json::json;

use axum::Router;
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, ListPromptsResult,
        ListResourcesResult, ListToolsResult, Prompt, PromptMessage, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, Role,
        ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData, RoleServer, ServerHandler,
};
use se_manager_lib::mcp_core::{
    build_snapshot, AllowAllTools, BuiltInRegistry, DenyListedTools, InlineSecretResolver, McpCore,
    McpCoreConfig, McpDomainError, McpSnapshotController, McpUpstreamServer, McpUpstreamTransport,
    Operation, SnapshotError, BUILTIN_SESSION_MEMORY,
};
use se_manager_lib::memory_index::service::MemoryIndexService;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct FixtureServer;

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = serde_json::json!({"type": "object"})
            .as_object()
            .unwrap()
            .clone();
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new("echo", "Echo input", schema.clone()),
            Tool::new("slow", "Wait before replying", schema.clone()),
            Tool::new("large", "Return a large response", schema),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.name.as_ref() {
            "echo" => Ok(CallToolResult::success(vec![ContentBlock::text("echoed")]).into()),
            "slow" => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                Ok(CallToolResult::success(vec![ContentBlock::text("slow")]).into())
            }
            "large" => {
                Ok(CallToolResult::success(vec![ContentBlock::text("x".repeat(4096))]).into())
            }
            _ => Err(ErrorData::internal_error("unknown tool", None)),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![Resource::new(
            "fixture://hello",
            "hello",
        )
        .with_mime_type("text/plain")]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            format!("read:{}", request.uri),
            request.uri,
        )])
        .into())
    }

    async fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "greeting",
            Some("A greeting"),
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!("prompt:{}", request.name),
        )])
        .into())
    }
}

async fn spawn_fixture() -> (String, CancellationToken) {
    let server_ct = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(server_ct.child_token());
    let service: StreamableHttpService<FixtureServer, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(FixtureServer), Default::default(), config);
    let router = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn({
        let shutdown = server_ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        }
    });
    (format!("http://{address}/mcp"), server_ct)
}

fn http_server(id: &str, url: String) -> McpUpstreamServer {
    McpUpstreamServer {
        id: id.into(),
        enabled: true,
        transport: McpUpstreamTransport::StreamableHttp {
            url,
            headers: BTreeMap::new(),
        },
    }
}

#[tokio::test]
async fn routes_tools_resources_and_prompts_through_http_upstream() {
    let (url, shutdown) = spawn_fixture().await;
    let core = McpCore::default();
    core.connect_and_add(http_server("fixture", url))
        .await
        .unwrap();

    let tools = core.list_tools().await.unwrap();
    assert!(tools.failures.is_empty());
    assert!(tools
        .items
        .iter()
        .any(|item| item.tool.name == "fixture_echo"));

    let result = core.call_tool("fixture_echo", None).await.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "echoed");

    let resources = core.list_resources().await.unwrap();
    assert_eq!(resources.items[0].resource.uri, "fixture://hello");
    let read = core
        .read_resource("fixture", "fixture://hello")
        .await
        .unwrap();
    assert!(serde_json::to_string(&read)
        .unwrap()
        .contains("read:fixture://hello"));

    let prompts = core.list_prompts().await.unwrap();
    assert!(prompts
        .items
        .iter()
        .any(|item| item.prompt.name == "fixture_greeting"));
    let prompt = core.get_prompt("fixture_greeting", None).await.unwrap();
    assert!(serde_json::to_string(&prompt)
        .unwrap()
        .contains("prompt:greeting"));

    core.shutdown().await;
    shutdown.cancel();
}

#[tokio::test]
async fn permission_timeout_cancellation_and_response_limit_are_enforced() {
    let (url, shutdown) = spawn_fixture().await;
    let permission = Arc::new(DenyListedTools::new([("fixture".into(), "echo".into())]));
    let core = McpCore::new(
        McpCoreConfig {
            operation_timeout: Duration::from_millis(40),
            max_response_bytes: 256,
            ..Default::default()
        },
        permission,
    );
    core.connect_and_add(http_server("fixture", url))
        .await
        .unwrap();

    let tools = core.list_tools().await.unwrap();
    assert!(!tools
        .items
        .iter()
        .any(|item| item.tool.name == "fixture_echo"));
    assert!(matches!(
        core.call_tool("fixture_echo", None).await,
        Err(McpDomainError::PermissionDenied { .. })
    ));
    assert!(matches!(
        core.call_tool("fixture_slow", None).await,
        Err(McpDomainError::Timeout {
            operation: Operation::CallTool,
            ..
        })
    ));
    assert!(matches!(
        core.call_tool("fixture_large", None).await,
        Err(McpDomainError::ResponseTooLarge { .. })
    ));

    let cancellation = CancellationToken::new();
    let cancel_clone = cancellation.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_clone.cancel();
    });
    assert!(matches!(
        core.call_tool_with_cancel("fixture_slow", None, cancellation)
            .await,
        Err(McpDomainError::Cancelled {
            operation: Operation::CallTool,
            ..
        })
    ));
    task.await.unwrap();

    shutdown.cancel();
}

#[tokio::test]
async fn routes_local_stdio_upstream_through_the_same_core() {
    let core = McpCore::default();
    core.connect_and_add(McpUpstreamServer {
        id: "stdio".into(),
        enabled: true,
        transport: McpUpstreamTransport::Stdio {
            command: env!("CARGO_BIN_EXE_se-mcp-fixture").into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    })
    .await
    .unwrap();

    let tools = core.list_tools().await.unwrap();
    assert!(tools
        .items
        .iter()
        .any(|item| item.tool.name == "stdio_echo"));
    let result = core.call_tool("stdio_echo", None).await.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "stdio");
    core.shutdown().await;
}

#[tokio::test]
async fn rejected_snapshot_preserves_last_good_runtime_and_revision() {
    let core = Arc::new(McpCore::default());
    let controller = McpSnapshotController::new(core.clone());
    let good = build_snapshot(
        &json!([{
            "id": "stdio",
            "type": "stdio",
            "command": env!("CARGO_BIN_EXE_se-mcp-fixture"),
            "enabled": true
        }]),
        1,
        &InlineSecretResolver,
    )
    .unwrap();
    controller.apply(good).await.unwrap();
    assert_eq!(controller.accepted_revision().await, 1);
    assert_eq!(core.upstream_ids().await, vec!["stdio"]);

    let bad = build_snapshot(
        &json!([{
            "id": "broken",
            "type": "stdio",
            "command": "/definitely/missing/mcp-fixture",
            "enabled": true
        }]),
        2,
        &InlineSecretResolver,
    )
    .unwrap();
    assert!(matches!(
        controller.apply(bad).await,
        Err(SnapshotError::ApplyFailed(McpDomainError::ConnectFailed(_)))
    ));
    assert_eq!(controller.accepted_revision().await, 1);
    assert_eq!(core.upstream_ids().await, vec!["stdio"]);
    assert!(matches!(
        controller
            .apply_registry(&json!([]), 1, &InlineSecretResolver)
            .await,
        Err(SnapshotError::StaleRevision { .. })
    ));
    core.shutdown().await;
}

#[tokio::test]
async fn one_failed_http_upstream_does_not_hide_healthy_upstream() {
    let (good_url, good_shutdown) = spawn_fixture().await;
    let (bad_url, bad_shutdown) = spawn_fixture().await;
    let core = McpCore::new(McpCoreConfig::default(), Arc::new(AllowAllTools));
    core.connect_and_add(http_server("good", good_url))
        .await
        .unwrap();
    core.connect_and_add(http_server("bad", bad_url))
        .await
        .unwrap();
    bad_shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let tools = core.list_tools().await.unwrap();
    assert!(tools.items.iter().any(|item| item.tool.name == "good_echo"));
    assert!(tools
        .failures
        .iter()
        .any(|failure| failure.server_id == "bad"));

    core.shutdown().await;
    good_shutdown.cancel();
}

#[tokio::test]
async fn builtins_aggregate_with_upstreams_and_win_id_collisions() {
    let (builtin_url, builtin_shutdown) = spawn_fixture().await;
    let (fixture_url, fixture_shutdown) = spawn_fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let memory = Arc::new(MemoryIndexService::new(state.canonicalize().unwrap()));
    let core = McpCore::new_with_builtins(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
        BuiltInRegistry::memory_backed(memory, None),
    );
    core.connect_and_add(http_server(BUILTIN_SESSION_MEMORY, builtin_url))
        .await
        .unwrap();
    core.connect_and_add(http_server("fixture", fixture_url))
        .await
        .unwrap();

    let tools = core.list_tools().await.unwrap();
    assert!(tools.items.iter().any(|item| {
        item.server_id == BUILTIN_SESSION_MEMORY && item.tool.name.ends_with("_memory_search")
    }));
    assert!(!tools
        .items
        .iter()
        .any(|item| item.tool.name == format!("{BUILTIN_SESSION_MEMORY}_echo")));
    assert!(tools
        .items
        .iter()
        .any(|item| item.tool.name == "fixture_echo"));

    let echoed = core.call_tool("fixture_echo", None).await.unwrap();
    assert_eq!(echoed.content[0].as_text().unwrap().text, "echoed");
    assert!(matches!(
        core.call_tool(&format!("{BUILTIN_SESSION_MEMORY}_echo"), None)
            .await,
        Err(McpDomainError::ToolNotFound(_)) | Err(McpDomainError::InvalidArguments { .. })
    ));

    core.shutdown().await;
    builtin_shutdown.cancel();
    fixture_shutdown.cancel();
}
