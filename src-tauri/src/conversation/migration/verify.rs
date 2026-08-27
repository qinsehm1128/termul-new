//! Verification gate for staged legacy Conversations.
//!
//! Verification re-hashes every pre-inventory source, validates one-to-one durable mappings,
//! checks repository materialization/count/provenance receipts, and rebuilds the disposable catalog
//! twice to prove deterministic equality before cutover is allowed.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::inventory::{hash_legacy_file_no_follow, LegacyInventoryV1};
use super::legacy::{
    load_migration_map, load_staged_manifest, stage_digest, stage_output_digest, MigrationMapV1,
    StageReceiptV1, StagedManifestV1,
};
use super::recovery::{RecoveryQueueV1, RECOVERY_ITEMS_FILE, RECOVERY_QUEUE_SCHEMA_VERSION};
use super::{MigrationError, MigrationErrorCode, Result};
use crate::conversation::catalog::{rebuild_catalog, CATALOG_FILE};
use crate::conversation::durable_fs::DurableFileSystem;
use crate::conversation::locator::ConversationLocator;
use crate::conversation::repository::ConversationRepository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationReportV1 {
    pub operation_id: uuid::Uuid,
    pub source_file_count: usize,
    pub mapped_source_count: usize,
    pub staged_conversation_count: usize,
    pub message_count: u64,
    pub tool_count: u64,
    pub source_snapshot_sha256: String,
    pub catalog_sha256: String,
    pub validation_sha256: String,
}

pub fn verify_staged_layout(
    host_state_root: &Path,
    operation_dir: &Path,
    inventory: &LegacyInventoryV1,
) -> Result<VerificationReportV1> {
    verify_source_snapshot(inventory)?;
    let map = load_migration_map(operation_dir)?.ok_or_else(|| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "verify_staged_layout",
            "migration map is missing",
        )
    })?;
    let staged = load_staged_manifest(operation_dir)?;
    let recovery_queue = load_recovery_queue(operation_dir)?;
    verify_map(inventory, &map)?;
    verify_staged_manifest(inventory, &map, &recovery_queue, &staged)?;

    let repository_root = host_state_root.join("conversations").join("v2");
    let (repository, open_report) =
        ConversationRepository::open_staging(repository_root.clone()).map_err(repository_error)?;
    if open_report
        .recovery_items
        .iter()
        .any(|item| item.requires_action)
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_staged_layout",
            "canonical repository contains actionable recovery records",
        ));
    }

    let staged_ids = staged
        .receipts
        .iter()
        .map(|receipt| receipt.conversation_id)
        .collect::<HashSet<_>>();
    let mut message_count = 0_u64;
    let mut tool_count = 0_u64;
    for receipt in &staged.receipts {
        verify_receipt(&repository, inventory, receipt)?;
        message_count += receipt.message_count;
        tool_count += receipt.tool_count;
    }
    let repository_ids = repository
        .list_conversations()
        .into_iter()
        .map(|record| record.conversation_id)
        .collect::<HashSet<_>>();
    if !staged_ids.is_subset(&repository_ids) {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_staged_layout",
            "one or more staged Conversations are absent from the repository",
        ));
    }

    let locator = ConversationLocator::new(repository_root.clone()).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_catalog",
            error.to_string(),
        )
    })?;
    let durable_fs = DurableFileSystem::new();
    let first = rebuild_catalog(&locator, &durable_fs).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_catalog",
            error.to_string(),
        )
    })?;
    let first_bytes = first.catalog.deterministic_bytes();
    let second = rebuild_catalog(&locator, &durable_fs).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_catalog",
            error.to_string(),
        )
    })?;
    if first_bytes != second.catalog.deterministic_bytes() {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_catalog",
            "catalog rebuild is not byte deterministic",
        ));
    }
    if let Ok(on_disk) = fs::read(repository_root.join(CATALOG_FILE)) {
        if on_disk != first_bytes {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "verify_catalog",
                "on-disk catalog differs from authoritative rebuild",
            ));
        }
    }

    let source_file_count = inventory.roots.iter().map(|root| root.files.len()).sum();
    let source_snapshot_sha256 = source_snapshot_digest(inventory)?;
    let catalog_sha256 = sha256_bytes(&first_bytes);
    let validation_sha256 = validation_digest(
        inventory,
        &map,
        &staged,
        &source_snapshot_sha256,
        &catalog_sha256,
    )?;
    let report = VerificationReportV1 {
        operation_id: inventory.operation_id,
        source_file_count,
        mapped_source_count: map.entries.len(),
        staged_conversation_count: staged.receipts.len(),
        message_count,
        tool_count,
        source_snapshot_sha256,
        catalog_sha256,
        validation_sha256,
    };
    log::info!(
        "[conversation-migration] verification complete operation_id={} source_file_count={} mapped_source_count={} staged_count={} message_count={} tool_count={} validation_digest={}",
        report.operation_id,
        report.source_file_count,
        report.mapped_source_count,
        report.staged_conversation_count,
        report.message_count,
        report.tool_count,
        digest_prefix(&report.validation_sha256)
    );
    Ok(report)
}

pub fn verify_source_snapshot(inventory: &LegacyInventoryV1) -> Result<()> {
    for root in &inventory.roots {
        let root_path = PathBuf::from(&root.canonical_path);
        for expected in &root.files {
            let relative = PathBuf::from(
                expected
                    .relative_path
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            );
            let (size, sha256) =
                hash_legacy_file_no_follow(&root_path, &relative).map_err(|error| {
                    MigrationError::new(
                        MigrationErrorCode::MigrationSourceChanged,
                        "verify_source_snapshot",
                        error.to_string(),
                    )
                })?;
            if size != expected.size || sha256 != expected.sha256 {
                log::error!(
                    "[conversation-migration] source changed operation_id={} source={} expected_size={} actual_size={}",
                    inventory.operation_id,
                    expected.relative_path,
                    expected.size,
                    size
                );
                return Err(MigrationError::new(
                    MigrationErrorCode::MigrationSourceChanged,
                    "verify_source_snapshot",
                    format!(
                        "legacy source '{}' changed after inventory",
                        expected.relative_path
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn load_recovery_queue(operation_dir: &Path) -> Result<RecoveryQueueV1> {
    let path = operation_dir.join(RECOVERY_ITEMS_FILE);
    let bytes = fs::read(&path).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_recovery_queue",
            error.to_string(),
        )
    })?;
    let queue: RecoveryQueueV1 = serde_json::from_slice(&bytes).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_recovery_queue",
            error.to_string(),
        )
    })?;
    if queue.schema_version != RECOVERY_QUEUE_SCHEMA_VERSION {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "read_recovery_queue",
            "unsupported recovery queue schemaVersion",
        ));
    }
    Ok(queue)
}

fn verify_map(inventory: &LegacyInventoryV1, map: &MigrationMapV1) -> Result<()> {
    if map.operation_id != inventory.operation_id || map.schema_version != 1 {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_migration_map",
            "migration map identity or schemaVersion does not match inventory",
        ));
    }
    let source_keys = map
        .entries
        .iter()
        .map(|entry| entry.source_key.as_str())
        .collect::<HashSet<_>>();
    let ids = map
        .entries
        .iter()
        .map(|entry| entry.conversation_id)
        .collect::<HashSet<_>>();
    if source_keys.len() != map.entries.len() || ids.len() != map.entries.len() {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_migration_map",
            "migration map contains duplicate source keys or ConversationIds",
        ));
    }
    if map.entries.iter().any(|entry| {
        entry
            .legacy_agent_session_id
            .as_ref()
            .is_some_and(|opaque| path_contains_component(inventory, opaque))
    }) {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_migration_map",
            "opaque ACP session id appeared as a source or canonical directory component",
        ));
    }
    Ok(())
}

fn verify_staged_manifest(
    inventory: &LegacyInventoryV1,
    map: &MigrationMapV1,
    recovery_queue: &RecoveryQueueV1,
    staged: &StagedManifestV1,
) -> Result<()> {
    let receipt_sources = staged
        .receipts
        .iter()
        .map(|receipt| receipt.source_key.as_str())
        .collect::<HashSet<_>>();
    let receipt_ids = staged
        .receipts
        .iter()
        .map(|receipt| receipt.conversation_id)
        .collect::<HashSet<_>>();
    let migration_map_sha256 = sha256_json(map)?;
    let recovery_queue_sha256 = sha256_json(recovery_queue)?;
    let expected_staged_digest = stage_digest(
        inventory.operation_id,
        &inventory.inventory_sha256,
        &migration_map_sha256,
        &recovery_queue_sha256,
        &staged.receipts,
    )?;
    if staged.schema_version != 1
        || staged.operation_id != inventory.operation_id
        || recovery_queue.operation_id != inventory.operation_id
        || staged.inventory_sha256 != inventory.inventory_sha256
        || staged.migration_map_sha256 != migration_map_sha256
        || staged.recovery_queue_sha256 != recovery_queue_sha256
        || staged.staged_manifest_sha256 != expected_staged_digest
        || receipt_sources.len() != staged.receipts.len()
        || receipt_ids.len() != staged.receipts.len()
        || staged.receipts.iter().any(|receipt| {
            !map.entries.iter().any(|entry| {
                entry.source_key == receipt.source_key
                    && entry.conversation_id == receipt.conversation_id
            })
        })
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_staged_manifest",
            "staged manifest inputs or per-source receipts do not match the durable map",
        ));
    }
    Ok(())
}

fn verify_receipt(
    repository: &ConversationRepository,
    inventory: &LegacyInventoryV1,
    receipt: &StageReceiptV1,
) -> Result<()> {
    let record = repository
        .get_conversation(receipt.conversation_id)
        .map_err(repository_error)?;
    let events = repository
        .read_events(receipt.conversation_id, 0)
        .map_err(repository_error)?;
    if events.len() as u64 != receipt.event_count || record.last_seq != receipt.event_count {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_stage_receipt",
            "staged event/materialized sequence count differs from receipt",
        ));
    }
    let message_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.type_,
                crate::conversation::event_log::ConversationEventType::UserPrompt
                    | crate::conversation::event_log::ConversationEventType::MessageChunk
                    | crate::conversation::event_log::ConversationEventType::PromptComplete
                    | crate::conversation::event_log::ConversationEventType::CreationFailed
            )
        })
        .count() as u64;
    let tool_count = events
        .iter()
        .filter(|event| {
            event.type_ == crate::conversation::event_log::ConversationEventType::ToolCall
        })
        .count() as u64;
    if message_count != receipt.message_count || tool_count != receipt.tool_count {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_stage_receipt",
            "message/tool materialization counts differ from receipt",
        ));
    }
    let provenance = repository
        .read_provenance(receipt.conversation_id)
        .map_err(repository_error)?
        .ok_or_else(|| {
            MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "verify_stage_receipt",
                "staged Conversation is missing immutable provenance",
            )
        })?;
    let inventory_sources = inventory
        .roots
        .iter()
        .enumerate()
        .flat_map(|(root_index, root)| {
            root.files.iter().map(move |file| {
                (
                    format!(
                        "{}/{}/{}",
                        root.source_kind.as_str(),
                        root_index,
                        file.relative_path
                    ),
                    file.sha256.as_str(),
                )
            })
        })
        .collect::<HashSet<_>>();
    if sha256_json(&provenance)? != receipt.provenance_sha256
        || provenance.source_records.iter().any(|source| {
            !source.preserved_read_only
                || !inventory_sources
                    .contains(&(source.relative_path.clone(), source.sha256.as_str()))
        })
        || stage_output_digest(&record, &events, &provenance)? != receipt.output_sha256
    {
        return Err(MigrationError::new(
            MigrationErrorCode::MigrationVerificationFailed,
            "verify_stage_receipt",
            "provenance digest or preserved-read-only marker differs from receipt",
        ));
    }
    if let Some(opaque) = &receipt.opaque_agent_session_id {
        let binding = repository
            .current_binding(receipt.conversation_id)
            .map_err(repository_error)?;
        if binding
            .as_ref()
            .is_none_or(|binding| binding.agent_session_id != *opaque)
        {
            return Err(MigrationError::new(
                MigrationErrorCode::MigrationVerificationFailed,
                "verify_stage_receipt",
                "opaque ACP session id was not preserved in the binding",
            ));
        }
    }
    Ok(())
}

fn path_contains_component(inventory: &LegacyInventoryV1, value: &str) -> bool {
    inventory.roots.iter().any(|root| {
        Path::new(&root.canonical_path)
            .components()
            .any(|component| component.as_os_str() == value)
    })
}

fn source_snapshot_digest(inventory: &LegacyInventoryV1) -> Result<String> {
    let values = inventory
        .roots
        .iter()
        .flat_map(|root| {
            root.files.iter().map(move |file| {
                (
                    root.source_kind,
                    root.canonical_path.as_str(),
                    file.relative_path.as_str(),
                    file.size,
                    file.sha256.as_str(),
                )
            })
        })
        .collect::<Vec<_>>();
    sha256_json(&values)
}

fn validation_digest(
    inventory: &LegacyInventoryV1,
    map: &MigrationMapV1,
    staged: &StagedManifestV1,
    source_snapshot_sha256: &str,
    catalog_sha256: &str,
) -> Result<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Validation<'a> {
        operation_id: uuid::Uuid,
        inventory_sha256: &'a str,
        migration_map_sha256: String,
        staged_manifest_sha256: &'a str,
        source_snapshot_sha256: &'a str,
        catalog_sha256: &'a str,
    }
    sha256_json(&Validation {
        operation_id: inventory.operation_id,
        inventory_sha256: &inventory.inventory_sha256,
        migration_map_sha256: sha256_json(map)?,
        staged_manifest_sha256: &staged.staged_manifest_sha256,
        source_snapshot_sha256,
        catalog_sha256,
    })
}

fn sha256_json(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationJournalCorrupt,
            "hash_verification_json",
            error.to_string(),
        )
    })?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn repository_error(error: crate::conversation::repository::RepositoryError) -> MigrationError {
    MigrationError::new(
        MigrationErrorCode::MigrationVerificationFailed,
        "verify_repository",
        error.to_string(),
    )
}

fn digest_prefix(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::migration::inventory::{
        inventory_legacy_roots, LegacyRootConfiguration,
    };
    use crate::conversation::migration::legacy::{
        stage_legacy_conversations, LegacyStageConfiguration,
    };
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, LegacyInventoryV1) {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().canonicalize().unwrap().join("host");
        let sessions = host.join("acp-sessions");
        let directory = sessions.join("018f7a1c-1b4d-7c8a-9f01-0123456789ab");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("metadata.json"),
            serde_json::to_vec(&json!({
                "schemaVersion":1,
                "storageKey":"018f7a1c-1b4d-7c8a-9f01-0123456789ab",
                "sessionId":"provider/session:abc",
                "stableAgentNamespace":"config:test",
                "runtimeAgentId":"runtime-test",
                "cwd":"/legacy",
                "createdAt":1_700_000_000_000_u64
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join("messages.jsonl"),
            format!("{}\n", json!({"schemaVersion":1,"sessionId":"provider/session:abc","seq":1,"type":"user_prompt","recordedAt":1_700_000_001_000_u64,"payload":{"turnId":"one"}})),
        )
        .unwrap();
        fs::write(directory.join("tool-calls.jsonl"), b"").unwrap();
        let operation_dir = host.join("conversation-migrations/op");
        let inventory = inventory_legacy_roots(
            &LegacyRootConfiguration {
                host_state_root: host.clone(),
                ..Default::default()
            },
            Uuid::new_v4(),
            Utc::now(),
            &operation_dir,
        )
        .unwrap();
        (temp, host, operation_dir, inventory)
    }

    #[tokio::test]
    async fn unchanged_read_only_sources_verify_and_source_change_fails_closed() {
        let (_temp, host, operation_dir, inventory) = fixture();
        let legacy_roots = inventory
            .roots
            .iter()
            .map(|root| PathBuf::from(&root.canonical_path))
            .collect::<Vec<_>>();
        #[cfg(unix)]
        make_read_only(&legacy_roots);
        stage_legacy_conversations(
            &LegacyStageConfiguration {
                host_state_root: host.clone(),
                operation_dir: operation_dir.clone(),
                project_worktrees: Vec::new(),
            },
            &inventory,
        )
        .await
        .unwrap();
        let report = verify_staged_layout(&host, &operation_dir, &inventory).unwrap();
        assert_eq!(report.staged_conversation_count, 1);
        assert_eq!(report.message_count, 1);
        #[cfg(unix)]
        make_writable(&legacy_roots);
        let source = legacy_roots[0].join("018f7a1c-1b4d-7c8a-9f01-0123456789ab/messages.jsonl");
        fs::write(&source, b"changed\n").unwrap();
        let error = verify_staged_layout(&host, &operation_dir, &inventory).unwrap_err();
        assert_eq!(error.code, MigrationErrorCode::MigrationSourceChanged);
    }

    #[cfg(unix)]
    fn make_read_only(roots: &[PathBuf]) {
        use std::os::unix::fs::PermissionsExt;
        for root in roots {
            for path in walk(root) {
                let mode = if path.is_dir() { 0o555 } else { 0o444 };
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }

    #[cfg(unix)]
    fn make_writable(roots: &[PathBuf]) {
        use std::os::unix::fs::PermissionsExt;
        for root in roots {
            for path in walk(root) {
                let mode = if path.is_dir() { 0o755 } else { 0o644 };
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }

    #[cfg(unix)]
    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut paths = vec![root.to_path_buf()];
        let mut index = 0;
        while index < paths.len() {
            let current = paths[index].clone();
            index += 1;
            if current.is_dir() {
                if let Ok(entries) = fs::read_dir(&current) {
                    paths.extend(entries.flatten().map(|entry| entry.path()));
                }
            }
        }
        paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        paths
    }
}
