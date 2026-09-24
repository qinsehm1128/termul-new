use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use axum::Router;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
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
    AuthBootstrap, McpCore, McpHttpGateway, McpHttpGatewayConfig, McpHttpGatewayError,
    McpUpstreamServer, McpUpstreamTransport,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Fixture;

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "echo", "Echo", schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() == "echo" {
            Ok(CallToolResult::success(vec![ContentBlock::text("fixture")]).into())
        } else {
            Err(ErrorData::internal_error("unknown tool", None))
        }
    }
}

async fn spawn_fixture() -> (String, CancellationToken) {
    let shutdown = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_cancellation_token(shutdown.child_token());
    let service: StreamableHttpService<Fixture, LocalSessionManager> = StreamableHttpService::new(
        || Ok(Fixture),
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
    (format!("http://{address}/mcp"), shutdown)
}

fn gateway_config(token: &str) -> McpHttpGatewayConfig {
    McpHttpGatewayConfig {
        bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        path: "/mcp".into(),
        generation: 1,
        auth: AuthBootstrap::new(1, token).unwrap(),
        request_body_limit: 64 * 1024,
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

#[tokio::test]
async fn authenticated_http_gateway_supports_two_isolated_clients_and_both_paths() {
    let (upstream_url, upstream_shutdown) = spawn_fixture().await;
    let core = Arc::new(McpCore::default());
    core.connect_and_add(McpUpstreamServer {
        id: "fixture".into(),
        enabled: true,
        transport: McpUpstreamTransport::StreamableHttp {
            url: upstream_url,
            headers: Default::default(),
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
    assert!(first_tools.iter().any(|tool| tool.name == "fixture_echo"));
    assert!(second_tools.iter().any(|tool| tool.name == "fixture_echo"));

    let first_result = first
        .call_tool(CallToolRequestParams::new("fixture_echo"))
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
async fn credential_rotation_invalidates_old_requests_without_restarting_gateway() {
    let gateway = McpHttpGateway::bind(Arc::new(McpCore::default()), gateway_config("old-token"))
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
        Arc::new(McpCore::default()),
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
