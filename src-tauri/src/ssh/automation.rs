//! Window-independent MCP-only SSH automation. Interactive terminals and PTYs do not use
//! this service or its command policy. Context and grants MUST be supplied by the host,
//! never deserialized from tool arguments. No UI/PTY sessions are used here.
use super::{
    command_policy::{validate_command, CommandDenyRule, CompiledCommandPolicy},
    profile_manager::SSHProfile,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

pub const OPERATIONS: &[&str] = &[
    "profile_list",
    "profile_get",
    "test_connection",
    "execute",
    "list_files",
    "read_file",
    "write_file",
    "upload",
    "download",
    "job_start",
    "job_status",
    "job_list",
    "job_cancel",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationError {
    pub code: String,
    pub message: String,
}
impl AutomationError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn denied() -> Self {
        Self::new(
            "permission_denied",
            "SSH operation is not granted to MCP for this profile; the Se host owner must enable it in Settings > MCP > Host access",
        )
    }
    /// Same stable code as [`denied`], but tells the caller which host setting is missing.
    /// Never echoes configured paths or the requested path.
    pub fn denied_because(message: &str) -> Self {
        Self::new("permission_denied", message)
    }
    pub fn invalid(message: &str) -> Self {
        Self::new("invalid_arguments", message)
    }
    pub fn backend() -> Self {
        Self::new(
            "ssh_error",
            "SSH operation failed; inspect host diagnostics",
        )
    }
}
impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AutomationError {}
pub type AutomationResult<T> = Result<T, AutomationError>;

pub trait ProfileSource: Send + Sync {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>>;
}
/// Only this host-injected resolver may access the OS credential store. No Debug/Serialize.
#[derive(Default)]
pub struct Credentials {
    pub secret: Option<String>,
}
pub trait CredentialResolver: Send + Sync {
    fn resolve(&self, profile: &SSHProfile) -> AutomationResult<Credentials>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SshPolicy {
    pub command_deny: Vec<CommandDenyRule>,
    pub profiles: HashMap<String, ProfileGrant>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGrant {
    /// Required for network operations; changing connection identity requires consent again.
    pub profile_fingerprint: Option<String>,
    pub operations: HashSet<String>,
    pub remote_roots: Vec<String>,
    pub local_roots: Vec<PathBuf>,
    pub posix_jobs: bool,
    pub command_deny: Vec<CommandDenyRule>,
}
#[derive(Clone)]
pub struct SshGrantSnapshot {
    pub policy: SshPolicy,
    pub commands: Arc<CompiledCommandPolicy>,
    pub revision: u64,
}
pub trait GrantProvider: Send + Sync {
    fn policy(&self, caller_id: &str) -> AutomationResult<SshPolicy>;
    fn snapshot(&self, caller_id: &str) -> AutomationResult<SshGrantSnapshot> {
        let policy = self.policy(caller_id)?;
        let commands = Arc::new(CompiledCommandPolicy::compile(&policy)?);
        Ok(SshGrantSnapshot {
            policy,
            commands,
            revision: 0,
        })
    }
}
#[derive(Clone)]
pub struct CallContext {
    pub caller_id: String,
    pub grants: Arc<dyn GrantProvider>,
}
impl CallContext {
    pub fn authorize_command(
        &self,
        profile: &SSHProfile,
        operation: &str,
        command: &str,
    ) -> AutomationResult<ProfileGrant> {
        validate_command(command)?;
        if self.caller_id.is_empty() {
            return Err(AutomationError::denied());
        }
        let snapshot = self.grants.snapshot(&self.caller_id)?;
        let grant = snapshot
            .policy
            .profiles
            .get(&profile.id)
            .filter(|g| g.operations.contains(operation))
            .cloned()
            .ok_or_else(AutomationError::denied)?;
        check_profile_identity(profile, &grant)?;
        if operation == "job_start" && !grant.posix_jobs {
            return Err(posix_jobs_disabled());
        }
        let result = snapshot.commands.check(&profile.id, command);
        let operation = if operation == "job_start" {
            "job_start"
        } else {
            "execute"
        };
        match &result {
            Ok(()) => {
                log::debug!(target:"se_manager::mcp_core::host", "operation={} stable_code=COMMAND_NOT_MATCHED revision={}", operation, snapshot.revision)
            }
            Err(_) => {
                log::warn!(target:"se_manager::mcp_core::host", "operation={} stable_code=COMMAND_DENIED revision={}", operation, snapshot.revision)
            }
        }
        result?;
        Ok(grant)
    }
    pub fn authorize(&self, profile_id: &str, operation: &str) -> AutomationResult<ProfileGrant> {
        self.grants
            .policy(&self.caller_id)?
            .profiles
            .get(profile_id)
            .filter(|g| g.operations.contains(operation))
            .cloned()
            .ok_or_else(AutomationError::denied)
    }
}
#[derive(Clone)]
pub struct ExecRequest {
    pub command: String,
    pub timeout_ms: u64,
    pub max_bytes: usize,
    /// Jobs require a POSIX shell + setsid and an isolated process group.
    pub job_token: Option<String>,
    pub cancel: Arc<AtomicBool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub truncated: bool,
    pub timed_out: bool,
    pub cancel_confirmed: bool,
    /// A timeout/channel close alone never establishes remote termination.
    pub remote_termination_confirmed: bool,
}
/// Optional bounded-progress consumer. Backends emit command output only (never
/// protocol preambles, commands or credentials). Consumers must enforce quotas.
pub trait ExecutionProgress: Send + Sync {
    fn output(&self, stdout: bool, chunk: &[u8]);
}

pub trait AutomationBackend: Send + Sync {
    /// Optional live progress; non-streaming/fake backends remain source compatible.
    fn execute_with_progress(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
        _progress: Arc<dyn ExecutionProgress>,
    ) -> AutomationResult<ExecResult> {
        self.execute(profile, credentials, request)
    }
    fn execute(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
    ) -> AutomationResult<ExecResult>;
    fn files(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        operation: &str,
        args: &Value,
        grant: &ProfileGrant,
    ) -> AutomationResult<Value>;
    fn test_connection(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
    ) -> AutomationResult<Value>;
}

pub struct SshAutomationService {
    source: Arc<dyn ProfileSource>,
    resolver: Arc<dyn CredentialResolver>,
    backend: Arc<dyn AutomationBackend>,
    jobs: Arc<super::automation_jobs::JobManager>,
    workers: Arc<tokio::sync::Semaphore>,
}

impl SshAutomationService {
    pub fn new(
        source: Arc<dyn ProfileSource>,
        resolver: Arc<dyn CredentialResolver>,
        backend: Arc<dyn AutomationBackend>,
    ) -> Self {
        Self {
            source,
            resolver,
            backend,
            jobs: Arc::new(super::automation_jobs::JobManager::new()),
            workers: Arc::new(tokio::sync::Semaphore::new(16)),
        }
    }
    /// Opt-in host-private persistence. Fail closed on an unsafe/unreadable store;
    /// callers must not silently fall back to a memory-only service on error.
    ///
    /// The host owns this path and must provide a dedicated private directory.
    /// Persisted data excludes commands and credentials, but command output can
    /// itself be sensitive and requires the same private retention controls.
    /// Recovery is observation-only: previous active records become interrupted
    /// with unknown remote liveness. No PID is trusted or killed during recovery.
    /// Use one service instance per host and share it with all host transports.
    pub fn with_job_store(
        source: Arc<dyn ProfileSource>,
        resolver: Arc<dyn CredentialResolver>,
        backend: Arc<dyn AutomationBackend>,
        job_store: PathBuf,
    ) -> AutomationResult<Self> {
        Ok(Self {
            source,
            resolver,
            backend,
            jobs: Arc::new(super::automation_jobs::JobManager::with_store(job_store)?),
            workers: Arc::new(tokio::sync::Semaphore::new(16)),
        })
    }
    pub fn production_with_job_store(
        source: Arc<dyn ProfileSource>,
        resolver: Arc<dyn CredentialResolver>,
        job_store: PathBuf,
    ) -> AutomationResult<Self> {
        Self::with_job_store(
            source,
            resolver,
            Arc::new(super::automation_backend::Libssh2Backend::new()),
            job_store,
        )
    }

    pub fn production(
        source: Arc<dyn ProfileSource>,
        resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self::new(
            source,
            resolver,
            Arc::new(super::automation_backend::Libssh2Backend::new()),
        )
    }
    pub async fn call(
        &self,
        operation: &str,
        args: Value,
        context: CallContext,
    ) -> AutomationResult<Value> {
        self.call_cancellable(
            operation,
            args,
            context,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }
    /// Host transport cancellation is not remote termination confirmation. Dropping
    /// this future also signals execute's atomic cancellation flag. File operations
    /// already in flight remain bounded but cannot be rolled back. Jobs detach at start.
    pub async fn call_cancellable(
        &self,
        operation: &str,
        args: Value,
        context: CallContext,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AutomationResult<Value> {
        use std::sync::atomic::Ordering;
        struct CancelOnDrop(Arc<AtomicBool>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let cancel = Arc::new(AtomicBool::new(cancellation.is_cancelled()));
        let _on_drop = CancelOnDrop(cancel.clone());
        let work = self.call_inner(operation, args, context, cancel.clone());
        tokio::pin!(work);
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                cancel.store(true, Ordering::Release);
                work.await
            }
            result = &mut work => result,
        };
        if let Err(ref error) = result {
            // The operation is selected from a fixed vocabulary; never log command,
            // arguments, paths, credentials, backend errors or returned output.
            let label = if OPERATIONS.contains(&operation) {
                operation
            } else {
                "unknown"
            };
            let category = match error.code.as_str() {
                "permission_denied" => "permission_denied",
                "invalid_arguments" => "invalid_arguments",
                "profile_identity_changed" => "profile_identity_changed",
                "busy" => "busy",
                _ => "operation_failed",
            };
            log::warn!(
                "[SSH automation] operation={} failed code={}",
                label,
                category
            );
        }
        result
    }
    async fn call_inner(
        &self,
        operation: &str,
        mut args: Value,
        context: CallContext,
        cancel: Arc<AtomicBool>,
    ) -> AutomationResult<Value> {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AutomationError::new(
                "cancelled",
                "SSH request was cancelled before dispatch",
            ));
        }
        validate_arguments(operation, &args)?;
        if context.caller_id.is_empty() {
            return Err(AutomationError::denied());
        }
        if ["execute", "job_start"].contains(&operation) {
            let id = required_string(&args, "profileId", 256)?;
            context.authorize(id, operation)?;
            let profile = self
                .source
                .list_profiles()?
                .into_iter()
                .find(|p| p.id == id)
                .ok_or_else(|| {
                    AutomationError::new("profile_not_found", "SSH profile is unavailable")
                })?;
            context.authorize_command(
                &profile,
                operation,
                required_string(&args, "command", 32 * 1024)?,
            )?;
        }
        let permit = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| AutomationError::new("busy", "SSH automation worker limit reached"))?;
        let source = self.source.clone();
        let resolver = self.resolver.clone();
        let backend = self.backend.clone();
        let jobs = self.jobs.clone();
        let operation = operation.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if cancel.load(std::sync::atomic::Ordering::Acquire) { return Err(AutomationError::new("cancelled", "SSH request was cancelled before dispatch")); }
            if operation == "profile_list" {
                let query = args.get("query").and_then(Value::as_str).unwrap_or("").to_lowercase();
                let offset = integer(&args, "offset", 0, 0, 100_000)? as usize;
                let limit = integer(&args, "limit", 50, 1, 200)? as usize;
                let tags: Vec<&str> = args.get("tags").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let available_profiles = source.list_profiles()?;
                let policy = context.grants.policy(&context.caller_id)?;
                let mut profiles: Vec<_> = available_profiles.into_iter().filter(|p| {
                    policy.profiles.get(&p.id).map(|g| g.operations.contains("profile_list")).unwrap_or(false)
                    && (query.is_empty() || p.name.to_lowercase().contains(&query))
                    && tags.iter().all(|tag| p.tags.as_ref().map(|t| t.iter().any(|v| v == tag)).unwrap_or(false))
                }).collect();
                profiles.sort_by(|a,b| a.id.cmp(&b.id));
                let total = profiles.len();
                let mut items = Vec::new();
                // Pagination and counts derive from the same freshly authorized
                // snapshot, never from ungranted or subsequently skipped entries.
                for profile in profiles.into_iter().skip(offset).take(limit) {
                    if let Some(grant) = policy.profiles.get(&profile.id) { items.push(profile_metadata(&profile, grant)); }
                }
                return Ok(serde_json::json!({"profiles":items,"nextOffset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None}}));
            }
            let profile_id = required_string(&args, "profileId", 256)?.to_owned();
            context.authorize(&profile_id, &operation)?;
            if operation == "job_status" { return jobs.status(&context, &profile_id, &args); }
            if operation == "job_list" { return jobs.list(&context, &profile_id, &args); }
            if operation == "job_cancel" { return jobs.cancel(&context, &profile_id, &args); }
            let mut profile = source.list_profiles()?.into_iter().find(|p| p.id == profile_id)
                .ok_or_else(|| AutomationError::new("profile_not_found", "SSH profile is unavailable"))?;
            // A repository can accidentally return populated transient fields. Only
            // the injected resolver is permitted to provide authentication material.
            profile.password = None;
            profile.passphrase = None;
            let grant = context.authorize(&profile_id, &operation)?;
            if operation == "profile_get" { return Ok(profile_metadata(&profile, &grant)); }
            if profile.jump_host_id.is_some() { return Err(AutomationError::new("unsupported_profile", "Automation jump hosts are not supported")); }
            check_profile_identity(&profile, &grant)?;
            if ["list_files", "read_file", "write_file", "upload", "download"].contains(&operation.as_str()) {
                check_remote_path(required_string(&args,"remotePath",4096)?, &grant.remote_roots)?;
                if ["upload", "download"].contains(&operation.as_str()) && grant.local_roots.is_empty() {
                    return Err(no_local_roots());
                }
            }
            if ["execute", "job_start"].contains(&operation.as_str()) {
                context.authorize_command(&profile, &operation, required_string(&args, "command", 32 * 1024)?)?;
            }
            let credentials = resolver.resolve(&profile)?;
            let grant = context.authorize(&profile_id, &operation)?;
            check_profile_identity(&profile, &grant)?;
            if cancel.load(std::sync::atomic::Ordering::Acquire) { return Err(AutomationError::new("cancelled", "SSH request was cancelled before dispatch")); }
            match operation.as_str() {
                "test_connection" => backend.test_connection(&profile, &credentials),
                "execute" | "job_start" => {
                    if args.get("cwd").is_some() { return Err(AutomationError::invalid("cwd is not supported; explicitly include it in the authorized command")); }
                    let command = required_string(&args, "command", 32 * 1024)?.to_owned();
                    let is_job = operation == "job_start";
                    let timeout_ms = if is_job { integer(&args,"runtimeLimit",300_000,100,3_600_000)? } else { integer(&args,"timeoutMs",30_000,100,120_000)? };
                    let max_bytes = integer(&args,"maxOutputBytes",256*1024,1,1024*1024)? as usize;
                    let request = ExecRequest { command, timeout_ms, max_bytes, job_token:None, cancel:cancel.clone() };
                    context.authorize_command(&profile, &operation, &request.command)?;
                    if is_job { jobs.start(context, profile, credentials, backend, request) }
                    else { serde_json::to_value(backend.execute(&profile, &credentials, request)?).map_err(|_| AutomationError::backend()) }
                }
                "list_files" | "read_file" | "write_file" | "upload" | "download" => {
                    // Canonical/symlink and local root checks also happen inside the
                    // backend. This lexical gate prevents obvious traversal early.
                    let path = required_string(&args,"remotePath",4096)?.to_owned();
                    check_remote_path(&path, &grant.remote_roots)?;
                    args.as_object_mut().unwrap().insert("path".into(), Value::String(path));
                    backend.files(&profile, &credentials, &operation, &args, &grant)
                }
                _ => Err(AutomationError::invalid("Unknown SSH operation")),
            }
        }).await.map_err(|_| AutomationError::new("worker_failed", "SSH worker failed"))?
    }
}

/// Stable consent identity. Excludes secrets and mutable UI metadata/timestamps.
pub fn profile_fingerprint(profile: &SSHProfile) -> String {
    use sha2::{Digest, Sha256};
    let identity = serde_json::json!([
        profile.id,
        profile.host,
        profile.port,
        profile.username,
        profile.auth_method,
        profile.private_key_path,
        profile.jump_host_id
    ]);
    format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
}
fn check_profile_identity(profile: &SSHProfile, grant: &ProfileGrant) -> AutomationResult<()> {
    if grant.profile_fingerprint.as_deref() != Some(profile_fingerprint(profile).as_str()) {
        return Err(AutomationError::new(
            "profile_identity_changed",
            "SSH profile identity needs host authorization",
        ));
    }
    Ok(())
}

fn profile_metadata(profile: &SSHProfile, grant: &ProfileGrant) -> Value {
    let mut operations: Vec<_> = grant
        .operations
        .iter()
        .filter(|o| OPERATIONS.contains(&o.as_str()))
        .cloned()
        .collect();
    operations.sort();
    serde_json::json!({
        "profileId": profile.id, "profileFingerprint": profile_fingerprint(profile), "name": profile.name, "host": profile.host,
        "port": profile.port, "username": profile.username, "authMethod": profile.auth_method,
        "tags": profile.tags, "lastConnected": profile.last_connected,
        "credentialConfigured": profile.auth_method == "agent" || profile.has_stored_password || profile.has_stored_passphrase || profile.private_key_path.is_some(),
        "operations": operations, "posixJobs": grant.posix_jobs,
        // Grant scope, not credentials: callers need it to choose paths that will pass.
        "remoteRoots": grant.remote_roots, "localRoots": grant.local_roots,
    })
}
pub(crate) fn posix_jobs_disabled() -> AutomationError {
    AutomationError::denied_because(
        "POSIX jobs are disabled for this profile; the Se host owner must enable 'Allow POSIX jobs' in Settings > MCP > Host access",
    )
}
pub(crate) fn no_local_roots() -> AutomationError {
    AutomationError::denied_because(
        "No Se host local roots are configured for this profile; upload/download stay denied until the host owner adds one in Settings > MCP > Host access",
    )
}
pub(crate) fn outside_remote_roots(roots: &[String]) -> AutomationError {
    AutomationError::denied_because(if roots.is_empty() {
        "No remote roots are configured for this profile; remote file operations stay denied until the host owner adds one in Settings > MCP > Host access"
    } else {
        "remotePath is outside the authorized remote roots (see profile_get remoteRoots)"
    })
}

pub(crate) fn required_string<'a>(
    args: &'a Value,
    key: &str,
    max: usize,
) -> AutomationResult<&'a str> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AutomationError::invalid("Required string argument is missing"))?;
    if value.is_empty() || value.len() > max || value.contains('\0') {
        return Err(AutomationError::invalid(
            "String argument is empty, oversized or contains NUL",
        ));
    }
    Ok(value)
}
pub(crate) fn integer(
    args: &Value,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> AutomationResult<u64> {
    let value = match args.get(key) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| AutomationError::invalid("Expected nonnegative integer"))?,
        None => default,
    };
    if !(min..=max).contains(&value) {
        return Err(AutomationError::invalid(
            "Integer argument exceeds allowed bounds",
        ));
    }
    Ok(value)
}
pub(crate) fn check_remote_path(path: &str, roots: &[String]) -> AutomationResult<()> {
    fn safe(path: &str) -> bool {
        path.starts_with('/')
            && !path.contains('\0')
            && !path.split('/').any(|p| p == ".." || p == ".")
            && !path.contains("//")
    }
    if !safe(path) {
        return Err(AutomationError::denied_because(
            "remotePath must be an absolute normalized path without '.', '..' or '//'",
        ));
    }
    if roots.iter().any(|root| {
        safe(root)
            && (root == "/"
                || path == root
                || path
                    .strip_prefix(root.trim_end_matches('/'))
                    .map(|rest| rest.starts_with('/'))
                    .unwrap_or(false))
    }) {
        Ok(())
    } else {
        Err(outside_remote_roots(roots))
    }
}

fn validate_arguments(operation: &str, args: &Value) -> AutomationResult<()> {
    let allowed: &[&str] = match operation {
        "profile_list" => &["query", "tags", "offset", "limit"],
        "profile_get" | "test_connection" => &["profileId"],
        "execute" => &["profileId", "command", "cwd", "timeoutMs", "maxOutputBytes"],
        "list_files" => &["profileId", "remotePath", "offset", "limit"],
        "read_file" => &["profileId", "remotePath", "offset", "maxBytes"],
        "write_file" => &["profileId", "remotePath", "content", "overwrite"],
        "upload" | "download" => &[
            "profileId",
            "remotePath",
            "localPath",
            "overwrite",
            "maxBytes",
        ],
        "job_start" => &[
            "profileId",
            "command",
            "cwd",
            "runtimeLimit",
            "maxOutputBytes",
        ],
        "job_status" => &["profileId", "jobId", "offset", "maxBytes"],
        "job_list" => &["profileId", "offset", "limit", "state"],
        "job_cancel" => &["profileId", "jobId"],
        _ => return Err(AutomationError::invalid("Unknown SSH operation")),
    };
    let object = args
        .as_object()
        .ok_or_else(|| AutomationError::invalid("Arguments must be an object"))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(AutomationError::invalid("Unknown or host-only argument"));
    }
    // Check scalar/container sizes before queueing work. The transport must still
    // enforce its own frame limit before JSON allocation.
    if serde_json::to_vec(args)
        .map_err(|_| AutomationError::invalid("Invalid arguments"))?
        .len()
        > 512 * 1024
    {
        return Err(AutomationError::invalid("Arguments exceed 512KiB"));
    }
    if let Some(tags) = args.get("tags") {
        let tags = tags
            .as_array()
            .ok_or_else(|| AutomationError::invalid("tags must be an array"))?;
        if tags.len() > 100
            || tags
                .iter()
                .any(|v| v.as_str().map(|s| s.len() > 256).unwrap_or(true))
        {
            return Err(AutomationError::invalid("Invalid tags"));
        }
    }
    if let Some(query) = args.get("query") {
        if query.as_str().map(|v| v.len() > 1024).unwrap_or(true) {
            return Err(AutomationError::invalid("Invalid query"));
        }
    }
    if let Some(overwrite) = args.get("overwrite") {
        if !overwrite.is_boolean() {
            return Err(AutomationError::invalid("overwrite must be boolean"));
        }
    }
    Ok(())
}
