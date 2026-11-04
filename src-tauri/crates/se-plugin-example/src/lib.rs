mod activity;
mod counter;
mod studio;

// The generic SDK intentionally makes Host callback-local (!Send). This adapter
// captures only the explicitly background-safe wake broker, never a Host value.
use se_plugin_abi::*;
unsafe extern "C" fn create(
    host: *const SeHostApiV1,
    activation: SeSlice,
    out: *mut *mut std::ffi::c_void,
) -> SeStatus {
    if let Ok(api) = unsafe { validate_host_api(host) } {
        studio::capture_waker(api);
    }
    unsafe { export::create::<counter::CounterPlugin>(host, activation, out) }
}
static API: SePluginApiV1 = SePluginApiV1 {
    abi_version: SEP_ABI_VERSION,
    struct_size: std::mem::size_of::<SePluginApiV1>() as u32,
    manifest: Some(export::manifest::<counter::CounterPlugin>),
    create: Some(create),
    render_view: Some(export::render_view::<counter::CounterPlugin>),
    on_action: Some(export::on_action::<counter::CounterPlugin>),
    on_command: Some(export::on_command::<counter::CounterPlugin>),
    on_config_changed: Some(export::on_config_changed::<counter::CounterPlugin>),
    on_service_call: Some(export::on_service_call::<counter::CounterPlugin>),
    destroy: Some(export::destroy::<counter::CounterPlugin>),
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
