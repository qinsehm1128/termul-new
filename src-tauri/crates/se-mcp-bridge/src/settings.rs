//! The gateway settings file: where the gateway listens and how to reach it.
//!
//! Se Manager writes it; the gateway service reads it to bind, and the stdio
//! client reads it to connect (and to start the gateway when it is not
//! running). It holds the bearer token, so it is written owner-only.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u16 = 1;

/// Settings file the stdio client falls back to when given no `--config`.
pub const DEFAULT_FILE_NAME: &str = "mcp-gateway.json";
pub const DEFAULT_WORKSPACE_DIR: &str = ".se-manager";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub schema_version: u16,
    /// Loopback port the gateway binds.
    pub port: u16,
    /// Bearer token every gateway request must carry.
    pub token: String,
    /// Binary that runs the gateway (`<executable> --mcp-core`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    /// App data directory the gateway reads built-in state from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_root: Option<PathBuf>,
    /// Where the gateway writes its log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<PathBuf>,
}

impl GatewaySettings {
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        let settings: Self = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if settings.port == 0 || settings.token.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "gateway settings need a port and a token",
            ));
        }
        Ok(settings)
    }

    /// Write atomically, readable by the owner only.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        write_private(&temporary, &bytes)?;
        fs::rename(&temporary, path)
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// `~/.se-manager/mcp-gateway.json`.
pub fn default_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(DEFAULT_WORKSPACE_DIR).join(DEFAULT_FILE_NAME))
}

/// How the gateway exposes the aggregated servers; each mode has its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `<server>_tool_list` / `<server>_tool_call` per server.
    Grouped,
    /// Four fixed tools, mcp-router compatible.
    Entry,
    /// Every tool of every server.
    Direct,
}

impl Mode {
    pub const fn path(self) -> &'static str {
        match self {
            Self::Grouped => "/mcp",
            Self::Entry => "/mcp/entry",
            Self::Direct => "/mcp/aggregator",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Grouped => "grouped",
            Self::Entry => "entry",
            Self::Direct => "direct",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "grouped" => Some(Self::Grouped),
            "entry" => Some(Self::Entry),
            "direct" | "aggregator" => Some(Self::Direct),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GatewaySettings {
        GatewaySettings {
            schema_version: SCHEMA_VERSION,
            port: 3290,
            token: "se-mcp-token".into(),
            executable: Some("/Applications/Se Manager.app/Contents/MacOS/se-manager".into()),
            profile_root: None,
            log_file: None,
        }
    }

    #[test]
    fn saves_owner_only_and_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp-gateway.json");
        sample().save(&path).unwrap();
        assert_eq!(GatewaySettings::load(&path).unwrap(), sample());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn a_file_without_port_or_token_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-gateway.json");
        fs::write(&path, r#"{"schemaVersion":1,"port":0,"token":"x"}"#).unwrap();
        assert!(GatewaySettings::load(&path).is_err());
        fs::write(&path, r#"{"schemaVersion":1,"port":3290,"token":" "}"#).unwrap();
        assert!(GatewaySettings::load(&path).is_err());
    }

    #[test]
    fn modes_round_trip_through_their_names() {
        for mode in [Mode::Grouped, Mode::Entry, Mode::Direct] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse("aggregator"), Some(Mode::Direct));
        assert_eq!(Mode::parse("other"), None);
    }
}
