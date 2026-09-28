//! Untrusted description drafts and the approved metadata overlay.
//!
//! Draft safety opinions never become authority. The overlay stores reviewed
//! descriptions only and is a different document from the user MCP registry.

use std::{
    collections::BTreeSet,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::facade::McpFacadeCatalog;
use crate::acp::atomic_file;
use serde_json::Value;

#[cfg(test)]
use super::config::{ConfigSource, McpControlPlaneConfig};

pub const MCP_METADATA_SCHEMA_VERSION: u16 = 1;
pub const MCP_SERVER_ID_MAX_LENGTH: usize = 64;
pub const MCP_TOOL_NAME_MAX_LENGTH: usize = 128;
pub const MCP_TEXT_MAX_CHARS: usize = 4096;
pub const MCP_LIST_MAX: usize = 32;
pub const MCP_TOOLS_MAX: usize = 256;

const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpMetadataError {
    InvalidJson,
    ForbiddenCredentialField,
    InvalidId,
    InvalidDocument,
    StaleRevision,
    UnknownServer,
    UnknownTool,
    CatalogMismatch,
    PolicyViolation,
}

impl fmt::Display for McpMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJson => "MCP metadata JSON is invalid",
            Self::ForbiddenCredentialField => {
                "MCP metadata contract contains a forbidden credential field"
            }
            Self::InvalidId => "MCP metadata id is invalid",
            Self::InvalidDocument => "MCP metadata contract is invalid",
            Self::StaleRevision => "MCP metadata revision is stale",
            Self::UnknownServer => "MCP metadata references an unknown server",
            Self::UnknownTool => "MCP metadata references an unknown tool",
            Self::CatalogMismatch => "MCP metadata catalog revision is stale",
            Self::PolicyViolation => "MCP metadata draft violates the deterministic safety policy",
        })
    }
}

impl std::error::Error for McpMetadataError {}

#[derive(Debug)]
pub enum McpMetadataStoreError {
    Io(std::io::Error),
    Contract(McpMetadataError),
    StaleRevision,
}

impl fmt::Display for McpMetadataStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("MCP metadata storage I/O failed"),
            Self::Contract(error) => error.fmt(formatter),
            Self::StaleRevision => formatter.write_str("MCP metadata storage revision is stale"),
        }
    }
}

impl std::error::Error for McpMetadataStoreError {}

impl From<McpMetadataError> for McpMetadataStoreError {
    fn from(error: McpMetadataError) -> Self {
        Self::Contract(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpDraftTrust {
    Untrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpDraftPurpose {
    DescriptionAnalysis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAnalysisDraftTool {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub avoid_when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAnalysisDraft {
    pub schema_version: u16,
    pub revision: u64,
    pub trust: McpDraftTrust,
    pub purpose: McpDraftPurpose,
    pub server_id: String,
    pub catalog_revision: u64,
    pub channel_id: String,
    pub profile_id: String,
    pub description: String,
    pub when_to_use: Vec<String>,
    pub avoid_when: Vec<String>,
    pub tools: Vec<McpAnalysisDraftTool>,
}

impl McpAnalysisDraft {
    pub fn from_json(raw: &str) -> Result<Self, McpMetadataError> {
        let value = serde_json::from_str(raw).map_err(|_| McpMetadataError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, McpMetadataError> {
        inspect_closed(&value)?;
        let draft: Self =
            serde_json::from_value(value).map_err(|_| McpMetadataError::InvalidDocument)?;
        draft.validate()?;
        Ok(draft)
    }

    pub fn validate(&self) -> Result<(), McpMetadataError> {
        if self.schema_version != MCP_METADATA_SCHEMA_VERSION {
            return Err(McpMetadataError::InvalidDocument);
        }
        require_revision(self.revision)?;
        require_revision(self.catalog_revision)?;
        require_id(&self.server_id, MCP_SERVER_ID_MAX_LENGTH)?;
        require_id(&self.channel_id, MCP_SERVER_ID_MAX_LENGTH)?;
        require_id(&self.profile_id, MCP_SERVER_ID_MAX_LENGTH)?;
        require_prose(&self.description)?;
        require_prose_list(&self.when_to_use)?;
        require_prose_list(&self.avoid_when)?;
        if self.tools.len() > MCP_TOOLS_MAX {
            return Err(McpMetadataError::InvalidDocument);
        }
        let mut names = BTreeSet::new();
        for tool in &self.tools {
            if !names.insert(tool.name.clone()) {
                return Err(McpMetadataError::InvalidId);
            }
            validate_draft_tool(tool)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpMetadataTool {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub avoid_when: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpMetadataServer {
    pub server_id: String,
    pub description: String,
    pub when_to_use: Vec<String>,
    pub avoid_when: Vec<String>,
    pub tools: Vec<McpMetadataTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpMetadataDocument {
    pub schema_version: u16,
    pub revision: u64,
    pub servers: Vec<McpMetadataServer>,
}

impl McpMetadataDocument {
    pub fn approve_draft(
        &self,
        draft: &McpAnalysisDraft,
        catalog: &McpFacadeCatalog,
    ) -> Result<Self, McpMetadataError> {
        draft.validate()?;
        if draft.server_id != catalog.server_id
            || draft.catalog_revision != catalog.catalog_revision
        {
            return Err(McpMetadataError::CatalogMismatch);
        }
        let mut approved = self.clone();
        let server = if let Some(server) = approved
            .servers
            .iter_mut()
            .find(|server| server.server_id == draft.server_id)
        {
            server
        } else {
            approved.servers.push(McpMetadataServer {
                server_id: draft.server_id.clone(),
                description: String::new(),
                when_to_use: Vec::new(),
                avoid_when: Vec::new(),
                tools: Vec::new(),
            });
            approved
                .servers
                .last_mut()
                .ok_or(McpMetadataError::InvalidDocument)?
        };
        let catalog_tools = catalog
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect::<std::collections::BTreeMap<_, _>>();
        for draft_tool in &draft.tools {
            let Some(catalog_tool) = catalog_tools.get(draft_tool.name.as_str()) else {
                return Err(McpMetadataError::UnknownTool);
            };
            if (catalog_tool.destructive && draft_tool.destructive == Some(false))
                || (catalog_tool.confirmation_required
                    && draft_tool.confirmation_required == Some(false))
                || (catalog_tool.read_only && draft_tool.read_only == Some(false))
            {
                return Err(McpMetadataError::PolicyViolation);
            }
            let overlay = McpMetadataTool {
                name: draft_tool.name.clone(),
                description: draft_tool.description.clone(),
                when_to_use: draft_tool.when_to_use.clone(),
                avoid_when: draft_tool.avoid_when.clone(),
            };
            if let Some(existing) = server
                .tools
                .iter_mut()
                .find(|tool| tool.name == overlay.name)
            {
                *existing = overlay;
            } else {
                server.tools.push(overlay);
            }
        }
        server.description = draft.description.clone();
        server.when_to_use = draft.when_to_use.clone();
        server.avoid_when = draft.avoid_when.clone();
        approved.revision = approved.revision.saturating_add(1).max(1);
        approved.validate()?;
        Ok(approved)
    }

    pub fn empty() -> Self {
        Self {
            schema_version: MCP_METADATA_SCHEMA_VERSION,
            revision: 1,
            servers: Vec::new(),
        }
    }

    pub fn from_json(raw: &str) -> Result<Self, McpMetadataError> {
        let value = serde_json::from_str(raw).map_err(|_| McpMetadataError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, McpMetadataError> {
        inspect_closed(&value)?;
        let document: Self =
            serde_json::from_value(value).map_err(|_| McpMetadataError::InvalidDocument)?;
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), McpMetadataError> {
        if self.schema_version != MCP_METADATA_SCHEMA_VERSION || self.servers.len() > MCP_TOOLS_MAX
        {
            return Err(McpMetadataError::InvalidDocument);
        }
        require_revision(self.revision)?;
        let mut server_ids = BTreeSet::new();
        for server in &self.servers {
            if !server_ids.insert(server.server_id.clone()) {
                return Err(McpMetadataError::InvalidId);
            }
            validate_server(server)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct McpMetadataStore {
    path: PathBuf,
}

impl McpMetadataStore {
    pub fn for_project(project_root: impl AsRef<Path>) -> Self {
        Self {
            path: project_root
                .as_ref()
                .join(".se-manager")
                .join("mcp-metadata.json"),
        }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<McpMetadataDocument, McpMetadataStoreError> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => McpMetadataDocument::from_json(&raw).map_err(Into::into),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(McpMetadataDocument::empty())
            }
            Err(error) => Err(McpMetadataStoreError::Io(error)),
        }
    }

    pub fn save(
        &self,
        document: &McpMetadataDocument,
        expected_revision: u64,
    ) -> Result<(), McpMetadataStoreError> {
        document.validate()?;
        let current_revision = match fs::read_to_string(&self.path) {
            Ok(raw) => McpMetadataDocument::from_json(&raw)?.revision,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(McpMetadataStoreError::Io(error)),
        };
        if current_revision != expected_revision
            || document.revision != expected_revision.saturating_add(1)
        {
            return Err(McpMetadataStoreError::StaleRevision);
        }
        let bytes = serde_json::to_vec_pretty(document)
            .map_err(|_| McpMetadataStoreError::Contract(McpMetadataError::InvalidDocument))?;
        atomic_file::replace(&self.path, &bytes).map_err(McpMetadataStoreError::Io)
    }
}

pub fn validate_overlay(
    catalog: &McpFacadeCatalog,
    metadata: &McpMetadataDocument,
) -> Result<(), McpMetadataError> {
    metadata.validate()?;
    let Some(server) = metadata
        .servers
        .iter()
        .find(|server| server.server_id == catalog.server_id)
    else {
        return Err(McpMetadataError::UnknownServer);
    };
    if server.tools.iter().any(|metadata_tool| {
        !catalog
            .tools
            .iter()
            .any(|tool| tool.name == metadata_tool.name)
    }) {
        return Err(McpMetadataError::UnknownTool);
    }
    Ok(())
}

pub fn parse_analysis_output(
    raw: &str,
    catalog: &McpFacadeCatalog,
    channel_id: &str,
    profile_id: &str,
) -> Result<McpAnalysisDraft, McpMetadataError> {
    let draft = McpAnalysisDraft::from_json(raw)?;
    if draft.server_id != catalog.server_id
        || draft.catalog_revision != catalog.catalog_revision
        || draft.channel_id != channel_id
        || draft.profile_id != profile_id
    {
        return Err(McpMetadataError::CatalogMismatch);
    }
    if draft
        .tools
        .iter()
        .any(|tool| !catalog.tools.iter().any(|item| item.name == tool.name))
    {
        return Err(McpMetadataError::UnknownTool);
    }
    Ok(draft)
}

pub fn validate_overlay_at_revision(
    catalog: &McpFacadeCatalog,
    metadata: &McpMetadataDocument,
    expected_catalog_revision: u64,
) -> Result<(), McpMetadataError> {
    if expected_catalog_revision != catalog.catalog_revision {
        return Err(McpMetadataError::CatalogMismatch);
    }
    validate_overlay(catalog, metadata)
}

pub fn apply_overlay(
    mut catalog: McpFacadeCatalog,
    metadata: &McpMetadataDocument,
) -> Result<McpFacadeCatalog, McpMetadataError> {
    validate_overlay(&catalog, metadata)?;
    let server = &metadata.servers[0];
    catalog.description = server.description.clone();
    catalog.when_to_use = server.when_to_use.clone();
    catalog.avoid_when = server.avoid_when.clone();
    for tool in &mut catalog.tools {
        if let Some(overlay) = server.tools.iter().find(|item| item.name == tool.name) {
            tool.description = overlay.description.clone();
            tool.when_to_use = overlay.when_to_use.clone();
            tool.avoid_when = overlay.avoid_when.clone();
        }
    }
    Ok(catalog)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataRevisionFence {
    last_accepted: Option<u64>,
}

impl MetadataRevisionFence {
    pub fn last_accepted(&self) -> Option<u64> {
        self.last_accepted
    }

    pub fn accept(&mut self, revision: u64) -> Result<(), McpMetadataError> {
        if revision == 0 || revision > MAX_SAFE_INTEGER {
            return Err(McpMetadataError::InvalidDocument);
        }
        if self.last_accepted.is_some_and(|last| revision <= last) {
            return Err(McpMetadataError::StaleRevision);
        }
        self.last_accepted = Some(revision);
        Ok(())
    }
}

fn validate_draft_tool(tool: &McpAnalysisDraftTool) -> Result<(), McpMetadataError> {
    require_id(&tool.name, MCP_TOOL_NAME_MAX_LENGTH)?;
    require_prose(&tool.description)?;
    require_prose(&tool.when_to_use)?;
    require_prose(&tool.avoid_when)?;
    if let Some(confidence) = tool.confidence {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(McpMetadataError::InvalidDocument);
        }
    }
    Ok(())
}

fn validate_server(server: &McpMetadataServer) -> Result<(), McpMetadataError> {
    require_id(&server.server_id, MCP_SERVER_ID_MAX_LENGTH)?;
    require_prose(&server.description)?;
    require_prose_list(&server.when_to_use)?;
    require_prose_list(&server.avoid_when)?;
    if server.tools.len() > MCP_TOOLS_MAX {
        return Err(McpMetadataError::InvalidDocument);
    }
    let mut names = BTreeSet::new();
    for tool in &server.tools {
        if !names.insert(tool.name.clone()) {
            return Err(McpMetadataError::InvalidId);
        }
        require_id(&tool.name, MCP_TOOL_NAME_MAX_LENGTH)?;
        require_prose(&tool.description)?;
        require_prose(&tool.when_to_use)?;
        require_prose(&tool.avoid_when)?;
    }
    Ok(())
}

fn require_id(value: &str, max_length: usize) -> Result<(), McpMetadataError> {
    if is_safe_id(value, max_length) {
        Ok(())
    } else {
        Err(McpMetadataError::InvalidId)
    }
}

fn require_revision(revision: u64) -> Result<(), McpMetadataError> {
    if revision == 0 || revision > MAX_SAFE_INTEGER {
        Err(McpMetadataError::InvalidDocument)
    } else {
        Ok(())
    }
}

fn require_prose(value: &str) -> Result<(), McpMetadataError> {
    if is_prose(value, MCP_TEXT_MAX_CHARS, true, true) {
        Ok(())
    } else {
        Err(McpMetadataError::InvalidDocument)
    }
}

fn require_prose_list(values: &[String]) -> Result<(), McpMetadataError> {
    if values.len() > MCP_LIST_MAX {
        return Err(McpMetadataError::InvalidDocument);
    }
    values.iter().try_for_each(|value| require_prose(value))
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

fn inspect_closed(value: &Value) -> Result<(), McpMetadataError> {
    inspect_closed_at(value, 0)
}

fn inspect_closed_at(value: &Value, depth: usize) -> Result<(), McpMetadataError> {
    if depth > 32 {
        return Err(McpMetadataError::InvalidDocument);
    }
    match value {
        Value::Null => Err(McpMetadataError::InvalidDocument),
        Value::String(text) if text.len() > 100_000 => Err(McpMetadataError::InvalidDocument),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Ok(()),
        Value::Array(items) => {
            if items.len() > 10_000 {
                return Err(McpMetadataError::InvalidDocument);
            }
            items
                .iter()
                .try_for_each(|item| inspect_closed_at(item, depth + 1))
        }
        Value::Object(map) => {
            if map.len() > 256 {
                return Err(McpMetadataError::InvalidDocument);
            }
            for (key, child) in map {
                if is_forbidden_credential_key(key) {
                    return Err(McpMetadataError::ForbiddenCredentialField);
                }
                inspect_closed_at(child, depth + 1)?;
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

    #[test]
    fn round_trips_untrusted_drafts_and_rejects_approved_trust() {
        let draft = McpAnalysisDraft::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-analysis-draft.json"
        ))
        .unwrap();
        assert_eq!(draft.trust, McpDraftTrust::Untrusted);
        assert_eq!(draft.tools[0].confidence, Some(0.25));
        let encoded = serde_json::to_value(&draft).unwrap();
        assert_eq!(McpAnalysisDraft::from_value(encoded).unwrap(), draft);

        let mut approved = serde_json::to_value(&draft).unwrap();
        approved["trust"] = json!("approved");
        assert_eq!(
            McpAnalysisDraft::from_value(approved).unwrap_err(),
            McpMetadataError::InvalidDocument
        );
    }

    #[test]
    fn metadata_overlay_rejects_safety_authority_and_fences_revisions() {
        let document = McpMetadataDocument::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-metadata.document.json"
        ))
        .unwrap();
        assert_eq!(document.servers[0].tools[0].name, "mongo_find_documents");
        let mut unsafe_overlay = serde_json::to_value(&document).unwrap();
        unsafe_overlay["servers"][0]["tools"][0]["destructive"] = json!(false);
        assert_eq!(
            McpMetadataDocument::from_value(unsafe_overlay).unwrap_err(),
            McpMetadataError::InvalidDocument
        );

        let mut fence = MetadataRevisionFence::default();
        assert!(fence.accept(document.revision).is_ok());
        assert_eq!(
            fence.accept(document.revision).unwrap_err(),
            McpMetadataError::StaleRevision
        );
    }

    #[test]
    fn metadata_store_writes_atomically_and_fences_revisions() {
        let root = std::env::temp_dir().join(format!(
            "se-manager-metadata-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let registry = root.join("mcp-servers.json");
        std::fs::write(&registry, "registry-before").unwrap();
        let store = McpMetadataStore::for_project(&root);
        let mut document = McpMetadataDocument::empty();
        document.revision = 1;
        store.save(&document, 0).unwrap();
        assert_eq!(store.load().unwrap(), document);
        document.revision = 2;
        assert!(matches!(
            store.save(&document, 0),
            Err(McpMetadataStoreError::StaleRevision)
        ));
        assert_eq!(
            std::fs::read_to_string(&registry).unwrap(),
            "registry-before"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn strict_analysis_output_is_untrusted_until_explicit_approval() {
        let catalog = McpFacadeCatalog::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        let raw = include_str!("../../../src/shared/types/fixtures/mcp-analysis-draft.json");
        let draft = parse_analysis_output(raw, &catalog, "gateway", "profileA").unwrap();
        assert_eq!(draft.trust, McpDraftTrust::Untrusted);
        assert_eq!(
            parse_analysis_output(raw, &catalog, "wrong", "profileA").unwrap_err(),
            McpMetadataError::CatalogMismatch
        );
    }

    #[test]
    fn approval_requires_live_catalog_and_never_copies_untrusted_safety_flags() {
        let catalog = McpFacadeCatalog::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        let draft = McpAnalysisDraft::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-analysis-draft.json"
        ))
        .unwrap();
        let original = McpMetadataDocument::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-metadata.document.json"
        ))
        .unwrap();
        let approved = original.approve_draft(&draft, &catalog).unwrap();
        assert_eq!(approved.revision, original.revision + 1);
        assert_eq!(approved.servers[0].tools[0].name, "mongo_find_documents");
        assert_eq!(approved.servers[0].tools[0].description, "Find documents");
        let mut stale = draft.clone();
        stale.catalog_revision += 1;
        assert_eq!(
            original.approve_draft(&stale, &catalog).unwrap_err(),
            McpMetadataError::CatalogMismatch
        );
        assert_eq!(
            original,
            McpMetadataDocument::from_json(include_str!(
                "../../../src/shared/types/fixtures/mcp-metadata.document.json"
            ))
            .unwrap()
        );
    }

    #[test]
    fn metadata_overlay_rejects_unknown_tools_and_stale_catalogs() {
        let catalog = McpFacadeCatalog::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-facade-catalog.json"
        ))
        .unwrap();
        let document = McpMetadataDocument::from_json(include_str!(
            "../../../src/shared/types/fixtures/mcp-metadata.document.json"
        ))
        .unwrap();
        assert!(validate_overlay(&catalog, &document).is_ok());
        assert_eq!(
            validate_overlay_at_revision(&catalog, &document, catalog.catalog_revision + 1)
                .unwrap_err(),
            McpMetadataError::CatalogMismatch
        );
        let mut unknown = document.clone();
        unknown.servers[0].tools[0].name = "not_in_catalog".into();
        assert_eq!(
            validate_overlay(&catalog, &unknown).unwrap_err(),
            McpMetadataError::UnknownTool
        );
    }

    #[test]
    fn legacy_mcp_registry_still_deserializes() {
        let legacy = json!([
            {
                "id": "local",
                "type": "stdio",
                "name": "Files",
                "command": "npx",
                "args": ["--stdio"],
                "enabled": true
            },
            {
                "id": "remote",
                "type": "http",
                "name": "Remote",
                "url": "http://127.0.0.1:43123/mcp",
                "enabled": false
            }
        ]);
        let parsed = McpControlPlaneConfig::from_stored_json(&legacy).unwrap();
        assert_eq!(parsed.source, ConfigSource::LegacyArray);
        assert_eq!(parsed.config.upstreams[0].id, "local");
        assert!(!parsed.config.upstreams[1].enabled);

        let canonical = json!({
            "schemaVersion": 1,
            "revision": 2,
            "builtIns": [],
            "upstreams": [{
                "id": "dbx",
                "name": "DBX",
                "type": "http",
                "url": "https://example.test/mcp"
            }],
            "routing": {"nameCollision": "prefixServerId"}
        });
        let canonical = McpControlPlaneConfig::from_stored_json(&canonical).unwrap();
        assert_eq!(canonical.source, ConfigSource::Canonical);
        assert_eq!(canonical.config.revision, 2);
        assert_eq!(canonical.config.upstreams[0].id, "dbx");
    }
}
