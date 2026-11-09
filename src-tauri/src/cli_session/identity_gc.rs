//! Conservative, receiver-owned identity retention. Never run on the hook hot path.
use super::identity_store::{self, StoredIdentity};
use crate::conversation::ConversationRepository;
use crate::core::TerminalServiceHandle;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone)]
pub struct GcContext {
    pub repository: Arc<ConversationRepository>,
    pub manifest_root: PathBuf,
    pub layout_file: PathBuf,
    pub terminal: TerminalServiceHandle,
}
static CONTEXTS: OnceLock<Mutex<Vec<GcContext>>> = OnceLock::new();
pub fn register(context: GcContext) {
    CONTEXTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(context);
}

fn collect_json_refs(value: &Value, refs: &mut HashSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "descriptorId" | "descriptor_id") {
                    if let Some(id) = value.as_str() {
                        refs.insert(id.to_owned());
                    }
                }
                collect_json_refs(value, refs);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_json_refs(value, refs);
            }
        }
        _ => {}
    }
}
fn read_refs(path: &Path, refs: &mut HashSet<String>) -> Result<(), String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let json: Value =
                serde_json::from_slice(&bytes).map_err(|_| "invalid reference document")?;
            collect_json_refs(&json, refs);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("reference document unavailable".into()),
    }
}
fn persisted_refs(context: &GcContext, refs: &mut HashSet<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(context.repository.root())
        .map_err(|_| "conversation enumeration failed")?;
    for entry in entries {
        let entry = entry.map_err(|_| "conversation enumeration failed")?;
        let kind = entry
            .file_type()
            .map_err(|_| "conversation enumeration failed")?;
        if kind.is_symlink() {
            return Err("ambiguous conversation reference root".into());
        }
        if kind.is_dir() {
            read_refs(&entry.path().join("workspace.json"), refs)?;
        }
    }
    match std::fs::read_dir(&context.manifest_root) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(|_| "manifest enumeration failed")?.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    read_refs(&path, refs)?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("manifest enumeration failed".into()),
    }
    read_refs(&context.layout_file, refs)
}

/// Any incomplete reference/liveness evidence skips the whole pass. Foreign
/// host identities are pinned: this process cannot enumerate another host's
/// layout or live PTYs, and the HOME store is shared across desktop/server.
async fn once(home: PathBuf, contexts: Vec<GcContext>) -> Result<usize, String> {
    if contexts.is_empty() {
        return Ok(0);
    }
    let mut live = HashSet::new();
    for context in &contexts {
        if let Some(client) = context.terminal.core_client() {
            for status in client
                .list()
                .await
                .map_err(|_| "terminal enumeration failed")?
            {
                if status.active {
                    live.insert(status.id);
                }
            }
        } else if let Some(manager) = context.terminal.in_process_pty() {
            for terminal in manager.get_all() {
                if terminal.is_active() {
                    live.insert(terminal.id.clone());
                }
            }
        } else {
            return Err("terminal enumeration unavailable".into());
        }
    }
    tokio::task::spawn_blocking(move || {
        let mut refs = HashSet::new();
        for context in &contexts {
            persisted_refs(context, &mut refs)?;
        }
        if let Ok(entries) = std::fs::read_dir(identity_store::root(&home)) {
            for entry in entries {
                let path = entry.map_err(|_| "identity enumeration failed")?.path();
                if path.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                if let Ok(identity) = identity_store::read_json::<StoredIdentity>(&path) {
                    if identity.last_host_instance_id != crate::agent_hooks::state::instance_id() {
                        refs.insert(identity.descriptor_id);
                    }
                }
            }
        }
        super::restore_service::gc_expired_plans(&home)
            .map_err(|_| "expired plan collection failed")?;
        identity_store::gc_unreferenced(&home, &refs, &live)
            .map_err(|_| "identity collection failed".into())
    })
    .await
    .map_err(|_| "identity maintenance task failed")?
}

/// One pass at receiver start and then no more than hourly. Filesystem work is
/// moved off the receiver runtime, and failures never affect hook availability.
pub async fn run(home: PathBuf) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let contexts = CONTEXTS
            .get()
            .map(|contexts| contexts.lock().unwrap_or_else(|p| p.into_inner()).clone())
            .unwrap_or_default();
        if once(home.clone(), contexts).await.is_err() {
            log::warn!(target: "se_manager::agent_hooks", "operation=identity_gc kind=incomplete_evidence count=1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn maintenance_keeps_every_reference_and_live_pty_and_collects_only_old_orphans() {
        let home = tempfile::tempdir().unwrap();
        let (repository, _) =
            ConversationRepository::open(home.path().join("conversations")).unwrap();
        let manager = crate::web::test_pty_manager();
        let spawned = manager
            .spawn(
                se_pty::SpawnOptions {
                    program: Some("/bin/sh".into()),
                    args: Some(vec!["-c".into(), "sleep 30".into()]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let live_id = spawned.info.id.clone();
        let mut ids = Vec::new();
        for terminal in [
            "workspace",
            "layout",
            live_id.as_str(),
            "orphan",
            "foreign",
            "recent",
        ] {
            let mut state = crate::cli_session::restore_tests::state();
            state.terminal_id = terminal.into();
            state.instance_id = if terminal == "foreign" {
                "other-host".into()
            } else {
                crate::agent_hooks::state::instance_id().into()
            };
            let identity = identity_store::upsert_verified_identity(
                home.path(),
                "owner",
                &state,
                "snapshot",
                "default",
            )
            .unwrap();
            let id = identity.descriptor_id.unwrap();
            let mut stored = identity_store::load(home.path(), &id).unwrap();
            stored.unreferenced_since = if terminal == "recent" { None } else { Some(1) };
            identity_store::save(home.path(), &stored).unwrap();
            ids.push(id);
        }
        let context = GcContext {
            repository,
            manifest_root: home.path().join("manifests"),
            layout_file: home.path().join("layout.json"),
            terminal: TerminalServiceHandle::in_process(manager.clone()),
        };
        std::fs::create_dir_all(context.repository.root().join("record")).unwrap();
        std::fs::write(
            context.repository.root().join("record/workspace.json"),
            serde_json::json!({"resources":[{"agentDescriptor":{"descriptorId":ids[0]}}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            &context.layout_file,
            serde_json::json!({"agentSession":{"descriptor_ref":{"descriptor_id":ids[1]}}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            once(home.path().into(), vec![context.clone()])
                .await
                .unwrap(),
            1
        );
        for index in [0, 1, 2, 4, 5] {
            assert!(identity_store::load(home.path(), &ids[index]).is_ok());
        }
        assert!(identity_store::load(home.path(), &ids[3]).is_err());
        assert!(identity_store::load(home.path(), &ids[5])
            .unwrap()
            .unreferenced_since
            .is_some());
        // Incomplete reference evidence must prevent even old orphan deletion.
        let mut recent = identity_store::load(home.path(), &ids[5]).unwrap();
        recent.unreferenced_since = Some(1);
        identity_store::save(home.path(), &recent).unwrap();
        std::fs::write(&context.layout_file, "invalid json").unwrap();
        assert!(once(home.path().into(), vec![context]).await.is_err());
        assert!(identity_store::load(home.path(), &ids[5]).is_ok());
        manager.terminate(&live_id).await.unwrap();
    }

    #[test]
    fn persisted_layout_and_workspace_references_are_collected_and_corruption_fails_closed() {
        let home = tempfile::tempdir().unwrap();
        let layout = home.path().join("layout.json");
        std::fs::write(&layout, r#"{"projects":[{"agentSession":{"descriptor_ref":{"descriptor_id":"legacy"}}}],"resources":[{"agentDescriptor":{"descriptorId":"workspace"}}]}"#).unwrap();
        let mut refs = HashSet::new();
        read_refs(&layout, &mut refs).unwrap();
        assert_eq!(refs, HashSet::from(["legacy".into(), "workspace".into()]));
        std::fs::write(&layout, b"corrupt").unwrap();
        assert!(read_refs(&layout, &mut HashSet::new()).is_err());
    }
}
