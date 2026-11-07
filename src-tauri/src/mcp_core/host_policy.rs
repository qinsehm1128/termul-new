//! Host-owned explicit grants. MCP callers can use these grants but cannot
//! change them. A single host-global principal is intentional, not multi-tenant.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub use crate::ssh::automation::profile_fingerprint;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{host_capabilities::HostCapabilityError, workspace::WorkspaceGrant};
use crate::ssh::{
    automation::{
        AutomationError, AutomationResult, GrantProvider, ProfileSource, SshGrantSnapshot,
        SshPolicy, OPERATIONS,
    },
    command_policy::CompiledCommandPolicy,
    profile_manager::SSHProfile,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAccessPolicy {
    pub schema_version: u32,
    pub revision: u64,
    pub enabled: bool,
    pub workspace: WorkspaceGrant,
    pub ssh: SshPolicy,
    #[serde(default)]
    pub profile_fingerprints: BTreeMap<String, String>,
}

impl Default for HostAccessPolicy {
    fn default() -> Self {
        Self {
            schema_version: 2,
            revision: 0,
            enabled: false,
            workspace: WorkspaceGrant::default(),
            ssh: SshPolicy::default(),
            profile_fingerprints: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct HostPolicySnapshot {
    pub policy: HostAccessPolicy,
    pub commands: Arc<CompiledCommandPolicy>,
}
pub struct HostPolicyStore {
    path: PathBuf,
    policy: Mutex<HostPolicySnapshot>,
}

impl HostPolicyStore {
    pub fn open(path: PathBuf) -> Result<Self, HostCapabilityError> {
        let mut policy: HostAccessPolicy = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                serde_json::from_slice(&bytes).map_err(|_| {
                    HostCapabilityError::new("POLICY_INVALID", "Host access policy is invalid")
                })?
            }
            Ok(_) => {
                return Err(HostCapabilityError::new(
                    "POLICY_INVALID",
                    "Host access policy is too large",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HostAccessPolicy::default(),
            Err(_) => {
                return Err(HostCapabilityError::new(
                    "POLICY_UNAVAILABLE",
                    "Host access policy cannot be read",
                ))
            }
        };
        let commands = Arc::new(compile(&policy)?);
        policy.schema_version = 2;
        Ok(Self {
            path,
            policy: Mutex::new(HostPolicySnapshot { policy, commands }),
        })
    }

    pub fn snapshot(&self) -> HostAccessPolicy {
        self.policy.lock().policy.clone()
    }
    pub fn compiled_snapshot(&self) -> HostPolicySnapshot {
        self.policy.lock().clone()
    }

    pub fn put(
        &self,
        expected_revision: u64,
        mut next: HostAccessPolicy,
        profiles: &[SSHProfile],
    ) -> Result<HostAccessPolicy, HostCapabilityError> {
        let commands = Arc::new(compile(&next)?);
        next.schema_version = 2;
        let mut current = self.policy.lock();
        if current.policy.revision != expected_revision {
            return Err(HostCapabilityError::new(
                "REVISION_CONFLICT",
                "Host access policy changed; reload before saving",
            ));
        }
        for id in next.ssh.profiles.keys() {
            if !next.enabled {
                // Disabling is always permitted, even after a profile was
                // removed or changed. Re-enabling still requires fresh consent.
                continue;
            }
            let profile = profiles.iter().find(|p| &p.id == id).ok_or_else(|| {
                HostCapabilityError::new("PROFILE_NOT_FOUND", "SSH profile no longer exists")
            })?;
            if next.profile_fingerprints.get(id) != Some(&profile_fingerprint(profile)) {
                return Err(HostCapabilityError::new(
                    "PROFILE_CHANGED",
                    "SSH profile identity changed; review and authorize again",
                ));
            }
        }
        next.profile_fingerprints
            .retain(|id, _| next.ssh.profiles.contains_key(id));
        next.revision = current.policy.revision.checked_add(1).ok_or_else(|| {
            HostCapabilityError::new("POLICY_INVALID", "Policy revision exhausted")
        })?;
        persist_private(&self.path, &next)?;
        *current = HostPolicySnapshot {
            policy: next.clone(),
            commands,
        };
        log::info!(target:"se_manager::mcp_core::host", "operation=host_access_put revision={} stable_code=UPDATED", next.revision);
        Ok(next)
    }
}

pub fn compile(policy: &HostAccessPolicy) -> Result<CompiledCommandPolicy, HostCapabilityError> {
    validate(policy)?;
    CompiledCommandPolicy::compile(&policy.ssh)
        .map_err(|_| HostCapabilityError::new("POLICY_INVALID", "Invalid host command policy"))
}
fn validate(policy: &HostAccessPolicy) -> Result<(), HostCapabilityError> {
    let invalid = || HostCapabilityError::new("POLICY_INVALID", "Invalid host access policy");
    if ![1, 2].contains(&policy.schema_version) || policy.ssh.profiles.len() > 256 {
        return Err(invalid());
    }
    for grant in policy.ssh.profiles.values() {
        if grant
            .operations
            .iter()
            .any(|op| !OPERATIONS.contains(&op.as_str()))
            || grant.remote_roots.len() > 32
            || grant.local_roots.len() > 32
        {
            return Err(invalid());
        }
        if grant.local_roots.iter().any(|path| !path.is_absolute())
            || grant.remote_roots.iter().any(|path| {
                !path.starts_with('/')
                    || path.contains('\0')
                    || path.split('/').any(|part| part == "..")
            })
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn persist_private(path: &Path, policy: &HostAccessPolicy) -> Result<(), HostCapabilityError> {
    let fail = || {
        HostCapabilityError::new(
            "POLICY_WRITE_FAILED",
            "Host access policy could not be saved",
        )
    };
    let parent = path.parent().ok_or_else(fail)?;
    std::fs::create_dir_all(parent).map_err(|_| fail())?;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(fail());
    }
    let temp = parent.join(format!(".mcp-policy-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(policy)?)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|_| fail())
}

pub struct HostGrants {
    pub store: Arc<HostPolicyStore>,
    pub profiles: Arc<dyn ProfileSource>,
    pub caller_id: String,
}

impl GrantProvider for HostGrants {
    fn policy(&self, caller_id: &str) -> AutomationResult<SshPolicy> {
        Ok(self.snapshot(caller_id)?.policy)
    }
    fn snapshot(&self, caller_id: &str) -> AutomationResult<SshGrantSnapshot> {
        let snapshot = self.store.compiled_snapshot();
        let policy = snapshot.policy;
        if !policy.enabled || caller_id != self.caller_id {
            return Err(AutomationError::denied());
        }
        let profiles = self.profiles.list_profiles()?;
        let mut ssh = policy.ssh;
        // Identity changes revoke the grant even if the stable id is unchanged.
        ssh.profiles.retain(|id, _| {
            profiles.iter().any(|profile| {
                &profile.id == id
                    && policy.profile_fingerprints.get(id) == Some(&profile_fingerprint(profile))
            })
        });
        // The host-managed fingerprint map is authoritative. Never trust an
        // embedded fingerprint supplied by a management client.
        for (id, grant) in &mut ssh.profiles {
            grant.profile_fingerprint = policy.profile_fingerprints.get(id).cloned();
        }
        Ok(SshGrantSnapshot {
            policy: ssh,
            commands: snapshot.commands,
            revision: policy.revision,
        })
    }
}
