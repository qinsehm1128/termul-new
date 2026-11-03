//! Native plugin ABI v1 and a std-only Rust SDK.
//!
//! Native pointers are not a sandbox. Validation checks nullness, alignment,
//! version, size and required callbacks, not whether arbitrary memory is mapped.
//! All calls for an instance must be serialized on its creating thread.

use core::ffi::c_void;
use core::mem::{align_of, size_of};

#[doc(hidden)]
pub mod export;
mod sdk;

pub use sdk::{Host, OutputWriter, PluginV1};

pub type SeStatus = i32;
pub const SEP_ABI_VERSION: u32 = 1;
pub const SEP_ENTRY_SYMBOL: &[u8] = b"se_plugin_entry_v1\0";
pub const SEP_OK: SeStatus = 0;
pub const SEP_INVALID_ARGUMENT: SeStatus = 1;
pub const SEP_ABI_UNSUPPORTED: SeStatus = 2;
pub const SEP_BUFFER_TOO_SMALL: SeStatus = 3;
pub const SEP_INVALID_JSON: SeStatus = 4;
pub const SEP_NOT_FOUND: SeStatus = 5;
pub const SEP_UNAVAILABLE: SeStatus = 6;
pub const SEP_BUSY: SeStatus = 7;
pub const SEP_PANIC: SeStatus = 8;
pub const SEP_INTERNAL: SeStatus = 9;
pub const SEP_DEADLINE_EXCEEDED: SeStatus = 10;
pub const SEP_STALE_GENERATION: SeStatus = 11;
pub const SEP_REENTRANT: SeStatus = 12;
pub const SEP_LOG_ERROR: u32 = 1;
pub const SEP_LOG_WARN: u32 = 2;
pub const SEP_LOG_INFO: u32 = 3;
pub const SEP_LOG_DEBUG: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeAbiHeader {
    pub abi_version: u32,
    pub struct_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SeSlice {
    pub ptr: *const u8,
    pub len: usize,
}

impl SeSlice {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct SeOutput {
    pub ptr: *mut u8,
    pub capacity: usize,
    pub len: usize,
}

pub type SeLogFn =
    unsafe extern "C" fn(host_context: *mut c_void, level: u32, message_utf8: SeSlice) -> SeStatus;
pub type SeBrokerCallFn = unsafe extern "C" fn(
    host_context: *mut c_void,
    request_json: SeSlice,
    output: *mut SeOutput,
) -> SeStatus;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SeHostApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub host_context: *mut c_void,
    pub log: Option<SeLogFn>,
    pub broker_call: Option<SeBrokerCallFn>,
}

pub type SeManifestFn = unsafe extern "C" fn(output: *mut SeOutput) -> SeStatus;
pub type SeCreateFn = unsafe extern "C" fn(
    host: *const SeHostApiV1,
    activation_json: SeSlice,
    out_instance: *mut *mut c_void,
) -> SeStatus;
pub type SeJsonCallbackFn = unsafe extern "C" fn(
    instance: *mut c_void,
    request_json: SeSlice,
    output: *mut SeOutput,
) -> SeStatus;
pub type SeConfigChangedFn =
    unsafe extern "C" fn(instance: *mut c_void, config_json: SeSlice) -> SeStatus;
pub type SeDestroyFn = unsafe extern "C" fn(instance: *mut c_void) -> SeStatus;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SePluginApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub manifest: Option<SeManifestFn>,
    pub create: Option<SeCreateFn>,
    pub render_view: Option<SeJsonCallbackFn>,
    pub on_action: Option<SeJsonCallbackFn>,
    pub on_command: Option<SeJsonCallbackFn>,
    pub on_config_changed: Option<SeConfigChangedFn>,
    pub on_service_call: Option<SeJsonCallbackFn>,
    pub destroy: Option<SeDestroyFn>,
}

pub type SeEntryFn = unsafe extern "C" fn(
    host_abi_min: u32,
    host_abi_max: u32,
    out_api: *mut *const SePluginApiV1,
) -> SeStatus;

/// Detailed negotiation failures for loaders (which may need distinct errors
/// for an unsupported ABI and an undersized structure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiValidationError {
    NullPointer,
    MisalignedPointer,
    UnsupportedVersion { actual: u32 },
    StructTooSmall { actual: u32, required: u32 },
    MissingCallback(&'static str),
}

impl AbiValidationError {
    pub fn status(self) -> SeStatus {
        match self {
            Self::UnsupportedVersion { .. } | Self::StructTooSmall { .. } => SEP_ABI_UNSUPPORTED,
            _ => SEP_INVALID_ARGUMENT,
        }
    }
}

impl core::fmt::Display for AbiValidationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid plugin ABI table: {self:?}")
    }
}
impl std::error::Error for AbiValidationError {}

pub(crate) fn check_pointer<T>(ptr: *const T) -> Result<(), SeStatus> {
    if ptr.is_null() || !(ptr as usize).is_multiple_of(align_of::<T>()) {
        Err(SEP_INVALID_ARGUMENT)
    } else {
        Ok(())
    }
}

/// Read only the two-field prefix before checking version and full-table size.
/// Extra trailing fields are deliberately ignored.
///
/// # Safety
/// `ptr`, when non-null and aligned, must point to a readable `SeAbiHeader`.
/// This does not dereference any bytes beyond that header, even on rejection.
pub unsafe fn validate_abi_header(
    ptr: *const SeAbiHeader,
    required_size: u32,
) -> Result<SeAbiHeader, AbiValidationError> {
    if ptr.is_null() {
        return Err(AbiValidationError::NullPointer);
    }
    if !(ptr as usize).is_multiple_of(align_of::<SeAbiHeader>()) {
        return Err(AbiValidationError::MisalignedPointer);
    }
    let header = unsafe { ptr.read() };
    if header.abi_version != SEP_ABI_VERSION {
        return Err(AbiValidationError::UnsupportedVersion {
            actual: header.abi_version,
        });
    }
    let required = required_size.max(size_of::<SeAbiHeader>() as u32);
    if header.struct_size < required {
        return Err(AbiValidationError::StructTooSmall {
            actual: header.struct_size,
            required,
        });
    }
    Ok(header)
}

unsafe fn validate_table_header<T>(ptr: *const T) -> Result<(), AbiValidationError> {
    if ptr.is_null() {
        return Err(AbiValidationError::NullPointer);
    }
    if !(ptr as usize).is_multiple_of(align_of::<T>()) {
        return Err(AbiValidationError::MisalignedPointer);
    }
    unsafe {
        validate_abi_header(ptr.cast(), size_of::<T>() as u32)?;
    }
    Ok(())
}

/// Validate a foreign plugin table without first forming a full-table reference.
///
/// # Safety
/// A non-null aligned pointer must expose a readable header. If its version and
/// size pass, the known v1 prefix must be initialized and readable for `'a`.
/// The library/code mapping must remain alive for every use of the returned API.
pub unsafe fn validate_plugin_api<'a>(
    ptr: *const SePluginApiV1,
) -> Result<&'a SePluginApiV1, AbiValidationError> {
    unsafe {
        validate_table_header(ptr)?;
    }
    let api = unsafe { &*ptr };
    macro_rules! required {
        ($($field:ident),+ $(,)?) => { $(
            if api.$field.is_none() {
                return Err(AbiValidationError::MissingCallback(stringify!($field)));
            }
        )+ };
    }
    required!(
        manifest,
        create,
        render_view,
        on_action,
        on_command,
        on_config_changed,
        on_service_call,
        destroy
    );
    Ok(api)
}

/// Validate the host table using the same header-first rules as the loader.
///
/// # Safety
/// As for `validate_plugin_api`, the header must be readable and, on accepted
/// version/size, the known prefix must be initialized and readable for `'a`.
/// Callbacks/context must outlive all uses by the plugin instance.
pub unsafe fn validate_host_api<'a>(
    ptr: *const SeHostApiV1,
) -> Result<&'a SeHostApiV1, AbiValidationError> {
    unsafe {
        validate_table_header(ptr)?;
    }
    let api = unsafe { &*ptr };
    if api.log.is_none() {
        return Err(AbiValidationError::MissingCallback("log"));
    }
    if api.broker_call.is_none() {
        return Err(AbiValidationError::MissingCallback("broker_call"));
    }
    Ok(api)
}
