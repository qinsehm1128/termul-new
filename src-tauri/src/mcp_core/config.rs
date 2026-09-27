//! Canonical project-scoped MCP control-plane configuration.
//!
//! This is the single persisted document for a project's MCP settings. The
//! Core snapshot remains an internal runtime projection and is never written
//! back as a second authority. Built-in capability *providers* live in
//! [`crate::mcp_core::builtins`]; this module only stores their ids, enabled
//! flags, and policy. Providers must not introduce a second persistence path.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::oauth::{sanitize_mcp_oauth_metadata, McpOAuthConfig};

pub const MCP_CONTROL_PLANE_SCHEMA_VERSION: u16 = 1;
pub const BUILTIN_SESSION_MEMORY: &str = "session-memory";
pub const BUILTIN_PROJECT_SCOPE: &str = "project-scope";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Invalid(String),
    UnsupportedSchema { found: u16, expected: u16 },
    RevisionConflict { accepted: u64, requested: u64 },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "{message}"),
            Self::UnsupportedSchema { found, expected } => {
                write!(
                    f,
                    "unsupported MCP control-plane schema version {found} (expected {expected})"
                )
            }
            Self::RevisionConflict {
                accepted,
                requested,
            } => write!(
                f,
                "MCP config revision {requested} is not newer than {accepted}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    LegacyArray,
    Canonical,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpControlPlaneConfig {
    pub schema_version: u16,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub built_ins: Vec<McpBuiltInConfig>,
    #[serde(default)]
    pub upstreams: Vec<McpUpstreamConfig>,
    #[serde(default)]
    pub routing: McpRoutingConfig,
}

impl fmt::Debug for McpControlPlaneConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpControlPlaneConfig")
            .field("schema_version", &self.schema_version)
            .field("revision", &self.revision)
            .field("built_ins", &self.built_ins)
            .field("upstreams", &self.upstreams)
            .field("routing", &self.routing)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBuiltInConfig {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "McpCapabilityPolicy::is_empty")]
    pub policy: McpCapabilityPolicy,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpUpstreamConfig {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub transport: McpPersistedTransport,
    #[serde(default, skip_serializing_if = "McpCapabilityPolicy::is_empty")]
    pub policy: McpCapabilityPolicy,
}

impl fmt::Debug for McpUpstreamConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpUpstreamConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("transport", &self.transport)
            .field("policy", &self.policy)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpPersistedTransport {
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<NamedSecret>,
    },
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<NamedSecret>,
        /// Credential-free OAuth metadata. Tokens stay in the keyring.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthConfig>,
    },
    Sse {
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<NamedSecret>,
        /// Credential-free OAuth metadata. Tokens stay in the keyring.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthConfig>,
    },
}

impl fmt::Debug for McpPersistedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", env)
                .finish(),
            Self::Http {
                url,
                headers,
                oauth,
            } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", headers)
                .field("oauth", oauth)
                .finish(),
            Self::Sse {
                url,
                headers,
                oauth,
            } => f
                .debug_struct("Sse")
                .field("url", url)
                .field("headers", headers)
                .field("oauth", oauth)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilityPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_tools: Vec<String>,
}

impl fmt::Debug for McpCapabilityPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpCapabilityPolicy")
            .field("allow_tools", &self.allow_tools)
            .field("deny_tools", &self.deny_tools)
            .finish()
    }
}

impl McpCapabilityPolicy {
    pub fn is_empty(&self) -> bool {
        self.allow_tools.is_none() && self.deny_tools.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NameCollisionPolicy {
    #[default]
    PrefixServerId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpRoutingConfig {
    #[serde(default)]
    pub name_collision: NameCollisionPolicy,
}

/// A persisted env/header value. Inline secrets remain for compatibility with
/// existing project files; the canonical form prefers `ref` and never prints
/// inline values in Debug/error output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedSecret {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl fmt::Debug for NamedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamedSecret")
            .field("name", &self.name)
            .field("value", &self.value.as_ref().map(|_| "<redacted>"))
            .field("ref", &self.reference)
            .finish()
    }
}

impl NamedSecret {
    pub fn inline(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
            reference: None,
        }
    }

    pub fn by_ref(name: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            reference: Some(reference.into()),
        }
    }

    /// Value handed to [`crate::mcp_core::snapshot::McpSecretResolver`].
    /// References pass the ref key, never an inline secret.
    pub fn resolver_token(&self) -> Result<&str, ConfigError> {
        if let Some(reference) = self.reference.as_deref().filter(|value| !value.is_empty()) {
            return Ok(reference);
        }
        self.value
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "secret {} must have a non-empty value or ref",
                    self.name
                ))
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedControlPlane {
    pub config: McpControlPlaneConfig,
    pub source: ConfigSource,
}

fn default_true() -> bool {
    true
}

pub fn default_built_ins() -> Vec<McpBuiltInConfig> {
    vec![
        McpBuiltInConfig {
            id: BUILTIN_SESSION_MEMORY.into(),
            enabled: true,
            policy: McpCapabilityPolicy::default(),
        },
        McpBuiltInConfig {
            id: BUILTIN_PROJECT_SCOPE.into(),
            enabled: true,
            policy: McpCapabilityPolicy::default(),
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpControlPlaneStatus {
    pub schema_version: u16,
    pub revision: u64,
    pub built_ins: Vec<McpBuiltInStatus>,
    pub upstreams: Vec<McpUpstreamStatus>,
    pub routing: McpRoutingConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBuiltInStatus {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpUpstreamStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "type")]
    pub kind: String,
}

impl McpControlPlaneConfig {
    pub fn empty() -> Self {
        Self {
            schema_version: MCP_CONTROL_PLANE_SCHEMA_VERSION,
            revision: 0,
            built_ins: default_built_ins(),
            upstreams: Vec::new(),
            routing: McpRoutingConfig::default(),
        }
    }

    /// Redacted operator status: ids, names, enablement, and transport kind only.
    pub fn to_status(&self) -> McpControlPlaneStatus {
        McpControlPlaneStatus {
            schema_version: self.schema_version,
            revision: self.revision,
            built_ins: self
                .built_ins
                .iter()
                .map(|item| McpBuiltInStatus {
                    id: item.id.clone(),
                    enabled: item.enabled,
                })
                .collect(),
            upstreams: self
                .upstreams
                .iter()
                .map(|item| McpUpstreamStatus {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    enabled: item.enabled,
                    kind: match item.transport {
                        McpPersistedTransport::Stdio { .. } => "stdio".into(),
                        McpPersistedTransport::Http { .. } => "http".into(),
                        McpPersistedTransport::Sse { .. } => "sse".into(),
                    },
                })
                .collect(),
            routing: self.routing.clone(),
        }
    }

    pub fn from_stored_json(value: &Value) -> Result<ParsedControlPlane, ConfigError> {
        let mut owned = value.clone();
        sanitize_mcp_oauth_metadata(&mut owned)
            .map_err(|error| ConfigError::Invalid(error.to_string()))?;
        if let Some(entries) = owned.as_array() {
            let config = migrate_legacy_array(entries)?;
            return Ok(ParsedControlPlane {
                config,
                source: ConfigSource::LegacyArray,
            });
        }
        if owned.as_object().is_some_and(is_canonical_object) {
            merge_bearer_tokens_into_headers(&mut owned)?;
            let mut config: McpControlPlaneConfig =
                serde_json::from_value(owned).map_err(|error| {
                    ConfigError::Invalid(format!("invalid MCP control-plane document: {error}"))
                })?;
            if config.schema_version == 0 {
                config.schema_version = MCP_CONTROL_PLANE_SCHEMA_VERSION;
            }
            if config.schema_version != MCP_CONTROL_PLANE_SCHEMA_VERSION {
                return Err(ConfigError::UnsupportedSchema {
                    found: config.schema_version,
                    expected: MCP_CONTROL_PLANE_SCHEMA_VERSION,
                });
            }
            config.normalize()?;
            Ok(ParsedControlPlane {
                config,
                source: ConfigSource::Canonical,
            })
        } else {
            Err(ConfigError::Invalid(
                "MCP registry must be a JSON array or control-plane object".into(),
            ))
        }
    }

    pub fn to_canonical_json(&self) -> Result<Value, ConfigError> {
        serde_json::to_value(self)
            .map_err(|error| ConfigError::Invalid(format!("cannot serialize MCP config: {error}")))
    }

    pub fn with_filled_defaults(mut self) -> Self {
        fill_default_built_ins(&mut self.built_ins);
        self
    }

    fn normalize(&mut self) -> Result<(), ConfigError> {
        fill_default_built_ins(&mut self.built_ins);
        validate_unique_ids(
            self.built_ins.iter().map(|item| item.id.as_str()),
            "built-in",
        )?;
        validate_unique_ids(
            self.upstreams.iter().map(|item| item.id.as_str()),
            "upstream",
        )?;
        for upstream in &mut self.upstreams {
            if upstream.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "upstream server id must not be empty".into(),
                ));
            }
            match &mut upstream.transport {
                McpPersistedTransport::Stdio { command, env, .. } => {
                    if command.trim().is_empty() {
                        return Err(ConfigError::Invalid(format!(
                            "stdio command for upstream {} must not be empty",
                            upstream.id
                        )));
                    }
                    normalize_secrets(env)?;
                }
                McpPersistedTransport::Http {
                    url,
                    headers,
                    oauth,
                }
                | McpPersistedTransport::Sse {
                    url,
                    headers,
                    oauth,
                } => {
                    if url.trim().is_empty() {
                        return Err(ConfigError::Invalid(format!(
                            "url for upstream {} must not be empty",
                            upstream.id
                        )));
                    }
                    normalize_secrets(headers)?;
                    if let Some(oauth_config) = oauth.as_mut() {
                        oauth_config
                            .normalize()
                            .map_err(|error| ConfigError::Invalid(error.to_string()))?;
                    }
                    if oauth.as_ref().is_some_and(McpOAuthConfig::is_blank) {
                        *oauth = None;
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_canonical_object(object: &Map<String, Value>) -> bool {
    object.contains_key("schemaVersion")
        || object.contains_key("upstreams")
        || object.contains_key("builtIns")
}

fn fill_default_built_ins(built_ins: &mut Vec<McpBuiltInConfig>) {
    for default in default_built_ins() {
        if !built_ins.iter().any(|item| item.id == default.id) {
            built_ins.push(default);
        }
    }
}

fn validate_unique_ids<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    kind: &str,
) -> Result<(), ConfigError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(ConfigError::Invalid(format!("duplicate {kind} id {id}")));
        }
    }
    Ok(())
}

fn normalize_secrets(secrets: &mut [NamedSecret]) -> Result<(), ConfigError> {
    let mut names = BTreeSet::new();
    for secret in secrets.iter_mut() {
        if secret.name.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "secret name must be a non-empty string".into(),
            ));
        }
        if !names.insert(secret.name.clone()) {
            return Err(ConfigError::Invalid(format!(
                "duplicate secret name {}",
                secret.name
            )));
        }
        if secret
            .reference
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            secret.value = None;
        }
        secret.resolver_token()?;
    }
    Ok(())
}

fn migrate_legacy_array(entries: &[Value]) -> Result<McpControlPlaneConfig, ConfigError> {
    let mut used_ids = BTreeSet::new();
    let mut upstreams = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        upstreams.push(migrate_legacy_entry(entry, index, &mut used_ids)?);
    }
    let mut config = McpControlPlaneConfig {
        schema_version: MCP_CONTROL_PLANE_SCHEMA_VERSION,
        revision: 0,
        built_ins: default_built_ins(),
        upstreams,
        routing: McpRoutingConfig::default(),
    };
    config.normalize()?;
    Ok(config)
}

fn migrate_legacy_entry(
    entry: &Value,
    index: usize,
    used_ids: &mut BTreeSet<String>,
) -> Result<McpUpstreamConfig, ConfigError> {
    let object = entry
        .as_object()
        .ok_or_else(|| ConfigError::Invalid("each registry entry must be an object".into()))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_owned();
    let requested_id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| (!name.is_empty()).then(|| name.clone()))
        .unwrap_or_else(|| format!("upstream-{index}"));
    let id = unique_id(requested_id, used_ids);
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    let transport = match kind {
        "stdio" => McpPersistedTransport::Stdio {
            command: required_string(object, "command")?.to_owned(),
            args: string_array(object.get("args"), "args")?,
            env: parse_named_secrets(object.get("env"), "env")?,
        },
        "http" => McpPersistedTransport::Http {
            url: required_string(object, "url")?.to_owned(),
            headers: parse_http_headers(object)?,
            oauth: parse_oauth_config(object)?,
        },
        "sse" => McpPersistedTransport::Sse {
            url: required_string(object, "url")?.to_owned(),
            headers: parse_http_headers(object)?,
            oauth: parse_oauth_config(object)?,
        },
        other => {
            return Err(ConfigError::Invalid(format!(
                "unsupported upstream type {other}"
            )));
        }
    };
    let display_name = if name.is_empty() { id.clone() } else { name };
    Ok(McpUpstreamConfig {
        id,
        name: display_name,
        enabled,
        transport,
        policy: McpCapabilityPolicy::default(),
    })
}

fn unique_id(requested: String, used_ids: &mut BTreeSet<String>) -> String {
    if used_ids.insert(requested.clone()) {
        return requested;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{requested}-{suffix}");
        if used_ids.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, ConfigError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ConfigError::Invalid(format!("{field} must be a non-empty string")))
}

fn string_array(value: Option<&Value>, field: &str) -> Result<Vec<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| ConfigError::Invalid(format!("{field} must be an array")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ConfigError::Invalid(format!("{field} entries must be strings")))
        })
        .collect()
}

fn parse_oauth_config(object: &Map<String, Value>) -> Result<Option<McpOAuthConfig>, ConfigError> {
    let Some(value) = object.get("oauth") else {
        return Ok(None);
    };
    let mut config: McpOAuthConfig = serde_json::from_value(value.clone())
        .map_err(|_| ConfigError::Invalid("oauth metadata is invalid".into()))?;
    config
        .normalize()
        .map_err(|error| ConfigError::Invalid(error.to_string()))?;
    if config.is_blank() {
        Ok(None)
    } else {
        Ok(Some(config))
    }
}

fn parse_http_headers(object: &Map<String, Value>) -> Result<Vec<NamedSecret>, ConfigError> {
    let mut headers = parse_named_secrets(object.get("headers"), "headers")?;
    let bearer_token = object
        .get("bearerToken")
        .or_else(|| object.get("bearer_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(token) = bearer_token {
        if !headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"))
        {
            headers.push(NamedSecret::inline(
                "Authorization",
                format!("Bearer {token}"),
            ));
        }
    }
    Ok(headers)
}

fn merge_bearer_tokens_into_headers(value: &mut Value) -> Result<(), ConfigError> {
    let Some(upstreams) = value
        .as_object_mut()
        .and_then(|object| object.get_mut("upstreams"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for upstream in upstreams {
        let Some(object) = upstream.as_object_mut() else {
            continue;
        };
        let bearer_token = object
            .get("bearerToken")
            .or_else(|| object.get("bearer_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let Some(token) = bearer_token else {
            continue;
        };
        let headers = object
            .entry("headers")
            .or_insert_with(|| Value::Object(Map::new()));
        match headers {
            Value::Object(header_map)
                if !header_map
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case("authorization")) =>
            {
                header_map.insert(
                    "Authorization".into(),
                    Value::String(format!("Bearer {token}")),
                );
            }
            Value::Array(header_list) => {
                let has_authorization = header_list.iter().any(|header| {
                    header
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case("authorization"))
                });
                if !has_authorization {
                    header_list.push(serde_json::json!({
                        "name": "Authorization",
                        "value": format!("Bearer {token}")
                    }));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_named_secrets(
    value: Option<&Value>,
    field: &str,
) -> Result<Vec<NamedSecret>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if let Some(entries) = value.as_array() {
        entries
            .iter()
            .map(|entry| {
                let object = entry.as_object().ok_or_else(|| {
                    ConfigError::Invalid(format!("{field} entries must be objects"))
                })?;
                let name = required_string(object, "name")?.to_owned();
                let reference = object
                    .get("ref")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                let value = object
                    .get("value")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                if reference.is_none() && value.is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "{field} entry {name} must have a value or ref"
                    )));
                }
                Ok(NamedSecret {
                    name,
                    value: if reference.is_some() { None } else { value },
                    reference,
                })
            })
            .collect()
    } else if let Some(entries) = value.as_object() {
        entries
            .iter()
            .map(|(name, value)| {
                let value = value.as_str().ok_or_else(|| {
                    ConfigError::Invalid(format!("{field} values must be strings"))
                })?;
                if name.trim().is_empty() || value.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "{field} names and values must be non-empty strings"
                    )));
                }
                Ok(NamedSecret::inline(name, value))
            })
            .collect()
    } else {
        Err(ConfigError::Invalid(format!(
            "{field} must be an array or object"
        )))
    }
}

/// Prepare a document for a canonical project-file write.
///
/// Legacy array payloads keep existing built-ins/routing and bump revision so
/// the current settings UI can keep sending an array. Canonical object
/// payloads are a full replace and reject stale revisions.
pub fn prepare_write(
    incoming: &Value,
    existing: Option<&McpControlPlaneConfig>,
) -> Result<McpControlPlaneConfig, ConfigError> {
    let parsed = McpControlPlaneConfig::from_stored_json(incoming)?;
    match parsed.source {
        ConfigSource::LegacyArray => {
            let mut config = parsed.config.with_filled_defaults();
            if let Some(existing) = existing {
                config.built_ins = existing.built_ins.clone();
                config.routing = existing.routing.clone();
                config.revision = existing.revision.saturating_add(1).max(1);
            } else {
                config.revision = 1;
            }
            Ok(config)
        }
        ConfigSource::Canonical => {
            let mut config = parsed.config.with_filled_defaults();
            if config.revision == 0 {
                config.revision = existing
                    .map(|item| item.revision.saturating_add(1).max(1))
                    .unwrap_or(1);
            } else if let Some(existing) = existing {
                if config.revision <= existing.revision {
                    return Err(ConfigError::RevisionConflict {
                        accepted: existing.revision,
                        requested: config.revision,
                    });
                }
            }
            Ok(config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn legacy_array() -> Value {
        json!([
            {
                "id": "local",
                "type": "stdio",
                "name": "Files",
                "command": "npx",
                "args": ["--stdio"],
                "env": [{"name": "TOKEN", "value": "secret-token"}],
                "enabled": true
            },
            {
                "id": "remote",
                "type": "http",
                "name": "Remote",
                "url": "http://127.0.0.1:43123/mcp",
                "headers": {"Authorization": "Bearer secret"},
                "enabled": false
            }
        ])
    }

    #[test]
    fn migrates_legacy_array_and_is_idempotent() {
        let first = McpControlPlaneConfig::from_stored_json(&legacy_array())
            .unwrap()
            .config;
        assert_eq!(first.schema_version, MCP_CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(first.revision, 0);
        assert_eq!(first.upstreams.len(), 2);
        assert_eq!(first.upstreams[0].name, "Files");
        assert!(!first.upstreams[1].enabled);
        assert!(first
            .built_ins
            .iter()
            .any(|item| item.id == BUILTIN_SESSION_MEMORY && item.enabled));

        let encoded = first.to_canonical_json().unwrap();
        let second = McpControlPlaneConfig::from_stored_json(&encoded)
            .unwrap()
            .config;
        assert_eq!(first, second);
        assert_eq!(
            McpControlPlaneConfig::from_stored_json(&second.to_canonical_json().unwrap())
                .unwrap()
                .config,
            second
        );
    }

    #[test]
    fn reads_bearer_token_into_plain_http_headers() {
        let parsed = McpControlPlaneConfig::from_stored_json(&json!([
            {
                "id": "legacy",
                "type": "http",
                "name": "Legacy",
                "url": "https://legacy.test/mcp",
                "bearerToken": "legacy-token"
            }
        ]))
        .unwrap()
        .config;
        let McpPersistedTransport::Http { headers, .. } = &parsed.upstreams[0].transport else {
            panic!("expected HTTP transport");
        };
        assert_eq!(headers[0].name, "Authorization");
        assert_eq!(headers[0].value.as_deref(), Some("Bearer legacy-token"));

        let canonical = json!({
            "schemaVersion": 1,
            "revision": 1,
            "builtIns": [],
            "upstreams": [{
                "id": "canonical",
                "type": "http",
                "name": "Canonical",
                "url": "https://canonical.test/mcp",
                "bearerToken": "canonical-token",
                "headers": []
            }],
            "routing": {"nameCollision": "prefixServerId"}
        });
        let canonical = McpControlPlaneConfig::from_stored_json(&canonical)
            .unwrap()
            .config;
        let McpPersistedTransport::Http { headers, .. } = &canonical.upstreams[0].transport else {
            panic!("expected HTTP transport");
        };
        assert_eq!(headers[0].value.as_deref(), Some("Bearer canonical-token"));
    }

    #[test]
    fn generates_stable_ids_for_legacy_entries_without_id() {
        let parsed = McpControlPlaneConfig::from_stored_json(&json!([
            { "name": "filesystem", "command": "npx" },
            { "name": "filesystem", "command": "npx" }
        ]))
        .unwrap()
        .config;
        assert_eq!(parsed.upstreams[0].id, "filesystem");
        assert_eq!(parsed.upstreams[1].id, "filesystem-2");
    }

    #[test]
    fn rejects_invalid_documents() {
        assert!(McpControlPlaneConfig::from_stored_json(&json!({})).is_err());
        assert!(McpControlPlaneConfig::from_stored_json(&json!("nope")).is_err());
        assert!(McpControlPlaneConfig::from_stored_json(&json!([
            { "id": "x", "type": "stdio" }
        ]))
        .is_err());
        let error = McpControlPlaneConfig::from_stored_json(&json!({
            "schemaVersion": 99,
            "upstreams": []
        }))
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::UnsupportedSchema {
                found: 99,
                expected: 1
            }
        ));
    }

    #[test]
    fn debug_redacts_inline_secret_values() {
        let config = McpControlPlaneConfig::from_stored_json(&legacy_array())
            .unwrap()
            .config;
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-token"));
        assert!(!debug.contains("Bearer secret"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("TOKEN") || debug.contains("Authorization"));
    }

    #[test]
    fn secret_references_are_preferred_over_inline_values() {
        let parsed = McpControlPlaneConfig::from_stored_json(&json!({
            "schemaVersion": 1,
            "revision": 3,
            "upstreams": [{
                "id": "remote",
                "name": "Remote",
                "type": "http",
                "url": "https://example.test/mcp",
                "headers": [{
                    "name": "Authorization",
                    "value": "Bearer leaked",
                    "ref": "mcp/remote/authorization"
                }]
            }]
        }))
        .unwrap()
        .config;
        match &parsed.upstreams[0].transport {
            McpPersistedTransport::Http { headers, .. } => {
                assert_eq!(
                    headers[0].reference.as_deref(),
                    Some("mcp/remote/authorization")
                );
                assert_eq!(headers[0].value, None);
                assert_eq!(
                    headers[0].resolver_token().unwrap(),
                    "mcp/remote/authorization"
                );
            }
            other => panic!("expected http upstream, got {other:?}"),
        }
        assert!(!format!("{parsed:?}").contains("Bearer leaked"));
        let status = parsed.to_status();
        let encoded = serde_json::to_string(&status).unwrap();
        assert!(!encoded.contains("Bearer leaked"));
        assert!(!encoded.contains("mcp/remote/authorization"));
        assert_eq!(status.upstreams[0].kind, "http");
        assert_eq!(status.upstreams[0].name, "Remote");
    }

    #[test]
    fn prepare_write_bumps_legacy_array_and_rejects_stale_canonical_revision() {
        let first = prepare_write(&legacy_array(), None).unwrap();
        assert_eq!(first.revision, 1);
        let second = prepare_write(&legacy_array(), Some(&first)).unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.built_ins, first.built_ins);

        let mut stale = second.clone();
        stale.revision = 2;
        let error = prepare_write(&stale.to_canonical_json().unwrap(), Some(&second)).unwrap_err();
        assert!(error.to_string().contains("not newer than 2"));

        let mut newer = second.clone();
        newer.revision = 3;
        let accepted = prepare_write(&newer.to_canonical_json().unwrap(), Some(&second)).unwrap();
        assert_eq!(accepted.revision, 3);
    }

    #[test]
    fn oauth_metadata_round_trips_without_credential_fields() {
        for transport in ["http", "sse"] {
            let parsed = McpControlPlaneConfig::from_stored_json(&json!({
                "schemaVersion": 1,
                "revision": 2,
                "upstreams": [{
                    "id": "remote",
                    "name": "Remote",
                    "type": transport,
                    "url": "https://example.test/mcp",
                    "oauth": {
                        "authMode": "oauth",
                        "registrationMode": "clientMetadata",
                        "clientId": " https://client.example/oauth.json ",
                        "clientMetadataUrl": "https://client.example/oauth.json",
                        "scopes": ["mcp", "mcp", "offline_access"],
                        "endpoints": {
                            "protectedResourceMetadataUrl": "https://example.test/.well-known/oauth-protected-resource",
                            "authorizationEndpoint": "https://auth.example/authorize",
                            "tokenEndpoint": "https://auth.example/token"
                        },
                        "discoveredAt": 1700000000,
                        "accessToken": "access-token-canary",
                        "refreshToken": "refresh-token-canary",
                        "clientSecret": "client-secret-canary"
                    }
                }]
            }))
            .unwrap()
            .config;
            let encoded = parsed.to_canonical_json().unwrap();
            let text = encoded.to_string();
            assert!(!text.contains("access-token-canary"), "{transport}");
            assert!(!text.contains("refresh-token-canary"), "{transport}");
            assert!(!text.contains("client-secret-canary"), "{transport}");
            assert!(!text.contains("accessToken"), "{transport}");
            assert!(!text.contains("clientSecret"), "{transport}");
            let oauth = encoded["upstreams"][0]["oauth"].clone();
            assert_eq!(
                oauth,
                json!({
                    "authMode": "oauth",
                    "registrationMode": "clientMetadata",
                    "clientId": "https://client.example/oauth.json",
                    "clientMetadataUrl": "https://client.example/oauth.json",
                    "scopes": ["mcp", "offline_access"],
                    "endpoints": {
                        "protectedResourceMetadataUrl": "https://example.test/.well-known/oauth-protected-resource",
                        "authorizationEndpoint": "https://auth.example/authorize",
                        "tokenEndpoint": "https://auth.example/token"
                    },
                    "discoveredAt": 1700000000
                })
            );
            let again = McpControlPlaneConfig::from_stored_json(&encoded)
                .unwrap()
                .config;
            assert_eq!(parsed, again);
            assert!(!format!("{parsed:?}").contains("access-token-canary"));
        }
    }

    #[test]
    fn oauth_metadata_rejects_credential_urls_without_echoing_them() {
        let error = McpControlPlaneConfig::from_stored_json(&json!({
            "schemaVersion": 1,
            "upstreams": [{
                "id": "remote",
                "name": "Remote",
                "type": "http",
                "url": "https://example.test/mcp",
                "oauth": {
                    "authMode": "oauth",
                    "clientMetadataUrl": "https://user:super-secret-password@example.test/client.json"
                }
            }]
        }))
        .unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("super-secret-password"));
        assert!(text.contains("clientMetadataUrl"));
    }

    #[test]
    fn static_bearer_still_round_trips_beside_oauth_metadata() {
        let parsed = McpControlPlaneConfig::from_stored_json(&json!([{
            "id": "remote",
            "type": "http",
            "name": "Remote",
            "url": "https://example.test/mcp",
            "bearerToken": "legacy-token",
            "oauth": {
                "authMode": "static",
                "registrationMode": "none"
            }
        }]))
        .unwrap()
        .config;
        let McpPersistedTransport::Http { headers, oauth, .. } = &parsed.upstreams[0].transport
        else {
            panic!("expected HTTP transport");
        };
        assert_eq!(headers[0].value.as_deref(), Some("Bearer legacy-token"));
        assert_eq!(
            oauth.as_ref().map(|config| config.auth_mode),
            Some(super::super::oauth::McpAuthMode::Static)
        );
        let encoded = parsed.to_canonical_json().unwrap();
        assert!(encoded.to_string().contains("legacy-token"));
        assert_eq!(encoded["upstreams"][0]["oauth"]["authMode"], "static");
    }

    #[test]
    fn prepare_write_preserves_existing_built_ins_on_legacy_array_save() {
        let mut existing = prepare_write(&legacy_array(), None).unwrap();
        existing.built_ins[0].enabled = false;
        let next = prepare_write(
            &json!([{ "id": "only", "command": "node" }]),
            Some(&existing),
        )
        .unwrap();
        assert_eq!(next.upstreams.len(), 1);
        assert_eq!(next.upstreams[0].id, "only");
        assert!(!next.built_ins[0].enabled);
    }
}
