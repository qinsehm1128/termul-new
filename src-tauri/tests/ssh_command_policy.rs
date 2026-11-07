//! Command-policy acceptance using only temporary stores and fake SSH adapters.
use se_manager_lib::{
    mcp_core::{
        host_policy::{profile_fingerprint, HostAccessPolicy, HostGrants, HostPolicyStore},
        host_service::{CommandPolicyPreviewRequest, HostCapabilityService},
    },
    ssh::{
        automation::*, automation_jobs::JobManager, command_policy::*, profile_manager::SSHProfile,
    },
    web::project_registry::ProjectRegistry,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

fn profile() -> SSHProfile {
    serde_json::from_value(
        json!({"id":"p","name":"fixture","host":"never-connect.invalid","port":22,
        "username":"fixture","authMethod":"agent","tags":[],"hasStoredPassword":false,
        "hasStoredPassphrase":false,"portForwards":[],"privateKeyPath":null,"jumpHostId":null,
        "lastConnected":null,"importedFrom":null}),
    )
    .unwrap()
}
fn rule(id: &str, kind: CommandDenyKind, pattern: &str) -> CommandDenyRule {
    CommandDenyRule {
        id: id.into(),
        kind,
        pattern: pattern.into(),
    }
}
fn deny() -> CommandDenyRule {
    rule("deny", CommandDenyKind::Exact, "danger")
}
fn policy() -> SshPolicy {
    SshPolicy {
        profiles: HashMap::from([(
            "p".into(),
            ProfileGrant {
                profile_fingerprint: Some(profile_fingerprint(&profile())),
                operations: OPERATIONS.iter().map(|s| s.to_string()).collect(),
                posix_jobs: true,
                ..Default::default()
            },
        )]),
        ..Default::default()
    }
}
struct Source;
impl ProfileSource for Source {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        Ok(vec![profile()])
    }
}
struct Grants {
    policy: Mutex<SshPolicy>,
    snapshots: AtomicUsize,
    deny_at: usize,
}
impl Grants {
    fn new(deny_at: usize) -> Arc<Self> {
        Arc::new(Self {
            policy: Mutex::new(policy()),
            snapshots: AtomicUsize::new(0),
            deny_at,
        })
    }
}
impl GrantProvider for Grants {
    fn policy(&self, _: &str) -> AutomationResult<SshPolicy> {
        Ok(self.policy.lock().unwrap().clone())
    }
    fn snapshot(&self, _: &str) -> AutomationResult<SshGrantSnapshot> {
        let n = self.snapshots.fetch_add(1, Ordering::SeqCst) + 1;
        let mut policy = self.policy.lock().unwrap().clone();
        if n >= self.deny_at {
            policy.command_deny = vec![deny()];
        }
        let commands = Arc::new(CompiledCommandPolicy::compile(&policy)?);
        Ok(SshGrantSnapshot {
            policy,
            commands,
            revision: n as u64,
        })
    }
}
fn context(grants: Arc<dyn GrantProvider>) -> CallContext {
    CallContext {
        caller_id: "host".into(),
        grants,
    }
}
#[derive(Default)]
struct Resolver {
    calls: AtomicUsize,
    update: Option<Arc<Grants>>,
}
impl CredentialResolver for Resolver {
    fn resolve(&self, _: &SSHProfile) -> AutomationResult<Credentials> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(g) = &self.update {
            g.policy.lock().unwrap().command_deny = vec![deny()];
        }
        Ok(Credentials { secret: None })
    }
}
#[derive(Default)]
struct Backend {
    spoof_policy_error: bool,
    calls: AtomicUsize,
    commands: Mutex<Vec<String>>,
    hold: AtomicBool,
    started: AtomicBool,
    cancelled: AtomicBool,
}
impl AutomationBackend for Backend {
    fn execute(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        req: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.spoof_policy_error {
            return Err(AutomationError::new(
                "command_denied",
                "untrusted backend sentinel",
            ));
        }
        self.commands.lock().unwrap().push(req.command);
        self.started.store(true, Ordering::SeqCst);
        while self.hold.load(Ordering::SeqCst) {
            if req.cancel.load(Ordering::SeqCst) {
                self.cancelled.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(ExecResult {
            exit_code: Some(0),
            stdout: "ok".into(),
            stderr: String::new(),
            truncated: false,
            timed_out: false,
            cancel_confirmed: false,
            remote_termination_confirmed: false,
            duration_ms: 1,
        })
    }
    fn files(
        &self,
        _: &SSHProfile,
        _: &Credentials,
        _: &str,
        _: &Value,
        _: &ProfileGrant,
    ) -> AutomationResult<Value> {
        unreachable!()
    }
    fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
        unreachable!()
    }
}
fn request() -> ExecRequest {
    ExecRequest {
        command: "danger".into(),
        timeout_ms: 1000,
        max_bytes: 1024,
        job_token: None,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
async fn poll_job(service: &SshAutomationService, ctx: CallContext, id: &str) -> Value {
    for _ in 0..100 {
        let result = service
            .call(
                "job_status",
                json!({"profileId":"p","jobId":id}),
                ctx.clone(),
            )
            .await
            .unwrap();
        if !["queued", "running"].contains(&result["state"].as_str().unwrap()) {
            return result;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("fake job did not finish")
}

fn host_policy() -> HostAccessPolicy {
    HostAccessPolicy {
        enabled: true,
        ssh: policy(),
        profile_fingerprints: std::collections::BTreeMap::from([(
            "p".into(),
            profile_fingerprint(&profile()),
        )]),
        ..Default::default()
    }
}
fn host(path: &std::path::Path) -> (HostCapabilityService, Arc<Resolver>, Arc<Backend>) {
    let source = Arc::new(Source);
    let r = Arc::new(Resolver::default());
    let b = Arc::new(Backend::default());
    let ssh = Arc::new(SshAutomationService::new(
        source.clone(),
        r.clone(),
        b.clone(),
    ));
    (
        HostCapabilityService::new(
            Arc::new(HostPolicyStore::open(path.into()).unwrap()),
            source,
            Arc::new(ProjectRegistry::new()),
            ssh,
            "test",
        ),
        r,
        b,
    )
}
fn preview_request(policy: Option<HostAccessPolicy>) -> CommandPolicyPreviewRequest {
    CommandPolicyPreviewRequest {
        profile_id: "p".into(),
        operation: "execute".into(),
        command: "danger".into(),
        policy,
    }
}
#[test]
fn pure_preview_drafts_do_not_write_or_change_runtime_and_use_persisted_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    let (h, r, b) = host(&path);
    let ungranted = h.preview(preview_request(None)).unwrap();
    assert_eq!(ungranted.outcome, "not_matched");
    assert!(ungranted.blocked_by.is_some());
    assert!(!path.exists());
    let saved = h.put_access(0, host_policy()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut draft = saved.clone();
    draft.revision = 999;
    draft.ssh.command_deny = vec![deny()];
    let result = h.preview(preview_request(Some(draft))).unwrap();
    assert_eq!(result.outcome, "matched");
    assert_eq!(result.policy_revision, saved.revision);
    assert!(result.draft);
    assert_eq!(result.matches[0].id, "deny");
    assert_eq!(result.matches[0].source, CommandRuleSource::Global);
    assert_eq!(
        h.preview(preview_request(None)).unwrap().outcome,
        "not_matched"
    );
    assert_eq!(h.policy.snapshot().revision, 1);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    let mut bad = saved;
    bad.ssh.command_deny = vec![rule("bad", CommandDenyKind::Regex, "secret-sentinel(")];
    let e = h.preview(preview_request(Some(bad))).unwrap_err();
    assert_eq!(e.code, "POLICY_INVALID");
    assert!(!e.message.contains("sentinel"));
    let mut bad = preview_request(None);
    bad.operation = "job_cancel".into();
    assert_eq!(h.preview(bad).unwrap_err().code, "INVALID_ARGUMENTS");
}
#[test]
fn v1_migration_v2_reload_preserves_grants_roots_pins_and_reuses_compiler() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    let mut p = host_policy();
    p.schema_version = 1;
    p.revision = 7;
    p.ssh.profiles.get_mut("p").unwrap().remote_roots = vec!["/allowed".into()];
    p.profile_fingerprints
        .insert("p".into(), profile_fingerprint(&profile()));
    let mut v = serde_json::to_value(&p).unwrap();
    v["ssh"].as_object_mut().unwrap().remove("commandDeny");
    v["ssh"]["profiles"]["p"]
        .as_object_mut()
        .unwrap()
        .remove("commandDeny");
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let store = Arc::new(HostPolicyStore::open(path.clone()).unwrap());
    let migrated = store.snapshot();
    assert_eq!(migrated.schema_version, 2);
    assert_eq!(migrated.revision, 7);
    assert!(migrated.enabled);
    assert_eq!(migrated.ssh.profiles["p"].remote_roots, vec!["/allowed"]);
    assert_eq!(migrated.profile_fingerprints, p.profile_fingerprints);
    assert!(migrated.ssh.command_deny.is_empty());
    assert!(migrated.ssh.profiles["p"].command_deny.is_empty());
    let mut next = migrated;
    next.ssh.command_deny = vec![deny()];
    let saved = store.put(7, next, &[profile()]).unwrap();
    assert_eq!(saved.revision, 8);
    let grants = HostGrants {
        store: store.clone(),
        profiles: Arc::new(Source),
        caller_id: "host".into(),
    };
    let a = grants.snapshot("host").unwrap();
    let b = grants.snapshot("host").unwrap();
    assert!(Arc::ptr_eq(&a.commands, &b.commands));
    assert_eq!(a.revision, 8);
    assert_eq!(
        a.commands.check("p", "danger").unwrap_err().code,
        "command_denied"
    );
    let reopened = HostPolicyStore::open(path.clone()).unwrap();
    assert_eq!(reopened.snapshot().schema_version, 2);
    assert_eq!(
        reopened
            .compiled_snapshot()
            .commands
            .check("p", "danger")
            .unwrap_err()
            .code,
        "command_denied"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap()["schemaVersion"],
        2
    );
}
#[test]
fn invalid_and_stale_saves_do_not_publish_and_corrupt_reload_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    let store = HostPolicyStore::open(path.clone()).unwrap();
    let first = store.put(0, host_policy(), &[profile()]).unwrap();
    let snapshot = store.compiled_snapshot();
    let bytes = std::fs::read(&path).unwrap();
    let mut bad = first.clone();
    bad.ssh.command_deny = vec![rule("bad", CommandDenyKind::Regex, "(")];
    assert_eq!(
        store.put(1, bad, &[profile()]).unwrap_err().code,
        "POLICY_INVALID"
    );
    let mut stale = first;
    stale.ssh.command_deny = vec![deny()];
    assert_eq!(
        store.put(0, stale, &[profile()]).unwrap_err().code,
        "REVISION_CONFLICT"
    );
    assert!(Arc::ptr_eq(
        &snapshot.commands,
        &store.compiled_snapshot().commands
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(store
        .compiled_snapshot()
        .commands
        .check("p", "danger")
        .is_ok());
    for bytes in [b"{bad".to_vec(),serde_json::to_vec(&json!({"schemaVersion":2,"ssh":{"commandDeny":[{"id":"bad","kind":"regex","pattern":"("}]}})).unwrap()] {
        std::fs::write(&path,bytes).unwrap(); assert_eq!(HostPolicyStore::open(path.clone()).err().unwrap().code,"POLICY_INVALID");
    }
}
#[test]
fn failed_persistence_does_not_publish_compiled_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    let store = HostPolicyStore::open(path.clone()).unwrap();
    let before = store.compiled_snapshot();
    std::fs::create_dir(&path).unwrap();
    let mut next = host_policy();
    next.ssh.command_deny = vec![deny()];
    assert_eq!(
        store.put(0, next, &[profile()]).unwrap_err().code,
        "POLICY_WRITE_FAILED"
    );
    let after = store.compiled_snapshot();
    assert_eq!(after.policy.revision, 0);
    assert!(!after.policy.enabled);
    assert!(Arc::ptr_eq(&before.commands, &after.commands));
    assert!(after.commands.check("p", "danger").is_ok());
}
#[test]
fn operation_grant_and_identity_take_precedence_over_matches() {
    let g = Grants::new(1);
    let ctx = context(g.clone());
    g.policy
        .lock()
        .unwrap()
        .profiles
        .get_mut("p")
        .unwrap()
        .operations
        .clear();
    assert_eq!(
        ctx.authorize_command(&profile(), "execute", "danger")
            .unwrap_err()
            .code,
        "permission_denied"
    );
    *g.policy.lock().unwrap() = policy();
    let mut changed = profile();
    changed.host = "different.invalid".into();
    assert_eq!(
        ctx.authorize_command(&changed, "execute", "danger")
            .unwrap_err()
            .code,
        "profile_identity_changed"
    );
    g.policy
        .lock()
        .unwrap()
        .profiles
        .get_mut("p")
        .unwrap()
        .posix_jobs = false;
    assert_eq!(
        ctx.authorize_command(&profile(), "job_start", "danger")
            .unwrap_err()
            .code,
        "permission_denied"
    );
}

#[test]
fn concurrent_cas_publishes_exactly_one_complete_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(HostPolicyStore::open(dir.path().join("policy.json")).unwrap());
    store.put(0, host_policy(), &[profile()]).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let threads = (0..2)
        .map(|i| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut p = host_policy();
                p.ssh.command_deny = vec![rule(
                    &format!("r{i}"),
                    CommandDenyKind::Exact,
                    &format!("cmd{i}"),
                )];
                barrier.wait();
                store.put(1, p, &[profile()])
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let s = store.compiled_snapshot();
    assert_eq!(s.policy.revision, 2);
    let winner = &s.policy.ssh.command_deny[0];
    assert_eq!(
        s.commands.evaluate("p", &winner.pattern).unwrap()[0].id,
        winner.id
    );
}
#[test]
fn three_modes_union_and_original_text() {
    let mut p = policy();
    p.command_deny = vec![
        rule("exact", CommandDenyKind::Exact, "danger"),
        rule("substring", CommandDenyKind::Contains, "sudo"),
    ];
    p.profiles.get_mut("p").unwrap().command_deny =
        vec![rule("regexp", CommandDenyKind::Regex, r"(?m)^rm\s+-rf\b")];
    let c = CompiledCommandPolicy::compile(&p).unwrap();
    assert_eq!(
        c.evaluate("p", "danger").unwrap()[0].source,
        CommandRuleSource::Global
    );
    assert!(c.evaluate("p", " danger ").unwrap().is_empty());
    assert!(c.evaluate("p", "DANGER").unwrap().is_empty());
    assert_eq!(
        c.evaluate("p", "echo sudo;\nrm -rf /fixture")
            .unwrap()
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["substring", "regexp"]
    );
    assert_eq!(
        c.evaluate("p", "rm -rf /fixture").unwrap()[0].source,
        CommandRuleSource::Profile
    );
    assert!(c.evaluate("other", "rm -rf /fixture").unwrap().is_empty());
    assert_eq!(c.check("p", "danger").unwrap_err().code, "command_denied");
}
#[test]
fn validation_limits_and_invalid_rule_fail_closed() {
    for pattern in [
        "".to_string(),
        "x".repeat(513),
        "é".repeat(257),
        "a\0b".into(),
    ] {
        let mut p = policy();
        p.command_deny = vec![rule("x", CommandDenyKind::Exact, &pattern)];
        assert_eq!(
            CompiledCommandPolicy::compile(&p).err().unwrap().code,
            "POLICY_INVALID"
        );
    }
    for pattern in [
        "(",
        r"(?=a)",
        "((((((((((((((((((((((((((((((((((((a))))))))))))))))))))))))))))))))))))",
        r"a{1000000}",
    ] {
        let mut p = policy();
        p.command_deny = vec![rule("bad", CommandDenyKind::Regex, pattern)];
        let e = CompiledCommandPolicy::compile(&p).err().unwrap();
        assert_eq!(e.code, "POLICY_INVALID");
        assert!(!e.message.contains(pattern));
    }
    let mut p = policy();
    p.command_deny = (0..32)
        .map(|i| rule(&format!("r{i}"), CommandDenyKind::Exact, &"é".repeat(256)))
        .collect();
    assert!(CompiledCommandPolicy::compile(&p).is_ok());
    p.command_deny.push(deny());
    assert!(CompiledCommandPolicy::compile(&p).is_err());
    p.command_deny = vec![deny(), deny()];
    assert!(CompiledCommandPolicy::compile(&p).is_err());
    p.command_deny.clear();
    for i in 0..4 {
        let mut g = ProfileGrant::default();
        g.command_deny = (0..32)
            .map(|j| rule(&format!("r{j}"), CommandDenyKind::Exact, "a"))
            .collect();
        p.profiles.insert(format!("p{i}"), g);
    }
    assert!(CompiledCommandPolicy::compile(&p).is_ok());
    p.command_deny = vec![deny()];
    assert!(CompiledCommandPolicy::compile(&p).is_err());
    assert!(validate_command(&"x".repeat(32768)).is_ok());
    for command in [
        "".to_string(),
        "x".repeat(32769),
        "é".repeat(16385),
        "a\0b".into(),
    ] {
        assert_eq!(
            validate_command(&command).unwrap_err().code,
            "invalid_arguments"
        );
    }
    assert!(serde_json::from_value::<CommandDenyRule>(
        json!({"id":"x","kind":"glob","pattern":"*"})
    )
    .is_err());
}
#[tokio::test]
async fn initial_denial_has_no_credentials_backend_or_jobs() {
    let g = Grants::new(1);
    let r = Arc::new(Resolver::default());
    let b = Arc::new(Backend::default());
    let s = SshAutomationService::new(Arc::new(Source), r.clone(), b.clone());
    for op in ["execute", "job_start"] {
        assert_eq!(
            s.call(
                op,
                json!({"profileId":"p","command":"danger"}),
                context(g.clone())
            )
            .await
            .unwrap_err()
            .code,
            "command_denied"
        );
    }
    assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.call("job_list", json!({"profileId":"p"}), context(g))
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}
#[tokio::test]
async fn denial_precedes_full_worker_permits_and_allowed_text_is_unchanged() {
    let g = Grants::new(usize::MAX);
    let r = Arc::new(Resolver::default());
    let b = Arc::new(Backend::default());
    b.hold.store(true, Ordering::SeqCst);
    let s = Arc::new(SshAutomationService::new(
        Arc::new(Source),
        r.clone(),
        b.clone(),
    ));
    let original = "  printf 'fixture'\n";
    let mut tasks = vec![];
    for _ in 0..16 {
        let s = s.clone();
        let ctx = context(g.clone());
        tasks.push(tokio::spawn(async move {
            s.call("execute", json!({"profileId":"p","command":original}), ctx)
                .await
        }));
    }
    for _ in 0..200 {
        if b.calls.load(Ordering::SeqCst) == 16 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    g.policy.lock().unwrap().command_deny = vec![deny()];
    let denied = s
        .call(
            "execute",
            json!({"profileId":"p","command":"danger"}),
            context(g),
        )
        .await;
    b.hold.store(false, Ordering::SeqCst);
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(denied.unwrap_err().code, "command_denied");
    assert_eq!(r.calls.load(Ordering::SeqCst), 16);
    assert!(b.commands.lock().unwrap().iter().all(|c| c == original));
}

#[tokio::test]
async fn rule_edit_before_resolver_and_final_dispatch_is_rechecked() {
    let g = Grants::new(2);
    let r = Arc::new(Resolver::default());
    let b = Arc::new(Backend::default());
    let s = SshAutomationService::new(Arc::new(Source), r.clone(), b.clone());
    assert_eq!(
        s.call(
            "execute",
            json!({"profileId":"p","command":"danger"}),
            context(g)
        )
        .await
        .unwrap_err()
        .code,
        "command_denied"
    );
    assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    let g = Grants::new(usize::MAX);
    let r = Arc::new(Resolver {
        calls: AtomicUsize::new(0),
        update: Some(g.clone()),
    });
    let s = SshAutomationService::new(Arc::new(Source), r.clone(), b.clone());
    assert_eq!(
        s.call(
            "execute",
            json!({"profileId":"p","command":"danger"}),
            context(g)
        )
        .await
        .unwrap_err()
        .code,
        "command_denied"
    );
    assert_eq!(r.calls.load(Ordering::SeqCst), 1);
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn direct_job_entry_checks_before_record_or_backend() {
    let manager = Arc::new(JobManager::new());
    let g = Grants::new(1);
    let b = Arc::new(Backend::default());
    assert_eq!(
        manager
            .start(
                context(g),
                profile(),
                Credentials { secret: None },
                b.clone(),
                request()
            )
            .unwrap_err()
            .code,
        "command_denied"
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn job_worker_rechecks_and_retains_trusted_denial() {
    let g = Grants::new(5);
    let r = Arc::new(Resolver::default());
    let b = Arc::new(Backend::default());
    let s = SshAutomationService::new(Arc::new(Source), r, b.clone());
    let job = s
        .call(
            "job_start",
            json!({"profileId":"p","command":"danger"}),
            context(g.clone()),
        )
        .await
        .unwrap();
    let result = poll_job(&s, context(g.clone()), job["jobId"].as_str().unwrap()).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["errorCode"], "command_denied");
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    assert_eq!(g.snapshots.load(Ordering::SeqCst), 5);
}
#[tokio::test]
async fn backend_cannot_forge_a_trusted_policy_denial() {
    let g = Grants::new(usize::MAX);
    let b = Arc::new(Backend {
        spoof_policy_error: true,
        ..Default::default()
    });
    let s = SshAutomationService::new(Arc::new(Source), Arc::new(Resolver::default()), b);
    let job = s
        .call(
            "job_start",
            json!({"profileId":"p","command":"danger"}),
            context(g.clone()),
        )
        .await
        .unwrap();
    let result = poll_job(&s, context(g), job["jobId"].as_str().unwrap()).await;
    assert_eq!(result["errorCode"], "ssh_error");
    assert!(!result.to_string().contains("sentinel"));
}

#[tokio::test]
async fn new_rules_do_not_kill_jobs_or_filter_control_scripts() {
    let g = Grants::new(usize::MAX);
    let b = Arc::new(Backend::default());
    b.hold.store(true, Ordering::SeqCst);
    let s = SshAutomationService::new(Arc::new(Source), Arc::new(Resolver::default()), b.clone());
    let job = s
        .call(
            "job_start",
            json!({"profileId":"p","command":"danger"}),
            context(g.clone()),
        )
        .await
        .unwrap();
    for _ in 0..100 {
        if b.started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(b.started.load(Ordering::SeqCst));
    g.policy.lock().unwrap().command_deny = vec![rule("all", CommandDenyKind::Regex, "(?s).*")];
    let status = s
        .call(
            "job_status",
            json!({"profileId":"p","jobId":job["jobId"]}),
            context(g.clone()),
        )
        .await
        .unwrap();
    assert_eq!(status["state"], "running");
    assert!(!b.cancelled.load(Ordering::SeqCst));
    assert_eq!(
        s.call("job_list", json!({"profileId":"p"}), context(g.clone()))
            .await
            .unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    s.call(
        "job_cancel",
        json!({"profileId":"p","jobId":job["jobId"]}),
        context(g.clone()),
    )
    .await
    .unwrap();
    b.hold.store(false, Ordering::SeqCst);
    poll_job(&s, context(g), job["jobId"].as_str().unwrap()).await;
}
