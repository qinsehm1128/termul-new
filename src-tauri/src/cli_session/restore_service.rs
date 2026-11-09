//! Host-authorized, snapshot-only restore. Renderer argv and filesystem hints are never authority.
use super::identity_store::{self as store, error, now_ms, StoredIdentity};
use crate::agent_hooks::contract::*;
use crate::core::TerminalServiceHandle;
use se_pty::manager::SpawnOptions;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct RestoreContext<'a> {
    pub home: &'a Path,
    pub host_instance_id: &'a str,
    pub project_id: &'a str,
    pub owner_session_id: &'a str,
    pub automatic: bool,
    pub auto_resume_enabled: bool,
    pub terminal_service: &'a TerminalServiceHandle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct DurablePlan {
    dto: ResumePlanDto,
    owner_session_id: String,
    instance_id: String,
    project_id: String,
    conversation_id: Option<String>,
    descriptor: StoredIdentity,
    argv: Vec<String>,
    cwd: String,
    consumed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ResumeClaim {
    descriptor_id: String,
    previous_generation: u64,
    owner_instance: String,
    owner_pid: u32,
    owner_session_id: String,
    attempt_id: String,
    newpty: Option<String>,
    generation: Option<u64>,
    phase: RestoreState,
    started_at: u64,
    /// The existing manager claim is preserved, not replaced by a restore credential.
    resume_grant: Option<String>,
}
/// Host resolver only. The shared binary name alone never proves qin self-dev.
pub fn verified_variant(
    service: &TerminalServiceHandle,
    terminal_id: &str,
    adapter_id: &str,
) -> Result<String, HookErrorDto> {
    if adapter_id != "qin-code" {
        return Ok("default".into());
    }
    let manager = service.in_process_pty().ok_or_else(|| {
        error(
            HookErrorCode::UnsupportedPlatform,
            "host launch evidence unavailable",
        )
    })?;
    let terminal = manager
        .get(terminal_id)
        .ok_or_else(|| error(HookErrorCode::IdentityStale, "terminal process unavailable"))?;
    let (program, argv) = manager.host_launch_evidence(terminal_id).ok_or_else(|| {
        error(
            HookErrorCode::IdentityStale,
            "host launch evidence unavailable",
        )
    })?;
    let launch: Vec<String> = std::iter::once(program).chain(argv).collect();
    super::live::verified_qin_variant(terminal.pid, &launch).ok_or_else(|| {
        error(
            HookErrorCode::IdentityStale,
            "launch and process variant evidence disagree",
        )
    })
}

/// Host-only helper after terminal ownership verification; never accept a client-submitted root PID.
pub fn capture_process_evidence(
    home: &Path,
    descriptor_id: &str,
    terminal_root_pid: u32,
) -> Result<(), HookErrorDto> {
    let identity = store::load(home, descriptor_id)?;
    let pids = super::live::verified_agent_processes(
        terminal_root_pid,
        identity.identity.adapter_id.as_str(),
    );
    store::record_verified_processes(home, descriptor_id, pids)
}

fn claim_path(home: &Path, id: &str) -> PathBuf {
    store::root(home).join(format!("{id}.claim.json"))
}
fn plan_path(home: &Path, id: &str) -> PathBuf {
    store::root(home).join(format!("plan-{id}.json"))
}
fn descriptor_owner(
    context: &RestoreContext<'_>,
    identity: &StoredIdentity,
    project: &str,
) -> Result<(), HookErrorDto> {
    if identity.owner_session_id != context.owner_session_id
        || project != identity.project_id
        || context.project_id != identity.project_id
    {
        return Err(error(
            HookErrorCode::Forbidden,
            "restore ownership mismatch",
        ));
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= super::types::SESSION_ID_MAX_LEN
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
/// Frozen installed snapshot is the only command source. No live custom source or submitted argv.
fn resume_argv(home: &Path, identity: &StoredIdentity) -> Result<Vec<String>, HookErrorDto> {
    let d = &identity.identity;
    let loaded = crate::agent_hooks::install::load_resume_snapshot(
        home,
        d.adapter_id.as_str(),
        &identity.adapter_install_id,
        &identity.snapshot_ref,
    )
    .map_err(|_| {
        error(
            HookErrorCode::IdentityStale,
            "snapshot_missing: installed resume snapshot unavailable",
        )
    })?;
    let manifest = &loaded.manifest;
    manifest
        .validate(d.adapter_id.as_str(), &loaded.origin)
        .map_err(|_| {
            error(
                HookErrorCode::ValidationError,
                "invalid installed resume snapshot",
            )
        })?;
    let resume = manifest.resume.as_ref().ok_or_else(|| {
        error(
            HookErrorCode::IdentityStale,
            "adapter does not support resume",
        )
    })?;
    let variant = match Some(identity.variant.as_str()) {
        Some(id) => resume.variants.iter().find(|v| v.id == id),
        None if resume.variants.len() == 1 => resume.variants.first(),
        None => resume.variants.iter().find(|v| v.id == "default"),
    }
    .ok_or_else(|| {
        error(
            HookErrorCode::IdentityStale,
            "verified launch variant unavailable",
        )
    })?;
    if !valid_id(&d.agent_session_id) {
        return Err(error(
            HookErrorCode::ValidationError,
            "invalid session identity",
        ));
    }
    let needs_path = resume
        .requires
        .iter()
        .chain(&variant.requires)
        .any(|r| r == "transcript_path");
    let path =
        if needs_path {
            let path =
                Path::new(d.transcript_path.as_deref().ok_or_else(|| {
                    error(HookErrorCode::IdentityStale, "exact transcript required")
                })?);
            let root = if resume.path_root == "pi-sessions" {
                home.join(".pi/agent/sessions")
            } else {
                home.to_path_buf()
            };
            store::safe_path(path)?;
            store::safe_path(&root)?;
            let canonical = path
                .canonicalize()
                .map_err(|_| error(HookErrorCode::IdentityStale, "transcript unavailable"))?;
            if !path.is_absolute()
                || !canonical.starts_with(root.canonicalize().map_err(|_| {
                    error(HookErrorCode::IdentityStale, "transcript root unavailable")
                })?)
                || !canonical.is_file()
                || fs::File::open(&canonical).is_err()
            {
                return Err(error(
                    HookErrorCode::IdentityStale,
                    "transcript outside approved root",
                ));
            }
            if d.adapter_id.as_str() == "pi" && super::live::safe_pi_path(&root, path).is_none() {
                return Err(error(HookErrorCode::IdentityStale, "invalid pi transcript"));
            }
            Some(canonical.to_string_lossy().into_owned())
        } else {
            None
        };
    let mut argv = vec![variant.binary.clone()];
    for arg in &variant.argv {
        argv.push(match arg.as_str() {
            "{{session_id}}" => d.agent_session_id.clone(),
            "{{transcript_path}}" => path
                .clone()
                .ok_or_else(|| error(HookErrorCode::IdentityStale, "exact transcript required"))?,
            _ => arg.clone(),
        });
    }
    Ok(argv)
}

pub async fn resume_plan(
    context: &RestoreContext<'_>,
    request: ResumePlanRequest,
) -> Result<ResumePlanDto, HookErrorDto> {
    let home = context.home.to_path_buf();
    let id = request.descriptor_id.clone();
    let _lock = tokio::task::spawn_blocking(move || store::lock(&home, &id))
        .await
        .map_err(|_| error(HookErrorCode::InternalError, "cannot lock restore identity"))??;
    let mut identity = store::load(&context.home, &request.descriptor_id)?;
    descriptor_owner(context, &identity, &request.project_id)?;
    if context.automatic
        && (!context.auto_resume_enabled || identity.identity.source != HookIdentitySource::Hook)
    {
        return Err(error(
            HookErrorCode::Forbidden,
            "automatic restore is not authorized",
        ));
    }
    if request.expected_generation != identity.generation {
        return Err(error(
            HookErrorCode::IdentityStale,
            "identity generation changed",
        ));
    }
    if context
        .terminal_service
        .in_process_pty()
        .ok_or_else(|| {
            error(
                HookErrorCode::UnsupportedPlatform,
                "restore requires an in-process terminal manager",
            )
        })?
        .get(&identity.terminal_id)
        .is_some()
    {
        return Err(error(
            HookErrorCode::Conflict,
            "identity already has a live terminal",
        ));
    }
    if matches!(
        identity.restore_state,
        RestoreState::Ended
            | RestoreState::Stale
            | RestoreState::Unconfirmed
            | RestoreState::PendingConfirmation
    ) {
        return Err(error(
            HookErrorCode::IdentityStale,
            "identity is not eligible for automatic restore",
        ));
    }
    // Losing a PTY does not prove its child agent died (orphaned/background
    // processes survive). All automatic restore paths require captured evidence.
    {
        if identity.process_ids.is_empty() {
            return Err(error(
                HookErrorCode::IdentityStale,
                "previous agent process ownership is unproven",
            ));
        }
        #[cfg(unix)]
        if identity.process_ids.iter().any(|pid| unsafe { libc::kill(*pid as libc::pid_t, 0) == 0 } || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)) {
            return Err(error(HookErrorCode::Conflict, "previous agent process may still be alive"));
        }
        #[cfg(not(unix))]
        return Err(error(
            HookErrorCode::UnsupportedPlatform,
            "previous agent process liveness cannot be verified",
        ));
    }
    let argv = match resume_argv(&context.home, &identity) {
        Ok(argv) => argv,
        Err(err) => {
            if err.message.starts_with("snapshot_missing:")
                && identity.restore_state != RestoreState::SnapshotMissing
            {
                identity.restore_state = RestoreState::SnapshotMissing;
                store::save(context.home, &identity)?;
            }
            return Err(err);
        }
    };
    let cwd = identity.identity.cwd.clone().ok_or_else(|| {
        error(
            HookErrorCode::IdentityStale,
            "verified working directory unavailable",
        )
    })?;
    if !Path::new(&cwd).is_absolute() || !Path::new(&cwd).is_dir() {
        return Err(error(
            HookErrorCode::IdentityStale,
            "verified working directory unavailable",
        ));
    }
    if identity.restore_state == RestoreState::Confirmed {
        identity.restore_state = RestoreState::Interrupted;
        store::save(&context.home, &identity)?;
    }
    let dto = ResumePlanDto {
        plan_id: uuid::Uuid::new_v4().to_string(),
        descriptor_id: request.descriptor_id,
        expected_generation: identity.generation,
        expires_at: now_ms() + 60_000,
        expected_identity: identity.identity.clone(),
        display_argv: argv.clone(),
    };
    store::atomic_json(
        &plan_path(context.home, &dto.plan_id),
        &DurablePlan {
            dto: dto.clone(),
            owner_session_id: context.owner_session_id.into(),
            instance_id: context.host_instance_id.into(),
            project_id: request.project_id,
            conversation_id: Some(context.owner_session_id.into()),
            descriptor: identity,
            argv,
            cwd,
            consumed: false,
        },
    )?;
    Ok(dto)
}
fn result(claim: &ResumeClaim) -> Result<ResumeSpawnResult, HookErrorDto> {
    Ok(ResumeSpawnResult {
        pty_id: claim
            .newpty
            .clone()
            .ok_or_else(|| error(HookErrorCode::Conflict, "prior spawn outcome is unproven"))?,
        generation: claim
            .generation
            .ok_or_else(|| error(HookErrorCode::Conflict, "prior generation is unproven"))?,
        attempt_id: claim.attempt_id.clone(),
        pending_confirmation: claim.phase == RestoreState::PendingConfirmation,
        restore_state: claim.phase,
    })
}
fn expire_confirmation(home: &Path, claim: &mut ResumeClaim) -> Result<(), HookErrorDto> {
    if claim.phase == RestoreState::PendingConfirmation
        && now_ms().saturating_sub(claim.started_at) >= 10_000
    {
        claim.phase = RestoreState::Unconfirmed;
        store::atomic_json(&claim_path(home, &claim.descriptor_id), claim)?;
        let mut identity = store::load(home, &claim.descriptor_id)?;
        identity.restore_state = RestoreState::Unconfirmed;
        store::save(home, &identity)?;
    }
    Ok(())
}
pub async fn resume_spawn(
    context: &RestoreContext<'_>,
    request: ResumeSpawnRequest,
) -> Result<ResumeSpawnResult, HookErrorDto> {
    store::validate_key(&request.plan_id)?;
    store::validate_key(&request.descriptor_id)?;
    let mut plan: DurablePlan = store::read_json(&plan_path(&context.home, &request.plan_id))?;
    let home = context.home.to_path_buf();
    let descriptor_id = plan.dto.descriptor_id.clone();
    let _lock = tokio::task::spawn_blocking(move || store::lock(&home, &descriptor_id))
        .await
        .map_err(|_| error(HookErrorCode::InternalError, "cannot lock restore identity"))??;
    // Re-read under the descriptor lock: the plan is a one-shot CAS token.
    plan = store::read_json(&plan_path(&context.home, &request.plan_id))?;
    if plan.owner_session_id != context.owner_session_id
        || plan.instance_id != context.host_instance_id
        || plan.project_id != context.project_id
    {
        return Err(error(
            HookErrorCode::Forbidden,
            "restore plan ownership mismatch",
        ));
    }
    if context.automatic
        && (!context.auto_resume_enabled
            || plan.descriptor.identity.source != HookIdentitySource::Hook)
    {
        return Err(error(
            HookErrorCode::Forbidden,
            "automatic restore is not authorized",
        ));
    }
    if request.descriptor_id != plan.dto.descriptor_id
        || request.expected_generation != plan.dto.expected_generation
    {
        return Err(error(
            HookErrorCode::Conflict,
            "restore generation mismatch",
        ));
    }
    let pty = context.terminal_service.in_process_pty().ok_or_else(|| {
        error(
            HookErrorCode::UnsupportedPlatform,
            "restore requires an in-process terminal manager",
        )
    })?;
    let claim_file = claim_path(&context.home, &plan.dto.descriptor_id);
    if claim_file.exists() {
        let mut claim: ResumeClaim = store::read_json(&claim_file)?;
        if claim.previous_generation == request.expected_generation {
            if claim.owner_instance != context.host_instance_id
                || claim.owner_session_id != context.owner_session_id
            {
                // Even a dead owner's child may survive. Liveness informs the
                // diagnostic, never permits an unproven second spawn.
                #[cfg(unix)]
                let owner_alive = unsafe { libc::kill(claim.owner_pid as libc::pid_t, 0) == 0 }
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                #[cfg(not(unix))]
                let owner_alive = true;
                return Err(error(
                    HookErrorCode::Conflict,
                    if owner_alive {
                        "restore is owned by another live host instance"
                    } else {
                        "prior host exited but its spawn outcome is unproven"
                    },
                ));
            }
            if claim.newpty.is_none() {
                if let Some(spawned) = pty
                    .resume_attempt_terminal(
                        &claim.descriptor_id,
                        claim.previous_generation,
                        &claim.attempt_id,
                    )
                    .await
                {
                    crate::agent_hooks::server::register_owned_terminal(&spawned.info.id);
                    claim.generation = pty.claim_generation(&spawned.info.id);
                    claim.resume_grant = Some(spawned.claim);
                    claim.newpty = Some(spawned.info.id);
                    store::atomic_json(&claim_file, &claim)?;
                } else {
                    return Err(error(
                        HookErrorCode::Conflict,
                        "prior spawn outcome is unproven",
                    ));
                }
            }
            expire_confirmation(&context.home, &mut claim)?;
            return result(&claim);
        }
    }
    if plan.consumed || now_ms() >= plan.dto.expires_at {
        return Err(error(
            HookErrorCode::PlanExpired,
            "restore plan expired or consumed",
        ));
    }
    let identity = store::load(&context.home, &plan.dto.descriptor_id)?;
    descriptor_owner(context, &identity, &plan.project_id)?;
    if identity != plan.descriptor || pty.get(&identity.terminal_id).is_some() {
        return Err(error(
            HookErrorCode::IdentityStale,
            "restore identity changed",
        ));
    }
    // Revalidate installation and exact path at use time, not just at preview time.
    if resume_argv(&context.home, &identity)? != plan.argv {
        return Err(error(
            HookErrorCode::IdentityStale,
            "resume snapshot changed",
        ));
    }
    let mut claim = ResumeClaim {
        descriptor_id: plan.dto.descriptor_id.clone(),
        previous_generation: request.expected_generation,
        owner_instance: context.host_instance_id.into(),
        owner_pid: std::process::id(),
        owner_session_id: context.owner_session_id.into(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        newpty: None,
        generation: None,
        phase: RestoreState::PendingConfirmation,
        started_at: now_ms(),
        resume_grant: None,
    };
    store::atomic_json(&claim_file, &claim)?;
    plan.consumed = true;
    store::atomic_json(&plan_path(&context.home, &plan.dto.plan_id), &plan)?;
    let spawned = pty
        .spawn_resume_idempotent(
            &claim.descriptor_id,
            claim.previous_generation,
            &claim.attempt_id,
            SpawnOptions {
                cwd: Some(plan.cwd),
                project_id: Some(plan.project_id),
                env: Some(crate::agent_hooks::server::receiver_environment()),
                conversation_id: plan
                    .conversation_id
                    .map(|id| serde_json::from_value(serde_json::Value::String(id)))
                    .transpose()
                    .map_err(|_| {
                        error(
                            HookErrorCode::IdentityStale,
                            "invalid stored terminal owner",
                        )
                    })?,
                program: plan.argv.first().cloned(),
                args: Some(plan.argv[1..].to_vec()),
                ..SpawnOptions::default()
            },
        )
        .await
        .map_err(|_| {
            error(
                HookErrorCode::InternalError,
                "resume spawn failed; outcome requires inspection",
            )
        })?;
    crate::agent_hooks::server::register_owned_terminal(&spawned.info.id);
    claim.generation = pty.claim_generation(&spawned.info.id);
    claim.newpty = Some(spawned.info.id);
    claim.resume_grant = Some(spawned.claim);
    store::atomic_json(&claim_file, &claim)?;
    // A very fast hook may have arrived before newpty was persisted. Its trusted start
    // receipt closes that window while we still hold the same descriptor lock.
    if let Ok(start) = store::read_json::<VerifiedStart>(&start_path(
        context.home,
        claim.newpty.as_deref().unwrap(),
    )) {
        let _ = confirm_claim_locked(context.home, &start.owner, &start.state, &mut claim);
    }
    let answer = result(&claim)?;
    drop(_lock);
    crate::agent_hooks::state::set_restore_state(
        &answer.pty_id,
        answer.generation,
        answer.restore_state,
    );
    let home = context.home.to_path_buf();
    let id = claim.descriptor_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = store::lock(&home, &id)?;
            let mut claim: ResumeClaim = store::read_json(&claim_path(&home, &id))?;
            expire_confirmation(&home, &mut claim)?;
            let publication = claim
                .newpty
                .clone()
                .zip(claim.generation)
                .map(|(pty, generation)| (pty, generation, claim.phase));
            drop(_guard);
            if let Some((pty, generation, phase)) = publication {
                crate::agent_hooks::state::set_restore_state(&pty, generation, phase);
            }
            Ok::<(), HookErrorDto>(())
        })
        .await;
    });
    Ok(answer)
}

pub(crate) fn gc_safe(
    home: &Path,
    descriptor: &str,
    live_ptys: &std::collections::HashSet<String>,
) -> Result<bool, HookErrorDto> {
    let path = claim_path(home, descriptor);
    if path.exists() {
        let claim: ResumeClaim = store::read_json(&path)?;
        if claim.newpty.is_none()
            || claim
                .newpty
                .as_ref()
                .is_some_and(|pty| live_ptys.contains(pty))
        {
            return Ok(false);
        }
    }
    for entry in fs::read_dir(store::root(home))
        .map_err(|_| error(HookErrorCode::InternalError, "cannot inspect restore plans"))?
        .flatten()
    {
        if !entry.file_name().to_string_lossy().starts_with("plan-") {
            continue;
        }
        let plan: DurablePlan = store::read_json(&entry.path())?;
        if plan.dto.descriptor_id == descriptor && !plan.consumed && plan.dto.expires_at > now_ms()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn guard_inflight_hook(
    home: &Path,
    owner: &str,
    state: &HookStateDto,
) -> Result<(), HookErrorDto> {
    let Ok(entries) = fs::read_dir(store::root(home)) else {
        return Ok(());
    };
    for entry in entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".claim.json"))
    {
        let Ok(claim) = store::read_json::<ResumeClaim>(&entry.path()) else {
            continue;
        };
        if claim.phase == RestoreState::PendingConfirmation
            && claim.newpty.is_none()
            && claim.owner_session_id == owner
            && claim.owner_instance == state.instance_id
        {
            let identity = store::load(home, &claim.descriptor_id)?;
            if identity.adapter_install_id == state.adapter_install_id {
                return Err(error(
                    HookErrorCode::Conflict,
                    "restore spawn identity is not yet published",
                ));
            }
        }
        if claim.newpty.as_deref() == Some(&state.terminal_id) && claim.phase == RestoreState::Stale
        {
            return Err(error(
                HookErrorCode::IdentityStale,
                "resumed identity is stale",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VerifiedStart {
    owner: String,
    state: HookStateDto,
}
fn start_path(home: &Path, pty: &str) -> PathBuf {
    store::root(home).join(format!("{pty}.start.json"))
}

/// Receiver-only: the next start confirms the exact expected identity, never a renderer prediction.
pub fn confirm_verified_start(
    home: &Path,
    owner: &str,
    state: &HookStateDto,
) -> Result<Option<HookIdentityDto>, HookErrorDto> {
    store::validate_key(&state.terminal_id)?;
    {
        let _guard = store::lock(home, &format!("start-{}", state.terminal_id))?;
        store::atomic_json(
            &start_path(home, &state.terminal_id),
            &VerifiedStart {
                owner: owner.into(),
                state: state.clone(),
            },
        )?;
    }
    let Ok(entries) = fs::read_dir(store::root(home)) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().ends_with(".claim.json") {
            continue;
        }
        let Ok(initial) = store::read_json::<ResumeClaim>(&entry.path()) else {
            continue;
        };
        if initial.newpty.as_deref() != Some(&state.terminal_id) {
            continue;
        }
        let _lock = store::lock(home, &initial.descriptor_id)?;
        let mut claim: ResumeClaim = store::read_json(&entry.path())?;
        return confirm_claim_locked(home, owner, state, &mut claim);
    }
    Ok(None)
}

fn confirm_claim_locked(
    home: &Path,
    owner: &str,
    state: &HookStateDto,
    claim: &mut ResumeClaim,
) -> Result<Option<HookIdentityDto>, HookErrorDto> {
    if claim.phase == RestoreState::Stale {
        return Err(error(
            HookErrorCode::IdentityStale,
            "resumed identity is stale",
        ));
    }
    let mut identity = store::load(home, &claim.descriptor_id)?;
    if claim.phase == RestoreState::Confirmed {
        return Ok(None);
    }
    if !matches!(
        claim.phase,
        RestoreState::PendingConfirmation | RestoreState::Unconfirmed
    ) {
        return Ok(None);
    }
    let d = &identity.identity;
    let matched = claim.owner_session_id == owner
        && claim.owner_instance == state.instance_id
        && claim.generation == Some(state.generation)
        && identity.adapter_install_id == state.adapter_install_id
        && d.adapter_id == state.identity.adapter_id
        && d.agent_session_id == state.agent_session_id
        && (d.transcript_path.is_none() || d.transcript_path == state.identity.transcript_path);
    claim.phase = if matched {
        RestoreState::Confirmed
    } else {
        RestoreState::Stale
    };
    identity.restore_state = claim.phase;
    if matched {
        identity.terminal_id = state.terminal_id.clone();
        identity.generation = state.generation;
        identity.last_host_instance_id = state.instance_id.clone();
    }
    store::save(home, &identity)?;
    store::atomic_json(&claim_path(home, &claim.descriptor_id), claim)?;
    if !matched {
        return Err(error(
            HookErrorCode::IdentityStale,
            "resumed start identity mismatch",
        ));
    }
    // Claim generations are local to a PTY (new PTYs start at zero). A new
    // terminal-scoped descriptor prevents future restores colliding with this
    // descriptor's durable (descriptor, previous_generation) attempt.
    let key = format!(
        "{}:{}:{}",
        owner, state.adapter_install_id, state.terminal_id
    );
    let next_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes()).to_string();
    if next_id != identity.descriptor_id {
        let _next_lock = store::lock(home, &next_id)?;
        identity.descriptor_id = next_id.clone();
        identity.process_ids.clear();
        identity.identity.descriptor_id = Some(next_id);
        identity.identity.cwd = state.identity.cwd.clone().or(identity.identity.cwd);
        identity.identity.transcript_path = state
            .identity
            .transcript_path
            .clone()
            .or(identity.identity.transcript_path);
        identity.updated_at = now_ms();
        store::save(home, &identity)?;
    }
    Ok(Some(identity.identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_plan_is_snapshot_only_generation_bound_and_expires_after_sixty_seconds() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let mut state = crate::cli_session::restore_tests::state();
        state.instance_id = "host".into();
        state.adapter_install_id = uuid::Uuid::new_v4().to_string();
        state.identity.cwd = Some(home.to_string_lossy().into_owned());
        let sessions = home.join(".pi/agent/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("stamp_session-one.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        state.identity.transcript_path = Some(transcript.to_string_lossy().into_owned());
        let manifest: crate::agent_hooks::adapter::AdapterManifest = toml::from_str(include_str!(
            "../../resources/agent-hook-adapters/pi/adapter.toml"
        ))
        .unwrap();
        let snapshot = serde_json::json!({"schema_version":3,"adapter_id":"pi","install_id":state.adapter_install_id,"revision":1,"adapter_version":"1.0.0","origin":"builtin","source_digest":"test","manifest":manifest,"effective_events":[],"probe_version":null,"targets":[]});
        let installed = home.join(".se-manager/agent-hooks/installed/pi");
        fs::create_dir_all(installed.join("snapshots")).unwrap();
        store::atomic_json(&installed.join("receipt.json"), &snapshot).unwrap();
        store::atomic_json(
            &installed
                .join("snapshots")
                .join(format!("{}-1.json", state.adapter_install_id)),
            &snapshot,
        )
        .unwrap();
        let identity = store::upsert_verified_identity(
            &home,
            "owner",
            &state,
            &format!("{}:1", state.adapter_install_id),
            "default",
        )
        .unwrap();
        let id = identity.descriptor_id.unwrap();
        let handle = TerminalServiceHandle::in_process(se_pty::test_pty_manager());
        let context = RestoreContext {
            home: &home,
            host_instance_id: "host",
            project_id: "project-one",
            owner_session_id: "owner",
            automatic: true,
            auto_resume_enabled: true,
            terminal_service: &handle,
        };
        let request = ResumePlanRequest {
            descriptor_id: id.clone(),
            project_id: "project-one".into(),
            expected_generation: 1,
        };
        // No PTY in the registry is not enough: an orphaned agent may survive.
        assert_eq!(
            resume_plan(&context, request.clone())
                .await
                .unwrap_err()
                .code,
            HookErrorCode::IdentityStale
        );
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            struct ChildGuard(std::process::Child);
            impl Drop for ChildGuard {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            let mut child = ChildGuard(
                std::process::Command::new("/bin/sleep")
                    .arg0("pi")
                    .arg("30")
                    .spawn()
                    .unwrap(),
            );
            capture_process_evidence(&home, &id, child.0.id()).unwrap();
            assert_eq!(
                store::load(&home, &id).unwrap().process_ids,
                vec![child.0.id()]
            );
            assert_eq!(
                resume_plan(&context, request.clone())
                    .await
                    .unwrap_err()
                    .code,
                HookErrorCode::Conflict
            );
            let restarted = RestoreContext {
                home: &home,
                host_instance_id: "after-restart",
                project_id: "project-one",
                owner_session_id: "owner",
                automatic: true,
                auto_resume_enabled: true,
                terminal_service: &handle,
            };
            assert_eq!(
                resume_plan(&restarted, request.clone())
                    .await
                    .unwrap_err()
                    .code,
                HookErrorCode::Conflict
            );
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            assert!(resume_plan(&restarted, request.clone()).await.is_ok());
        }
        let before = now_ms();
        let dto = resume_plan(&context, request.clone()).await.unwrap();
        assert!(dto.expires_at >= before + 60_000 && dto.expires_at <= now_ms() + 60_000);
        assert_eq!(
            dto.display_argv,
            vec![
                "pi".to_owned(),
                "--session".to_owned(),
                transcript.to_string_lossy().into_owned()
            ]
        );
        let mut plan: DurablePlan = store::read_json(&plan_path(&home, &dto.plan_id)).unwrap();
        plan.dto.expires_at = now_ms();
        store::atomic_json(&plan_path(&home, &dto.plan_id), &plan).unwrap();
        let err = resume_spawn(
            &context,
            ResumeSpawnRequest {
                plan_id: dto.plan_id,
                descriptor_id: id,
                expected_generation: 1,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, HookErrorCode::PlanExpired);
        let mut forbidden = context;
        forbidden.auto_resume_enabled = false;
        assert_eq!(
            resume_plan(&forbidden, request).await.unwrap_err().code,
            HookErrorCode::Forbidden
        );
    }
    fn claim(id: &str) -> ResumeClaim {
        ResumeClaim {
            descriptor_id: id.into(),
            previous_generation: 7,
            owner_instance: "host".into(),
            owner_pid: std::process::id(),
            owner_session_id: "owner".into(),
            attempt_id: "attempt".into(),
            newpty: Some("pty-two".into()),
            generation: Some(1),
            phase: RestoreState::PendingConfirmation,
            started_at: now_ms() - 10_001,
            resume_grant: Some("private-manager-grant".into()),
        }
    }
    #[test]
    fn ten_second_timeout_preserves_terminal_without_retry_or_kill() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let state = crate::cli_session::restore_tests::state();
        let identity =
            store::upsert_verified_identity(&home, "owner", &state, "snapshot", "default").unwrap();
        let id = identity.descriptor_id.unwrap();
        let _guard = store::lock(&home, &id).unwrap();
        let mut c = claim(&id);
        expire_confirmation(&home, &mut c).unwrap();
        let answer = result(&c).unwrap();
        assert_eq!(answer.pty_id, "pty-two");
        assert_eq!(answer.restore_state, RestoreState::Unconfirmed);
        assert!(!answer.pending_confirmation);
        assert_eq!(c.resume_grant.as_deref(), Some("private-manager-grant"));
        assert!(!serde_json::to_string(&answer)
            .unwrap()
            .contains("private-manager-grant"));
    }
    #[test]
    fn late_matching_start_confirms_but_mismatch_is_stale() {
        for matched in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().canonicalize().unwrap();
            let mut state = crate::cli_session::restore_tests::state();
            state.instance_id = "host".into();
            let identity =
                store::upsert_verified_identity(&home, "owner", &state, "snapshot", "default")
                    .unwrap();
            let id = identity.descriptor_id.unwrap();
            let mut c = claim(&id);
            c.phase = RestoreState::Unconfirmed;
            store::atomic_json(&claim_path(&home, &id), &c).unwrap();
            state.terminal_id = "pty-two".into();
            if !matched {
                state.agent_session_id = "wrong-session".into();
                state.identity.agent_session_id = "wrong-session".into();
            }
            assert_eq!(
                confirm_verified_start(&home, "owner", &state).is_ok(),
                matched
            );
            let saved: ResumeClaim = store::read_json(&claim_path(&home, &id)).unwrap();
            assert_eq!(
                saved.phase,
                if matched {
                    RestoreState::Confirmed
                } else {
                    RestoreState::Stale
                }
            );
            assert_eq!(saved.newpty.as_deref(), Some("pty-two"));
        }
    }
}
