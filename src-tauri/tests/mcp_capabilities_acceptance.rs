//! End-to-end in-process MCP host capability acceptance. All credentials and
//! target I/O are injected fixtures: this suite never connects to user SSH.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use se_manager_lib::{
    mcp_core::{
        builtins::BuiltInCapability,
        host_capabilities::{HostBackedCapability, HostCapabilityBackend},
        host_policy::{profile_fingerprint, HostAccessPolicy, HostPolicyStore},
        host_service::HostCapabilityService,
    },
    ssh::{
        automation::{
            AutomationBackend, AutomationResult, CredentialResolver, Credentials, ExecRequest,
            ExecResult, ProfileGrant, ProfileSource, SshAutomationService, SshPolicy,
        },
        profile_manager::SSHProfile,
    },
    web::project_registry::{ProjectRegistry, ProjectSummary},
};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

struct Profiles(SSHProfile);
impl ProfileSource for Profiles {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        Ok(vec![self.0.clone()])
    }
}
struct Secrets;
impl CredentialResolver for Secrets {
    fn resolve(&self, _: &SSHProfile) -> AutomationResult<Credentials> {
        Ok(Credentials {
            secret: Some("secret-sentinel-not-for-mcp".into()),
        })
    }
}
struct Backend(AtomicUsize);
impl AutomationBackend for Backend {
    fn execute(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ExecResult {
            stdout: "output".into(),
            stderr: "diagnostic".into(),
            exit_code: Some(7),
            duration_ms: 1,
            truncated: false,
            timed_out: false,
            cancel_confirmed: false,
            remote_termination_confirmed: true,
        })
    }
    fn files(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        operation: &str,
        _: &Value,
        _: &ProfileGrant,
    ) -> AutomationResult<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"operation":operation}))
    }
    fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"connected":true}))
    }
}

fn profile() -> SSHProfile {
    serde_json::from_value(json!({
        "id":"fixture","name":"Fixture","host":"fixture.invalid","port":22,
        "username":"tester","authMethod":"password","tags":[],
        "hasStoredPassword":true,"hasStoredPassphrase":false,
        "portForwards":[],"privateKeyPath":null,"jumpHostId":null,
        "lastConnected":null,"importedFrom":null
    }))
    .unwrap()
}

fn host(path: &std::path::Path) -> (HostCapabilityService, Arc<Backend>) {
    let source = Arc::new(Profiles(profile()));
    let backend = Arc::new(Backend(AtomicUsize::new(0)));
    let ssh = Arc::new(SshAutomationService::new(
        source.clone(),
        Arc::new(Secrets),
        backend.clone(),
    ));
    let registry = Arc::new(ProjectRegistry::new());
    let project: ProjectSummary = serde_json::from_value(json!({"id":"no-memory","name":"New project","color":"blue","path":"/private/project-path","isArchived":false,"isDefault":false})).unwrap();
    registry.set(vec![project], None);
    let host = HostCapabilityService::new(
        Arc::new(HostPolicyStore::open(path.into()).unwrap()),
        source,
        registry,
        ssh,
        "test",
    );
    (host, backend)
}

fn policy() -> HostAccessPolicy {
    let mut policy = HostAccessPolicy::default();
    policy.enabled = true;
    policy.workspace.enabled = true;
    policy.workspace.project_ids = None;
    policy.workspace.include_paths = false;
    let mut grants = std::collections::HashMap::new();
    grants.insert(
        "fixture".into(),
        ProfileGrant {
            profile_fingerprint: None,
            operations: ["profile_list", "profile_get", "execute", "test_connection"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            remote_roots: vec![],
            local_roots: vec![],
            posix_jobs: false,
        },
    );
    policy.ssh = SshPolicy { profiles: grants };
    policy.profile_fingerprints =
        BTreeMap::from([("fixture".into(), profile_fingerprint(&profile()))]);
    policy
}

#[tokio::test]
async fn default_denial_happens_before_any_target_io_and_client_cannot_self_authorize() {
    let dir = tempfile::tempdir().unwrap();
    let (host, backend) = host(&dir.path().join("policy.json"));
    let error = host
        .call(
            "ssh",
            "execute",
            json!({"profileId":"fixture","command":"true"})
                .as_object()
                .unwrap()
                .clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "PERMISSION_DENIED");
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    host.put_access(0, policy()).unwrap();
    let error = host
        .call(
            "ssh",
            "execute",
            json!({"callerId":"operator","profileId":"fixture","command":"true"})
                .as_object()
                .unwrap()
                .clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "INVALID_ARGUMENTS");
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn canonical_providers_invoke_the_host_and_preserve_execution_result() {
    let dir = tempfile::tempdir().unwrap();
    let (host, backend) = host(&dir.path().join("policy.json"));
    host.put_access(0, policy()).unwrap();
    let host = Arc::new(host);
    let workspace = HostBackedCapability::workspace(host.clone());
    let ssh = HostBackedCapability::ssh(host.clone());
    assert_eq!(workspace.tools().len(), 3);
    assert_eq!(ssh.tools().len(), 13);
    let projects = workspace
        .call_tool("project_list", Some(Map::new()), CancellationToken::new())
        .await
        .unwrap();
    let encoded = serde_json::to_string(&projects).unwrap();
    assert!(encoded.contains("no-memory"));
    assert!(!encoded.contains("/private/project-path"));
    let metadata = ssh
        .call_tool(
            "profile_get",
            Some(json!({"profileId":"fixture"}).as_object().unwrap().clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!serde_json::to_string(&metadata)
        .unwrap()
        .contains("secret-sentinel"));
    let result = host
        .call(
            "ssh",
            "execute",
            json!({"profileId":"fixture","command":"fixture-only"})
                .as_object()
                .unwrap()
                .clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["exitCode"], 7);
    assert_eq!(result["stdout"], "output");
    assert_eq!(result["stderr"], "diagnostic");
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn policy_cas_persists_and_revocation_denies_subsequent_calls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    let (host, backend) = host(&path);
    let granted = host.put_access(0, policy()).unwrap();
    assert_eq!(granted.revision, 1);
    assert_eq!(
        host.put_access(0, policy()).unwrap_err().code,
        "REVISION_CONFLICT"
    );
    let reopened = HostPolicyStore::open(path).unwrap();
    assert_eq!(reopened.snapshot().revision, 1);
    let mut revoke = granted;
    revoke.enabled = false;
    host.put_access(1, revoke).unwrap();
    let error = host
        .call(
            "ssh",
            "test_connection",
            json!({"profileId":"fixture"}).as_object().unwrap().clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "PERMISSION_DENIED");
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn http_mcp_routes_through_private_bridge_and_rechecks_revocation() {
    use rmcp::{
        model::{CallToolRequestParams, ClientConfig},
        service::serve_client,
        transport::{
            streamable_http_client::StreamableHttpClientTransportConfig,
            StreamableHttpClientTransport,
        },
    };
    use se_manager_lib::mcp_core::{
        host_bridge::{HostBridge, RemoteHostBackend},
        AllowAllTools, AuthBootstrap, BuiltInRegistry, McpCore, McpCoreConfig, McpHttpGateway,
        McpHttpGatewayConfig,
    };
    let dir = tempfile::tempdir().unwrap();
    let (host, backend) = host(&dir.path().join("policy.json"));
    let host = Arc::new(host);
    let bridge = HostBridge::bind(host.clone()).await.unwrap();
    let descriptor = dir.path().join("host.json");
    bridge.endpoint.publish(&descriptor).unwrap();
    let remote = Arc::new(RemoteHostBackend::new(descriptor).unwrap());
    let mut builtins = BuiltInRegistry::empty();
    builtins.register(Arc::new(HostBackedCapability::workspace(remote.clone())));
    builtins.register(Arc::new(HostBackedCapability::ssh(remote)));
    let gateway = McpHttpGateway::bind(
        Arc::new(McpCore::new_with_builtins(
            McpCoreConfig::default(),
            Arc::new(AllowAllTools),
            builtins,
        )),
        McpHttpGatewayConfig {
            bind_address: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            path: "/mcp".into(),
            generation: 1,
            auth: AuthBootstrap::new(1, "synthetic-public-token").unwrap(),
            request_body_limit: 64 * 1024,
            control: None,
        },
    )
    .await
    .unwrap();
    let config = StreamableHttpClientTransportConfig::with_uri(format!(
        "http://127.0.0.1:{}/mcp/direct",
        gateway.endpoint().port,
    ))
    .custom_headers(std::collections::HashMap::from([(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_static("Bearer synthetic-public-token"),
    )]));
    let client = serve_client(
        ClientConfig::default(),
        StreamableHttpClientTransport::from_config(config),
    )
    .await
    .unwrap();
    assert_eq!(client.list_tools(None).await.unwrap().tools.len(), 16);
    let execute = || {
        CallToolRequestParams::new("ssh_execute").with_arguments(
            json!({"profileId":"fixture", "command":"fixture-only"})
                .as_object()
                .unwrap()
                .clone(),
        )
    };
    let denied = client.call_tool(execute()).await.unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    let mut granted = host.put_access(0, policy()).unwrap();
    let result = client.call_tool(execute()).await.unwrap();
    assert_ne!(result.is_error, Some(true));
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(encoded.contains("output"));
    assert!(!encoded.contains("secret-sentinel"));
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    granted.enabled = false;
    host.put_access(1, granted).unwrap();
    assert_eq!(
        client.call_tool(execute()).await.unwrap().is_error,
        Some(true)
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    client.cancel().await.unwrap();
    gateway.shutdown().await;
}

#[test]
fn global_disable_does_not_require_stale_profiles_to_be_reauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let store = HostPolicyStore::open(dir.path().join("policy.json")).unwrap();
    let granted = store.put(0, policy(), &[profile()]).unwrap();
    let mut disabled = granted.clone();
    disabled.enabled = false;
    // Deleted profiles must never prevent the host operator's kill switch.
    let saved = store.put(1, disabled, &[]).unwrap();
    assert!(!saved.enabled);
    assert_eq!(
        store.put(2, granted, &[]).unwrap_err().code,
        "PROFILE_NOT_FOUND"
    );
    assert!(!store.snapshot().enabled);
}

#[test]
fn host_grants_use_authoritative_identity_and_revoke_changed_targets() {
    use se_manager_lib::{mcp_core::host_policy::HostGrants, ssh::automation::GrantProvider};
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(HostPolicyStore::open(dir.path().join("policy.json")).unwrap());
    let mut granted = policy();
    granted
        .ssh
        .profiles
        .get_mut("fixture")
        .unwrap()
        .profile_fingerprint = Some("client-supplied-untrusted-pin".into());
    store.put(0, granted, &[profile()]).unwrap();
    let grants = HostGrants {
        store: store.clone(),
        profiles: Arc::new(Profiles(profile())),
        caller_id: "mcp-global".into(),
    };
    let effective = grants.policy("mcp-global").unwrap();
    assert_eq!(
        effective.profiles["fixture"].profile_fingerprint,
        Some(profile_fingerprint(&profile()))
    );
    assert!(grants.policy("another-caller").is_err());
    let mut changed = profile();
    changed.host = "different.invalid".into();
    let changed_grants = HostGrants {
        store,
        profiles: Arc::new(Profiles(changed)),
        caller_id: "mcp-global".into(),
    };
    assert!(changed_grants
        .policy("mcp-global")
        .unwrap()
        .profiles
        .is_empty());
}

#[cfg(unix)]
#[test]
fn production_host_ssh_uses_a_private_durable_store_without_target_io() {
    use se_manager_lib::mcp_core::host_adapters::production_host_ssh;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // macOS may supply TMPDIR through /var (a symlink to /private/var).
    // Exercise the secure store against the actual owned fixture directory.
    let root = dir.path().canonicalize().unwrap();
    let source = Arc::new(Profiles(profile()));
    let service = production_host_ssh(source.clone(), Arc::new(Secrets), &root).unwrap();
    let parent = root.join("mcp-ssh-jobs");
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(parent.join("jobs.json").is_file());
    assert!(production_host_ssh(source, Arc::new(Secrets), &root).is_err());
    drop(service);
}

#[cfg(not(unix))]
#[tokio::test]
async fn non_unix_host_reports_unsupported_jobs_without_target_io() {
    let dir = tempfile::tempdir().unwrap();
    let (host, backend) = host(&dir.path().join("policy.json"));
    host.put_access(0, policy()).unwrap();
    let error = host
        .call(
            "ssh",
            "job_start",
            json!({"profileId":"fixture","command":"never-run"})
                .as_object()
                .unwrap()
                .clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "UNSUPPORTED_PLATFORM");
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
}
