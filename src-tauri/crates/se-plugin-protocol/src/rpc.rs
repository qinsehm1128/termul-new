use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const METHOD_LIST: &str = "plugins.list";
pub const METHOD_INSTALL: &str = "plugins.install";
pub const METHOD_SET_ENABLED: &str = "plugins.set_enabled";
pub const METHOD_RELOAD: &str = "plugins.reload";
pub const METHOD_UNINSTALL: &str = "plugins.uninstall";
pub const METHOD_GET_VIEW: &str = "plugins.view.get";
pub const METHOD_ACTION: &str = "plugins.view.action";
pub const METHOD_COMMAND: &str = "plugins.command.execute";
pub const METHOD_GET_CONFIG: &str = "plugins.config.get";
pub const METHOD_SET_CONFIG: &str = "plugins.config.set";
pub const METHOD_HEALTH: &str = "plugins.health";
pub const METHOD_SHUTDOWN: &str = "shutdown";
pub const TOPIC_PLUGIN_EVENT: &str = "plugin.event";
pub const EVENT_PLUGINS_CHANGED: &str = "plugins:changed";
pub const EVENT_PLUGINS_CONNECTION_CHANGED: &str = "plugins:connection_changed";
pub const METHOD_HOST_CONTEXT_PUBLISH: &str = "plugins.host.context.publish";
pub const METHOD_HOST_CONTEXT_HEARTBEAT: &str = "plugins.host.context.heartbeat";
pub const METHOD_HOST_UI_GET: &str = "plugins.host.ui.get";
pub const METHOD_HOST_UI_DISMISS: &str = "plugins.host.ui.dismiss";
pub const METHOD_CONTRIBUTION_EXECUTE: &str = "plugins.contribution.execute";
/// Advertised and required once Plugin Core serves the host RPCs above (P3).
pub const HOST_CAPABILITIES_V1: &str = "plugins.host-capabilities.v1";
pub const PLUGIN_CAPABILITIES: [&str; 5] = [
    HOST_CAPABILITIES_V1,
    "plugins.lifecycle.v1",
    "plugins.ui.v1",
    "plugins.configuration.v1",
    "plugins.broker.v1",
];

macro_rules! dto {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
    };
}
dto!(ListRequest {});
dto!(HealthRequest {});
dto!(ShutdownRequest {});
dto!(InstallRequest { path: String });
dto!(SetEnabledRequest {
    id: String,
    enabled: bool
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReloadRequest {
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub path: Option<String>,
}
dto!(UninstallRequest { id: String });
dto!(UninstallResult {
    id: String,
    removed: bool
});
dto!(GetViewRequest {
    id: String,
    view_id: String
});
dto!(ExecuteCommandRequest {
    id: String,
    generation: String,
    command_id: String,
    #[serde(deserialize_with = "required_value")]
    args: Value
});
dto!(GetConfigRequest { id: String });
dto!(SetConfigRequest {
    id: String,
    expected_revision: u64,
    values: ConfigValues
});
dto!(RenderRequest {
    schema_version: u32,
    view_id: String
});
dto!(CommandRequest {
    schema_version: u32,
    command_id: String,
    #[serde(deserialize_with = "required_value")]
    args: Value
});
dto!(CommandReply { schema_version: u32, #[serde(deserialize_with = "required_value")] result: Value, invalidate_views: Vec<String> });
dto!(ServiceRequest {
    schema_version: u32,
    service: String,
    version: u32,
    method: String,
    #[serde(deserialize_with = "required_value")]
    input: Value
});
dto!(ServiceReply {
    schema_version: u32,
    #[serde(deserialize_with = "required_value")]
    result: Value
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInfo {
    pub id: String,
    pub manifest: Manifest,
    pub desired_enabled: bool,
    pub state: PluginState,
    #[serde(deserialize_with = "nullable_option")]
    pub generation: Option<String>,
    pub config_revision: u64,
    pub sha256: String,
    #[serde(deserialize_with = "nullable_option")]
    pub last_error: Option<PluginError>,
}
dto!(PluginSnapshot { schema_version: u32, boot_id: String, seq: u64, plugins: Vec<PluginInfo> });
dto!(HealthSnapshot {
    schema_version: u32,
    boot_id: String,
    pid: u32,
    ready: bool,
    safe_mode: bool,
    active_resources: u64,
});
pub type PluginHealth = HealthSnapshot;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    Installed,
    Disabled,
    Pending,
    Activating,
    Active,
    Reloading,
    Failed,
    Quarantined,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginEventType {
    RegistryChanged,
    StateChanged,
    ViewChanged,
    ConfigChanged,
    ResyncRequired,
    HostUiChanged,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginEvent {
    pub schema_version: u32,
    pub boot_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: PluginEventType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub plugin_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub generation: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub view_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub config_revision: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connected,
    Disconnected,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConnectionEvent {
    pub state: ConnectionState,
    #[serde(deserialize_with = "nullable_option")]
    pub boot_id: Option<String>,
}

pub fn validate_render_request(request: &RenderRequest) -> Result<(), PluginError> {
    version(request.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_local_id(&request.view_id),
        PLUGIN_INVALID_REQUEST,
        "invalid viewId",
    )
}
pub fn validate_command_request(request: &CommandRequest) -> Result<(), PluginError> {
    version(request.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_local_id(&request.command_id),
        PLUGIN_INVALID_REQUEST,
        "invalid commandId",
    )?;
    json_size(request, PLUGIN_INVALID_REQUEST)
}
pub fn validate_command_reply(reply: &CommandReply) -> Result<(), PluginError> {
    version(reply.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        reply.invalidate_views.len() <= 8 && reply.invalidate_views.iter().all(|v| is_local_id(v)),
        PLUGIN_INVALID_REQUEST,
        "invalid invalidateViews",
    )?;
    let ids: std::collections::BTreeSet<_> = reply.invalidate_views.iter().collect();
    require(
        ids.len() == reply.invalidate_views.len(),
        PLUGIN_INVALID_REQUEST,
        "duplicate invalidated view",
    )?;
    json_size(reply, PLUGIN_INVALID_REQUEST)
}
pub fn validate_service_request(request: &ServiceRequest) -> Result<(), PluginError> {
    version(request.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_plugin_id(&request.service) && (1..=i32::MAX as u32).contains(&request.version),
        PLUGIN_INVALID_REQUEST,
        "invalid service identity",
    )?;
    text(&request.method, 0, 8192, PLUGIN_INVALID_REQUEST)?;
    json_size(request, PLUGIN_INVALID_REQUEST)
}
pub fn parse_service_request(bytes: &[u8]) -> Result<ServiceRequest, PluginError> {
    let request = json(bytes, PLUGIN_INVALID_REQUEST)?;
    validate_service_request(&request)?;
    Ok(request)
}
pub fn validate_service_reply(reply: &ServiceReply) -> Result<(), PluginError> {
    version(reply.schema_version, PLUGIN_INVALID_REQUEST)?;
    json_size(reply, PLUGIN_INVALID_REQUEST)
}
pub fn parse_service_reply(bytes: &[u8]) -> Result<ServiceReply, PluginError> {
    let reply = json(bytes, PLUGIN_INVALID_REQUEST)?;
    validate_service_reply(&reply)?;
    Ok(reply)
}
pub fn validate_set_config_request(
    request: &SetConfigRequest,
    schema: &ConfigurationSchema,
) -> Result<ConfigValues, PluginError> {
    require(
        is_plugin_id(&request.id),
        PLUGIN_CONFIG_INVALID,
        "invalid plugin id",
    )?;
    revision(request.expected_revision, PLUGIN_CONFIG_INVALID)?;
    json_size(request, PLUGIN_CONFIG_INVALID)?;
    complete_config_values(schema, &request.values)
}
pub fn validate_plugin_info(info: &PluginInfo) -> Result<(), PluginError> {
    validate_manifest(&info.manifest)?;
    require(
        info.id == info.manifest.id,
        PLUGIN_INVALID_REQUEST,
        "plugin info id mismatch",
    )?;
    revision(info.config_revision, PLUGIN_INVALID_REQUEST)?;
    require(
        info.generation.as_ref().is_none_or(|g| is_uuid(g)),
        PLUGIN_INVALID_REQUEST,
        "invalid generation",
    )?;
    require(
        info.sha256.len() == 64 && info.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        PLUGIN_INVALID_REQUEST,
        "invalid SHA256",
    )
}
pub fn validate_plugin_snapshot(snapshot: &PluginSnapshot) -> Result<(), PluginError> {
    version(snapshot.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_uuid(&snapshot.boot_id) && snapshot.seq <= MAX_SAFE_INTEGER,
        PLUGIN_INVALID_REQUEST,
        "invalid snapshot identity",
    )?;
    let mut ids = std::collections::BTreeSet::new();
    for info in &snapshot.plugins {
        validate_plugin_info(info)?;
        require(
            ids.insert(&info.id),
            PLUGIN_INVALID_REQUEST,
            "duplicate plugin id",
        )?;
    }
    Ok(())
}
pub fn validate_plugin_event(event: &PluginEvent) -> Result<(), PluginError> {
    version(event.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_uuid(&event.boot_id) && event.seq <= MAX_SAFE_INTEGER,
        PLUGIN_INVALID_REQUEST,
        "invalid event identity",
    )?;
    require(
        event.plugin_id.as_ref().is_none_or(|id| is_plugin_id(id))
            && event.generation.as_ref().is_none_or(|g| is_uuid(g))
            && event.view_id.as_ref().is_none_or(|id| is_local_id(id)),
        PLUGIN_INVALID_REQUEST,
        "invalid event resource",
    )?;
    if let Some(rev) = event.config_revision {
        revision(rev, PLUGIN_INVALID_REQUEST)?;
    }
    Ok(())
}
