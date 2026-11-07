//! Entirely in-memory automation contract tests. Never read SSH config, keys, or
//! credential stores, never start a UI, and never connect to a remote host.
use se_manager_lib::ssh::{automation::*, profile_manager::SSHProfile};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

fn profile(id: &str) -> SSHProfile {
    SSHProfile {
        id: id.into(),
        name: format!("Profile {id}"),
        host: "fake.invalid".into(),
        port: 22,
        username: "tester".into(),
        auth_method: "password".into(),
        private_key_path: Some("/DO-NOT-READ/secret-key".into()),
        password: Some("SOURCE-SECRET".into()),
        passphrase: Some("SOURCE-PASSPHRASE".into()),
        jump_host_id: None,
        port_forwards: vec![],
        tags: Some(vec!["test".into()]),
        last_connected: None,
        imported_from: None,
        has_stored_password: true,
        has_stored_passphrase: false,
    }
}
struct Source;
impl ProfileSource for Source {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        Ok(vec![profile("p"), profile("hidden")])
    }
}
struct Resolver(Arc<AtomicUsize>);
impl CredentialResolver for Resolver {
    fn resolve(&self, p: &SSHProfile) -> AutomationResult<Credentials> {
        assert!(
            p.password.is_none() && p.passphrase.is_none(),
            "service must strip source secrets"
        );
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Credentials {
            secret: Some("RESOLVER-SECRET".into()),
        })
    }
}
#[derive(Default)]
struct Grants(Mutex<HashMap<String, SshPolicy>>);
impl GrantProvider for Grants {
    fn policy(&self, caller: &str) -> AutomationResult<SshPolicy> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(caller)
            .cloned()
            .unwrap_or_default())
    }
}
fn all_grant() -> ProfileGrant {
    ProfileGrant {
        profile_fingerprint: Some(profile_fingerprint(&profile("p"))),
        operations: OPERATIONS.iter().map(|s| s.to_string()).collect(),
        remote_roots: vec!["/allowed".into()],
        local_roots: vec!["/authorized-local".into()],
        posix_jobs: true,
    }
}
fn ctx(caller: &str, grants: &Arc<Grants>) -> CallContext {
    CallContext {
        caller_id: caller.into(),
        grants: grants.clone(),
    }
}
fn allow(grants: &Arc<Grants>, caller: &str, grant: ProfileGrant) {
    grants.0.lock().unwrap().insert(
        caller.into(),
        SshPolicy {
            profiles: HashMap::from([("p".into(), grant)]),
        },
    );
}
#[derive(Default)]
struct FakeBackend {
    calls: AtomicUsize,
    cancellations: AtomicUsize,
    files: Mutex<HashMap<String, Vec<u8>>>,
}
impl AutomationBackend for FakeBackend {
    fn execute_with_progress(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
        progress: Arc<dyn ExecutionProgress>,
    ) -> AutomationResult<ExecResult> {
        if request.command == "long" {
            progress.output(true, b"running output\n");
        }
        self.execute(profile, credentials, request)
    }
    fn execute(
        &self,
        _: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        assert_eq!(credentials.secret.as_deref(), Some("RESOLVER-SECRET"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let began = Instant::now();
        if request.command == "long" {
            while began.elapsed() < Duration::from_millis(request.timeout_ms) {
                if request.cancel.load(Ordering::SeqCst) {
                    self.cancellations.fetch_add(1, Ordering::SeqCst);
                    return Ok(ExecResult {
                        stdout: String::new(),
                        stderr: String::new(),
                        exit_code: Some(143),
                        duration_ms: began.elapsed().as_millis() as u64,
                        truncated: false,
                        timed_out: false,
                        cancel_confirmed: true,
                        remote_termination_confirmed: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let timed_out = request.command == "timeout" || request.command == "long";
        let stdout = "fake stdout".as_bytes();
        let n = request.max_bytes.min(stdout.len());
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&stdout[..n]).into(),
            stderr: if n < request.max_bytes {
                "fake stderr".chars().take(request.max_bytes - n).collect()
            } else {
                String::new()
            },
            exit_code: if timed_out { None } else { Some(0) },
            duration_ms: began.elapsed().as_millis() as u64,
            truncated: request.max_bytes < 22,
            timed_out,
            cancel_confirmed: false,
            remote_termination_confirmed: false,
        })
    }
    fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"connected":true}))
    }
    fn files(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        op: &str,
        args: &Value,
        grant: &ProfileGrant,
    ) -> AutomationResult<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let path = args["path"].as_str().unwrap();
        if path.contains("/symlink") {
            return Err(AutomationError::new(
                "unsafe_path",
                "Symbolic links are not permitted",
            ));
        }
        if op == "upload" || op == "download" {
            let local = args["localPath"]
                .as_str()
                .ok_or_else(AutomationError::denied)?;
            if !grant
                .local_roots
                .iter()
                .any(|r| local.starts_with(&format!("{}/", r.display())))
            {
                return Err(AutomationError::denied());
            }
            return Ok(json!({"bytesTransferred":4}));
        }
        let mut files = self.files.lock().unwrap();
        match op {
            "write_file" => {
                if files.contains_key(path) {
                    return Err(AutomationError::new("already_exists", "Destination exists"));
                }
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| AutomationError::invalid("content required"))?;
                files.insert(path.into(), content.as_bytes().to_vec());
                Ok(json!({"bytesWritten":content.len()}))
            }
            "read_file" => {
                let data = files
                    .get(path)
                    .ok_or_else(|| AutomationError::new("not_found", "File missing"))?;
                Ok(json!({"content":String::from_utf8_lossy(data),"truncated":false}))
            }
            "list_files" => {
                let mut entries: Vec<_> = files
                    .keys()
                    .filter(|p| p.starts_with(&format!("{path}/")))
                    .cloned()
                    .collect();
                entries.sort();
                Ok(json!({"entries":entries}))
            }
            _ => Err(AutomationError::invalid("Unknown files operation")),
        }
    }
}
fn fixture() -> (
    SshAutomationService,
    Arc<Grants>,
    Arc<FakeBackend>,
    Arc<AtomicUsize>,
) {
    let grants = Arc::new(Grants::default());
    allow(&grants, "alice", all_grant());
    let backend = Arc::new(FakeBackend::default());
    let resolves = Arc::new(AtomicUsize::new(0));
    (
        SshAutomationService::new(
            Arc::new(Source),
            Arc::new(Resolver(resolves.clone())),
            backend.clone(),
        ),
        grants,
        backend,
        resolves,
    )
}

#[tokio::test]
async fn metadata_filters_profiles_and_never_serializes_secrets_or_key_paths() {
    let (service, grants, backend, resolves) = fixture();
    let list = service
        .call("profile_list", json!({}), ctx("alice", &grants))
        .await
        .unwrap();
    assert_eq!(list["profiles"].as_array().unwrap().len(), 1);
    let metadata = service
        .call(
            "profile_get",
            json!({"profileId":"p"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let serialized = serde_json::to_string(&metadata).unwrap();
    for forbidden in [
        "SOURCE-SECRET",
        "SOURCE-PASSPHRASE",
        "RESOLVER-SECRET",
        "DO-NOT-READ",
        "privateKeyPath",
        "\"password\":",
        "\"passphrase\":",
    ] {
        assert!(!serialized.contains(forbidden), "leaked {forbidden}");
    }
    assert_eq!(resolves.load(Ordering::SeqCst), 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    let filtered = service
        .call(
            "profile_list",
            json!({"query":"missing","tags":["test"]}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert!(filtered["profiles"].as_array().unwrap().is_empty());
    assert_eq!(
        service
            .call(
                "profile_get",
                json!({"profileId":"hidden"}),
                ctx("alice", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
}

#[tokio::test]
async fn deny_by_default_and_args_cannot_grant_permissions() {
    let (service, grants, backend, resolves) = fixture();
    assert_eq!(
        service
            .call(
                "execute",
                json!({"profileId":"p","command":"x"}),
                ctx("stranger", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    for field in [
        "grant",
        "selfApproved",
        "callerId",
        "password",
        "privateKey",
    ] {
        let mut args = json!({"profileId":"p","command":"x"});
        args[field] = json!(true);
        assert_eq!(
            service
                .call("execute", args, ctx("alice", &grants))
                .await
                .unwrap_err()
                .code,
            "invalid_arguments"
        );
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(resolves.load(Ordering::SeqCst), 0);
    assert!(serde_json::from_value::<SshPolicy>(json!({"allowAll":true})).is_err());
    assert!(SshPolicy::default().profiles.is_empty());
}

#[tokio::test]
async fn denied_path_roots_do_not_resolve_credentials() {
    let (service, grants, backend, resolves) = fixture();
    let mut grant = all_grant();
    grant.remote_roots.clear();
    grant.local_roots.clear();
    allow(&grants, "alice", grant);
    for op in [
        "read_file",
        "write_file",
        "list_files",
        "upload",
        "download",
    ] {
        let mut args = json!({"profileId":"p","remotePath":"/allowed/file"});
        if op == "write_file" {
            args["content"] = json!("test");
        }
        if op == "upload" || op == "download" {
            args["localPath"] = json!("/authorized-local/file");
        }
        assert_eq!(
            service
                .call(op, args, ctx("alice", &grants))
                .await
                .unwrap_err()
                .code,
            "permission_denied"
        );
    }
    assert_eq!(resolves.load(Ordering::SeqCst), 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rechecks_latest_policy_and_operation_permissions() {
    let (service, grants, backend, _) = fixture();
    service
        .call(
            "test_connection",
            json!({"profileId":"p"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let mut grant = all_grant();
    grant.operations = HashSet::from(["profile_get".into()]);
    allow(&grants, "alice", grant);
    assert_eq!(
        service
            .call(
                "test_connection",
                json!({"profileId":"p"}),
                ctx("alice", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    service
        .call(
            "profile_get",
            json!({"profileId":"p"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn execute_bounded_timeout_does_not_claim_remote_was_killed() {
    let (service, grants, _, _) = fixture();
    let output = service
        .call(
            "execute",
            json!({"profileId":"p","command":"ok","maxOutputBytes":4}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(output["exitCode"], 0);
    assert_eq!(output["stdout"], "fake");
    assert_eq!(output["truncated"], true);
    assert!(output["stderr"].as_str().unwrap().is_empty());
    let output = service
        .call(
            "execute",
            json!({"profileId":"p","command":"timeout"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(output["timedOut"], true);
    assert!(output["exitCode"].is_null());
    assert_eq!(output["remoteTerminationConfirmed"], false);
    for bad in [
        json!({"profileId":"p","command":"ok","timeoutMs":120001}),
        json!({"profileId":"p","command":"ok","maxOutputBytes":0}),
        json!({"profileId":"p","command":"ok","cwd":"/"}),
    ] {
        assert!(service
            .call("execute", bad, ctx("alice", &grants))
            .await
            .is_err());
    }
}

#[tokio::test]
async fn file_operations_use_single_file_paths_and_explicit_roots() {
    let (service, grants, _, _) = fixture();
    service
        .call(
            "write_file",
            json!({"profileId":"p","remotePath":"/allowed/a","content":"hello"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let read = service
        .call(
            "read_file",
            json!({"profileId":"p","remotePath":"/allowed/a"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(read["content"], "hello");
    let list = service
        .call(
            "list_files",
            json!({"profileId":"p","remotePath":"/allowed"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(list["entries"][0], "/allowed/a");
    assert_eq!(
        service
            .call(
                "write_file",
                json!({"profileId":"p","remotePath":"/allowed/a","content":"replacement"}),
                ctx("alice", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "already_exists"
    );
    for path in [
        "/allowed/../escape",
        "/allowed-other/a",
        "relative",
        "/allowed/./a",
        "/allowed//a",
    ] {
        assert_eq!(
            service
                .call(
                    "read_file",
                    json!({"profileId":"p","remotePath":path}),
                    ctx("alice", &grants)
                )
                .await
                .unwrap_err()
                .code,
            "permission_denied"
        );
    }
    assert_eq!(
        service
            .call(
                "read_file",
                json!({"profileId":"p","remotePath":"/allowed/symlink/file"}),
                ctx("alice", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "unsafe_path"
    );
    for op in ["upload", "download"] {
        service.call(op,json!({"profileId":"p","remotePath":"/allowed/file","localPath":"/authorized-local/file"}),ctx("alice",&grants)).await.unwrap();
        assert!(service
            .call(
                op,
                json!({"profileId":"p","remotePath":"/allowed/file","localPath":"/other/file"}),
                ctx("alice", &grants)
            )
            .await
            .is_err());
    }
}

#[tokio::test]
async fn jobs_owner_profile_rechecks_cancel_and_log_offsets() {
    let (service, grants, _, _) = fixture();
    allow(&grants, "bob", all_grant());
    let start = service
        .call(
            "job_start",
            json!({"profileId":"p","command":"long","runtimeLimit":5000}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let job_id = start["jobId"].as_str().expect("jobId");
    let args = json!({"profileId":"p","jobId":job_id});
    assert!(service
        .call("job_status", args.clone(), ctx("bob", &grants))
        .await
        .is_err());
    assert!(service
        .call("job_cancel", args.clone(), ctx("bob", &grants))
        .await
        .is_err());
    let list = service
        .call("job_list", json!({"profileId":"p"}), ctx("alice", &grants))
        .await
        .unwrap();
    assert!(list.to_string().contains(job_id));
    service
        .call("job_cancel", args.clone(), ctx("alice", &grants))
        .await
        .unwrap();
    let mut terminal = false;
    for _ in 0..100 {
        let status = service
            .call("job_status", args.clone(), ctx("alice", &grants))
            .await
            .unwrap();
        if status["state"] == "cancelled" {
            assert_eq!(status["cancelConfirmed"], true);
            terminal = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(terminal, "fake backend cancellation must be confirmed");
    let mut grant = all_grant();
    grant.operations.remove("job_status");
    allow(&grants, "alice", grant);
    assert_eq!(
        service
            .call("job_status", args, ctx("alice", &grants))
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
}

#[cfg(unix)]
fn private_fixture() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[cfg(unix)]
fn durable_service(path: std::path::PathBuf, backend: Arc<FakeBackend>) -> SshAutomationService {
    SshAutomationService::with_job_store(
        Arc::new(Source),
        Arc::new(Resolver(Arc::new(AtomicUsize::new(0)))),
        backend,
        path,
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn durable_store_rejects_public_or_symlink_files_without_loading_them() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = private_fixture();
    let path = directory.path().join("unsafe.json");
    std::fs::write(&path, b"{\"DO-NOT-EXPOSE\":true}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let backend = Arc::new(FakeBackend::default());
    let make = |path| {
        SshAutomationService::with_job_store(
            Arc::new(Source),
            Arc::new(Resolver(Arc::new(AtomicUsize::new(0)))),
            backend.clone(),
            path,
        )
    };
    let error = make(path.clone()).err().expect("public store must fail");
    assert!(!error.to_string().contains("DO-NOT-EXPOSE"));
    let link = directory.path().join("link.json");
    symlink(&path, &link).unwrap();
    assert!(make(link).is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn durable_jobs_recover_completed_results_without_commands_or_secrets() {
    let directory = private_fixture();
    let path = directory.path().join("jobs.json");
    let grants = Arc::new(Grants::default());
    allow(&grants, "alice", all_grant());
    allow(&grants, "bob", all_grant());
    let backend = Arc::new(FakeBackend::default());
    let service = durable_service(path.clone(), backend.clone());
    let start = service
        .call(
            "job_start",
            json!({"profileId":"p","command":"DO-NOT-PERSIST-COMMAND"}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let args = json!({"profileId":"p","jobId":start["jobId"]});
    let mut complete = false;
    for _ in 0..100 {
        let status = service
            .call("job_status", args.clone(), ctx("alice", &grants))
            .await
            .unwrap();
        if status["state"] == "succeeded" {
            complete = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(complete);
    // Poll the atomic snapshot as well: the worker's state transition may be
    // observed immediately before its durable flush completes.
    let mut durable = false;
    for _ in 0..100 {
        if std::fs::read_to_string(&path).is_ok_and(|s| s.contains("succeeded")) {
            durable = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(durable);
    drop(service);
    let bytes = std::fs::read_to_string(&path).unwrap();
    for secret in [
        "DO-NOT-PERSIST-COMMAND",
        "SOURCE-SECRET",
        "SOURCE-PASSPHRASE",
        "RESOLVER-SECRET",
        "DO-NOT-READ",
    ] {
        assert!(!bytes.contains(secret), "store leaked {secret}");
    }
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let restarted = durable_service(path, backend.clone());
    let status = restarted
        .call("job_status", args.clone(), ctx("alice", &grants))
        .await
        .unwrap();
    assert_eq!(status["state"], "succeeded");
    assert!(status["output"].as_str().unwrap().contains("fake stdout"));
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "recovery must not execute or reconnect"
    );
    assert!(restarted
        .call("job_status", args.clone(), ctx("bob", &grants))
        .await
        .is_err());
    allow(&grants, "alice", ProfileGrant::default());
    assert!(restarted
        .call("job_status", args, ctx("alice", &grants))
        .await
        .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn running_logs_and_crash_snapshot_recover_as_interrupted_without_blind_cancel() {
    use std::os::unix::fs::PermissionsExt;
    let directory = private_fixture();
    let path = directory.path().join("live.json");
    let grants = Arc::new(Grants::default());
    allow(&grants, "alice", all_grant());
    let backend = Arc::new(FakeBackend::default());
    let service = durable_service(path.clone(), backend.clone());
    let start = service
        .call(
            "job_start",
            json!({"profileId":"p","command":"long","runtimeLimit":5000,"maxOutputBytes":32}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    let args = json!({"profileId":"p","jobId":start["jobId"]});
    let mut observed = false;
    for _ in 0..100 {
        let status = service
            .call("job_status", args.clone(), ctx("alice", &grants))
            .await
            .unwrap();
        if status["state"] == "running"
            && status["output"]
                .as_str()
                .is_some_and(|s| s.contains("running output"))
        {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        observed,
        "job_status must expose output before backend returns"
    );
    let paged = service
        .call(
            "job_status",
            json!({"profileId":"p","jobId":start["jobId"],"offset":0,"maxBytes":7}),
            ctx("alice", &grants),
        )
        .await
        .unwrap();
    assert_eq!(paged["output"], "running");
    assert_eq!(paged["nextOffset"], 7);
    // Clone an active atomic snapshot into a DIFFERENT private store to simulate
    // a crash without allowing two writers to the original store.
    let recovered_path = directory.path().join("crashed.json");
    std::fs::copy(&path, &recovered_path).unwrap();
    std::fs::set_permissions(&recovered_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let recovered = durable_service(recovered_path, backend.clone());
    let status = recovered
        .call("job_status", args.clone(), ctx("alice", &grants))
        .await
        .unwrap();
    assert_eq!(status["state"], "interrupted");
    assert_eq!(status["remoteTerminationConfirmed"], false);
    assert_eq!(status["cancelConfirmed"], false);
    let before = backend.calls.load(Ordering::SeqCst);
    assert!(
        recovered
            .call("job_cancel", args.clone(), ctx("alice", &grants))
            .await
            .is_err(),
        "recovered unknown process must not be blindly killed"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), before);
    service
        .call("job_cancel", args, ctx("alice", &grants))
        .await
        .unwrap();
}

#[tokio::test]
async fn host_cancellation_and_dropped_future_reach_blocking_execute() {
    use tokio_util::sync::CancellationToken;
    let (service, grants, backend, _) = fixture();
    let service = Arc::new(service);
    let token = CancellationToken::new();
    let task = {
        let service = service.clone();
        let context = ctx("alice", &grants);
        let token = token.clone();
        tokio::spawn(async move {
            service
                .call_cancellable(
                    "execute",
                    json!({"profileId":"p","command":"long","timeoutMs":5000}),
                    context,
                    token,
                )
                .await
        })
    };
    for _ in 0..100 {
        if backend.calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result["cancelConfirmed"], true);
    let task = {
        let service = service.clone();
        let context = ctx("alice", &grants);
        tokio::spawn(async move {
            service
                .call(
                    "execute",
                    json!({"profileId":"p","command":"long","timeoutMs":5000}),
                    context,
                )
                .await
        })
    };
    for _ in 0..100 {
        if backend.calls.load(Ordering::SeqCst) > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    task.abort();
    let _ = task.await;
    for _ in 0..100 {
        if backend.cancellations.load(Ordering::SeqCst) > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        backend.cancellations.load(Ordering::SeqCst),
        2,
        "dropped host future must not leave execute running"
    );
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        service
            .call_cancellable(
                "execute",
                json!({"profileId":"p","command":"long"}),
                ctx("alice", &grants),
                token
            )
            .await
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn network_grants_pin_connection_identity_but_not_metadata() {
    let (service, grants, backend, resolves) = fixture();
    for fingerprint in [None, Some("stale-consent".into())] {
        let mut grant = all_grant();
        grant.profile_fingerprint = fingerprint;
        allow(&grants, "alice", grant);
        assert_eq!(
            service
                .call(
                    "test_connection",
                    json!({"profileId":"p"}),
                    ctx("alice", &grants)
                )
                .await
                .unwrap_err()
                .code,
            "profile_identity_changed"
        );
        let metadata = service
            .call(
                "profile_get",
                json!({"profileId":"p"}),
                ctx("alice", &grants),
            )
            .await
            .unwrap();
        assert_eq!(
            metadata["profileFingerprint"],
            profile_fingerprint(&profile("p"))
        );
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(resolves.load(Ordering::SeqCst), 0);
    let original = profile("p");
    let pin = profile_fingerprint(&original);
    let mut changed = original.clone();
    changed.last_connected = Some("tomorrow".into());
    changed.name = "renamed".into();
    changed.password = Some("different".into());
    assert_eq!(profile_fingerprint(&changed), pin);
    changed.host = "different.invalid".into();
    assert_ne!(profile_fingerprint(&changed), pin);
    changed = original.clone();
    changed.port = 2222;
    assert_ne!(profile_fingerprint(&changed), pin);
    changed = original.clone();
    changed.username = "other".into();
    assert_ne!(profile_fingerprint(&changed), pin);
    changed = original.clone();
    changed.auth_method = "agent".into();
    assert_ne!(profile_fingerprint(&changed), pin);
    changed = original;
    changed.private_key_path = Some("/another/key".into());
    assert_ne!(profile_fingerprint(&changed), pin);
}

#[tokio::test]
async fn revocation_during_credential_resolution_prevents_backend_call() {
    struct RevokingResolver(Arc<Grants>);
    impl CredentialResolver for RevokingResolver {
        fn resolve(&self, _: &SSHProfile) -> AutomationResult<Credentials> {
            self.0 .0.lock().unwrap().clear();
            Ok(Credentials::default())
        }
    }
    let grants = Arc::new(Grants::default());
    allow(&grants, "alice", all_grant());
    let backend = Arc::new(FakeBackend::default());
    let service = SshAutomationService::new(
        Arc::new(Source),
        Arc::new(RevokingResolver(grants.clone())),
        backend.clone(),
    );
    assert_eq!(
        service
            .call(
                "execute",
                json!({"profileId":"p","command":"ok"}),
                ctx("alice", &grants)
            )
            .await
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn jobs_require_explicit_posix_opt_in() {
    let (service, grants, backend, _) = fixture();
    let mut grant = all_grant();
    grant.posix_jobs = false;
    allow(&grants, "alice", grant);
    assert!(service
        .call(
            "job_start",
            json!({"profileId":"p","command":"ok"}),
            ctx("alice", &grants)
        )
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_bodies_and_command_bounds_are_rejected_without_backend_work() {
    let (service, grants, backend, _) = fixture();
    for (operation, args) in [
        ("bogus", json!({})),
        ("profile_list", json!([])),
        ("execute", json!({"profileId":"p","command":""})),
        (
            "execute",
            json!({"profileId":"p","command":"x".repeat(32*1024+1)}),
        ),
        ("execute", json!({"profileId":"p","command":"x\u{0}y"})),
        ("profile_list", json!({"offset":-1})),
        ("profile_list", json!({"limit":201})),
        ("profile_list", json!({"tags":[true]})),
    ] {
        assert!(service
            .call(operation, args, ctx("alice", &grants))
            .await
            .is_err());
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}
