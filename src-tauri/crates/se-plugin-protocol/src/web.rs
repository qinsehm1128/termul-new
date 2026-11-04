//! Versioned web UI transport contracts, deliberately independent of a webview runtime.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const WEB_UI_CAPABILITY: &str = "plugins.web-ui.v1";
pub const PLUGIN_WEB_UI_CAPABILITY: &str = WEB_UI_CAPABILITY;
pub const WEB_OPEN_METHOD: &str = "plugins.web.open";
pub const WEB_REQUEST_METHOD: &str = "plugins.web.request";
pub const WEB_READ_METHOD: &str = "plugins.web.read";
pub const WEB_CLOSE_METHOD: &str = "plugins.web.close";
pub const WEB_ASSET_METHOD: &str = "plugins.web.asset";
pub const HOST_UI_SERVICE: &str = "se.host.ui";
pub const HOST_UI_METHOD_POST_MESSAGE: &str = "postMessage";
pub const WEB_BRIDGE_CHANNEL: &str = "se-plugin-ui";
pub const WEB_SHIM_VERSION: u32 = 1;
pub const MAX_WEB_SESSIONS: usize = 16;
pub const MAX_WEB_SESSIONS_PER_PLUGIN: usize = 8;
pub const MAX_WEB_SESSIONS_PER_VIEW: usize = 2;
pub const WEB_SESSION_LEASE_MS: u64 = 15_000;
pub const WEB_POLL_INTERVAL_MS: u64 = 250;
pub const MAX_WEB_PENDING_REQUESTS: usize = 32;
pub const WEB_RATE_PER_SECOND: usize = 20;
pub const WEB_RATE_BURST: usize = 40;
pub const WEB_REQUEST_TIMEOUT_MS: u64 = 30_000;
pub const MAX_WEB_REQUEST_ID_CHARS: usize = 64;
pub const MAX_WEB_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_WEB_ASSET_REPLY_BYTES: usize = 6 * 1024 * 1024;
pub const MAX_WEB_QUEUED_EVENTS: usize = 64;
pub const MAX_WEB_READ_EVENTS: usize = 16;
pub const MAX_WEB_QUEUED_BYTES: usize = 256 * 1024;
pub const WEB_SHIM_METHODS: &[&str] = &[
    "invoke",
    "view.dispatch",
    "config.get",
    "config.set",
    "context.snapshot",
    "storage.get",
    "storage.set",
    "storage.delete",
    "feedback.notification.upsert",
    "feedback.notification.close",
    "feedback.progress.upsert",
    "theme.get",
];
pub const WEB_SHIM_API_METHODS: &[&str] = &[
    "se.invoke",
    "se.view.dispatch",
    "se.config.get",
    "se.config.set",
    "se.config.onChange",
    "se.context.snapshot",
    "se.context.onEvent",
    "se.storage.get",
    "se.storage.set",
    "se.storage.delete",
    "se.feedback.notification.upsert",
    "se.feedback.notification.close",
    "se.feedback.progress.upsert",
    "se.theme.get",
    "se.theme.onChange",
    "se.onMessage",
    "se.onInvalidate",
    "se.onResyncRequired",
];
/// Trusted plugin pages may use inline/eval scripts and network resources, but not host IPC.
pub const WEB_UI_CSP: &str = "default-src 'self' se-plugin: http: https: ws: wss: data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' se-plugin: http: https: data: blob:; style-src 'self' 'unsafe-inline' se-plugin: http: https: data: blob:; connect-src http: https: ws: wss: data: blob:; img-src 'self' se-plugin: http: https: ws: wss: data: blob:; font-src 'self' se-plugin: http: https: ws: wss: data: blob:; media-src 'self' se-plugin: http: https: ws: wss: data: blob:; frame-src http: https: ws: wss: data: blob:; object-src 'none'; base-uri 'none'; form-action 'none'";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewKind {
    #[default]
    #[serde(rename = "uiTree")]
    UiTree,
    #[serde(rename = "web")]
    Web,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WebAllow {
    #[serde(rename = "commands")]
    Commands,
    #[serde(rename = "view.dispatch")]
    ViewDispatch,
    #[serde(rename = "config.read")]
    ConfigRead,
    #[serde(rename = "config.write")]
    ConfigWrite,
    #[serde(rename = "context.read")]
    ContextRead,
    #[serde(rename = "context.events")]
    ContextEvents,
    #[serde(rename = "storage.read")]
    StorageRead,
    #[serde(rename = "storage.write")]
    StorageWrite,
    #[serde(rename = "feedback")]
    Feedback,
}
impl WebAllow {
    pub fn required_service(self) -> Option<&'static str> {
        match self {
            Self::ViewDispatch => Some("se.host.ui"),
            Self::ContextRead => Some("se.host.context"),
            Self::ContextEvents => Some("se.host.events"),
            Self::StorageRead | Self::StorageWrite => Some("se.host.storage"),
            Self::Feedback => Some("se.host.feedback"),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebOpenRequest {
    pub id: String,
    pub view_id: String,
    pub generation: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebOpenReply {
    pub boot_id: String,
    pub plugin_id: String,
    pub view_id: String,
    pub generation: String,
    pub session_id: String,
    pub nonce: String,
    pub package_digest: String,
    pub entry: String,
    pub allow: Vec<WebAllow>,
    pub shim_version: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebRequest {
    pub session_id: String,
    pub request_id: String,
    pub method: String,
    #[serde(deserialize_with = "required_json")]
    pub params: Value,
}
/// RPC success values are arbitrary bounded JSON; RPC failures use PluginError.
pub type WebRequestReply = Value;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebReadRequest {
    pub session_id: String,
    pub after_sequence: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WebEventType {
    Message,
    Invalidate,
    ConfigChanged,
    ContextEvent,
    ThemeChanged,
    ResyncRequired,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebReadEvent {
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: WebEventType,
    #[serde(deserialize_with = "required_json")]
    pub payload: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebReadReply {
    pub sequence: u64,
    pub resync_required: bool,
    pub events: Vec<WebReadEvent>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebCloseRequest {
    pub session_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebCloseReply {
    pub closed: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebAssetRequest {
    pub session_id: String,
    pub path: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebAssetReply {
    pub mime: String,
    pub sha256: String,
    pub size: u64,
    pub data_base64: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUiPostMessageRequest {
    pub view_id: String,
    #[serde(deserialize_with = "required_json")]
    pub payload: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostUiPostMessageReply {
    pub delivered: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeSnapshot {
    pub revision: u64,
    pub appearance: ThemeAppearance,
    pub variables: BTreeMap<String, String>,
    pub font_family: String,
    pub font_size: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeAppearance {
    Light,
    Dark,
}
/// Fixed theme color/surface/border/radius allowlist; never arbitrary computed CSS.
pub const WEB_THEME_VARIABLES: &[&str] = &[
    "--background",
    "--foreground",
    "--card",
    "--card-foreground",
    "--popover",
    "--popover-foreground",
    "--primary",
    "--primary-foreground",
    "--secondary",
    "--secondary-foreground",
    "--muted",
    "--muted-foreground",
    "--accent",
    "--accent-foreground",
    "--destructive",
    "--destructive-foreground",
    "--border",
    "--input",
    "--ring",
    "--radius",
];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WebBridgeEnvelope {
    #[serde(rename_all = "camelCase")]
    Request {
        channel: String,
        version: u32,
        session_id: String,
        nonce: String,
        request_id: String,
        method: String,
        #[serde(deserialize_with = "required_json")]
        params: Value,
    },
    #[serde(rename_all = "camelCase")]
    Response {
        channel: String,
        version: u32,
        session_id: String,
        nonce: String,
        request_id: String,
        ok: bool,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        result: Option<Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_option"
        )]
        error: Option<PluginError>,
    },
    #[serde(rename_all = "camelCase")]
    Event {
        channel: String,
        version: u32,
        session_id: String,
        nonce: String,
        sequence: u64,
        event: WebEventType,
        #[serde(deserialize_with = "required_json")]
        payload: Value,
    },
    #[serde(rename_all = "camelCase")]
    Ready {
        channel: String,
        version: u32,
        session_id: String,
        nonce: String,
    },
    #[serde(rename_all = "camelCase")]
    Welcome {
        channel: String,
        version: u32,
        session_id: String,
        nonce: String,
        plugin_id: String,
        view_id: String,
        generation: String,
        theme: ThemeSnapshot,
    },
}
fn session(v: &str) -> Result<(), PluginError> {
    require(is_uuid(v), PLUGIN_INVALID_REQUEST, "invalid web session")
}
fn nonce(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn request_id(v: &str) -> bool {
    (1..=MAX_WEB_REQUEST_ID_CHARS).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn web_method(v: &str) -> bool {
    WEB_SHIM_METHODS.contains(&v)
}
pub fn validate_web_open_request(v: &WebOpenRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&v.id) && is_local_id(&v.view_id) && is_uuid(&v.generation),
        PLUGIN_INVALID_REQUEST,
        "invalid web view identity",
    )
}
pub fn validate_web_open_reply(v: &WebOpenReply) -> Result<(), PluginError> {
    let mut allow = BTreeSet::new();
    require(
        is_uuid(&v.boot_id)
            && is_plugin_id(&v.plugin_id)
            && is_local_id(&v.view_id)
            && is_uuid(&v.generation)
            && is_uuid(&v.session_id)
            && nonce(&v.nonce)
            && is_sha256(&v.package_digest)
            && is_web_entry(&v.entry)
            && v.shim_version == 1
            && v.allow.iter().all(|a| allow.insert(a)),
        PLUGIN_INVALID_REQUEST,
        "invalid web open reply",
    )
}
pub fn validate_web_request(v: &WebRequest) -> Result<(), PluginError> {
    session(&v.session_id)?;
    require(
        request_id(&v.request_id) && web_method(&v.method),
        PLUGIN_INVALID_REQUEST,
        "invalid web request",
    )?;
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_web_read_request(v: &WebReadRequest) -> Result<(), PluginError> {
    session(&v.session_id)?;
    require(
        v.after_sequence <= MAX_SAFE_INTEGER,
        PLUGIN_INVALID_REQUEST,
        "invalid cursor",
    )
}
pub fn validate_web_read_reply(v: &WebReadReply) -> Result<(), PluginError> {
    let mut previous = 0;
    require(
        v.sequence <= MAX_SAFE_INTEGER && v.events.len() <= MAX_WEB_READ_EVENTS,
        PLUGIN_INVALID_REQUEST,
        "invalid event batch",
    )?;
    for event in &v.events {
        require(
            event.sequence > previous && event.sequence <= v.sequence,
            PLUGIN_INVALID_REQUEST,
            "unordered event sequence",
        )?;
        previous = event.sequence;
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_web_close_request(v: &WebCloseRequest) -> Result<(), PluginError> {
    session(&v.session_id)
}
pub fn validate_web_close_reply(v: &WebCloseReply) -> Result<(), PluginError> {
    require(v.closed, PLUGIN_INVALID_REQUEST, "closed must be true")
}
pub fn validate_web_asset_request(v: &WebAssetRequest) -> Result<(), PluginError> {
    session(&v.session_id)?;
    require(
        is_package_path(&v.path) && v.path.starts_with("ui/"),
        PLUGIN_INVALID_REQUEST,
        "invalid asset path",
    )
}
pub const WEB_ASSET_MIME_TYPES: &[&str] = &[
    "text/html",
    "text/javascript",
    "application/javascript",
    "text/css",
    "application/json",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/x-icon",
    "font/woff",
    "font/woff2",
    "font/ttf",
    "font/otf",
    "application/wasm",
    "text/plain",
    "application/octet-stream",
];
pub fn validate_web_asset_reply(v: &WebAssetReply) -> Result<(), PluginError> {
    require(
        WEB_ASSET_MIME_TYPES.contains(&v.mime.as_str())
            && is_sha256(&v.sha256)
            && v.size <= MAX_PACKAGE_UI_BYTES
            && valid_base64(&v.data_base64, v.size),
        PLUGIN_INVALID_REQUEST,
        "invalid asset reply",
    )?;
    require(
        serde_json::to_vec(v).is_ok_and(|b| b.len() <= MAX_WEB_ASSET_REPLY_BYTES),
        PLUGIN_INVALID_REQUEST,
        "asset reply too large",
    )
}
fn valid_base64(v: &str, size: u64) -> bool {
    let expected = size.div_ceil(3) * 4;
    if v.len() as u64 != expected {
        return false;
    }
    let padding = match size % 3 {
        1 => 2,
        2 => 1,
        _ => 0,
    };
    let body_len = v.len().saturating_sub(padding);
    if !v.as_bytes()[..body_len]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
        || !v.as_bytes()[body_len..].iter().all(|b| *b == b'=')
    {
        return false;
    }
    // Reject noncanonical pad bits without introducing a base64 dependency.
    if padding > 0 {
        let b = v.as_bytes()[body_len - 1];
        let n = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        return if padding == 2 {
            n & 15 == 0
        } else {
            n & 3 == 0
        };
    }
    true
}
pub fn validate_host_ui_post_message_request(
    v: &HostUiPostMessageRequest,
) -> Result<(), PluginError> {
    require(
        is_local_id(&v.view_id),
        PLUGIN_INVALID_REQUEST,
        "invalid view id",
    )?;
    json_size(v, PLUGIN_INVALID_REQUEST)
}
pub fn validate_host_ui_post_message_reply(v: &HostUiPostMessageReply) -> Result<(), PluginError> {
    require(
        v.delivered <= MAX_WEB_SESSIONS as u32,
        PLUGIN_INVALID_REQUEST,
        "invalid delivery count",
    )
}
pub fn validate_theme_snapshot(v: &ThemeSnapshot) -> Result<(), PluginError> {
    require(
        v.revision <= MAX_SAFE_INTEGER
            && !v.variables.is_empty()
            && v.variables.iter().all(|(k, val)| {
                WEB_THEME_VARIABLES.contains(&k.as_str()) && (1..=1024).contains(&chars(val))
            })
            && (1..=512).contains(&chars(&v.font_family))
            && (1..=64).contains(&chars(&v.font_size)),
        PLUGIN_INVALID_REQUEST,
        "invalid theme",
    )
}
pub fn validate_web_bridge_envelope(v: &WebBridgeEnvelope) -> Result<(), PluginError> {
    use WebBridgeEnvelope::*;
    let (channel, version, session_id, token) = match v {
        Request {
            channel,
            version,
            session_id,
            nonce,
            ..
        }
        | Response {
            channel,
            version,
            session_id,
            nonce,
            ..
        }
        | Event {
            channel,
            version,
            session_id,
            nonce,
            ..
        }
        | Ready {
            channel,
            version,
            session_id,
            nonce,
        }
        | Welcome {
            channel,
            version,
            session_id,
            nonce,
            ..
        } => (channel, version, session_id, nonce),
    };
    require(
        channel == WEB_BRIDGE_CHANNEL && *version == 1 && is_uuid(session_id) && nonce(token),
        PLUGIN_INVALID_REQUEST,
        "invalid bridge identity",
    )?;
    match v {
        Request {
            request_id: id,
            method,
            ..
        } => require(
            request_id(id) && web_method(method),
            PLUGIN_INVALID_REQUEST,
            "invalid bridge request",
        )?,
        Response {
            request_id: id,
            ok,
            result,
            error,
            ..
        } => {
            require(
                request_id(id)
                    && ((*ok && result.is_some() && error.is_none())
                        || (!*ok && result.is_none() && error.is_some())),
                PLUGIN_INVALID_REQUEST,
                "invalid bridge response",
            )?;
            if let Some(error) = error {
                text(&error.code, 1, 128, PLUGIN_INVALID_REQUEST)?;
                text(&error.message, 1, 4096, PLUGIN_INVALID_REQUEST)?;
            }
        }
        Event { sequence, .. } => {
            revision(*sequence, PLUGIN_INVALID_REQUEST)?;
        }
        Welcome {
            plugin_id,
            view_id,
            generation,
            theme,
            ..
        } => {
            require(
                is_plugin_id(plugin_id) && is_local_id(view_id) && is_uuid(generation),
                PLUGIN_INVALID_REQUEST,
                "invalid welcome",
            )?;
            validate_theme_snapshot(theme)?;
        }
        Ready { .. } => {}
    }
    json_size(v, PLUGIN_INVALID_REQUEST)
}
macro_rules! parser {
    ($name:ident, $ty:ty, $validate:ident) => {
        pub fn $name(bytes: &[u8]) -> Result<$ty, PluginError> {
            let v =
                crate::host::strict_contract_json(bytes, PLUGIN_INVALID_REQUEST, MAX_JSON_BYTES)?;
            $validate(&v)?;
            Ok(v)
        }
    };
}
parser!(
    parse_web_open_request,
    WebOpenRequest,
    validate_web_open_request
);
parser!(parse_web_open_reply, WebOpenReply, validate_web_open_reply);
parser!(parse_web_request, WebRequest, validate_web_request);
parser!(
    parse_web_read_request,
    WebReadRequest,
    validate_web_read_request
);
parser!(parse_web_read_reply, WebReadReply, validate_web_read_reply);
parser!(
    parse_web_close_request,
    WebCloseRequest,
    validate_web_close_request
);
parser!(
    parse_web_close_reply,
    WebCloseReply,
    validate_web_close_reply
);
parser!(
    parse_web_asset_request,
    WebAssetRequest,
    validate_web_asset_request
);
parser!(
    parse_host_ui_post_message_request,
    HostUiPostMessageRequest,
    validate_host_ui_post_message_request
);
parser!(
    parse_host_ui_post_message_reply,
    HostUiPostMessageReply,
    validate_host_ui_post_message_reply
);
parser!(
    parse_web_bridge_envelope,
    WebBridgeEnvelope,
    validate_web_bridge_envelope
);
pub fn parse_web_asset_reply(bytes: &[u8]) -> Result<WebAssetReply, PluginError> {
    require(
        bytes.len() <= MAX_WEB_ASSET_REPLY_BYTES,
        PLUGIN_INVALID_REQUEST,
        "asset reply too large",
    )?;
    let v = crate::host::strict_contract_json(
        bytes,
        PLUGIN_INVALID_REQUEST,
        MAX_WEB_ASSET_REPLY_BYTES,
    )?;
    validate_web_asset_reply(&v)?;
    Ok(v)
}

fn required_json<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    Value::deserialize(d)
}
fn present_json<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
