use se_plugin_protocol::*;
use serde_json::{json, Value};

fn parse<T>(value: Value, parser: impl FnOnce(&[u8]) -> Result<T, PluginError>) -> T {
    parser(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn web_capability_is_optional_and_csp_is_explicitly_relaxed() {
    assert!(!PLUGIN_CAPABILITIES.contains(&WEB_UI_CAPABILITY));
    assert_eq!(WEB_UI_CAPABILITIES_V1, "plugins.web-ui.v1");
    for directive in [
        "'unsafe-inline'",
        "'unsafe-eval'",
        "se-plugin:",
        "http:",
        "https:",
        "ws:",
        "wss:",
        "data:",
        "blob:",
    ] {
        assert!(WEB_UI_CSP.contains(directive));
    }
    for service in [HOST_UI_SERVICE, HOST_WAKE_SERVICE, HOST_PROCESS_SERVICE] {
        assert_eq!(
            known_host_service(service, 1),
            Some(HostServiceAvailability::Available)
        );
        assert_eq!(known_host_service(service, 2), None);
    }
    assert_eq!(ViewKind::default(), ViewKind::UiTree);
    assert_eq!(
        WebAllow::ViewDispatch.required_service(),
        Some(HOST_UI_SERVICE)
    );
    assert_eq!(MAX_PACKAGE_ARCHIVE_BYTES, 128 * 1024 * 1024);
    assert_eq!(MAX_PACKAGE_EXPANDED_BYTES, 256 * 1024 * 1024);
}

#[test]
fn legacy_manifest_roundtrip_omits_new_optional_web_fields() {
    let value = include_bytes!("../../../../src/shared/plugin-fixtures/contracts.json");
    let fixtures: Value = serde_json::from_slice(value).unwrap();
    let source = fixtures
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "manifest" && v["valid"] == true)
        .unwrap()["value"]
        .clone();
    let manifest: Manifest = parse(source, parse_manifest);
    let wire = serde_json::to_value(manifest).unwrap();
    assert!(wire["contributes"].get("processes").is_none());
    for view in wire["contributes"]["views"].as_array().unwrap() {
        assert!(view.get("kind").is_none());
        assert!(view.get("entry").is_none());
        assert!(view.get("allow").is_none());
    }
}

#[test]
fn process_defaults_are_serialized_and_wake_delivery_reuses_existing_reply() {
    let process: ProcessContribution =
        serde_json::from_value(json!({"id":"server","command":"bin/server"})).unwrap();
    validate_process_contribution(&process).unwrap();
    assert!(process.autostart);
    assert_eq!(process.restart, ProcessRestart::OnFailure);
    assert!(process.args.is_empty() && process.env.is_empty());
    assert!(parse_host_wake_request(br#"{}"#).is_ok());
    assert!(parse_host_wake_request(br#"{"reason":null}"#).is_err());
    assert!(
        parse_host_delivery_reply(br#"{"schemaVersion":1,"result":{"invalidateViews":[]}}"#)
            .is_ok()
    );
}

#[test]
fn bridge_success_preserves_null_but_requires_a_result() {
    let base = json!({"channel":"se-plugin-ui","version":1,"sessionId":"12345678-1234-1234-1234-123456789abc","nonce":"a".repeat(64),"type":"response","requestId":"r","ok":true,"result":null});
    let value = parse(base.clone(), parse_web_bridge_envelope);
    assert_eq!(serde_json::to_value(value).unwrap(), base);
    let mut missing = base;
    missing.as_object_mut().unwrap().remove("result");
    assert!(parse_web_bridge_envelope(&serde_json::to_vec(&missing).unwrap()).is_err());
}

#[test]
fn package_and_web_path_boundaries_are_portable() {
    for good in [
        "index.html",
        "assets/app-v1.0.js",
        "a_1/x.png",
        "native/aarch64-apple-darwin/plugin.dylib",
    ] {
        assert!(is_package_path(good), "{good}");
    }
    for bad in [
        "/a",
        "a\\b",
        "a/../b",
        "a//b",
        "a/./b",
        "CON.txt",
        "nul",
        "COM9.js",
        "lpt1.bin",
        "a./b",
        "__SE__/shim.js",
        "a%20b",
        "a:b",
        "a?b",
        "a#b",
        "é.html",
    ] {
        assert!(!is_package_path(bad), "{bad}");
    }
    assert!(is_package_path(&"a".repeat(512)));
    assert!(!is_package_path(&"a".repeat(513)));
    assert!(is_web_entry("index.html"));
    assert!(!is_web_entry("index.HTML"));
}

#[test]
fn asset_base64_is_canonical_and_raw_asset_limit_is_separate() {
    let base = json!({"mime":"text/plain","sha256":"a".repeat(64),"size":1,"dataBase64":"YQ=="});
    parse(base.clone(), parse_web_asset_reply);
    for data in ["YR==", "YQ=", "YQ==\n", "????"] {
        let mut value = base.clone();
        value["dataBase64"] = data.into();
        assert!(parse_web_asset_reply(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    // Asset transport intentionally exceeds the ordinary 256 KiB JSON cap.
    let data = "AAAA".repeat(100_000);
    let bytes = serde_json::to_vec(&json!({"mime":"application/octet-stream","sha256":"a".repeat(64),"size":300_000,"dataBase64":data})).unwrap();
    assert!(bytes.len() > MAX_JSON_BYTES);
    assert!(parse_web_asset_reply(&bytes).is_ok());
}

#[test]
fn host_broker_directionality_is_strict_for_new_services() {
    for (service, method, input) in [
        (
            HOST_UI_SERVICE,
            "postMessage",
            json!({"viewId":"web","payload":null}),
        ),
        (HOST_WAKE_SERVICE, "request", json!({"reason":"timer"})),
        (HOST_PROCESS_SERVICE, "status", json!({"id":"server"})),
        (HOST_PROCESS_SERVICE, "start", json!({"id":"server"})),
        (HOST_PROCESS_SERVICE, "stop", json!({"id":"server"})),
        (HOST_PROCESS_SERVICE, "restart", json!({"id":"server"})),
        (
            HOST_PROCESS_SERVICE,
            "logs",
            json!({"id":"server","tail":500}),
        ),
    ] {
        let request =
            json!({"schemaVersion":1,"service":service,"version":1,"method":method,"input":input});
        assert!(
            parse_host_request(&serde_json::to_vec(&request).unwrap()).is_ok(),
            "{service}/{method}"
        );
    }
    let wake = json!({"schemaVersion":1,"service":HOST_WAKE_SERVICE,"version":1,"method":"wake","input":{"generation":"12345678-1234-1234-1234-123456789abc","count":1,"reasons":[]}});
    assert!(parse_host_request(&serde_json::to_vec(&wake).unwrap()).is_err());
    assert!(parse_host_delivery_request(&serde_json::to_vec(&wake).unwrap()).is_ok());
}
