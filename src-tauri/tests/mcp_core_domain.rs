use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

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
    Operation, SnapshotError, UpstreamState, BUILTIN_SESSION_MEMORY,
};
use se_manager_lib::memory_index::service::MemoryIndexService;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct FixtureServer(Arc<AtomicUsize>);

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
        self.0.fetch_add(1, Ordering::SeqCst);
        let schema = serde_json::json!({"type": "object"})
            .as_object()
            .unwrap()
            .clone();
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new("echo", "Echo input", schema.clone()),
            Tool::new("slow", "Wait before replying", schema.clone()),
            Tool::new("large", "Return a large response", schema.clone()),
            Tool::new("group_list", "External workspace fixture", schema),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.0.fetch_add(1, Ordering::SeqCst);
        match request.name.as_ref() {
            "group_list" => {
                Ok(CallToolResult::success(vec![ContentBlock::text("externalfixture")]).into())
            }
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
    let (url, shutdown, _) = spawn_counted_fixture().await;
    (url, shutdown)
}

async fn spawn_counted_fixture() -> (String, CancellationToken, Arc<AtomicUsize>) {
    let requests = Arc::new(AtomicUsize::new(0));
    let server_ct = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(server_ct.child_token());
    let service: StreamableHttpService<FixtureServer, LocalSessionManager> =
        StreamableHttpService::new(
            {
                let requests = requests.clone();
                move || Ok(FixtureServer(requests.clone()))
            },
            Default::default(),
            config,
        );
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
    (format!("http://{address}/mcp"), server_ct, requests)
}

fn http_server(id: &str, url: String) -> McpUpstreamServer {
    McpUpstreamServer {
        id: id.into(),
        name: String::new(),
        policy: Default::default(),
        enabled: true,
        transport: McpUpstreamTransport::StreamableHttp {
            url,
            headers: BTreeMap::new(),
            oauth: None,
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
            call_timeout: Duration::from_millis(40),
            max_response_bytes: 512,
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
        name: String::new(),
        policy: Default::default(),
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
async fn a_failing_upstream_is_reported_without_taking_healthy_ones_down() {
    let core = Arc::new(McpCore::default());
    let controller = McpSnapshotController::new(core.clone());
    let snapshot = build_snapshot(
        &json!([
            {
                "id": "stdio",
                "type": "stdio",
                "command": env!("CARGO_BIN_EXE_se-mcp-fixture"),
                "enabled": true
            },
            {
                "id": "broken",
                "type": "stdio",
                "command": "/definitely/missing/mcp-fixture",
                "enabled": true
            },
            {
                "id": "off",
                "type": "stdio",
                "command": "/definitely/missing/mcp-fixture",
                "enabled": false
            }
        ]),
        1,
        &InlineSecretResolver,
    )
    .unwrap();
    controller.apply(snapshot).await.unwrap();

    assert_eq!(core.upstream_ids().await, vec!["stdio"]);
    let states = core
        .upstream_statuses()
        .await
        .into_iter()
        .map(|status| (status.id, status.state, status.error.is_some()))
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            ("stdio".into(), UpstreamState::Connected, false),
            ("broken".into(), UpstreamState::Failed, true),
            ("off".into(), UpstreamState::Disabled, false),
        ]
    );
    assert!(matches!(
        controller
            .apply_registry(&json!([]), 1, &InlineSecretResolver)
            .await,
        Err(SnapshotError::StaleRevision { .. })
    ));
    core.shutdown().await;
}

#[tokio::test]
async fn reapplying_an_unchanged_upstream_keeps_its_live_connection() {
    let (url, shutdown) = spawn_fixture().await;
    let core = Arc::new(McpCore::default());
    let controller = McpSnapshotController::new(core.clone());
    let registry = |revision: u64, extra: bool| {
        let mut servers = vec![json!({"id": "a", "name": "Fixture", "type": "http", "url": url})];
        if extra {
            servers.push(json!({
                "id": "b",
                "type": "stdio",
                "command": env!("CARGO_BIN_EXE_se-mcp-fixture")
            }));
        }
        build_snapshot(&json!(servers), revision, &InlineSecretResolver).unwrap()
    };
    controller.apply(registry(1, false)).await.unwrap();
    assert_eq!(core.upstream_ids().await, vec!["fixture"]);
    assert!(core.call_tool("fixture_echo", None).await.is_ok());

    // Stop the fixture: reconnecting to it now fails, so the upstream stays
    // connected only if the reapply kept the connection it already had.
    shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    controller.apply(registry(2, true)).await.unwrap();
    assert_eq!(core.upstream_ids().await, vec!["b", "fixture"]);
    let fixture = core
        .upstream_statuses()
        .await
        .into_iter()
        .find(|status| status.id == "a")
        .unwrap();
    assert_eq!(fixture.state, UpstreamState::Connected);
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
async fn internal_rejects_upstreams_and_exposes_only_builtins() {
    let (builtin_url, builtin_shutdown, builtin_calls) = spawn_counted_fixture().await;
    let (fixture_url, fixture_shutdown, fixture_calls) = spawn_counted_fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let memory = Arc::new(MemoryIndexService::new(state.canonicalize().unwrap()));
    let core = McpCore::new_with_builtins(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
        BuiltInRegistry::memory_backed(memory, None),
    );
    assert_eq!(core.source(), se_mcp_bridge::Source::Internal);
    for (id, url) in [
        (BUILTIN_SESSION_MEMORY, builtin_url),
        ("fixture", fixture_url),
    ] {
        assert!(matches!(
            core.connect_and_add(http_server(id, url)).await,
            Err(McpDomainError::InvalidConfiguration(_))
        ));
    }
    assert!(core.upstream_ids().await.is_empty());
    assert!(core.upstream_statuses().await.is_empty());
    assert!(core.list_resources().await.unwrap().items.is_empty());
    assert!(core.list_prompts().await.unwrap().items.is_empty());
    assert!(core
        .read_resource("fixture", "fixture://hello")
        .await
        .is_err());
    assert!(core.get_prompt("fixture_greeting", None).await.is_err());
    let tools = core.list_tools().await.unwrap();
    assert!(tools.items.iter().any(|item| {
        item.server_id == BUILTIN_SESSION_MEMORY && item.tool.name.ends_with("_memory_search")
    }));
    assert!(tools
        .items
        .iter()
        .all(|item| item.server_id == BUILTIN_SESSION_MEMORY));
    assert!(core.call_tool("fixture_echo", None).await.is_err());
    assert!(core
        .call_server_tool("fixture", "echo", None, CancellationToken::new())
        .await
        .is_err());

    assert_eq!(builtin_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture_calls.load(Ordering::SeqCst), 0);
    core.shutdown().await;
    builtin_shutdown.cancel();
    fixture_shutdown.cancel();
}

#[tokio::test]
async fn aggregator_allows_external_workspace_but_never_installs_se_providers() {
    let (url, shutdown, requests) = spawn_counted_fixture().await;
    let core = McpCore::new_aggregator(McpCoreConfig::default(), Arc::new(AllowAllTools));
    assert_eq!(core.source(), se_mcp_bridge::Source::Aggregator);
    assert_eq!(
        McpCore::default().source(),
        se_mcp_bridge::Source::Aggregator
    );
    assert!(core
        .replace_builtins(BuiltInRegistry::empty())
        .await
        .is_err());
    assert!(core.apply_built_in_config(&[]).await.is_err());
    core.connect_and_add(http_server("workspace", url))
        .await
        .unwrap();
    let before = requests.load(Ordering::SeqCst);
    let result = core.call_tool("workspace_group_list", None).await.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "externalfixture");
    let result = core
        .call_server_tool("workspace", "group_list", None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "externalfixture");
    assert_eq!(requests.load(Ordering::SeqCst), before + 2);
    // A reserved/hidden Se name cannot route to a host provider in this core.
    assert!(core
        .call_tool("project-scope_memory_search", None)
        .await
        .is_err());
    assert!(core
        .call_server_tool(
            "project-scope",
            "memory_search",
            None,
            CancellationToken::new()
        )
        .await
        .is_err());
    assert_eq!(requests.load(Ordering::SeqCst), before + 2);
    assert!(core.built_in_ids().await.is_empty());
    assert!(core
        .list_tools()
        .await
        .unwrap()
        .items
        .iter()
        .all(|tool| tool.server_id == "workspace"));
    core.shutdown().await;
    shutdown.cancel();
}
