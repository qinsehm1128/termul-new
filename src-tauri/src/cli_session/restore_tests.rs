use super::identity_store as store;
use crate::agent_hooks::contract::*;
use std::path::Path;
pub(super) fn state() -> HookStateDto {
    HookStateDto {
        instance_id: "host-one".into(),
        terminal_id: "pty-one".into(),
        generation: 1,
        project_id: "project-one".into(),
        adapter_install_id: "install-one".into(),
        agent_session_id: "session-one".into(),
        identity: HookIdentityDto {
            descriptor_id: None,
            adapter_id: "pi".to_owned().try_into().unwrap(),
            agent_session_id: "session-one".into(),
            cwd: Some("/work".into()),
            transcript_path: None,
            source: HookIdentitySource::Hook,
            resume_argv: None,
        },
        state: DerivedState::Idle,
        restore_state: RestoreState::Confirmed,
        revision: 1,
    }
}
fn put(home: &Path, event: &HookStateDto) -> HookIdentityDto {
    store::upsert_verified_identity(home, "owner-one", event, "snapshot-one", "default").unwrap()
}
#[test]
fn qin_self_dev_requires_launch_and_process_argv_agreement() {
    let argv = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };
    let standard = argv(&["/opt/qin-code", "--resume", "session"]);
    let self_dev = argv(&["/opt/qin-code", "self-dev", "--resume", "session"]);
    assert_eq!(
        super::live::qin_variant_from_evidence(&self_dev, &self_dev),
        Some("self-dev")
    );
    assert_eq!(
        super::live::qin_variant_from_evidence(&standard, &standard),
        Some("default")
    );
    assert_eq!(
        super::live::qin_variant_from_evidence(&standard, &self_dev),
        None
    );
    assert_eq!(
        super::live::qin_variant_from_evidence(&argv(&["/bin/sh"]), &self_dev),
        None
    );
}

#[test]
fn trusted_identity_ignores_busy_revisions_and_preserves_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let mut event = state();
    let first = put(&home, &event);
    let path = store::root(&home).join(format!("{}.json", first.descriptor_id.as_ref().unwrap()));
    let before = std::fs::read(&path).unwrap();
    event.revision += 1;
    event.state = DerivedState::Busy;
    assert_eq!(put(&home, &event), first);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn normal_end_is_durable_and_origin_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let mut event = state();
    let first = put(&home, &event);
    let id = first.descriptor_id.unwrap();
    event.instance_id = "host-two".into();
    put(&home, &event);
    store::mark_ended(&home, &id, 1).unwrap();
    let ended = store::load(&home, &id).unwrap();
    assert_eq!(ended.origin_host_instance_id, "host-one");
    assert_eq!(ended.last_host_instance_id, "host-two");
    assert_eq!(ended.restore_state, RestoreState::Ended);
    assert!(store::mark_ended(&home, &id, 2).is_err());
}
#[test]
fn gc_requires_thirty_days_without_any_reference_or_live_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let identity = put(&home, &state());
    let id = identity.descriptor_id.unwrap();
    let empty = std::collections::HashSet::new();
    assert_eq!(store::gc_unreferenced(&home, &empty, &empty).unwrap(), 0);
    let mut saved = store::load(&home, &id).unwrap();
    saved.updated_at = 1;
    store::save(&home, &saved).unwrap();
    assert_eq!(
        store::gc_unreferenced(
            &home,
            &std::collections::HashSet::from([id.clone()]),
            &empty
        )
        .unwrap(),
        0
    );
    assert_eq!(
        store::gc_unreferenced(
            &home,
            &empty,
            &std::collections::HashSet::from([saved.terminal_id])
        )
        .unwrap(),
        0
    );
    let mut expired = store::load(&home, &id).unwrap();
    expired.unreferenced_since = Some(1);
    store::save(&home, &expired).unwrap();
    assert_eq!(store::gc_unreferenced(&home, &empty, &empty).unwrap(), 1);
    assert!(store::load(&home, &id).is_err());
}

#[test]
fn identity_references_cannot_escape_private_store() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(store::load(tmp.path(), "../escape").is_err());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".se-manager");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("agent-hooks")).unwrap();
        assert!(store::lock(tmp.path(), "safe-id").is_err());
    }
}
#[test]
fn pi_exact_path_rejects_symlinks_and_outside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let sessions = root.join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let path = sessions.join("stamp_session-one.jsonl");
    std::fs::write(&path, "{}").unwrap();
    assert_eq!(
        super::live::safe_pi_path(&sessions, &path),
        Some(path.clone())
    );
    assert!(super::live::safe_pi_path(&root.join("elsewhere"), &path).is_none());
    #[cfg(unix)]
    {
        let link = sessions.join("link.jsonl");
        std::os::unix::fs::symlink(path, &link).unwrap();
        assert!(super::live::safe_pi_path(&sessions, &link).is_none());
    }
}
