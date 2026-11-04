//! Real .seplugin/native/service lifecycle. Run via scripts/test-plugins.sh.
#![cfg(unix)]
use base64::Engine;
use se_plugin_host::{HostOptions, PluginHost};
use se_plugin_protocol::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NATIVE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static REQUEST: AtomicU64 = AtomicU64::new(0);
const ID: &str = "org.termul.example";
const OWNER: &str = "web-e2e-owner";
fn archive(variant: &str) -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("SE_PLUGIN_TEST_ARTIFACTS").expect("run scripts/test-plugins.sh"),
    );
    let path = root.join(variant).join("activity-studio.seplugin");
    assert!(path.is_file(), "missing archive {}", path.display());
    path
}
async fn enable(host: &PluginHost, enabled: bool) -> PluginInfo {
    host.set_enabled(SetEnabledRequest {
        id: ID.into(),
        enabled,
    })
    .await
    .expect("set enabled")
}
async fn open(host: &PluginHost, generation: &str) -> WebOpenReply {
    host.web_open(
        OWNER,
        WebOpenRequest {
            id: ID.into(),
            view_id: "activity-studio".into(),
            generation: generation.into(),
        },
    )
    .await
    .expect("open web view")
}
async fn request(
    host: &PluginHost,
    session: &WebOpenReply,
    method: &str,
    params: Value,
) -> Result<Value, PluginError> {
    host.web_request(
        OWNER,
        WebRequest {
            session_id: session.session_id.clone(),
            request_id: format!("r{}", REQUEST.fetch_add(1, Ordering::Relaxed)),
            method: method.into(),
            params,
        },
    )
    .await
}
async fn invoke(host: &PluginHost, session: &WebOpenReply, command: &str) -> Value {
    request(
        host,
        session,
        "invoke",
        json!({"commandId":command,"args":null}),
    )
    .await
    .expect("native web command")
}
fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
async fn gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while pid_alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("managed child must be reaped");
}
async fn ready(host: &PluginHost, session: &WebOpenReply) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let value = invoke(host, session, "service-ping").await;
            if value["service"]["ok"] == true && value["wakeCount"].as_u64().unwrap_or(0) > 0 {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("background TCP ping must wake native instance and publish service result")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_web_bridge_wake_managed_service_upgrade_and_revocation() {
    let _guard = NATIVE_TEST_LOCK.lock().await;
    let profile = tempfile::tempdir().unwrap();
    let host = PluginHost::open(profile.path().into(), HostOptions::default())
        .await
        .unwrap();
    let mut context: HostContextState = serde_json::from_value(json!({
        "activeProject":{"id":"studio-project","name":"Studio project","path":"/synthetic/studio"},
        "projectIds":["studio-project"],"activeTerminalId":"t1","activeSessionId":null,
        "terminals":[{"id":"t1","ptyId":null,"projectId":"studio-project"}],"agentSessions":[],
        "focus":{"textInput":false,"terminal":false},"truncated":false
    }))
    .unwrap();
    host.publish_host_context(
        OWNER,
        HostContextPublishRequest {
            source_id: "11111111-2222-4333-8444-555555555555".into(),
            source_sequence: 1,
            reset: true,
            snapshot: context.clone(),
            events: vec![],
        },
    )
    .await
    .unwrap();
    let installed = host
        .install(InstallRequest {
            path: archive("normal").to_str().unwrap().into(),
        })
        .await
        .expect("validate archive + load ABI manifest");
    let expected: Manifest =
        serde_json::from_str(include_str!("../../se-plugin-example/src/manifest.json")).unwrap();
    assert_eq!(
        installed.manifest, expected,
        "archive manifest must equal real ABI manifest"
    );
    let active = enable(&host, true).await;
    let session = open(&host, active.generation.as_deref().unwrap()).await;
    assert_eq!(session.entry, "index.html");
    assert_eq!(session.shim_version, 1);
    for (path, mime, expected) in [
        (
            "index.html",
            "text/html",
            include_bytes!("../../se-plugin-example/ui/index.html").as_slice(),
        ),
        (
            "app.js",
            "text/javascript",
            include_bytes!("../../se-plugin-example/ui/app.js").as_slice(),
        ),
        (
            "app.css",
            "text/css",
            include_bytes!("../../se-plugin-example/ui/app.css").as_slice(),
        ),
    ] {
        let asset = host
            .web_asset(
                OWNER,
                WebAssetRequest {
                    session_id: session.session_id.clone(),
                    path: format!("ui/{path}"),
                },
            )
            .await
            .unwrap();
        assert_eq!(asset.mime, mime);
        assert_eq!(asset.size, expected.len() as u64);
        assert_eq!(asset.sha256, format!("{:x}", Sha256::digest(expected)));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(asset.data_base64)
                .unwrap(),
            expected
        );
    }
    // The transport, not inventory-backed web_asset, owns the reserved shim.
    assert!(
        include_str!("../../se-plugin-example/ui/index.html").contains("__se__/se-plugin-ui.js")
    );
    assert!(host
        .web_asset(
            OWNER,
            WebAssetRequest {
                session_id: session.session_id.clone(),
                path: "__se__/se-plugin-ui.js".into()
            }
        )
        .await
        .is_err());
    assert_eq!(
        invoke(&host, &session, "increment").await["count"],
        json!(1.0)
    );
    assert_eq!(invoke(&host, &session, "reset").await["count"], json!(0.0));
    let summary = invoke(&host, &session, "summary").await;
    assert_eq!(summary["terminalCount"], 1);
    assert!(!summary["activity"].as_array().unwrap().is_empty());
    let terminal: TerminalContext =
        serde_json::from_value(json!({"id":"t2","ptyId":null,"projectId":"studio-project"}))
            .unwrap();
    context.terminals.push(terminal.clone());
    host.publish_host_context(OWNER, HostContextPublishRequest {
        source_id:"11111111-2222-4333-8444-555555555555".into(),source_sequence:2,reset:false,snapshot:context.clone(),
        events:vec![serde_json::from_value(json!({"topic":"terminal.created","projectId":"studio-project","data":{"terminal":terminal}})).unwrap()]
    }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let probe = invoke(&host, &session, "summary").await;
            if probe["terminalCount"] == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("native dashboard must observe real terminal context events");

    assert!(
        request(&host, &session, "config.set", json!({}))
            .await
            .is_err(),
        "config.write was not allowed"
    );
    assert!(
        request(
            &host,
            &session,
            "view.dispatch",
            json!({"action":"nope","payload":null})
        )
        .await
        .is_err(),
        "view.dispatch was not allowed"
    );
    let response = ready(&host, &session).await;
    let pid = response["service"]["pid"].as_u64().unwrap() as u32;
    assert!(pid_alive(pid));
    let port = response["service"]["port"].as_u64().unwrap() as u16;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .write_all(b"GET /ping HTTP/1.0\r\nHost: localhost\r\nOrigin: null\r\n\r\n")
        .unwrap();
    let mut http = String::new();
    stream.read_to_string(&mut http).unwrap();
    assert!(http.starts_with("HTTP/1.1 200 OK"));
    assert!(http.contains("Access-Control-Allow-Origin: *\r\n"));
    let direct: Value = serde_json::from_str(http.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(direct["ok"], true);
    assert_eq!(direct["pid"], pid);

    let statuses = host.process_list(ID).await.unwrap();
    assert_eq!(
        statuses["activity-service"].state,
        HostProcessState::Running
    );
    assert_eq!(statuses["activity-service"].pid, Some(u64::from(pid)));
    assert!(!host
        .process_logs(ID, "activity-service", 100)
        .await
        .unwrap()
        .lines
        .is_empty());
    let events = host
        .web_read(
            OWNER,
            WebReadRequest {
                session_id: session.session_id.clone(),
                after_sequence: 0,
            },
        )
        .await
        .unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| e.event_type == WebEventType::Message
                && e.payload["wakeCount"].as_u64().unwrap_or(0) > 0
                && e.payload["service"]["ok"] == true),
        "wake callback must post actual TCP result to UI: {events:?}"
    );
    // The wake reply invalidates the web view: it must reach the page, not a declarative render.
    assert!(
        events
            .events
            .iter()
            .any(|e| e.event_type == WebEventType::Invalidate
                && e.payload["code"] == "PLUGIN_UI_INVALIDATED"),
        "web view invalidation must be delivered to the page: {events:?}"
    );
    let listed = host.list().await.unwrap();
    let entry = listed.plugins.iter().find(|p| p.id == ID).unwrap();
    assert!(
        entry.last_error.is_none(),
        "invalidating a web view must not record an error: {:?}",
        entry.last_error
    );
    assert!(
        events
            .events
            .iter()
            .any(|e| e.event_type == WebEventType::ContextEvent
                && e.payload["topic"] == "terminal.created"),
        "web timeline must receive the real context event"
    );
    let note = json!({"text":"persist across the upgrade"});
    let saved = request(
        &host,
        &session,
        "storage.set",
        json!({"requestId":"web-note-set","scope":"profile","key":"activity-studio.notes","value":note,"expectedRevision":0}),
    )
    .await
    .unwrap();
    assert!(
        saved.to_string().contains("revision"),
        "storage CAS reply: {saved}"
    );
    enable(&host, false).await;
    gone(pid).await;
    assert!(host
        .web_read(
            OWNER,
            WebReadRequest {
                session_id: session.session_id.clone(),
                after_sequence: 0
            }
        )
        .await
        .is_err());
    assert!(request(
        &host,
        &session,
        "invoke",
        json!({"commandId":"summary","args":null})
    )
    .await
    .is_err());
    let active = enable(&host, true).await;
    let second = open(&host, active.generation.as_deref().unwrap()).await;
    let before_upgrade = ready(&host, &second).await;
    let second_pid = before_upgrade["service"]["pid"].as_u64().unwrap() as u32;
    assert_ne!(pid, second_pid);
    let upgraded = host
        .reload(ReloadRequest {
            id: ID.into(),
            path: Some(archive("upgrade").to_str().unwrap().into()),
        })
        .await
        .unwrap();
    assert_eq!(upgraded.manifest.version, "0.1.1");
    gone(second_pid).await;
    assert_ne!(upgraded.generation, active.generation);
    assert!(host
        .web_read(
            OWNER,
            WebReadRequest {
                session_id: second.session_id.clone(),
                after_sequence: 0
            }
        )
        .await
        .is_err());
    let third = open(&host, upgraded.generation.as_deref().unwrap()).await;
    let after_upgrade = ready(&host, &third).await;
    let third_pid = after_upgrade["service"]["pid"].as_u64().unwrap() as u32;
    assert_ne!(third_pid, second_pid);
    let stored = request(
        &host,
        &third,
        "storage.get",
        json!({"requestId":"web-note-get","scope":"profile","key":"activity-studio.notes"}),
    )
    .await
    .unwrap();
    assert_eq!(
        stored["value"], note,
        "upgrade must retain exact profile CAS value"
    );
    enable(&host, false).await;
    gone(third_pid).await;
    host.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_archive_hash_is_rejected_before_native_load() {
    let _guard = NATIVE_TEST_LOCK.lock().await;
    let profile = tempfile::tempdir().unwrap();
    let host = PluginHost::open(profile.path().into(), HostOptions::default())
        .await
        .unwrap();
    let input = fs::File::open(archive("normal")).unwrap();
    let mut source = zip::ZipArchive::new(input).unwrap();
    let bad = profile.path().join("bad.seplugin");
    let mut out = zip::ZipWriter::new(fs::File::create(&bad).unwrap());
    for i in 0..source.len() {
        let mut entry = source.by_index(i).unwrap();
        let name = entry.name().to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == "ui/app.js" {
            bytes[0] ^= 1;
        }
        out.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o755),
        )
        .unwrap();
        out.write_all(&bytes).unwrap();
    }
    out.finish().unwrap();
    let before = host.diagnostics().loaded_libraries;
    assert!(host
        .install(InstallRequest {
            path: bad.to_str().unwrap().into()
        })
        .await
        .is_err());
    assert_eq!(
        host.diagnostics().loaded_libraries,
        before,
        "corrupt inventory must be rejected before dlopen"
    );
    assert!(host.list().await.unwrap().plugins.is_empty());
    host.shutdown().await.unwrap();
}
