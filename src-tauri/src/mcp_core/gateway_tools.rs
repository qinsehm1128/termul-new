//! The two compact ways of exposing aggregated servers to an agent.
//!
//! * Grouped: every server contributes exactly two tools,
//!   `<name>_tool_list` and `<name>_tool_call`. The tool name already says
//!   which server it reaches, so an agent never has to discover server names
//!   first, and the tool list costs two short entries per server instead of
//!   every schema of every server.
//! * Entry: four fixed tools (`list_mcp_servers`, `list_mcp_tools`,
//!   `call_mcp_tool`, `get_mcp_server_status`), named exactly as in
//!   mcp-router so existing agent instructions keep working. Smallest tool
//!   list; one extra hop to learn server names.
//!
//! Lookup failures come back as tool results with `isError`, so the agent
//! reads the reason instead of the client surfacing a protocol error.

use std::{sync::Arc, time::Duration};

use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::{McpCore, McpDomainError, ServerSummary};

pub const GROUPED_LIST_SUFFIX: &str = "_tool_list";
pub const GROUPED_CALL_SUFFIX: &str = "_tool_call";

pub const ENTRY_LIST_SERVERS: &str = "list_mcp_servers";
pub const ENTRY_LIST_TOOLS: &str = "list_mcp_tools";
pub const ENTRY_CALL_TOOL: &str = "call_mcp_tool";
pub const ENTRY_SERVER_STATUS: &str = "get_mcp_server_status";

fn schema(value: Value) -> Arc<Map<String, Value>> {
    Arc::new(value.as_object().cloned().unwrap_or_default())
}

fn with_summary(lead: String, summary: &ServerSummary) -> String {
    if summary.description.is_empty() {
        lead
    } else {
        format!("{lead} Server: {}", summary.description)
    }
}

pub fn grouped_tools(servers: &[ServerSummary]) -> Vec<Tool> {
    let mut tools = Vec::with_capacity(servers.len() * 2);
    for server in servers {
        let name = &server.name;
        tools.push(Tool::new(
            format!("{name}{GROUPED_LIST_SUFFIX}"),
            with_summary(
                format!("List the tools of the `{name}` MCP server with their inputSchema."),
                server,
            ),
            schema(json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional keywords; keeps only tools whose name or description matches, best match first."
                    },
                    "includeSchema": {
                        "type": "boolean",
                        "description": "Return each tool's inputSchema. Default true."
                    }
                }
            })),
        ));
        tools.push(Tool::new(
            format!("{name}{GROUPED_CALL_SUFFIX}"),
            format!(
                "Call a tool of the `{name}` MCP server. Take toolName and the shape of arguments from {name}{GROUPED_LIST_SUFFIX}."
            ),
            schema(json!({
                "type": "object",
                "required": ["toolName"],
                "properties": {
                    "toolName": { "type": "string" },
                    "arguments": { "type": "object", "additionalProperties": true }
                }
            })),
        ));
    }
    tools
}

pub fn entry_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            ENTRY_LIST_SERVERS,
            "List the available MCP servers with what each is for. Start here.",
            schema(json!({ "type": "object", "properties": {} })),
        ),
        Tool::new(
            ENTRY_LIST_TOOLS,
            "List the tools of one MCP server with their inputSchema. mcpName may be omitted when only one server exists; query filters tools by keywords.",
            schema(json!({
                "type": "object",
                "properties": {
                    "mcpName": { "type": "string", "description": "Server name from list_mcp_servers." },
                    "query": { "type": "string", "description": "Optional keywords to filter tools." }
                }
            })),
        ),
        Tool::new(
            ENTRY_CALL_TOOL,
            "Call a tool on an MCP server. Read the tool's inputSchema with list_mcp_tools first.",
            schema(json!({
                "type": "object",
                "required": ["mcpName", "toolName"],
                "properties": {
                    "mcpName": { "type": "string" },
                    "toolName": { "type": "string" },
                    "arguments": { "type": "object", "additionalProperties": true },
                    "timeoutSec": { "type": "number", "minimum": 1, "description": "Optional; capped by the gateway's own limit." }
                }
            })),
        ),
        Tool::new(
            ENTRY_SERVER_STATUS,
            "Report whether an MCP server is connected, and why not when it failed.",
            schema(json!({
                "type": "object",
                "required": ["mcpName"],
                "properties": { "mcpName": { "type": "string" } }
            })),
        ),
    ]
}

/// Dispatch a grouped-mode call. `None` means the name is not a grouped tool.
pub async fn call_grouped(
    core: &McpCore,
    name: &str,
    arguments: Map<String, Value>,
    cancellation: CancellationToken,
) -> Option<CallToolResult> {
    if let Some(server) = name.strip_suffix(GROUPED_LIST_SUFFIX) {
        let query = string_argument(&arguments, "query");
        let include_schema = arguments
            .get("includeSchema")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        return Some(list_tools_result(core, server, query, include_schema).await);
    }
    let server = name.strip_suffix(GROUPED_CALL_SUFFIX)?;
    // Older clients also sent the server id; accept it only when it agrees.
    if let Some(requested) = string_argument(&arguments, "serverId") {
        if requested != server {
            return Some(failure(format!(
                "serverId `{requested}` does not match {name}; omit it"
            )));
        }
    }
    let Some(tool) = string_argument(&arguments, "toolName") else {
        return Some(failure(format!(
            "toolName is required; list the tools with {server}{GROUPED_LIST_SUFFIX}"
        )));
    };
    Some(
        call_result(
            core,
            server,
            tool,
            object_argument(&arguments),
            None,
            cancellation,
        )
        .await,
    )
}

/// Dispatch an entry-mode call. `None` means the name is not an entry tool.
pub async fn call_entry(
    core: &McpCore,
    name: &str,
    arguments: Map<String, Value>,
    cancellation: CancellationToken,
) -> Option<CallToolResult> {
    match name {
        ENTRY_LIST_SERVERS => {
            let mut servers = Vec::new();
            for summary in core.server_summaries().await {
                servers.push(json!({
                    "name": summary.name,
                    "description": summary.description,
                }));
            }
            Some(success(json!({
                "servers": servers,
                "recommendedNextAction": "Call list_mcp_tools with the mcpName that fits the task, then call_mcp_tool."
            })))
        }
        ENTRY_LIST_TOOLS => {
            let query = string_argument(&arguments, "query");
            let server = match string_argument(&arguments, "mcpName") {
                Some(server) => server.to_owned(),
                None => match only_server(core).await {
                    Ok(server) => server,
                    Err(result) => return Some(result),
                },
            };
            Some(list_tools_result(core, &server, query, true).await)
        }
        ENTRY_CALL_TOOL => {
            let (Some(server), Some(tool)) = (
                string_argument(&arguments, "mcpName"),
                string_argument(&arguments, "toolName"),
            ) else {
                return Some(failure("mcpName and toolName are required"));
            };
            let timeout = arguments
                .get("timeoutSec")
                .and_then(Value::as_f64)
                .filter(|seconds| seconds.is_finite() && *seconds >= 1.0)
                .map(Duration::from_secs_f64);
            Some(
                call_result(
                    core,
                    server,
                    tool,
                    object_argument(&arguments),
                    timeout,
                    cancellation,
                )
                .await,
            )
        }
        ENTRY_SERVER_STATUS => {
            let Some(server) = string_argument(&arguments, "mcpName") else {
                return Some(failure("mcpName is required"));
            };
            Some(match core.server_status(server).await {
                Some(status) => success(json!(status)),
                None => unknown_server(core, server).await,
            })
        }
        _ => None,
    }
}

async fn only_server(core: &McpCore) -> Result<String, CallToolResult> {
    let servers = core.server_summaries().await;
    match servers.as_slice() {
        [only] => Ok(only.name.clone()),
        _ => Err(failure(format!(
            "mcpName is required; servers: {}",
            server_names(&servers)
        ))),
    }
}

async fn list_tools_result(
    core: &McpCore,
    server: &str,
    query: Option<&str>,
    include_schema: bool,
) -> CallToolResult {
    let summary = core
        .server_summaries()
        .await
        .into_iter()
        .find(|summary| summary.name == server);
    let Some(summary) = summary else {
        return unknown_server(core, server).await;
    };
    let tools = match core.list_server_tools(server).await {
        Ok(tools) => tools,
        Err(error) => return failure(error.to_string()),
    };
    let tools = rank(tools, query)
        .into_iter()
        .map(|tool| {
            let mut entry = json!({
                "name": tool.name,
                "description": tool.description.as_deref().unwrap_or_default(),
            });
            if include_schema {
                entry["inputSchema"] = Value::Object((*tool.input_schema).clone());
            }
            entry
        })
        .collect::<Vec<_>>();
    success(json!({
        "server": summary.name,
        "description": summary.description,
        "tools": tools,
    }))
}

async fn call_result(
    core: &McpCore,
    server: &str,
    tool: &str,
    arguments: Option<Map<String, Value>>,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> CallToolResult {
    if core.server_status(server).await.is_none() {
        return unknown_server(core, server).await;
    }
    let call = core.call_server_tool(server, tool, arguments, cancellation);
    let result = match timeout {
        Some(limit) => match tokio::time::timeout(limit, call).await {
            Ok(result) => result,
            Err(_) => Err(McpDomainError::Timeout {
                server_id: server.to_owned(),
                operation: super::Operation::CallTool,
            }),
        },
        None => call.await,
    };
    result.unwrap_or_else(|error| failure(error.to_string()))
}

async fn unknown_server(core: &McpCore, server: &str) -> CallToolResult {
    let servers = core.server_summaries().await;
    let reason = match core.server_status(server).await {
        Some(status) => match status.error {
            Some(error) => format!("MCP server `{server}` is {:?}: {error}", status.state),
            None => format!("MCP server `{server}` is {:?}", status.state),
        },
        None => format!("no MCP server named `{server}`"),
    };
    failure(format!("{reason}. Available: {}", server_names(&servers)))
}

fn server_names(servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return "none".to_owned();
    }
    servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Keep tools matching any query word (name or description, case
/// insensitive), best first: a hit in the name counts double.
fn rank(tools: Vec<Tool>, query: Option<&str>) -> Vec<Tool> {
    let words = query
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return tools;
    }
    let mut scored = tools
        .into_iter()
        .filter_map(|tool| {
            let name = tool.name.to_lowercase();
            let description = tool
                .description
                .as_deref()
                .unwrap_or_default()
                .to_lowercase();
            let score = words
                .iter()
                .map(|word| {
                    2 * usize::from(name.contains(word)) + usize::from(description.contains(word))
                })
                .sum::<usize>();
            (score > 0).then_some((score, tool))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.cmp(&left.0));
    scored.into_iter().map(|(_, tool)| tool).collect()
}

fn string_argument<'a>(arguments: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn object_argument(arguments: &Map<String, Value>) -> Option<Map<String, Value>> {
    arguments
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
}

fn success(value: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}

fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> Tool {
        Tool::new(name.to_owned(), description.to_owned(), schema(json!({})))
    }

    #[test]
    fn ranking_prefers_name_hits_and_drops_misses() {
        let ranked = rank(
            vec![
                tool("fetch", "download a page"),
                tool("search", "search the web"),
                tool("crawl", "follow links and search"),
            ],
            Some("Search"),
        );
        let names = ranked
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(names, ["search", "crawl"]);
    }

    #[test]
    fn grouped_tools_are_two_per_server_and_carry_the_summary() {
        let tools = grouped_tools(&[ServerSummary {
            name: "context7".into(),
            description: "Library docs".into(),
            built_in: false,
        }]);
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(names, ["context7_tool_list", "context7_tool_call"]);
        assert!(tools[0]
            .description
            .as_deref()
            .unwrap()
            .contains("Library docs"));
    }

    #[test]
    fn entry_tool_names_match_mcp_router() {
        let names = entry_tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_mcp_servers",
                "list_mcp_tools",
                "call_mcp_tool",
                "get_mcp_server_status"
            ]
        );
    }
}
