//! Host HTTP regressions: no SSH sockets, desktop credentials, or user stores.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension, Router,
};
use se_manager_lib::{
    core::{AcpWebHostHandle, TerminalServiceHandle},
    mcp_core::{
        host_adapters::{HostCredentials, ServerProfiles},
        host_policy::{HostAccessPolicy, HostPolicyStore},
        host_service::HostCapabilityService,
    },
    ssh::{automation::*, profile_manager::SSHProfile},
    web::{
        self,
        auth::{IngressProvenance, RemoteAccessAuthority},
        store::WebStore,
        EventSink, ProjectRegistry, WsRelaySink,
    },
    AcpManager, CwdTracker, ExitCodeTracker, GitTracker, PtyManager, TerminalEventHub,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "host-web-regression-operator-token-0123456789";
struct FakeBackend;
impl AutomationBackend for FakeBackend {
    fn execute(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        panic!("these HTTP tests must not execute SSH commands")
    }
    fn files(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: &str,
        _: &Value,
        _: &ProfileGrant,
    ) -> AutomationResult<Value> {
        panic!("these HTTP tests must not open SFTP")
    }
    fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
        Ok(json!({"connected":true,"backend":"fake"}))
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    service: Arc<HostCapabilityService>,
    store: Arc<WebStore>,
    registry: Arc<ProjectRegistry>,
    authority: Arc<RemoteAccessAuthority>,
}
impl Fixture {
    fn new(provenance: IngressProvenance) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("operator-token");
        std::fs::write(&token_file, TOKEN).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let authority = Arc::new(RemoteAccessAuthority::from_token_file(&token_file).unwrap());
        authority.set_ingress_provenance(provenance);
        let store = Arc::new(WebStore::open(dir.path().join("store.json")));
        let profiles = Arc::new(ServerProfiles(store.clone()));
        let credentials = Arc::new(HostCredentials::new(
            None,
            "test-service".into(),
            "legacy-test-service".into(),
        ));
        let ssh = Arc::new(SshAutomationService::new(
            profiles.clone(),
            credentials,
            Arc::new(FakeBackend),
        ));
        let registry = Arc::new(ProjectRegistry::default());
        let policy = Arc::new(HostPolicyStore::open(dir.path().join("policy.json")).unwrap());
        let service = Arc::new(HostCapabilityService::new(
            policy,
            profiles,
            registry.clone(),
            ssh,
            "shared-live",
        ));
        Self {
            _dir: dir,
            service,
            store,
            registry,
            authority,
        }
    }
    fn app(&self, inject_service: bool) -> Router {
        let relay = Arc::new(WsRelaySink::new());
        let sink: Arc<dyn EventSink> = relay.clone();
        let acp = Arc::new(AcpManager::new(vec![sink]));
        let events = TerminalEventHub::standalone();
        let cwd = Arc::new(CwdTracker::new(events.clone()));
        let git = Arc::new(GitTracker::new(events.clone()));
        let exit = Arc::new(ExitCodeTracker::new(events.clone()));
        let pty = Arc::new(PtyManager::new(
            events.clone(),
            cwd.clone(),
            git.clone(),
            exit.clone(),
            se_pty::TerminalProgram {
                name: "Se".into(),
                version: "test".into(),
            },
        ));
        let app = web::router::router(
            AcpWebHostHandle::in_process(acp, relay.clone()),
            pty.clone(),
            TerminalServiceHandle::in_process(pty),
            events,
            cwd,
            git,
            exit,
            relay,
            self.registry.clone(),
            None,
            None,
            self._dir.path().into(),
            web::HistoryMode::Server,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(self.store.clone()),
            self.authority.clone(),
        );
        if inject_service {
            app.layer(Extension(self.service.clone()))
        } else {
            app
        }
    }
    fn profile(&self, name: &str) -> Value {
        json!({"id":"p","name":name,"host":"fake.invalid","port":22,"username":"tester","authMethod":"agent","privateKeyPath":null,"password":"NEVER-RETURN-SECRET","passphrase":null,"jumpHostId":null,"portForwards":[],"tags":[],"lastConnected":null,"importedFrom":null,"hasStoredPassword":false,"hasStoredPassphrase":false})
    }
}

fn request(method: &str, path: &str, authenticated: bool, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if authenticated {
        builder = builder.header("authorization", format!("Bearer {TOKEN}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
async fn send(app: Router, method: &str, path: &str, body: Value) -> Value {
    let response = app
        .oneshot(request(method, path, true, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
fn endpoints() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("GET", "/mcp/host/access", Value::Null),
        (
            "PUT",
            "/mcp/host/access",
            json!({"expectedRevision":0,"policy":HostAccessPolicy::default()}),
        ),
        (
            "POST",
            "/mcp/host/call",
            json!({"provider":"ssh","tool":"profile_list","arguments":{}}),
        ),
    ]
}

#[tokio::test]
async fn host_endpoints_require_authentication() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    for (method, path, body) in endpoints() {
        let response = fixture
            .app(true)
            .oneshot(request(method, path, false, body))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn public_tunnel_cannot_manage_or_call_even_with_forged_headers() {
    let fixture = Fixture::new(IngressProvenance::PublicTunnel);
    for (method, path, body) in endpoints() {
        let mut request = request(method, path, true, body);
        for (key, value) in [
            ("x-forwarded-for", "127.0.0.1"),
            ("x-real-ip", "127.0.0.1"),
            ("x-ingress-provenance", "LocalOperator"),
        ] {
            request.headers_mut().insert(
                axum::http::HeaderName::from_static(key),
                value.parse().unwrap(),
            );
        }
        let response = fixture.app(true).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert_eq!(
        fixture.service.access_view().unwrap()["policy"]["revision"],
        0
    );
}

#[tokio::test]
async fn absent_service_returns_ipc_unavailable_not_missing_extension_rejection() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    for (method, path, body) in endpoints() {
        let result = send(fixture.app(false), method, path, body).await;
        assert_eq!(result["success"], false);
        assert_eq!(result["code"], "UNAVAILABLE");
    }
}

#[tokio::test]
async fn injected_service_shares_policy_and_live_web_store_and_uses_fake_ssh() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    fixture
        .store
        .write("ssh/profiles", json!([fixture.profile("first")]), None)
        .unwrap();
    let initial = send(fixture.app(true), "GET", "/mcp/host/access", Value::Null).await;
    assert_eq!(initial["success"], true);
    assert_eq!(initial["data"]["profiles"][0]["name"], "first");
    assert!(!initial.to_string().contains("NEVER-RETURN-SECRET"));
    let mut policy: HostAccessPolicy =
        serde_json::from_value(initial["data"]["policy"].clone()).unwrap();
    policy.enabled = true;
    policy.ssh.profiles.insert(
        "p".into(),
        ProfileGrant {
            operations: ["profile_list".to_string(), "test_connection".to_string()]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
    policy.profile_fingerprints.insert(
        "p".into(),
        initial["data"]["profiles"][0]["fingerprint"]
            .as_str()
            .unwrap()
            .into(),
    );
    let result = send(
        fixture.app(true),
        "PUT",
        "/mcp/host/access",
        json!({"expectedRevision":0,"policy":policy}),
    )
    .await;
    assert_eq!(result["success"], true, "{result}");
    // The already-held desktop/service handle observes the web mutation immediately.
    assert_eq!(
        fixture.service.access_view().unwrap()["policy"]["revision"],
        1
    );
    let stale = send(
        fixture.app(true),
        "PUT",
        "/mcp/host/access",
        json!({"expectedRevision":0,"policy":policy}),
    )
    .await;
    assert_eq!(stale["code"], "REVISION_CONFLICT");
    // A web persistence mutation is visible without reopening either store or service.
    fixture
        .store
        .write("ssh/profiles", json!([fixture.profile("updated")]), None)
        .unwrap();
    let updated = send(fixture.app(true), "GET", "/mcp/host/access", Value::Null).await;
    assert_eq!(updated["data"]["profiles"][0]["name"], "updated");
    let call = send(
        fixture.app(true),
        "POST",
        "/mcp/host/call",
        json!({"provider":"ssh","tool":"test_connection","arguments":{"profileId":"p"}}),
    )
    .await;
    assert_eq!(call["success"], true, "{call}");
    assert_eq!(call["data"]["backend"], "fake");
}

#[tokio::test]
async fn shared_live_serve_router_uses_the_existing_host_service_instance() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let relay = Arc::new(WsRelaySink::new());
    let sink: Arc<dyn EventSink> = relay.clone();
    let acp = Arc::new(AcpManager::new(vec![sink]));
    let events = TerminalEventHub::standalone();
    let pty = Arc::new(PtyManager::new(
        events.clone(),
        Arc::new(CwdTracker::new(events.clone())),
        Arc::new(GitTracker::new(events.clone())),
        Arc::new(ExitCodeTracker::new(events)),
        se_pty::TerminalProgram {
            name: "Se".into(),
            version: "test".into(),
        },
    ));
    let root = fixture._dir.path().canonicalize().unwrap();
    let cfg = web::config::ServerConfig {
        host: "127.0.0.1".into(),
        port: 0,
        event_log_capacity: 32,
        permission_timeout_secs: 60,
        permission_reconnect_grace_secs: 15,
        project_root: root.clone(),
        projects_file: None,
        sessions_dir: None,
        conversation_workspace_root: root.join("conversation"),
        workspace_manifests_dir: None,
        acp_catalog_dir: None,
        store_file: None,
        remote_access_token_file: None,
        allowed_origins: Vec::new(),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (address, handle) = web::serve_router(
        AcpWebHostHandle::in_process(acp, relay.clone()),
        pty.clone(),
        TerminalServiceHandle::in_process(pty.clone()),
        pty.terminal_events(),
        pty.cwd_tracker(),
        pty.git_tracker(),
        pty.exit_code_tracker(),
        relay,
        fixture.registry.clone(),
        None,
        None,
        cfg,
        async move {
            let _ = shutdown_rx.await;
        },
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        fixture.authority.clone(),
        web::WebHostServices {
            service: Some(fixture.service.clone()),
            store: Some(fixture.store.clone()),
        },
    )
    .await
    .unwrap();
    let client = reqwest::Client::new();
    let mut policy = HostAccessPolicy::default();
    policy.enabled = true;
    let response: Value = client
        .put(format!("http://{address}/mcp/host/access"))
        .bearer_auth(TOKEN)
        .json(&json!({"expectedRevision":0,"policy":policy}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["success"], true, "{response}");
    let view = fixture.service.access_view().unwrap();
    assert_eq!(view["policy"]["revision"], 1);
    assert_eq!(view["policy"]["enabled"], true);
    // A mutation through the web-owned store is immediately visible over the
    // host endpoint; neither startup nor the adapter may reopen the store.
    fixture
        .store
        .write("ssh/profiles", json!([fixture.profile("from-web")]), None)
        .unwrap();
    let response: Value = client
        .get(format!("http://{address}/mcp/host/access"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["data"]["profiles"][0]["name"], "from-web");
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap();
}
