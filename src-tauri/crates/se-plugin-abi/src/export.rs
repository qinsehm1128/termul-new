//! Implementation details used by `export_plugin!`. Not a separate stable SDK.
//! Raw callers must uphold the memory/lifetime and same-thread, non-reentrant
//! instance contracts documented by the C ABI. Checks cannot validate mapping.

use crate::sdk::input_bytes;
use crate::*;
use core::ffi::c_void;

/// Every exported boundary invokes this *inside* its extern-C function. Do not
/// drop arbitrary panic payloads: a malicious payload can itself panic in Drop.
#[doc(hidden)]
pub fn boundary(call: impl FnOnce() -> Result<(), SeStatus>) -> SeStatus {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(Ok(())) => SE_OK,
        Ok(Err(SE_OK)) => SE_INTERNAL,
        Ok(Err(status)) => status,
        Err(payload) => {
            std::mem::forget(payload);
            SE_PANIC
        }
    }
}

/// # Safety
/// `output` must satisfy `OutputWriter::from_raw`'s contract.
pub unsafe extern "C" fn manifest<P: PluginV1>(output: *mut SeOutput) -> SeStatus {
    boundary(|| {
        let mut out = unsafe { OutputWriter::from_raw(output)? };
        let bytes = P::manifest();
        std::str::from_utf8(bytes).map_err(|_| SE_INVALID_JSON)?;
        out.write(bytes)
    })
}

/// # Safety
/// Host/input must satisfy their documented borrowed-memory contracts. The
/// aligned non-null `out_instance` must be exclusively writable for this call.
pub unsafe extern "C" fn create<P: PluginV1>(
    host: *const SeHostApiV1,
    activation: SeSlice,
    out_instance: *mut *mut c_void,
) -> SeStatus {
    boundary(|| {
        check_pointer(out_instance)?;
        unsafe {
            *out_instance = std::ptr::null_mut();
        }
        let host = unsafe { Host::from_raw(host)? };
        let activation = unsafe { input_bytes(activation)? };
        let instance = Box::new(P::create(host, activation)?);
        unsafe {
            *out_instance = Box::into_raw(instance).cast();
        }
        Ok(())
    })
}

/// # Safety
/// Instance must be the live pointer returned by `create::<P>`, on its original
/// thread, with exclusive/non-reentrant access. Input/output storage is borrowed
/// only for this call, valid, and disjoint from the instance and each other.
unsafe fn json_callback<P: PluginV1>(
    instance: *mut c_void,
    input: SeSlice,
    output: *mut SeOutput,
    call: fn(&mut P, &[u8], &mut OutputWriter<'_>) -> Result<(), SeStatus>,
) -> Result<(), SeStatus> {
    let mut out = unsafe { OutputWriter::from_raw(output)? };
    check_pointer(instance.cast::<P>())?;
    let input = unsafe { input_bytes(input)? };
    call(unsafe { &mut *instance.cast::<P>() }, input, &mut out)?;
    if out.len() > out.capacity() {
        return Err(SE_BUFFER_TOO_SMALL);
    }
    Ok(())
}

macro_rules! json_trampoline {
    ($name:ident) => {
        /// # Safety
        /// The pointer, lifetime, thread and exclusivity requirements of the
        /// plugin ABI must hold; the instance must originate in `create::<P>`.
        pub unsafe extern "C" fn $name<P: PluginV1>(
            instance: *mut c_void,
            input: SeSlice,
            output: *mut SeOutput,
        ) -> SeStatus {
            boundary(|| unsafe { json_callback::<P>(instance, input, output, P::$name) })
        }
    };
}
json_trampoline!(render_view);
json_trampoline!(on_action);
json_trampoline!(on_command);
json_trampoline!(on_service_call);

/// # Safety
/// Instance must be live, from `create::<P>`, and accessed exclusively on its
/// creating thread; input is valid borrowed memory disjoint from the instance.
pub unsafe extern "C" fn on_config_changed<P: PluginV1>(
    instance: *mut c_void,
    input: SeSlice,
) -> SeStatus {
    boundary(|| {
        check_pointer(instance.cast::<P>())?;
        let input = unsafe { input_bytes(input)? };
        unsafe { &mut *instance.cast::<P>() }.on_config_changed(input)
    })
}

/// # Safety
/// Instance must be live, from `create::<P>`, exclusively owned on its creating
/// thread. It is consumed even on error/panic: never call destroy twice.
pub unsafe extern "C" fn destroy<P: PluginV1>(instance: *mut c_void) -> SeStatus {
    boundary(|| {
        check_pointer(instance.cast::<P>())?;
        let instance = unsafe { Box::from_raw(instance.cast::<P>()) };
        P::destroy(*instance)
    })
}

/// Shared entry body; the generated extern-C entry surrounds this with boundary.
///
/// # Safety
/// `out_api` must be an exclusively writable pointer slot for this call.
pub unsafe fn entry(
    min: u32,
    max: u32,
    out_api: *mut *const SePluginApiV1,
    api: &'static SePluginApiV1,
) -> Result<(), SeStatus> {
    check_pointer(out_api)?;
    unsafe {
        *out_api = std::ptr::null();
    }
    if min > SE_ABI_VERSION || max < SE_ABI_VERSION || min > max {
        return Err(SE_ABI_UNSUPPORTED);
    }
    unsafe {
        *out_api = api;
    }
    Ok(())
}

/// Export exactly one `se_plugin_entry_v1` symbol and all required v1 callbacks.
/// Invoke once in the root of a Rust cdylib: `export_plugin!(CounterPlugin);`.
///
/// SDK plugins require panic=unwind, since abort cannot be converted to status 8.
/// The host still must isolate native abort, invalid memory and infinite loops
/// in Plugin Core; this macro is not a sandbox.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[cfg(panic = "abort")]
        compile_error!("se-plugin-abi SDK plugins require panic=unwind, not panic=abort");

        const _: () = {
            static API: $crate::SePluginApiV1 = $crate::SePluginApiV1 {
                abi_version: $crate::SE_ABI_VERSION,
                struct_size: ::core::mem::size_of::<$crate::SePluginApiV1>() as u32,
                manifest: Some($crate::export::manifest::<$plugin>),
                create: Some($crate::export::create::<$plugin>),
                render_view: Some($crate::export::render_view::<$plugin>),
                on_action: Some($crate::export::on_action::<$plugin>),
                on_command: Some($crate::export::on_command::<$plugin>),
                on_config_changed: Some($crate::export::on_config_changed::<$plugin>),
                on_service_call: Some($crate::export::on_service_call::<$plugin>),
                destroy: Some($crate::export::destroy::<$plugin>),
            };

            /// # Safety
            /// `out_api` must be a valid, aligned writable pointer slot. The host
            /// must preserve code mapping and honor all ABI callback contracts.
            #[no_mangle]
            pub unsafe extern "C" fn se_plugin_entry_v1(
                min: u32,
                max: u32,
                out_api: *mut *const $crate::SePluginApiV1,
            ) -> $crate::SeStatus {
                $crate::export::boundary(|| unsafe {
                    $crate::export::entry(min, max, out_api, &API)
                })
            }
        };
    };
}
