use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, ServiceExt},
    ErrorData, RoleServer, ServerHandler,
};

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
            "echo",
            "Echo input",
            schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() == "echo" {
            Ok(CallToolResult::success(vec![ContentBlock::text("stdio")]).into())
        } else {
            Err(ErrorData::internal_error("unknown tool", None))
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (stdin, stdout) = rmcp::transport::io::stdio();
    Fixture.serve((stdin, stdout)).await?.waiting().await?;
    Ok(())
}
