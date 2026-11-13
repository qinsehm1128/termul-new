//! qin-code board plugin: native side. The web view asks `endpoint` for the
//! sidecar's loopback WebSocket address; the sidecar announces it on stdout.
pub mod endpoint;
mod plugin;
pub mod token;

use se_plugin_abi::*;

static API: SePluginApiV1 = SePluginApiV1 {
    abi_version: SEP_ABI_VERSION,
    struct_size: std::mem::size_of::<SePluginApiV1>() as u32,
    manifest: Some(export::manifest::<plugin::BoardPlugin>),
    create: Some(export::create::<plugin::BoardPlugin>),
    render_view: Some(export::render_view::<plugin::BoardPlugin>),
    on_action: Some(export::on_action::<plugin::BoardPlugin>),
    on_command: Some(export::on_command::<plugin::BoardPlugin>),
    on_config_changed: Some(export::on_config_changed::<plugin::BoardPlugin>),
    on_service_call: Some(export::on_service_call::<plugin::BoardPlugin>),
    destroy: Some(export::destroy::<plugin::BoardPlugin>),
};

/// # Safety
/// The host must supply a writable output pointer and honor the ABI v1 lifetime.
#[no_mangle]
pub unsafe extern "C" fn se_plugin_entry_v1(
    min: u32,
    max: u32,
    out: *mut *const SePluginApiV1,
) -> SeStatus {
    export::boundary(|| unsafe { export::entry(min, max, out, &API) })
}

/// Embedded manifest bytes (also used by tests).
pub const MANIFEST: &[u8] = include_bytes!("manifest.json");
