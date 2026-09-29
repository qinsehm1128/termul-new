//! AI channels: external model APIs Se Manager itself calls (for example to
//! describe MCP servers for agents).
//!
//! A channel is one provider endpoint speaking one wire protocol — OpenAI Chat
//! Completions, OpenAI Responses or Anthropic Messages — with the models it
//! serves. Several channels may speak the same protocol with different URLs
//! and keys. API keys never enter the document: each channel's key lives in
//! the OS keychain under `ai/channel/<id>`.

pub mod client;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub use client::{AiError, ChatRequest};

pub const SCHEMA_VERSION: u16 = 2;
const FILE_NAME: &str = "ai-channels.json";
const MAX_CHANNELS: usize = 32;
const MAX_MODELS: usize = 256;

/// Wire protocol of a channel, named as pi names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiApi {
    /// `POST {base}/chat/completions`
    OpenaiCompletions,
    /// `POST {base}/responses`
    OpenaiResponses,
    /// `POST {base}/v1/messages`
    AnthropicMessages,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChannel {
    pub id: String,
    pub name: String,
    pub api: AiApi,
    pub base_url: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<String>,
    /// Extra request headers (not secrets: the key lives in the keychain).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

fn enabled() -> bool {
    true
}

/// Which model a feature uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelRef {
    pub channel_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPurposes {
    /// Writes the "what is this MCP server for" line agents read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_summary: Option<AiModelRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChannelsDocument {
    pub schema_version: u16,
    #[serde(default)]
    pub channels: Vec<AiChannel>,
    #[serde(default)]
    pub purposes: AiPurposes,
}

impl Default for AiChannelsDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            channels: Vec::new(),
            purposes: AiPurposes::default(),
        }
    }
}

impl AiChannelsDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported AI channels schema {}",
                self.schema_version
            ));
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(format!("at most {MAX_CHANNELS} channels"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for channel in &self.channels {
            if !is_channel_id(&channel.id) {
                return Err(format!("channel id `{}` is invalid", channel.id));
            }
            if !ids.insert(channel.id.as_str()) {
                return Err(format!("channel id `{}` is used twice", channel.id));
            }
            if channel.name.trim().is_empty() {
                return Err("every channel needs a name".into());
            }
            client::normalize_base_url(&channel.base_url)?;
            if channel.models.len() > MAX_MODELS
                || channel.models.iter().any(|model| model.trim().is_empty())
            {
                return Err(format!("channel `{}` has invalid models", channel.name));
            }
            for name in channel.headers.keys() {
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| format!("header `{name}` is invalid"))?;
            }
        }
        Ok(())
    }

    pub fn channel(&self, id: &str) -> Option<&AiChannel> {
        self.channels.iter().find(|channel| channel.id == id)
    }
}

/// `[A-Za-z][A-Za-z0-9._-]{0,63}` — also the keychain key suffix.
pub fn is_channel_id(id: &str) -> bool {
    let mut characters = id.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic())
        && id.len() <= 64
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

pub fn credential_key(channel_id: &str) -> String {
    format!("ai/channel/{channel_id}")
}

pub fn document_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(FILE_NAME)
}

pub fn load(app_data_dir: &Path) -> Result<Option<AiChannelsDocument>, String> {
    match std::fs::read(document_path(app_data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("AI channels file is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub fn save(app_data_dir: &Path, document: &AiChannelsDocument) -> Result<(), String> {
    document.validate()?;
    let bytes = serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?;
    crate::acp::atomic_file::replace(&document_path(app_data_dir), &bytes)
        .map_err(|error| error.to_string())
}

/// The channel, key and model configured for a purpose.
pub struct Resolved {
    pub channel: AiChannel,
    pub key: String,
    pub model: String,
}

pub fn resolve(
    document: &AiChannelsDocument,
    reference: Option<&AiModelRef>,
) -> Result<Resolved, AiError> {
    let reference = reference.ok_or(AiError::NotConfigured)?;
    let channel = document
        .channel(&reference.channel_id)
        .filter(|channel| channel.enabled)
        .ok_or(AiError::NotConfigured)?;
    let key = crate::keyring_get(&credential_key(&channel.id))
        .map_err(AiError::Transport)?
        .filter(|key| !key.is_empty())
        .ok_or(AiError::MissingKey)?;
    Ok(Resolved {
        channel: channel.clone(),
        key,
        model: reference.model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: &str) -> AiChannel {
        AiChannel {
            id: id.into(),
            name: "Relay".into(),
            api: AiApi::AnthropicMessages,
            base_url: "https://relay.test".into(),
            enabled: true,
            models: vec!["claude-sonnet-5".into()],
            headers: BTreeMap::new(),
        }
    }

    #[test]
    fn document_round_trips_with_pi_protocol_names() {
        let document = AiChannelsDocument {
            channels: vec![channel("relay")],
            purposes: AiPurposes {
                mcp_summary: Some(AiModelRef {
                    channel_id: "relay".into(),
                    model: "claude-sonnet-5".into(),
                }),
            },
            ..AiChannelsDocument::default()
        };
        let value = serde_json::to_value(&document).unwrap();
        assert_eq!(value["channels"][0]["api"], "anthropic-messages");
        assert_eq!(value["purposes"]["mcpSummary"]["channelId"], "relay");
        let back: AiChannelsDocument = serde_json::from_value(value).unwrap();
        assert_eq!(back, document);

        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &document).unwrap();
        assert_eq!(load(dir.path()).unwrap(), Some(document));
    }

    #[test]
    fn validation_rejects_duplicates_bad_ids_and_urls() {
        let mut document = AiChannelsDocument {
            channels: vec![channel("a"), channel("a")],
            ..AiChannelsDocument::default()
        };
        assert!(document.validate().unwrap_err().contains("twice"));
        document.channels = vec![channel("1bad")];
        assert!(document.validate().is_err());
        let mut bad_url = channel("a");
        bad_url.base_url = "ftp://relay.test".into();
        document.channels = vec![bad_url];
        assert!(document.validate().is_err());
    }
}
