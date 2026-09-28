//! MCP server credentials and credential-free OAuth metadata.
//!
//! Access tokens, refresh tokens, client secrets, and static bearer/header
//! values live only in the process keyring. The account is scoped to one
//! canonical project root and one MCP server id, so the same server id in
//! another project cannot read the record.
//! The project document may store [`McpOAuthConfig`] (mode, registration,
//! public client id, metadata URL, scopes, and discovered endpoints) and must
//! never store those secrets. Discovery, authorization, and HTTP injection are
//! owned by the command and transport callers; this module persists their
//! credential result and adapts it to rmcp.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Historical keyring account prefix. Current accounts use
/// [`mcp_oauth_credential_key`] and include a project-root fingerprint.
pub const MCP_OAUTH_KEY_PREFIX: &str = "mcp/oauth/";

const CREDENTIAL_PAYLOAD_VERSION: u16 = 2;
const MAX_SERVER_ID_LEN: usize = 256;
const MAX_SECRET_LEN: usize = 64 * 1024;
const MAX_CLIENT_ID_LEN: usize = 2_048;
const MAX_SCOPE_LEN: usize = 4_096;
const MAX_TOKEN_TYPE_LEN: usize = 64;
const MAX_HEADER_NAME_LEN: usize = 256;

const OAUTH_CREDENTIAL_KEYS: &[&str] = &[
    "accessToken",
    "access_token",
    "refreshToken",
    "refresh_token",
    "clientSecret",
    "client_secret",
    "idToken",
    "id_token",
    "token",
    "bearerToken",
    "bearer_token",
];

/// Failure while reading or writing one server's credential record.
///
/// Display and Debug text is static. It never includes keyring account names,
/// backend errors, or secret values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCredentialError {
    InvalidServerId,
    InvalidCredential,
    StorageUnavailable,
    Corrupt,
}

impl fmt::Display for McpCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidServerId => "MCP server id cannot be used for credential storage",
            Self::InvalidCredential => "MCP credential is invalid",
            Self::StorageUnavailable => "MCP credential storage is unavailable",
            Self::Corrupt => "stored MCP credential is unreadable",
        })
    }
}

impl std::error::Error for McpCredentialError {}

/// Credential-free OAuth metadata was present but not usable.
///
/// Variants are static. Error text never echoes the rejected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthConfigError {
    InvalidShape,
    InvalidAuthMode,
    InvalidRegistrationMode,
    InvalidClientId,
    InvalidClientMetadataUrl,
    InvalidScope,
    InvalidEndpoint,
    InvalidDiscoveredAt,
}

impl fmt::Display for McpOAuthConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidShape => "oauth metadata must be an object",
            Self::InvalidAuthMode => "oauth authMode is invalid",
            Self::InvalidRegistrationMode => "oauth registrationMode is invalid",
            Self::InvalidClientId => "oauth clientId is invalid",
            Self::InvalidClientMetadataUrl => "oauth clientMetadataUrl is invalid",
            Self::InvalidScope => "oauth scopes are invalid",
            Self::InvalidEndpoint => "oauth metadata endpoint is invalid",
            Self::InvalidDiscoveredAt => "oauth discoveredAt is invalid",
        })
    }
}

impl std::error::Error for McpOAuthConfigError {}

/// How an HTTP or SSE upstream authenticates. This is project metadata, not a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum McpAuthMode {
    #[default]
    #[serde(rename = "none")]
    None,
    /// Static bearer or named header material stored in the keyring.
    #[serde(rename = "static")]
    Static,
    #[serde(rename = "oauth")]
    OAuth,
}

/// How the OAuth client id was obtained. No client secret is stored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum McpOAuthRegistrationMode {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "preregistered")]
    Preregistered,
    #[serde(rename = "dynamic")]
    Dynamic,
    /// Client ID Metadata Document. `client_id` is typically the metadata URL.
    #[serde(rename = "clientMetadata")]
    ClientMetadata,
}

/// Discovered authorization endpoints. URLs only; never tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthEndpoints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_resource_metadata_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_server_metadata_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    /// RFC 8707 resource identifier from protected-resource metadata.
    /// This is a public URL, never a token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

impl McpOAuthEndpoints {
    pub fn is_empty(&self) -> bool {
        self.protected_resource_metadata_url.is_none()
            && self.authorization_server_metadata_url.is_none()
            && self.issuer.is_none()
            && self.authorization_endpoint.is_none()
            && self.token_endpoint.is_none()
            && self.registration_endpoint.is_none()
            && self.revocation_endpoint.is_none()
            && self.resource.is_none()
    }
}

/// Credential-free OAuth settings safe to persist in the project MCP document
/// and to show in the renderer.
///
/// `discovered_at` is a unix timestamp in seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    #[serde(default)]
    pub auth_mode: McpAuthMode,
    #[serde(default)]
    pub registration_mode: McpOAuthRegistrationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_metadata_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "McpOAuthEndpoints::is_empty")]
    pub endpoints: McpOAuthEndpoints,
    /// Loopback callback used for the authorization-code flow. The transport
    /// must reuse this exact URI so the client registration and token requests
    /// stay bound to the browser grant. It is not a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_at: Option<i64>,
}

impl McpOAuthConfig {
    pub fn is_blank(&self) -> bool {
        self.auth_mode == McpAuthMode::None
            && self.registration_mode == McpOAuthRegistrationMode::None
            && self.client_id.is_none()
            && self.client_metadata_url.is_none()
            && self.scopes.is_empty()
            && self.endpoints.is_empty()
            && self.redirect_uri.is_none()
            && self.discovered_at.is_none()
    }

    pub fn normalize(&mut self) -> Result<(), McpOAuthConfigError> {
        normalize_client_id(&mut self.client_id)?;
        normalize_optional_url(
            &mut self.client_metadata_url,
            McpOAuthConfigError::InvalidClientMetadataUrl,
        )?;
        normalize_scopes(&mut self.scopes)?;
        normalize_optional_url(
            &mut self.endpoints.protected_resource_metadata_url,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.authorization_server_metadata_url,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.issuer,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.authorization_endpoint,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.token_endpoint,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.registration_endpoint,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.revocation_endpoint,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(
            &mut self.endpoints.resource,
            McpOAuthConfigError::InvalidEndpoint,
        )?;
        normalize_optional_url(&mut self.redirect_uri, McpOAuthConfigError::InvalidEndpoint)?;
        if self.discovered_at.is_some_and(|value| value < 0) {
            return Err(McpOAuthConfigError::InvalidDiscoveredAt);
        }
        Ok(())
    }
}

/// What the keyring currently holds for one server. Safe to serialize to the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpStoredCredentialKind {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "staticBearer")]
    StaticBearer,
    #[serde(rename = "staticHeader")]
    StaticHeader,
    #[serde(rename = "static")]
    Static,
    #[serde(rename = "oauth")]
    OAuth,
    #[serde(rename = "mixed")]
    Mixed,
}

/// Redacted view of stored credentials. Token and header values are booleans only.
///
/// Timestamps are unix seconds. `access_token_expired` is `now >= expires_at`
/// when an expiry is stored, and false when the access token has no expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCredentialStatus {
    pub kind: McpStoredCredentialKind,
    pub has_static_bearer: bool,
    pub static_header_names: Vec<String>,
    pub has_client_id: bool,
    pub has_client_secret: bool,
    pub has_access_token: bool,
    pub has_refresh_token: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub access_token_expired: bool,
}

impl McpCredentialStatus {
    pub fn absent() -> Self {
        Self {
            kind: McpStoredCredentialKind::None,
            has_static_bearer: false,
            static_header_names: Vec::new(),
            has_client_id: false,
            has_client_secret: false,
            has_access_token: false,
            has_refresh_token: false,
            token_type: None,
            scope: None,
            issued_at: None,
            expires_at: None,
            access_token_expired: false,
        }
    }
}

/// Static `Authorization: Bearer` token. The value is not serializable.
#[derive(Clone, PartialEq, Eq)]
pub struct StaticBearerSecret {
    token: String,
}

impl fmt::Debug for StaticBearerSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StaticBearerSecret")
            .field("token", &"<redacted>")
            .finish()
    }
}

impl StaticBearerSecret {
    pub fn new(token: impl Into<String>) -> Result<Self, McpCredentialError> {
        Ok(Self {
            token: require_opaque_secret(token.into())?,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn authorization_value(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

/// One static request header. The value is not serializable.
#[derive(Clone, PartialEq, Eq)]
pub struct StaticHeaderSecret {
    name: String,
    value: String,
}

impl fmt::Debug for StaticHeaderSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StaticHeaderSecret")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl StaticHeaderSecret {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, McpCredentialError> {
        let name = name.into();
        if !is_header_name(&name) {
            return Err(McpCredentialError::InvalidCredential);
        }
        let value = value.into();
        if value.is_empty() || value.len() > MAX_SECRET_LEN || value.chars().any(char::is_control) {
            return Err(McpCredentialError::InvalidCredential);
        }
        Ok(Self { name, value })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Input for [`OAuthSecret::try_new`]. Debug output is fully redacted.
#[derive(Clone)]
pub struct OAuthSecretInput {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub scope: Option<String>,
    pub issued_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub issuer: Option<String>,
}

impl fmt::Debug for OAuthSecretInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OAuthSecretInput(<redacted>)")
    }
}

/// OAuth client and token material. Not serializable outside the keyring payload.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthSecret {
    client_id: String,
    client_secret: Option<String>,
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    scope: Option<String>,
    issued_at: Option<i64>,
    expires_at: Option<i64>,
    issuer: Option<String>,
}

impl fmt::Debug for OAuthSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthSecret")
            .field("client_id", &"<redacted>")
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("issuer", &self.issuer.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl OAuthSecret {
    pub fn try_new(input: OAuthSecretInput) -> Result<Self, McpCredentialError> {
        let client_id = normalize_stored_client_id(input.client_id)?;
        let client_secret = optional_opaque_secret(input.client_secret)?;
        let access_token = require_opaque_secret(input.access_token)?;
        let refresh_token = optional_opaque_secret(input.refresh_token)?;
        let token_type = input.token_type;
        if !is_token_type(&token_type) {
            return Err(McpCredentialError::InvalidCredential);
        }
        let scope = match input.scope {
            None => None,
            Some(scope) => {
                if scope.is_empty()
                    || scope.len() > MAX_SCOPE_LEN
                    || scope
                        .chars()
                        .any(|ch| ch.is_control() || (ch.is_whitespace() && ch != ' '))
                {
                    return Err(McpCredentialError::InvalidCredential);
                }
                Some(scope)
            }
        };
        let issued_at = validate_timestamp(input.issued_at)?;
        let expires_at = validate_timestamp(input.expires_at)?;
        let issuer = normalize_optional_issuer(input.issuer)?;
        Ok(Self {
            client_id,
            client_secret,
            access_token,
            refresh_token,
            token_type,
            scope,
            issued_at,
            expires_at,
            issuer,
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client_secret(&self) -> Option<&str> {
        self.client_secret.as_deref()
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn token_type(&self) -> &str {
        &self.token_type
    }

    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub fn issued_at(&self) -> Option<i64> {
        self.issued_at
    }

    pub fn expires_at(&self) -> Option<i64> {
        self.expires_at
    }

    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    /// True when `expires_at` is set and `now_unix_secs >= expires_at`.
    pub fn is_access_token_expired_at(&self, now_unix_secs: i64) -> bool {
        self.expires_at
            .is_some_and(|expires_at| now_unix_secs >= expires_at)
    }
}

/// All secret material for one MCP server. There is no `Serialize` impl.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerSecrets {
    static_bearer: Option<StaticBearerSecret>,
    static_headers: Vec<StaticHeaderSecret>,
    oauth: Option<OAuthSecret>,
}

impl fmt::Debug for McpServerSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpServerSecrets")
            .field("static_bearer", &self.static_bearer)
            .field("static_headers", &self.static_headers)
            .field("oauth", &self.oauth)
            .finish()
    }
}

impl Default for McpServerSecrets {
    fn default() -> Self {
        Self::empty()
    }
}

impl McpServerSecrets {
    pub fn empty() -> Self {
        Self {
            static_bearer: None,
            static_headers: Vec::new(),
            oauth: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.static_bearer.is_none() && self.static_headers.is_empty() && self.oauth.is_none()
    }

    pub fn static_bearer(&self) -> Option<&StaticBearerSecret> {
        self.static_bearer.as_ref()
    }

    pub fn set_static_bearer(&mut self, bearer: Option<StaticBearerSecret>) {
        self.static_bearer = bearer;
    }

    pub fn static_headers(&self) -> &[StaticHeaderSecret] {
        &self.static_headers
    }

    pub fn upsert_static_header(&mut self, header: StaticHeaderSecret) {
        if let Some(existing) = self
            .static_headers
            .iter_mut()
            .find(|item| item.name.eq_ignore_ascii_case(&header.name))
        {
            *existing = header;
            return;
        }
        self.static_headers.push(header);
    }

    pub fn oauth(&self) -> Option<&OAuthSecret> {
        self.oauth.as_ref()
    }

    pub fn set_oauth(&mut self, oauth: Option<OAuthSecret>) {
        self.oauth = oauth;
    }

    pub fn kind(&self) -> McpStoredCredentialKind {
        let has_bearer = self.static_bearer.is_some();
        let has_headers = !self.static_headers.is_empty();
        let has_static = has_bearer || has_headers;
        let has_oauth = self.oauth.is_some();
        match (has_static, has_oauth, has_bearer, has_headers) {
            (false, false, _, _) => McpStoredCredentialKind::None,
            (true, true, _, _) => McpStoredCredentialKind::Mixed,
            (false, true, _, _) => McpStoredCredentialKind::OAuth,
            (true, false, true, false) => McpStoredCredentialKind::StaticBearer,
            (true, false, false, true) => McpStoredCredentialKind::StaticHeader,
            (true, false, true, true) => McpStoredCredentialKind::Static,
            (true, false, false, false) => McpStoredCredentialKind::None,
        }
    }

    pub fn status_at(&self, now_unix_secs: i64) -> McpCredentialStatus {
        let oauth = self.oauth.as_ref();
        McpCredentialStatus {
            kind: self.kind(),
            has_static_bearer: self.static_bearer.is_some(),
            static_header_names: self
                .static_headers
                .iter()
                .map(|header| header.name.clone())
                .collect(),
            has_client_id: oauth.is_some(),
            has_client_secret: oauth.is_some_and(|secret| secret.client_secret.is_some()),
            has_access_token: oauth.is_some(),
            has_refresh_token: oauth.is_some_and(|secret| secret.refresh_token.is_some()),
            token_type: oauth.map(|secret| secret.token_type.clone()),
            scope: oauth.and_then(|secret| secret.scope.clone()),
            issued_at: oauth.and_then(|secret| secret.issued_at),
            expires_at: oauth.and_then(|secret| secret.expires_at),
            access_token_expired: oauth
                .is_some_and(|secret| secret.is_access_token_expired_at(now_unix_secs)),
        }
    }
}

/// Keyring-backed credential record for one MCP server.
///
/// The account name is [`mcp_oauth_credential_key`]. Values are a private JSON
/// payload and are never logged.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpCredentialStore;

impl McpCredentialStore {
    pub fn load(
        project_root: &Path,
        server_id: &str,
    ) -> Result<Option<McpServerSecrets>, McpCredentialError> {
        let key = mcp_oauth_credential_key(project_root, server_id)?;
        match crate::keyring_get(&key) {
            Ok(None) => Ok(None),
            Ok(Some(payload)) => parse_payload(&payload),
            Err(_) => Err(McpCredentialError::StorageUnavailable),
        }
    }

    pub fn save(
        project_root: &Path,
        server_id: &str,
        secrets: &McpServerSecrets,
    ) -> Result<(), McpCredentialError> {
        if secrets.is_empty() {
            return Self::clear(project_root, server_id);
        }
        let key = mcp_oauth_credential_key(project_root, server_id)?;
        let payload = serde_json::to_string(&secrets.to_payload())
            .map_err(|_| McpCredentialError::InvalidCredential)?;
        crate::keyring_set(&key, &payload).map_err(|_| McpCredentialError::StorageUnavailable)
    }

    pub fn clear(project_root: &Path, server_id: &str) -> Result<(), McpCredentialError> {
        let key = mcp_oauth_credential_key(project_root, server_id)?;
        crate::keyring_delete(&key).map_err(|_| McpCredentialError::StorageUnavailable)
    }
}

pub fn clear_oauth_credentials(
    project_root: &Path,
    server_id: &str,
) -> Result<(), McpCredentialError> {
    let mut secrets = McpCredentialStore::load(project_root, server_id)?.unwrap_or_default();
    secrets.set_oauth(None);
    if secrets.is_empty() {
        McpCredentialStore::clear(project_root, server_id)
    } else {
        McpCredentialStore::save(project_root, server_id, &secrets)
    }
}

/// Drop a stored OAuth grant after the authorization server rejects refresh.
///
/// Static bearer and header credentials are preserved. A missing record is
/// already invalidated and returns success.
pub fn invalidate_rejected_oauth_grant(
    project_root: &Path,
    server_id: &str,
    error: &AuthError,
) -> Result<bool, McpCredentialError> {
    if !oauth_grant_was_rejected(error) {
        return Ok(false);
    }
    if McpCredentialStore::load(project_root, server_id)?.is_none() {
        return Ok(true);
    }
    clear_oauth_credentials(project_root, server_id)?;
    Ok(true)
}

fn oauth_grant_was_rejected(error: &AuthError) -> bool {
    match error {
        AuthError::AuthorizationRequired => true,
        AuthError::TokenRefreshRejected(message) => {
            message.to_ascii_lowercase().contains("invalid_grant")
        }
        AuthError::HttpError(error) => error
            .to_string()
            .to_ascii_lowercase()
            .contains("invalid_grant"),
        _ => false,
    }
}

pub fn mcp_oauth_credential_key(
    project_root: &Path,
    server_id: &str,
) -> Result<String, McpCredentialError> {
    validate_server_id(server_id)?;
    let scope = project_credential_scope(project_root)?;
    Ok(format!("{MCP_OAUTH_KEY_PREFIX}{scope}/{server_id}"))
}

fn project_credential_scope(project_root: &Path) -> Result<String, McpCredentialError> {
    let canonical = project_root
        .canonicalize()
        .map_err(|_| McpCredentialError::InvalidServerId)?;
    if !canonical.is_dir() {
        return Err(McpCredentialError::InvalidServerId);
    }
    Ok(credential_scope_fingerprint(&canonical))
}

fn credential_scope_fingerprint(canonical_root: &Path) -> String {
    use sha2::{Digest, Sha256};
    let encoded = canonical_root
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("\u{001f}");
    let digest = Sha256::digest(encoded.as_bytes());
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Remove credential fields from HTTP/SSE `oauth` objects in a stored document.
///
/// Stdio entries lose the whole `oauth` object. Known token fields are deleted
/// before deserialization so they cannot be written back. Invalid metadata
/// returns a static error and does not include the rejected value.
pub(crate) fn sanitize_mcp_oauth_metadata(value: &mut Value) -> Result<(), McpOAuthConfigError> {
    if value.is_array() {
        let Some(entries) = value.as_array_mut() else {
            return Ok(());
        };
        for entry in entries {
            sanitize_server(entry)?;
        }
        return Ok(());
    }
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let Some(upstreams) = object.get_mut("upstreams").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for entry in upstreams {
        sanitize_server(entry)?;
    }
    Ok(())
}

fn sanitize_server(value: &mut Value) -> Result<(), McpOAuthConfigError> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio")
        .to_owned();
    if kind != "http" && kind != "sse" {
        object.remove("oauth");
        return Ok(());
    }
    let presence = match object.get("oauth") {
        None => return Ok(()),
        Some(Value::Null) => OauthPresence::Null,
        Some(Value::Object(_)) => OauthPresence::Object,
        Some(_) => return Err(McpOAuthConfigError::InvalidShape),
    };
    if presence == OauthPresence::Null {
        object.remove("oauth");
        return Ok(());
    }
    let Some(oauth) = object.get_mut("oauth") else {
        return Ok(());
    };
    strip_credential_keys(oauth);
    let Some(oauth_object) = oauth.as_object_mut() else {
        return Err(McpOAuthConfigError::InvalidShape);
    };
    normalize_oauth_object(oauth_object)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OauthPresence {
    Null,
    Object,
}

fn strip_credential_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in OAUTH_CREDENTIAL_KEYS {
                map.remove(*key);
            }
            for child in map.values_mut() {
                strip_credential_keys(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_credential_keys(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn normalize_oauth_object(object: &mut Map<String, Value>) -> Result<(), McpOAuthConfigError> {
    rename_alias(object, "auth_mode", "authMode");
    rename_alias(object, "registration_mode", "registrationMode");
    rename_alias(object, "client_id", "clientId");
    rename_alias(object, "client_metadata_url", "clientMetadataUrl");
    rename_alias(object, "redirect_uri", "redirectUri");
    rename_alias(object, "discovered_at", "discoveredAt");
    if let Some(mode) = object.get("authMode") {
        require_enum(
            mode,
            &["none", "static", "oauth"],
            McpOAuthConfigError::InvalidAuthMode,
        )?;
    }
    if let Some(mode) = object.get("registrationMode") {
        require_enum(
            mode,
            &["none", "preregistered", "dynamic", "clientMetadata"],
            McpOAuthConfigError::InvalidRegistrationMode,
        )?;
    }
    require_optional_string(object, "clientId", McpOAuthConfigError::InvalidClientId)?;
    require_optional_string(
        object,
        "clientMetadataUrl",
        McpOAuthConfigError::InvalidClientMetadataUrl,
    )?;
    require_optional_string(object, "redirectUri", McpOAuthConfigError::InvalidEndpoint)?;
    coerce_scopes(object)?;
    normalize_endpoints_value(object)?;
    validate_discovered_at(object)?;
    Ok(())
}

fn normalize_endpoints_value(object: &mut Map<String, Value>) -> Result<(), McpOAuthConfigError> {
    let Some(endpoints) = object.get_mut("endpoints") else {
        return Ok(());
    };
    if endpoints.is_null() {
        object.remove("endpoints");
        return Ok(());
    }
    let Some(endpoints) = endpoints.as_object_mut() else {
        return Err(McpOAuthConfigError::InvalidEndpoint);
    };
    rename_alias(
        endpoints,
        "protected_resource_metadata_url",
        "protectedResourceMetadataUrl",
    );
    rename_alias(
        endpoints,
        "authorization_server_metadata_url",
        "authorizationServerMetadataUrl",
    );
    rename_alias(endpoints, "authorization_endpoint", "authorizationEndpoint");
    rename_alias(endpoints, "token_endpoint", "tokenEndpoint");
    rename_alias(endpoints, "registration_endpoint", "registrationEndpoint");
    rename_alias(endpoints, "revocation_endpoint", "revocationEndpoint");
    for key in [
        "protectedResourceMetadataUrl",
        "authorizationServerMetadataUrl",
        "issuer",
        "authorizationEndpoint",
        "tokenEndpoint",
        "registrationEndpoint",
        "revocationEndpoint",
        "resource",
    ] {
        require_optional_string(endpoints, key, McpOAuthConfigError::InvalidEndpoint)?;
    }
    Ok(())
}

fn rename_alias(object: &mut Map<String, Value>, from: &str, to: &str) {
    if object.contains_key(to) {
        object.remove(from);
        return;
    }
    if let Some(value) = object.remove(from) {
        object.insert(to.to_owned(), value);
    }
}

fn require_enum(
    value: &Value,
    allowed: &[&str],
    error: McpOAuthConfigError,
) -> Result<(), McpOAuthConfigError> {
    match value.as_str() {
        Some(text) if allowed.contains(&text) => Ok(()),
        _ => Err(error),
    }
}

fn require_optional_string(
    object: &Map<String, Value>,
    key: &str,
    error: McpOAuthConfigError,
) -> Result<(), McpOAuthConfigError> {
    match object.get(key) {
        None | Some(Value::String(_)) => Ok(()),
        Some(_) => Err(error),
    }
}

fn coerce_scopes(object: &mut Map<String, Value>) -> Result<(), McpOAuthConfigError> {
    let Some(scopes) = object.get("scopes").cloned() else {
        return Ok(());
    };
    if let Some(text) = scopes.as_str() {
        let list = text
            .split_whitespace()
            .filter(|item| !item.is_empty())
            .map(|item| Value::String(item.to_owned()))
            .collect::<Vec<_>>();
        object.insert("scopes".into(), Value::Array(list));
        return Ok(());
    }
    match scopes.as_array() {
        Some(items) if items.iter().all(Value::is_string) => Ok(()),
        _ => Err(McpOAuthConfigError::InvalidScope),
    }
}

fn validate_discovered_at(object: &Map<String, Value>) -> Result<(), McpOAuthConfigError> {
    match object.get("discoveredAt") {
        None => Ok(()),
        Some(value) => match value.as_i64() {
            Some(timestamp) if timestamp >= 0 => Ok(()),
            _ => Err(McpOAuthConfigError::InvalidDiscoveredAt),
        },
    }
}

fn normalize_client_id(value: &mut Option<String>) -> Result<(), McpOAuthConfigError> {
    let Some(raw) = value.clone() else {
        return Ok(());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        *value = None;
        return Ok(());
    }
    if trimmed.len() > MAX_CLIENT_ID_LEN
        || trimmed
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(McpOAuthConfigError::InvalidClientId);
    }
    if trimmed != raw {
        *value = Some(trimmed.to_owned());
    }
    Ok(())
}

fn normalize_scopes(scopes: &mut Vec<String>) -> Result<(), McpOAuthConfigError> {
    let mut normalized = Vec::with_capacity(scopes.len());
    for scope in scopes.drain(..) {
        let trimmed = scope.trim();
        if trimmed.is_empty()
            || trimmed
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(McpOAuthConfigError::InvalidScope);
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing == trimmed)
        {
            normalized.push(trimmed.to_owned());
        }
    }
    *scopes = normalized;
    Ok(())
}

fn normalize_optional_url(
    value: &mut Option<String>,
    error: McpOAuthConfigError,
) -> Result<(), McpOAuthConfigError> {
    let Some(raw) = value.clone() else {
        return Ok(());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        *value = None;
        return Ok(());
    }
    if !is_public_http_url(trimmed) {
        return Err(error);
    }
    if trimmed != raw {
        *value = Some(trimmed.to_owned());
    }
    Ok(())
}

fn is_public_http_url(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return false;
    }
    parsed.username().is_empty() && parsed.password().is_none()
}

fn validate_server_id(server_id: &str) -> Result<(), McpCredentialError> {
    if server_id.trim().is_empty()
        || server_id.len() > MAX_SERVER_ID_LEN
        || server_id
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
    {
        return Err(McpCredentialError::InvalidServerId);
    }
    Ok(())
}

fn require_opaque_secret(value: String) -> Result<String, McpCredentialError> {
    if value.is_empty()
        || value.len() > MAX_SECRET_LEN
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(McpCredentialError::InvalidCredential);
    }
    Ok(value)
}

fn optional_opaque_secret(value: Option<String>) -> Result<Option<String>, McpCredentialError> {
    value.map(require_opaque_secret).transpose()
}

fn normalize_stored_client_id(value: String) -> Result<String, McpCredentialError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_CLIENT_ID_LEN
        || trimmed
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(McpCredentialError::InvalidCredential);
    }
    Ok(trimmed.to_owned())
}

fn is_token_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_TYPE_LEN
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

fn is_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_HEADER_NAME_LEN
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

fn validate_timestamp(value: Option<i64>) -> Result<Option<i64>, McpCredentialError> {
    if value.is_some_and(|timestamp| timestamp < 0) {
        return Err(McpCredentialError::InvalidCredential);
    }
    Ok(value)
}

fn normalize_optional_issuer(value: Option<String>) -> Result<Option<String>, McpCredentialError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let Ok(url) = url::Url::parse(trimmed) else {
        return Err(McpCredentialError::InvalidCredential);
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(McpCredentialError::InvalidCredential);
    }
    Ok(Some(url.to_string()))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialPayload {
    version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    static_bearer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    static_headers: Vec<HeaderPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth: Option<OAuthPayload>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeaderPayload {
    name: String,
    value: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OAuthPayload {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issued_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issuer: Option<String>,
}

impl McpServerSecrets {
    fn to_payload(&self) -> CredentialPayload {
        CredentialPayload {
            version: CREDENTIAL_PAYLOAD_VERSION,
            static_bearer: self
                .static_bearer
                .as_ref()
                .map(|secret| secret.token.clone()),
            static_headers: self
                .static_headers
                .iter()
                .map(|header| HeaderPayload {
                    name: header.name.clone(),
                    value: header.value.clone(),
                })
                .collect(),
            oauth: self.oauth.as_ref().map(|secret| OAuthPayload {
                client_id: secret.client_id.clone(),
                client_secret: secret.client_secret.clone(),
                access_token: secret.access_token.clone(),
                refresh_token: secret.refresh_token.clone(),
                token_type: secret.token_type.clone(),
                scope: secret.scope.clone(),
                issued_at: secret.issued_at,
                expires_at: secret.expires_at,
                issuer: secret.issuer.clone(),
            }),
        }
    }
}

fn parse_payload(raw: &str) -> Result<Option<McpServerSecrets>, McpCredentialError> {
    let payload: CredentialPayload =
        serde_json::from_str(raw).map_err(|_| McpCredentialError::Corrupt)?;
    if payload.version != CREDENTIAL_PAYLOAD_VERSION {
        return Err(McpCredentialError::Corrupt);
    }
    let mut secrets = McpServerSecrets::empty();
    if let Some(token) = payload.static_bearer {
        secrets.static_bearer =
            Some(StaticBearerSecret::new(token).map_err(|_| McpCredentialError::Corrupt)?);
    }
    for header in payload.static_headers {
        secrets.upsert_static_header(
            StaticHeaderSecret::new(header.name, header.value)
                .map_err(|_| McpCredentialError::Corrupt)?,
        );
    }
    if let Some(oauth) = payload.oauth {
        secrets.oauth = Some(
            OAuthSecret::try_new(OAuthSecretInput {
                client_id: oauth.client_id,
                client_secret: oauth.client_secret,
                access_token: oauth.access_token,
                refresh_token: oauth.refresh_token,
                token_type: oauth.token_type,
                scope: oauth.scope,
                issued_at: oauth.issued_at,
                expires_at: oauth.expires_at,
                issuer: oauth.issuer,
            })
            .map_err(|_| McpCredentialError::Corrupt)?,
        );
    }
    if secrets.is_empty() {
        Ok(None)
    } else {
        Ok(Some(secrets))
    }
}

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationMetadata,
    AuthorizationMetadataSource, AuthorizationRequest, AuthorizationSession, CredentialStore,
    OAuthClientConfig, StoredCredentials, WWWAuthenticateParams,
};
use std::collections::{BTreeMap, HashMap};
use tokio::sync::Mutex;

/// Persistent credential adapter for rmcp's OAuth manager.
///
/// rmcp owns the PKCE and token state machine; this adapter keeps its token
/// response in Termul's OS keyring record instead of rmcp's in-memory store.
#[derive(Debug, Clone)]
pub struct KeyringCredentialStore {
    project_root: PathBuf,
    server_id: String,
}

impl KeyringCredentialStore {
    pub fn new(project_root: impl Into<PathBuf>, server_id: impl Into<String>) -> Self {
        Self {
            project_root: project_root.into(),
            server_id: server_id.into(),
        }
    }
}

#[async_trait]
impl CredentialStore for KeyringCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(secrets) =
            McpCredentialStore::load(&self.project_root, &self.server_id).map_err(|_| {
                AuthError::CredentialStoreError("MCP credential storage unavailable".into())
            })?
        else {
            return Ok(None);
        };
        let Some(oauth) = secrets.oauth() else {
            return Ok(None);
        };
        let mut token = serde_json::Map::new();
        token.insert("access_token".into(), oauth.access_token().into());
        token.insert("token_type".into(), oauth.token_type().into());
        if let Some(refresh) = oauth.refresh_token() {
            token.insert("refresh_token".into(), refresh.into());
        }
        if let Some(scope) = oauth.scope() {
            token.insert("scope".into(), scope.into());
        }
        if let (Some(expires_at), Some(issued_at)) = (oauth.expires_at(), oauth.issued_at()) {
            if expires_at >= issued_at {
                token.insert("expires_in".into(), (expires_at - issued_at).into());
            }
        }
        let token_response =
            serde_json::from_value(serde_json::Value::Object(token)).map_err(|_| {
                AuthError::CredentialStoreError("stored MCP OAuth credentials are invalid".into())
            })?;
        Ok(Some(
            StoredCredentials::new(
                oauth.client_id().to_string(),
                Some(token_response),
                oauth
                    .scope()
                    .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
                    .unwrap_or_default(),
                oauth
                    .issued_at()
                    .and_then(|value| u64::try_from(value).ok()),
            )
            .with_issuer(oauth.issuer().map(str::to_owned)),
        ))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let Some(response) = credentials.token_response else {
            return self.clear().await;
        };
        let value = serde_json::to_value(&response).map_err(|_| {
            AuthError::CredentialStoreError("OAuth token response could not be stored".into())
        })?;
        let object = value.as_object().ok_or_else(|| {
            AuthError::CredentialStoreError("OAuth token response is invalid".into())
        })?;
        let access_token = object
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AuthError::CredentialStoreError("OAuth token response has no access token".into())
            })?;
        let token_type = object
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer");
        let mut refresh_token = object
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let scope = object
            .get("scope")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let expires_in = object.get("expires_in").and_then(|v| v.as_i64());
        let issued_at = credentials
            .token_received_at
            .and_then(|value| i64::try_from(value).ok());
        let expires_at =
            expires_in.and_then(|value| issued_at.map(|issued| issued.saturating_add(value)));
        let existing = McpCredentialStore::load(&self.project_root, &self.server_id)
            .ok()
            .flatten();
        let existing_oauth = existing.as_ref().and_then(McpServerSecrets::oauth);
        if refresh_token.is_none() {
            refresh_token =
                existing_oauth.and_then(|oauth| oauth.refresh_token().map(str::to_owned));
        }
        let oauth = OAuthSecret::try_new(OAuthSecretInput {
            client_id: credentials.client_id,
            client_secret: existing_oauth
                .and_then(|oauth| oauth.client_secret().map(str::to_owned)),
            access_token: access_token.to_string(),
            refresh_token,
            token_type: token_type.to_string(),
            scope,
            issued_at,
            expires_at,
            issuer: credentials
                .issuer
                .or_else(|| existing_oauth.and_then(|oauth| oauth.issuer().map(str::to_owned))),
        })
        .map_err(|_| AuthError::CredentialStoreError("OAuth credentials are invalid".into()))?;
        let mut secrets = existing.unwrap_or_default();
        secrets.set_oauth(Some(oauth));
        McpCredentialStore::save(&self.project_root, &self.server_id, &secrets).map_err(|_| {
            AuthError::CredentialStoreError("MCP credential storage unavailable".into())
        })
    }

    async fn clear(&self) -> Result<(), AuthError> {
        clear_oauth_credentials(&self.project_root, &self.server_id).map_err(|_| {
            AuthError::CredentialStoreError("MCP credential storage unavailable".into())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuthAuthorizationState {
    Idle,
    Pending,
    Authorized,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthBeginResult {
    pub server_id: String,
    pub authorization_url: String,
    pub expires_at: i64,
    pub state: OAuthAuthorizationState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStatusResult {
    pub server_id: String,
    pub state: OAuthAuthorizationState,
    pub has_credentials: bool,
    pub can_refresh: bool,
    pub expires_at: Option<i64>,
    pub authorization_required: bool,
}

pub struct PendingOAuthAuthorization {
    pub session: AuthorizationSession,
    pub expires_at: i64,
}

pub struct OAuthCoordinator {
    pending: Mutex<HashMap<String, PendingOAuthAuthorization>>,
}

impl Default for OAuthCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthCoordinator {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn insert(&self, server_id: String, flow: PendingOAuthAuthorization) {
        self.pending.lock().await.insert(server_id, flow);
    }

    pub async fn take(&self, server_id: &str) -> Option<PendingOAuthAuthorization> {
        self.pending.lock().await.remove(server_id)
    }

    pub async fn is_pending(&self, server_id: &str) -> bool {
        self.pending
            .lock()
            .await
            .get(server_id)
            .is_some_and(|flow| flow.expires_at > chrono::Utc::now().timestamp())
    }

    pub async fn cancel(&self, server_id: &str) -> bool {
        self.pending.lock().await.remove(server_id).is_some()
    }
}

const DEFAULT_OAUTH_REDIRECT: &str = "http://127.0.0.1/oauth/callback";

/// Client identity selected for one authorization or token request.
///
/// Client ID Metadata Documents stay ahead of a stored client id when that
/// registration mode is explicit. The metadata URL is public; this value never
/// carries a client secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientIdentity {
    pub preregistered_client_id: Option<String>,
    pub client_metadata_url: Option<String>,
}

impl OAuthClientIdentity {
    pub fn transport_client_id(&self) -> Option<&str> {
        self.preregistered_client_id
            .as_deref()
            .or(self.client_metadata_url.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct OAuthChallengeHints {
    protected_resource_metadata_url: Option<String>,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ProtectedResourceBinding {
    resource: Option<String>,
    authorization_server: Option<String>,
}

/// Redirect registered for this server, or the stable loopback default before
/// the first browser authorization chooses a port.
pub fn oauth_redirect_uri(config: &McpOAuthConfig) -> String {
    config
        .redirect_uri
        .clone()
        .filter(|value| is_public_http_url(value))
        .unwrap_or_else(|| DEFAULT_OAUTH_REDIRECT.to_owned())
}

pub fn oauth_client_identity(config: &McpOAuthConfig) -> OAuthClientIdentity {
    match config.registration_mode {
        McpOAuthRegistrationMode::ClientMetadata => OAuthClientIdentity {
            preregistered_client_id: None,
            client_metadata_url: config.client_metadata_url.clone().or_else(|| {
                config
                    .client_id
                    .clone()
                    .filter(|value| is_https_client_metadata_url(value))
            }),
        },
        McpOAuthRegistrationMode::Preregistered
        | McpOAuthRegistrationMode::Dynamic
        | McpOAuthRegistrationMode::None => OAuthClientIdentity {
            preregistered_client_id: config.client_id.clone(),
            client_metadata_url: None,
        },
    }
}

/// Build the browser authorization request without replacing configured scopes
/// or treating a Client ID Metadata URL as an ordinary pre-registered client.
pub fn authorization_request(
    config: &McpOAuthConfig,
    redirect_uri: &str,
    client_secret: Option<&str>,
) -> AuthorizationRequest {
    let identity = oauth_client_identity(config);
    let mut request =
        AuthorizationRequest::new(redirect_uri.to_owned()).with_client_name("Termul MCP");
    if !config.scopes.is_empty() {
        request = request.with_scopes(config.scopes.clone());
    }
    if let Some(url) = identity.client_metadata_url {
        request = request.with_client_metadata_url(url);
    }
    if let Some(client_id) = identity.preregistered_client_id {
        request = request.with_preregistered_client(client_id);
        if let Some(secret) = client_secret.filter(|value| !value.is_empty()) {
            request = request.with_client_secret(secret);
        }
    }
    request
}

/// Keep an explicit scope list. A challenge scope is only a fallback when the
/// user configured nothing; authorization-server `scopes_supported` is never
/// copied into the project document.
fn scopes_for_authorization(configured: &[String], challenge_scopes: &[String]) -> Vec<String> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    challenge_scopes.to_vec()
}

fn oauth_challenge_hints(endpoint: &str, challenge: &str) -> OAuthChallengeHints {
    let Ok(base) = url::Url::parse(endpoint) else {
        return OAuthChallengeHints::default();
    };
    let params = WWWAuthenticateParams::parse(challenge, &base);
    let protected_resource_metadata_url = params
        .resource_metadata_url
        .as_ref()
        .map(url::Url::to_string)
        .filter(|value| public_same_origin(endpoint, value));
    let mut scopes = params
        .scope
        .unwrap_or_default()
        .split_whitespace()
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if normalize_scopes(&mut scopes).is_err() {
        scopes.clear();
    }
    OAuthChallengeHints {
        protected_resource_metadata_url,
        scopes,
    }
}

/// Persist the challenge pointer, protected-resource binding, and configured
/// scopes. Authorization-server advertisements do not replace the scope list.
pub async fn record_oauth_discovery(
    config: &mut McpOAuthConfig,
    endpoint: &str,
    challenge: Option<&str>,
    redirect_uri: &str,
    metadata: &AuthorizationMetadata,
) -> Result<(), McpOAuthConfigError> {
    let configured_scopes = config.scopes.clone();
    let hints = challenge
        .map(|value| oauth_challenge_hints(endpoint, value))
        .unwrap_or_default();
    if let Some(url) = hints.protected_resource_metadata_url.clone() {
        config.endpoints.protected_resource_metadata_url = Some(url);
    }
    if let Some(url) = config
        .endpoints
        .protected_resource_metadata_url
        .clone()
        .filter(|url| public_same_origin(endpoint, url))
    {
        if let Ok(binding) = read_protected_resource_binding(&url).await {
            if let Some(resource) = binding.resource {
                config.endpoints.resource = Some(resource);
            }
            if let Some(server) = binding.authorization_server {
                config.endpoints.authorization_server_metadata_url = Some(server);
            }
        } else if hints.protected_resource_metadata_url.is_some() {
            return Err(McpOAuthConfigError::InvalidEndpoint);
        }
    }
    config.endpoints.authorization_endpoint = Some(metadata.authorization_endpoint.clone());
    config.endpoints.token_endpoint = Some(metadata.token_endpoint.clone());
    config.endpoints.registration_endpoint = metadata.registration_endpoint.clone();
    config.endpoints.issuer = metadata.issuer.clone();
    config.scopes = scopes_for_authorization(&configured_scopes, &hints.scopes);
    if is_public_http_url(redirect_uri) {
        config.redirect_uri = Some(redirect_uri.to_owned());
    }
    config.auth_mode = McpAuthMode::OAuth;
    config.discovered_at = Some(chrono::Utc::now().timestamp());
    config.normalize()
}

async fn install_authorization_metadata(
    manager: &mut AuthorizationManager,
    endpoint: &str,
    config: &McpOAuthConfig,
) -> Result<(), AuthError> {
    if let Some(url) = config
        .endpoints
        .protected_resource_metadata_url
        .as_deref()
        .filter(|url| public_same_origin(endpoint, url) && challenge_safe_metadata_url(url))
    {
        let challenge = format!("Bearer resource_metadata=\"{url}\"");
        if let Ok(resolution) = manager
            .resolve_metadata_from_challenge(Some(&challenge))
            .await
        {
            if resolution.source == AuthorizationMetadataSource::ProtectedResourceMetadata {
                manager.set_metadata(resolution.metadata);
                return Ok(());
            }
        }
    }
    if let Ok(metadata) = authorization_metadata(config) {
        manager.set_metadata(metadata);
        return Ok(());
    }
    let resolution = manager.resolve_metadata().await?;
    manager.set_metadata(resolution.metadata);
    Ok(())
}

fn challenge_safe_metadata_url(url: &str) -> bool {
    !url.chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\' | ','))
}

fn public_same_origin(endpoint: &str, candidate: &str) -> bool {
    if !is_public_http_url(candidate) {
        return false;
    }
    let Ok(base) = url::Url::parse(endpoint) else {
        return false;
    };
    let Ok(target) = url::Url::parse(candidate) else {
        return false;
    };
    base.scheme() == target.scheme()
        && base.host_str() == target.host_str()
        && base.port_or_known_default() == target.port_or_known_default()
}

fn is_https_client_metadata_url(value: &str) -> bool {
    url::Url::parse(value).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.path() != "/"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

async fn read_protected_resource_binding(
    url: &str,
) -> Result<ProtectedResourceBinding, McpOAuthConfigError> {
    if !is_public_http_url(url) {
        return Err(McpOAuthConfigError::InvalidEndpoint);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| McpOAuthConfigError::InvalidEndpoint)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| McpOAuthConfigError::InvalidEndpoint)?;
    if !response.status().is_success() {
        return Err(McpOAuthConfigError::InvalidEndpoint);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| McpOAuthConfigError::InvalidEndpoint)?;
    if bytes.len() > 65_536 {
        return Err(McpOAuthConfigError::InvalidEndpoint);
    }
    let document: ProtectedResourceDocument =
        serde_json::from_slice(&bytes).map_err(|_| McpOAuthConfigError::InvalidEndpoint)?;
    let mut servers = Vec::new();
    if let Some(server) = document.authorization_server {
        servers.push(server);
    }
    if let Some(more) = document.authorization_servers {
        servers.extend(more);
    }
    Ok(ProtectedResourceBinding {
        resource: document.resource.filter(|value| is_public_http_url(value)),
        authorization_server: servers.into_iter().find(|value| is_public_http_url(value)),
    })
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceDocument {
    resource: Option<String>,
    authorization_server: Option<String>,
    authorization_servers: Option<Vec<String>>,
}

pub fn authorization_metadata(config: &McpOAuthConfig) -> Result<AuthorizationMetadata, AuthError> {
    let endpoints = &config.endpoints;
    let authorization_endpoint = endpoints
        .authorization_endpoint
        .clone()
        .ok_or_else(|| AuthError::MetadataError("missing authorization endpoint".into()))?;
    let token_endpoint = endpoints
        .token_endpoint
        .clone()
        .ok_or_else(|| AuthError::MetadataError("missing token endpoint".into()))?;
    let mut additional_fields = HashMap::new();
    if let Some(value) = config.endpoints.issuer.clone() {
        additional_fields.insert("issuer".into(), value.into());
    }
    let mut metadata = AuthorizationMetadata::default();
    metadata.authorization_endpoint = authorization_endpoint;
    metadata.token_endpoint = token_endpoint;
    metadata.registration_endpoint = endpoints.registration_endpoint.clone();
    metadata.issuer = endpoints.issuer.clone();
    metadata.scopes_supported = (!config.scopes.is_empty()).then(|| config.scopes.clone());
    metadata.response_types_supported = Some(vec!["code".into()]);
    metadata.code_challenge_methods_supported = Some(vec!["S256".into()]);
    metadata.additional_fields = additional_fields;
    Ok(metadata)
}

pub async fn configured_authorization_manager(
    endpoint: &str,
    project_root: &Path,
    server_id: &str,
    config: &McpOAuthConfig,
    redirect_uri: &str,
) -> Result<AuthorizationManager, AuthError> {
    let mut manager = AuthorizationManager::new(endpoint).await?;
    manager.set_credential_store(KeyringCredentialStore::new(project_root, server_id));
    install_authorization_metadata(&mut manager, endpoint, config).await?;
    let identity = oauth_client_identity(config);
    if let Some(client_id) = identity.transport_client_id() {
        let mut client = OAuthClientConfig::new(client_id, redirect_uri.to_owned())
            .with_scopes(config.scopes.clone());
        if identity.preregistered_client_id.is_some() {
            if let Ok(Some(secrets)) = McpCredentialStore::load(project_root, server_id) {
                if let Some(secret) = secrets.oauth().and_then(|oauth| oauth.client_secret()) {
                    client = client.with_client_secret(secret);
                }
            }
        }
        manager.configure_client(client)?;
    }
    Ok(manager)
}

/// Build an rmcp Streamable HTTP transport backed by the persistent OAuth
/// credential store. Static headers remain on the ordinary transport path;
/// OAuth supplies the bearer header through `AuthClient` and refreshes it when
/// rmcp sees an authentication challenge.
pub async fn oauth_transport(
    endpoint: &str,
    project_root: &Path,
    server_id: &str,
    config: &McpOAuthConfig,
    headers: &BTreeMap<String, String>,
) -> Result<rmcp::transport::StreamableHttpClientTransport<AuthClient<reqwest::Client>>, String> {
    let manager = configured_authorization_manager(
        endpoint,
        project_root,
        server_id,
        config,
        &oauth_redirect_uri(config),
    )
    .await
    .map_err(|_| "OAuth client configuration failed".to_string())?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|_| "OAuth HTTP client could not be created".to_string())?;
    let mut custom_headers = HashMap::new();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization") {
            continue;
        }
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "MCP header name is invalid".to_string())?;
        let header_value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| "MCP header value is invalid".to_string())?;
        custom_headers.insert(header_name, header_value);
    }
    let mut transport_config =
        rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
            endpoint,
        );
    transport_config.allow_stateless = true;
    transport_config.custom_headers = custom_headers;
    Ok(rmcp::transport::StreamableHttpClientTransport::with_client(
        AuthClient::new(client, manager),
        transport_config,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{self, CredentialBackend, CredentialError};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    const ACCESS_CANARY: &str = "access-token-canary";
    const REFRESH_CANARY: &str = "refresh-token-canary";
    const SECRET_CANARY: &str = "client-secret-canary";
    const BEARER_CANARY: &str = "bearer-token-canary";
    const HEADER_CANARY: &str = "header-value-canary";
    const CLIENT_CANARY: &str = "client-id-canary";

    #[derive(Default)]
    struct MemoryBackend {
        entries: Mutex<BTreeMap<(String, String), String>>,
    }

    impl CredentialBackend for MemoryBackend {
        fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
            Ok(self
                .entries
                .lock()
                .expect("credential map")
                .get(&(service.to_owned(), key.to_owned()))
                .cloned())
        }

        fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
            self.entries
                .lock()
                .expect("credential map")
                .insert((service.to_owned(), key.to_owned()), value.to_owned());
            Ok(())
        }

        fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
            self.entries
                .lock()
                .expect("credential map")
                .remove(&(service.to_owned(), key.to_owned()));
            Ok(())
        }
    }

    struct FailingBackend;

    impl CredentialBackend for FailingBackend {
        fn get(&self, _service: &str, _key: &str) -> Result<Option<String>, CredentialError> {
            Err(CredentialError::Backend("leak-canary-backend".into()))
        }

        fn set(&self, _service: &str, _key: &str, _value: &str) -> Result<(), CredentialError> {
            Err(CredentialError::Backend("leak-canary-backend".into()))
        }

        fn delete(&self, _service: &str, _key: &str) -> Result<(), CredentialError> {
            Err(CredentialError::Unavailable("leak-canary-backend".into()))
        }
    }

    fn sample_secrets() -> McpServerSecrets {
        let mut secrets = McpServerSecrets::empty();
        secrets.set_static_bearer(Some(StaticBearerSecret::new(BEARER_CANARY).unwrap()));
        secrets.upsert_static_header(StaticHeaderSecret::new("X-Api-Key", HEADER_CANARY).unwrap());
        secrets.set_oauth(Some(
            OAuthSecret::try_new(OAuthSecretInput {
                client_id: CLIENT_CANARY.into(),
                client_secret: Some(SECRET_CANARY.into()),
                access_token: ACCESS_CANARY.into(),
                refresh_token: Some(REFRESH_CANARY.into()),
                token_type: "Bearer".into(),
                scope: Some("mcp offline_access".into()),
                issued_at: Some(1_700_000_000),
                expires_at: Some(1_700_003_600),
                issuer: Some("https://issuer.example".into()),
            })
            .unwrap(),
        ));
        secrets
    }

    fn assert_no_canaries(text: &str) {
        for canary in [
            ACCESS_CANARY,
            REFRESH_CANARY,
            SECRET_CANARY,
            BEARER_CANARY,
            HEADER_CANARY,
            CLIENT_CANARY,
            "leak-canary-backend",
        ] {
            assert!(!text.contains(canary), "leaked credential material");
        }
    }

    #[test]
    fn keyring_roundtrip_uses_server_prefix_and_preserves_secrets() {
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let original = sample_secrets();
        let root = std::env::temp_dir();
        McpCredentialStore::save(&root, "remote", &original).unwrap();

        let key = mcp_oauth_credential_key(&root, "remote").unwrap();
        assert!(key.starts_with("mcp/oauth/"));
        assert!(key.ends_with("/remote"));
        assert_ne!(key, "mcp/oauth/remote");
        let stored = backend
            .entries
            .lock()
            .unwrap()
            .values()
            .find(|value| value.contains(ACCESS_CANARY))
            .cloned()
            .expect("payload stored");
        assert!(stored.contains(REFRESH_CANARY));
        assert!(
            backend
                .entries
                .lock()
                .unwrap()
                .keys()
                .any(|(_, account)| account == &key),
            "credential account must be the mcp/oauth/{{server_id}} key"
        );

        let loaded = McpCredentialStore::load(&root, "remote").unwrap().unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.static_bearer().unwrap().token(), BEARER_CANARY);
        assert_eq!(
            loaded.static_bearer().unwrap().authorization_value(),
            format!("Bearer {BEARER_CANARY}")
        );
        assert_eq!(loaded.static_headers()[0].value(), HEADER_CANARY);
        let oauth = loaded.oauth().unwrap();
        assert_eq!(oauth.client_id(), CLIENT_CANARY);
        assert_eq!(oauth.client_secret(), Some(SECRET_CANARY));
        assert_eq!(oauth.access_token(), ACCESS_CANARY);
        assert_eq!(oauth.refresh_token(), Some(REFRESH_CANARY));
        assert_eq!(oauth.token_type(), "Bearer");
        assert_eq!(oauth.scope(), Some("mcp offline_access"));
        assert_eq!(oauth.issued_at(), Some(1_700_000_000));
        assert_eq!(oauth.expires_at(), Some(1_700_003_600));
        assert_eq!(oauth.issuer(), Some("https://issuer.example/"));
        assert!(McpCredentialStore::load(&root, "other").unwrap().is_none());
    }

    #[test]
    fn project_roots_do_not_share_the_same_server_id() {
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let left = std::env::temp_dir();
        let right = std::env::current_dir().unwrap();
        McpCredentialStore::save(&left, "remote", &sample_secrets()).unwrap();
        assert!(McpCredentialStore::load(&right, "remote")
            .unwrap()
            .is_none());
        let other = McpServerSecrets {
            static_bearer: Some(StaticBearerSecret::new("other-project-token").unwrap()),
            ..McpServerSecrets::empty()
        };
        McpCredentialStore::save(&right, "remote", &other).unwrap();
        assert_eq!(
            McpCredentialStore::load(&left, "remote")
                .unwrap()
                .unwrap()
                .oauth()
                .unwrap()
                .access_token(),
            ACCESS_CANARY
        );
        assert_eq!(
            McpCredentialStore::load(&right, "remote")
                .unwrap()
                .unwrap()
                .static_bearer()
                .unwrap()
                .token(),
            "other-project-token"
        );
    }

    #[test]
    fn clear_removes_only_the_server_record_and_is_idempotent() {
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let root = std::env::temp_dir();
        McpCredentialStore::save(&root, "remote", &sample_secrets()).unwrap();
        let other = McpServerSecrets {
            static_bearer: Some(StaticBearerSecret::new("other-bearer-token").unwrap()),
            ..McpServerSecrets::empty()
        };
        McpCredentialStore::save(&root, "other", &other).unwrap();

        McpCredentialStore::clear(&root, "remote").unwrap();
        assert!(McpCredentialStore::load(&root, "remote").unwrap().is_none());
        assert_eq!(
            McpCredentialStore::load(&root, "other")
                .unwrap()
                .unwrap()
                .static_bearer()
                .unwrap()
                .token(),
            "other-bearer-token"
        );
        McpCredentialStore::clear(&root, "remote").unwrap();
        McpCredentialStore::save(&root, "other", &McpServerSecrets::empty()).unwrap();
        assert!(McpCredentialStore::load(&root, "other").unwrap().is_none());
    }

    #[test]
    fn expiry_is_inclusive_and_absent_expiry_is_not_expired() {
        let mut secrets = sample_secrets();
        let oauth = secrets.oauth_mut_for_test();
        assert!(!oauth.is_access_token_expired_at(1_700_003_599));
        assert!(oauth.is_access_token_expired_at(1_700_003_600));
        assert!(oauth.is_access_token_expired_at(1_700_003_601));

        let mut status = secrets.status_at(1_700_003_599);
        assert!(!status.access_token_expired);
        assert_eq!(status.expires_at, Some(1_700_003_600));
        status = secrets.status_at(1_700_003_600);
        assert!(status.access_token_expired);
        assert_eq!(status.kind, McpStoredCredentialKind::Mixed);

        secrets.set_oauth(Some(
            OAuthSecret::try_new(OAuthSecretInput {
                client_id: CLIENT_CANARY.into(),
                client_secret: None,
                access_token: ACCESS_CANARY.into(),
                refresh_token: None,
                token_type: "Bearer".into(),
                scope: None,
                issued_at: Some(10),
                expires_at: None,
                issuer: None,
            })
            .unwrap(),
        ));
        assert!(!secrets
            .oauth()
            .unwrap()
            .is_access_token_expired_at(i64::MAX));
        assert!(!secrets.status_at(50).access_token_expired);
        secrets.set_static_bearer(None);
        secrets.static_headers.clear();
        assert_eq!(secrets.status_at(50).kind, McpStoredCredentialKind::OAuth);
    }

    #[test]
    fn debug_status_and_errors_do_not_leak_secret_values() {
        let secrets = sample_secrets();
        assert_no_canaries(&format!("{secrets:?}"));
        assert_no_canaries(&format!("{:?}", secrets.oauth().unwrap()));
        assert_no_canaries(&format!(
            "{:?}",
            OAuthSecretInput {
                client_id: CLIENT_CANARY.into(),
                client_secret: Some(SECRET_CANARY.into()),
                access_token: ACCESS_CANARY.into(),
                refresh_token: Some(REFRESH_CANARY.into()),
                token_type: "Bearer".into(),
                scope: None,
                issued_at: None,
                expires_at: None,
                issuer: None,
            }
        ));

        let status = secrets.status_at(1_700_003_600);
        let status_json = serde_json::to_string(&status).unwrap();
        assert_no_canaries(&status_json);
        assert_no_canaries(&format!("{status:?}"));
        assert!(status_json.contains("\"hasAccessToken\":true"));
        assert!(status_json.contains("\"hasClientSecret\":true"));
        assert!(status_json.contains("\"tokenType\":\"Bearer\""));
        assert!(status_json.contains("X-Api-Key"));
        assert!(!status_json.contains("\"accessToken\""));
        assert!(!status_json.contains("\"refreshToken\""));
        assert!(!status_json.contains("\"clientSecret\""));

        let config = McpOAuthConfig {
            auth_mode: McpAuthMode::OAuth,
            registration_mode: McpOAuthRegistrationMode::Dynamic,
            client_id: Some("public-client".into()),
            client_metadata_url: None,
            scopes: vec!["mcp".into()],
            endpoints: McpOAuthEndpoints::default(),
            redirect_uri: None,
            discovered_at: Some(10),
        };
        let config_json = serde_json::to_string(&config).unwrap();
        assert!(config_json.contains("public-client"));
        assert!(!config_json.contains("accessToken"));
        assert!(!config_json.contains("refreshToken"));
        assert!(!config_json.contains("clientSecret"));

        let backend = Arc::new(FailingBackend);
        let _guard = credentials::override_backend(backend as Arc<dyn CredentialBackend>);
        let error = McpCredentialStore::load(&std::env::temp_dir(), "remote").unwrap_err();
        assert_eq!(error, McpCredentialError::StorageUnavailable);
        assert_no_canaries(&error.to_string());
        assert_no_canaries(&format!("{error:?}"));

        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let root = std::env::temp_dir();
        let key = mcp_oauth_credential_key(&root, "remote").unwrap();
        crate::keyring_set(
            &key,
            r#"{"version":1,"accessToken":"access-token-canary","oauth":{"clientSecret":"client-secret-canary"}}"#,
        )
        .unwrap();
        let error = McpCredentialStore::load(&root, "remote").unwrap_err();
        assert_eq!(error, McpCredentialError::Corrupt);
        assert_no_canaries(&error.to_string());
        assert_no_canaries(&format!("{error:?}"));
    }

    #[test]
    fn invalid_server_ids_are_rejected_without_echoing_the_id() {
        let root = std::env::temp_dir();
        let error = mcp_oauth_credential_key(&root, "").unwrap_err();
        assert_eq!(error, McpCredentialError::InvalidServerId);
        assert!(mcp_oauth_credential_key(&root, "remote/nested").is_err());
        assert!(mcp_oauth_credential_key(&root, "bad\nid").is_err());
        let key = mcp_oauth_credential_key(&root, "remote.server_1").unwrap();
        assert!(key.starts_with("mcp/oauth/"));
        assert!(key.ends_with("/remote.server_1"));
        assert!(!error.to_string().contains('/'));
    }

    #[tokio::test]
    async fn refresh_save_preserves_secret_refresh_token_and_issuer() {
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let root = std::env::temp_dir();
        McpCredentialStore::save(&root, "remote", &sample_secrets()).unwrap();
        let store = KeyringCredentialStore::new(&root, "remote");
        let mut stored = store.load().await.unwrap().unwrap();
        let mut response = stored.token_response.take().unwrap();
        response.set_refresh_token(None);
        stored.token_response = Some(response);
        stored.issuer = None;
        store.save(stored).await.unwrap();

        let loaded = McpCredentialStore::load(&root, "remote").unwrap().unwrap();
        let oauth = loaded.oauth().unwrap();
        assert_eq!(oauth.client_secret(), Some(SECRET_CANARY));
        assert_eq!(oauth.refresh_token(), Some(REFRESH_CANARY));
        assert_eq!(oauth.issuer(), Some("https://issuer.example/"));
        let exposed = store.load().await.unwrap().unwrap();
        assert_eq!(exposed.issuer.as_deref(), Some("https://issuer.example/"));
    }

    #[test]
    fn rejected_refresh_clears_only_the_oauth_grant() {
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let root = std::env::temp_dir();
        McpCredentialStore::save(&root, "remote", &sample_secrets()).unwrap();
        assert!(invalidate_rejected_oauth_grant(
            &root,
            "remote",
            &AuthError::TokenRefreshRejected("invalid_grant".into()),
        )
        .unwrap());
        let loaded = McpCredentialStore::load(&root, "remote").unwrap().unwrap();
        assert!(loaded.oauth().is_none());
        assert!(loaded.static_bearer().is_some());
        assert!(!invalidate_rejected_oauth_grant(
            &root,
            "remote",
            &AuthError::TokenRefreshFailed("temporary".into()),
        )
        .unwrap());
        assert!(McpCredentialStore::load(&root, "remote")
            .unwrap()
            .unwrap()
            .static_bearer()
            .is_some());
    }

    #[test]
    fn sanitize_drops_token_fields_and_rejects_bad_modes_quietly() {
        let mut document = serde_json::json!({
            "upstreams": [{
                "type": "http",
                "oauth": {
                    "auth_mode": "oauth",
                    "registration_mode": "clientMetadata",
                    "client_id": "public-client",
                    "scopes": "mcp offline_access",
                    "accessToken": ACCESS_CANARY,
                    "refresh_token": REFRESH_CANARY,
                    "clientSecret": SECRET_CANARY,
                    "endpoints": {
                        "authorization_endpoint": "https://auth.example/authorize",
                        "token": "nested-token-canary"
                    }
                }
            }]
        });
        sanitize_mcp_oauth_metadata(&mut document).unwrap();
        let text = document.to_string();
        assert_no_canaries(&text);
        assert!(!text.contains("nested-token-canary"));
        assert!(text.contains("public-client"));
        assert!(text.contains("authMode"));
        assert!(text.contains("offline_access"));

        let mut bad = serde_json::json!({
            "upstreams": [{
                "type": "sse",
                "oauth": { "authMode": "access-token-canary" }
            }]
        });
        let error = sanitize_mcp_oauth_metadata(&mut bad).unwrap_err();
        assert_eq!(error, McpOAuthConfigError::InvalidAuthMode);
        assert_no_canaries(&error.to_string());

        let mut stdio = serde_json::json!([{
            "type": "stdio",
            "oauth": { "accessToken": ACCESS_CANARY, "authMode": "oauth" }
        }]);
        sanitize_mcp_oauth_metadata(&mut stdio).unwrap();
        assert!(!stdio.to_string().contains("oauth"));
        assert_no_canaries(&stdio.to_string());
    }

    #[test]
    fn configured_scopes_survive_challenge_and_advertised_scopes() {
        assert_eq!(
            scopes_for_authorization(
                &["mcp:read".to_owned()],
                &["mcp:read".to_owned(), "mcp:admin".to_owned()]
            ),
            vec!["mcp:read".to_owned()]
        );
        assert_eq!(
            scopes_for_authorization(&[], &["mcp:read".to_owned(), "mcp:admin".to_owned()]),
            vec!["mcp:read".to_owned(), "mcp:admin".to_owned()]
        );

        let hints = oauth_challenge_hints(
            "https://mcp.example/mcp",
            r#"Bearer error="invalid_token", resource_metadata="/.well-known/oauth-protected-resource", scope="mcp:read mcp:admin""#,
        );
        assert_eq!(
            hints.protected_resource_metadata_url.as_deref(),
            Some("https://mcp.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            hints.scopes,
            vec!["mcp:read".to_owned(), "mcp:admin".to_owned()]
        );

        let cross_origin = oauth_challenge_hints(
            "https://mcp.example/mcp",
            r#"Bearer resource_metadata="https://evil.example/metadata", scope="mcp:read""#,
        );
        assert!(cross_origin.protected_resource_metadata_url.is_none());
        assert_eq!(cross_origin.scopes, vec!["mcp:read".to_owned()]);
    }

    #[test]
    fn client_metadata_and_redirect_stay_bound_to_the_configured_client() {
        let config = McpOAuthConfig {
            registration_mode: McpOAuthRegistrationMode::ClientMetadata,
            client_id: Some("issued-client".into()),
            client_metadata_url: Some("https://client.example/oauth.json".into()),
            scopes: vec!["mcp:read".into()],
            redirect_uri: Some("http://127.0.0.1:43111/oauth/callback".into()),
            ..McpOAuthConfig::default()
        };
        let identity = oauth_client_identity(&config);
        assert!(identity.preregistered_client_id.is_none());
        assert_eq!(
            identity.transport_client_id(),
            Some("https://client.example/oauth.json")
        );
        let request =
            authorization_request(&config, &oauth_redirect_uri(&config), Some(SECRET_CANARY));
        assert_eq!(
            request.redirect_uri,
            "http://127.0.0.1:43111/oauth/callback"
        );
        assert_eq!(
            request.client_metadata_url.as_deref(),
            Some("https://client.example/oauth.json")
        );
        assert!(request.client_id.is_none());
        assert!(request.client_secret.is_none());
        assert_eq!(request.scopes, vec!["mcp:read".to_owned()]);
        assert_eq!(
            oauth_redirect_uri(&McpOAuthConfig::default()),
            "http://127.0.0.1/oauth/callback"
        );

        let preregistered = McpOAuthConfig {
            registration_mode: McpOAuthRegistrationMode::Preregistered,
            client_id: Some("issued-client".into()),
            client_metadata_url: Some("https://client.example/oauth.json".into()),
            ..McpOAuthConfig::default()
        };
        let request = authorization_request(
            &preregistered,
            &oauth_redirect_uri(&preregistered),
            Some(SECRET_CANARY),
        );
        assert_eq!(request.client_id.as_deref(), Some("issued-client"));
        assert_eq!(request.client_secret.as_deref(), Some(SECRET_CANARY));
        assert!(request.client_metadata_url.is_none());
        assert_eq!(request.redirect_uri, "http://127.0.0.1/oauth/callback");

        let mut credential_url = McpOAuthConfig {
            endpoints: McpOAuthEndpoints {
                resource: Some("https://user:super-secret-password@example.test/mcp".into()),
                ..McpOAuthEndpoints::default()
            },
            ..McpOAuthConfig::default()
        };
        let error = credential_url.normalize().unwrap_err();
        assert_eq!(error, McpOAuthConfigError::InvalidEndpoint);
        assert!(!error.to_string().contains("super-secret-password"));
    }

    #[tokio::test]
    async fn challenge_resource_binding_is_reused_on_refresh() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let token_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_task = Arc::clone(&paths);
        let bodies_task = Arc::clone(&token_bodies);
        let server = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let paths_task = Arc::clone(&paths_task);
                let bodies_task = Arc::clone(&bodies_task);
                tokio::spawn(async move {
                    let _ = answer_oauth_fixture(socket, port, paths_task, bodies_task).await;
                });
            }
        });
        let _stop = AbortOnDrop(server);

        let origin = format!("http://127.0.0.1:{port}");
        let endpoint = format!("{origin}/mcp?transport=stream");
        let resource = format!("{origin}/mcp");
        let challenge = format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{origin}/.well-known/oauth-protected-resource\", scope=\"mcp:read mcp:admin\""
        );
        let redirect = format!("{origin}/oauth/callback");
        let manager = AuthorizationManager::new(&endpoint).await.unwrap();
        let resolution = manager
            .resolve_metadata_from_challenge(Some(&challenge))
            .await
            .unwrap();
        assert_eq!(
            resolution.source,
            AuthorizationMetadataSource::ProtectedResourceMetadata
        );

        let mut config = McpOAuthConfig {
            auth_mode: McpAuthMode::OAuth,
            registration_mode: McpOAuthRegistrationMode::ClientMetadata,
            client_id: Some("issued-client".into()),
            client_metadata_url: Some("https://client.example/oauth.json".into()),
            scopes: vec!["mcp:read".into()],
            ..McpOAuthConfig::default()
        };
        record_oauth_discovery(
            &mut config,
            &endpoint,
            Some(&challenge),
            &redirect,
            &resolution.metadata,
        )
        .await
        .unwrap();
        let prm_url = format!("{origin}/.well-known/oauth-protected-resource");
        assert_eq!(config.scopes, vec!["mcp:read".to_owned()]);
        assert_eq!(
            config.endpoints.resource.as_deref(),
            Some(resource.as_str())
        );
        assert_eq!(
            config.endpoints.protected_resource_metadata_url.as_deref(),
            Some(prm_url.as_str())
        );
        assert_eq!(
            config
                .endpoints
                .authorization_server_metadata_url
                .as_deref(),
            Some(origin.as_str())
        );
        assert_eq!(config.redirect_uri.as_deref(), Some(redirect.as_str()));
        let persisted = serde_json::to_string(&config).unwrap();
        assert!(!persisted.contains("mcp:admin"));
        assert!(!persisted.contains("access_token"));
        assert!(!persisted.contains("refresh_token"));
        assert!(!persisted.contains("client_secret"));
        assert!(persisted.contains("\"resource\":"));

        let root = std::env::temp_dir().join(format!(
            "termul-oauth-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let backend = Arc::new(MemoryBackend::default());
        let _guard =
            credentials::override_backend(Arc::clone(&backend) as Arc<dyn CredentialBackend>);
        let mut secrets = McpServerSecrets::default();
        secrets.set_oauth(Some(
            OAuthSecret::try_new(OAuthSecretInput {
                client_id: "https://client.example/oauth.json".into(),
                client_secret: None,
                access_token: "resource-bound-access".into(),
                refresh_token: Some("resource-bound-refresh".into()),
                token_type: "Bearer".into(),
                scope: Some("mcp:read".into()),
                issued_at: Some(1_700_000_000),
                expires_at: Some(1_700_003_600),
                issuer: Some(origin.clone()),
            })
            .unwrap(),
        ));
        McpCredentialStore::save(&root, "discovery-server", &secrets).unwrap();
        let transport = configured_authorization_manager(
            &endpoint,
            &root,
            "discovery-server",
            &config,
            &oauth_redirect_uri(&config),
        )
        .await
        .unwrap();
        transport.refresh_token().await.unwrap();
        let bodies = token_bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "token paths: {:?}", paths.lock().unwrap());
        let fields = url::form_urlencoded::parse(bodies[0].as_bytes())
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            fields.get("resource").map(String::as_str),
            Some(resource.as_str())
        );
        assert!(!bodies[0].contains("transport%3Dstream"));
        assert!(!bodies[0].contains("transport=stream"));
        let _ = std::fs::remove_dir_all(&root);
    }

    async fn answer_oauth_fixture(
        mut socket: tokio::net::TcpStream,
        port: u16,
        paths: Arc<Mutex<Vec<String>>>,
        token_bodies: Arc<Mutex<Vec<String>>>,
    ) -> std::io::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let count =
                tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut buffer))
                    .await
                    .map_err(std::io::Error::other)??;
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request[..end]).to_string();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                let header_end = end + 4;
                while request.len() < header_end + length {
                    let count = socket.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let body = String::from_utf8_lossy(
                    &request[header_end..(header_end + length).min(request.len())],
                )
                .to_string();
                let mut parts = header.lines().next().unwrap_or("").split_whitespace();
                let method = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("");
                let path = url::Url::parse(target)
                    .ok()
                    .map(|url| url.path().to_owned())
                    .unwrap_or_else(|| target.split('?').next().unwrap_or(target).to_owned());
                paths.lock().expect("paths").push(path.clone());
                let origin = format!("http://127.0.0.1:{port}");
                let response = if method == "GET" && path == "/.well-known/oauth-protected-resource"
                {
                    fixture_json(&format!(
                        r#"{{"resource":"{origin}/mcp","authorization_servers":["{origin}"],"scopes_supported":["mcp:read","mcp:admin","offline_access"]}}"#
                    ))
                } else if method == "GET"
                    && (path == "/.well-known/oauth-authorization-server"
                        || path == "/.well-known/openid-configuration")
                {
                    fixture_json(&format!(
                        r#"{{"issuer":"{origin}","authorization_endpoint":"{origin}/authorize","token_endpoint":"{origin}/token","registration_endpoint":"{origin}/register","scopes_supported":["mcp:read","mcp:admin","offline_access"],"response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"client_id_metadata_document_supported":true}}"#
                    ))
                } else if method == "POST" && path == "/token" {
                    token_bodies.lock().expect("token bodies").push(body);
                    fixture_json(
                        r#"{"access_token":"resource-bound-access-2","token_type":"Bearer","expires_in":3600,"refresh_token":"resource-bound-refresh-2"}"#,
                    )
                } else {
                    fixture_json_status("404 Not Found", "{}")
                };
                socket.write_all(response.as_bytes()).await?;
                return Ok(());
            }
            if request.len() > 65_536 {
                break;
            }
        }
        Ok(())
    }

    fn fixture_json(body: &str) -> String {
        fixture_json_status("200 OK", body)
    }

    fn fixture_json_status(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    struct AbortOnDrop(tokio::task::JoinHandle<()>);

    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    impl McpServerSecrets {
        fn oauth_mut_for_test(&mut self) -> &mut OAuthSecret {
            self.oauth.as_mut().expect("oauth")
        }
    }
}
