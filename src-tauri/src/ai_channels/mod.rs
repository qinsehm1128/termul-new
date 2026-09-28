//! Runtime-neutral AI channel, credential, analysis, and Fx contracts.
//!
//! These types describe redacted configuration and status. API keys and other
//! secrets are not representable; credential fields are opaque keyring
//! references plus a presence flag. This module does not perform network I/O
//! or keyring access.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod fx;
pub mod router;

pub use fx::{FxExecutionError, FxRuntime, FxRuntimeCapabilities};
pub use router::{AiCompletionRequest, AiCompletionResponse, AiRouter, AiRouterError};

pub const AI_CHANNELS_SCHEMA_VERSION: u16 = 1;
pub const AI_ANALYSIS_SCHEMA_VERSION: u16 = 1;
pub const FX_CAPABILITY_SCHEMA_VERSION: u16 = 1;
pub const AI_ID_MAX_LENGTH: usize = 64;
pub const AI_DISPLAY_NAME_MAX_LENGTH: usize = 128;
pub const AI_MODEL_ID_MAX_LENGTH: usize = 256;
pub const AI_URL_MAX_LENGTH: usize = 2048;
pub const AI_CREDENTIAL_REF_MAX_LENGTH: usize = 256;
pub const AI_CHANNELS_MAX: usize = 32;
pub const AI_MODELS_PER_CHANNEL_MAX: usize = 32;
pub const AI_PROFILES_MAX: usize = 128;
pub const AI_ROUTE_PROFILES_MAX: usize = 8;
pub const AI_MAX_ATTEMPTS_LIMIT: u32 = 8;
pub const AI_TIMEOUT_MS_MAX: u32 = 120_000;
pub const AI_MAX_OUTPUT_TOKENS_LIMIT: u32 = 1_000_000;

const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiContractError {
    InvalidJson,
    ForbiddenCredentialField,
    InvalidProvider,
    InvalidEndpoint,
    InvalidId,
    InvalidDocument,
    StaleRevision,
}

impl fmt::Display for AiContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJson => "AI contract JSON is invalid",
            Self::ForbiddenCredentialField => "AI contract contains a forbidden credential field",
            Self::InvalidProvider => "AI provider is invalid",
            Self::InvalidEndpoint => "AI endpoint is invalid",
            Self::InvalidId => "AI id is invalid",
            Self::InvalidDocument => "AI contract document is invalid",
            Self::StaleRevision => "AI contract revision is stale",
        })
    }
}

impl std::error::Error for AiContractError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AiProviderKind {
    VercelGateway,
    OpenAiCompatible,
    OpenRouter,
    Ollama,
    Vllm,
    Custom,
}

impl AiProviderKind {
    fn parse(value: &str) -> Result<Self, AiContractError> {
        match value {
            "vercelGateway" => Ok(Self::VercelGateway),
            "openAiCompatible" => Ok(Self::OpenAiCompatible),
            "openRouter" => Ok(Self::OpenRouter),
            "ollama" => Ok(Self::Ollama),
            "vllm" => Ok(Self::Vllm),
            "custom" => Ok(Self::Custom),
            _ => Err(AiContractError::InvalidProvider),
        }
    }

    fn requires_endpoint(self) -> bool {
        matches!(self, Self::OpenAiCompatible | Self::Vllm | Self::Custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiRoutePurpose {
    DescriptionAnalysis,
    FxRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiCredentialKind {
    Keyring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiCredentialState {
    Present,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiAnalysisState {
    Idle,
    Running,
    DraftReady,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiAnalysisErrorCode {
    MissingCredential,
    InvalidRoute,
    TimedOut,
    Cancelled,
    ProviderRejected,
    MalformedOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FxRuntimeKind {
    Wasm,
    Native,
    Server,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FxFallback {
    AiRouter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredentialRef {
    pub kind: AiCredentialKind,
    #[serde(rename = "ref")]
    pub reference: String,
    pub has_credential: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiChannel {
    pub id: String,
    pub display_name: String,
    #[serde(deserialize_with = "deserialize_provider")]
    pub provider: AiProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub enabled: bool,
    pub credential_ref: AiCredentialRef,
    pub model_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelCapabilities {
    pub description_analysis: bool,
    pub fx_runtime: bool,
    pub structured_output: bool,
    pub tool_calling: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiModelProfile {
    pub id: String,
    pub channel_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub capabilities: AiModelCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiRoute {
    pub purpose: AiRoutePurpose,
    pub profile_ids: Vec<String>,
    pub max_attempts: u32,
    pub timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiChannelsDocument {
    pub schema_version: u16,
    pub revision: u64,
    pub channels: Vec<AiChannel>,
    pub profiles: Vec<AiModelProfile>,
    pub routes: Vec<AiRoute>,
}

impl AiChannelsDocument {
    pub fn from_json(raw: &str) -> Result<Self, AiContractError> {
        let value = serde_json::from_str(raw).map_err(|_| AiContractError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, AiContractError> {
        inspect_closed(&value)?;
        let document: Self = serde_json::from_value(value).map_err(map_deserialize_error)?;
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), AiContractError> {
        if self.schema_version != AI_CHANNELS_SCHEMA_VERSION
            || self.revision == 0
            || self.revision > MAX_SAFE_INTEGER
            || self.channels.len() > AI_CHANNELS_MAX
            || self.profiles.len() > AI_PROFILES_MAX
        {
            return Err(AiContractError::InvalidDocument);
        }

        let mut channel_ids = BTreeSet::new();
        let mut models_by_channel = BTreeSet::new();
        for channel in &self.channels {
            if !channel_ids.insert(channel.id.clone()) {
                return Err(AiContractError::InvalidId);
            }
            validate_channel(channel)?;
            for model_id in &channel.model_ids {
                if !models_by_channel.insert(format!("{}::{model_id}", channel.id)) {
                    return Err(AiContractError::InvalidId);
                }
            }
        }

        let mut profile_ids = BTreeSet::new();
        for profile in &self.profiles {
            if !profile_ids.insert(profile.id.clone()) {
                return Err(AiContractError::InvalidId);
            }
            validate_profile(profile, &channel_ids, &models_by_channel)?;
        }

        let mut purposes = BTreeSet::new();
        for route in &self.routes {
            if !purposes.insert(route.purpose) {
                return Err(AiContractError::InvalidDocument);
            }
            validate_route(route, &profile_ids)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiCredentialStatus {
    pub channel_id: String,
    pub kind: AiCredentialKind,
    #[serde(rename = "ref")]
    pub reference: String,
    pub has_credential: bool,
    pub state: AiCredentialState,
}

impl AiCredentialStatus {
    pub fn from_json(raw: &str) -> Result<Self, AiContractError> {
        let value = serde_json::from_str(raw).map_err(|_| AiContractError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, AiContractError> {
        inspect_closed(&value)?;
        let status: Self = serde_json::from_value(value).map_err(map_deserialize_error)?;
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> Result<(), AiContractError> {
        if !is_safe_id(&self.channel_id, AI_ID_MAX_LENGTH) {
            return Err(AiContractError::InvalidId);
        }
        if !is_credential_ref(&self.reference)
            || self.has_credential != matches!(self.state, AiCredentialState::Present)
        {
            return Err(AiContractError::InvalidDocument);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiAnalysisStatus {
    pub schema_version: u16,
    pub purpose: AiRoutePurpose,
    pub state: AiAnalysisState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<AiAnalysisErrorCode>,
}

impl AiAnalysisStatus {
    pub fn from_json(raw: &str) -> Result<Self, AiContractError> {
        let value = serde_json::from_str(raw).map_err(|_| AiContractError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, AiContractError> {
        inspect_closed(&value)?;
        let status: Self = serde_json::from_value(value).map_err(map_deserialize_error)?;
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> Result<(), AiContractError> {
        if self.schema_version != AI_ANALYSIS_SCHEMA_VERSION
            || self.purpose != AiRoutePurpose::DescriptionAnalysis
        {
            return Err(AiContractError::InvalidDocument);
        }
        validate_optional_id(self.channel_id.as_deref())?;
        validate_optional_id(self.profile_id.as_deref())?;
        validate_optional_id(self.server_id.as_deref())?;
        if let Some(revision) = self.catalog_revision {
            require_positive_revision(revision)?;
        }
        if let Some(revision) = self.draft_revision {
            require_positive_revision(revision)?;
        }
        match self.state {
            AiAnalysisState::Idle if self.error_code.is_some() || self.draft_revision.is_some() => {
                Err(AiContractError::InvalidDocument)
            }
            AiAnalysisState::Failed | AiAnalysisState::Cancelled if self.error_code.is_none() => {
                Err(AiContractError::InvalidDocument)
            }
            AiAnalysisState::Cancelled
                if self.error_code != Some(AiAnalysisErrorCode::Cancelled) =>
            {
                Err(AiContractError::InvalidDocument)
            }
            AiAnalysisState::DraftReady if self.draft_revision.is_none() => {
                Err(AiContractError::InvalidDocument)
            }
            _ if self.state != AiAnalysisState::Cancelled
                && self.error_code == Some(AiAnalysisErrorCode::Cancelled) =>
            {
                Err(AiContractError::InvalidDocument)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FxCapabilityStatus {
    pub schema_version: u16,
    pub wasm: bool,
    pub jspi: bool,
    pub native: bool,
    pub server: bool,
    pub selected_runtime: FxRuntimeKind,
    pub fallback: FxFallback,
}

impl FxCapabilityStatus {
    pub fn from_json(raw: &str) -> Result<Self, AiContractError> {
        let value = serde_json::from_str(raw).map_err(|_| AiContractError::InvalidJson)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, AiContractError> {
        inspect_closed(&value)?;
        let status: Self = serde_json::from_value(value).map_err(map_deserialize_error)?;
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> Result<(), AiContractError> {
        if self.schema_version != FX_CAPABILITY_SCHEMA_VERSION {
            return Err(AiContractError::InvalidDocument);
        }
        let consistent = match self.selected_runtime {
            FxRuntimeKind::Wasm => self.wasm,
            FxRuntimeKind::Native => self.native,
            FxRuntimeKind::Server => self.server,
            FxRuntimeKind::Unavailable => true,
        };
        if consistent {
            Ok(())
        } else {
            Err(AiContractError::InvalidDocument)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiRevisionFence {
    last_accepted: Option<u64>,
}

impl AiRevisionFence {
    pub fn last_accepted(&self) -> Option<u64> {
        self.last_accepted
    }

    pub fn accept(&mut self, revision: u64) -> Result<(), AiContractError> {
        if revision == 0 || revision > MAX_SAFE_INTEGER {
            return Err(AiContractError::InvalidDocument);
        }
        if self.last_accepted.is_some_and(|last| revision <= last) {
            return Err(AiContractError::StaleRevision);
        }
        self.last_accepted = Some(revision);
        Ok(())
    }
}

fn deserialize_provider<'de, D>(deserializer: D) -> Result<AiProviderKind, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    AiProviderKind::parse(&value).map_err(|_| serde::de::Error::custom("invalid provider"))
}

fn map_deserialize_error(error: serde_json::Error) -> AiContractError {
    if error.to_string().contains("invalid provider") {
        AiContractError::InvalidProvider
    } else {
        AiContractError::InvalidDocument
    }
}

fn validate_channel(channel: &AiChannel) -> Result<(), AiContractError> {
    if !is_safe_id(&channel.id, AI_ID_MAX_LENGTH) {
        return Err(AiContractError::InvalidId);
    }
    if !is_display_text(
        &channel.display_name,
        AI_DISPLAY_NAME_MAX_LENGTH,
        false,
        false,
    ) || !is_credential_ref(&channel.credential_ref.reference)
        || channel.model_ids.len() > AI_MODELS_PER_CHANNEL_MAX
    {
        return Err(AiContractError::InvalidDocument);
    }
    let mut models = BTreeSet::new();
    for model_id in &channel.model_ids {
        if !is_model_id(model_id) {
            return Err(AiContractError::InvalidId);
        }
        if !models.insert(model_id) {
            return Err(AiContractError::InvalidId);
        }
    }
    match &channel.base_url {
        Some(url) => validate_endpoint(url)?,
        None if channel.provider.requires_endpoint() => {
            return Err(AiContractError::InvalidEndpoint);
        }
        None => {}
    }
    Ok(())
}

fn validate_profile(
    profile: &AiModelProfile,
    channel_ids: &BTreeSet<String>,
    models_by_channel: &BTreeSet<String>,
) -> Result<(), AiContractError> {
    if !is_safe_id(&profile.id, AI_ID_MAX_LENGTH)
        || !is_safe_id(&profile.channel_id, AI_ID_MAX_LENGTH)
    {
        return Err(AiContractError::InvalidId);
    }
    if !is_model_id(&profile.model_id) {
        return Err(AiContractError::InvalidId);
    }
    if !channel_ids.contains(&profile.channel_id)
        || !models_by_channel.contains(&format!("{}::{}", profile.channel_id, profile.model_id))
    {
        return Err(AiContractError::InvalidDocument);
    }
    if let Some(tokens) = profile.max_output_tokens {
        if tokens == 0 || tokens > AI_MAX_OUTPUT_TOKENS_LIMIT {
            return Err(AiContractError::InvalidDocument);
        }
    }
    if let Some(temperature) = profile.temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(AiContractError::InvalidDocument);
        }
    }
    Ok(())
}

fn validate_route(route: &AiRoute, profile_ids: &BTreeSet<String>) -> Result<(), AiContractError> {
    if route.profile_ids.is_empty()
        || route.profile_ids.len() > AI_ROUTE_PROFILES_MAX
        || route.max_attempts == 0
        || route.max_attempts > AI_MAX_ATTEMPTS_LIMIT
        || route.timeout_ms == 0
        || route.timeout_ms > AI_TIMEOUT_MS_MAX
    {
        return Err(AiContractError::InvalidDocument);
    }
    let mut seen = BTreeSet::new();
    for profile_id in &route.profile_ids {
        if !is_safe_id(profile_id, AI_ID_MAX_LENGTH) || !seen.insert(profile_id) {
            return Err(AiContractError::InvalidId);
        }
        if !profile_ids.contains(profile_id) {
            return Err(AiContractError::InvalidDocument);
        }
    }
    Ok(())
}

fn validate_optional_id(value: Option<&str>) -> Result<(), AiContractError> {
    match value {
        Some(id) if !is_safe_id(id, AI_ID_MAX_LENGTH) => Err(AiContractError::InvalidId),
        _ => Ok(()),
    }
}

fn require_positive_revision(revision: u64) -> Result<(), AiContractError> {
    if revision == 0 || revision > MAX_SAFE_INTEGER {
        Err(AiContractError::InvalidDocument)
    } else {
        Ok(())
    }
}

fn validate_endpoint(raw: &str) -> Result<(), AiContractError> {
    if raw.is_empty() || raw.len() > AI_URL_MAX_LENGTH || raw.chars().any(char::is_whitespace) {
        return Err(AiContractError::InvalidEndpoint);
    }
    let parsed = url::Url::parse(raw).map_err(|_| AiContractError::InvalidEndpoint)?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(AiContractError::InvalidEndpoint);
    }
    let host = parsed.host_str().ok_or(AiContractError::InvalidEndpoint)?;
    let loopback = is_loopback(host);
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err(AiContractError::InvalidEndpoint),
    }
    for (key, _) in parsed.query_pairs() {
        if is_forbidden_credential_key(&key) {
            return Err(AiContractError::InvalidEndpoint);
        }
    }
    Ok(())
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn is_safe_id(value: &str, max_length: usize) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && value.len() <= max_length
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn is_model_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= AI_MODEL_ID_MAX_LENGTH
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '/' | '-'))
}

fn is_credential_ref(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= AI_CREDENTIAL_REF_MAX_LENGTH
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | ':' | '-'))
}

fn is_display_text(value: &str, max_chars: usize, allow_empty: bool, allow_newlines: bool) -> bool {
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

fn inspect_closed(value: &Value) -> Result<(), AiContractError> {
    inspect_closed_at(value, 0)
}

fn inspect_closed_at(value: &Value, depth: usize) -> Result<(), AiContractError> {
    if depth > 32 {
        return Err(AiContractError::InvalidDocument);
    }
    match value {
        Value::Null => Err(AiContractError::InvalidDocument),
        Value::String(text) if text.len() > 100_000 => Err(AiContractError::InvalidDocument),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Ok(()),
        Value::Array(items) => {
            if items.len() > 10_000 {
                return Err(AiContractError::InvalidDocument);
            }
            items
                .iter()
                .try_for_each(|item| inspect_closed_at(item, depth + 1))
        }
        Value::Object(map) => {
            if map.len() > 256 {
                return Err(AiContractError::InvalidDocument);
            }
            for (key, child) in map {
                if is_forbidden_credential_key(key) {
                    return Err(AiContractError::ForbiddenCredentialField);
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

    const CANARY: &str = "sk-canary-secret";

    #[test]
    fn round_trips_shared_fixtures_and_omits_optional_fields() {
        let full = AiChannelsDocument::from_json(include_str!(
            "../../../src/shared/types/fixtures/ai-channels.document.json"
        ))
        .unwrap();
        assert_eq!(full.channels[0].provider, AiProviderKind::VercelGateway);
        assert_eq!(
            full.channels[0].base_url.as_deref(),
            Some("https://gateway.example.test/v1")
        );
        assert_eq!(
            full.channels[0].credential_ref.reference,
            "ai/channel/gateway"
        );
        assert_eq!(full.profiles[0].temperature, Some(0.5));
        assert_eq!(
            full.routes
                .iter()
                .map(|route| route.purpose)
                .collect::<Vec<_>>(),
            vec![
                AiRoutePurpose::DescriptionAnalysis,
                AiRoutePurpose::FxRuntime
            ]
        );
        let encoded = serde_json::to_value(&full).unwrap();
        assert_eq!(AiChannelsDocument::from_value(encoded).unwrap(), full);

        let minimal = AiChannelsDocument::from_json(include_str!(
            "../../../src/shared/types/fixtures/ai-channels.minimal.json"
        ))
        .unwrap();
        let encoded = serde_json::to_value(&minimal).unwrap();
        assert!(encoded["channels"][0].get("baseUrl").is_none());
        assert!(encoded["profiles"][0].get("maxOutputTokens").is_none());
        assert!(encoded["profiles"][0].get("temperature").is_none());
        assert_eq!(AiChannelsDocument::from_value(encoded).unwrap(), minimal);
    }

    #[test]
    fn rejects_invalid_provider_endpoint_and_id_without_echoing_secrets() {
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../src/shared/types/fixtures/ai-channels.document.json"
        ))
        .unwrap();
        value["channels"][0]["provider"] = json!("azure");
        assert_eq!(
            AiChannelsDocument::from_value(value.clone()).unwrap_err(),
            AiContractError::InvalidProvider
        );

        value["channels"][0]["provider"] = json!("custom");
        value["channels"][0]["baseUrl"] = json!(format!("https://user:{CANARY}@example.test/v1"));
        let error = AiChannelsDocument::from_value(value.clone()).unwrap_err();
        assert_eq!(error, AiContractError::InvalidEndpoint);
        assert!(!error.to_string().contains(CANARY));

        value["channels"][0]["baseUrl"] = json!("http://192.168.1.20/v1?api_key=sk-live");
        assert_eq!(
            AiChannelsDocument::from_value(value.clone()).unwrap_err(),
            AiContractError::InvalidEndpoint
        );
        value["channels"][0]
            .as_object_mut()
            .unwrap()
            .remove("baseUrl");
        assert_eq!(
            AiChannelsDocument::from_value(value.clone()).unwrap_err(),
            AiContractError::InvalidEndpoint
        );

        value["channels"][0]["provider"] = json!("ollama");
        value["channels"][0]["baseUrl"] = json!("http://[::1]:11434/v1");
        assert!(AiChannelsDocument::from_value(value.clone()).is_ok());
        value["channels"][0]["id"] = json!("not an id");
        assert_eq!(
            AiChannelsDocument::from_value(value).unwrap_err(),
            AiContractError::InvalidId
        );
    }

    #[test]
    fn rejects_credential_fields_on_the_persisted_document() {
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../src/shared/types/fixtures/ai-channels.document.json"
        ))
        .unwrap();
        value["channels"][0]["apiKey"] = json!(CANARY);
        let error = AiChannelsDocument::from_value(value).unwrap_err();
        assert_eq!(error, AiContractError::ForbiddenCredentialField);
        assert!(!error.to_string().contains(CANARY));
    }

    #[test]
    fn parses_credential_analysis_and_fx_status() {
        let status = AiCredentialStatus::from_json(include_str!(
            "../../../src/shared/types/fixtures/ai-credential-status.json"
        ))
        .unwrap();
        assert_eq!(status.state, AiCredentialState::Missing);
        assert!(!status.has_credential);

        let mut inconsistent: Value = serde_json::from_str(include_str!(
            "../../../src/shared/types/fixtures/ai-credential-status.json"
        ))
        .unwrap();
        inconsistent["hasCredential"] = json!(true);
        assert_eq!(
            AiCredentialStatus::from_value(inconsistent).unwrap_err(),
            AiContractError::InvalidDocument
        );

        let analysis = AiAnalysisStatus::from_json(include_str!(
            "../../../src/shared/types/fixtures/ai-analysis-status.json"
        ))
        .unwrap();
        assert_eq!(analysis.state, AiAnalysisState::DraftReady);
        let encoded = serde_json::to_value(&analysis).unwrap();
        assert!(encoded.get("errorCode").is_none());
        assert_eq!(AiAnalysisStatus::from_value(encoded).unwrap(), analysis);

        let fx = FxCapabilityStatus::from_json(include_str!(
            "../../../src/shared/types/fixtures/fx-capability.json"
        ))
        .unwrap();
        assert_eq!(fx.selected_runtime, FxRuntimeKind::Wasm);
        assert_eq!(fx.fallback, FxFallback::AiRouter);
        let mut invalid = serde_json::to_value(fx).unwrap();
        invalid["selectedRuntime"] = json!("native");
        assert_eq!(
            FxCapabilityStatus::from_value(invalid).unwrap_err(),
            AiContractError::InvalidDocument
        );
    }

    #[test]
    fn revision_fence_is_strict() {
        let mut fence = AiRevisionFence::default();
        assert!(fence.accept(1).is_ok());
        assert_eq!(fence.accept(1).unwrap_err(), AiContractError::StaleRevision);
        assert!(fence.accept(2).is_ok());
        assert_eq!(fence.last_accepted(), Some(2));
    }
}
