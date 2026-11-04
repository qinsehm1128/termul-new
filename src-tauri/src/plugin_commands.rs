//! GUI-only RPC adapters. This module never constructs a PluginHost or loads a library.
use crate::commands::IpcResult;
use crate::core::plugin::{
    parse_process_control_request, parse_process_list_request, parse_process_logs_request,
    ProcessListReply,
};
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

fn require_main_window(label: &str) -> Result<(), PluginError> {
    if label == "main" {
        Ok(())
    } else {
        log::warn!(target: "se_manager::core", "operation=host_context_publish rejected=untrusted_window");
        Err(PluginError::new(
            PLUGIN_INVALID_REQUEST,
            "Host context requires the main window",
        ))
    }
}

// Value admission deliberately keeps serde's value-bearing errors out of Tauri replies.
fn host_payload<T>(
    payload: serde_json::Value,
    parser: fn(&[u8]) -> Result<T, PluginError>,
) -> Result<T, PluginError> {
    let invalid = || PluginError::new(PLUGIN_INVALID_REQUEST, "Invalid plugin request");
    let bytes = serde_json::to_vec(&payload).map_err(|_| invalid())?;
    parser(&bytes).map_err(|_| invalid())
}
fn host_result<T>(result: Result<T, PluginError>) -> IpcResult<T> {
    match result {
        Ok(value) => IpcResult::success(value),
        Err(error) => IpcResult::error(error.message, error.code),
    }
}

async fn publish_host_context_from_window(
    service: &PluginServiceHandle,
    label: &str,
    payload: serde_json::Value,
) -> IpcResult<HostContextAck> {
    host_result(
        async {
            require_main_window(label)?;
            service
                .publish_host_context(host_payload(payload, parse_host_context_publish_request)?)
                .await
        }
        .await,
    )
}
async fn heartbeat_host_context_from_window(
    service: &PluginServiceHandle,
    label: &str,
    payload: serde_json::Value,
) -> IpcResult<HostContextAck> {
    host_result(
        async {
            require_main_window(label)?;
            service
                .heartbeat_host_context(host_payload(
                    payload,
                    parse_host_context_heartbeat_request,
                )?)
                .await
        }
        .await,
    )
}
#[tauri::command(async)]
pub fn plugin_publish_host_context(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, PluginServiceHandle>,
    payload: serde_json::Value,
) -> impl std::future::Future<Output = IpcResult<HostContextAck>> + Send + 'static {
    let service = service.inner().clone();
    let label = window.label().to_owned();
    async move { publish_host_context_from_window(&service, &label, payload).await }
}
#[tauri::command(async)]
pub fn plugin_heartbeat_host_context(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, PluginServiceHandle>,
    payload: serde_json::Value,
) -> impl std::future::Future<Output = IpcResult<HostContextAck>> + Send + 'static {
    let service = service.inner().clone();
    let label = window.label().to_owned();
    async move { heartbeat_host_context_from_window(&service, &label, payload).await }
}
#[tauri::command(async)]
pub fn plugin_get_host_ui(
    service: tauri::State<'_, PluginServiceHandle>,
    payload: serde_json::Value,
) -> impl std::future::Future<Output = IpcResult<HostUiSnapshot>> + Send + 'static {
    let service = service.inner().clone();
    async move {
        host_result(
            async {
                host_payload(payload, parse_host_ui_get_request)?;
                service.host_ui_snapshot().await
            }
            .await,
        )
    }
}
macro_rules! host_command {
    ($name:ident, $parser:ident, $operation:ident, $reply:ty) => {
        #[tauri::command(async)]
        pub fn $name(
            service: tauri::State<'_, PluginServiceHandle>,
            payload: serde_json::Value,
        ) -> impl std::future::Future<Output = IpcResult<$reply>> + Send + 'static {
            let service = service.inner().clone();
            async move {
                host_result(
                    async { service.$operation(host_payload(payload, $parser)?).await }.await,
                )
            }
        }
    };
}
host_command!(
    plugin_dismiss_host_ui,
    parse_host_ui_dismiss_request,
    dismiss_host_ui,
    HostUiDismissResult
);
host_command!(
    plugin_execute_contribution,
    parse_execute_contribution_request,
    execute_contribution,
    CommandReply
);

/// GUI addition to the frozen Core reply: W4 uses `url` on every platform.
#[derive(Serialize)]
pub struct PluginWebOpenReply {
    #[serde(flatten)]
    session: WebOpenReply,
    url: String,
}

#[tauri::command(async)]
pub fn plugin_web_open(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, PluginServiceHandle>,
    sessions: tauri::State<'_, std::sync::Arc<crate::plugin_ui_protocol::PluginUiSessions>>,
    payload: serde_json::Value,
) -> impl std::future::Future<Output = IpcResult<PluginWebOpenReply>> + Send + 'static {
    let service = service.inner().clone();
    let sessions = sessions.inner().clone();
    let label = window.label().to_owned();
    async move {
        host_result(
            async {
                require_main_window(&label)?;
                let session = service
                    .web_open(host_payload(payload, parse_web_open_request)?)
                    .await?;
                sessions.insert(&session)?;
                let url = crate::plugin_ui_protocol::session_url(&session);
                Ok(PluginWebOpenReply { session, url })
            }
            .await,
        )
    }
}

#[tauri::command(async)]
pub fn plugin_web_close(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, PluginServiceHandle>,
    sessions: tauri::State<'_, std::sync::Arc<crate::plugin_ui_protocol::PluginUiSessions>>,
    payload: serde_json::Value,
) -> impl std::future::Future<Output = IpcResult<WebCloseReply>> + Send + 'static {
    let service = service.inner().clone();
    let sessions = sessions.inner().clone();
    let label = window.label().to_owned();
    async move {
        host_result(
            async {
                require_main_window(&label)?;
                let request = host_payload(payload, parse_web_close_request)?;
                sessions.remove(&request.session_id);
                service.web_close(request).await
            }
            .await,
        )
    }
}

// Only the trusted main renderer can use these adapters; plugin frames have no Tauri IPC.
macro_rules! web_command {
    ($name:ident, $parser:path, $operation:ident, $reply:ty) => {
        #[tauri::command(async)]
        pub fn $name(
            window: tauri::WebviewWindow,
            service: tauri::State<'_, PluginServiceHandle>,
            payload: serde_json::Value,
        ) -> impl std::future::Future<Output = IpcResult<$reply>> + Send + 'static {
            let service = service.inner().clone();
            let label = window.label().to_owned();
            async move {
                host_result(
                    async {
                        require_main_window(&label)?;
                        service.$operation(host_payload(payload, $parser)?).await
                    }
                    .await,
                )
            }
        }
    };
}
web_command!(
    plugin_web_read,
    parse_web_read_request,
    web_read,
    WebReadReply
);
web_command!(
    plugin_web_request,
    parse_web_request,
    web_request,
    serde_json::Value
);

web_command!(
    plugin_process_list,
    parse_process_list_request,
    process_list,
    ProcessListReply
);
web_command!(
    plugin_process_logs,
    parse_process_logs_request,
    process_logs,
    HostProcessLogsReply
);
web_command!(
    plugin_process_control,
    parse_process_control_request,
    process_control,
    HostProcessStatus
);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_main_window_cannot_publish_host_context() {
        let service = PluginServiceHandle::default();
        for label in ["secondary", "", "main-other"] {
            let result =
                publish_host_context_from_window(&service, label, serde_json::json!({})).await;
            assert_eq!(result.code.as_deref(), Some(PLUGIN_INVALID_REQUEST));
            let result =
                heartbeat_host_context_from_window(&service, label, serde_json::json!({})).await;
            assert_eq!(result.code.as_deref(), Some(PLUGIN_INVALID_REQUEST));
        }
        assert!(require_main_window("main").is_ok());
        let result = heartbeat_host_context_from_window(
            &service,
            "main",
            serde_json::json!({"sourceId": uuid::Uuid::new_v4().to_string()}),
        )
        .await;
        assert_eq!(result.code.as_deref(), Some(PLUGIN_CORE_UNAVAILABLE));
    }

    #[test]
    fn malformed_host_payload_diagnostics_are_static() {
        let error = host_payload(
            serde_json::json!({"sourceId": "private-value", "private-key": "secret"}),
            parse_host_context_heartbeat_request,
        )
        .unwrap_err();
        assert_eq!(error.code, PLUGIN_INVALID_REQUEST);
        assert_eq!(error.message, "Invalid plugin request");
    }
    #[tokio::test]
    async fn unavailable_service_is_managed_without_loading_plugins() {
        let service = PluginServiceHandle::default();
        let result: IpcResult<PluginSnapshot> = call(&service, METHOD_LIST, ListRequest {}).await;
        assert!(!result.success);
        assert_eq!(result.code.as_deref(), Some(PLUGIN_CORE_UNAVAILABLE));
    }
}
