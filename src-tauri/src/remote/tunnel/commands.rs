//! Tauri commands for reading and updating the desktop tunnel provider config.

use std::sync::Arc;

use tauri::State;

use super::config::{TunnelConfigStore, TunnelConfigUpdate, TunnelConfigView};
use crate::commands::IpcResult;

#[tauri::command]
pub async fn tunnel_config_get(
    store: State<'_, Arc<TunnelConfigStore>>,
) -> Result<IpcResult<TunnelConfigView>, String> {
    match store.view() {
        Ok(view) => Ok(IpcResult::success(view)),
        Err(error) => Ok(IpcResult::error(error, "TUNNEL_CONFIG_READ_FAILED")),
    }
}

#[tauri::command]
pub async fn tunnel_config_set(
    update: TunnelConfigUpdate,
    store: State<'_, Arc<TunnelConfigStore>>,
) -> Result<IpcResult<TunnelConfigView>, String> {
    match store.apply_update(update) {
        Ok(_) => match store.view() {
            Ok(view) => Ok(IpcResult::success(view)),
            Err(error) => Ok(IpcResult::error(error, "TUNNEL_CONFIG_READ_FAILED")),
        },
        Err(error) => Ok(IpcResult::error(error, "TUNNEL_CONFIG_WRITE_FAILED")),
    }
}
