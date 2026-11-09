//! Dedicated, window-independent libssh2 automation transport.
//!
//! DNS resolution in the shared TCP helper uses the OS resolver and cannot be
//! interrupted by a socket deadline. Socket/session deadlines apply before the
//! handshake. SFTP v3 has no openat/RESOLVE_BENEATH: canonical/lstat checks reject
//! existing symlinks but cannot prevent a hostile concurrent ancestor rename.
//! Grants must therefore name directories not concurrently renamed by untrusted
//! actors. Overwrite is deliberately unsupported; creates use EXCL. POSIX job
//! cancellation targets only the generated setsid group, not escaped descendants;
//! POSIX PID/group identity checks cannot eliminate a concurrent PID-reuse race.
use super::{
    automation::{
        AutomationBackend, AutomationError, AutomationResult, Credentials, ExecRequest, ExecResult,
        ExecutionProgress, ProfileGrant,
    },
    connection::SSHConnectionManager,
    profile_manager::SSHProfile,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ssh2::{OpenFlags, OpenType, Session, Sftp};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{atomic::Ordering, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SSHProfile {
        serde_json::from_value(json!({
            "id":"test", "name":"test", "host":"invalid.example", "port":22,
            "username":"test", "authMethod":"password", "privateKeyPath":null,
            "password":null, "passphrase":null, "jumpHostId":null,
            "portForwards":[], "tags":null, "lastConnected":null, "importedFrom":null
        }))
        .unwrap()
    }

    #[test]
    fn strict_trust_accepts_only_a_readable_matching_key() {
        use ssh2::CheckResult;
        assert!(SSHConnectionManager::strict_host_key_decision(Ok(CheckResult::Match)).is_ok());
        assert!(SSHConnectionManager::strict_host_key_decision(Ok(CheckResult::NotFound)).is_err());
        assert!(SSHConnectionManager::strict_host_key_decision(Ok(CheckResult::Mismatch)).is_err());
        assert!(SSHConnectionManager::strict_host_key_decision(Ok(CheckResult::Failure)).is_err());
        assert!(SSHConnectionManager::strict_host_key_decision(Err(())).is_err());
    }

    #[test]
    fn fingerprint_tracks_profile_and_credentials() {
        let mut profile = profile();
        let first = Libssh2Backend::fingerprint(&profile, &Credentials::default());
        profile.port = 23;
        assert_ne!(
            first,
            Libssh2Backend::fingerprint(&profile, &Credentials::default())
        );
        profile.port = 22;
        assert_ne!(
            first,
            Libssh2Backend::fingerprint(
                &profile,
                &Credentials {
                    secret: Some("secret".into())
                }
            )
        );
        assert_eq!(
            first,
            Libssh2Backend::fingerprint(&profile, &Credentials::default())
        );
    }

    #[test]
    fn rejects_jump_hosts_without_network() {
        let mut profile = profile();
        profile.jump_host_id = Some("jump".into());
        let result = Libssh2Backend::new().test_connection(&profile, &Credentials::default());
        assert_eq!(result.unwrap_err().code, "unsupported_profile");
    }

    #[test]
    fn bounded_streams_share_one_budget() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut budget = 5;
        let mut truncated = false;
        append_bounded(&mut stdout, b"abc", &mut budget, &mut truncated);
        append_bounded(&mut stderr, b"def", &mut budget, &mut truncated);
        assert_eq!(stdout, b"abc");
        assert_eq!(stderr, b"de");
        assert!(truncated);
        assert_eq!(budget, 0);
    }

    #[derive(Default)]
    struct CapturedProgress(Mutex<Vec<(bool, Vec<u8>)>>);

    impl ExecutionProgress for CapturedProgress {
        fn output(&self, stdout: bool, chunk: &[u8]) {
            self.0.lock().unwrap().push((stdout, chunk.to_vec()));
        }
    }

    #[test]
    fn progress_excludes_split_managed_preamble_and_emits_each_read_once() {
        let progress = Arc::new(CapturedProgress::default());
        let mut output = ExecutionOutput::new(32, Some(progress.clone()));
        output
            .receive(true, b"TERMUL_te", Some("TERMUL_test"))
            .unwrap();
        assert!(progress.0.lock().unwrap().is_empty());
        output.receive(false, b"err", Some("TERMUL_test")).unwrap();
        output
            .receive(true, b"st=123\nhello", Some("TERMUL_test"))
            .unwrap();
        output
            .receive(true, b" world", Some("TERMUL_test"))
            .unwrap();
        assert_eq!(output.pid, Some(123));
        assert_eq!(output.stdout, b"hello world");
        assert_eq!(output.stderr, b"err");
        assert_eq!(
            *progress.0.lock().unwrap(),
            vec![
                (false, b"err".to_vec()),
                (true, b"hello".to_vec()),
                (true, b" world".to_vec())
            ]
        );
        assert_eq!(output.remaining, 18);
        assert!(output.preamble.is_empty());
    }

    #[test]
    fn progress_emits_only_retained_bytes_under_one_combined_budget() {
        let progress = Arc::new(CapturedProgress::default());
        let mut output = ExecutionOutput::new(5, Some(progress.clone()));
        output.receive(true, b"abc", None).unwrap();
        output.receive(false, b"def", None).unwrap();
        output.receive(true, b"discarded", None).unwrap();
        output.receive(false, b"", None).unwrap();
        assert_eq!(output.stdout, b"abc");
        assert_eq!(output.stderr, b"de");
        assert_eq!(output.remaining, 0);
        assert!(output.truncated);
        assert_eq!(
            *progress.0.lock().unwrap(),
            vec![(true, b"abc".to_vec()), (false, b"de".to_vec())]
        );
    }

    #[test]
    fn progress_respects_one_mib_service_budget_across_both_streams() {
        let progress = Arc::new(CapturedProgress::default());
        let maximum = 1024 * 1024;
        let mut output = ExecutionOutput::new(maximum, Some(progress.clone()));
        let chunk = vec![b'x'; 8192];
        for index in 0..130 {
            output.receive(index % 2 == 0, &chunk, None).unwrap();
        }
        let chunks = progress.0.lock().unwrap();
        assert_eq!(chunks.len(), 128);
        assert_eq!(
            chunks.iter().map(|(_, bytes)| bytes.len()).sum::<usize>(),
            maximum
        );
        for stdout in [true, false] {
            let emitted: Vec<u8> = chunks
                .iter()
                .filter(|(stream, _)| *stream == stdout)
                .flat_map(|(_, bytes)| bytes.iter().copied())
                .collect();
            assert_eq!(
                emitted,
                if stdout {
                    &output.stdout
                } else {
                    &output.stderr
                }
                .as_slice()
            );
        }
        assert_eq!(output.remaining, 0);
        assert!(output.truncated);
    }

    #[test]
    fn progress_rejects_invalid_control_frames_without_leaking_them() {
        let progress = Arc::new(CapturedProgress::default());
        let mut output = ExecutionOutput::new(10, Some(progress.clone()));
        assert!(output
            .receive(true, b"OTHER=123\nsecret", Some("TERMUL_test"))
            .is_err());
        assert!(progress.0.lock().unwrap().is_empty());
        assert!(output.stdout.is_empty());
        assert_eq!(output.remaining, 10);
    }

    #[test]
    fn progress_preserves_raw_utf8_chunks_and_works_without_callback() {
        let progress = Arc::new(CapturedProgress::default());
        let mut output = ExecutionOutput::new(3, Some(progress.clone()));
        output.receive(true, &[0xe4], None).unwrap();
        output.receive(true, &[0xb8, 0xad], None).unwrap();
        assert_eq!(output.stdout, "中".as_bytes());
        assert_eq!(
            *progress.0.lock().unwrap(),
            vec![(true, vec![0xe4]), (true, vec![0xb8, 0xad])]
        );
        let mut compatible = ExecutionOutput::new(3, None);
        compatible.receive(true, "中".as_bytes(), None).unwrap();
        assert_eq!(compatible.stdout, output.stdout);
        assert!(!compatible.truncated);
    }

    #[test]
    fn identity_parser_is_strict_and_incremental() {
        assert!(parse_preamble(b"TERMUL_test=12", "TERMUL_test")
            .unwrap()
            .is_none());
        assert_eq!(
            parse_preamble(b"TERMUL_test=123\noutput", "TERMUL_test").unwrap(),
            Some((123, 16))
        );
        for value in [
            "TERMUL_test=0\n",
            "TERMUL_test=1\n",
            "TERMUL_test=-12\n",
            "TERMUL_test=12;kill\n",
            "TERMUL_test=2147483648\n",
            "OTHER=12\n",
        ] {
            assert!(parse_preamble(value.as_bytes(), "TERMUL_test").is_err());
        }
        assert!(parse_preamble(&[b'x'; 129], "TERMUL_test").is_err());
    }

    #[test]
    fn paths_require_absolute_non_traversing_names() {
        for path in ["relative", "/safe/../escape", "/safe/./file", "/safe/\0bad"] {
            assert!(lexical_path(Path::new(path)).is_err());
        }
        assert!(lexical_path(Path::new("/safe/file")).is_ok());
        assert!(!Path::new("/safe-sibling/file").starts_with("/safe"));
    }

    #[test]
    fn file_bounds_and_overwrite_are_conservative() {
        assert_eq!(read_bounded(&b"abc"[..], 3).unwrap(), b"abc");
        assert_eq!(read_bounded(&b"abcd"[..], 3).unwrap_err().code, "too_large");
        assert!(no_overwrite(&json!({})).is_ok());
        assert_eq!(
            no_overwrite(&json!({"overwrite":true})).unwrap_err().code,
            "unsupported"
        );
        assert!(no_overwrite(&json!({"overwrite":"true"})).is_err());
        assert!(bound(&json!({"limit":-1}), "limit", 10, 100).is_err());
        assert!(bound(&json!({"limit":101}), "limit", 10, 100).is_err());
        assert_eq!(bound(&json!({}), "limit", 10, 100).unwrap(), 10);
    }

    #[test]
    fn execution_bounds_are_checked_without_network() {
        let mut request = ExecRequest {
            command: "true".into(),
            timeout_ms: 120_001,
            max_bytes: 1024,
            job_token: None,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let result =
            Libssh2Backend::new().execute(&profile(), &Credentials::default(), request.clone());
        assert_eq!(result.err().unwrap().code, "invalid_arguments");
        let result = Libssh2Backend::new().execute_with_progress(
            &profile(),
            &Credentials::default(),
            request.clone(),
            Arc::new(CapturedProgress::default()),
        );
        assert_eq!(result.err().unwrap().code, "invalid_arguments");
        request.job_token = Some("not-interpolated".into());
        request.timeout_ms = 3_600_000;
        assert!(validate_execution(&request).is_ok());
        request.timeout_ms += 1;
        assert!(validate_execution(&request).is_err());
        request.timeout_ms = 1000;
        request.cancel.store(true, Ordering::Release);
        assert_eq!(validate_execution(&request).unwrap_err().code, "cancelled");
    }

    #[test]
    fn utf8_expansion_cannot_exceed_returned_budget() {
        let mut budget = 5;
        let mut truncated = false;
        let stdout = utf8_bounded(&[255, 255], &mut budget, &mut truncated);
        let stderr = utf8_bounded(b"abcd", &mut budget, &mut truncated);
        assert_eq!(stdout.len() + stderr.len(), 5);
        assert!(truncated);
    }

    #[test]
    fn text_pages_preserve_offsets_and_utf8_boundaries() {
        let page = text_page(b"abcdef".to_vec(), 5, 3).unwrap();
        assert_eq!(page["content"], "abc");
        assert_eq!(page["nextOffset"], 8);
        assert_eq!(page["truncated"], true);
        let page = text_page("a中x".as_bytes().to_vec(), 7, 3).unwrap();
        assert_eq!(page["content"], "a");
        assert_eq!(page["nextOffset"], 8);
        assert!(text_page("中".as_bytes().to_vec(), 0, 1).is_err());
        assert!(text_page(vec![0xff], 0, 10).is_err());
        let page = text_page(vec![], 9, 4).unwrap();
        assert_eq!(page["nextOffset"], 9);
        assert_eq!(page["truncated"], false);
    }

    #[test]
    fn shell_quoting_keeps_command_in_one_argument() {
        assert_eq!(quote("a'b"), "'a'\\''b'");
        let wrapper = job_wrapper("printf '%s' \"hello\"", "TERMUL_test");
        assert!(wrapper.contains("exec setsid sh -c"));
        assert!(wrapper.contains("status=$?"));
        assert!(!wrapper.contains("kill "));
    }
}
const MAX_BYTES: usize = 8 * 1024 * 1024;
const POOL_SIZE: usize = 8;
const IO_TIMEOUT_MS: u32 = 10_000;
const MAX_SCAN: usize = 10_000;
type Slot = Arc<Mutex<Option<Session>>>;

#[derive(Default)]
pub struct Libssh2Backend {
    sessions: Mutex<HashMap<[u8; 32], Slot>>,
}

fn error(code: &str, message: &str) -> AutomationError {
    AutomationError::new(code, message)
}
fn transport<T>(_: T) -> AutomationError {
    AutomationError::backend()
}
fn invalid(message: &str) -> AutomationError {
    AutomationError::invalid(message)
}

impl Libssh2Backend {
    pub fn new() -> Self {
        Self::default()
    }

    fn fingerprint(profile: &SSHProfile, credentials: &Credentials) -> [u8; 32] {
        // Hash all profile fields (including transient credentials) and the
        // independently resolved secret; never retain/log the serialized bytes.
        let mut hash = Sha256::new();
        let bytes = serde_json::to_vec(profile).unwrap_or_default();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
        hash.update([u8::from(credentials.secret.is_some())]);
        if let Some(secret) = &credentials.secret {
            hash.update(secret.as_bytes());
        }
        hash.finalize().into()
    }

    fn connect(profile: &SSHProfile, credentials: &Credentials) -> AutomationResult<Session> {
        if profile.jump_host_id.is_some() {
            return Err(error(
                "unsupported_profile",
                "Jump hosts are not supported for automation",
            ));
        }
        let tcp =
            SSHConnectionManager::connect_tcp(&profile.host, profile.port).map_err(transport)?;
        let timeout = Some(Duration::from_millis(IO_TIMEOUT_MS.into()));
        tcp.set_read_timeout(timeout).map_err(transport)?;
        tcp.set_write_timeout(timeout).map_err(transport)?;
        let mut session = Session::new().map_err(transport)?;
        session.set_timeout(IO_TIMEOUT_MS);
        session.set_tcp_stream(tcp);
        session.handshake().map_err(transport)?;
        SSHConnectionManager::verify_host_key_strict(&session, &profile.host, profile.port)
            .map_err(|_| error("host_key_error", "SSH host verification failed"))?;
        SSHConnectionManager::authenticate_session(
            &session,
            profile,
            credentials.secret.as_deref(),
        )
        .map_err(|_| error("authentication_failed", "SSH authentication failed"))?;
        Ok(session)
    }

    fn with_session<T>(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        work: impl FnOnce(&Session) -> AutomationResult<T>,
    ) -> AutomationResult<T> {
        if profile.jump_host_id.is_some() {
            return Err(error(
                "unsupported_profile",
                "Jump hosts are not supported for automation",
            ));
        }
        let key = Self::fingerprint(profile, credentials);
        let slot = {
            let mut pool = self.sessions.lock().map_err(transport)?;
            if let Some(slot) = pool.get(&key) {
                slot.clone()
            } else {
                if pool.len() >= POOL_SIZE {
                    let idle = pool
                        .iter()
                        .find(|(_, s)| Arc::strong_count(s) == 1)
                        .map(|(k, _)| *k);
                    if let Some(idle) = idle {
                        pool.remove(&idle);
                    } else {
                        return Err(error("busy", "Automation connection pool is busy"));
                    }
                }
                let slot = Arc::new(Mutex::new(None));
                pool.insert(key, slot.clone());
                slot
            }
        };
        // Connect under the slot lock: concurrent first callers cannot duplicate
        // authentication. No UI sessions, heartbeats, reconnects or retries.
        // Fail fast instead of retaining a stale grant in a queue behind an
        // hour-long job. The host may submit a new independently authorized call;
        // this backend never retries/replays side effects on its own.
        let mut guard = slot
            .try_lock()
            .map_err(|_| error("busy", "Automation profile connection is in use"))?;
        if guard.is_none() {
            *guard = Some(Self::connect(profile, credentials)?);
        }
        let session = guard.as_ref().ok_or_else(AutomationError::backend)?;
        session.set_blocking(true);
        session.set_timeout(IO_TIMEOUT_MS);
        // Trust revocation/unreadable files must also fail closed on reused
        // transports, not only at initial authentication.
        let result =
            SSHConnectionManager::verify_host_key_strict(session, &profile.host, profile.port)
                .map_err(|_| error("host_key_error", "SSH host verification failed"))
                .and_then(|_| work(session));
        session.set_blocking(true);
        session.set_timeout(IO_TIMEOUT_MS);
        if result.is_err() {
            *guard = None;
        }
        result
    }
}

fn validate_execution(request: &ExecRequest) -> AutomationResult<()> {
    let maximum_ms = if request.job_token.is_some() {
        3_600_000
    } else {
        120_000
    };
    if request.timeout_ms == 0
        || request.timeout_ms > maximum_ms
        || request.max_bytes == 0
        || request.max_bytes > MAX_BYTES
    {
        return Err(invalid("Execution bounds are invalid"));
    }
    if request.command.contains('\0') {
        return Err(invalid("Command contains a NUL byte"));
    }
    if request.cancel.load(Ordering::Acquire) {
        return Err(error("cancelled", "Operation cancelled before execution"));
    }
    Ok(())
}

/// Retains a combined stdout/stderr budget; control framing never reaches callbacks.
struct ExecutionOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    preamble: Vec<u8>,
    pid: Option<u32>,
    remaining: usize,
    truncated: bool,
    progress: Option<Arc<dyn ExecutionProgress>>,
}

impl ExecutionOutput {
    fn new(maximum: usize, progress: Option<Arc<dyn ExecutionProgress>>) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            preamble: Vec::new(),
            pid: None,
            remaining: maximum,
            truncated: false,
            progress,
        }
    }

    fn receive(
        &mut self,
        stdout: bool,
        bytes: &[u8],
        marker: Option<&str>,
    ) -> AutomationResult<()> {
        if let Some(marker) = marker.filter(|_| stdout && self.pid.is_none()) {
            self.preamble.extend_from_slice(bytes);
            if let Some((pid, start)) = parse_preamble(&self.preamble, marker)? {
                self.pid = Some(pid);
                let framed = std::mem::take(&mut self.preamble);
                self.retain(true, &framed[start..]);
            }
        } else {
            self.retain(stdout, bytes);
        }
        Ok(())
    }

    fn retain(&mut self, stdout: bool, bytes: &[u8]) {
        let count = bytes.len().min(self.remaining);
        let target = if stdout {
            &mut self.stdout
        } else {
            &mut self.stderr
        };
        append_bounded(target, bytes, &mut self.remaining, &mut self.truncated);
        if count > 0 {
            if let Some(progress) = &self.progress {
                progress.output(stdout, &bytes[..count]);
            }
        }
    }
}

fn utf8_bounded(bytes: &[u8], budget: &mut usize, truncated: &mut bool) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut end = text.len().min(*budget);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    *truncated |= end < text.len();
    text.truncate(end);
    *budget -= end;
    text
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn job_wrapper(command: &str, marker: &str) -> String {
    // The token is generated here, NOT interpolated from caller job_token.
    let body = format!(
        "pg=$(ps -o pgid= -p $$) || exit 125; set -- $pg; [ \"$1\" = \"$$\" ] || exit 125; printf '{}=%s\\n' \"$$\"; sh -c {}; status=$?; exit \"$status\"",
        marker, quote(command));
    format!("command -v setsid >/dev/null 2>&1 && command -v ps >/dev/null 2>&1 || exit 125; exec setsid sh -c {} {}", quote(&body), quote(marker))
}

fn parse_preamble(data: &[u8], marker: &str) -> AutomationResult<Option<(u32, usize)>> {
    let Some(end) = data.iter().position(|b| *b == b'\n') else {
        if data.len() > 128 {
            return Err(error(
                "unsupported",
                "Isolated POSIX job could not be established",
            ));
        }
        return Ok(None);
    };
    let prefix = format!("{}=", marker);
    let text = std::str::from_utf8(&data[..end])
        .map_err(|_| error("unsupported", "Invalid job identity"))?;
    let digits = text
        .strip_prefix(&prefix)
        .ok_or_else(|| error("unsupported", "Invalid job identity"))?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error("unsupported", "Invalid job identity"));
    }
    let pid: u32 = digits
        .parse()
        .map_err(|_| error("unsupported", "Invalid job identity"))?;
    if pid <= 1 || pid > i32::MAX as u32 {
        return Err(error("unsupported", "Invalid job identity"));
    }
    Ok(Some((pid, end + 1)))
}

fn append_bounded(dst: &mut Vec<u8>, data: &[u8], budget: &mut usize, truncated: &mut bool) {
    let count = data.len().min(*budget);
    dst.extend_from_slice(&data[..count]);
    *budget -= count;
    *truncated |= count < data.len();
}

fn string_arg<'a>(args: &'a Value, key: &str) -> AutomationResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.contains('\0'))
        .ok_or_else(|| invalid("A required path or text argument is invalid"))
}
fn bound(args: &Value, key: &str, default: usize, maximum: usize) -> AutomationResult<usize> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|n| *n <= maximum as u64)
            .map(|n| n as usize)
            .ok_or_else(|| invalid("A numeric bound is invalid")),
    }
}
fn lexical_path(path: &Path) -> AutomationResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path
            .to_str()
            .is_none_or(|s| s.contains('\0') || s.split('/').any(|p| p == "." || p == ".."))
    {
        return Err(invalid("An absolute path without traversal is required"));
    }
    Ok(())
}
fn remote_no_links(sftp: &Sftp, path: &Path) -> AutomationResult<()> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        let stat = sftp.lstat(&prefix).map_err(transport)?;
        if stat.file_type().is_symlink() {
            return Err(error("permission_denied", "Symbolic links are not allowed"));
        }
    }
    Ok(())
}
fn remote_path(
    sftp: &Sftp,
    value: &str,
    grant: &ProfileGrant,
    create: bool,
) -> AutomationResult<PathBuf> {
    let path = Path::new(value);
    lexical_path(path)?;
    let check = if create {
        path.parent()
            .filter(|_| path.file_name().is_some())
            .ok_or_else(|| invalid("A file path is required"))?
    } else {
        path
    };
    remote_no_links(sftp, check)?;
    let canonical = sftp.realpath(check).map_err(transport)?;
    let mut allowed = false;
    for root in &grant.remote_roots {
        let root = Path::new(root);
        lexical_path(root)?;
        remote_no_links(sftp, root)?;
        let root = sftp.realpath(root).map_err(transport)?;
        if !sftp.stat(&root).map_err(transport)?.is_dir() {
            return Err(AutomationError::denied_because(
                "An authorized remote root is not a directory on the SSH target",
            ));
        }
        allowed |= canonical.starts_with(root);
    }
    if !allowed {
        return Err(super::automation::outside_remote_roots(&grant.remote_roots));
    }
    if create {
        let result = canonical.join(
            path.file_name()
                .ok_or_else(|| invalid("A file path is required"))?,
        );
        match sftp.lstat(&result) {
            Ok(_) => Err(error("already_exists", "Destination already exists")),
            Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => Ok(result),
            Err(_) => Err(AutomationError::backend()),
        }
    } else {
        Ok(canonical)
    }
}
fn local_no_links(path: &Path) -> AutomationResult<()> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        if fs::symlink_metadata(&prefix)
            .map_err(|_| error("local_io_error", "Local path could not be inspected"))?
            .file_type()
            .is_symlink()
        {
            return Err(error("permission_denied", "Symbolic links are not allowed"));
        }
    }
    Ok(())
}
fn local_path(value: &str, grant: &ProfileGrant, create: bool) -> AutomationResult<PathBuf> {
    let path = Path::new(value);
    lexical_path(path)?;
    let check = if create {
        path.parent()
            .filter(|_| path.file_name().is_some())
            .ok_or_else(|| invalid("A file path is required"))?
    } else {
        path
    };
    local_no_links(check)?;
    let canonical = fs::canonicalize(check)
        .map_err(|_| error("local_io_error", "Local path could not be resolved"))?;
    let mut allowed = false;
    for root in &grant.local_roots {
        lexical_path(root)?;
        local_no_links(root)?;
        let root = fs::canonicalize(root)
            .map_err(|_| error("local_io_error", "Local grant root could not be resolved"))?;
        if !root.is_dir() {
            return Err(AutomationError::denied_because(
                "An authorized Se host local root is not a directory",
            ));
        }
        allowed |= canonical.starts_with(root);
    }
    if !allowed {
        return Err(if grant.local_roots.is_empty() {
            super::automation::no_local_roots()
        } else {
            AutomationError::denied_because(
                "localPath is outside the authorized Se host local roots (see profile_get localRoots)",
            )
        });
    }
    if create {
        Ok(canonical.join(
            path.file_name()
                .ok_or_else(|| invalid("A file path is required"))?,
        ))
    } else {
        Ok(canonical)
    }
}
fn text_page(mut bytes: Vec<u8>, offset: usize, maximum: usize) -> AutomationResult<Value> {
    let mut truncated = bytes.len() > maximum;
    bytes.truncate(maximum);
    let valid = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() && truncated => e.valid_up_to(),
        Err(_) => {
            return Err(error(
                "invalid_encoding",
                "File page must start at a UTF-8 boundary and contain UTF-8 text",
            ))
        }
    };
    if valid == 0 && !bytes.is_empty() {
        return Err(invalid(
            "maxBytes is too small for the next UTF-8 character",
        ));
    }
    truncated |= valid < bytes.len();
    bytes.truncate(valid);
    let content = String::from_utf8(bytes)
        .map_err(|_| error("invalid_encoding", "File is not UTF-8 text"))?;
    Ok(
        json!({"bytesRead": content.len(), "nextOffset": offset + content.len(), "offset": offset, "content": content, "truncated": truncated}),
    )
}

fn read_bounded(reader: impl Read, maximum: usize) -> AutomationResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(transport)?;
    if bytes.len() > maximum {
        return Err(error("too_large", "File exceeds the requested byte limit"));
    }
    Ok(bytes)
}
fn no_overwrite(args: &Value) -> AutomationResult<()> {
    match args.get("overwrite") {
        None | Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) => Err(error("unsupported", "Safe overwrite is not supported")),
        _ => Err(invalid("overwrite must be a boolean")),
    }
}
fn new_remote(sftp: &Sftp, path: &Path) -> AutomationResult<ssh2::File> {
    sftp.open_mode(
        path,
        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
        0o600,
        OpenType::File,
    )
    .map_err(|_| error("create_failed", "Exclusive destination creation failed"))
}
fn open_local(path: &Path, create: bool) -> AutomationResult<fs::File> {
    let mut options = OpenOptions::new();
    options.read(!create).write(create).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600);
    }
    // Windows has no portable no-follow open through std::fs; refuse transfers
    // rather than silently weaken the local-path contract.
    #[cfg(not(unix))]
    {
        return Err(error(
            "unsupported",
            "Safe local transfers are unavailable on this platform",
        ));
    }
    #[cfg(unix)]
    {
        let file = options
            .open(path)
            .map_err(|_| error("local_io_error", "Safe local file open failed"))?;
        if !file.metadata().map_err(transport)?.is_file() {
            return Err(invalid("Only regular files are supported"));
        }
        Ok(file)
    }
}

impl AutomationBackend for Libssh2Backend {
    fn execute(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
    ) -> AutomationResult<ExecResult> {
        validate_execution(&request)?;
        self.with_session(profile, credentials, |session| {
            Self::run(session, profile, credentials, request, None)
        })
    }
    fn execute_with_progress(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
        progress: Arc<dyn ExecutionProgress>,
    ) -> AutomationResult<ExecResult> {
        validate_execution(&request)?;
        self.with_session(profile, credentials, |session| {
            Self::run(session, profile, credentials, request, Some(progress))
        })
    }
    fn test_connection(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
    ) -> AutomationResult<Value> {
        self.with_session(profile, credentials, |session| {
            // A new channel roundtrip tests an already pooled transport too.
            let mut channel = session.channel_session().map_err(transport)?;
            channel.close().map_err(transport)?;
            channel.wait_close().map_err(transport)?;
            Ok(json!({"connected": true}))
        })
    }
    fn files(
        &self,
        profile: &SSHProfile,
        credentials: &Credentials,
        operation: &str,
        args: &Value,
        grant: &ProfileGrant,
    ) -> AutomationResult<Value> {
        if ![
            "list_files",
            "read_file",
            "write_file",
            "upload",
            "download",
        ]
        .contains(&operation)
        {
            return Err(invalid("Unknown file operation"));
        }
        if args.get("cwd").is_some() {
            return Err(error(
                "unsupported",
                "Working directory overrides are not supported",
            ));
        }
        no_overwrite(args)?;
        let byte_cap = if operation == "read_file" {
            256 * 1024
        } else {
            MAX_BYTES
        };
        let maximum = bound(args, "maxBytes", byte_cap, byte_cap)?;
        if maximum == 0 {
            return Err(invalid("maxBytes must be positive"));
        }
        let transfer = operation == "upload" || operation == "download";
        let remote = string_arg(
            args,
            if transfer && args.get("remotePath").is_some() {
                "remotePath"
            } else {
                "path"
            },
        )?;
        // Validate all local paths and buffer upload data before touching remote
        // destinations. Never accept recursive operations or arbitrary local paths.
        let local = if transfer {
            Some(local_path(
                string_arg(args, "localPath")?,
                grant,
                operation == "download",
            )?)
        } else {
            None
        };
        let upload = if operation == "upload" {
            let file = open_local(
                local
                    .as_deref()
                    .ok_or_else(|| invalid("localPath is required"))?,
                false,
            )?;
            if file.metadata().map_err(transport)?.len() > maximum as u64 {
                return Err(error("too_large", "File exceeds the requested byte limit"));
            }
            Some(read_bounded(file, maximum)?)
        } else {
            None
        };
        let content = if operation == "write_file" {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("content must be UTF-8 text"))?;
            if content.len() > maximum {
                return Err(error(
                    "too_large",
                    "Content exceeds the requested byte limit",
                ));
            }
            Some(content.as_bytes())
        } else {
            None
        };
        self.with_session(profile, credentials, |session| {
            let sftp = session.sftp().map_err(transport)?;
            let path = remote_path(&sftp, remote, grant, operation == "write_file" || operation == "upload")?;
            if operation == "list_files" {
                if !sftp.stat(&path).map_err(transport)?.is_dir() { return Err(invalid("A directory is required")); }
                let offset = bound(args, "offset", 0, MAX_SCAN)?;
                let limit = bound(args, "limit", 100, 1000)?;
                if limit == 0 { return Err(invalid("limit must be positive")); }
                let mut directory = sftp.opendir(&path).map_err(transport)?;
                let mut entries = Vec::new();
                let mut scanned = 0;
                let mut eof = false;
                while scanned < MAX_SCAN && entries.len() < limit {
                    let (name, stat) = match directory.readdir() {
                        Ok(entry) => entry,
                        Err(e) if e.code() == ssh2::ErrorCode::SFTP(1) => { eof = true; break; },
                        Err(_) => return Err(AutomationError::backend()),
                    };
                    scanned += 1;
                    if scanned <= offset || name == Path::new(".") || name == Path::new("..") { continue; }
                    let name = name.to_str().ok_or_else(|| error("invalid_encoding", "Directory contains a non-UTF-8 name"))?;
                    entries.push(json!({"name": name, "size": stat.size, "isDirectory": stat.is_dir(), "isSymlink": stat.file_type().is_symlink()}));
                }
                return Ok(json!({"entries": entries, "offset": offset, "nextOffset": if eof || scanned == MAX_SCAN { None } else { Some(scanned) }, "scanLimited": scanned == MAX_SCAN}));
            }
            if operation == "write_file" || operation == "upload" {
                let bytes = content.or(upload.as_deref()).ok_or_else(|| invalid("Missing file content"))?;
                let mut file = new_remote(&sftp, &path)?;
                // No replay, rollback or unlink on errors: partial exclusive
                // files remain visible rather than risking a race deleting a
                // different actor's replacement file.
                file.write_all(bytes).map_err(transport)?;
                file.close().map_err(transport)?;
                return Ok(json!({"bytesWritten": bytes.len()}));
            }
            let stat = sftp.stat(&path).map_err(transport)?;
            if !stat.is_file() { return Err(invalid("Only regular files are supported")); }
            if operation != "read_file" && stat.size.is_some_and(|size| size > maximum as u64) { return Err(error("too_large", "File exceeds the requested byte limit")); }
            let mut file = sftp.open(&path).map_err(transport)?;
            if !file.stat().map_err(transport)?.is_file() { return Err(invalid("Only regular files are supported")); }
            if operation == "read_file" {
                let offset = bound(args, "offset", 0, MAX_BYTES)?;
                std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset as u64)).map_err(transport)?;
                let mut bytes = Vec::new();
                file.take((maximum + 1) as u64).read_to_end(&mut bytes).map_err(transport)?;
                text_page(bytes, offset, maximum)
            } else {
                let bytes = read_bounded(file, maximum)?;
                let mut file = open_local(local.as_deref().ok_or_else(|| invalid("localPath is required"))?, true)?;
                file.write_all(&bytes).map_err(|_| error("local_io_error", "Local write failed; a partial file may remain"))?;
                file.sync_all().map_err(|_| error("local_io_error", "Local file flush failed"))?;
                Ok(json!({"bytesWritten": bytes.len()}))
            }
        })
    }
}
impl Libssh2Backend {
    fn terminate_group(
        profile: &SSHProfile,
        credentials: &Credentials,
        pid: u32,
        marker: &str,
    ) -> bool {
        // A separate transport avoids blocking behind the active session's
        // channel and never retries the user's command. Marker AND pgid must
        // match before any signal. Never signal a bare PID or another group.
        let result = (|| -> AutomationResult<bool> {
            let session = Self::connect(profile, credentials)?;
            let mut channel = session.channel_session().map_err(transport)?;
            let command = format!(
                "p={pid}; pg=$(ps -o pgid= -p \"$p\") || exit 1; set -- $pg; [ \"$1\" = \"$p\" ] || exit 1; a=$(ps -o args= -p \"$p\") || exit 1; case \"$a\" in *{marker}*) ;; *) exit 1;; esac; kill -TERM -\"$p\" 2>/dev/null || exit 1; n=0; while kill -0 -\"$p\" 2>/dev/null; do n=$((n+1)); [ \"$n\" -lt 20 ] || break; sleep 0.1; done; if kill -0 -\"$p\" 2>/dev/null; then kill -KILL -\"$p\" 2>/dev/null || exit 1; fi; n=0; while kill -0 -\"$p\" 2>/dev/null; do n=$((n+1)); [ \"$n\" -lt 20 ] || exit 1; sleep 0.1; done; printf 'TERMUL_GROUP_GONE\\n'"
            );
            channel.exec(&command).map_err(transport)?;
            session.set_blocking(false);
            let started = Instant::now();
            let mut out = Vec::new();
            let mut buffer = [0; 256];
            while !channel.eof() && started.elapsed() < Duration::from_secs(8) {
                match channel.read(&mut buffer) {
                    Ok(n) => {
                        if out.len() + n > 1024 {
                            return Ok(false);
                        }
                        out.extend_from_slice(&buffer[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Ok(false),
                }
                thread::sleep(Duration::from_millis(10));
            }
            // Drain final buffered output even if EOF has just arrived.
            while let Ok(n) = channel.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                if out.len() + n > 1024 {
                    return Ok(false);
                }
                out.extend_from_slice(&buffer[..n]);
            }
            Ok(channel.eof()
                && out == b"TERMUL_GROUP_GONE\n"
                && channel.exit_status().ok() == Some(0))
        })();
        result.unwrap_or(false)
    }

    fn run(
        session: &Session,
        profile: &SSHProfile,
        credentials: &Credentials,
        request: ExecRequest,
        progress: Option<Arc<dyn ExecutionProgress>>,
    ) -> AutomationResult<ExecResult> {
        validate_execution(&request)?;
        let start = Instant::now();
        let marker = format!("TERMUL_{}", uuid::Uuid::new_v4().simple());
        let managed = request.job_token.is_some();
        if request.cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Operation cancelled before execution"));
        }
        let command = if managed {
            job_wrapper(&request.command, &marker)
        } else {
            request.command.clone()
        };
        session.set_timeout(request.timeout_ms.min(IO_TIMEOUT_MS.into()) as u32);
        let mut channel = session.channel_session().map_err(transport)?;
        channel.exec(&command).map_err(transport)?;
        session.set_blocking(false);
        let mut output = ExecutionOutput::new(request.max_bytes, progress);
        let mut timed_out = false;
        let mut cancelled = false;
        let mut confirmed = false;
        let mut closed = false;
        let mut buffer = [0; 8192];
        loop {
            // Always service BOTH streams, even once the output budget is full.
            let mut progress = false;
            for stream in 0..2 {
                let read = if stream == 0 {
                    channel.read(&mut buffer)
                } else {
                    channel.stderr().read(&mut buffer)
                };
                match read {
                    Ok(n) if n > 0 => {
                        progress = true;
                        output.receive(
                            stream == 0,
                            &buffer[..n],
                            managed.then_some(marker.as_str()),
                        )?;
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(AutomationError::backend()),
                }
            }
            if channel.eof() && !progress {
                match channel.wait_close() {
                    Ok(()) => {
                        closed = true;
                        break;
                    }
                    Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                    Err(_) => return Err(AutomationError::backend()),
                }
            }
            cancelled = request.cancel.load(Ordering::Acquire);
            timed_out = start.elapsed() >= Duration::from_millis(request.timeout_ms);
            if cancelled || timed_out {
                if let Some(pid) = output.pid {
                    confirmed = Self::terminate_group(profile, credentials, pid, &marker);
                }
                break;
            }
            if !progress {
                thread::sleep(Duration::from_millis(5));
            }
        }
        if managed && output.pid.is_none() && !timed_out && !cancelled {
            return Err(error(
                "unsupported",
                "POSIX shell and setsid job isolation are required",
            ));
        }
        let exited = closed;
        let exit_code = if exited {
            channel.exit_status().ok()
        } else {
            None
        };
        // Nonblocking close is advisory only, never evidence of remote death.
        let _ = channel.close();
        let mut output_budget = request.max_bytes;
        let stdout = utf8_bounded(&output.stdout, &mut output_budget, &mut output.truncated);
        let stderr = utf8_bounded(&output.stderr, &mut output_budget, &mut output.truncated);
        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
            duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            truncated: output.truncated,
            timed_out,
            cancel_confirmed: cancelled && confirmed,
            remote_termination_confirmed: confirmed || (exited && !managed),
        })
    }
}
