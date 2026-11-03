use crate::*;

/// Structural checks for GUI requests before resolving runtime resources.
pub fn validate_install_request(request: &InstallRequest) -> Result<(), PluginError> {
    text(&request.path, 1, 8192, PLUGIN_INVALID_REQUEST)
}
pub fn validate_set_enabled_request(request: &SetEnabledRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id),
        PLUGIN_INVALID_REQUEST,
        "invalid plugin id",
    )
}
pub fn validate_reload_request(request: &ReloadRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id)
            && request
                .path
                .as_ref()
                .is_none_or(|p| (1..=8192).contains(&chars(p))),
        PLUGIN_INVALID_REQUEST,
        "invalid reload identity or path",
    )
}
pub fn validate_uninstall_request(request: &UninstallRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id),
        PLUGIN_INVALID_REQUEST,
        "invalid plugin id",
    )
}
pub fn validate_uninstall_result(result: &UninstallResult) -> Result<(), PluginError> {
    require(
        is_plugin_id(&result.id) && result.removed,
        PLUGIN_INVALID_REQUEST,
        "invalid uninstall result",
    )
}
pub fn validate_get_view_request(request: &GetViewRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id) && is_local_id(&request.view_id),
        PLUGIN_INVALID_REQUEST,
        "invalid view identity",
    )
}
pub fn validate_execute_command_request(
    request: &ExecuteCommandRequest,
) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id)
            && is_uuid(&request.generation)
            && is_local_id(&request.command_id),
        PLUGIN_INVALID_REQUEST,
        "invalid command identity",
    )?;
    json_size(request, PLUGIN_INVALID_REQUEST)
}
pub fn validate_get_config_request(request: &GetConfigRequest) -> Result<(), PluginError> {
    require(
        is_plugin_id(&request.id),
        PLUGIN_CONFIG_INVALID,
        "invalid plugin id",
    )
}
pub fn validate_health_snapshot(snapshot: &HealthSnapshot) -> Result<(), PluginError> {
    version(snapshot.schema_version, PLUGIN_INVALID_REQUEST)?;
    require(
        is_uuid(&snapshot.boot_id)
            && snapshot.pid > 0
            && snapshot.active_resources <= MAX_SAFE_INTEGER,
        PLUGIN_INVALID_REQUEST,
        "invalid health snapshot",
    )
}
pub fn validate_connection_event(event: &PluginConnectionEvent) -> Result<(), PluginError> {
    require(
        event.boot_id.as_ref().is_none_or(|id| is_uuid(id)),
        PLUGIN_INVALID_REQUEST,
        "invalid bootId",
    )
}
pub fn parse_render_request(bytes: &[u8]) -> Result<RenderRequest, PluginError> {
    let request = json(bytes, PLUGIN_INVALID_REQUEST)?;
    validate_render_request(&request)?;
    Ok(request)
}
pub fn parse_command_request(bytes: &[u8]) -> Result<CommandRequest, PluginError> {
    let request = json(bytes, PLUGIN_INVALID_REQUEST)?;
    validate_command_request(&request)?;
    Ok(request)
}
pub fn parse_command_reply(bytes: &[u8]) -> Result<CommandReply, PluginError> {
    let reply = json(bytes, PLUGIN_INVALID_REQUEST)?;
    validate_command_reply(&reply)?;
    Ok(reply)
}
