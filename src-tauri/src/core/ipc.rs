//! Local IPC primitives shared by the Terminal Core and ACP Core.
//!
//! The first core boundary is intentionally local-only. Unix uses an
//! owner-scoped Unix socket; Windows uses a named-pipe endpoint. The protocol
//! layer is transport-agnostic so both cores share framing, handshakes, and
//! error semantics instead of growing separate authentication stacks.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const CURRENT_PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoreRole {
    Gui,
    TerminalCore,
    AcpCore,
}

impl CoreRole {
    pub const fn endpoint_name(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::TerminalCore => "terminal-core",
            Self::AcpCore => "acp-core",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEndpoint {
    #[cfg(unix)]
    UnixSocket(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
}

impl CoreEndpoint {
    #[cfg(unix)]
    pub fn for_profile(profile_root: impl AsRef<Path>, role: CoreRole) -> Self {
        let runtime_root = profile_root.as_ref().join("core-runtime");
        Self::UnixSocket(runtime_root.join(format!("{}.sock", role.endpoint_name())))
    }

    #[cfg(windows)]
    pub fn for_profile(profile_root: impl AsRef<Path>, role: CoreRole) -> Self {
        let runtime_root = profile_root.as_ref().join("core-runtime");
        let name = runtime_root.join(role.endpoint_name());
        Self::NamedPipe(format!(r"\\.\pipe\{}", name.to_string_lossy()))
    }

    #[cfg(unix)]
    pub fn as_path(&self) -> Option<&Path> {
        let Self::UnixSocket(path) = self;
        Some(path)
    }

    #[cfg(not(unix))]
    pub fn as_path(&self) -> Option<&Path> {
        None
    }

    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::UnixSocket(path) => path.to_string_lossy().into_owned(),
            #[cfg(windows)]
            Self::NamedPipe(name) => name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    Io(String),
    InvalidFrame(String),
    InvalidHandshake(String),
    Unauthorized,
    UnsupportedPlatform,
    UnsupportedProtocol { offered: Vec<u16> },
    InvalidRequest(String),
}

impl CoreError {
    /// Message for command-layer mapping: keep the request detail (often a
    /// stable code) when present, else the generic client message.
    pub fn command_message(&self) -> String {
        match self {
            Self::InvalidRequest(detail) if !detail.is_empty() => detail.clone(),
            other => other.to_string(),
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "CORE_IPC_IO",
            Self::InvalidFrame(_) => "CORE_IPC_INVALID_FRAME",
            Self::InvalidHandshake(_) => "CORE_IPC_INVALID_HANDSHAKE",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::UnsupportedPlatform => "CORE_IPC_UNSUPPORTED_PLATFORM",
            Self::UnsupportedProtocol { .. } => "CORE_IPC_UNSUPPORTED_PROTOCOL",
            Self::InvalidRequest(_) => "CORE_IPC_INVALID_REQUEST",
        }
    }

    pub fn client_message(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::UnsupportedProtocol { .. } => "unsupported core protocol",
            Self::UnsupportedPlatform => "local core IPC is unsupported on this platform",
            Self::Io(_) => "core IPC unavailable",
            Self::InvalidFrame(_) | Self::InvalidHandshake(_) | Self::InvalidRequest(_) => {
                "invalid core IPC request"
            }
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(detail) if !detail.is_empty() => {
                write!(formatter, "{}: {detail}", self.code())
            }
            other => write!(formatter, "{}: {}", other.code(), other.client_message()),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreHello {
    pub role: CoreRole,
    pub protocol_versions: Vec<u16>,
    pub client_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreHelloAck {
    pub role: CoreRole,
    pub protocol_version: u16,
}

pub fn negotiate_protocol(offered: &[u16]) -> Result<u16, CoreError> {
    if offered.contains(&CURRENT_PROTOCOL_VERSION) {
        Ok(CURRENT_PROTOCOL_VERSION)
    } else {
        Err(CoreError::UnsupportedProtocol {
            offered: offered.to_vec(),
        })
    }
}

pub fn validate_hello(
    hello: &CoreHello,
    expected_role: CoreRole,
) -> Result<CoreHelloAck, CoreError> {
    if hello.role != expected_role {
        return Err(CoreError::Unauthorized);
    }

    Ok(CoreHelloAck {
        role: expected_role,
        protocol_version: negotiate_protocol(&hello.protocol_versions)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CoreErrorPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreEvent {
    pub topic: String,
    #[serde(default)]
    pub payload: Value,
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>, CoreError>
where
    R: AsyncRead + Unpin,
{
    let length = reader.read_u32().await.map_err(CoreError::from)? as usize;
    if length > MAX_FRAME_BYTES {
        return Err(CoreError::InvalidFrame(format!(
            "frame exceeds {} bytes",
            MAX_FRAME_BYTES
        )));
    }

    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(CoreError::from)?;
    Ok(payload)
}

pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), CoreError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME_BYTES {
        return Err(CoreError::InvalidFrame(format!(
            "frame exceeds {} bytes",
            MAX_FRAME_BYTES
        )));
    }

    writer
        .write_u32(payload.len() as u32)
        .await
        .map_err(CoreError::from)?;
    writer.write_all(payload).await.map_err(CoreError::from)?;
    writer.flush().await.map_err(CoreError::from)?;
    Ok(())
}

pub async fn read_json_frame<R, T>(reader: &mut R) -> Result<T, CoreError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let payload = read_frame(reader).await?;
    serde_json::from_slice(&payload)
        .map_err(|error| CoreError::InvalidFrame(format!("invalid JSON frame: {error}")))
}

pub async fn write_json_frame<W, T>(writer: &mut W, value: &T) -> Result<(), CoreError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(|error| {
        CoreError::InvalidFrame(format!("failed to encode JSON frame: {error}"))
    })?;
    write_frame(writer, &payload).await
}

#[cfg(unix)]
pub fn prepare_runtime_dir(endpoint: &CoreEndpoint) -> Result<(), CoreError> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let Some(socket_path) = endpoint.as_path() else {
        return Err(CoreError::UnsupportedPlatform);
    };
    let parent = socket_path
        .parent()
        .ok_or_else(|| CoreError::InvalidRequest("core endpoint has no parent".to_string()))?;
    fs::create_dir_all(parent).map_err(CoreError::from)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(CoreError::from)?;
    Ok(())
}

#[cfg(windows)]
pub fn prepare_runtime_dir(_endpoint: &CoreEndpoint) -> Result<(), CoreError> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn prepare_runtime_dir(_endpoint: &CoreEndpoint) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform)
}

#[cfg(unix)]
pub fn remove_stale_socket(endpoint: &CoreEndpoint) -> Result<bool, CoreError> {
    use std::fs;
    use std::os::unix::fs::FileTypeExt;

    let Some(path) = endpoint.as_path() else {
        return Err(CoreError::UnsupportedPlatform);
    };
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(false);
    };
    if !metadata.file_type().is_socket() {
        return Err(CoreError::InvalidRequest(
            "core endpoint exists and is not a socket".to_string(),
        ));
    }
    fs::remove_file(path).map_err(CoreError::from)?;
    Ok(true)
}

#[cfg(not(unix))]
pub fn remove_stale_socket(_endpoint: &CoreEndpoint) -> Result<bool, CoreError> {
    Err(CoreError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn negotiates_current_protocol() {
        assert_eq!(negotiate_protocol(&[99, CURRENT_PROTOCOL_VERSION]), Ok(1));
    }

    #[test]
    fn rejects_unknown_protocol() {
        assert_eq!(
            negotiate_protocol(&[99]),
            Err(CoreError::UnsupportedProtocol { offered: vec![99] })
        );
    }

    #[test]
    fn rejects_wrong_role_without_leaking_endpoint_state() {
        let hello = CoreHello {
            role: CoreRole::Gui,
            protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
            client_name: "test".to_string(),
        };
        assert_eq!(
            validate_hello(&hello, CoreRole::TerminalCore),
            Err(CoreError::Unauthorized)
        );
        assert_eq!(CoreError::Unauthorized.client_message(), "unauthorized");
    }

    #[tokio::test]
    async fn round_trips_length_delimited_frames() {
        let (mut writer, mut reader) = duplex(128);
        let payload = b"hello core".to_vec();
        let expected = payload.clone();
        let write = tokio::spawn(async move {
            write_frame(&mut writer, &payload).await.unwrap();
        });
        assert_eq!(read_frame(&mut reader).await.unwrap(), expected);
        write.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_frames_before_allocating_payload() {
        let (mut writer, mut reader) = duplex(16);
        let write = tokio::spawn(async move {
            writer
                .write_u32((MAX_FRAME_BYTES as u32) + 1)
                .await
                .unwrap();
        });
        assert!(matches!(
            read_frame(&mut reader).await,
            Err(CoreError::InvalidFrame(_))
        ));
        write.await.unwrap();
    }

    #[test]
    fn endpoint_is_profile_scoped() {
        let endpoint = CoreEndpoint::for_profile("/tmp/termul-test", CoreRole::AcpCore);
        assert!(endpoint.describe().contains("acp-core"));
    }
}
