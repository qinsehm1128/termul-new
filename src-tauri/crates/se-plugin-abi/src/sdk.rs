use crate::*;
use std::marker::PhantomData;
use std::rc::Rc;

/// Thread-affine access to the host. This handle is deliberately neither Send
/// nor Sync; it may be retained in the plugin instance, but not a background
/// thread. Construction is unsafe because native context lifetimes are external.
///
/// ```compile_fail
/// fn needs_send<T: Send>() {}
/// needs_send::<se_plugin_abi::Host>();
/// ```
/// ```compile_fail
/// fn needs_sync<T: Sync>() {}
/// needs_sync::<se_plugin_abi::Host>();
/// ```
pub struct Host {
    api: SeHostApiV1,
    _thread_affine: PhantomData<Rc<()>>,
}

impl Host {
    /// Copy the known v1 table after validating its prefix and callbacks.
    ///
    /// # Safety
    /// The table must satisfy `validate_host_api`'s readable-memory contract.
    /// Its callbacks/context must remain valid until this handle and the plugin
    /// instance are destroyed, including any call that outlives a host timeout.
    /// Callbacks must honor the ABI output/buffer contract and never unwind.
    pub unsafe fn from_raw(api: *const SeHostApiV1) -> Result<Self, SeStatus> {
        let api = *unsafe { validate_host_api(api) }.map_err(AbiValidationError::status)?;
        Ok(Self {
            api,
            _thread_affine: PhantomData,
        })
    }

    pub fn log(&self, level: u32, message: &str) -> Result<(), SeStatus> {
        if !(SE_LOG_ERROR..=SE_LOG_DEBUG).contains(&level) {
            return Err(SE_INVALID_ARGUMENT);
        }
        // from_raw establishes callback/context validity. No borrowed input is
        // retained by a conforming host callback.
        let status = unsafe {
            (self.api.log.ok_or(SE_UNAVAILABLE)?)(
                self.api.host_context,
                level,
                SeSlice::from_bytes(message.as_bytes()),
            )
        };
        status_result(status)
    }

    /// Call once, never resize/retry: the request may have side effects.
    /// JSON shape validation belongs to the host/protocol, not this std-only SDK.
    pub fn broker_call(&self, request: &[u8], output: &mut [u8]) -> Result<usize, SeStatus> {
        std::str::from_utf8(request).map_err(|_| SE_INVALID_JSON)?;
        let mut raw = SeOutput {
            ptr: output.as_mut_ptr(),
            capacity: output.len(),
            len: 0,
        };
        let status = unsafe {
            (self.api.broker_call.ok_or(SE_UNAVAILABLE)?)(
                self.api.host_context,
                SeSlice::from_bytes(request),
                &mut raw,
            )
        };
        status_result(status)?;
        if raw.len > output.len() || raw.ptr != output.as_mut_ptr() || raw.capacity != output.len()
        {
            return Err(SE_INTERNAL);
        }
        Ok(raw.len)
    }
}

/// An exclusive call-scoped view of a caller-owned buffer. `write` writes a
/// complete result (not a stream); insufficient capacity sets the required len
/// and copies no bytes. No allocation, implicit callback or retry occurs.
pub struct OutputWriter<'a> {
    output: &'a mut SeOutput,
    _buffer: PhantomData<&'a mut [u8]>,
}

impl<'a> OutputWriter<'a> {
    /// # Safety
    /// `output` must be a live, aligned, exclusively borrowed descriptor for
    /// `'a`; a nonzero-capacity buffer must be writable for that capacity and
    /// disjoint from this descriptor, inputs, and plugin state. Neither pointer
    /// may be retained after the call. Capacity must be at most isize::MAX.
    pub unsafe fn from_raw(output: *mut SeOutput) -> Result<Self, SeStatus> {
        check_pointer(output)?;
        let output = unsafe { &mut *output };
        output.len = 0;
        if output.capacity > isize::MAX as usize || (output.ptr.is_null() && output.capacity != 0) {
            return Err(SE_INVALID_ARGUMENT);
        }
        Ok(Self {
            output,
            _buffer: PhantomData,
        })
    }

    pub fn capacity(&self) -> usize {
        self.output.capacity
    }
    pub fn len(&self) -> usize {
        self.output.len
    }
    pub fn is_empty(&self) -> bool {
        self.output.len == 0
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), SeStatus> {
        self.output.len = bytes.len();
        if bytes.len() > self.output.capacity {
            return Err(SE_BUFFER_TOO_SMALL);
        }
        if !bytes.is_empty() {
            // The constructor guarantees valid exclusive storage. memmove also
            // permits overlap for callers implementing native buffer transforms.
            unsafe {
                std::ptr::copy(bytes.as_ptr(), self.output.ptr, bytes.len());
            }
        }
        Ok(())
    }
}

pub trait PluginV1: Sized + 'static {
    fn manifest() -> &'static [u8];
    fn create(host: Host, activation_json: &[u8]) -> Result<Self, SeStatus>;
    fn render_view(&mut self, _json: &[u8], _out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        Err(SE_UNAVAILABLE)
    }
    fn on_action(&mut self, _json: &[u8], _out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        Err(SE_UNAVAILABLE)
    }
    fn on_command(&mut self, _json: &[u8], _out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        Err(SE_UNAVAILABLE)
    }
    fn on_config_changed(&mut self, _json: &[u8]) -> Result<(), SeStatus> {
        Err(SE_UNAVAILABLE)
    }
    fn on_service_call(
        &mut self,
        _json: &[u8],
        _out: &mut OutputWriter<'_>,
    ) -> Result<(), SeStatus> {
        Err(SE_UNAVAILABLE)
    }
    fn destroy(self) -> Result<(), SeStatus> {
        Ok(())
    }
}

pub(crate) fn status_result(status: SeStatus) -> Result<(), SeStatus> {
    if status == SE_OK {
        Ok(())
    } else {
        Err(status)
    }
}

/// # Safety
/// Nonempty slices must reference initialized, readable bytes for `'a`; their
/// storage must not alias any mutable instance/output accessed during the call.
pub(crate) unsafe fn input_bytes<'a>(input: SeSlice) -> Result<&'a [u8], SeStatus> {
    if input.len > isize::MAX as usize || (input.ptr.is_null() && input.len != 0) {
        return Err(SE_INVALID_ARGUMENT);
    }
    let bytes = if input.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input.ptr, input.len) }
    };
    std::str::from_utf8(bytes).map_err(|_| SE_INVALID_JSON)?;
    Ok(bytes)
}
