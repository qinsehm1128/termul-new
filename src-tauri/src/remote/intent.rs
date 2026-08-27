//! Operator intent for desktop shared-live: last toggle and last publish surface.
//!
//! Persisted next to tunnel settings. This is the user's wish, not process
//! liveness — a crash with `wanted=true` restores the listener on next launch.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::acp::atomic_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PublishMode {
    #[default]
    Tunnel,
    Lan,
}

impl PublishMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tunnel => "tunnel",
            Self::Lan => "lan",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tunnel" => Some(Self::Tunnel),
            "lan" => Some(Self::Lan),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessIntent {
    pub wanted: bool,
    pub publish_mode: PublishMode,
}

impl Default for RemoteAccessIntent {
    fn default() -> Self {
        Self {
            wanted: false,
            publish_mode: PublishMode::Tunnel,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteAccessIntentStore {
    path: PathBuf,
}

impl RemoteAccessIntentStore {
    #[must_use]
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            path: app_data_dir.join("remote-tunnel").join("remote-access.json"),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn for_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<RemoteAccessIntent, String> {
        if !self.path.exists() {
            return Ok(RemoteAccessIntent::default());
        }
        let bytes = std::fs::read(&self.path)
            .map_err(|error| format!("failed to read remote access intent: {error}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("remote access intent is invalid: {error}"))
    }

    pub fn save(&self, intent: &RemoteAccessIntent) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(intent)
            .map_err(|error| format!("failed to serialize remote access intent: {error}"))?;
        atomic_file::replace(&self.path, &bytes)
            .map_err(|error| format!("failed to write remote access intent: {error}"))?;
        log::info!(
            target: "termul::remote::intent",
            "operation=intent_save wanted={} publish_mode={} stable_code=OK",
            intent.wanted,
            intent.publish_mode.as_str()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off_and_tunnel() {
        let intent = RemoteAccessIntent::default();
        assert!(!intent.wanted);
        assert_eq!(intent.publish_mode, PublishMode::Tunnel);
    }

    #[test]
    fn persist_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = RemoteAccessIntentStore::for_path(dir.path().join("remote-access.json"));
        let intent = RemoteAccessIntent {
            wanted: true,
            publish_mode: PublishMode::Lan,
        };
        store.save(&intent).unwrap();
        assert_eq!(store.load().unwrap(), intent);
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = RemoteAccessIntentStore::for_path(dir.path().join("missing.json"));
        assert_eq!(store.load().unwrap(), RemoteAccessIntent::default());
    }
}
