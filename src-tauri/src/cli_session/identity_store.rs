//! Durable, host-only identity authority. Never populate this store from a renderer snapshot.
use crate::agent_hooks::contract::{
    HookErrorCode, HookErrorDto, HookIdentityDto, HookIdentitySource, HookStateDto, RestoreState,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn error(code: HookErrorCode, message: &str) -> HookErrorDto {
    HookErrorDto {
        code,
        message: message.into(),
    }
}
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub(crate) fn validate_key(key: &str) -> Result<(), HookErrorDto> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(error(
            HookErrorCode::ValidationError,
            "invalid identity reference",
        ));
    }
    Ok(())
}
pub(crate) fn root(home: &Path) -> PathBuf {
    home.join(".se-manager/agent-hooks/identities")
}
pub(crate) fn safe_path(path: &Path) -> Result<(), HookErrorDto> {
    crate::agent_hooks::resources::reject_symlink_components(path)
        .map_err(|_| error(HookErrorCode::ValidationError, "unsafe identity path"))
}
pub(crate) struct IdentityLock {
    _file: File,
}
pub(crate) fn lock(home: &Path, id: &str) -> Result<IdentityLock, HookErrorDto> {
    validate_key(id)?;
    let dir = root(home);
    safe_path(&dir)?;
    fs::create_dir_all(&dir)
        .map_err(|_| error(HookErrorCode::InternalError, "cannot create identity store"))?;
    let path = dir.join(format!("{id}.lock"));
    safe_path(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| error(HookErrorCode::InternalError, "cannot open identity lock"))?;
    file.lock_exclusive()
        .map_err(|_| error(HookErrorCode::Conflict, "identity is locked"))?;
    Ok(IdentityLock { _file: file })
}
pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), HookErrorDto> {
    safe_path(path)?;
    let bytes = serde_json::to_vec(value)
        .map_err(|_| error(HookErrorCode::InternalError, "cannot encode identity"))?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut file = options
            .open(&tmp)
            .map_err(|_| error(HookErrorCode::InternalError, "cannot persist identity"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| error(HookErrorCode::InternalError, "cannot sync identity"))?;
        fs::rename(&tmp, path)
            .map_err(|_| error(HookErrorCode::InternalError, "cannot publish identity"))?;
        File::open(path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(|_| {
                error(
                    HookErrorCode::InternalError,
                    "cannot sync identity directory",
                )
            })
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, HookErrorDto> {
    safe_path(path)?;
    let bytes = fs::read(path).map_err(|_| {
        error(
            HookErrorCode::IdentityStale,
            "identity metadata unavailable",
        )
    })?;
    if bytes.len() > 1024 * 1024 {
        return Err(error(
            HookErrorCode::ValidationError,
            "identity metadata exceeds limit",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| error(HookErrorCode::IdentityStale, "invalid identity metadata"))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredIdentity {
    pub descriptor_id: String,
    pub origin_host_instance_id: String,
    pub last_host_instance_id: String,
    pub adapter_install_id: String,
    pub terminal_id: String,
    pub generation: u64,
    pub identity: HookIdentityDto,
    pub owner_session_id: String,
    pub project_id: String,
    pub variant: String,
    pub snapshot_ref: String,
    pub restore_state: RestoreState,
    pub updated_at: u64,
    #[serde(default)]
    pub process_ids: Vec<u32>,
    #[serde(default)]
    pub unreferenced_since: Option<u64>,
}
pub(crate) fn load(home: &Path, id: &str) -> Result<StoredIdentity, HookErrorDto> {
    validate_key(id)?;
    read_json(&root(home).join(format!("{id}.json")))
}
pub(crate) fn save(home: &Path, value: &StoredIdentity) -> Result<(), HookErrorDto> {
    atomic_json(
        &root(home).join(format!("{}.json", value.descriptor_id)),
        value,
    )
}
/// Only the verified hook receiver can supply ownership, snapshot and launch variant.
pub fn upsert_verified_identity(
    home: &Path,
    owner_session_id: &str,
    state: &HookStateDto,
    snapshot_ref: &str,
    variant: &str,
) -> Result<HookIdentityDto, HookErrorDto> {
    if state.identity.source != HookIdentitySource::Hook
        || state.identity.agent_session_id != state.agent_session_id
    {
        return Err(error(
            HookErrorCode::ValidationError,
            "invalid verified identity",
        ));
    }
    super::restore_service::guard_inflight_hook(home, owner_session_id, state)?;
    let key = format!(
        "{}:{}:{}",
        owner_session_id, state.adapter_install_id, state.terminal_id
    );
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes()).to_string();
    let _lock = lock(home, &id)?;
    let previous = load(home, &id).ok();
    let mut identity = state.identity.clone();
    identity.descriptor_id = Some(id.clone());
    identity.resume_argv = None;
    if let Some(p) = previous
        .as_ref()
        .filter(|p| p.identity.agent_session_id == identity.agent_session_id)
    {
        if identity.transcript_path.is_none() {
            identity.transcript_path = p.identity.transcript_path.clone();
        }
        if identity.cwd.is_none() {
            identity.cwd = p.identity.cwd.clone();
        }
    }
    let value = StoredIdentity {
        descriptor_id: id,
        origin_host_instance_id: previous
            .as_ref()
            .map(|p| p.origin_host_instance_id.clone())
            .unwrap_or_else(|| state.instance_id.clone()),
        last_host_instance_id: state.instance_id.clone(),
        adapter_install_id: state.adapter_install_id.clone(),
        terminal_id: state.terminal_id.clone(),
        generation: state.generation,
        identity,
        owner_session_id: owner_session_id.into(),
        project_id: state.project_id.clone(),
        variant: variant.into(),
        snapshot_ref: snapshot_ref.into(),
        restore_state: state.restore_state,
        unreferenced_since: None,
        process_ids: previous
            .as_ref()
            .filter(|p| p.identity.agent_session_id == state.agent_session_id)
            .map(|p| p.process_ids.clone())
            .unwrap_or_default(),
        updated_at: previous
            .as_ref()
            .map(|p| p.updated_at)
            .unwrap_or_else(now_ms),
    };
    if previous.as_ref() != Some(&value) {
        let mut value = value.clone();
        value.updated_at = now_ms();
        save(home, &value)?;
    }
    Ok(value.identity)
}
/// Host transport helper, never an IPC command. Caller must authorize project/conversation membership.
pub fn descriptor_ownership(
    home: &Path,
    descriptor_id: &str,
) -> Result<(String, String), HookErrorDto> {
    let value = load(home, descriptor_id)?;
    Ok((value.project_id, value.owner_session_id))
}
/// Record only host-verified session-owning agent processes, never renderer-submitted PIDs.
pub fn record_verified_processes(
    home: &Path,
    descriptor_id: &str,
    mut pids: Vec<u32>,
) -> Result<(), HookErrorDto> {
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() || pids.len() > 64 || pids.contains(&0) {
        return Err(error(
            HookErrorCode::ValidationError,
            "invalid process evidence",
        ));
    }
    let _guard = lock(home, descriptor_id)?;
    let mut identity = load(home, descriptor_id)?;
    if identity.process_ids != pids {
        identity.process_ids = pids;
        identity.updated_at = now_ms();
        save(home, &identity)?;
    }
    Ok(())
}

/// Host-only collection; inputs must include all layout/workspace references and live instances.
/// Unproven claims and live plans always retain their descriptor.
pub fn gc_unreferenced(
    home: &Path,
    references: &std::collections::HashSet<String>,
    live_ptys: &std::collections::HashSet<String>,
) -> Result<usize, HookErrorDto> {
    const RETAIN_MS: u64 = 30 * 24 * 60 * 60 * 1000;
    let Ok(entries) = fs::read_dir(root(home)) else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let Ok(candidate) = read_json::<StoredIdentity>(&entry.path()) else {
            continue;
        };
        let _guard = lock(home, &candidate.descriptor_id)?;
        let mut current = load(home, &candidate.descriptor_id)?;
        if references.contains(&current.descriptor_id) || live_ptys.contains(&current.terminal_id) {
            if current.unreferenced_since.take().is_some() {
                save(home, &current)?;
            }
            continue;
        }
        let Some(since) = current.unreferenced_since else {
            current.unreferenced_since = Some(now_ms());
            save(home, &current)?;
            continue;
        };
        if now_ms().saturating_sub(since) < RETAIN_MS
            || !super::restore_service::gc_safe(home, &current.descriptor_id, live_ptys)?
        {
            continue;
        }
        fs::remove_file(entry.path())
            .map_err(|_| error(HookErrorCode::InternalError, "cannot collect identity"))?;
        count += 1;
    }
    Ok(count)
}

pub fn mark_ended(
    home: &Path,
    descriptor_id: &str,
    expected_generation: u64,
) -> Result<(), HookErrorDto> {
    let _lock = lock(home, descriptor_id)?;
    let mut value = load(home, descriptor_id)?;
    if value.generation != expected_generation {
        return Err(error(
            HookErrorCode::IdentityStale,
            "identity generation changed",
        ));
    }
    if value.restore_state != RestoreState::Ended {
        value.restore_state = RestoreState::Ended;
        value.updated_at = now_ms();
        save(home, &value)?;
    }
    Ok(())
}
