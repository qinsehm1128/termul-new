//! Cross-process kernel lock coverage for migration-maintenance.json.
//!
//! Two schedulers must produce exactly one durable pending request or a
//! pending/idempotency conflict. Silent last-writer-wins is forbidden.

use std::path::Path;
use std::process::{Command, Stdio};

use chrono::{TimeZone, Utc};
use se_manager_lib::conversation::migration::{
    ConversationMigrationControlService, MigrationErrorCode, MigrationMaintenanceAction,
    MigrationMaintenanceRequestV1,
};
use uuid::Uuid;

const CHILD_ENV: &str = "SE_MAINT_LOCK_CHILD";
const ROOT_ENV: &str = "SE_MAINT_LOCK_ROOT";
const REQUEST_ID_ENV: &str = "SE_MAINT_LOCK_REQUEST_ID";

fn at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap()
}

fn request_with_id(request_id: String) -> MigrationMaintenanceRequestV1 {
    MigrationMaintenanceRequestV1 {
        action: MigrationMaintenanceAction::Rollback,
        request_id,
        requested_at_utc: at(),
        approval_receipt: None,
    }
}

fn run_child_request() -> i32 {
    let root = std::env::var(ROOT_ENV).expect("child root");
    let request_id = std::env::var(REQUEST_ID_ENV).expect("child request id");
    let service = ConversationMigrationControlService::new(Path::new(&root)).unwrap();
    match service.request(request_with_id(request_id)) {
        Ok(receipt) => {
            println!(
                "CHILD_OK already_scheduled={} request_id={}",
                receipt.already_scheduled, receipt.request_id
            );
            0
        }
        Err(error) => {
            println!("CHILD_ERR code={}", error.code.as_str());
            1
        }
    }
}

fn spawn_child(root: &Path, request_id: &str, test_name: &str) -> std::process::Output {
    Command::new(std::env::current_exe().expect("current test exe"))
        .env(CHILD_ENV, "1")
        .env(ROOT_ENV, root)
        .env(REQUEST_ID_ENV, request_id)
        .args(["--exact", test_name, "--nocapture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn maintenance lock child")
}

#[test]
fn migration_control_request_serializes_across_processes() {
    if std::env::var_os(CHILD_ENV).is_some() {
        std::process::exit(run_child_request());
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let parent_id = Uuid::new_v4().to_string();
    let child_id = Uuid::new_v4().to_string();
    let child = spawn_child(
        &root,
        &child_id,
        "migration_control_request_serializes_across_processes",
    );
    let service = ConversationMigrationControlService::new(&root).unwrap();
    let parent_result = service.request(request_with_id(parent_id.clone()));
    let pending = service
        .pending()
        .expect("pending readable after race")
        .expect("exactly one durable pending request");
    let winners = [parent_id.as_str(), child_id.as_str()];
    assert!(
        winners.contains(&pending.request_id.as_str()),
        "pending request_id must be one of the two racers"
    );

    match parent_result {
        Ok(receipt) => {
            assert_eq!(receipt.request_id, parent_id);
            assert_eq!(pending.request_id, parent_id);
            let child_stdout = String::from_utf8_lossy(&child.stdout);
            assert!(
                child_stdout.contains("CHILD_ERR code=MIGRATION_RESTART_REQUIRED")
                    || child_stdout.contains("CHILD_ERR code=MIGRATION_IDEMPOTENCY_CONFLICT")
                    || child_stdout.contains("CHILD_ERR code=MIGRATION_IN_PROGRESS"),
                "losing child must see a conflict, got: {child_stdout}"
            );
        }
        Err(error) => {
            assert!(
                matches!(
                    error.code,
                    MigrationErrorCode::MigrationRestartRequired
                        | MigrationErrorCode::MigrationIdempotencyConflict
                        | MigrationErrorCode::MigrationInProgress
                ),
                "losing parent must see a conflict, got {}",
                error.code.as_str()
            );
            assert_eq!(pending.request_id, child_id);
            let child_stdout = String::from_utf8_lossy(&child.stdout);
            assert!(
                child_stdout.contains("CHILD_OK"),
                "winning child must persist, got: {child_stdout}"
            );
        }
    }

    let decoded: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            root.join("conversation-migrations")
                .join(se_manager_lib::conversation::migration::MIGRATION_MAINTENANCE_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    let durable_id = decoded["pending"]["requestId"].as_str().unwrap();
    assert_eq!(durable_id, pending.request_id);
}

#[test]
fn migration_control_second_process_sees_pending_conflict() {
    if std::env::var_os(CHILD_ENV).is_some() {
        std::process::exit(run_child_request());
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let parent_id = Uuid::new_v4().to_string();
    let child_id = Uuid::new_v4().to_string();
    let service = ConversationMigrationControlService::new(&root).unwrap();
    let first = service.request(request_with_id(parent_id.clone())).unwrap();
    assert!(!first.already_scheduled);
    assert_eq!(first.request_id, parent_id);

    let child = spawn_child(
        &root,
        &child_id,
        "migration_control_second_process_sees_pending_conflict",
    );
    let child_stdout = String::from_utf8_lossy(&child.stdout);
    assert!(
        child_stdout.contains("CHILD_ERR code=MIGRATION_RESTART_REQUIRED")
            || child_stdout.contains("CHILD_ERR code=MIGRATION_IDEMPOTENCY_CONFLICT"),
        "second process must see pending/idempotency conflict, got: {child_stdout}"
    );
    let pending = service.pending().unwrap().unwrap();
    assert_eq!(pending.request_id, parent_id);
    assert_ne!(pending.request_id, child_id);
}
