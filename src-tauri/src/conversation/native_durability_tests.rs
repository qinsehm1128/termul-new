use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::migration::{inventory_legacy_roots, LegacyRootConfiguration};
use super::{
    CrashInjector, CrashPoint, DurableFileSystem, HostMigrationLock, MigrationErrorCode,
    NamespaceState, OwnedTempDisposition,
};

#[derive(Debug)]
struct InterruptAt(CrashPoint);

impl CrashInjector for InterruptAt {
    fn should_interrupt(&self, point: CrashPoint) -> bool {
        point == self.0
    }
}

fn injected(point: CrashPoint) -> DurableFileSystem {
    DurableFileSystem::with_crash_injector(Arc::new(InterruptAt(point)))
}

fn generation(path: &std::path::Path) -> String {
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["generation"].as_str().unwrap().to_string()
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-15T09:45:15.123Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn native_replace_recovery_matrix() {
    let platform = std::env::consts::OS;
    assert!(matches!(platform, "linux" | "macos" | "windows"));

    for point in [
        CrashPoint::BeforeTempSync,
        CrashPoint::AfterTempSync,
        CrashPoint::AfterReplace,
        CrashPoint::AfterNamespaceSync,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("conversation-layout.json");
        fs::write(&target, br#"{"generation":"old"}"#).unwrap();
        let source_before = fs::read(&target).unwrap();
        let outcome = injected(point)
            .replace_bytes(&target, br#"{"generation":"new"}"#)
            .unwrap();

        match point {
            CrashPoint::BeforeTempSync | CrashPoint::AfterTempSync => {
                assert_eq!(outcome.namespace_state, NamespaceState::OldComplete);
                assert_eq!(generation(&target), "old");
                assert_eq!(fs::read(&target).unwrap(), source_before);
            }
            CrashPoint::AfterReplace | CrashPoint::AfterNamespaceSync => {
                assert_eq!(outcome.namespace_state, NamespaceState::NewComplete);
                assert_eq!(generation(&target), "new");
            }
            CrashPoint::AfterJsonlAppend => unreachable!(),
        }

        let restart = DurableFileSystem::new()
            .replace_bytes(&target, br#"{"generation":"recovered"}"#)
            .unwrap();
        assert_eq!(restart.namespace_state, NamespaceState::NewComplete);
        assert_eq!(restart.crash_point, CrashPoint::AfterNamespaceSync);
        assert_eq!(generation(&target), "recovered");
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            name == "conversation-layout.json" || name.ends_with(".tmp")
        }));
    }
}

#[test]
fn native_workspace_revision_and_jsonl_restart_matrix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("workspace.json");
    fs::write(&workspace, br#"{"revision":1}"#).unwrap();

    let old = injected(CrashPoint::AfterTempSync)
        .replace_bytes(&workspace, br#"{"revision":2}"#)
        .unwrap();
    assert_eq!(old.namespace_state, NamespaceState::OldComplete);
    assert_eq!(
        old.owned_temp_disposition,
        OwnedTempDisposition::RetainedForRetry
    );
    let value: Value = serde_json::from_slice(&fs::read(&workspace).unwrap()).unwrap();
    assert_eq!(value["revision"], 1);

    let new = injected(CrashPoint::AfterReplace)
        .replace_bytes(&workspace, br#"{"revision":2}"#)
        .unwrap();
    assert_eq!(new.namespace_state, NamespaceState::NewComplete);
    let value: Value = serde_json::from_slice(&fs::read(&workspace).unwrap()).unwrap();
    assert_eq!(value["revision"], 2);

    let log = root.join("messages.jsonl");
    let appended = injected(CrashPoint::AfterJsonlAppend)
        .append_jsonl(&log, br#"{"schemaVersion":2,"seq":1}"#)
        .unwrap();
    assert_eq!(appended.crash_point, CrashPoint::AfterJsonlAppend);
    assert_eq!(
        fs::read(&log).unwrap(),
        b"{\"schemaVersion\":2,\"seq\":1}\n"
    );
    DurableFileSystem::new()
        .sync_file_and_namespace(&log)
        .unwrap();
}

#[test]
fn native_kernel_lock_owner() {
    let Some(root) = std::env::var_os("TERMUL_NATIVE_LOCK_ROOT") else {
        return;
    };
    let barrier = PathBuf::from(std::env::var_os("TERMUL_NATIVE_LOCK_BARRIER").unwrap());
    let lock = HostMigrationLock::new(&PathBuf::from(root)).unwrap();
    let _guard = lock.acquire().unwrap();
    fs::write(barrier, b"locked").unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn native_kernel_lock_releases_after_forced_process_exit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let barrier = temp.path().join("native-lock-acquired");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "conversation::native_durability_tests::native_kernel_lock_owner",
            "--nocapture",
        ])
        .env("TERMUL_NATIVE_LOCK_ROOT", &root)
        .env("TERMUL_NATIVE_LOCK_BARRIER", &barrier)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !barrier.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        barrier.exists(),
        "subprocess did not acquire native kernel lock"
    );

    let contender = HostMigrationLock::new(&root).unwrap();
    assert_eq!(
        contender.acquire().unwrap_err().code,
        MigrationErrorCode::MigrationInProgress
    );
    child.kill().unwrap();
    child.wait().unwrap();

    let recovered = HostMigrationLock::new(&root).unwrap();
    let _guard = recovered
        .acquire()
        .expect("kernel must release migration lock after process death");
}

#[cfg(unix)]
#[test]
fn native_inventory_refuses_symlinked_root_component_without_reading_source() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let outside = base.join("outside");
    let sessions = outside.join("acp-sessions");
    fs::create_dir_all(&sessions).unwrap();
    let source = sessions.join("sessions.json");
    fs::write(&source, br#"{"preserved":true}"#).unwrap();
    let host_link = base.join("linked-host-state");
    symlink(&outside, &host_link).unwrap();

    let error = inventory_legacy_roots(
        &LegacyRootConfiguration {
            host_state_root: host_link,
            ..Default::default()
        },
        Uuid::new_v4(),
        fixed_time(),
        &base.join("operation"),
    )
    .unwrap_err();
    assert_eq!(error.code, MigrationErrorCode::MigrationVerificationFailed);
    assert_eq!(fs::read(source).unwrap(), br#"{"preserved":true}"#);
}

#[cfg(windows)]
#[test]
fn native_inventory_refuses_windows_junction_root_without_reading_source() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let outside = base.join("outside");
    let sessions = outside.join("acp-sessions");
    fs::create_dir_all(&sessions).unwrap();
    let source = sessions.join("sessions.json");
    fs::write(&source, br#"{"preserved":true}"#).unwrap();
    let junction = base.join("junction-host-state");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .status()
        .unwrap();
    assert!(status.success(), "junction fixture creation failed");

    let error = inventory_legacy_roots(
        &LegacyRootConfiguration {
            host_state_root: junction,
            ..Default::default()
        },
        Uuid::new_v4(),
        fixed_time(),
        &base.join("operation"),
    )
    .unwrap_err();
    assert_eq!(error.code, MigrationErrorCode::MigrationVerificationFailed);
    assert_eq!(fs::read(source).unwrap(), br#"{"preserved":true}"#);
}

#[test]
fn native_platform_claim_is_explicit_and_never_ignored() {
    #[cfg(target_os = "linux")]
    assert_eq!(std::env::consts::OS, "linux");
    #[cfg(target_os = "macos")]
    assert_eq!(std::env::consts::OS, "macos");
    #[cfg(target_os = "windows")]
    assert_eq!(std::env::consts::OS, "windows");

    let source = include_str!("durable_fs.rs");
    #[cfg(target_os = "linux")]
    {
        assert!(source.contains("File::open(parent)?.sync_all()"));
        assert!(source.contains("fs::rename(source, target)"));
    }
    #[cfg(target_os = "macos")]
    {
        assert!(source.contains("libc::F_FULLFSYNC"));
        assert!(source.contains("File::open(parent)?.sync_all()"));
    }
    #[cfg(target_os = "windows")]
    {
        assert!(source.contains("MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH"));
        assert!(source.contains("Windows has no portable parent-directory fsync"));
    }
}
