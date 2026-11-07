use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::Router;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, ListPromptsResult,
        ListResourcesResult, ListToolsResult, Prompt, PromptMessage, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, Role,
        ServerCapabilities, ServerConfig, Tool,
    },
    service::{serve_client, RequestContext, RoleServer},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
        },
        StreamableHttpClientTransport,
    },
    ErrorData, ServerHandler,
};
use se_manager_lib::mcp_core::{
    AllowAllTools, AuthBootstrap, BuiltInRegistry, McpCore, McpCoreConfig, McpHttpGateway,
    McpHttpGatewayConfig, McpHttpGatewayError, McpUpstreamServer, McpUpstreamTransport,
    BUILTIN_SESSION_MEMORY,
};
use se_manager_lib::memory_index::service::MemoryIndexService;
use se_mcp_bridge::{Source, PROTOCOL_VERSION_HEADER, SOURCE_HEADER};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Fixture {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let schema = serde_json::json!({"type": "object"})
            .as_object()
            .unwrap()
            .clone();
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "echo", "Echo", schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.name.as_ref() == "echo" {
            Ok(CallToolResult::success(vec![ContentBlock::text("fixture")]).into())
        } else {
            Err(ErrorData::internal_error("unknown tool", None))
        }
    }

    async fn list_resources(
        &self,
        _: Option<rmcp::model::PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ListResourcesResult::with_all_items(vec![Resource::new(
            "fixture://one",
            "one",
        )]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            "fixture-resource",
            request.uri,
        )])
        .into())
    }

    async fn list_prompts(
        &self,
        _: Option<rmcp::model::PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "review",
            None::<String>,
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, request.name)]).into())
    }
}

async fn spawn_fixture() -> (String, CancellationToken) {
    let (url, shutdown, _) = spawn_counted_fixture().await;
    (url, shutdown)
}

async fn spawn_counted_fixture() -> (String, CancellationToken, Arc<AtomicUsize>) {
    let shutdown = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_cancellation_token(shutdown.child_token());
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_service = calls.clone();
    let service: StreamableHttpService<Fixture, LocalSessionManager> = StreamableHttpService::new(
        move || {
            Ok(Fixture {
                calls: calls_for_service.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown_for_task = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_for_task.cancelled_owned())
            .await;
    });
    (format!("http://{address}/mcp"), shutdown, calls)
}

fn gateway_config(token: &str) -> McpHttpGatewayConfig {
    McpHttpGatewayConfig {
        bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        path: "/mcp".into(),
        generation: 1,
        auth: AuthBootstrap::new(1, token).unwrap(),
        request_body_limit: 64 * 1024,
        control: None,
    }
}

fn client_transport(
    url: &str,
    token: &str,
) -> impl rmcp::transport::Transport<rmcp::service::RoleClient> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    config = config.custom_headers(headers);
    StreamableHttpClientTransport::from_config(config)
}

fn assert_markers(response: &reqwest::Response, source: Source) {
    assert_eq!(
        response.headers().get(SOURCE_HEADER).unwrap(),
        source.as_str()
    );
    assert_eq!(
        response.headers().get(PROTOCOL_VERSION_HEADER).unwrap(),
        "1"
    );
}

#[tokio::test]
async fn identity_is_public_and_marker_mismatch_never_dispatches() {
    let (upstream_url, upstream_shutdown, calls) = spawn_counted_fixture().await;
    for source in [Source::Internal, Source::Aggregator] {
        let core = Arc::new(match source {
            Source::Internal => McpCore::new_internal(
                McpCoreConfig::default(),
                Arc::new(AllowAllTools),
                BuiltInRegistry::empty(),
            ),
            Source::Aggregator => {
                McpCore::new_aggregator(McpCoreConfig::default(), Arc::new(AllowAllTools))
            }
        });
        if source == Source::Aggregator {
            core.connect_and_add(McpUpstreamServer {
                id: "fixture".into(),
                name: String::new(),
                policy: Default::default(),
                enabled: true,
                transport: McpUpstreamTransport::StreamableHttp {
                    url: upstream_url.clone(),
                    headers: Default::default(),
                    oauth: None,
                },
            })
            .await
            .unwrap();
        }
        let mut config = gateway_config("marker-token");
        config.request_body_limit = 1024;
        let gateway = McpHttpGateway::bind(core, config).await.unwrap();
        let base = format!("http://127.0.0.1:{}", gateway.endpoint().port);
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let identity = http
            .get(format!("{base}/mcp-core/identity"))
            .send()
            .await
            .unwrap();
        assert_eq!(identity.status(), reqwest::StatusCode::OK);
        assert_markers(&identity, source);
        assert_eq!(
            identity.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({
                "protocol":"se-mcp-gateway", "protocolVersion":1, "source":source.as_str()
            })
        );
        let initialize = http
            .post(format!("{base}/mcp"))
            .bearer_auth("marker-token")
            .header(SOURCE_HEADER, source.as_str())
            .header(PROTOCOL_VERSION_HEADER, "1")
            .header("accept", "application/json, text/event-stream")
            .json(&serde_json::json!({
                "jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                    "protocolVersion":"2025-03-26", "capabilities":{},
                    "clientInfo":{"name":"source-test", "version":"1"}
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(initialize.status(), reqwest::StatusCode::OK);
        assert_markers(&initialize, source);
        assert!(initialize
            .json::<serde_json::Value>()
            .await
            .unwrap()
            .get("result")
            .is_some());
        let mut before = calls.load(Ordering::SeqCst);
        for path in ["/mcp", "/mcp/entry", "/mcp/aggregator", "/mcp/direct"] {
            let url = format!("{base}{path}");
            let other = if source == Source::Internal {
                "aggregator"
            } else {
                "internal"
            };
            let params = match path {
                "/mcp" => {
                    serde_json::json!({"name":"fixture_tool_call", "arguments":{"toolName":"echo", "arguments":{}}})
                }
                "/mcp/entry" => {
                    serde_json::json!({"name":"call_mcp_tool", "arguments":{"mcpName":"fixture", "toolName":"echo", "arguments":{}}})
                }
                _ => serde_json::json!({"name":"fixture_echo", "arguments":{}}),
            };
            for (header, value) in [
                (SOURCE_HEADER, other),
                (SOURCE_HEADER, "unknown"),
                (PROTOCOL_VERSION_HEADER, "2"),
                (PROTOCOL_VERSION_HEADER, ""),
            ] {
                let response = http.post(&url).bearer_auth("marker-token")
                    .header("accept", "application/json, text/event-stream")
                    .header(header, value)
                    .json(&serde_json::json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":params}))
                    .send().await.unwrap();
                assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
                assert_markers(&response, source);
                assert_eq!(calls.load(Ordering::SeqCst), before);
            }
            // Duplicate values cannot smuggle a mismatched second marker.
            let duplicate = http
                .post(&url)
                .bearer_auth("marker-token")
                .header(SOURCE_HEADER, source.as_str())
                .header(SOURCE_HEADER, other)
                .send()
                .await
                .unwrap();
            assert_eq!(duplicate.status(), reqwest::StatusCode::CONFLICT);
            assert_markers(&duplicate, source);
            let unauthorized = http.post(&url).body("{}").send().await.unwrap();
            assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
            assert_markers(&unauthorized, source);
            let malformed = http
                .post(&url)
                .bearer_auth("marker-token")
                .header("accept", "application/json, text/event-stream")
                .header("content-type", "application/json")
                .body("not-json")
                .send()
                .await
                .unwrap();
            assert!(malformed.status().is_client_error());
            assert_markers(&malformed, source);
            // Both declared-size and unknown-size chunked JSON must be rejected
            // before the otherwise-valid call can reach any backend.
            let oversized_json = serde_json::to_vec(&serde_json::json!({
                "jsonrpc":"2.0", "id":90, "method":"tools/call", "params":params,
                "padding":"x".repeat(2048)
            }))
            .unwrap();
            for streamed in [false, true] {
                let mut request = http
                    .post(&url)
                    .bearer_auth("marker-token")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json");
                if streamed {
                    let chunks = oversized_json
                        .chunks(257)
                        .map(|chunk| Ok::<_, std::io::Error>(chunk.to_vec()))
                        .collect::<Vec<_>>();
                    request = request.header("transfer-encoding", "chunked").body(
                        reqwest::Body::wrap_stream(futures_util::stream::iter(chunks)),
                    );
                } else {
                    request = request
                        .header("content-length", oversized_json.len())
                        .body(oversized_json.clone());
                }
                let request = request.build().unwrap();
                assert_eq!(request.headers().contains_key("content-length"), !streamed);
                let oversized = http.execute(request).await.unwrap();
                assert_eq!(
                    oversized.status(),
                    reqwest::StatusCode::PAYLOAD_TOO_LARGE,
                    "{source:?} {path} streamed={streamed}"
                );
                assert_markers(&oversized, source);
                assert_eq!(calls.load(Ordering::SeqCst), before);
            }
            let method = http
                .put(&url)
                .bearer_auth("marker-token")
                .send()
                .await
                .unwrap();
            assert_eq!(method.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
            assert_markers(&method, source);
            assert_eq!(calls.load(Ordering::SeqCst), before);
            if source == Source::Aggregator {
                let valid = http.post(&url).bearer_auth("marker-token")
                    .header("accept", "application/json, text/event-stream")
                    .header(SOURCE_HEADER, source.as_str())
                    .header(PROTOCOL_VERSION_HEADER, "1")
                    .json(&serde_json::json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":params}))
                    .send().await.unwrap();
                assert_eq!(valid.status(), reqwest::StatusCode::OK);
                assert_markers(&valid, source);
                let body = valid.json::<serde_json::Value>().await.unwrap();
                assert!(body.get("result").is_some(), "{body}");
                assert_ne!(body["result"]["isError"], serde_json::json!(true));
                assert!(calls.load(Ordering::SeqCst) > before);
                before = calls.load(Ordering::SeqCst);
                let mut boundary = serde_json::json!({
                    "jsonrpc":"2.0", "id":91, "method":"tools/call", "params":params,
                    "padding":""
                });
                let base_len = serde_json::to_vec(&boundary).unwrap().len();
                boundary["padding"] = serde_json::json!("x".repeat(1024 - base_len));
                let boundary = serde_json::to_vec(&boundary).unwrap();
                assert_eq!(boundary.len(), 1024);
                for streamed in [false, true] {
                    let mut request = http
                        .post(&url)
                        .bearer_auth("marker-token")
                        .header("accept", "application/json, text/event-stream")
                        .header("content-type", "application/json");
                    if streamed {
                        let chunks = boundary
                            .chunks(127)
                            .map(|chunk| Ok::<_, std::io::Error>(chunk.to_vec()))
                            .collect::<Vec<_>>();
                        request = request.header("transfer-encoding", "chunked").body(
                            reqwest::Body::wrap_stream(futures_util::stream::iter(chunks)),
                        );
                    } else {
                        request = request
                            .header("content-length", boundary.len())
                            .body(boundary.clone());
                    }
                    let request = request.build().unwrap();
                    assert_eq!(request.headers().contains_key("content-length"), !streamed);
                    let response = http.execute(request).await.unwrap();
                    assert_eq!(response.status(), reqwest::StatusCode::OK);
                    assert_markers(&response, source);
                    let body = response.json::<serde_json::Value>().await.unwrap();
                    assert!(body.get("result").is_some(), "{body}");
                    assert_ne!(body["result"]["isError"], serde_json::json!(true));
                    assert!(calls.load(Ordering::SeqCst) > before);
                    before = calls.load(Ordering::SeqCst);
                }
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), before);
        gateway.shutdown().await;
    }
    upstream_shutdown.cancel();
}

#[tokio::test]
async fn sources_stay_pure_across_modes_entry_methods_and_direct_alias() {
    let (upstream_url, upstream_shutdown, calls) = spawn_counted_fixture().await;
    let root = tempfile::tempdir().unwrap();
    let memory = Arc::new(MemoryIndexService::new(root.path().to_path_buf()));
    let internal = Arc::new(McpCore::new_internal(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
        BuiltInRegistry::memory_backed(memory, None),
    ));
    // Rejected before opening even the fixture connection.
    assert!(internal
        .connect_and_add(McpUpstreamServer {
            id: "fixture".into(),
            name: String::new(),
            policy: Default::default(),
            enabled: true,
            transport: McpUpstreamTransport::StreamableHttp {
                url: upstream_url.clone(),
                headers: Default::default(),
                oauth: None,
            },
        })
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let aggregator = Arc::new(McpCore::new_aggregator(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
    ));
    aggregator
        .connect_and_add(McpUpstreamServer {
            id: "fixture".into(),
            name: String::new(),
            policy: Default::default(),
            enabled: true,
            transport: McpUpstreamTransport::StreamableHttp {
                url: upstream_url,
                headers: Default::default(),
                oauth: None,
            },
        })
        .await
        .unwrap();

    for (source, core) in [
        (Source::Internal, internal),
        (Source::Aggregator, aggregator),
    ] {
        let gateway = McpHttpGateway::bind(core, gateway_config("matrix-token"))
            .await
            .unwrap();
        let server = if source == Source::Internal {
            BUILTIN_SESSION_MEMORY
        } else {
            "fixture"
        };
        let tool = if source == Source::Internal {
            "memory_projects"
        } else {
            "echo"
        };
        let other = if source == Source::Internal {
            "fixture"
        } else {
            BUILTIN_SESSION_MEMORY
        };
        for path in ["/mcp", "/mcp/entry", "/mcp/aggregator", "/mcp/direct"] {
            let endpoint = format!("http://127.0.0.1:{}{path}", gateway.endpoint().port);
            let client = serve_client(
                rmcp::model::ClientConfig::default(),
                client_transport(&endpoint, "matrix-token"),
            )
            .await
            .unwrap();
            let listed = client.list_all_tools().await.unwrap();
            let names = listed
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>();
            let (name, arguments) = match path {
                "/mcp" => {
                    assert!(
                        names.contains(&format!("{server}_tool_call").as_str()),
                        "{source:?} {path}: {names:?}"
                    );
                    assert!(!names.contains(&format!("{other}_tool_call").as_str()));
                    (
                        format!("{server}_tool_call"),
                        serde_json::json!({"toolName":tool,"arguments":{}}),
                    )
                }
                "/mcp/entry" => {
                    assert_eq!(names.len(), 4);
                    for (method, args) in [
                        ("list_mcp_servers", serde_json::json!({})),
                        ("list_mcp_tools", serde_json::json!({"mcpName":server})),
                        (
                            "get_mcp_server_status",
                            serde_json::json!({"mcpName":server}),
                        ),
                    ] {
                        let result = client
                            .call_tool(
                                CallToolRequestParams::new(method)
                                    .with_arguments(args.as_object().unwrap().clone()),
                            )
                            .await
                            .unwrap();
                        assert_ne!(result.is_error, Some(true));
                        let text = serde_json::to_string(&result).unwrap();
                        assert!(text.contains(server), "{source:?} {method}: {text}");
                        assert!(!text.contains(other), "{source:?} {method}: {text}");
                    }
                    (
                        "call_mcp_tool".into(),
                        serde_json::json!({"mcpName":server,"toolName":tool,"arguments":{}}),
                    )
                }
                _ => {
                    assert!(names.contains(&format!("{server}_{tool}").as_str()));
                    assert!(!names
                        .iter()
                        .any(|name| name.starts_with(&format!("{other}_"))));
                    (format!("{server}_{tool}"), serde_json::json!({}))
                }
            };
            let result = client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(arguments.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_ne!(result.is_error, Some(true), "{source:?} {path}: {result:?}");
            let denied = match path {
                "/mcp" => CallToolRequestParams::new(other.to_owned()).with_arguments(
                    serde_json::json!({"tool":"echo"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                "/mcp/entry" => CallToolRequestParams::new("call_mcp_tool").with_arguments(
                    serde_json::json!({"mcpName":other,"toolName":"echo"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                _ => CallToolRequestParams::new(format!("{other}_echo")),
            };
            let before = calls.load(Ordering::SeqCst);
            let denied = client.call_tool(denied).await;
            assert!(denied.is_err() || denied.unwrap().is_error == Some(true));
            assert_eq!(calls.load(Ordering::SeqCst), before);

            let resources = client.list_resources(None).await.unwrap().resources;
            let prompts = client.list_prompts(None).await.unwrap().prompts;
            if source == Source::Internal {
                assert!(resources.is_empty());
                assert!(prompts.is_empty());
                let before = calls.load(Ordering::SeqCst);
                assert!(client
                    .read_resource(ReadResourceRequestParams::new("fixture://one"))
                    .await
                    .is_err());
                assert!(client
                    .get_prompt(GetPromptRequestParams::new("fixture_review"))
                    .await
                    .is_err());
                assert_eq!(calls.load(Ordering::SeqCst), before);
            } else {
                assert_eq!(resources.len(), 1);
                assert_eq!(prompts.len(), 1);
                client
                    .read_resource(ReadResourceRequestParams::new(resources[0].uri.clone()))
                    .await
                    .unwrap();
                client
                    .get_prompt(GetPromptRequestParams::new(prompts[0].name.clone()))
                    .await
                    .unwrap();
            }
            let _ = client.cancel().await;
        }
        gateway.shutdown().await;
    }
    upstream_shutdown.cancel();
}

#[tokio::test]
async fn authenticated_http_gateway_supports_two_isolated_clients_and_both_paths() {
    let (upstream_url, upstream_shutdown) = spawn_fixture().await;
    let core = Arc::new(McpCore::new_aggregator(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
    ));
    core.connect_and_add(McpUpstreamServer {
        id: "fixture".into(),
        name: String::new(),
        policy: Default::default(),
        enabled: true,
        transport: McpUpstreamTransport::StreamableHttp {
            url: upstream_url,
            headers: Default::default(),
            oauth: None,
        },
    })
    .await
    .unwrap();
    let gateway = McpHttpGateway::bind(core, gateway_config("token-a"))
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}", gateway.endpoint().port);
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{base}/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/mcp"))
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    let first = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp"), "token-a"),
    )
    .await
    .unwrap();
    let second = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp/aggregator"), "token-a"),
    )
    .await
    .unwrap();

    let first_tools = first.list_all_tools().await.unwrap();
    let second_tools = second.list_all_tools().await.unwrap();
    assert!(first_tools
        .iter()
        .any(|tool| tool.name == "fixture_tool_list"));
    assert!(first_tools
        .iter()
        .any(|tool| tool.name == "fixture_tool_call"));
    assert!(second_tools.iter().any(|tool| tool.name == "fixture_echo"));

    let first_catalog = first
        .call_tool(
            CallToolRequestParams::new("fixture_tool_list")
                .with_arguments(serde_json::json!({}).as_object().cloned().unwrap()),
        )
        .await
        .unwrap();
    assert!(first_catalog.content[0]
        .as_text()
        .unwrap()
        .text
        .contains("echo"));
    let first_result = first
        .call_tool(
            CallToolRequestParams::new("fixture_tool_call").with_arguments(
                serde_json::json!({
                    "serverId": "fixture",
                    "toolName": "echo",
                    "arguments": {}
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    let second_result = second
        .call_tool(CallToolRequestParams::new("fixture_echo"))
        .await
        .unwrap();
    assert_eq!(first_result.content[0].as_text().unwrap().text, "fixture");
    assert_eq!(second_result.content[0].as_text().unwrap().text, "fixture");

    first.cancel().await.unwrap();
    let still_usable = second
        .call_tool(CallToolRequestParams::new("fixture_echo"))
        .await
        .unwrap();
    assert_eq!(still_usable.content[0].as_text().unwrap().text, "fixture");
    second.cancel().await.unwrap();
    gateway.shutdown().await;
    upstream_shutdown.cancel();
}

#[tokio::test]
async fn grouped_and_entry_modes_route_to_one_named_server() {
    let (upstream_url, upstream_shutdown) = spawn_fixture().await;
    let core = Arc::new(McpCore::new_aggregator(
        McpCoreConfig::default(),
        Arc::new(AllowAllTools),
    ));
    core.connect_and_add(McpUpstreamServer {
        id: "0b84257e".into(),
        name: "fixture".into(),
        policy: Default::default(),
        enabled: true,
        transport: McpUpstreamTransport::StreamableHttp {
            url: upstream_url,
            headers: Default::default(),
            oauth: None,
        },
    })
    .await
    .unwrap();
    let gateway = McpHttpGateway::bind(core, gateway_config("token-m"))
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}", gateway.endpoint().port);
    let arguments = |value: serde_json::Value| value.as_object().cloned().unwrap();
    let text =
        |result: &rmcp::model::CallToolResult| result.content[0].as_text().unwrap().text.clone();

    let grouped = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp"), "token-m"),
    )
    .await
    .unwrap();
    let names = grouped
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, ["fixture_tool_list", "fixture_tool_call"]);
    let listed = grouped
        .call_tool(
            CallToolRequestParams::new("fixture_tool_list")
                .with_arguments(arguments(serde_json::json!({ "query": "echo" }))),
        )
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_str(&text(&listed)).unwrap();
    assert_eq!(listed["tools"][0]["name"], "echo");
    assert!(listed["tools"][0]["inputSchema"].is_object());
    let called = grouped
        .call_tool(
            CallToolRequestParams::new("fixture_tool_call")
                .with_arguments(arguments(serde_json::json!({ "toolName": "echo" }))),
        )
        .await
        .unwrap();
    assert_eq!(text(&called), "fixture");
    grouped.cancel().await.unwrap();

    let entry = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp/entry"), "token-m"),
    )
    .await
    .unwrap();
    assert_eq!(entry.list_all_tools().await.unwrap().len(), 4);
    let servers = entry
        .call_tool(CallToolRequestParams::new("list_mcp_servers"))
        .await
        .unwrap();
    let servers: serde_json::Value = serde_json::from_str(&text(&servers)).unwrap();
    assert_eq!(servers["servers"][0]["name"], "fixture");
    let called = entry
        .call_tool(
            CallToolRequestParams::new("call_mcp_tool").with_arguments(arguments(
                serde_json::json!({ "mcpName": "fixture", "toolName": "echo", "arguments": {} }),
            )),
        )
        .await
        .unwrap();
    assert_eq!(text(&called), "fixture");
    let missing = entry
        .call_tool(
            CallToolRequestParams::new("call_mcp_tool").with_arguments(arguments(
                serde_json::json!({ "mcpName": "nope", "toolName": "echo" }),
            )),
        )
        .await
        .unwrap();
    assert_eq!(missing.is_error, Some(true));
    assert!(text(&missing).contains("Available: fixture"));
    entry.cancel().await.unwrap();

    gateway.shutdown().await;
    upstream_shutdown.cancel();
}

#[tokio::test]
async fn credential_rotation_invalidates_old_requests_without_restarting_gateway() {
    let gateway = McpHttpGateway::bind(
        Arc::new(McpCore::new_aggregator(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
        )),
        gateway_config("old-token"),
    )
    .await
    .unwrap();
    let base = format!("http://127.0.0.1:{}", gateway.endpoint().port);
    let old = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp"), "old-token"),
    )
    .await
    .unwrap();
    assert!(old.list_all_tools().await.unwrap().is_empty());

    gateway
        .rotate_auth(AuthBootstrap::new(2, "new-token").unwrap())
        .await
        .unwrap();
    assert_eq!(gateway.current_endpoint().await.auth_generation, 2);
    assert!(matches!(
        gateway
            .rotate_auth(AuthBootstrap::new(1, "stale-token").unwrap())
            .await,
        Err(McpHttpGatewayError::StaleAuthGeneration { .. })
    ));
    assert!(old.list_all_tools().await.is_err());
    let _ = old.cancel().await;

    let fresh = serve_client(
        rmcp::model::ClientConfig::default(),
        client_transport(&format!("{base}/mcp"), "new-token"),
    )
    .await
    .unwrap();
    assert!(fresh.list_all_tools().await.unwrap().is_empty());
    fresh.cancel().await.unwrap();
    gateway.shutdown().await;
}

#[tokio::test]
async fn gateway_rejects_wildcard_bind_before_opening_listener() {
    let result = McpHttpGateway::bind(
        Arc::new(McpCore::new_aggregator(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
        )),
        McpHttpGatewayConfig {
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            ..gateway_config("token")
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(McpHttpGatewayError::NonLoopbackBind(_))
    ));
}
