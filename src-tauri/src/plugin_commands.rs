//! GUI-only RPC adapters. This module never constructs a PluginHost or loads a library.
use crate::commands::IpcResult;
use crate::core::PluginServiceHandle;
use se_plugin_protocol::*;
use serde::{de::DeserializeOwned, Serialize};

async fn call<T: DeserializeOwned, P: Serialize>(
    service: &PluginServiceHandle,
    method: &str,
    payload: P,
) -> IpcResult<T> {
    let Some(client) = service.client() else {
        return IpcResult::error("Plugin Core is unavailable", PLUGIN_CORE_UNAVAILABLE);
    };
    let payload = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(_) => return IpcResult::error("Invalid plugin request", PLUGIN_INVALID_REQUEST),
    };
    match client.request(method, payload).await {
        Ok(value) => match serde_json::from_value(value) {
            Ok(value) => IpcResult::success(value),
            Err(_) => IpcResult::error("Invalid Plugin Core response", PLUGIN_INVALID_REQUEST),
        },
        Err(error) => IpcResult::error(error.message, error.code),
    }
}

#[tauri::command(async)]
pub fn plugin_list(
    service: tauri::State<'_, PluginServiceHandle>,
) -> impl std::future::Future<Output = IpcResult<PluginSnapshot>> + Send + 'static {
    let service = service.inner().clone();
    async move { call(&service, METHOD_LIST, ListRequest {}).await }
}
macro_rules! command {
    ($name:ident, $method:ident, $request:ty, $reply:ty) => {
        #[tauri::command(async)]
        pub fn $name(
            service: tauri::State<'_, PluginServiceHandle>,
            payload: $request,
        ) -> impl std::future::Future<Output = IpcResult<$reply>> + Send + 'static {
            let service = service.inner().clone();
            async move { call(&service, $method, payload).await }
        }
    };
}
command!(plugin_install, METHOD_INSTALL, InstallRequest, PluginInfo);
command!(
    plugin_set_enabled,
    METHOD_SET_ENABLED,
    SetEnabledRequest,
    PluginInfo
);
command!(plugin_reload, METHOD_RELOAD, ReloadRequest, PluginInfo);
command!(
    plugin_uninstall,
    METHOD_UNINSTALL,
    UninstallRequest,
    UninstallResult
);
command!(
    plugin_get_view,
    METHOD_GET_VIEW,
    GetViewRequest,
    ViewSnapshot
);
command!(
    plugin_dispatch_action,
    METHOD_ACTION,
    ActionEvent,
    ViewSnapshot
);
command!(
    plugin_execute_command,
    METHOD_COMMAND,
    ExecuteCommandRequest,
    CommandReply
);
command!(
    plugin_get_config,
    METHOD_GET_CONFIG,
    GetConfigRequest,
    ConfigSnapshot
);
command!(
    plugin_set_config,
    METHOD_SET_CONFIG,
    SetConfigRequest,
    ConfigSnapshot
);

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unavailable_service_is_managed_without_loading_plugins() {
        let service = PluginServiceHandle::default();
        let result: IpcResult<PluginSnapshot> = call(&service, METHOD_LIST, ListRequest {}).await;
        assert!(!result.success);
        assert_eq!(result.code.as_deref(), Some(PLUGIN_CORE_UNAVAILABLE));
    }
}
