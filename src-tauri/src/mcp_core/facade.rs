//! Facade catalog and tool-call contracts.
//!
//! Each enabled server is projected as `<server_id>_tool_list` and
//! `<server_id>_tool_call`. Failures stay attached to that server. This module
//! does not dispatch tools or read the user MCP registry.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MCP_SERVER_ID_MAX_LENGTH: usize = 64;
pub const MCP_TOOL_NAME_MAX_LENGTH: usize = 128;
pub const MCP_TEXT_MAX_CHARS: usize = 4096;
pub const MCP_LIST_MAX: usize = 32;
pub const MCP_TOOLS_MAX: usize = 256;
pub const MCP_SCHEMA_MAX_BYTES: usize = 65_536;
pub const MCP_FAILURES_MAX: usize = 16;
pub const MCP_QUERY_MAX_CHARS: usize = 256;
pub const MCP_FAILURE_MESSAGE_MAX_CHARS: usize = 512;

const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpFacadeError {
    InvalidJson,
    ForbiddenCredentialField,
    InvalidId,
    InvalidDocument,
    StaleCatalog,
}

impl fmt::Display for McpFacadeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJson => "MCP facade JSON is invalid",
            Self::ForbiddenCredentialField => "MCP facade contains a forbidden credential field",
            Self::InvalidId => "MCP facade id is invalid",
            Self::InvalidDocument => "MCP facade document is invalid",
            Self::StaleCatalog => "MCP facade catalog revision is stale",
        })
    }
}

impl std::error::Error for McpFacadeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpFacadeFailureCode {
    UpstreamUnavailable,
    SchemaInvalid,
    TimedOut,
    Cancelled,
    Unauthorized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacadeTool {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub avoid_when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    pub read_only: bool,
    pub destructive: bool,
    pub confirmation_required: bool,
    pub allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacadeFailure {
    pub server_id: String,
    pub code: McpFacadeFailureCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacadeCatalog {
    pub server_id: String,
    pub catalog_revision: u64,
    pub description: String,
    pub when_to_use: Vec<String>,
    pub avoid_when: Vec<String>,
    pub tools: Vec<McpFacadeTool>,
    pub failures: Vec<McpFacadeFailure>,
}

impl McpFacadeCatalog {
    pub fn from_json(raw: &str) -> Result<Self, McpFacadeError> {
        let value = serde_json::from_str(raw).map_err(|_| McpFacadeError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, McpFacadeError> {
        inspect_facade(&value, false, 0)?;
        let catalog: Self =
            serde_json::from_value(value).map_err(|_| McpFacadeError::InvalidDocument)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), McpFacadeError> {
        if !is_safe_id(&self.server_id, MCP_SERVER_ID_MAX_LENGTH) {
            return Err(McpFacadeError::InvalidId);
        }
        require_revision(self.catalog_revision)?;
        require_prose(&self.description)?;
        require_prose_list(&self.when_to_use)?;
        require_prose_list(&self.avoid_when)?;
        if self.tools.len() > MCP_TOOLS_MAX || self.failures.len() > MCP_FAILURES_MAX {
            return Err(McpFacadeError::InvalidDocument);
        }
        let mut names = BTreeSet::new();
        for tool in &self.tools {
            if !is_safe_id(&tool.name, MCP_TOOL_NAME_MAX_LENGTH) || !names.insert(tool.name.clone())
            {
                return Err(McpFacadeError::InvalidId);
            }
            require_prose(&tool.description)?;
            require_prose(&tool.when_to_use)?;
            require_prose(&tool.avoid_when)?;
            if let Some(schema) = &tool.input_schema {
                require_schema_object(schema)?;
            }
        }
        for failure in &self.failures {
            if !is_safe_id(&failure.server_id, MCP_SERVER_ID_MAX_LENGTH) {
                return Err(McpFacadeError::InvalidId);
            }
            if failure.server_id != self.server_id
                || !is_prose(&failure.message, MCP_FAILURE_MESSAGE_MAX_CHARS, true, false)
            {
                return Err(McpFacadeError::InvalidDocument);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacadeListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default)]
    pub include_schema: bool,
}

impl McpFacadeListQuery {
    pub fn from_value(value: Value) -> Result<Self, McpFacadeError> {
        inspect_facade(&value, false, 0)?;
        let query: Self =
            serde_json::from_value(value).map_err(|_| McpFacadeError::InvalidDocument)?;
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<(), McpFacadeError> {
        if let Some(query) = &self.query {
            if !is_prose(query, MCP_QUERY_MAX_CHARS, true, false) {
                return Err(McpFacadeError::InvalidDocument);
            }
        }
        if let Some(limit) = self.limit {
            if limit == 0 || usize::try_from(limit).unwrap_or(usize::MAX) > MCP_TOOLS_MAX {
                return Err(McpFacadeError::InvalidDocument);
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacadeToolCall {
    pub server_id: String,
    pub tool_name: String,
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<u64>,
}

impl fmt::Debug for McpFacadeToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpFacadeToolCall")
            .field("server_id", &self.server_id)
            .field("tool_name", &self.tool_name)
            .field("arguments", &"<redacted>")
            .field("catalog_revision", &self.catalog_revision)
            .finish()
    }
}

impl McpFacadeToolCall {
    pub fn from_json(raw: &str) -> Result<Self, McpFacadeError> {
        let value = serde_json::from_str(raw).map_err(|_| McpFacadeError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, McpFacadeError> {
        inspect_facade(&value, false, 0)?;
        let call: Self =
            serde_json::from_value(value).map_err(|_| McpFacadeError::InvalidDocument)?;
        call.validate()?;
        Ok(call)
    }

    pub fn validate(&self) -> Result<(), McpFacadeError> {
        if !is_safe_id(&self.server_id, MCP_SERVER_ID_MAX_LENGTH)
            || !is_safe_id(&self.tool_name, MCP_TOOL_NAME_MAX_LENGTH)
        {
            return Err(McpFacadeError::InvalidId);
        }
        require_schema_object(&self.arguments)?;
        if let Some(revision) = self.catalog_revision {
            require_revision(revision)?;
        }
        Ok(())
    }
}

pub fn facade_tool_names(server_id: &str) -> Result<(String, String), McpFacadeError> {
    if !is_safe_id(server_id, MCP_SERVER_ID_MAX_LENGTH) {
        return Err(McpFacadeError::InvalidId);
    }
    Ok((
        format!("{server_id}_tool_list"),
        format!("{server_id}_tool_call"),
    ))
}

pub fn require_current_catalog_revision(
    requested: Option<u64>,
    current: u64,
) -> Result<(), McpFacadeError> {
    require_revision(current)?;
    match requested {
        None => Ok(()),
        Some(revision) if revision == current => Ok(()),
        Some(revision) => {
            require_revision(revision)?;
            Err(McpFacadeError::StaleCatalog)
        }
    }
}

fn require_revision(revision: u64) -> Result<(), McpFacadeError> {
    if revision == 0 || revision > MAX_SAFE_INTEGER {
        Err(McpFacadeError::InvalidDocument)
    } else {
        Ok(())
    }
}

fn require_prose(value: &str) -> Result<(), McpFacadeError> {
    if is_prose(value, MCP_TEXT_MAX_CHARS, true, true) {
        Ok(())
    } else {
        Err(McpFacadeError::InvalidDocument)
    }
}

fn require_prose_list(values: &[String]) -> Result<(), McpFacadeError> {
    if values.len() > MCP_LIST_MAX {
        return Err(McpFacadeError::InvalidDocument);
    }
    values.iter().try_for_each(|value| require_prose(value))
}

fn require_schema_object(value: &Value) -> Result<(), McpFacadeError> {
    if !value.is_object() {
        return Err(McpFacadeError::InvalidDocument);
    }
    let bytes = serde_json::to_vec(value).map_err(|_| McpFacadeError::InvalidDocument)?;
    if bytes.len() > MCP_SCHEMA_MAX_BYTES {
        Err(McpFacadeError::InvalidDocument)
    } else {
        Ok(())
    }
}

fn is_safe_id(value: &str, max_length: usize) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && value.len() <= max_length
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn is_prose(value: &str, max_chars: usize, allow_empty: bool, allow_newlines: bool) -> bool {
    if value.trim() != value {
        return false;
    }
    let count = value.chars().count();
    if count > max_chars || (!allow_empty && count == 0) {
        return false;
    }
    value.chars().all(|ch| {
        let code = u32::from(ch);
        let newline = allow_newlines && (code == 9 || code == 10);
        newline || (code >= 32 && code != 127)
    })
}

fn inspect_facade(value: &Value, in_schema: bool, depth: usize) -> Result<(), McpFacadeError> {
    if depth > 40 {
        return Err(McpFacadeError::InvalidDocument);
    }
    match value {
        Value::Null if in_schema => Ok(()),
        Value::Null => Err(McpFacadeError::InvalidDocument),
        Value::String(text) if text.len() > 100_000 => Err(McpFacadeError::InvalidDocument),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Ok(()),
        Value::Array(items) => {
            if items.len() > 10_000 {
                return Err(McpFacadeError::InvalidDocument);
            }
            items
                .iter()
                .try_for_each(|item| inspect_facade(item, in_schema, depth + 1))
        }
        Value::Object(map) => {
            if !in_schema && map.len() > 256 {
                return Err(McpFacadeError::InvalidDocument);
            }
            for (key, child) in map {
                if !in_schema && is_forbidden_credential_key(key) {
                    return Err(McpFacadeError::ForbiddenCredentialField);
                }
                let child_in_schema = in_schema || key == "inputSchema" || key == "arguments";
                inspect_facade(child, child_in_schema, depth + 1)?;
            }
            Ok(())
        }
    }
}

fn is_forbidden_credential_key(key: &str) -> bool {
    matches!(
        normalize_key(key).as_str(),
        "apikey"
            | "apisecret"
            | "secret"
            | "token"
            | "bearer"
            | "bearertoken"
            | "accesstoken"
            | "refreshtoken"
            | "password"
            | "passwd"
            | "authorization"
            | "clientsecret"
            | "idtoken"
            | "privatekey"
            | "credential"
            | "credentials"
            | "rawkey"
            | "sessiontoken"
    )
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CANARY: &str = "sk-canary-secret";

    #[test]
    fn round_trips_catalog_and_omits_absent_schema() {
        let catalog = McpFacadeCatalog::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        assert_eq!(catalog.server_id, "dbx");
        assert_eq!(catalog.catalog_revision, 18);
        assert_eq!(
            catalog.tools[0].input_schema,
            Some(json!({"type": "object"}))
        );
        assert!(catalog.failures.is_empty());
        let encoded = serde_json::to_value(&catalog).unwrap();
        assert_eq!(McpFacadeCatalog::from_value(encoded).unwrap(), catalog);

        let mut lightweight = serde_json::to_value(&catalog).unwrap();
        lightweight["tools"][0]
            .as_object_mut()
            .unwrap()
            .remove("inputSchema");
        let parsed = McpFacadeCatalog::from_value(lightweight).unwrap();
        assert!(parsed.tools[0].input_schema.is_none());
        assert!(serde_json::to_value(&parsed).unwrap()["tools"][0]
            .get("inputSchema")
            .is_none());
    }

    #[test]
    fn rejects_invalid_ids_stale_revisions_and_credential_fields() {
        assert_eq!(
            facade_tool_names("dbx").unwrap(),
            ("dbx_tool_list".to_owned(), "dbx_tool_call".to_owned())
        );
        assert_eq!(
            facade_tool_names("../dbx").unwrap_err(),
            McpFacadeError::InvalidId
        );

        let mut catalog: Value = serde_json::from_str(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        catalog["tools"][0]["name"] = json!("has space");
        assert_eq!(
            McpFacadeCatalog::from_value(catalog.clone()).unwrap_err(),
            McpFacadeError::InvalidId
        );
        catalog["tools"][0]["name"] = json!("mongo_find_documents");
        catalog["tools"][0]["token"] = json!(CANARY);
        let error = McpFacadeCatalog::from_value(catalog).unwrap_err();
        assert_eq!(error, McpFacadeError::ForbiddenCredentialField);
        assert!(!error.to_string().contains(CANARY));

        assert_eq!(
            require_current_catalog_revision(Some(17), 18).unwrap_err(),
            McpFacadeError::StaleCatalog
        );
        assert!(require_current_catalog_revision(None, 18).is_ok());
    }

    #[test]
    fn isolates_failures_to_the_catalog_server() {
        let mut catalog: Value = serde_json::from_str(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        catalog["failures"] = json!([{
            "serverId": "other",
            "code": "timedOut",
            "message": "upstream timed out"
        }]);
        assert_eq!(
            McpFacadeCatalog::from_value(catalog.clone()).unwrap_err(),
            McpFacadeError::InvalidDocument
        );
        catalog["failures"][0]["serverId"] = json!("dbx");
        assert_eq!(
            McpFacadeCatalog::from_value(catalog).unwrap().failures[0].code,
            McpFacadeFailureCode::TimedOut
        );
    }

    #[test]
    fn parses_tool_calls_and_list_queries() {
        let call = McpFacadeToolCall::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-call.json"
        ))
        .unwrap();
        assert!(call.catalog_revision.is_none());
        assert_eq!(call.arguments, json!({"collection": "docs"}));
        assert!(format!("{call:?}").contains("<redacted>"));
        assert!(!format!("{call:?}").contains("docs"));

        let mut pinned = serde_json::to_value(&call).unwrap();
        pinned["catalogRevision"] = json!(18);
        assert_eq!(
            McpFacadeToolCall::from_value(pinned)
                .unwrap()
                .catalog_revision,
            Some(18)
        );
        let query = McpFacadeListQuery::from_value(json!({"query": "find", "limit": 5})).unwrap();
        assert_eq!(query.query.as_deref(), Some("find"));
        assert_eq!(query.limit, Some(5));
        assert!(!query.include_schema);
    }

    #[test]
    fn debug_redaction_survives_a_secret_argument() {
        let call = McpFacadeToolCall::from_value(json!({
            "serverId": "dbx",
            "toolName": "mongo_find_documents",
            "arguments": {"token": CANARY}
        }))
        .unwrap();
        let debug = format!("{call:?}");
        assert!(!debug.contains(CANARY));
        assert!(debug.contains("<redacted>"));
    }
}
