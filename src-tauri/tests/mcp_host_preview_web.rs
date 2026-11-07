//! Preview runs the real authenticated router but never resolves secrets or opens SSH.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Extension, Router,
};
use se_manager_lib::{
    core::{AcpWebHostHandle, TerminalServiceHandle},
    mcp_core::{
        host_policy::{profile_fingerprint, HostAccessPolicy, HostPolicyStore},
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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;
const TOKEN: &str = "preview-fixture-operator-token-not-a-real-secret-0123456789";
const PATH: &str = "/mcp/host/access/preview";
struct Profiles(SSHProfile);
impl ProfileSource for Profiles {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        Ok(vec![self.0.clone()])
    }
}
struct Secrets(AtomicUsize);
impl CredentialResolver for Secrets {
    fn resolve(&self, _: &SSHProfile) -> AutomationResult<Credentials> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("preview must not resolve credentials")
    }
}
struct NoNetwork;
impl AutomationBackend for NoNetwork {
    fn execute(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        panic!("preview must not execute")
    }
    fn files(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: &str,
        _: &Value,
        _: &ProfileGrant,
    ) -> AutomationResult<Value> {
        panic!("preview must not open SFTP")
    }
    fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
        panic!("preview must not connect")
    }
}
fn profile() -> SSHProfile {
    serde_json::from_value(json!({"id":"p","name":"Preview fixture","host":"fixture.invalid","port":22,"username":"tester","authMethod":"agent","tags":[],"hasStoredPassword":false,"hasStoredPassphrase":false,"portForwards":[],"privateKeyPath":null,"jumpHostId":null,"lastConnected":null,"importedFrom":null})).unwrap()
}
struct Fixture {
    dir: tempfile::TempDir,
    service: Arc<HostCapabilityService>,
    secrets: Arc<Secrets>,
    registry: Arc<ProjectRegistry>,
    authority: Arc<RemoteAccessAuthority>,
}
impl Fixture {
    fn new(provenance: IngressProvenance) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, TOKEN).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let authority = Arc::new(RemoteAccessAuthority::from_token_file(&token).unwrap());
        authority.set_ingress_provenance(provenance);
        let source = Arc::new(Profiles(profile()));
        let secrets = Arc::new(Secrets(AtomicUsize::new(0)));
        let ssh = Arc::new(SshAutomationService::new(
            source.clone(),
            secrets.clone(),
            Arc::new(NoNetwork),
        ));
        let registry = Arc::new(ProjectRegistry::new());
        let policy = Arc::new(HostPolicyStore::open(dir.path().join("policy.json")).unwrap());
        let service = Arc::new(HostCapabilityService::new(
            policy,
            source,
            registry.clone(),
            ssh,
            "fixture",
        ));
        Self {
            dir,
            service,
            secrets,
            registry,
            authority,
        }
    }
    fn app(&self, inject: bool) -> Router {
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
            self.dir.path().into(),
            web::HistoryMode::Server,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(WebStore::open(self.dir.path().join("web.json")))),
            self.authority.clone(),
        );
        if inject {
            app.layer(Extension(self.service.clone()))
        } else {
            app
        }
    }
    fn policy(&self) -> HostAccessPolicy {
        serde_json::from_value(json!({
            "schemaVersion":2,"revision":0,"enabled":true,
            "workspace":{"enabled":false,"projectIds":[],"includePaths":false},
            "ssh":{"commandDeny":[{"id":"global-block","kind":"contains","pattern":"SENSITIVE-PATTERN"}],
                "profiles":{"p":{"operations":["execute","job_start"],"remoteRoots":[],"localRoots":[],"posixJobs":true,
                    "commandDeny":[{"id":"profile-block","kind":"exact","pattern":"local-only"}]}}},
            "profileFingerprints":{"p":profile_fingerprint(&profile())}
        })).unwrap()
    }
}
fn request(authenticated: bool, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("content-type", "application/json");
    if authenticated {
        builder = builder.header("authorization", format!("Bearer {TOKEN}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
async fn send(app: Router, body: Value) -> Value {
    let response = app.oneshot(request(true, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn preview_requires_authenticated_local_operator_and_cannot_be_header_spoofed() {
    let body = json!({"profileId":"p","operation":"execute","command":"fixture"});
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    assert_eq!(
        fixture
            .app(true)
            .oneshot(request(false, body.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let tunnel = Fixture::new(IngressProvenance::PublicTunnel);
    let mut forged = request(true, body);
    forged
        .headers_mut()
        .insert("x-se-ingress-provenance", "LocalOperator".parse().unwrap());
    forged
        .headers_mut()
        .insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
    assert_eq!(
        tunnel.app(true).oneshot(forged).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert_eq!(tunnel.secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_host_service_returns_an_explicit_unavailable_envelope() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let result = send(
        fixture.app(false),
        json!({"profileId":"p","operation":"execute","command":"fixture"}),
    )
    .await;
    assert_eq!(result["success"], false);
    assert_eq!(result["code"], "UNAVAILABLE");
}

#[tokio::test]
async fn persisted_preview_uses_both_layers_without_credentials_or_revealing_patterns() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    fixture.service.put_access(0, fixture.policy()).unwrap();
    let before = std::fs::read(fixture.dir.path().join("policy.json")).unwrap();
    for operation in ["execute", "job_start"] {
        let global = send(fixture.app(true), json!({"profileId":"p", "operation":operation, "command":"echo SENSITIVE-PATTERN COMMAND-SENTINEL"})).await;
        assert_eq!(global["success"], true, "{global}");
        assert_eq!(global["data"]["outcome"], "matched");
        assert_eq!(
            global["data"]["matches"],
            json!([{"id":"global-block","source":"global"}])
        );
        assert_eq!(global["data"]["draft"], false);
        assert_eq!(global["data"]["policyRevision"], 1);
        let encoded = global.to_string();
        assert!(!encoded.contains("SENSITIVE-PATTERN"));
        assert!(!encoded.contains("COMMAND-SENTINEL"));
        let local = send(
            fixture.app(true),
            json!({"profileId":"p", "operation":operation, "command":"local-only"}),
        )
        .await;
        assert_eq!(
            local["data"]["matches"],
            json!([{"id":"profile-block","source":"profile"}])
        );
        let clean = send(
            fixture.app(true),
            json!({"profileId":"p", "operation":operation, "command":"unmatched-fixture"}),
        )
        .await;
        assert_eq!(clean["data"]["outcome"], "not_matched");
        assert_eq!(clean["data"]["matches"], json!([]));
    }
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(fixture.dir.path().join("policy.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn draft_preview_cannot_enable_or_save_host_access() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let result = send(fixture.app(true), json!({"profileId":"p", "operation":"execute", "command":"SENSITIVE-PATTERN", "policy":fixture.policy()})).await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["data"]["outcome"], "matched");
    assert_eq!(result["data"]["draft"], true);
    assert_eq!(result["data"]["policyRevision"], 0);
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert!(!fixture.service.policy.snapshot().enabled);
    assert!(!fixture.dir.path().join("policy.json").exists());
}

#[tokio::test]
async fn invalid_draft_rules_fail_without_echoing_command_or_changing_stored_policy() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let mut policy = serde_json::to_value(fixture.policy()).unwrap();
    policy["ssh"]["commandDeny"] =
        json!([{"id":"invalid-rule","kind":"regex","pattern":"[SECRET-PATTERN"}]);
    let result = send(fixture.app(true), json!({"profileId":"p", "operation":"execute", "command":"COMMAND-SENTINEL", "policy":policy})).await;
    assert_eq!(result["success"], false);
    assert_eq!(result["code"], "POLICY_INVALID");
    assert!(!result.to_string().contains("SECRET-PATTERN"));
    assert!(!result.to_string().contains("COMMAND-SENTINEL"));
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert!(!fixture.dir.path().join("policy.json").exists());
}

#[tokio::test]
async fn preview_rejects_oversized_bodies_before_evaluation() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let body =
        json!({"profileId":"p", "operation":"execute", "command":"x".repeat(1024 * 1024 + 1)});
    let response = fixture
        .app(true)
        .oneshot(request(true, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn no_match_does_not_claim_execution_is_authorized() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let result = send(
        fixture.app(true),
        json!({"profileId":"p", "operation":"execute", "command":"unmatched-fixture"}),
    )
    .await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["data"]["outcome"], "not_matched");
    assert!(result["data"]["blockedBy"].as_str().is_some());
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert!(!fixture.dir.path().join("policy.json").exists());
}

#[test]
fn terminal_and_shared_ssh_transports_do_not_import_mcp_command_policy() {
    // This wiring invariant accompanies the behavioral MCP rejection tests.
    // No user PTY or real remote host is needed to assert the separation.
    for (name, source) in [
        ("ssh connection", include_str!("../src/ssh/connection.rs")),
        (
            "shared backend",
            include_str!("../src/ssh/automation_backend.rs"),
        ),
        ("interactive SFTP", include_str!("../src/ssh/sftp.rs")),
        ("port forwards", include_str!("../src/ssh/port_forward.rs")),
    ] {
        for forbidden in ["command_policy::", "HostPolicyStore", ".authorize_command("] {
            assert!(
                !source.contains(forbidden),
                "{name} must not enforce the MCP-only command blacklist"
            );
        }
    }
}

#[tokio::test]
async fn malformed_preview_schema_has_a_fixed_non_echoing_error_envelope() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let mut bad_policy = serde_json::to_value(fixture.policy()).unwrap();
    bad_policy["ssh"]["commandDeny"][0]["kind"] = json!("SECRET-VARIANT-SENTINEL");
    for (expected_status, body) in [
        // Operation is validated by the service after decoding, so it retains
        // the existing HTTP-200/failed-IPC business-error contract.
        (
            StatusCode::OK,
            json!({"profileId":"p","operation":"SECRET-OP-SENTINEL","command":"COMMAND-SENTINEL"}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"profileId":"p","operation":"execute","command":"COMMAND-SENTINEL","SECRET-FIELD-SENTINEL":true}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"profileId":"p","operation":"execute","command":"COMMAND-SENTINEL","policy":bad_policy}),
        ),
    ] {
        let response = fixture
            .app(true)
            .oneshot(request(true, body))
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["code"], "INVALID_ARGUMENTS");
        assert!(!String::from_utf8_lossy(&bytes).contains("SENTINEL"));
    }
    let malformed = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from("{\"SECRET-SYNTAX-SENTINEL\":"))
        .unwrap();
    let response = fixture.app(true).oneshot(malformed).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["code"], "INVALID_ARGUMENTS");
    assert!(!String::from_utf8_lossy(&bytes).contains("SENTINEL"));
    assert_eq!(fixture.secrets.0.load(Ordering::SeqCst), 0);
    assert!(!fixture.dir.path().join("policy.json").exists());
}

#[tokio::test]
async fn preview_authorization_precedes_malformed_body_details() {
    let fixture = Fixture::new(IngressProvenance::LocalOperator);
    let malformed = json!({"operation":"SECRET-INVALID-OP"});
    let response = fixture
        .app(true)
        .oneshot(request(false, malformed.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let tunnel = Fixture::new(IngressProvenance::PublicTunnel);
    let response = tunnel
        .app(true)
        .oneshot(request(true, malformed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
