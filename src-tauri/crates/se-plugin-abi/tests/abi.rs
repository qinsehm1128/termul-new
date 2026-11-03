use se_plugin_abi::*;
use std::cell::Cell;
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::ptr::{null, null_mut};

thread_local! {
    static PANIC_AT: Cell<&'static str> = const { Cell::new("") };
    static EFFECTS: Cell<usize> = const { Cell::new(0) };
    static DROPS: Cell<usize> = const { Cell::new(0) };
}
fn maybe_panic(name: &str) {
    PANIC_AT.with(|at| assert_ne!(at.get(), name, "injected {name} panic"));
}

struct TestPlugin;
impl Drop for TestPlugin {
    fn drop(&mut self) {
        DROPS.with(|n| n.set(n.get() + 1));
    }
}
impl PluginV1 for TestPlugin {
    fn manifest() -> &'static [u8] {
        maybe_panic("manifest");
        b"{}"
    }
    fn create(_host: Host, activation: &[u8]) -> Result<Self, SeStatus> {
        let plugin = Self;
        maybe_panic("create");
        if activation == b"fail" {
            return Err(SE_INTERNAL);
        }
        Ok(plugin)
    }
    fn render_view(&mut self, _: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        maybe_panic("render_view");
        out.write(b"{}")
    }
    fn on_action(&mut self, _: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        maybe_panic("on_action");
        EFFECTS.with(|n| n.set(n.get() + 1));
        out.write(b"12345")
    }
    fn on_command(&mut self, _: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        maybe_panic("on_command");
        out.write(b"{}")
    }
    fn on_config_changed(&mut self, _: &[u8]) -> Result<(), SeStatus> {
        maybe_panic("on_config_changed");
        Ok(())
    }
    fn on_service_call(&mut self, _: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        maybe_panic("on_service_call");
        out.write(b"{}")
    }
    fn destroy(self) -> Result<(), SeStatus> {
        maybe_panic("destroy");
        Ok(())
    }
}
export_plugin!(TestPlugin);

extern "C" {
    #[link_name = "se_plugin_entry_v1"]
    fn invoke_entry(min: u32, max: u32, api: *mut *const SePluginApiV1) -> SeStatus;
}
unsafe extern "C" fn log(_: *mut c_void, _: u32, _: SeSlice) -> SeStatus {
    SE_OK
}
unsafe extern "C" fn broker(_: *mut c_void, _: SeSlice, out: *mut SeOutput) -> SeStatus {
    export::boundary(|| {
        EFFECTS.with(|n| n.set(n.get() + 1));
        unsafe { OutputWriter::from_raw(out)? }.write(b"12345")
    })
}
fn host_api() -> SeHostApiV1 {
    SeHostApiV1 {
        abi_version: 1,
        struct_size: size_of::<SeHostApiV1>() as u32,
        host_context: null_mut(),
        log: Some(log),
        broker_call: Some(broker),
    }
}
fn api() -> &'static SePluginApiV1 {
    let mut ptr = null();
    assert_eq!(unsafe { invoke_entry(1, 1, &mut ptr) }, SE_OK);
    unsafe { validate_plugin_api(ptr) }.unwrap()
}
fn instance(api: &SePluginApiV1) -> *mut c_void {
    let mut instance = null_mut();
    assert_eq!(
        unsafe { api.create.unwrap()(&host_api(), SeSlice::from_bytes(b"{}"), &mut instance) },
        SE_OK
    );
    assert!(!instance.is_null());
    instance
}
fn output(bytes: &mut [u8]) -> SeOutput {
    SeOutput {
        ptr: bytes.as_mut_ptr(),
        capacity: bytes.len(),
        len: 99,
    }
}

#[test]
fn exact_layout_and_status_codes() {
    assert_eq!(
        [
            SE_OK,
            SE_INVALID_ARGUMENT,
            SE_ABI_UNSUPPORTED,
            SE_BUFFER_TOO_SMALL,
            SE_INVALID_JSON,
            SE_NOT_FOUND,
            SE_UNAVAILABLE,
            SE_BUSY,
            SE_PANIC,
            SE_INTERNAL,
            SE_DEADLINE_EXCEEDED,
            SE_STALE_GENERATION,
            SE_REENTRANT
        ],
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!(size_of::<SeAbiHeader>(), 8);
    assert_eq!(offset_of!(SeAbiHeader, abi_version), 0);
    assert_eq!(offset_of!(SeAbiHeader, struct_size), 4);
    assert_eq!(offset_of!(SeSlice, ptr), 0);
    assert_eq!(offset_of!(SeSlice, len), size_of::<usize>());
    assert_eq!(size_of::<SeSlice>(), 2 * size_of::<usize>());
    assert_eq!(offset_of!(SeOutput, ptr), 0);
    assert_eq!(offset_of!(SeOutput, capacity), size_of::<usize>());
    assert_eq!(offset_of!(SeOutput, len), 2 * size_of::<usize>());
    assert_eq!(size_of::<SeOutput>(), 3 * size_of::<usize>());
    let p = size_of::<usize>();
    assert_eq!(offset_of!(SeHostApiV1, abi_version), 0);
    assert_eq!(offset_of!(SeHostApiV1, struct_size), 4);
    assert_eq!(offset_of!(SeHostApiV1, host_context), 8);
    assert_eq!(offset_of!(SeHostApiV1, log), 8 + p);
    assert_eq!(offset_of!(SeHostApiV1, broker_call), 8 + 2 * p);
    assert_eq!(size_of::<SeHostApiV1>(), 8 + 3 * p);
    assert_eq!(offset_of!(SePluginApiV1, abi_version), 0);
    assert_eq!(offset_of!(SePluginApiV1, struct_size), 4);
    assert_eq!(
        [
            offset_of!(SePluginApiV1, manifest),
            offset_of!(SePluginApiV1, create),
            offset_of!(SePluginApiV1, render_view),
            offset_of!(SePluginApiV1, on_action),
            offset_of!(SePluginApiV1, on_command),
            offset_of!(SePluginApiV1, on_config_changed),
            offset_of!(SePluginApiV1, on_service_call),
            offset_of!(SePluginApiV1, destroy)
        ],
        [
            8,
            8 + p,
            8 + 2 * p,
            8 + 3 * p,
            8 + 4 * p,
            8 + 5 * p,
            8 + 6 * p,
            8 + 7 * p
        ]
    );
    assert_eq!(size_of::<SePluginApiV1>(), 8 + 8 * p);
    assert_eq!(align_of::<SePluginApiV1>(), align_of::<usize>());
    assert_eq!(
        [SE_LOG_ERROR, SE_LOG_WARN, SE_LOG_INFO, SE_LOG_DEBUG],
        [1, 2, 3, 4]
    );
}

#[test]
fn entry_negotiates_and_clears_errors() {
    let mut ptr = api() as *const _;
    for (min, max) in [(2, 2), (0, 0), (3, 1)] {
        assert_eq!(
            unsafe { invoke_entry(min, max, &mut ptr) },
            SE_ABI_UNSUPPORTED
        );
        assert!(ptr.is_null());
    }
    assert_eq!(unsafe { invoke_entry(0, 2, &mut ptr) }, SE_OK);
    assert!(!ptr.is_null());
    assert_eq!(
        unsafe { invoke_entry(1, 1, null_mut()) },
        SE_INVALID_ARGUMENT
    );
    // The entry contains no plugin-defined code; its panic fence is also used
    // by each trampoline, tested with an injected Rust panic here.
    assert_eq!(export::boundary(|| panic!("entry-boundary")), SE_PANIC);
}

#[test]
fn rejects_headers_before_forming_full_references() {
    #[repr(C, align(16))]
    struct HeaderOnly(SeAbiHeader);
    for (version, len, expected) in [(2, 8, SE_ABI_UNSUPPORTED), (1, 8, SE_ABI_UNSUPPORTED)] {
        let header = HeaderOnly(SeAbiHeader {
            abi_version: version,
            struct_size: len,
        });
        assert_eq!(
            unsafe { validate_plugin_api((&header.0 as *const SeAbiHeader).cast()) }
                .err()
                .unwrap()
                .status(),
            expected
        );
        assert_eq!(
            unsafe { validate_host_api((&header.0 as *const SeAbiHeader).cast()) }
                .err()
                .unwrap()
                .status(),
            expected
        );
    }
    assert_eq!(
        unsafe { validate_plugin_api(null()) }.err(),
        Some(AbiValidationError::NullPointer)
    );
    assert_eq!(
        unsafe { validate_host_api(null()) }.err(),
        Some(AbiValidationError::NullPointer)
    );
    let mut enlarged = *api();
    enlarged.struct_size += 32;
    assert!(unsafe { validate_plugin_api(&enlarged) }.is_ok());
    let mut host = host_api();
    host.struct_size += 32;
    assert!(unsafe { validate_host_api(&host) }.is_ok());
    let aligned = [0u64; 20];
    let misaligned = unsafe { aligned.as_ptr().cast::<u8>().add(1) };
    assert_eq!(
        unsafe { validate_plugin_api(misaligned.cast()) }.err(),
        Some(AbiValidationError::MisalignedPointer)
    );
}

#[test]
fn every_required_null_callback_is_rejected() {
    macro_rules! missing {
        ($($field:ident),+) => { $(
            let mut table = *api(); table.$field = None;
            assert_eq!(unsafe { validate_plugin_api(&table) }.err(),
                Some(AbiValidationError::MissingCallback(stringify!($field))));
        )+ };
    }
    missing!(
        manifest,
        create,
        render_view,
        on_action,
        on_command,
        on_config_changed,
        on_service_call,
        destroy
    );
    let mut host = host_api();
    host.log = None;
    assert_eq!(
        unsafe { validate_host_api(&host) }.err(),
        Some(AbiValidationError::MissingCallback("log"))
    );
    let mut host = host_api();
    host.broker_call = None;
    assert_eq!(
        unsafe { validate_host_api(&host) }.err(),
        Some(AbiValidationError::MissingCallback("broker_call"))
    );
}

#[test]
fn all_plugin_panics_are_contained_and_destroy_consumes_once() {
    let api = api();
    let mut bytes = [0; 32];
    let mut out = output(&mut bytes);
    PANIC_AT.with(|at| at.set("manifest"));
    assert_eq!(unsafe { api.manifest.unwrap()(&mut out) }, SE_PANIC);
    PANIC_AT.with(|at| at.set("create"));
    let before = DROPS.with(Cell::get);
    let mut ptr = std::ptr::dangling_mut();
    assert_eq!(
        unsafe { api.create.unwrap()(&host_api(), SeSlice::from_bytes(b"{}"), &mut ptr) },
        SE_PANIC
    );
    assert!(ptr.is_null());
    assert_eq!(DROPS.with(Cell::get), before + 1);
    PANIC_AT.with(|at| at.set(""));
    let ptr = instance(api);
    for (name, callback) in [
        ("render_view", api.render_view.unwrap()),
        ("on_action", api.on_action.unwrap()),
        ("on_command", api.on_command.unwrap()),
        ("on_service_call", api.on_service_call.unwrap()),
    ] {
        PANIC_AT.with(|at| at.set(name));
        assert_eq!(
            unsafe { callback(ptr, SeSlice::from_bytes(b"{}"), &mut out) },
            SE_PANIC
        );
    }
    PANIC_AT.with(|at| at.set("on_config_changed"));
    assert_eq!(
        unsafe { api.on_config_changed.unwrap()(ptr, SeSlice::from_bytes(b"{}")) },
        SE_PANIC
    );
    PANIC_AT.with(|at| at.set("destroy"));
    let before = DROPS.with(Cell::get);
    assert_eq!(unsafe { api.destroy.unwrap()(ptr) }, SE_PANIC);
    assert_eq!(DROPS.with(Cell::get), before + 1);
    PANIC_AT.with(|at| at.set(""));
}

struct PanicOnDrop;
impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        panic!("instance destructor");
    }
}
impl PluginV1 for PanicOnDrop {
    fn manifest() -> &'static [u8] {
        b"{}"
    }
    fn create(_: Host, _: &[u8]) -> Result<Self, SeStatus> {
        Ok(Self)
    }
}
#[test]
fn destructor_panic_is_inside_destroy_boundary() {
    let mut instance = null_mut();
    assert_eq!(
        unsafe {
            export::create::<PanicOnDrop>(&host_api(), SeSlice::from_bytes(b"{}"), &mut instance)
        },
        SE_OK
    );
    assert_eq!(
        unsafe { export::destroy::<PanicOnDrop>(instance) },
        SE_PANIC
    );
}

#[test]
fn invalid_host_tables_fail_create_and_clear_instance() {
    let api = api();
    for host in [
        SeHostApiV1 {
            abi_version: 2,
            ..host_api()
        },
        SeHostApiV1 {
            struct_size: 8,
            ..host_api()
        },
        SeHostApiV1 {
            log: None,
            ..host_api()
        },
        SeHostApiV1 {
            broker_call: None,
            ..host_api()
        },
    ] {
        let mut instance = std::ptr::dangling_mut();
        assert_ne!(
            unsafe { api.create.unwrap()(&host, SeSlice::from_bytes(b"{}"), &mut instance) },
            SE_OK
        );
        assert!(instance.is_null());
    }
}

#[test]
fn panic_payload_drop_cannot_unwind_again() {
    struct BadDrop;
    impl Drop for BadDrop {
        fn drop(&mut self) {
            panic!("payload destructor");
        }
    }
    assert_eq!(
        export::boundary(|| std::panic::panic_any(BadDrop)),
        SE_PANIC
    );
}

#[test]
fn null_outputs_instances_and_bad_inputs_never_enter_plugin() {
    let api = api();
    let ptr = instance(api);
    let mut bytes = [0; 8];
    let mut out = output(&mut bytes);
    let empty = SeSlice {
        ptr: null(),
        len: 0,
    };
    assert_eq!(
        unsafe { api.manifest.unwrap()(null_mut()) },
        SE_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { api.create.unwrap()(&host_api(), empty, null_mut()) },
        SE_INVALID_ARGUMENT
    );
    for callback in [
        api.render_view.unwrap(),
        api.on_action.unwrap(),
        api.on_command.unwrap(),
        api.on_service_call.unwrap(),
    ] {
        assert_eq!(
            unsafe { callback(ptr, empty, null_mut()) },
            SE_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { callback(null_mut(), empty, &mut out) },
            SE_INVALID_ARGUMENT
        );
        assert_eq!(out.len, 0);
        assert_eq!(
            unsafe {
                callback(
                    ptr,
                    SeSlice {
                        ptr: null(),
                        len: 1,
                    },
                    &mut out,
                )
            },
            SE_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { callback(ptr, SeSlice::from_bytes(&[0xff]), &mut out) },
            SE_INVALID_JSON
        );
    }
    assert_eq!(
        unsafe { api.on_config_changed.unwrap()(null_mut(), empty) },
        SE_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { api.on_config_changed.unwrap()(ptr, SeSlice::from_bytes(&[0xff])) },
        SE_INVALID_JSON
    );
    assert_eq!(
        unsafe { api.destroy.unwrap()(null_mut()) },
        SE_INVALID_ARGUMENT
    );
    let mut bad_out = SeOutput {
        ptr: null_mut(),
        capacity: 1,
        len: 9,
    };
    assert_eq!(
        unsafe { api.manifest.unwrap()(&mut bad_out) },
        SE_INVALID_ARGUMENT
    );
    assert_eq!(unsafe { api.destroy.unwrap()(ptr) }, SE_OK);
    let mut ptr = std::ptr::dangling_mut();
    assert_eq!(
        unsafe { api.create.unwrap()(null(), empty, &mut ptr) },
        SE_INVALID_ARGUMENT
    );
    assert!(ptr.is_null());
    let mut ptr = std::ptr::dangling_mut();
    let before = DROPS.with(Cell::get);
    assert_eq!(
        unsafe { api.create.unwrap()(&host_api(), SeSlice::from_bytes(b"fail"), &mut ptr) },
        SE_INTERNAL
    );
    assert!(ptr.is_null());
    assert_eq!(DROPS.with(Cell::get), before + 1);
}

#[test]
fn buffer_too_small_does_not_retry_or_overwrite_and_host_calls_once() {
    let api = api();
    let ptr = instance(api);
    EFFECTS.with(|n| n.set(0));
    let mut bytes = [0xab; 4];
    let mut out = output(&mut bytes[1..3]);
    let original = (out.ptr, out.capacity);
    assert_eq!(
        unsafe { api.on_action.unwrap()(ptr, SeSlice::from_bytes(b"{}"), &mut out) },
        SE_BUFFER_TOO_SMALL
    );
    assert_eq!(out.len, 5);
    assert_eq!((out.ptr, out.capacity), original);
    assert_eq!(bytes, [0xab; 4]);
    assert_eq!(EFFECTS.with(Cell::get), 1);
    assert_eq!(unsafe { api.destroy.unwrap()(ptr) }, SE_OK);
    let host = unsafe { Host::from_raw(&host_api()) }.unwrap();
    assert_eq!(host.broker_call(b"{}", &mut []), Err(SE_BUFFER_TOO_SMALL));
    assert_eq!(EFFECTS.with(Cell::get), 2);
    assert_eq!(host.broker_call(b"{}", &mut [0; 5]), Ok(5));
    assert_eq!(EFFECTS.with(Cell::get), 3);
    assert_eq!(host.log(SE_LOG_INFO, "hello"), Ok(()));
    assert_eq!(host.log(0, "no"), Err(SE_INVALID_ARGUMENT));
    assert_eq!(host.broker_call(&[0xff], &mut []), Err(SE_INVALID_JSON));
    assert_eq!(EFFECTS.with(Cell::get), 3);
}

#[test]
fn export_macro_rejects_abort_profile_in_downstream_crate() {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let library = std::fs::read_dir(&deps)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("libse_plugin_abi-") && name.ends_with(".rlib")
        })
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())
        .expect("cargo-built ABI rlib alongside integration test")
        .path();
    let scratch = std::env::temp_dir().join(format!(
        "se-plugin-abi-abort-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let result =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args([
                "--edition=2021",
                "--crate-type=cdylib",
                "--emit=metadata",
                "-C",
                "panic=abort",
            ])
            .arg("--extern")
            .arg(format!("se_plugin_abi={}", library.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(scratch.join("abort.rmeta"))
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/panic_abort.rs"
            ))
            .output()
            .expect("run rustc against panic=abort fixture");
    std::fs::remove_dir_all(&scratch).unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("SDK plugins require panic=unwind, not panic=abort"),
        "{stderr}"
    );
}

#[test]
fn output_zero_capacity_and_exact_capacity() {
    let mut raw = SeOutput {
        ptr: null_mut(),
        capacity: 0,
        len: 99,
    };
    let mut writer = unsafe { OutputWriter::from_raw(&mut raw) }.unwrap();
    assert_eq!(writer.write(b""), Ok(()));
    assert_eq!(writer.write(b"x"), Err(SE_BUFFER_TOO_SMALL));
    assert_eq!(raw.len, 1);
    let mut bytes = [0; 2];
    let mut raw = output(&mut bytes);
    let mut writer = unsafe { OutputWriter::from_raw(&mut raw) }.unwrap();
    assert_eq!(writer.write(b"{}"), Ok(()));
    assert_eq!(writer.len(), 2);
    assert_eq!(bytes, *b"{}");
}

struct Defaults;
impl PluginV1 for Defaults {
    fn manifest() -> &'static [u8] {
        b"{}"
    }
    fn create(_: Host, _: &[u8]) -> Result<Self, SeStatus> {
        Ok(Self)
    }
}
#[test]
fn unimplemented_callbacks_are_callable_unavailable_defaults() {
    let mut bytes = [0; 2];
    let mut raw = output(&mut bytes);
    let mut ptr = null_mut();
    assert_eq!(
        unsafe { export::create::<Defaults>(&host_api(), SeSlice::from_bytes(b"{}"), &mut ptr) },
        SE_OK
    );
    for callback in [
        export::render_view::<Defaults> as SeJsonCallbackFn,
        export::on_action::<Defaults>,
        export::on_command::<Defaults>,
        export::on_service_call::<Defaults>,
    ] {
        assert_eq!(
            unsafe { callback(ptr, SeSlice::from_bytes(b"{}"), &mut raw) },
            SE_UNAVAILABLE
        );
    }
    assert_eq!(
        unsafe { export::on_config_changed::<Defaults>(ptr, SeSlice::from_bytes(b"{}")) },
        SE_UNAVAILABLE
    );
    assert_eq!(unsafe { export::destroy::<Defaults>(ptr) }, SE_OK);
}
