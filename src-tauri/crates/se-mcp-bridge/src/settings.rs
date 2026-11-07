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

pub const SCHEMA_VERSION: u16 = 2;
pub const GATEWAY_PROTOCOL: &str = "se-mcp-gateway";
pub const GATEWAY_PROTOCOL_VERSION: u16 = 1;
pub const GATEWAY_IDENTITY_PATH: &str = "/mcp-core/identity";
pub const SOURCE_HEADER: &str = "x-se-mcp-source";
pub const PROTOCOL_VERSION_HEADER: &str = "x-se-mcp-protocol-version";

/// Immutable origin of every tool exposed by a gateway instance.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Internal,
    Aggregator,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Aggregator => "aggregator",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "internal" => Some(Self::Internal),
            "aggregator" => Some(Self::Aggregator),
            _ => None,
        }
    }

    pub const fn client_key(self) -> &'static str {
        match self {
            Self::Internal => "se-mcp",
            Self::Aggregator => "se-mcp-aggregator",
        }
    }

    pub const fn default_file_name(self) -> &'static str {
        match (self, cfg!(debug_assertions)) {
            (Self::Internal, false) => "mcp-gateway.json",
            (Self::Internal, true) => "mcp-gateway.dev.json",
            (Self::Aggregator, false) => "mcp-gateway-aggregator.json",
            (Self::Aggregator, true) => "mcp-gateway-aggregator.dev.json",
        }
    }
}

/// Public, credential-free gateway wire identity (not the MCP version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayIdentity {
    pub protocol: String,
    pub protocol_version: u16,
    pub source: Source,
}

impl GatewayIdentity {
    pub fn new(source: Source) -> Self {
        Self {
            protocol: GATEWAY_PROTOCOL.into(),
            protocol_version: GATEWAY_PROTOCOL_VERSION,
            source,
        }
    }
}

/// Legacy release file name. New code should use `Source::default_file_name`.
pub const DEFAULT_FILE_NAME: &str = "mcp-gateway.json";
pub const DEFAULT_WORKSPACE_DIR: &str = ".se-manager";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub schema_version: u16,
    pub source: Source,
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

impl<'de> Deserialize<'de> for GatewaySettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            schema_version: u16,
            #[serde(default, deserialize_with = "present_source")]
            source: Option<Source>,
            port: u16,
            token: String,
            executable: Option<PathBuf>,
            profile_root: Option<PathBuf>,
            log_file: Option<PathBuf>,
        }
        fn present_source<'de, D: serde::Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Source>, D::Error> {
            Source::deserialize(d).map(Some)
        }
        let wire = Wire::deserialize(deserializer)?;
        let invalid = || serde::de::Error::custom("MCP_SETTINGS_INVALID");
        let source = match wire.source {
            Some(value) => value,
            None if wire.schema_version == 1 => Source::Internal,
            None => return Err(invalid()),
        };
        if !matches!(wire.schema_version, 1 | SCHEMA_VERSION)
            || (wire.schema_version == 1 && source != Source::Internal)
            || wire.port == 0
            || wire.token.trim().is_empty()
        {
            return Err(invalid());
        }
        Ok(Self {
            schema_version: wire.schema_version,
            source,
            port: wire.port,
            token: wire.token,
            executable: wire.executable,
            profile_root: wire.profile_root,
            log_file: wire.log_file,
        })
    }
}

impl GatewaySettings {
    /// Read only. Legacy v1 is interpreted as internal, never rewritten here.
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)
            .map_err(|error| io::Error::new(error.kind(), "MCP_SETTINGS_READ_FAILED"))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "MCP_SETTINGS_INVALID"))
    }

    pub fn load_for_source(path: &Path, expected: Source) -> io::Result<Self> {
        let settings = Self::load(path)?;
        if settings.source != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP_SETTINGS_SOURCE_MISMATCH",
            ));
        }
        Ok(settings)
    }

    /// Write atomically, readable by the owner only.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut current = self.clone();
        current.schema_version = SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&current)
            .map_err(|_| io::Error::other("MCP_SETTINGS_INVALID"))?;
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
    default_path_for_source(Source::Internal)
}

pub fn default_path_for_source(source: Source) -> Option<PathBuf> {
    home_dir().map(|home| {
        home.join(DEFAULT_WORKSPACE_DIR)
            .join(source.default_file_name())
    })
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
            source: Source::Internal,
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
    fn legacy_settings_are_internal_and_read_only_until_explicit_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.json");
        let bytes = br#"{"schemaVersion":1,"port":3290,"token":"legacy-token"}"#;
        fs::write(&path, bytes).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let loaded = GatewaySettings::load_for_source(&path, Source::Internal).unwrap();
        assert_eq!(loaded.source, Source::Internal);
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.token, "legacy-token");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(GatewaySettings::load_for_source(&path, Source::Aggregator).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // A management lifecycle can deliberately migrate to a different temp file.
        let migrated = dir.path().join("migrated.json");
        loaded.save(&migrated).unwrap();
        let current = GatewaySettings::load_for_source(&migrated, Source::Internal).unwrap();
        assert_eq!(current.schema_version, SCHEMA_VERSION);
        assert_eq!(current.port, loaded.port);
        assert_eq!(current.token, loaded.token);
    }

    #[test]
    fn schema_source_matrix_is_strict() {
        for (version, source, valid) in [
            (1, None, true),
            (1, Some("internal"), true),
            (1, Some("aggregator"), false),
            (2, None, false),
            (2, Some("internal"), true),
            (2, Some("aggregator"), true),
            (0, Some("internal"), false),
            (3, Some("aggregator"), false),
            (2, Some("unknown"), false),
            (2, Some("Internal"), false),
        ] {
            let mut value = serde_json::json!({"schemaVersion":version,"port":3290,"token":"t"});
            if let Some(source) = source {
                value["source"] = source.into();
            }
            assert_eq!(
                serde_json::from_value::<GatewaySettings>(value).is_ok(),
                valid
            );
        }
        for version in [1, 2] {
            let value =
                serde_json::json!({"schemaVersion":version,"source":null,"port":3290,"token":"t"});
            assert!(serde_json::from_value::<GatewaySettings>(value).is_err());
        }
    }

    #[test]
    fn source_wire_names_keys_and_default_paths_are_distinct() {
        assert_eq!(Source::default(), Source::Internal);
        for source in [Source::Internal, Source::Aggregator] {
            assert_eq!(Source::parse(source.as_str()), Some(source));
            assert_eq!(serde_json::to_value(source).unwrap(), source.as_str());
            assert_eq!(
                default_path_for_source(source)
                    .unwrap()
                    .file_name()
                    .unwrap(),
                source.default_file_name()
            );
        }
        assert_eq!(Source::Internal.client_key(), "se-mcp");
        assert_eq!(Source::Aggregator.client_key(), "se-mcp-aggregator");
        assert_ne!(
            Source::Internal.default_file_name(),
            Source::Aggregator.default_file_name()
        );
        assert!(Source::parse("INTERNAL").is_none());
        let identity = serde_json::to_value(GatewayIdentity::new(Source::Aggregator)).unwrap();
        assert_eq!(
            identity,
            serde_json::json!({"protocol":"se-mcp-gateway","protocolVersion":1,"source":"aggregator"})
        );
        for field in ["protocol", "protocolVersion", "source"] {
            let mut missing = identity.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<GatewayIdentity>(missing).is_err());
        }
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
