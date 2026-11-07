//! Adapters use the existing profile repositories. No duplicate configuration
//! database and no credential values in tool arguments or metadata responses.
use std::sync::Arc;

use crate::{
    credentials::CredentialBackend,
    ssh::{
        automation::{
            AutomationError, AutomationResult, CredentialResolver, Credentials, ProfileSource,
        },
        profile_manager::SSHProfile,
        SSHManager,
    },
    web::store::WebStore,
};

/// The durable job store validates file ownership itself. Create only our new
/// private child directory, never chmod an existing shared host state root.
pub fn private_job_store_path(root: &std::path::Path) -> AutomationResult<std::path::PathBuf> {
    let directory = root.join("mcp-ssh-jobs");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Err(error) = builder.create(&directory) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(AutomationError::new(
                "JOB_STORE_UNAVAILABLE",
                "Private job directory is unavailable",
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| {
        AutomationError::new(
            "JOB_STORE_UNAVAILABLE",
            "Private job directory is unavailable",
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(AutomationError::new(
            "JOB_STORE_UNAVAILABLE",
            "Private job directory is invalid",
        ));
    }
    Ok(directory.join("jobs.json"))
}

/// Unix hosts require durable jobs. Other hosts retain directory/SSH access,
/// while HostCapabilityService explicitly rejects their unsupported job tools.
pub fn production_host_ssh(
    profiles: Arc<dyn ProfileSource>,
    credentials: Arc<dyn CredentialResolver>,
    root: &std::path::Path,
) -> AutomationResult<crate::ssh::automation::SshAutomationService> {
    #[cfg(unix)]
    {
        let path = private_job_store_path(root)?;
        crate::ssh::automation::SshAutomationService::production_with_job_store(
            profiles,
            credentials,
            path,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        log::warn!(target: "se_manager::mcp_core::host", "operation=host_jobs_init stable_code=UNSUPPORTED_PLATFORM");
        Ok(crate::ssh::automation::SshAutomationService::production(
            profiles,
            credentials,
        ))
    }
}

pub struct DesktopProfiles(pub Arc<SSHManager>);
impl ProfileSource for DesktopProfiles {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        self.0.profiles.list().map_err(|_| {
            AutomationError::new("PROFILES_UNAVAILABLE", "SSH profiles cannot be read")
        })
    }
}

pub struct ServerProfiles(pub Arc<WebStore>);
impl ProfileSource for ServerProfiles {
    fn list_profiles(&self) -> AutomationResult<Vec<SSHProfile>> {
        let value = self
            .0
            .read("ssh/profiles")
            .map_err(|_| {
                AutomationError::new("PROFILES_UNAVAILABLE", "SSH profiles cannot be read")
            })?
            .unwrap_or_else(|| serde_json::json!([]));
        let mut profiles: Vec<SSHProfile> = serde_json::from_value(value)
            .map_err(|_| AutomationError::new("PROFILES_INVALID", "SSH profiles are invalid"))?;
        // Web persistence is never an inline credential source.
        for profile in &mut profiles {
            profile.password = None;
            profile.passphrase = None;
        }
        Ok(profiles)
    }
}

/// Capture backend and service names on the composition thread. Tests can
/// inject a temporary backend without a blocking worker resolving global state.
pub struct HostCredentials {
    backend: Option<Arc<dyn CredentialBackend>>,
    service: String,
    legacy_service: String,
}
impl HostCredentials {
    pub fn new(
        backend: Option<Arc<dyn CredentialBackend>>,
        service: String,
        legacy_service: String,
    ) -> Self {
        Self {
            backend,
            service,
            legacy_service,
        }
    }
}
impl CredentialResolver for HostCredentials {
    fn resolve(&self, profile: &SSHProfile) -> AutomationResult<Credentials> {
        let suffix = match profile.auth_method.as_str() {
            "password" => "password",
            "key" if profile.has_stored_passphrase => "passphrase",
            _ => return Ok(Credentials::default()),
        };
        let missing = || {
            AutomationError::new(
                "CREDENTIALS_UNAVAILABLE",
                "Credentials are not configured on this host",
            )
        };
        let backend = self.backend.as_ref().ok_or_else(missing)?;
        let key = format!("{}-{suffix}", profile.id);
        let mut secret = backend.get(&self.service, &key).map_err(|_| missing())?;
        if secret.is_none() && self.legacy_service != self.service {
            secret = backend
                .get(&self.legacy_service, &key)
                .map_err(|_| missing())?;
        }
        if secret.is_none() {
            return Err(missing());
        }
        Ok(Credentials { secret })
    }
}
