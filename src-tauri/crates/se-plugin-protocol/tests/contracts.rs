use se_plugin_protocol::*;
use serde::Deserialize;
use serde_json::{json, Value};

const FIXTURES: &str = include_str!("../../../../src/shared/plugin-fixtures/contracts.json");
const UUID: &str = "12345678-1234-1234-1234-123456789abc";
#[derive(Deserialize)]
struct Fixture {
    kind: String,
    valid: bool,
    name: String,
    value: Value,
}
fn fixtures() -> Vec<Fixture> {
    serde_json::from_str(FIXTURES).unwrap()
}
fn valid_value(kind: &str) -> Value {
    fixtures()
        .into_iter()
        .find(|f| f.kind == kind && f.valid)
        .unwrap()
        .value
}
fn manifest() -> Manifest {
    parse_manifest(&serde_json::to_vec(&valid_value("manifest")).unwrap()).unwrap()
}
fn tree(root: Value) -> UiTree {
    parse_ui_tree(
        &serde_json::to_vec(&json!({"schemaVersion":1,"viewId":"main","root":root})).unwrap(),
    )
    .unwrap()
}
fn snapshot(root: Value) -> ViewSnapshot {
    ViewSnapshot {
        plugin_id: "org.test.plugin".into(),
        generation: UUID.into(),
        revision: 1,
        config_revision: 1,
        tree: tree(root),
    }
}
fn action() -> ActionEvent {
    ActionEvent {
        schema_version: 1,
        event_id: UUID.into(),
        plugin_id: "org.test.plugin".into(),
        generation: UUID.into(),
        view_id: "main".into(),
        view_revision: 1,
        node_id: "control".into(),
        action_id: "do-it".into(),
        kind: ActionKind::Click,
        value: ActionValue::Null,
    }
}
#[test]
fn shared_rust_typescript_fixtures() {
    let fixtures = fixtures();
    for kind in ["manifest", "ui", "config", "action"] {
        assert!(
            fixtures.iter().any(|f| f.kind == kind && f.valid),
            "missing valid {kind} fixture"
        );
        assert!(
            fixtures.iter().any(|f| f.kind == kind && !f.valid),
            "missing invalid {kind} fixture"
        );
    }
    for f in fixtures {
        let bytes = serde_json::to_vec(&f.value).unwrap();
        let valid = match f.kind.as_str() {
            "packageDescriptor" => parse_package_descriptor(&bytes).is_ok(),
            "webOpenRequest" => parse_web_open_request(&bytes).is_ok(),
            "webOpenReply" => parse_web_open_reply(&bytes).is_ok(),
            "webRequest" => parse_web_request(&bytes).is_ok(),
            "webReadRequest" => parse_web_read_request(&bytes).is_ok(),
            "webReadReply" => parse_web_read_reply(&bytes).is_ok(),
            "webCloseRequest" => parse_web_close_request(&bytes).is_ok(),
            "webCloseReply" => parse_web_close_reply(&bytes).is_ok(),
            "webAssetRequest" => parse_web_asset_request(&bytes).is_ok(),
            "webAssetReply" => parse_web_asset_reply(&bytes).is_ok(),
            "webBridgeEnvelope" => parse_web_bridge_envelope(&bytes).is_ok(),
            "hostUiPostMessageRequest" => parse_host_ui_post_message_request(&bytes).is_ok(),
            "hostUiPostMessageReply" => parse_host_ui_post_message_reply(&bytes).is_ok(),
            "hostWakeRequest" => parse_host_wake_request(&bytes).is_ok(),
            "hostWakeDelivery" => parse_host_wake_delivery(&bytes).is_ok(),
            "hostProcessRequest" => parse_host_process_request(&bytes).is_ok(),
            "hostProcessStatus" => parse_host_process_status(&bytes).is_ok(),
            "hostProcessLogsRequest" => parse_host_process_logs_request(&bytes).is_ok(),
            "hostProcessLogsReply" => parse_host_process_logs_reply(&bytes).is_ok(),
            "manifest" => parse_manifest(&bytes).is_ok(),
            "ui" => parse_ui_tree(&bytes).is_ok(),
            "action" => parse_action_event(&bytes).is_ok(),
            "config" => serde_json::from_value::<ConfigurationSchema>(f.value["schema"].clone())
                .ok()
                .is_some_and(|schema| {
                    parse_config_payload(&serde_json::to_vec(&f.value["payload"]).unwrap(), &schema)
                        .is_ok()
                }),
            "contextSnapshotRequest" => parse_host_context_snapshot_request(&bytes).is_ok(),
            "hostUiGet" => parse_host_ui_get_request(&bytes).is_ok(),
            "hostAccepted" => parse_host_accepted(&bytes).is_ok(),
            "notificationCloseResult" => parse_notification_close_result(&bytes).is_ok(),
            "statusClearResult" => parse_status_clear_result(&bytes).is_ok(),
            "hostDeliveryRequest" => parse_host_delivery_request(&bytes).is_ok(),
            "hostRequest" => parse_host_request(&bytes).is_ok(),
            "hostReply" => parse_host_reply(&bytes).is_ok(),
            "hostContextPublish" => parse_host_context_publish_request(&bytes).is_ok(),
            "hostContextHeartbeat" => parse_host_context_heartbeat_request(&bytes).is_ok(),
            "hostContextAck" => parse_host_context_ack(&bytes).is_ok(),
            "hostContextState" => parse_host_context_state(&bytes).is_ok(),
            "hostContextSnapshot" => parse_host_context_snapshot(&bytes).is_ok(),
            "hostSourceEvent" => parse_host_source_event(&bytes).is_ok(),
            "hostEventDelivery" => parse_host_event_delivery_input(&bytes).is_ok(),
            "hostDeliveryReply" => parse_host_delivery_reply(&bytes).is_ok(),
            "storageGet" => parse_storage_get_request(&bytes).is_ok(),
            "storageSet" => parse_storage_set_request(&bytes).is_ok(),
            "storageDelete" => parse_storage_delete_request(&bytes).is_ok(),
            "storageTicket" => parse_storage_ticket(&bytes).is_ok(),
            "storageCompletion" => parse_storage_completion_input(&bytes).is_ok(),
            "notificationUpsert" => parse_notification_upsert_request(&bytes).is_ok(),
            "notificationClose" => parse_notification_close_request(&bytes).is_ok(),
            "progressUpsert" => parse_progress_upsert_request(&bytes).is_ok(),
            "statusSet" => parse_status_set_request(&bytes).is_ok(),
            "statusClear" => parse_status_clear_request(&bytes).is_ok(),
            "hostUiSnapshot" => parse_host_ui_snapshot(&bytes).is_ok(),
            "hostUiDismiss" => parse_host_ui_dismiss_request(&bytes).is_ok(),
            "hostUiDismissResult" => parse_host_ui_dismiss_result(&bytes).is_ok(),
            "executeContribution" => parse_execute_contribution_request(&bytes).is_ok(),
            "pluginEvent" => serde_json::from_slice::<PluginEvent>(&bytes)
                .ok()
                .is_some_and(|e| validate_plugin_event(&e).is_ok()),
            kind => panic!("unknown fixture kind {kind}"),
        };
        assert_eq!(valid, f.valid, "{} ({})", f.name, f.kind);
    }
}
#[test]
fn parser_caps_utf8_unknown_fields_and_redacts_values() {
    for bytes in [vec![b' '; MAX_JSON_BYTES + 1], vec![0xff], b"null".to_vec()] {
        assert!(parse_manifest(&bytes).is_err());
        assert!(parse_ui_tree(&bytes).is_err());
        assert!(parse_action_event(&bytes).is_err());
    }
    let mut m = valid_value("manifest");
    m["secret"] = json!("DO-NOT-EXPOSE");
    let error = parse_manifest(&serde_json::to_vec(&m).unwrap()).unwrap_err();
    assert_eq!(error.code, PLUGIN_INVALID_MANIFEST);
    assert!(!error.message.contains("DO-NOT-EXPOSE"));
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"code":error.code,"message":error.message})
    );
}
#[test]
fn manifest_ids_versions_and_contribution_limits() {
    let original = manifest();
    for id in ["", "foo", "org..test", "Org.test", "org_test", "org.-test"] {
        let mut m = original.clone();
        m.id = id.into();
        assert!(m.validate().is_err(), "{id}");
    }
    let mut m = original.clone();
    m.version = "01.0.0".into();
    assert!(m.validate().is_err());
    m = original.clone();
    m.abi.min = m.abi.max + 1;
    assert!(m.validate().is_err());
    m = original.clone();
    m.contributes.commands = vec![
        CommandContribution {
            id: "same".into(),
            title: "title".into(),
            description: None
        };
        2
    ];
    assert!(m.validate().is_err());
    m = original.clone();
    m.contributes.commands = (0..65)
        .map(|i| CommandContribution {
            id: format!("cmd-{i}"),
            title: "title".into(),
            description: None,
        })
        .collect();
    assert!(m.validate().is_err());
    m = original.clone();
    m.services.provides = vec![ServiceDeclaration {
        name: "org.test.service".into(),
        version: 0,
    }];
    assert!(m.validate().is_err());
    assert!(is_local_id("A.a-1_2"));
    assert!(!is_local_id("-a"));
}
#[test]
fn view_locations_and_optional_icons_are_strict() {
    let mut value = valid_value("manifest");
    for (name, location) in [
        ("panel", ViewLocation::Panel),
        ("sidebar", ViewLocation::Sidebar),
    ] {
        value["contributes"]["views"] = json!([{"id":"counter","title":"Counter","location":name}]);
        let parsed = parse_manifest(&serde_json::to_vec(&value).unwrap()).unwrap();
        let view = &parsed.contributes.views[0];
        assert_eq!(view.location, location);
        assert_eq!(view.icon, None);
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            value["contributes"]["views"][0]
        );

        for icon in [
            json!(null),
            json!(false),
            json!(1),
            json!([]),
            json!({}),
            json!(""),
            json!("Calculator"),
            json!("unknown"),
            json!(" calculator"),
        ] {
            value["contributes"]["views"][0]["icon"] = icon.clone();
            let error = parse_manifest(&serde_json::to_vec(&value).unwrap()).unwrap_err();
            assert_eq!(error.code, PLUGIN_INVALID_MANIFEST, "{name}: {icon}");
        }
    }
    value["contributes"]["views"] =
        json!([{"id":"counter","title":"Counter","location":"unknown"}]);
    assert!(parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn view_icon_allowlist_matches_schema_and_roundtrips() {
    let icons: Vec<String> = serde_json::from_str(include_str!(
        "../../../../src/shared/plugin-fixtures/view-icons.json"
    ))
    .unwrap();
    let schema: Value =
        serde_json::from_str(include_str!("../schema/manifest.schema.json")).unwrap();
    let views_schema = &schema["properties"]["contributes"]["properties"]["views"];
    assert_eq!(
        views_schema["items"]["properties"]["icon"]["enum"],
        json!(icons)
    );
    assert_eq!(
        views_schema["items"]["properties"]["icon"]["type"],
        "string"
    );
    assert_eq!(
        views_schema["items"]["properties"]["location"]["enum"],
        json!(["panel", "sidebar"])
    );
    assert_eq!(views_schema["minContains"], 0);
    assert_eq!(views_schema["maxContains"], 2);
    assert_eq!(
        views_schema["contains"]["properties"]["location"]["const"],
        "sidebar"
    );
    assert_eq!(views_schema["maxItems"], 8);
    assert!(!icons.is_empty());
    let mut m = manifest();
    for location in [ViewLocation::Panel, ViewLocation::Sidebar] {
        for icon in &icons {
            m.contributes.views = vec![ViewContribution {
                kind: None,
                entry: None,
                allow: None,
                id: "counter".into(),
                title: "Counter".into(),
                location,
                icon: Some(icon.clone()),
            }];
            m.validate().unwrap();
            assert_eq!(parse_manifest(&serde_json::to_vec(&m).unwrap()).unwrap(), m);
        }
    }
    m.contributes.views[0].icon = Some("unknown".into());
    assert_eq!(m.validate().unwrap_err().code, PLUGIN_INVALID_MANIFEST);
}

#[test]
fn sidebar_view_limit_is_independent_of_total_view_limit() {
    let mut m = manifest();
    for sidebar_count in 0..=3 {
        m.contributes.views = (0..8)
            .map(|i| ViewContribution {
                kind: None,
                entry: None,
                allow: None,
                id: format!("view-{i}"),
                title: "View".into(),
                location: if i < sidebar_count {
                    ViewLocation::Sidebar
                } else {
                    ViewLocation::Panel
                },
                icon: None,
            })
            .collect();
        assert_eq!(m.validate().is_ok(), sidebar_count <= 2);
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&m).unwrap()).is_ok(),
            sidebar_count <= 2
        );
    }
    m.contributes.views.clear();
    assert!(m.validate().is_ok());
    m.contributes.views = (0..9)
        .map(|i| ViewContribution {
            kind: None,
            entry: None,
            allow: None,
            id: format!("view-{i}"),
            title: "View".into(),
            location: ViewLocation::Panel,
            icon: None,
        })
        .collect();
    assert!(m.validate().is_err());
    m.contributes.views.truncate(2);
    m.contributes.views[0].location = ViewLocation::Sidebar;
    m.contributes.views[1].id = m.contributes.views[0].id.clone();
    assert!(
        m.validate().is_err(),
        "view IDs stay unique across locations"
    );
}

#[test]
fn example_counter_manifests_contribute_sidebar_calculator() {
    for bytes in [
        include_bytes!("../../se-plugin-example/src/manifest.json").as_slice(),
        include_bytes!("../../se-plugin-example/src/manifest-v2.json").as_slice(),
    ] {
        let m = parse_manifest(bytes).unwrap();
        assert_eq!(m.contributes.views.len(), 2);
        let view = &m.contributes.views[0];
        assert_eq!(view.id, "example.counter");
        assert_eq!(view.location, ViewLocation::Sidebar);
        assert_eq!(view.icon.as_deref(), Some("calculator"));
        let web = &m.contributes.views[1];
        assert_eq!(web.id, "activity-studio");
        assert_eq!(web.location, ViewLocation::Sidebar);
        assert_eq!(web.kind, Some(ViewKind::Web));
        assert_eq!(web.entry.as_deref(), Some("index.html"));
        assert_eq!(web.icon.as_deref(), Some("chart-bar"));
    }
}

#[test]
fn config_defaults_are_complete_and_strict() {
    let schema = manifest().contributes.configuration;
    let values = complete_config_values(&schema, &ConfigValues::new()).unwrap();
    assert_eq!(values.len(), schema.properties.len());
    let mut unknown = ConfigValues::new();
    unknown.insert("unknown".into(), ConfigValue::Boolean(true));
    assert!(complete_config_values(&schema, &unknown).is_err());
    let mut payload = ConfigPayload {
        schema_version: 1,
        plugin_id: "org.test.plugin".into(),
        revision: 1,
        values,
        changed_keys: vec![],
    };
    assert!(validate_config_payload(&schema, &payload).is_ok());
    payload.values.pop_first();
    assert!(validate_config_payload(&schema, &payload).is_err());
    payload.values = schema.complete(&ConfigValues::new()).unwrap();
    let key = schema.properties.keys().next().unwrap().clone();
    payload.changed_keys = vec![key.clone(), key];
    assert!(validate_config_payload(&schema, &payload).is_err());
    payload.changed_keys.clear();
    payload.revision = MAX_SAFE_INTEGER + 1;
    assert!(validate_config_payload(&schema, &payload).is_err());
}
#[test]
fn configuration_schema_rejects_bad_bounds_enums_null_and_wrong_types() {
    let base = json!({"title":"test","properties":{"key":{"type":"string","title":"Key","description":"","default":"a"}}});
    for patch in [
        json!({"minLength":3,"maxLength":2}),
        json!({"maxLength":8193}),
        json!({"minimum":0}),
        json!({"enum":["b"]}),
        json!({"enum":["a","a"]}),
        json!({"enumLabels":["A"]}),
        json!({"enum":["a"],"enumLabels":[]}),
        json!({"default":false}),
        json!({"enum":null}),
    ] {
        let mut value = base.clone();
        for (k, v) in patch.as_object().unwrap() {
            value["properties"]["key"][k] = v.clone();
        }
        let decoded = serde_json::from_value::<ConfigurationSchema>(value);
        assert!(
            decoded.is_err() || decoded.unwrap().validate().is_err(),
            "{patch}"
        );
    }
    let value = json!({"title":"test","properties":{"key":{"type":"number","title":"Key","description":"","default":0,"minimum":1}}});
    assert!(serde_json::from_value::<ConfigurationSchema>(value)
        .unwrap()
        .validate()
        .is_err());
}
#[test]
fn ui_defaults_whitelist_duplicates_and_unicode() {
    let t = tree(json!({"id":"root","type":"text","props":{"text":"💡".repeat(8192)}}));
    if let UiNode::Text { props, .. } = t.root {
        assert!(props.wrap);
        assert_eq!(props.tone, Tone::Default);
    } else {
        panic!();
    }
    let bad = [
        json!({"id":"x","type":"text","props":{"text":"ok","style":"red"}}),
        json!({"id":"x","type":"text","props":{"text":"ok"},"children":[]}),
        json!({"id":"x","type":"column","props":{}}),
        json!({"id":"x","type":"input","props":{"label":"X","value":"long","actionId":"a","maxLength":2}}),
        json!({"id":"x","type":"select","props":{"label":"X","value":"x","actionId":"a","options":[]}}),
        json!({"id":"x","type":"column","props":{},"children":[{"id":"x","type":"divider","props":{}}]}),
    ];
    for root in bad {
        assert!(parse_ui_tree(
            &serde_json::to_vec(&json!({"schemaVersion":1,"viewId":"main","root":root})).unwrap()
        )
        .is_err());
    }
}
#[test]
fn tree_depth_and_node_count_boundaries() {
    let leaf = json!({"id":"leaf","type":"divider","props":{}});
    let mut root = leaf.clone();
    for i in 1..32 {
        root = json!({"id":format!("node-{i}"),"type":"column","props":{},"children":[root]});
    }
    let mut t = tree(root);
    t.root = UiNode::Column {
        id: "outer".into(),
        props: LayoutProps::default(),
        children: vec![t.root],
    };
    assert!(t.validate().is_err());
    let children: Vec<_> = (0..1023)
        .map(|i| json!({"id":format!("node-{i}"),"type":"divider","props":{}}))
        .collect();
    let mut t = tree(json!({"id":"root","type":"row","props":{},"children":children}));
    if let UiNode::Row { children, .. } = &mut t.root {
        children.push(UiNode::Divider {
            id: "extra".into(),
            props: DividerProps::default(),
        });
    }
    assert!(t.validate().is_err());
}
#[test]
fn actions_are_validated_against_current_tree() {
    let snap =
        snapshot(json!({"id":"control","type":"button","props":{"label":"Go","actionId":"do-it"}}));
    let mut event = action();
    assert!(validate_action_against_snapshot(&event, &snap).is_ok());
    event.action_id = "wrong".into();
    assert!(validate_action_against_snapshot(&event, &snap).is_err());
    event = action();
    event.generation = "22345678-1234-1234-1234-123456789abc".into();
    assert_eq!(
        validate_action_against_snapshot(&event, &snap)
            .unwrap_err()
            .code,
        PLUGIN_STALE_GENERATION
    );
    event = action();
    event.view_revision = 2;
    assert_eq!(
        validate_action_against_snapshot(&event, &snap)
            .unwrap_err()
            .code,
        PLUGIN_STALE_VIEW
    );
    let disabled = snapshot(
        json!({"id":"control","type":"button","props":{"label":"Go","actionId":"do-it","disabled":true}}),
    );
    assert!(validate_action_against_snapshot(&action(), &disabled).is_err());
    let input = snapshot(
        json!({"id":"control","type":"input","props":{"label":"Input","actionId":"do-it","value":"","maxLength":2}}),
    );
    event = action();
    event.kind = ActionKind::Change;
    event.value = ActionValue::String("💡💡".into());
    assert!(validate_action_against_snapshot(&event, &input).is_ok());
    event.value = ActionValue::String("123".into());
    assert!(validate_action_against_snapshot(&event, &input).is_err());
    let toggle = snapshot(
        json!({"id":"control","type":"toggle","props":{"label":"Toggle","actionId":"do-it","checked":false}}),
    );
    event.value = ActionValue::Boolean(true);
    assert!(validate_action_against_snapshot(&event, &toggle).is_ok());
    event.value = ActionValue::String("true".into());
    assert!(validate_action_against_snapshot(&event, &toggle).is_err());
    let select = snapshot(
        json!({"id":"control","type":"select","props":{"label":"Select","actionId":"do-it","value":"a","options":[{"value":"a","label":"A"}]}}),
    );
    event.value = ActionValue::String("a".into());
    assert!(validate_action_against_snapshot(&event, &select).is_ok());
    event.value = ActionValue::String("b".into());
    assert!(validate_action_against_snapshot(&event, &select).is_err());
}
#[test]
fn activation_and_health_require_initial_invariants() {
    let schema = manifest().contributes.configuration;
    let values = schema.complete(&ConfigValues::new()).unwrap();
    let mut payload = ActivationPayload {
        schema_version: 1,
        plugin_id: "org.test.plugin".into(),
        generation: UUID.into(),
        config: ConfigPayload {
            schema_version: 1,
            plugin_id: "org.test.plugin".into(),
            revision: 1,
            values,
            changed_keys: vec![],
        },
    };
    assert!(validate_activation_payload(&schema, &payload).is_ok());
    payload
        .config
        .changed_keys
        .push(schema.properties.keys().next().unwrap().clone());
    assert_eq!(
        validate_activation_payload(&schema, &payload)
            .unwrap_err()
            .code,
        PLUGIN_CONFIG_INVALID
    );
    let mut health = HealthSnapshot {
        schema_version: 1,
        boot_id: UUID.into(),
        pid: 1,
        ready: true,
        safe_mode: false,
        active_resources: 0,
    };
    assert!(validate_health_snapshot(&health).is_ok());
    health.pid = 0;
    assert!(validate_health_snapshot(&health).is_err());
    assert!(validate_install_request(&InstallRequest {
        path: "x".repeat(8192)
    })
    .is_ok());
    assert!(validate_install_request(&InstallRequest {
        path: "x".repeat(8193)
    })
    .is_err());
    assert!(validate_reload_request(&ReloadRequest {
        id: "org.test.plugin".into(),
        path: Some("x".repeat(8193))
    })
    .is_err());
    let info = PluginInfo {
        id: "org.test.plugin".into(),
        manifest: {
            let mut m = manifest();
            m.id = "org.test.plugin".into();
            m
        },
        desired_enabled: false,
        state: PluginState::Disabled,
        generation: None,
        config_revision: 1,
        sha256: "AB".repeat(32),
        last_error: None,
    };
    assert!(validate_plugin_info(&info).is_ok());
}

#[test]
fn rpc_dtos_are_strict_and_preserve_arbitrary_json() {
    for method in [String::new(), "💡".repeat(8192)] {
        let request = ServiceRequest {
            schema_version: 1,
            service: "org.test.service".into(),
            version: 1,
            method,
            input: Value::Null,
        };
        assert!(validate_service_request(&request).is_ok());
    }
    let request = ServiceRequest {
        schema_version: 1,
        service: "org.test.service".into(),
        version: 1,
        method: "x".repeat(8193),
        input: Value::Null,
    };
    assert!(validate_service_request(&request).is_err());
    assert!(
        serde_json::from_value::<CommandRequest>(json!({"schemaVersion":1,"commandId":"go"}))
            .is_err()
    );
    assert!(serde_json::from_value::<ServiceReply>(json!({"schemaVersion":1})).is_err());
    assert!(serde_json::from_value::<ServiceRequest>(
        json!({"schemaVersion":1,"service":"org.test.service","version":1,"method":"read"})
    )
    .is_err());
    assert!(serde_json::from_value::<ListRequest>(json!({"extra":1})).is_err());
    assert!(
        serde_json::from_value::<ReloadRequest>(json!({"id":"org.test.plugin","path":null}))
            .is_err()
    );
    assert!(serde_json::from_value::<PluginConnectionEvent>(json!({"state":"connected"})).is_err());
    let request: CommandRequest = serde_json::from_value(
        json!({"schemaVersion":1,"commandId":"go","args":{"nested":[null,1,"x"]}}),
    )
    .unwrap();
    assert!(validate_command_request(&request).is_ok());
    assert!(parse_service_request(br#"{"schemaVersion":1,"service":"org.test.service","version":1,"method":"read","input":null}"#).is_ok());
    assert_eq!(METHOD_GET_CONFIG, "plugins.config.get");
    assert_eq!(METHOD_SET_CONFIG, "plugins.config.set");
    assert_eq!(METHOD_ACTION, "plugins.view.action");
}
