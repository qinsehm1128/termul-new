//! The embedded manifest must pass the host's own parser and validator.
use se_plugin_protocol::manifest::parse_manifest;

#[test]
fn embedded_manifest_parses_and_validates() {
    let manifest = parse_manifest(qin_board_plugin::MANIFEST).expect("manifest parses");
    let value: serde_json::Value = serde_json::from_slice(qin_board_plugin::MANIFEST).unwrap();
    assert_eq!(value["id"], "org.qincode.board");
    let commands: Vec<_> = value["contributes"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(commands, ["endpoint", "restart-service", "open-board"]);
    assert_eq!(value["contributes"]["processes"][0]["restart"], "always");
    drop(manifest);
}

unsafe extern "C" fn noop_log(
    _ctx: *mut core::ffi::c_void,
    _level: u32,
    _msg: se_plugin_abi::SeSlice,
) -> se_plugin_abi::SeStatus {
    se_plugin_abi::SEP_OK
}

unsafe extern "C" fn fake_broker(
    _ctx: *mut core::ffi::c_void,
    req: se_plugin_abi::SeSlice,
    out: *mut se_plugin_abi::SeOutput,
) -> se_plugin_abi::SeStatus {
    let bytes = unsafe { core::slice::from_raw_parts(req.ptr, req.len) };
    let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let reply = match (v["service"].as_str(), v["method"].as_str()) {
        (Some("se.host.process"), Some("logs")) => serde_json::json!({
            "schemaVersion": 1,
            "result": {
                "ok": true,
                "value": {
                    "truncated": false,
                    "lines": [
                        {"stream": "stdout", "at": 1, "text": "QIN_BOARD_ENDPOINT {\"port\":50913,\"token\":\"abc123\"}\n"}
                    ]
                }
            }
        }),
        (Some("se.host.process"), Some("restart")) => serde_json::json!({
            "schemaVersion": 1,
            "result": {
                "ok": true,
                "value": {"state": "starting", "restarts": 1}
            }
        }),
        _ => return se_plugin_abi::SEP_UNAVAILABLE,
    };
    let encoded = serde_json::to_vec(&reply).unwrap();
    let out_ref = unsafe { &mut *out };
    unsafe { core::ptr::copy_nonoverlapping(encoded.as_ptr(), out_ref.ptr, encoded.len()) };
    out_ref.len = encoded.len();
    se_plugin_abi::SEP_OK
}

#[test]
fn native_callbacks_satisfy_host_protocol_contracts() {
    use core::ffi::c_void;
    use se_plugin_abi::*;
    use serde_json::json;

    let mut api_ptr: *const SePluginApiV1 = core::ptr::null();
    assert_eq!(
        unsafe {
            qin_board_plugin::se_plugin_entry_v1(SEP_ABI_VERSION, SEP_ABI_VERSION, &mut api_ptr)
        },
        SEP_OK
    );
    let api = unsafe { &*api_ptr };

    let host_api = SeHostApiV1 {
        abi_version: SEP_ABI_VERSION,
        struct_size: core::mem::size_of::<SeHostApiV1>() as u32,
        host_context: core::ptr::dangling_mut::<u8>().cast::<c_void>(),
        log: Some(noop_log),
        broker_call: Some(fake_broker),
    };
    let activation = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "pluginId": "org.qincode.board",
        "generation": "01234567-89ab-4def-8123-456789abcdef",
        "config": {"revision": 1, "values": {}, "changedKeys": []}
    }))
    .unwrap();
    let mut instance: *mut c_void = core::ptr::null_mut();
    assert_eq!(
        unsafe {
            (api.create.unwrap())(&host_api, SeSlice::from_bytes(&activation), &mut instance)
        },
        SEP_OK
    );

    let mut buf = vec![0u8; 64 * 1024];
    for (cmd, expected_ready) in [
        ("endpoint", Some(true)),
        ("restart-service", None),
        ("open-board", None),
    ] {
        let req = serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "commandId": cmd,
            "args": {}
        }))
        .unwrap();
        let mut out = SeOutput {
            ptr: buf.as_mut_ptr(),
            capacity: buf.len(),
            len: 0,
        };
        assert_eq!(
            unsafe { (api.on_command.unwrap())(instance, SeSlice::from_bytes(&req), &mut out) },
            SEP_OK
        );
        let parsed = se_plugin_protocol::parse_command_reply(&buf[..out.len])
            .unwrap_or_else(|e| panic!("command {cmd} reply failed host validation: {e:?}"));
        assert!(parsed.invalidate_views.is_empty());
        if let Some(ready) = expected_ready {
            assert_eq!(parsed.result["ready"], ready);
            assert_eq!(parsed.result["port"], 50913);
            assert_eq!(parsed.result["token"], "abc123");
        }
    }

    for delivery in [
        json!({"schemaVersion": 1, "service": "se.host.wake", "version": 1, "method": "wake", "input": {"generation": "01234567-89ab-4def-8123-456789abcdef", "count": 1, "reasons": []}}),
        json!({"schemaVersion": 1, "service": "se.host.events", "version": 1, "method": "deliver", "input": {"topic": "project.activeChanged"}}),
        json!({"schemaVersion": 1, "service": "se.host.storage", "version": 1, "method": "completed", "input": {"ticketId": "t1"}}),
    ] {
        let req = serde_json::to_vec(&delivery).unwrap();
        let mut out = SeOutput {
            ptr: buf.as_mut_ptr(),
            capacity: buf.len(),
            len: 0,
        };
        assert_eq!(
            unsafe {
                (api.on_service_call.unwrap())(instance, SeSlice::from_bytes(&req), &mut out)
            },
            SEP_OK
        );
        let parsed = se_plugin_protocol::parse_host_delivery_reply(&buf[..out.len])
            .expect("host delivery reply must pass host validation");
        assert!(parsed.result.invalidate_views.is_empty());
    }

    let cfg = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "revision": 2,
        "values": {},
        "changedKeys": []
    }))
    .unwrap();
    assert_eq!(
        unsafe { (api.on_config_changed.unwrap())(instance, SeSlice::from_bytes(&cfg)) },
        SEP_OK
    );

    assert_eq!(unsafe { (api.destroy.unwrap())(instance) }, SEP_OK);
}
