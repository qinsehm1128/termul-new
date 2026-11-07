//! Runtime-neutral boundary between MCP providers and the Se host.
//!
//! Authentication and grants belong to the host implementation. Tool arguments
//! cannot select a caller or grant themselves permissions. Credentials never
//! cross this boundary, including when the gateway runs in a separate process.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::{
    builtins::BuiltInCapability,
    domain::{McpDomainError, Operation},
    host_tools::host_tools,
};

pub const WORKSPACE_PROVIDER: &str = "workspace";
pub const SSH_PROVIDER: &str = "ssh";
pub const HOST_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
pub const HOST_REQUEST_LIMIT: usize = 1024 * 1024;

/// Messages must be safe for an external caller. Never embed raw OS/SSH errors,
/// credentials, commands, paths or remote output in this error envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCapabilityError {
    pub code: String,
    pub message: String,
}

impl HostCapabilityError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unavailable() -> Self {
        Self::new("HOST_UNAVAILABLE", "The Se host is not available")
    }
}

#[async_trait]
pub trait HostCapabilityBackend: Send + Sync {
    async fn call(
        &self,
        provider: &str,
        tool: &str,
        arguments: Map<String, Value>,
        cancellation: CancellationToken,
    ) -> Result<Value, HostCapabilityError>;
}

/// One wrapper works both in-process (standalone) and over the private host
/// bridge (desktop). It never executes SSH commands in the gateway process.
pub struct HostBackedCapability {
    provider: &'static str,
    backend: Arc<dyn HostCapabilityBackend>,
}

impl HostBackedCapability {
    pub fn workspace(backend: Arc<dyn HostCapabilityBackend>) -> Self {
        Self {
            provider: WORKSPACE_PROVIDER,
            backend,
        }
    }

    pub fn ssh(backend: Arc<dyn HostCapabilityBackend>) -> Self {
        Self {
            provider: SSH_PROVIDER,
            backend,
        }
    }
}

#[async_trait]
impl BuiltInCapability for HostBackedCapability {
    fn id(&self) -> &str {
        self.provider
    }

    fn description(&self) -> &str {
        match self.provider {
            WORKSPACE_PROVIDER => "Read Se's registered project groups and projects, including projects without conversation history.",
            _ => "Use explicitly authorized Se SSH profiles for commands, SFTP files and managed jobs. Credentials stay on the host.",
        }
    }

    fn tools(&self) -> Vec<Tool> {
        host_tools(self.provider)
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpDomainError> {
        if !self.tools().iter().any(|tool| tool.name.as_ref() == name) {
            return Err(McpDomainError::ToolNotFound(name.into()));
        }
        let arguments = arguments.unwrap_or_default();
        if arguments
            .keys()
            .any(|key| matches!(key.as_str(), "callerId" | "grant" | "confirmed"))
        {
            return Err(McpDomainError::InvalidArguments {
                server_id: self.provider.into(),
                name: name.into(),
            });
        }
        let call = self
            .backend
            .call(self.provider, name, arguments, cancellation.clone());
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(McpDomainError::Cancelled {
                server_id: self.provider.into(), operation: Operation::CallTool,
            }),
            result = call => result,
        };
        let (value, failed) = match result {
            Ok(value) => (value, false),
            Err(error) => {
                log::warn!(target: "se_manager::mcp_core::host", "operation=host_capability_call provider={} tool={} stable_code={}", self.provider, name, error.code);
                (
                    serde_json::to_value(error).map_err(|_| McpDomainError::Serialization)?,
                    true,
                )
            }
        };
        let text = serde_json::to_string(&value).map_err(|_| McpDomainError::Serialization)?;
        if text.len() > HOST_RESPONSE_LIMIT {
            return Err(McpDomainError::ResponseTooLarge {
                limit: HOST_RESPONSE_LIMIT,
                actual: text.len(),
            });
        }
        let content = vec![ContentBlock::text(text)];
        Ok(if failed {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        })
    }
}
